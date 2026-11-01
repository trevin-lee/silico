//! A profile's regions, as the core sends them for the kernel to make faces of
//! (docs/design/m3.5-profile.md, "The two halves"): the sketch's plane, and each region's
//! outer loop and holes, every loop its curves in walking order, already oriented. The core
//! decides the loops, their nesting, and their direction; the worker builds what it's sent
//! and classifies nothing. The rule for what can be sent lives here, with the types, so the
//! core refuses a bad request before sending it and the worker refuses it again before OCCT
//! sees it. It's plain arithmetic and `libm`, so it gives the same answer on every platform.

use std::collections::BTreeMap;
use std::f64::consts::TAU;

use serde::{Deserialize, Serialize};

use crate::primitive::{InvalidParameter, LENGTH_TOLERANCE, MAX_LENGTH, format_number};
use crate::sketch::MAX_SKETCH_GEOMETRY;
use crate::sweep::{UNIT_SLACK, check_unit};
use crate::{NORMAL_LENGTH_TOLERANCE, vec3};

/// The most curves a request may have, over all its loops: a sketch's most pieces of
/// geometry.
pub const MAX_FACE_CURVES: usize = MAX_SKETCH_GEOMETRY;

/// How far from 0 an arc's angle may be, in radians: two turns. The core's angles are its
/// start's direction, from -π to π, and the end's, at most a turn from it.
pub const MAX_ARC_ANGLE: f64 = 2.0 * TAU;

/// The plane a sketch lies on: one of FreeCAD's origin planes with its orientation (D6,
/// docs/design/m3.2-sketch-format.md), or a face's frame (docs/ports/attacher.md, M5.9). An
/// origin plane's axes are unit axes of model space, so its points are placed exactly, by
/// moving their coordinates; a face frame's generally aren't, so its points are placed to
/// rounding.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SketchPlane {
    /// The sketch's x along +X, its y along +Y, its normal +Z.
    Xy,
    /// The sketch's x along +X, its y along +Z, its normal -Y.
    Xz,
    /// The sketch's x along +Y, its y along +Z, its normal +X.
    Yz,
    /// A planar face's frame, as the core resolved it from the face ([`FaceFrame`]).
    Face(FaceFrame),
}

/// A sketch's frame on a planar face, as the core resolved it from the face's normal n and
/// plane, its stored axis, and `reversed` (docs/ports/attacher.md, F1 and F2,
/// [`FaceFrame::from_normal`]): its origin, the model's origin projected onto the face's
/// plane; its x, the stored axis projected onto the plane; and its y, n × x. It has no
/// normal of its own: its normal is x × y, normalized ([`SketchPlane::normal`]), the one
/// normal the kernel's plane, the fake kernel, and every sweep read. Its axes are unit and
/// square to rounding ([`FaceFrame::check`]), not unit axes of model space, so a point on it
/// is placed to rounding, not exactly.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FaceFrame {
    pub origin: [f64; 3],
    pub x: [f64; 3],
    pub y: [f64; 3],
}

/// How far from perpendicular a frame's x and y may be: |x · y| at most 16ε (4 ·
/// `UNIT_SLACK`, about 3.6e-15), a few roundings, as the core makes them (y = n × x,
/// normalized, is square to x to about 1.7e-16). OCCT builds the faces' plane from the
/// normal and x alone, normalizing x and making its own y as n × x, while a point is placed
/// along the frame's own y. The two y's are apart by about |x · y|, so at the largest
/// reach, 1e6 mm ([`MAX_LENGTH`]), a point placed on the frame is within 1e6 · 3.6e-15 ≈
/// 3.6e-9 mm of where OCCT puts an arc's end: the frame's share beside the 1e-7
/// ([`LENGTH_TOLERANCE`]) an arc's end may be off its circle (`Curve::off_circle`), which
/// the origin planes have to themselves.
pub const FACE_FRAME_SQUARENESS: f64 = 4.0 * UNIT_SLACK;

/// How far a frame's origin may be from the model's: √3 times the reach, and the rounding
/// of working it out (4 · 4ε, as a sweep's longest vector has). The origin is n(n·p), the
/// model's origin projected onto the face's plane: as far out as the plane's offset n·p,
/// which for a face within a kilometre on each axis is at most the box's corner, √3 · 1e6,
/// and which can lie further out on an axis than the face itself does (docs/ports/attacher.md,
/// the frame).
pub const MAX_FRAME_ORIGIN: f64 = 1.732_050_807_568_877_2 * MAX_LENGTH * (1.0 + 4.0 * UNIT_SLACK);

/// The least a stored axis may stand off a face's normal for a frame to be made from them:
/// |a − n(n·a)|, the length of its projection onto the face's plane, at least 1e-12. Below,
/// the axis lies along the normal, and x, the projection made a unit, would be mostly
/// rounding (docs/ports/attacher.md, open point 5).
pub const MIN_AXIS_OFF_NORMAL: f64 = 1e-12;

/// Why [`FaceFrame::from_normal`] made no frame.
#[derive(Debug, Clone, PartialEq)]
pub enum FaceFrameError {
    /// The stored axis lies along the face's normal: its projection onto the plane is this
    /// long, under [`MIN_AXIS_OFF_NORMAL`].
    AxisAlongNormal(f64),
    /// A value given or made isn't one a frame can hold (an input not finite, a normal that
    /// isn't a unit vector to [`NORMAL_LENGTH_TOLERANCE`], or the frame made refused by
    /// [`FaceFrame::check`]): what's wrong, naming the plane.
    Invalid(InvalidParameter),
}

