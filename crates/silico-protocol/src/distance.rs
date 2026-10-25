//! The least distance between two elements ([`crate::Op::Distance`], M4.5;
//! docs/ports/measure.md): OCCT's `BRepExtrema_DistShapeShape` gives the distance and every
//! pair of points where it found it met, in its own traversal's order; the worker sends the
//! distance, one of those pairs, chosen by its points' coordinates ([`witness`]), so the
//! pair is the same whatever order OCCT finds them in, and how many there were.

use serde::{Deserialize, Serialize};

use crate::{ElementKind, ElementRef, Refusal};

/// What [`crate::Op::Distance`] found: the least distance between the two elements as
/// bounded (never their infinite carriers), a pair of points where it's met, `from` on the
/// first element and `to` on the second, and how many pairs OCCT found it met at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Distance {
    pub distance: f64,
    pub from: [f64; 3],
    pub to: [f64; 3],
    pub solutions: u32,
}

/// Why a distance between `first` and `second` can't be asked, if it can't: one is a
/// solid, which a distance isn't between in M4.5, or numbered 0, which numbers nothing
/// (elements are numbered from 1); the first's reason before the second's. The core's
/// request, the worker, and the fake kernel each refuse by it, in one set of words.
pub fn check_distance(first: ElementRef, second: ElementRef) -> Result<(), Refusal> {
    for (element, parameter) in [(first, "first"), (second, "second")] {
        let message = if element.kind == ElementKind::Solid {
            format!("{parameter} must be a face, an edge, or a vertex, not a solid")
        } else if element.index == 0 {
            format!("elements are numbered from 1, so {parameter} can't be 0")
        } else {
            continue;
        };
        return Err(Refusal::InvalidParameter {
            parameter: parameter.to_owned(),
            message,
        });
    }
    Ok(())
}

/// The pair of `pairs` (each from the first element to the second) whose two points, as an
/// unordered pair, come first by (x, y, z): each pair's lesser point first, then its
/// greater, each compared by x, then y, then z (`f64::total_cmp`, so the order is total).
/// A witness of where the least distance is met, not an identity: swapping the elements
/// swaps its two points and nothing else. None for no pairs.
pub fn witness(pairs: &[[[f64; 3]; 2]]) -> Option<[[f64; 3]; 2]> {
    pairs
        .iter()
        .copied()
        .min_by(|a, b| unordered(a).cmp_by(&unordered(b)))
}

/// The witness of `pairs` ([`witness`]) among those whose two points are `distance` apart
/// within `precision`: OCCT means to keep only those (`BRepExtrema_DistShapeShape` at its
/// default precision, 1e-7), but its filter skips the pair after each it drops, so one
/// further off can stay. Among all of them, if none is. None for no pairs.
pub fn witness_within(
    pairs: &[[[f64; 3]; 2]],
    distance: f64,
    precision: f64,
) -> Option<[[f64; 3]; 2]> {
    let apart = |[a, b]: &[[f64; 3]; 2]| {
        let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
    };
    let near: Vec<[[f64; 3]; 2]> = pairs
        .iter()
        .filter(|pair| (apart(pair) - distance).abs() <= precision)
        .copied()
        .collect();
    witness(if near.is_empty() { pairs } else { &near })
}

/// A pair's points, the lesser by (x, y, z) first.
fn unordered(pair: &[[f64; 3]; 2]) -> Ordered {
    let [a, b] = *pair;
    if by_coordinates(&a, &b).is_le() {
        Ordered(a, b)
    } else {
        Ordered(b, a)
    }
}

struct Ordered([f64; 3], [f64; 3]);

impl Ordered {
    fn cmp_by(&self, other: &Ordered) -> std::cmp::Ordering {
        by_coordinates(&self.0, &other.0).then_with(|| by_coordinates(&self.1, &other.1))
    }
}

fn by_coordinates(a: &[f64; 3], b: &[f64; 3]) -> std::cmp::Ordering {
    a[0].total_cmp(&b[0])
        .then_with(|| a[1].total_cmp(&b[1]))
        .then_with(|| a[2].total_cmp(&b[2]))
}

#[cfg(test)]
mod tests {
    use super::{check_distance, witness, witness_within};
    use crate::{ElementKind, ElementRef, Refusal};

    #[test]
    fn the_witness_is_the_pair_first_by_its_lesser_point_then_its_greater() {
        // A box's bottom and top 10 apart: OCCT finds the corners' pairs, in its order.
        let pairs = [
            [[10.0, 20.0, 0.0], [10.0, 20.0, 10.0]],
            [[0.0, 20.0, 0.0], [0.0, 20.0, 10.0]],
            [[0.0, 0.0, 0.0], [0.0, 0.0, 10.0]],
            [[10.0, 0.0, 0.0], [10.0, 0.0, 10.0]],
        ];
        assert_eq!(witness(&pairs), Some([[0.0, 0.0, 0.0], [0.0, 0.0, 10.0]]));
        // In any order, the same pair.
        let mut reversed = pairs;
        reversed.reverse();
        assert_eq!(witness(&reversed), witness(&pairs));
        // Swapped elements: each pair's points swapped, and so the witness's, nothing else.
        let swapped = pairs.map(|[a, b]| [b, a]);
        assert_eq!(witness(&swapped), Some([[0.0, 0.0, 10.0], [0.0, 0.0, 0.0]]));
    }

