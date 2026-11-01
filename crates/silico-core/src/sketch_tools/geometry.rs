//! The plane's arithmetic the tools use: points and vectors as `[x, y]` in millimetres, and
//! angles by `libm`, the same on every platform, as every number that reaches a request is.

use super::Xy;

pub(super) fn sub(a: Xy, b: Xy) -> Xy {
    [a[0] - b[0], a[1] - b[1]]
}

pub(super) fn add(a: Xy, b: Xy) -> Xy {
    [a[0] + b[0], a[1] + b[1]]
}

pub(super) fn scale(a: Xy, by: f64) -> Xy {
    [a[0] * by, a[1] * by]
}

pub(super) fn dot(a: Xy, b: Xy) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// The z of `a` × `b`: positive when `b` turns counter-clockwise from `a`.
pub(super) fn cross(a: Xy, b: Xy) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

pub(super) fn length(a: Xy) -> f64 {
    libm::hypot(a[0], a[1])
}

pub(super) fn distance(a: Xy, b: Xy) -> f64 {
    length(sub(a, b))
}

/// `a` at unit length; none for a vector of no length (or one that isn't finite).
pub(super) fn unit(a: Xy) -> Option<Xy> {
    let l = length(a);
    (l > 0.0 && l.is_finite()).then(|| [a[0] / l, a[1] / l])
}

/// `a` turned a quarter turn counter-clockwise.
pub(super) fn left(a: Xy) -> Xy {
    [-a[1], a[0]]
}

/// The angle between the directions `a` and `b`, in radians, from 0 to π.
pub(super) fn angle_between(a: Xy, b: Xy) -> f64 {
    libm::atan2(cross(a, b).abs(), dot(a, b))
}

/// The angle of `a` from +x, in (−π, π].
pub(super) fn angle_of(a: Xy) -> f64 {
    libm::atan2(a[1], a[0])
}

/// `point`'s foot on the segment from `from` to `to`, and how far it is from it.
pub(super) fn to_segment(from: Xy, to: Xy, point: Xy) -> (Xy, f64) {
    let d = sub(to, from);
    let span = dot(d, d);
    let t = if span > 0.0 {
        (dot(sub(point, from), d) / span).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let foot = add(from, scale(d, t));
    (foot, distance(foot, point))
}

/// Whether every coordinate is finite.
pub(super) fn finite(a: Xy) -> bool {
    a[0].is_finite() && a[1].is_finite()
}

/// The centre of the circle through `a`, `b`, and `c`; none for three points in a line (or
/// so nearly that the centre isn't finite).
pub(super) fn circumcentre(a: Xy, b: Xy, c: Xy) -> Option<Xy> {
    let (ab, ac) = (sub(b, a), sub(c, a));
    let d = 2.0 * cross(ab, ac);
    if d == 0.0 {
        return None;
    }
    let (ab2, ac2) = (dot(ab, ab), dot(ac, ac));
    let centre = [
        a[0] + (ac[1] * ab2 - ab[1] * ac2) / d,
        a[1] + (ab[0] * ac2 - ac[0] * ab2) / d,
    ];
    finite(centre).then_some(centre)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_circumcentre_of_a_right_triangle_is_its_hypotenuses_middle() {
        // Thales: the right angle at (0, 0), the hypotenuse from (4, 0) to (0, 2).
        assert_eq!(
            circumcentre([4.0, 0.0], [0.0, 2.0], [0.0, 0.0]),
            Some([2.0, 1.0])
        );
        assert_eq!(circumcentre([0.0, 0.0], [1.0, 1.0], [2.0, 2.0]), None);
    }

    #[test]
    fn a_point_beyond_a_segments_end_is_measured_to_that_end() {
        assert_eq!(
            to_segment([0.0, 0.0], [10.0, 0.0], [13.0, 4.0]),
            ([10.0, 0.0], 5.0)
        );
        assert_eq!(
            to_segment([0.0, 0.0], [10.0, 0.0], [4.0, -3.0]),
            ([4.0, 0.0], 3.0)
        );
    }
}
