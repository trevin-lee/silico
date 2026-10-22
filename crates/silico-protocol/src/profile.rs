//! Flat profiles, and the rules for sweeping one into a solid by an extrusion or a revolve
//! (docs/ports/extrude-revolve.md). The rules live here, with the types, so the core can
//! refuse values before sending them and a worker can refuse them again before OCCT sees
//! them. They're plain arithmetic, so they give the same answer on every platform.

use serde::{Deserialize, Serialize};

use crate::primitive::{InvalidParameter, LENGTH_TOLERANCE, MAX_LENGTH, format_number};

/// The most points a profile may have. Checking that a profile doesn't cross itself costs
/// the square of its points.
pub const MAX_PROFILE_POINTS: usize = 1000;

/// The smallest |direction · normal| an extrusion may have: FreeCAD's limit
/// (`Precision::Confusion()`), below which the direction lies in the profile's plane.
const MIN_DIRECTION_FACTOR: f64 = 1e-7;

/// How far an axis's direction may lean out of a profile's plane (as the cosine of its
/// angle to the plane's normal) before it's refused outright. Below this, the axis is in
/// the plane when it's within OCCT's tolerance of it wherever it passes a corner (a
/// distance, as flatness is); this only catches axes nearly perpendicular to the plane,
/// which pass every corner close to where it meets the plane.
const MAX_AXIS_LEAN: f64 = 0.5;

/// A closed, flat polygon: its corners in order, the last joined back to the first. Seen
/// from the tip of its normal, the corners go counterclockwise (the right-hand rule).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub points: Vec<[f64; 3]>,
}

/// A profile's plane: a point on it (the first corner) and its unit normal, which follows
/// the order of the corners.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProfilePlane {
    pub origin: [f64; 3],
    pub normal: [f64; 3],
}

/// Degrees to radians as FreeCAD converts them (`Base::toRadians`), so OCCT gets the same
/// angles bit for bit. The rules and the kernel both use it, so they agree.
pub fn radians(degrees: f64) -> f64 {
    degrees * (std::f64::consts::PI / 180.0)
}

/// A line to revolve about: a point on it and its direction (any length but zero).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Axis {
    pub origin: [f64; 3],
    pub direction: [f64; 3],
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

/// The length of `a`, scaled by its largest component first so it can't overflow (1e300
/// squared is infinite) or underflow.
fn norm(a: [f64; 3]) -> f64 {
    let largest = a.iter().fold(0.0f64, |m, c| m.max(c.abs()));
    if largest == 0.0 || !largest.is_finite() {
        return largest;
    }
    let s = scale(a, 1.0 / largest);
    largest * dot(s, s).sqrt()
}