impl FaceFrame {
    /// The frame of a sketch on a plane through `on_plane` with the normal `n`, its x
    /// following the stored `axis` (one of model space's six signed unit axes), exactly as
    /// docs/ports/attacher.md's frame says, n first made a unit (`describe`'s normals are
    /// unit only to `NORMAL_LENGTH_TOLERANCE`, 1e-6: o = n(n·p) of one 1e-6 long, on a face
    /// √3 · 1e6 out, is about 3 mm off its plane): o = n(n·p); x = (a − n(n·a)) / |a − n(n·a)|,
    /// projected once more (x ← (x − n(n·x)) / |…|), since the first division leaves x·n
    /// off by about 1e-16 / |a − n(n·a)|; y = n × x, normalized; every component + 0, so a
    /// zero is +0 whatever the sign `describe` gave it. A `reversed` sketch is the frame of
    /// −n and −a: o is the same, and x negated, y the same, exactly.
    ///
    /// Refused as `AxisAlongNormal` if the axis lies along the normal (|a − n(n·a)| under
    /// [`MIN_AXIS_OFF_NORMAL`]); as invalid if any input isn't finite, if the normal isn't a
    /// unit vector to [`NORMAL_LENGTH_TOLERANCE`] (as `describe` gives a plane's: one zero,
    /// 1e-160, or 1e200 long is a fault in what described it, not a normal to make a unit
    /// of), or if the frame made isn't one [`FaceFrame::check`] takes (its origin past
    /// [`MAX_FRAME_ORIGIN`]).
    pub fn from_normal(
        on_plane: [f64; 3],
        n: [f64; 3],
        axis: [f64; 3],
    ) -> Result<FaceFrame, FaceFrameError> {
        for (what, v) in [
            ("point on the plane", on_plane),
            ("normal", n),
            ("axis", axis),
        ] {
            if !v.iter().all(|c| c.is_finite()) {
                return Err(FaceFrameError::Invalid(frame_invalid(format!(
                    "the face's {what} must be finite, got {v:?}"
                ))));
            }
        }
        let length = vec3::norm(n);
        let off_unit = (length - 1.0).abs();
        if off_unit.is_nan() || off_unit > NORMAL_LENGTH_TOLERANCE {
            // Its length as it is, measured scaled by its largest component: a normal 1e200
            // long squares past f64's range, and one 1e-160 long into its subnormals, where
            // the bare norm is inf or rounded.
            let largest = n.iter().fold(0.0_f64, |m, c| m.max(c.abs()));
            let length = if largest > 0.0 {
                largest * vec3::norm(n.map(|c| c / largest))
            } else {
                0.0
            };
            return Err(FaceFrameError::Invalid(frame_invalid(format!(
                "the face's normal must be a unit vector to {}, got {n:?}, {} long",
                format_number(NORMAL_LENGTH_TOLERANCE),
                format_number(length)
            ))));
        }
        let n = n.map(|c| c / length);
        let project = |v: [f64; 3]| {
            let along = vec3::dot(n, v);
            [0, 1, 2].map(|k| v[k] - n[k] * along)
        };
        let off = project(axis);
        let off_length = vec3::norm(off);
        if off_length.is_nan() || off_length < MIN_AXIS_OFF_NORMAL {
            return Err(FaceFrameError::AxisAlongNormal(off_length));
        }
        let x = vec3::normalize(project(off.map(|c| c / off_length)));
        let y = vec3::normalize(vec3::cross(n, x));
        let offset = vec3::dot(n, on_plane);
        let plus = |v: [f64; 3]| v.map(|c| c + 0.0);
        let frame = FaceFrame {
            origin: plus(n.map(|c| c * offset)),
            x: plus(x),
            y: plus(y),
        };
        frame.check().map_err(FaceFrameError::Invalid)?;
        Ok(frame)
    }

    /// Whether a sketch can be placed on it: every component finite; the origin at most
    /// [`MAX_FRAME_ORIGIN`] from the model's origin; x and y each a unit vector, to the
    /// rounding a sweep's direction may have (`UNIT_SLACK`, 4ε); and x and y perpendicular,
    /// to [`FACE_FRAME_SQUARENESS`]. Right-handedness needs no rule: the normal is x × y.
    /// Says which rule failed, with the values, if not.
    pub fn check(&self) -> Result<(), InvalidParameter> {
        let axes = [("x axis", self.x), ("y axis", self.y)];
        for (what, v) in std::iter::once(("origin", self.origin)).chain(axes) {
            if !v.iter().all(|c| c.is_finite()) {
                return Err(frame_invalid(format!(
                    "the face frame's {what} must be finite, got {v:?}"
                )));
            }
        }
        let reach = vec3::norm(self.origin);
        if reach > MAX_FRAME_ORIGIN {
            return Err(frame_invalid(format!(
                "the face frame's origin must be within √3 · 1e6 mm (a kilometre on each axis) of the model's origin, got {:?}, {} mm out",
                self.origin,
                format_number(reach)
            )));
        }
        for (what, v) in axes {
            check_unit("plane", &format!("face frame's {what}"), v)?;
        }
        let d = vec3::dot(self.x, self.y);
        if d.abs() > FACE_FRAME_SQUARENESS {
            return Err(frame_invalid(format!(
                "the face frame's x axis {:?} and y axis {:?} must be perpendicular, to 16ε (3.6e-15): their dot product is {}",
                self.x,
                self.y,
                format_number(d)
            )));
        }
        Ok(())
    }
}

fn frame_invalid(message: String) -> InvalidParameter {
    InvalidParameter {
        parameter: "plane",
        message,
    }
}

impl SketchPlane {
    /// The origin planes, every plane a sketch's file can name by a word (D6). A face's
    /// frame is the other kind.
    pub const ORIGIN_PLANES: [SketchPlane; 3] = [SketchPlane::Xy, SketchPlane::Xz, SketchPlane::Yz];

    /// Its word, as a step's line and the fake's shapes spell it: an origin plane's the
    /// document's (`xy`, `xz`, `yz`, D6), and `frame` for a face's frame, whose numbers
    /// follow it there.
    pub fn word(self) -> &'static str {
        match self {
            SketchPlane::Xy => "xy",
            SketchPlane::Xz => "xz",
            SketchPlane::Yz => "yz",
            SketchPlane::Face(_) => "frame",
        }
    }

    /// Where the sketch's (0, 0) is in model space: the model's origin for an origin plane,
    /// a face frame's origin for a face's, each component + 0, as the faces step's key takes
    /// it, so a −0 the key reads as +0 never reaches the kernel as −0.
    pub fn origin(self) -> [f64; 3] {
        match self {
            SketchPlane::Xy | SketchPlane::Xz | SketchPlane::Yz => [0.0; 3],
            SketchPlane::Face(frame) => frame.origin.map(|c| c + 0.0),
        }
    }

    /// The direction of the sketch's x in model space (a face frame's + 0, as its origin).
    pub fn x_axis(self) -> [f64; 3] {
        match self {
            SketchPlane::Xy | SketchPlane::Xz => [1.0, 0.0, 0.0],
            SketchPlane::Yz => [0.0, 1.0, 0.0],
            SketchPlane::Face(frame) => frame.x.map(|c| c + 0.0),
        }
    }

    /// The direction of the sketch's y in model space (a face frame's + 0, as its origin).
    pub fn y_axis(self) -> [f64; 3] {
        match self {
            SketchPlane::Xy => [0.0, 1.0, 0.0],
            SketchPlane::Xz | SketchPlane::Yz => [0.0, 0.0, 1.0],
            SketchPlane::Face(frame) => frame.y.map(|c| c + 0.0),
        }
    }

    /// The sketch's unit normal in model space: x × y, the direction counter-clockwise is
    /// measured about. An origin plane's is its axis exactly; a face frame's is x × y
    /// normalized, each component + 0, the one normal the kernel builds the faces' plane
    /// from and every sweep of them reads.
    pub fn normal(self) -> [f64; 3] {
        match self {
            SketchPlane::Xy => [0.0, 0.0, 1.0],
            SketchPlane::Xz => [0.0, -1.0, 0.0],
            SketchPlane::Yz => [1.0, 0.0, 0.0],
            SketchPlane::Face(frame) => {
                vec3::normalize(vec3::cross(frame.x, frame.y)).map(|c| c + 0.0)
            }
        }
    }

    /// The sketch's point `[x, y]` in model space: on an origin plane exactly (a zero of
    /// either sign as 0); on a face frame, its origin + x times its x axis + y times its y
    /// axis, component by component, to rounding (a zero of either sign as 0 there too).
    pub fn place(self, [x, y]: [f64; 2]) -> [f64; 3] {
        let (x, y) = (unsigned(x), unsigned(y));
        match self {
            SketchPlane::Xy => [x, y, 0.0],
            SketchPlane::Xz => [x, 0.0, y],
            SketchPlane::Yz => [0.0, x, y],
            SketchPlane::Face(FaceFrame { origin, x: u, y: v }) => {
                std::array::from_fn(|k| origin[k] + x * u[k] + y * v[k] + 0.0)
            }
        }
    }

    /// Whether its frame can be used: an origin plane always can; a face frame by
    /// [`FaceFrame::check`].
    pub fn check(&self) -> Result<(), InvalidParameter> {
        match self {
            SketchPlane::Xy | SketchPlane::Xz | SketchPlane::Yz => Ok(()),
            SketchPlane::Face(frame) => frame.check(),
        }
    }
}