    #[test]
    fn the_first_elements_reason_comes_before_the_seconds_and_a_solid_before_its_number() {
        let refused = |parameter: &str, message: &str| {
            Err(Refusal::InvalidParameter {
                parameter: parameter.to_owned(),
                message: message.to_owned(),
            })
        };
        let solid = |index| ElementRef {
            kind: ElementKind::Solid,
            index,
        };
        let edge = ElementRef::edge;
        // Both wrong: the first is said.
        assert_eq!(
            check_distance(solid(1), edge(0)),
            refused(
                "first",
                "first must be a face, an edge, or a vertex, not a solid"
            )
        );
        assert_eq!(
            check_distance(edge(0), solid(1)),
            refused("first", "elements are numbered from 1, so first can't be 0")
        );
        // A solid numbered 0: refused as a solid.
        assert_eq!(
            check_distance(edge(1), solid(0)),
            refused(
                "second",
                "second must be a face, an edge, or a vertex, not a solid"
            )
        );
        assert_eq!(check_distance(edge(1), edge(1)), Ok(()));
    }

    #[test]
    fn a_pair_further_than_the_precision_from_the_distance_is_never_the_witness() {
        // As OCCT's filter can leave them: a pair 5e-7 further than the distance, kept
        // after one it dropped, sorting first by its points; and the pair at the distance.
        let far = [[0.0, 0.0, 0.0], [0.0, 0.0, 10.000_000_5]];
        let at = [[1.0, 0.0, 0.0], [1.0, 0.0, 10.0]];
        assert_eq!(witness(&[far, at]), Some(far));
        assert_eq!(witness_within(&[far, at], 10.0, 1e-7), Some(at));
        assert_eq!(witness_within(&[at, far], 10.0, 1e-7), Some(at));
        // One a hair under the precision off (10.0000001 is 9.99999994e-8 past 10 in f64)
        // is within it.
        let edge = [[0.0, 0.0, 0.0], [0.0, 0.0, 10.000_000_1]];
        assert_eq!(witness_within(&[edge, at], 10.0, 1e-7), Some(edge));
        // None within: the witness of them all, never nothing.
        assert_eq!(witness_within(&[far, at], 20.0, 1e-7), Some(far));
        assert_eq!(witness_within(&[], 10.0, 1e-7), None);
    }

    #[test]
    fn the_witness_is_chosen_by_each_pairs_whole_length() {
        // Off the axes, every coordinate of both points its own: (3, 4, 12) apart is 13;
        // (3, 4, 12.0000005) apart is 13.00000046, past 13 by more than the precision, and
        // sorts first.
        let at = [[1.0, 2.0, 3.0], [4.0, 6.0, 15.0]];
        let far = [[0.0, 2.0, 3.0], [3.0, 6.0, 15.000_000_5]];
        assert_eq!(witness(&[far, at]), Some(far));
        assert_eq!(witness_within(&[far, at], 13.0, 1e-7), Some(at));
        // Exactly the precision off is within it: 1e-7 apart is 1e-7 in f64, against a
        // distance of 0; the pair at 0 sorts after it.
        let edge = [[0.0, 0.0, 0.0], [0.0, 0.0, 1e-7]];
        let zero = [[1.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        assert_eq!(witness_within(&[edge, zero], 0.0, 1e-7), Some(edge));
        // Shorter than the distance by more than the precision (5e-7) is outside it too.
        let short = [[0.0, 0.0, 0.0], [0.0, 0.0, 9.999_999_5]];
        let ten = [[1.0, 0.0, 0.0], [1.0, 0.0, 10.0]];
        assert_eq!(witness(&[short, ten]), Some(short));
        assert_eq!(witness_within(&[short, ten], 10.0, 1e-7), Some(ten));
    }

    #[test]
    fn pairs_alike_in_their_lesser_point_are_told_apart_by_their_greater() {
        // Both pairs' lesser point is the origin; the one whose other point is first wins,
        // whichever element it's on.
        let pairs = [
            [[0.0, 0.0, 0.0], [0.0, 5.0, 0.0]],
            [[0.0, 3.0, 0.0], [0.0, 0.0, 0.0]],
        ];
        assert_eq!(witness(&pairs), Some([[0.0, 3.0, 0.0], [0.0, 0.0, 0.0]]));
        // y decides only where x ties, z only where y does too; -0 comes before 0.
        let pairs = [
            [[1.0, 0.0, 0.0], [9.0, 9.0, 9.0]],
            [[0.0, 1.0, 5.0], [9.0, 9.0, 9.0]],
            [[0.0, 1.0, -0.0], [9.0, 9.0, 9.0]],
            [[0.0, 1.0, 0.0], [9.0, 9.0, 9.0]],
        ];
        assert_eq!(witness(&pairs), Some([[0.0, 1.0, -0.0], [9.0, 9.0, 9.0]]));
        assert_eq!(witness(&[]), None);
    }
}