/// The distance from `p` to the segment `a`–`b`: to the point on it nearest `p`, found by
/// projecting onto the segment and clamping to its ends.
fn point_segment_distance(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> f64 {
    let (u, w) = (sub(b, a), sub(p, a));
    let uu = dot(u, u);
    let t = if uu > 0.0 {
        (dot(w, u) / uu).clamp(0.0, 1.0)
    } else {
        0.0
    };
    norm(sub(w, scale(u, t)))
}

/// Shewchuk's first-stage error bound for an orientation, (3 + 16ε)ε with ε = 2⁻⁵³
/// ("Adaptive Precision Floating-Point Arithmetic and Fast Robust Geometric Predicates",
/// 1997, `ccwerrboundA`): an orientation above it times the sum of its two products'
/// magnitudes has the sign the exact one has.
const ORIENTATION_BOUND: f64 = (3.0 + 8.0 * f64::EPSILON) * (f64::EPSILON / 2.0);

/// Which side of the line from `a` to `b` `c` is on: 1 left, −1 right, or 0 on it or too
/// near it for rounding to tell (within about 1e-9 mm of it within reach, and closer
/// nearer the origin), by [`ORIENTATION_BOUND`]. Profiles and sketch profiles both decide
/// crossings by it.
pub fn side_of_line(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> i8 {
    let (left, right) = ((b[0] - a[0]) * (c[1] - a[1]), (b[1] - a[1]) * (c[0] - a[0]));
    let orientation = left - right;
    if orientation.abs() <= ORIENTATION_BOUND * (left.abs() + right.abs()) {
        0
    } else if orientation > 0.0 {
        1
    } else {
        -1
    }
}

fn invalid(parameter: &'static str, message: String) -> InvalidParameter {
    InvalidParameter { parameter, message }
}

/// Whether a fillet's radius can round an edge: a length above OCCT's tolerance and at
/// most a kilometre, as a chamfer's distance must be. (OCCT and FreeCAD build a fillet of
/// 1e-8, which leaves nothing OCCT can tell from the edge.)
pub fn check_fillet_radius(radius: f64) -> Result<(), InvalidParameter> {
    check_length("radius", "fillet's radius", radius)
}

/// A length's check: finite, above OCCT's tolerance, at most [`MAX_LENGTH`].
pub(crate) fn check_length(
    parameter: &'static str,
    what: &str,
    value: f64,
) -> Result<(), InvalidParameter> {
    if value > LENGTH_TOLERANCE && value <= MAX_LENGTH {
        Ok(())
    } else {
        Err(invalid(
            parameter,
            format!(
                "the {what} must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got {}",
                format_number(value)
            ),
        ))
    }
}

/// A vector's check: every component finite, and longer than OCCT's tolerance.
fn check_direction(
    parameter: &'static str,
    what: &str,
    v: [f64; 3],
) -> Result<f64, InvalidParameter> {
    if !v.iter().all(|c| c.is_finite()) {
        return Err(invalid(
            parameter,
            format!("the {what} isn't a finite vector: {v:?}"),
        ));
    }
    let length = norm(v);
    // Finite: every component is.
    if length <= LENGTH_TOLERANCE {
        return Err(invalid(
            parameter,
            format!(
                "the {what} has no direction: its length is {}",
                format_number(length)
            ),
        ));
    }
    Ok(length)
}

impl Profile {
    /// The corner (numbered from 0) whose side to the next corner is the shortest: where
    /// two corners are nearest to being one.
    pub fn shortest_side(&self) -> usize {
        let n = self.points.len();
        (0..n)
            .min_by(|&a, &b| {
                let side = |i: usize| norm(sub(self.points[(i + 1) % n], self.points[i]));
                side(a).total_cmp(&side(b))
            })
            .unwrap_or(0)
    }

    /// Whether the corners make a profile: three to [`MAX_PROFILE_POINTS`] of them, every
    /// coordinate finite and within [`MAX_LENGTH`] of the origin, consecutive corners
    /// further apart than [`LENGTH_TOLERANCE`], no side touching another except where they
    /// meet, enclosing an area wider than the tolerance, and all within it of one plane.
    /// Returns the profile's plane.
    pub fn check(&self) -> Result<ProfilePlane, InvalidParameter> {
        let points = &self.points;
        let n = points.len();
        if !(3..=MAX_PROFILE_POINTS).contains(&n) {
            return Err(invalid(
                "points",
                format!("a profile needs 3 to {MAX_PROFILE_POINTS} corners, got {n}"),
            ));
        }
        for (i, p) in points.iter().enumerate() {
            if !p.iter().all(|c| c.is_finite() && c.abs() <= MAX_LENGTH) {
                return Err(invalid(
                    "points",
                    format!(
                        "corner {} must have finite coordinates within 1e6 mm (a kilometre) of the origin, got {p:?}",
                        i + 1
                    ),
                ));
            }
        }
        let next = |i: usize| points[(i + 1) % n];
        for (i, &p) in points.iter().enumerate() {
            if norm(sub(next(i), p)) <= LENGTH_TOLERANCE {
                return Err(invalid(
                    "points",
                    format!(
                        "corners {} and {} are the same point: consecutive corners must be more than 1e-7 mm apart",
                        i + 1,
                        (i + 1) % n + 1
                    ),
                ));
            }
        }
        // The plane through the corners furthest apart: the first, the one furthest from it,
        // and the one furthest from the line through those two.
        let origin = points[0];
        let far = points
            .iter()
            .copied()
            .max_by(|a, b| norm(sub(*a, origin)).total_cmp(&norm(sub(*b, origin))))
            .unwrap_or(origin);
        let along = scale(sub(far, origin), 1.0 / norm(sub(far, origin)));
        let from_line = |p: [f64; 3]| norm(cross(sub(p, origin), along));
        let third = points
            .iter()
            .copied()
            .max_by(|a, b| from_line(*a).total_cmp(&from_line(*b)))
            .unwrap_or(origin);
        if from_line(third) <= LENGTH_TOLERANCE {
            return Err(invalid(
                "points",
                "the profile encloses no area: its corners are in a line".to_owned(),
            ));
        }
        let across = cross(along, sub(third, origin));
        let normal_guess = scale(across, 1.0 / norm(across));
        // Far off that plane is plainly not flat (said now, so it isn't taken for a
        // crossing below); within it, flatness is judged exactly against the average
        // plane, once the area gives one.
        let not_flat = |off: f64| {
            invalid(
                "points",
                format!(
                    "the profile isn't flat: its corners are up to {} mm off one plane, more than 1e-7 mm",
                    format_number(off)
                ),
            )
        };
        let off = points
            .iter()
            .map(|&p| dot(sub(p, origin), normal_guess).abs())
            .fold(0.0f64, f64::max);
        if off > 10.0 * LENGTH_TOLERANCE {
            return Err(not_flat(off));
        }
        // Each corner in the plane's own coordinates, for the crossing test.
        let side_way = cross(normal_guess, along);
        let flat: Vec<[f64; 2]> = points
            .iter()
            .map(|&p| [dot(sub(p, origin), along), dot(sub(p, origin), side_way)])
            .collect();
        // No side may touch another, except a side's neighbours at the corner they share,
        // and those may not fold back along it. Two sides touch where a corner of one is
        // within the tolerance of the other, and cross where each one's ends are surely on
        // opposite sides of the other's line (a test that holds at any angle, where a
        // distance between the lines would cancel to nothing at a shallow one). Only sides
        // rounding can't have chosen count: two sides on one line, apart along it, have
        // their ends on it by rounding only (found by M3.5's review). Two that cross with
        // an end too near the other's line to place have met already, within the tolerance:
        // the distances below find them.
        for i in 0..n {
            let (a, b) = (points[i], next(i));
            for (j, &c) in points.iter().enumerate().skip(i + 1) {
                let d = next(j);
                let touches = if j == i + 1 {
                    // b is c: d mustn't come back onto a–b, nor a onto c–d.
                    point_segment_distance(d, a, b) <= LENGTH_TOLERANCE
                        || point_segment_distance(a, c, d) <= LENGTH_TOLERANCE
                } else if i == 0 && j == n - 1 {
                    // d is a.
                    point_segment_distance(c, a, b) <= LENGTH_TOLERANCE
                        || point_segment_distance(b, c, d) <= LENGTH_TOLERANCE
                } else {
                    let (fa, fb, fc, fd) = (flat[i], flat[(i + 1) % n], flat[j], flat[(j + 1) % n]);
                    let crosses = side_of_line(fa, fb, fc) * side_of_line(fa, fb, fd) < 0
                        && side_of_line(fc, fd, fa) * side_of_line(fc, fd, fb) < 0;
                    crosses
                        || point_segment_distance(c, a, b) <= LENGTH_TOLERANCE
                        || point_segment_distance(d, a, b) <= LENGTH_TOLERANCE
                        || point_segment_distance(a, c, d) <= LENGTH_TOLERANCE
                        || point_segment_distance(b, c, d) <= LENGTH_TOLERANCE
                };
                if touches {
                    return Err(invalid(
                        "points",
                        format!(
                            "the profile crosses or touches itself: side {} (corners {} to {}) meets side {} (corners {} to {})",
                            i + 1,
                            i + 1,
                            (i + 1) % n + 1,
                            j + 1,
                            j + 1,
                            (j + 1) % n + 1
                        ),
                    ));
                }
            }
        }
        // Newell's normal, about the first corner: twice the enclosed area, along the
        // normal the corners' order gives.
        let mut twice_area = [0.0; 3];
        for (i, &p) in points.iter().enumerate() {
            let c = cross(sub(p, origin), sub(next(i), origin));
            twice_area = [
                twice_area[0] + c[0],
                twice_area[1] + c[1],
                twice_area[2] + c[2],
            ];
        }
        let perimeter: f64 = (0..n).map(|i| norm(sub(next(i), points[i]))).sum();
        let area = norm(twice_area) / 2.0;
        // Finite: every coordinate is.
        if area <= LENGTH_TOLERANCE * perimeter {
            return Err(invalid(
                "points",
                format!(
                    "the profile encloses no area to speak of ({} mm², against a perimeter of {} mm)",
                    format_number(area),
                    format_number(perimeter)
                ),
            ));
        }
        let normal = scale(twice_area, 1.0 / norm(twice_area));
        // Newell's plane through the first corner is an average of all the corners, so
        // no one corner is to blame: say how far off the furthest is.
        let off = points
            .iter()
            .map(|&p| dot(sub(p, origin), normal).abs())
            .fold(0.0f64, f64::max);
        if off > LENGTH_TOLERANCE {
            return Err(not_flat(off));
        }
        Ok(ProfilePlane { origin, normal })
    }
}

/// The vector an extrusion of the profile sweeps along: its normal times `length`, or,
/// along `direction`, as long as it needs to be to rise `length` along the normal (as
/// SolidWorks and FreeCAD's Pad measure). Refuses a length that isn't one, a direction
/// with none, a direction in the profile's plane, and a vector longer than a kilometre.
pub fn extrusion_vector(
    plane: &ProfilePlane,
    length: f64,
    direction: Option<[f64; 3]>,
) -> Result<[f64; 3], InvalidParameter> {
    check_length("length", "extrusion's length", length)?;
    let Some(direction) = direction else {
        return Ok(scale(plane.normal, length));
    };
    let unit = scale(
        direction,
        1.0 / check_direction("direction", "extrusion's direction", direction)?,
    );
    let factor = dot(unit, plane.normal).abs();
    if factor < MIN_DIRECTION_FACTOR {
        return Err(invalid(
            "direction",
            "the extrusion's direction lies in the profile's plane: it must point out of it"
                .to_owned(),
        ));
    }
    let vector = scale(unit, length / factor);
    if norm(vector) > MAX_LENGTH {
        return Err(invalid(
            "length",
            format!(
                "the extrusion would sweep {} mm along its direction to rise {} mm, more than a kilometre: shorten it, or point the direction nearer the profile's normal",
                format_number(norm(vector)),
                format_number(length)
            ),
        ));
    }
    Ok(vector)
}

/// Whether the profile can be revolved about `axis` by `angle` degrees: the axis must lie
/// in the profile's plane (within OCCT's tolerance wherever it passes a corner) with the
/// profile on one side of it (touching it at most), and the angle must be more than 0 and
/// at most 360, and move the profile further than OCCT's tolerance. A turn a hair short of
/// 360 is the turn it says, leaving a slit, as FreeCAD's is. Returns the axis's unit
/// direction.
pub fn check_revolve(
    plane: &ProfilePlane,
    points: &[[f64; 3]],
    axis: &Axis,
    angle: f64,
) -> Result<[f64; 3], InvalidParameter> {
    if !axis
        .origin
        .iter()
        .all(|c| c.is_finite() && c.abs() <= MAX_LENGTH)
    {
        return Err(invalid(
            "axis",
            format!(
                "the axis's point must have finite coordinates within 1e6 mm (a kilometre) of the origin, got {:?}",
                axis.origin
            ),
        ));
    }
    let unit = scale(
        axis.direction,
        1.0 / check_direction("axis", "axis's direction", axis.direction)?,
    );
    // In the plane: not leaning out of it, and, where it passes each corner, no further
    // from it than the tolerance (a lean too small to see at the axis's point can still
    // carry it off the plane a kilometre away).
    let lean = dot(unit, plane.normal).abs();
    let off_at = |p: [f64; 3]| {
        let nearest = [
            axis.origin[0] + unit[0] * dot(sub(p, axis.origin), unit),
            axis.origin[1] + unit[1] * dot(sub(p, axis.origin), unit),
            axis.origin[2] + unit[2] * dot(sub(p, axis.origin), unit),
        ];
        dot(sub(nearest, plane.origin), plane.normal).abs()
    };
    let off = points
        .iter()
        .map(|&p| off_at(p))
        .fold(off_at(axis.origin), f64::max);
    if lean > MAX_AXIS_LEAN || off > LENGTH_TOLERANCE {
        return Err(invalid(
            "axis",
            "the axis must lie in the profile's plane: revolving about any other line can make a solid that cuts itself".to_owned(),
        ));
    }
    // Each corner's distance from the axis, signed by its side, within the plane.
    // Of length 1 at any lean the plane allows, so these are true distances from the axis.
    let across = cross(plane.normal, unit);
    let across = scale(across, 1.0 / norm(across));
    let sides: Vec<f64> = points
        .iter()
        .map(|&p| dot(sub(p, axis.origin), across))
        .collect();
    let (low, high) = sides
        .iter()
        .fold((0.0f64, 0.0f64), |(l, h), &s| (l.min(s), h.max(s)));
    if low < -LENGTH_TOLERANCE && high > LENGTH_TOLERANCE {
        return Err(invalid(
            "axis",
            "the axis crosses the profile: it may touch the profile's edge, but the profile must lie on one side of it".to_owned(),
        ));
    }
    if !(angle.is_finite() && angle > 0.0 && angle <= 360.0) {
        // FreeCAD takes -360 to 360 (b2da06bfe0); Silico turns one way, as SolidWorks
        // does, and the axis's direction says which.
        let other_way = if (-360.0..0.0).contains(&angle) {
            ": to turn the other way, reverse the axis's direction"
        } else {
            ""
        };
        return Err(invalid(
            "angle",
            format!(
                "the revolve's angle must be more than 0 and at most 360 degrees, got {}{other_way}",
                format_number(angle)
            ),
        ));
    }
    // OCCT refuses a turn of 1e-12 radians or less (Precision::Angular()), as FreeCAD's
    // PartDesign does first ("Angle of revolution too small").
    if radians(angle) <= 1e-12 {
        return Err(invalid(
            "angle",
            format!(
                "the revolve's angle of {} degrees is too small: it must be more than 1e-12 radians",
                format_number(angle)
            ),
        ));
    }
    // The farthest corner travels its distance times the angle in radians.
    let reach = low.abs().max(high.abs());
    if reach * radians(angle) <= LENGTH_TOLERANCE {
        return Err(invalid(
            "angle",
            format!(
                "the revolve's angle of {} degrees moves the profile less than 1e-7 mm",
                format_number(angle)
            ),
        ));
    }
    Ok(unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(size: f64) -> Profile {
        Profile {
            points: vec![
                [0.0, 0.0, 0.0],
                [size, 0.0, 0.0],
                [size, size, 0.0],
                [0.0, size, 0.0],
            ],
        }
    }

    #[test]
    fn a_square_counterclockwise_from_above_faces_up() {
        let plane = square(2.0).check().unwrap();
        assert_eq!(plane.normal, [0.0, 0.0, 1.0]);
        assert_eq!(plane.origin, [0.0, 0.0, 0.0]);
        let mut reversed = square(2.0);
        reversed.points.reverse();
        assert_eq!(reversed.check().unwrap().normal, [0.0, 0.0, -1.0]);
    }

    fn refused(
        result: Result<impl std::fmt::Debug, InvalidParameter>,
        parameter: &str,
        says: &str,
    ) {
        let err = result.unwrap_err();
        assert_eq!(err.parameter, parameter, "{}", err.message);
        assert!(err.message.contains(says), "{}", err.message);
    }

    #[test]
    fn a_profile_that_cant_bound_a_face_is_refused_saying_why() {
        let p = |points: Vec<[f64; 3]>| Profile { points }.check();
        let z = |x: f64, y: f64| [x, y, 0.0];
        refused(
            p(vec![z(0.0, 0.0), z(1.0, 0.0)]),
            "points",
            "3 to 1000 corners, got 2",
        );
        refused(
            p(vec![z(0.0, 0.0); MAX_PROFILE_POINTS + 1]),
            "points",
            "got 1001",
        );
        refused(
            p(vec![z(0.0, 0.0), z(f64::NAN, 0.0), z(0.0, 1.0)]),
            "points",
            "corner 2 must have finite",
        );
        refused(
            p(vec![z(0.0, 0.0), z(2e6, 0.0), z(0.0, 1.0)]),
            "points",
            "corner 2",
        );
        // The last corner is the first again, or two corners nearer than the tolerance.
        refused(
            p(vec![z(0.0, 0.0), z(1.0, 0.0), z(0.0, 1.0), z(0.0, 0.0)]),
            "points",
            "corners 4 and 1 are the same",
        );
        refused(
            p(vec![z(0.0, 0.0), z(1.0, 0.0), z(1.0, 5e-8), z(0.0, 1.0)]),
            "points",
            "corners 2 and 3 are the same",
        );
        refused(
            p(vec![z(0.0, 0.0), z(1.0, 0.0), z(2.0, 0.0)]),
            "points",
            "corners are in a line",
        );
        // 10 long and 1.5e-7 wide: its corners are far enough apart, but its area (1.5e-6)
        // is less than the tolerance times its perimeter (2e-6).
        refused(
            p(vec![
                z(0.0, 0.0),
                z(10.0, 0.0),
                z(10.0, 1.5e-7),
                z(0.0, 1.5e-7),
            ]),
            "points",
            "no area",
        );
        refused(
            p(vec![
                z(0.0, 0.0),
                z(1.0, 0.0),
                z(1.0, 1.0),
                [0.0, 1.0, 1e-6],
            ]),
            "points",
            "isn't flat: its corners are up to",
        );
        // A bow tie; a side folded back onto the one before; a corner on another side.
        refused(
            p(vec![z(0.0, 0.0), z(1.0, 1.0), z(1.0, 0.0), z(0.0, 1.0)]),
            "points",
            "crosses or touches itself",
        );
        refused(
            p(vec![z(0.0, 0.0), z(2.0, 0.0), z(1.0, 0.0), z(1.0, 1.0)]),
            "points",
            "crosses or touches itself",
        );
        refused(
            p(vec![
                z(0.0, 0.0),
                z(2.0, 0.0),
                z(2.0, 2.0),
                z(1.0, 0.0),
                z(0.0, 2.0),
            ]),
            "points",
            "crosses or touches itself",
        );
        // But a concave L, a triangle, and a profile far from the origin are fine.
        assert!(
            p(vec![
                z(0.0, 0.0),
                z(2.0, 0.0),
                z(2.0, 1.0),
                z(1.0, 1.0),
                z(1.0, 2.0),
                z(0.0, 2.0)
            ])
            .is_ok()
        );
        assert!(p(vec![z(0.0, 0.0), z(1.0, 0.0), z(0.0, 1.0)]).is_ok());
        assert!(
            p(vec![
                [9e5, 9e5, 9e5],
                [9e5 + 1.0, 9e5, 9e5],
                [9e5, 9e5 + 1.0, 9e5]
            ])
            .is_ok()
        );
    }

    #[test]
    fn an_extrusion_rises_its_length_along_the_normal() {
        let plane = square(1.0).check().unwrap();
        assert_eq!(
            extrusion_vector(&plane, 5.0, None).unwrap(),
            [0.0, 0.0, 5.0]
        );
        // At 45° it travels 5√2 to rise 5; pointing down, it goes down.
        let v = extrusion_vector(&plane, 5.0, Some([1.0, 0.0, 1.0])).unwrap();
        assert!(
            (v[0] - 5.0).abs() < 1e-12 && (v[2] - 5.0).abs() < 1e-12,
            "{v:?}"
        );
        assert_eq!(
            extrusion_vector(&plane, 5.0, Some([0.0, 0.0, -3.0])).unwrap(),
            [0.0, 0.0, -5.0]
        );
        refused(extrusion_vector(&plane, 0.0, None), "length", "got 0");
        refused(extrusion_vector(&plane, -1.0, None), "length", "got -1");
        refused(extrusion_vector(&plane, f64::NAN, None), "length", "NaN");
        refused(extrusion_vector(&plane, 2e6, None), "length", "a kilometre");
        refused(
            extrusion_vector(&plane, 1.0, Some([0.0; 3])),
            "direction",
            "no direction",
        );
        refused(
            extrusion_vector(&plane, 1.0, Some([f64::INFINITY, 0.0, 1.0])),
            "direction",
            "finite",
        );
        refused(
            extrusion_vector(&plane, 1.0, Some([1.0, 1.0, 0.0])),
            "direction",
            "lies in the profile's plane",
        );
        refused(
            extrusion_vector(&plane, 1.0, Some([1.0, 0.0, 2e-7])),
            "length",
            "more than a kilometre",
        );
        // At an ordinary slant too: it's the length that's too long.
        refused(
            extrusion_vector(&plane, 1e6, Some([1.0, 0.0, 1.0])),
            "length",
            "to rise 1000000 mm, more than a kilometre",
        );
    }

    #[test]
    fn a_revolve_needs_an_axis_in_the_plane_beside_the_profile() {
        // A 1 × 1 square from x = 1 to 2, in the XY plane.
        let profile = Profile {
            points: vec![
                [1.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [2.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
            ],
        };
        let plane = profile.check().unwrap();
        let axis = |origin: [f64; 3], direction: [f64; 3]| Axis { origin, direction };
        let y = axis([0.0; 3], [0.0, 2.0, 0.0]);
        assert_eq!(
            check_revolve(&plane, &profile.points, &y, 360.0).unwrap(),
            [0.0, 1.0, 0.0]
        );
        // Touching it along a side is fine; crossing it isn't.
        let touching = axis([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(check_revolve(&plane, &profile.points, &touching, 90.0).is_ok());
        let crossing = axis([1.5, 0.0, 0.0], [0.0, 1.0, 0.0]);
        refused(
            check_revolve(&plane, &profile.points, &crossing, 90.0),
            "axis",
            "crosses the profile",
        );
        // Out of the plane: perpendicular to it, tilted out of it, or beside it.
        for out in [
            axis([0.0; 3], [0.0, 0.0, 1.0]),
            axis([0.0; 3], [0.0, 1.0, 1e-6]),
            axis([0.0, 0.0, 1e-6], [0.0, 1.0, 0.0]),
        ] {
            refused(
                check_revolve(&plane, &profile.points, &out, 90.0),
                "axis",
                "must lie in the profile's plane",
            );
        }
        refused(
            check_revolve(&plane, &profile.points, &axis([0.0; 3], [0.0; 3]), 90.0),
            "axis",
            "no direction",
        );
        refused(
            check_revolve(
                &plane,
                &profile.points,
                &axis([f64::NAN, 0.0, 0.0], [0.0, 1.0, 0.0]),
                90.0,
            ),
            "axis",
            "finite",
        );
        for angle in [0.0, -90.0, 360.000_001, f64::NAN, f64::INFINITY] {
            refused(
                check_revolve(&plane, &profile.points, &y, angle),
                "angle",
                "more than 0 and at most 360",
            );
        }
        // 1e-6 degrees moves the far corner (2 from the axis) 3.5e-8: less than the
        // tolerance.
        refused(
            check_revolve(&plane, &profile.points, &y, 1e-6),
            "angle",
            "less than 1e-7 mm",
        );
        assert!(check_revolve(&plane, &profile.points, &y, 1e-4).is_ok());
    }

    #[test]
    fn the_shortest_side_is_found_wherever_it_is() {
        let mut p = square(2.0);
        assert_eq!(p.shortest_side(), 0, "all equal: the first");
        p.points[2] = [2.0, 1e-3, 0.0];
        assert_eq!(p.shortest_side(), 1);
        p.points[0] = [0.0, 2.0 - 1e-6, 0.0];
        assert_eq!(p.shortest_side(), 3, "the closing side");
        // Off the origin, in a tilted plane, every coordinate nonzero, the shortest side in
        // the middle; another side's coordinates sum to nearly nothing, but it's long.
        let tilted = Profile {
            points: vec![
                [5.0, -3.0, 2.0],
                [7.0, 3.0, -2.0],
                [7.0 + 1e-6, 3.0 - 2e-6, -2.0 + 3e-6],
                [1.0, 4.0, 6.0],
            ],
        };
        assert_eq!(tilted.shortest_side(), 1);
    }

    #[test]
    fn point_segment_distances_match_hand_calculations() {
        // Beside the middle, past each end, on it, and to a segment with no length.
        let (a, b) = ([0.0, 0.0, 0.0], [2.0, 0.0, 0.0]);
        assert_eq!(point_segment_distance([1.0, 3.0, 0.0], a, b), 3.0);
        assert_eq!(point_segment_distance([5.0, 4.0, 0.0], a, b), 5.0);
        assert_eq!(point_segment_distance([-3.0, 0.0, 4.0], a, b), 5.0);
        assert_eq!(point_segment_distance([1.5, 0.0, 0.0], a, b), 0.0);
        assert_eq!(point_segment_distance([3.0, 4.0, 0.0], a, a), 5.0);
    }
}