/// `value`, with a zero of either sign as 0: the same point whatever the sign of its zeros.
fn unsigned(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

/// One curve of a loop, in the sketch's coordinates, in the direction the loop walks it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Curve {
    /// A line from `start` to `end`.
    Line { start: [f64; 2], end: [f64; 2] },
    /// An arc of the circle of `radius` about `center`, from `start` to `end`. Its angles
    /// are the directions of `start` and `end` from `center`, in radians from the sketch's
    /// x towards its y, as the core works them out: the arc turns from `start_angle` to
    /// `end_angle`, counter-clockwise where `end_angle` is the larger and clockwise where
    /// it's the smaller, less than a turn either way. A sketch's arc runs counter-clockwise
    /// from its start to its end; a loop that walks it the other way sends it end first,
    /// its angles swapped.
    Arc {
        center: [f64; 2],
        start: [f64; 2],
        end: [f64; 2],
        radius: f64,
        start_angle: f64,
        end_angle: f64,
    },
    /// A whole circle: a loop by itself, with no points. It runs counter-clockwise as an
    /// outer loop and clockwise as a hole, from its point at angle 0, `center + (radius, 0)`,
    /// where its one vertex is.
    Circle { center: [f64; 2], radius: f64 },
}

/// One of an arc's two ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArcEnd {
    Start,
    End,
}

impl ArcEnd {
    /// As messages say it: `start`, `end`.
    pub fn as_str(self) -> &'static str {
        match self {
            ArcEnd::Start => "start",
            ArcEnd::End => "end",
        }
    }
}

/// What's too small about a curve for OCCT to build ([`Curve::size_problem`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SizeProblem {
    /// An arc's or a circle's radius, not above [`LENGTH_TOLERANCE`].
    Radius(f64),
    /// How far apart a line's or an arc's ends are, not more than [`LENGTH_TOLERANCE`]
    /// (by `libm`'s `hypot`).
    Ends(f64),
}

impl Curve {
    /// What's too small about it to build, if anything: its radius (an arc's or a
    /// circle's) at most OCCT's tolerance, or else its ends (a line's or an arc's) at most
    /// the tolerance apart. Two places within the tolerance are one place to OCCT. The
    /// request's check refuses what this names, and the core's profile refuses it first,
    /// by the same rule on the same numbers.
    pub fn size_problem(&self) -> Option<SizeProblem> {
        let radius = match *self {
            Curve::Arc { radius, .. } | Curve::Circle { radius, .. } => Some(radius),
            Curve::Line { .. } => None,
        };
        // Not above the tolerance, or not a number.
        if let Some(radius) = radius
            && (radius <= LENGTH_TOLERANCE || radius.is_nan())
        {
            return Some(SizeProblem::Radius(radius));
        }
        let (start, end) = self.ends()?;
        let length = distance(start, end);
        (length <= LENGTH_TOLERANCE).then_some(SizeProblem::Ends(length))
    }

    /// For an arc, the first of its ends (its start, then its end) more than OCCT's
    /// tolerance from where its angle and radius put it, and how far. An arc's points are
    /// its definition; its radius and angles come from them, so an end off its circle is a
    /// solve's or an edit's, not the curve's. The request's check refuses what this names,
    /// and the core's profile refuses it first.
    pub fn off_circle(&self) -> Option<(ArcEnd, f64)> {
        let Curve::Arc {
            center,
            start,
            end,
            radius,
            start_angle,
            end_angle,
        } = *self
        else {
            return None;
        };
        [
            (ArcEnd::Start, start, start_angle),
            (ArcEnd::End, end, end_angle),
        ]
        .into_iter()
        .map(|(what, p, angle)| (what, distance(p, on_circle(center, radius, angle))))
        .find(|&(_, off)| off > LENGTH_TOLERANCE)
    }

    /// Where it starts and ends, if it isn't a circle.
    pub fn ends(&self) -> Option<([f64; 2], [f64; 2])> {
        match *self {
            Curve::Line { start, end } | Curve::Arc { start, end, .. } => Some((start, end)),
            Curve::Circle { .. } => None,
        }
    }
}

/// A face to make: its outer loop, counter-clockwise about the plane's normal, and its
/// holes, each clockwise. Each loop is its curves in walking order, each curve's end the
/// next one's start (the last's the first's), or one circle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub outer: Vec<Curve>,
    pub holes: Vec<Vec<Curve>>,
}

impl Region {
    /// Its loops in order: the outer loop, then the holes.
    pub fn loops(&self) -> impl Iterator<Item = &[Curve]> {
        std::iter::once(self.outer.as_slice()).chain(self.holes.iter().map(Vec::as_slice))
    }
}

/// What [`Op::MakeFaces`](crate::Op::MakeFaces) makes faces of: a sketch's plane and its
/// profile's regions, in the core's order (docs/design/m3.5-profile.md, question 2).
///
/// The faces come back as one shape, with the regions' history as the operation's one
/// input: its edges are the curves in the order sent (region by region, each region's
/// outer loop and then its holes, each loop's curves in order), and its vertices the points
/// in the order sent (each loop's curves' starts, in order; a circle has none). Each is kept
/// as one edge or vertex of the result. Face `n` is region `n`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceRegions {
    pub plane: SketchPlane,
    pub regions: Vec<Region>,
}

/// Where a curve is, for messages: its number in the order sent, its region, and its loop.
struct Place {
    curve: usize,
    region: usize,
    /// 0 for the outer loop, then each hole from 1.
    contour: usize,
    /// Its number in its loop, from 1.
    index: usize,
}

impl std::fmt::Display for Place {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "curve {} (curve {} of {})",
            self.curve,
            self.index,
            contour_name(self.region, self.contour)
        )
    }
}

/// `region 2's outer loop`, `region 2's hole 1`.
fn contour_name(region: usize, contour: usize) -> String {
    match contour {
        0 => format!("region {region}'s outer loop"),
        hole => format!("region {region}'s hole {hole}"),
    }
}

fn invalid(message: String) -> InvalidParameter {
    InvalidParameter {
        parameter: "regions",
        message,
    }
}

fn within_reach(p: [f64; 2]) -> bool {
    p.iter().all(|c| c.is_finite() && c.abs() <= MAX_LENGTH)
}

/// How far apart `a` and `b` are, by `libm`'s `hypot` (the same on every platform): the
/// distance every closeness rule of a profile and its request is measured by.
pub fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    libm::hypot(b[0] - a[0], b[1] - a[1])
}

/// The point at `angle` on the circle of `radius` about `center`, as the arc's angles put
/// it (`libm`'s `cos` and `sin`, the same on every platform): where the kernel ends an arc,
/// to an ulp (OCCT evaluates its circle in 3D, by the platform's `cos` and `sin`).
pub fn on_circle(center: [f64; 2], radius: f64, angle: f64) -> [f64; 2] {
    [
        center[0] + radius * libm::cos(angle),
        center[1] + radius * libm::sin(angle),
    ]
}

impl FaceRegions {
    /// Every curve, in the order sent.
    pub fn curves(&self) -> impl Iterator<Item = &Curve> {
        self.regions.iter().flat_map(Region::loops).flatten()
    }

    /// Every point, in the order sent: each loop's curves' starts, in order.
    pub fn points(&self) -> impl Iterator<Item = [f64; 2]> + '_ {
        self.curves()
            .filter_map(|c| c.ends().map(|(start, _)| start))
    }

    /// Whether the regions can be made into faces: the plane's frame sound
    /// ([`SketchPlane::check`]), at least one region, at most
    /// [`MAX_FACE_CURVES`] curves, every loop closed (each curve's end exactly the next
    /// one's start, as the sketch's points are shared) or one circle, each point the meeting
    /// of two curves only, every value finite and within a kilometre, each line and arc
    /// longer than OCCT's tolerance between its ends, each arc's ends where its angles put
    /// them (within the tolerance) and less than a turn apart, and each outer loop
    /// enclosing an area counter-clockwise and each hole clockwise, more than the tolerance
    /// times its perimeter. Crossing and nesting are the core's to decide, and OCCT checks
    /// the faces it makes.
    pub fn check(&self) -> Result<(), InvalidParameter> {
        self.plane.check()?;
        if self.regions.is_empty() {
            return Err(invalid("no regions to make faces of".to_owned()));
        }
        let curves = self.curves().count();
        if curves > MAX_FACE_CURVES {
            return Err(invalid(format!(
                "a profile may have at most {MAX_FACE_CURVES} curves, got {curves}"
            )));
        }
        // Where each point is, by its coordinates' bits (a zero of either sign as 0), to
        // find a point where more than two curves meet.
        let mut points: BTreeMap<(u64, u64), String> = BTreeMap::new();
        let mut curve = 0usize;
        for (r, region) in self.regions.iter().enumerate() {
            for (contour, curves) in region.loops().enumerate() {
                let name = || contour_name(r + 1, contour);
                if curves.is_empty() {
                    return Err(invalid(format!("{} has no curves", name())));
                }
                let first = curve + 1;
                for (i, c) in curves.iter().enumerate() {
                    curve += 1;
                    let place = Place {
                        curve,
                        region: r + 1,
                        contour,
                        index: i + 1,
                    };
                    check_curve(c, &place)?;
                    if matches!(c, Curve::Circle { .. }) && curves.len() > 1 {
                        return Err(invalid(format!(
                            "{place} is a circle in a loop of {} curves: a circle is a loop by itself",
                            curves.len()
                        )));
                    }
                }
                // Closed: each curve's end the next one's start.
                for (i, c) in curves.iter().enumerate() {
                    let next = &curves[(i + 1) % curves.len()];
                    let (Some((_, end)), Some((start, _))) = (c.ends(), next.ends()) else {
                        continue;
                    };
                    if end != start {
                        return Err(invalid(format!(
                            "{} doesn't close: curve {} ends at {end:?}, but curve {} starts at {start:?}",
                            name(),
                            first + i,
                            first + (i + 1) % curves.len()
                        )));
                    }
                    let key = (unsigned(end[0]).to_bits(), unsigned(end[1]).to_bits());
                    let here = format!(
                        "where curves {} and {} meet",
                        first + i,
                        first + (i + 1) % curves.len()
                    );
                    if let Some(before) = points.insert(key, here.clone()) {
                        return Err(invalid(format!(
                            "the point {end:?} is {before} and {here}: each of a profile's points joins two curves"
                        )));
                    }
                }
                check_area(curves, contour == 0, &name())?;
            }
        }
        Ok(())
    }
}

/// A curve's own values.
fn check_curve(c: &Curve, place: &Place) -> Result<(), InvalidParameter> {
    let unreachable = |what: &str, value: String| {
        invalid(format!(
            "{place}'s {what} must be finite and within 1e6 mm (a kilometre) of the origin, got {value}"
        ))
    };
    let check_radius = |radius: f64| {
        let small = matches!(c.size_problem(), Some(SizeProblem::Radius(_)));
        if !small && radius <= MAX_LENGTH {
            Ok(())
        } else {
            Err(invalid(format!(
                "{place}'s radius must be a length above 1e-7 mm and at most 1e6 mm (a kilometre), got {}",
                format_number(radius)
            )))
        }
    };
    let points: &[(&str, [f64; 2])] = match c {
        Curve::Line { start, end } => &[("start", *start), ("end", *end)],
        Curve::Arc {
            center, start, end, ..
        } => &[("center", *center), ("start", *start), ("end", *end)],
        Curve::Circle { center, .. } => &[("center", *center)],
    };
    for &(what, p) in points {
        if !within_reach(p) {
            return Err(unreachable(what, format!("{p:?}")));
        }
    }
    let short = |length: f64| {
        invalid(format!(
            "{place}'s ends are {} mm apart: a line or arc's ends must be more than 1e-7 mm apart",
            format_number(length)
        ))
    };
    let ends_short = || match c.size_problem() {
        Some(SizeProblem::Ends(length)) => Err(short(length)),
        _ => Ok(()),
    };
    match *c {
        Curve::Line { .. } => ends_short()?,
        Curve::Arc {
            radius,
            start_angle,
            end_angle,
            ..
        } => {
            check_radius(radius)?;
            for (what, angle) in [("start angle", start_angle), ("end angle", end_angle)] {
                if !(angle.is_finite() && angle.abs() <= MAX_ARC_ANGLE) {
                    return Err(invalid(format!(
                        "{place}'s {what} must be finite and within two turns (4π radians) of 0, got {}",
                        format_number(angle)
                    )));
                }
            }
            let sweep = (end_angle - start_angle).abs();
            if !(sweep > 0.0 && sweep < TAU) {
                return Err(invalid(format!(
                    "{place} turns {} radians: an arc turns more than 0 and less than a turn",
                    format_number(sweep)
                )));
            }
            ends_short()?;
            if let Some((end, off)) = c.off_circle() {
                return Err(invalid(format!(
                    "{place}'s {} is {} mm from where its angle and radius put it: no more than 1e-7 mm",
                    end.as_str(),
                    format_number(off)
                )));
            }
        }
        Curve::Circle { radius, .. } => check_radius(radius)?,
    }
    Ok(())
}

/// A loop's signed area, about its first point, a circle counter-clockwise
/// ([`loop_area`] runs one the way its role says), and its perimeter: the measure the
/// request's check and the core's profile both take a loop's area and perimeter by.
pub fn area_and_perimeter(curves: &[Curve]) -> (f64, f64) {
    use std::f64::consts::PI;
    let origin = curves
        .iter()
        .find_map(|c| c.ends().map(|(start, _)| start))
        .unwrap_or_default();
    let (mut twice_area, mut perimeter) = (0.0, 0.0);
    for c in curves {
        match *c {
            Curve::Line { start, end } => {
                twice_area += cross(start, end, origin);
                perimeter += distance(start, end);
            }
            Curve::Arc {
                start,
                end,
                radius,
                start_angle,
                end_angle,
                ..
            } => {
                let turn = end_angle - start_angle;
                twice_area +=
                    cross(start, end, origin) + radius * radius * (turn - libm::sin(turn));
                perimeter += radius * turn.abs();
            }
            Curve::Circle { radius, .. } => {
                twice_area += 2.0 * PI * radius * radius;
                perimeter += 2.0 * PI * radius;
            }
        }
    }
    (twice_area / 2.0, perimeter)
}

/// `a` × `b` about `origin`.
fn cross(a: [f64; 2], b: [f64; 2], origin: [f64; 2]) -> f64 {
    let (a, b) = (
        [a[0] - origin[0], a[1] - origin[1]],
        [b[0] - origin[0], b[1] - origin[1]],
    );
    a[0] * b[1] - a[1] * b[0]
}

/// Whether a loop (or a region, its loops' areas and perimeters summed) of `area` and
/// `perimeter` encloses no area to speak of: |area| at most the tolerance times the
/// perimeter, M1's rule (`Profile::check`). On a thin rectangle, a width up to about twice
/// the tolerance.
pub fn sliver(area: f64, perimeter: f64) -> bool {
    area.abs() <= LENGTH_TOLERANCE * perimeter
}

/// That a loop encloses an area, more than the tolerance times its perimeter, the way its
/// role says: counter-clockwise for an outer loop, clockwise for a hole.
fn check_area(curves: &[Curve], outer: bool, name: &str) -> Result<(), InvalidParameter> {
    let area = loop_area(curves, outer);
    let (_, perimeter) = area_and_perimeter(curves);
    if sliver(area, perimeter) {
        return Err(invalid(format!(
            "{name} encloses no area to speak of ({} mm², against a perimeter of {} mm)",
            format_number(area.abs()),
            format_number(perimeter)
        )));
    }
    match (outer, area > 0.0) {
        (true, false) => Err(invalid(format!(
            "{name} runs clockwise about the plane's normal: an outer loop runs counter-clockwise"
        ))),
        (false, true) => Err(invalid(format!(
            "{name} runs counter-clockwise about the plane's normal: a hole runs clockwise"
        ))),
        _ => Ok(()),
    }
}

/// A loop's signed area, counter-clockwise about the plane's normal positive, a circle
/// running the way its role says (`outer`, counter-clockwise; a hole, clockwise): the
/// shoelace sum over its chords, about its first point, and each arc's segment between its
/// chord and itself, (r²/2)(θ − sin θ), signed by its turn θ.
pub fn loop_area(curves: &[Curve], outer: bool) -> f64 {
    let (area, _) = area_and_perimeter(curves);
    match (curves, outer) {
        ([Curve::Circle { .. }], false) => -area,
        _ => area,
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{FRAC_PI_2, PI};

    use super::*;

    fn line(start: [f64; 2], end: [f64; 2]) -> Curve {
        Curve::Line { start, end }
    }

    fn circle(center: [f64; 2], radius: f64) -> Curve {
        Curve::Circle { center, radius }
    }

    /// The `w` × `h` rectangle from `(x, y)`, counter-clockwise from its lowest corner.
    fn rectangle(x: f64, y: f64, w: f64, h: f64) -> Vec<Curve> {
        let p = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
        (0..4).map(|i| line(p[i], p[(i + 1) % 4])).collect()
    }

    /// The loop walked the other way.
    fn reversed(curves: &[Curve]) -> Vec<Curve> {
        curves
            .iter()
            .rev()
            .map(|c| match *c {
                Curve::Line { start, end } => line(end, start),
                Curve::Arc {
                    center,
                    start,
                    end,
                    radius,
                    start_angle,
                    end_angle,
                } => Curve::Arc {
                    center,
                    start: end,
                    end: start,
                    radius,
                    start_angle: end_angle,
                    end_angle: start_angle,
                },
                circle => circle,
            })
            .collect()
    }

    /// A slot: half circles of radius 1 about (0, 0) and (4, 0), joined by lines 4 long,
    /// counter-clockwise: 8 + π.
    fn slot() -> Vec<Curve> {
        vec![
            line([0.0, -1.0], [4.0, -1.0]),
            Curve::Arc {
                center: [4.0, 0.0],
                start: [4.0, -1.0],
                end: [4.0, 1.0],
                radius: 1.0,
                start_angle: -FRAC_PI_2,
                end_angle: FRAC_PI_2,
            },
            line([4.0, 1.0], [0.0, 1.0]),
            Curve::Arc {
                center: [0.0, 0.0],
                start: [0.0, 1.0],
                end: [0.0, -1.0],
                radius: 1.0,
                start_angle: FRAC_PI_2,
                end_angle: 3.0 * FRAC_PI_2,
            },
        ]
    }

    fn one(outer: Vec<Curve>, holes: Vec<Vec<Curve>>) -> FaceRegions {
        FaceRegions {
            plane: SketchPlane::Xy,
            regions: vec![Region { outer, holes }],
        }
    }

    fn refused(regions: &FaceRegions, says: &str) {
        let err = regions.check().unwrap_err();
        assert_eq!(err.parameter, "regions", "{}", err.message);
        assert!(err.message.contains(says), "{}", err.message);
    }

    /// A box's top face at z = 0, reversed: x −X, y +Y, so its normal is −Z. Its axes are
    /// model space's, so it places as exactly as an origin plane.
    fn reversed_top(origin: [f64; 3]) -> FaceFrame {
        FaceFrame {
            origin,
            x: [-1.0, 0.0, 0.0],
            y: [0.0, 1.0, 0.0],
        }
    }

    #[test]
    fn each_plane_is_right_handed_and_places_a_point_by_moving_its_coordinates() {
        let every = SketchPlane::ORIGIN_PLANES
            .into_iter()
            .chain([SketchPlane::Face(reversed_top([0.0; 3]))]);
        for plane in every {
            let (x, y) = (plane.x_axis(), plane.y_axis());
            let x_cross_y = [
                x[1] * y[2] - x[2] * y[1],
                x[2] * y[0] - x[0] * y[2],
                x[0] * y[1] - x[1] * y[0],
            ];
            assert_eq!(x_cross_y, plane.normal(), "{plane:?}");
            // The point is its x along the x axis and its y along the y axis.
            let p = plane.place([3.5, -2.25]);
            let want: Vec<f64> = (0..3).map(|k| 3.5 * x[k] - 2.25 * y[k]).collect();
            assert_eq!(p.to_vec(), want, "{plane:?}");
        }
        // D6: XZ's y is +Z and its normal -Y; YZ's x is +Y.
        assert_eq!(SketchPlane::Xz.place([1.0, 2.0]), [1.0, 0.0, 2.0]);
        assert_eq!(SketchPlane::Yz.place([1.0, 2.0]), [0.0, 1.0, 2.0]);
        // The reversed top's normal, by hand: (−X) × (+Y) = −Z.
        assert_eq!(
            SketchPlane::Face(reversed_top([0.0; 3])).normal(),
            [0.0, 0.0, -1.0]
        );
        // A zero of either sign is the same point.
        assert_eq!(
            SketchPlane::Xy.place([-0.0, -0.0]).map(f64::to_bits),
            [0.0f64; 3].map(f64::to_bits)
        );
    }

    #[test]
    fn each_plane_has_its_word() {
        let words: Vec<&str> = SketchPlane::ORIGIN_PLANES
            .into_iter()
            .chain([SketchPlane::Face(reversed_top([0.0; 3]))])
            .map(SketchPlane::word)
            .collect();
        assert_eq!(words, ["xy", "xz", "yz", "frame"]);
    }

    /// The frame tilted 30° about X: x +X, y (0, cos 30°, sin 30°), so n (0, −sin 30°,
    /// cos 30°), its origin at (1, 2, 3).
    fn tilted() -> FaceFrame {
        let (c, s) = (libm::cos(PI / 6.0), libm::sin(PI / 6.0));
        FaceFrame {
            origin: [1.0, 2.0, 3.0],
            x: [1.0, 0.0, 0.0],
            y: [0.0, c, s],
        }
    }

    fn frame_refused(frame: FaceFrame, says: &str) {
        let err = frame.check().unwrap_err();
        assert_eq!(err.parameter, "plane", "{}", err.message);
        assert!(err.message.contains(says), "{}", err.message);
        // And every request on it, before its regions are looked at.
        let regions = FaceRegions {
            plane: SketchPlane::Face(frame),
            regions: vec![],
        };
        assert_eq!(regions.check(), Err(err));
    }

    #[test]
    fn a_sound_frame_passes_to_rounding_and_places_a_point_along_its_axes() {
        assert_eq!(tilted().check(), Ok(()));
        let reversed = reversed_top([0.0, 0.0, 10.0]);
        assert_eq!(reversed.check(), Ok(()));
        // An axis 4ε long or short of 1 (UNIT_SLACK), and x and y 16ε off square: within
        // the rules.
        for off in [
            FaceFrame {
                x: [-(1.0 + 4.0 * f64::EPSILON), 0.0, 0.0],
                ..reversed
            },
            FaceFrame {
                y: [0.0, 1.0 - 4.0 * f64::EPSILON, 0.0],
                ..reversed
            },
            FaceFrame {
                y: [FACE_FRAME_SQUARENESS, 1.0, 0.0],
                ..reversed
            },
        ] {
            assert_eq!(off.check(), Ok(()), "{off:?}");
        }
        // At a kilometre's reach on every axis, the origin passes.
        let far = reversed_top([-1e6, 1e6, -1e6]);
        assert_eq!(far.check(), Ok(()));
        // (3, −2) on the reversed top: (−3, −2, 10), by hand.
        assert_eq!(
            SketchPlane::Face(reversed).place([3.0, -2.0]),
            [-3.0, -2.0, 10.0]
        );
        // On the tilted frame: origin + u·x + v·y, component by component.
        let f = tilted();
        let (c, s) = (libm::cos(PI / 6.0), libm::sin(PI / 6.0));
        assert_eq!(
            SketchPlane::Face(f).place([4.0, 2.0]),
            [1.0 + 4.0, 2.0 + 2.0 * c, 3.0 + 2.0 * s]
        );
        // Its axes are the frame's, and its normal x × y: (0, −s, c), to rounding.
        let plane = SketchPlane::Face(f);
        assert_eq!((plane.x_axis(), plane.y_axis()), (f.x, f.y));
        let n = plane.normal();
        assert!(
            (n[0] - 0.0).abs() + (n[1] + s).abs() + (n[2] - c).abs() <= 4.0 * f64::EPSILON,
            "{n:?}"
        );
        // A zero of either sign comes out +0: the origin's and the products' −0s, and the
        // normal's.
        let negative = FaceFrame {
            origin: [-0.0, -0.0, -0.0],
            x: [1.0, -0.0, 0.0],
            y: [-0.0, 1.0, 0.0],
        };
        // x × y's first component is (−0)(0) − (0)(1) = −0, so the normal's + 0 shows.
        assert_eq!(
            vec3::cross(negative.x, negative.y)[0].to_bits(),
            (-0.0f64).to_bits()
        );
        assert_eq!(
            SketchPlane::Face(negative)
                .place([-0.0, -0.0])
                .map(f64::to_bits),
            [0.0f64; 3].map(f64::to_bits)
        );
        assert_eq!(
            SketchPlane::Face(negative).normal().map(f64::to_bits),
            [0.0, 0.0, 1.0f64].map(f64::to_bits)
        );
    }

    #[test]
    fn a_frame_that_isnt_orthonormal_is_refused_naming_the_rule() {
        let f = tilted();
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            frame_refused(
                FaceFrame {
                    origin: [1.0, bad, 3.0],
                    ..f
                },
                "the face frame's origin must be finite",
            );
            frame_refused(
                FaceFrame {
                    x: [bad, 0.0, 0.0],
                    ..f
                },
                "the face frame's x axis must be finite",
            );
            frame_refused(
                FaceFrame {
                    y: [0.0, bad, 0.5],
                    ..f
                },
                "the face frame's y axis must be finite",
            );
        }
        // √3e6 out along the diagonal passes (a box's far corner); a metre past it on one
        // axis, out of reach.
        assert_eq!(reversed_top([1e6, 1e6, 1e6]).check(), Ok(()));
        assert_eq!(reversed_top([0.0, 0.0, -1.7e6]).check(), Ok(()));
        frame_refused(
            reversed_top([0.0, 0.0, 1.733e6]),
            "the face frame's origin must be within √3 · 1e6 mm",
        );
        // An axis 8ε long or short of 1, past UNIT_SLACK's 4ε.
        frame_refused(
            FaceFrame {
                x: [1.0 + 8.0 * f64::EPSILON, 0.0, 0.0],
                ..f
            },
            "the face frame's x axis must be a unit vector, got [1.0000000000000018, 0.0, 0.0]",
        );
        frame_refused(
            FaceFrame {
                y: f.y.map(|c| c * (1.0 - 8.0 * f64::EPSILON)),
                ..f
            },
            "the face frame's y axis must be a unit vector",
        );
        // Unit, but not perpendicular: x leaning 2e-14 towards y.
        let lean = libm::sqrt(1.0 - 4e-28);
        frame_refused(
            FaceFrame {
                origin: [0.0; 3],
                x: [lean, 2e-14, 0.0],
                y: [0.0, 1.0, 0.0],
            },
            "the face frame's x axis [1.0, 2e-14, 0.0] and y axis [0.0, 1.0, 0.0] must be perpendicular, to 16ε (3.6e-15): their dot product is 2e-14",
        );
    }

    #[test]
    fn a_loops_area_is_its_shape_s_and_its_sign_its_direction() {
        let close = |got: f64, want: f64| {
            assert!((got - want).abs() <= 1e-12 * want.abs(), "{got} != {want}");
        };
        close(loop_area(&rectangle(0.0, 0.0, 4.0, 3.0), true), 12.0);
        close(
            loop_area(&reversed(&rectangle(0.0, 0.0, 4.0, 3.0)), false),
            -12.0,
        );
        // Far from the origin, the same.
        close(loop_area(&rectangle(9e5, -9e5, 4.0, 3.0), true), 12.0);
        close(loop_area(&[circle([5.0, 5.0], 2.0)], true), 4.0 * PI);
        close(loop_area(&[circle([5.0, 5.0], 2.0)], false), -4.0 * PI);
        close(loop_area(&slot(), true), 8.0 + PI);
        close(loop_area(&reversed(&slot()), false), -(8.0 + PI));
        // An arc of r 2 and its chord: a half circle, 2π.
        let half = [
            Curve::Arc {
                center: [0.0, 0.0],
                start: [2.0, 0.0],
                end: [-2.0, 0.0],
                radius: 2.0,
                start_angle: 0.0,
                end_angle: PI,
            },
            line([-2.0, 0.0], [2.0, 0.0]),
        ];
        close(loop_area(&half, true), 2.0 * PI);
        // A quarter arc's segment, (r²/2)(θ − sin θ), and its triangle to the centre.
        let quarter = [
            Curve::Arc {
                center: [0.0, 0.0],
                start: [3.0, 0.0],
                end: [0.0, 3.0],
                radius: 3.0,
                start_angle: 0.0,
                end_angle: FRAC_PI_2,
            },
            line([0.0, 3.0], [0.0, 0.0]),
            line([0.0, 0.0], [3.0, 0.0]),
        ];
        close(loop_area(&quarter, true), 9.0 * PI / 4.0);
    }

    #[test]
    fn regions_that_can_be_made_into_faces_pass() {
        let holes = vec![
            vec![circle([2.0, 1.5], 0.5)],
            reversed(&rectangle(0.5, 0.5, 0.5, 0.5)),
        ];
        assert_eq!(one(rectangle(0.0, 0.0, 4.0, 3.0), holes).check(), Ok(()));
        assert_eq!(one(slot(), vec![]).check(), Ok(()));
        assert_eq!(
            one(rectangle(-3.0, -3.0, 10.0, 6.0), vec![reversed(&slot())]).check(),
            Ok(())
        );
        assert_eq!(one(vec![circle([0.0, 0.0], 1.0)], vec![]).check(), Ok(()));
        // A zero of either sign is the same point: the loop closes.
        let mut signed = rectangle(0.0, 0.0, 4.0, 3.0);
        signed[3] = line([0.0, 3.0], [-0.0, 0.0]);
        assert_eq!(one(signed, vec![]).check(), Ok(()));
        // At the limits: a kilometre out, and a radius a hair over twice the tolerance.
        assert_eq!(one(rectangle(-1e6, -1e6, 2e6, 2e6), vec![]).check(), Ok(()));
        assert_eq!(
            one(vec![circle([0.0, 0.0], 2.1e-7)], vec![]).check(),
            Ok(())
        );
        // As many curves as a sketch may have.
        let mut many = one(rectangle(0.0, 0.0, 1000.0, 1000.0), vec![]);
        many.regions[0].holes = (0..MAX_FACE_CURVES - 4)
            .map(|i| {
                let (x, y) = ((i % 99) as f64, (i / 99) as f64);
                vec![circle([5.0 + 10.0 * x, 5.0 + 10.0 * y], 1.0)]
            })
            .collect();
        assert_eq!(many.check(), Ok(()));
        many.regions[0]
            .holes
            .push(vec![circle([500.0, 500.0], 1.0)]);
        refused(&many, "at most 10000 curves, got 10001");
    }

    #[test]
    fn regions_that_cant_be_made_into_faces_are_refused_saying_why() {
        let rect = || rectangle(0.0, 0.0, 4.0, 3.0);
        refused(
            &FaceRegions {
                plane: SketchPlane::Xy,
                regions: vec![],
            },
            "no regions",
        );
        refused(&one(vec![], vec![]), "region 1's outer loop has no curves");
        refused(
            &one(rect(), vec![vec![]]),
            "region 1's hole 1 has no curves",
        );
        // Open: three sides; a side ending a hair short of the next one's start.
        refused(
            &one(rect()[..3].to_vec(), vec![]),
            "doesn't close: curve 3 ends at [0.0, 3.0], but curve 1 starts at [0.0, 0.0]",
        );
        let mut short = rect();
        short[1] = line([4.0, 0.0], [4.0, 3.0 - 1e-15]);
        refused(
            &one(short, vec![]),
            "region 1's outer loop doesn't close: curve 2 ends",
        );
        // A point where four curves meet: two triangles at one corner; and a hole sharing
        // the outer loop's corner.
        let figure_eight = vec![
            line([0.0, 0.0], [1.0, 1.0]),
            line([1.0, 1.0], [0.0, 1.0]),
            line([0.0, 1.0], [0.0, 0.0]),
            line([0.0, 0.0], [-1.0, -1.0]),
            line([-1.0, -1.0], [0.0, -1.0]),
            line([0.0, -1.0], [0.0, 0.0]),
        ];
        refused(
            &one(figure_eight, vec![]),
            "each of a profile's points joins two curves",
        );
        let corner = reversed(&[
            line([0.0, 0.0], [1.0, 0.0]),
            line([1.0, 0.0], [0.0, 1.0]),
            line([0.0, 1.0], [0.0, 0.0]),
        ]);
        refused(
            &one(rect(), vec![corner]),
            "the point [0.0, 0.0] is where curves 4 and 1 meet and where curves",
        );
        refused(
            &one(
                vec![circle([0.0, 0.0], 1.0), line([1.0, 0.0], [2.0, 0.0])],
                vec![],
            ),
            "curve 1 (curve 1 of region 1's outer loop) is a circle in a loop of 2 curves",
        );
        // Values out of reach.
        for bad in [f64::NAN, f64::INFINITY, -2e6] {
            let mut r = rect();
            r[2] = line([4.0, 3.0], [bad, 3.0]);
            refused(
                &one(r, vec![]),
                "curve 3 (curve 3 of region 1's outer loop)'s end must be finite",
            );
            refused(
                &one(rect(), vec![vec![circle([bad, 1.0], 0.5)]]),
                "curve 5 (curve 1 of region 1's hole 1)'s center must be finite",
            );
        }
        for radius in [0.0, 1e-7, -1.0, 1.0000001e6, f64::NAN] {
            refused(
                &one(vec![circle([0.0, 0.0], radius)], vec![]),
                "radius must be a length above 1e-7 mm",
            );
        }
        refused(
            &one(vec![circle([0.0, 0.0], 2e-7)], vec![]),
            "encloses no area to speak of",
        );
        let mut tiny = rect();
        tiny.insert(1, line([4.0, 0.0], [4.0, 1e-7]));
        tiny[2] = line([4.0, 1e-7], [4.0, 3.0]);
        refused(
            &one(tiny, vec![]),
            "curve 2 (curve 2 of region 1's outer loop)'s ends are 1e-7 mm apart",
        );
        // Two lines between the same two points enclose nothing.
        refused(
            &one(
                vec![line([0.0, 0.0], [1.0, 0.0]), line([1.0, 0.0], [0.0, 0.0])],
                vec![],
            ),
            "encloses no area",
        );
        // Wrong way round, outer or hole.
        refused(
            &one(reversed(&rect()), vec![]),
            "region 1's outer loop runs clockwise",
        );
        refused(
            &one(rect(), vec![rectangle(1.0, 1.0, 1.0, 1.0)]),
            "region 1's hole 1 runs counter-clockwise",
        );
        refused(&one(reversed(&slot()), vec![]), "runs clockwise");
    }

    #[test]
    fn an_arc_must_be_where_its_angles_put_it_and_less_than_a_turn() {
        let arc = |start_angle: f64, end_angle: f64, radius: f64| {
            let mut s = slot();
            if let Curve::Arc {
                start_angle: a,
                end_angle: b,
                radius: r,
                ..
            } = &mut s[1]
            {
                (*a, *b, *r) = (start_angle, end_angle, radius);
            }
            one(s, vec![])
        };
        assert_eq!(arc(-FRAC_PI_2, FRAC_PI_2, 1.0).check(), Ok(()));
        // The same angles a turn on: still where they say.
        assert_eq!(arc(-FRAC_PI_2 + TAU, FRAC_PI_2 + TAU, 1.0).check(), Ok(()));
        refused(&arc(-FRAC_PI_2, FRAC_PI_2, 1.000_001), "start is 9.99999");
        refused(&arc(-FRAC_PI_2, FRAC_PI_2 + 1e-6, 1.0), "end is");
        // Turning the other way between the same ends: clockwise, through (3, 0), so the
        // slot's right end is bitten in where it bulged out: 8 − π/2 + π/2.
        let bitten = arc(-FRAC_PI_2, -3.0 * FRAC_PI_2, 1.0);
        assert_eq!(bitten.check(), Ok(()));
        let area = loop_area(&bitten.regions[0].outer, true);
        assert!((area - 8.0).abs() < 1e-12, "{area}");
        refused(&arc(-FRAC_PI_2, -FRAC_PI_2 + TAU, 1.0), "turns 6.2831853");
        refused(&arc(-FRAC_PI_2, -FRAC_PI_2, 1.0), "turns 0 radians");
        for bad in [f64::NAN, f64::INFINITY, 4.0 * PI + 1e-9, -13.0] {
            refused(
                &arc(bad, FRAC_PI_2, 1.0),
                "start angle must be finite and within two turns",
            );
            refused(
                &arc(-FRAC_PI_2, bad, 1.0),
                "end angle must be finite and within two turns",
            );
        }
        // Within the tolerance of where its angles put it, the arc passes.
        let mut nearly = slot();
        if let Curve::Arc { end, .. } = &mut nearly[1] {
            *end = [4.0, 1.0 + 5e-8];
        }
        nearly[2] = line([4.0, 1.0 + 5e-8], [0.0, 1.0]);
        assert_eq!(one(nearly, vec![]).check(), Ok(()));
    }

    #[test]
    fn curves_and_points_are_listed_in_the_order_sent() {
        let regions = FaceRegions {
            plane: SketchPlane::Yz,
            regions: vec![
                Region {
                    outer: rectangle(0.0, 0.0, 10.0, 10.0),
                    holes: vec![vec![circle([5.0, 5.0], 4.0)]],
                },
                Region {
                    outer: slot(),
                    holes: vec![],
                },
            ],
        };
        assert_eq!(regions.curves().count(), 9);
        assert_eq!(regions.curves().nth(4), Some(&circle([5.0, 5.0], 4.0)));
        let points: Vec<[f64; 2]> = regions.points().collect();
        assert_eq!(
            points,
            [
                [0.0, 0.0],
                [10.0, 0.0],
                [10.0, 10.0],
                [0.0, 10.0],
                [0.0, -1.0],
                [4.0, -1.0],
                [4.0, 1.0],
                [0.0, 1.0]
            ]
        );
    }
}
