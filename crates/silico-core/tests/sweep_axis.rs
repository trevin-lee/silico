//! M3.6: a revolve's or groove's axis of no length (docs/design/m3.6-sketch-features.md,
//! S6), refused from the solved sketch by a pure function of the checked document and the
//! solution. Solutions are made here, so each case is exact: ends at one point, an ulp
//! apart, and the sketch's own axes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/sweeps.rs"]
mod sweeps;

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::document::{Checked, Coordinate, ElementId};
use silico_core::sketch::SketchSolution;
use silico_core::sweep::check_axis_length;
use sweeps::*;

/// The PROFILE sketch with `revolve` about `axis`, checked, and a solution putting
/// pa000000 and pb000000 (na000000's ends) where given.
fn axis_case(type_name: &str, axis: &str, pa: [f64; 2], pb: [f64; 2]) -> (Checked, SketchSolution) {
    let base = (type_name == "groove").then_some("k7f2qx3m");
    let document = checked_document(&[
        PROFILE,
        &block("k7f2qx3m"),
        &turned("r0000001", type_name, base, "s0000003", axis, TURN),
    ]);
    let at = |[x, y]: [f64; 2]| [Coordinate::new(x).unwrap(), Coordinate::new(y).unwrap()];
    let mut points = BTreeMap::new();
    for (point, xy) in [
        ("pa000000", pa),
        ("pb000000", pb),
        ("pc000000", [5.0, 5.0]),
        ("pd000000", [6.0, 5.0]),
        ("pe000000", [5.0, 6.0]),
    ] {
        points.insert(ElementId::new(point).unwrap(), at(xy));
    }
    let solution = SketchSolution {
        points,
        radii: BTreeMap::new(),
        references: BTreeMap::new(),
        dofs: 0,
        free: BTreeSet::new(),
        redundant: BTreeMap::new(),
        algorithm: silico_protocol::Algorithm::DogLeg,
    };
    (document, solution)
}

#[test]
fn an_axis_of_no_length_as_the_solver_measures_it_is_refused() {
    // No length is M3.2's zero-size rule: dx² + dy² is exactly 0. Ends at one place, and
    // ends so close their squared distance underflows to 0 (5e-324² and 1e-162² are both 0
    // in doubles, worked by hand: 1e-324 is below half the least subnormal, 4.9e-324), where
    // OCCT's direction would throw.
    for type_name in ["revolve", "groove"] {
        for (pa, pb) in [
            ([0.0, 3.0], [0.0, 3.0]),
            ([-7.25, 1e-300], [-7.25, 1e-300]),
            // -0 and 0 are one place.
            ([0.0, -0.0], [-0.0, 0.0]),
            // One subnormal step apart, either way.
            ([0.0, 0.0], [5e-324, 0.0]),
            ([0.0, 0.0], [0.0, -5e-324]),
            ([0.0, 3.0], [f64::from_bits(1), 3.0]),
            // Apart in both, each square underflowing.
            ([0.0, 0.0], [1e-162, 1e-162]),
            ([3.0, 1e-200], [3.0, 0.0]),
        ] {
            let (document, solution) = axis_case(type_name, "na000000", pa, pb);
            let error = check_axis_length(&document, &id("r0000001"), &solution).unwrap_err();
            assert_eq!(error.code(), ErrorCode::InvalidInput);
            assert_eq!(error.feature(), Some("r0000001"));
            assert_eq!(error.parameter(), Some("axis"));
            assert_eq!(
                error.message(),
                "axis na000000, a line of sketch s0000003, has its ends at one point: it gives no direction to turn about"
            );
            assert_eq!(
                error.suggestion(),
                Some("give the line some length, or turn about another line")
            );
            assert!(error.elements().is_empty());
        }
    }
}

#[test]
fn an_axis_whose_squared_length_isnt_0_passes_however_short() {
    // An ulp apart away from 0 squares to a normal number; 1e-154 apart squares to 1e-308,
    // subnormal (below 2.2250738585072014e-308) but not 0; 1.5e-154 to 2.25e-308, normal;
    // 2e-162 to 4e-324, which rounds up to the least subnormal, 4.9e-324.
    let next = |v: f64| f64::from_bits(v.to_bits() + 1);
    for (pa, pb) in [
        ([0.0, 3.0], [0.0, next(3.0)]),
        ([1e6, -1e6], [next(1e6), -1e6]),
        ([0.0, 0.0], [0.0, 10.0]),
        ([0.0, 0.0], [1e-154, 0.0]),
        ([0.0, 0.0], [0.0, 1.5e-154]),
        ([0.0, 0.0], [2e-162, 0.0]),
    ] {
        let (document, solution) = axis_case("revolve", "na000000", pa, pb);
        assert_eq!(
            check_axis_length(&document, &id("r0000001"), &solution),
            Ok(()),
            "{pa:?} {pb:?}"
        );
    }
}

#[test]
fn the_sketchs_axes_are_never_refused() {
    // Whatever the solution holds, even nothing at all.
    for axis in ["x_axis", "y_axis"] {
        for type_name in ["revolve", "groove"] {
            let (document, mut solution) = axis_case(type_name, axis, [0.0, 0.0], [0.0, 0.0]);
            assert_eq!(
                check_axis_length(&document, &id("r0000001"), &solution),
                Ok(())
            );
            solution.points.clear();
            assert_eq!(
                check_axis_length(&document, &id("r0000001"), &solution),
                Ok(())
            );
        }
    }
}

#[test]
fn checking_the_axis_of_what_has_none_says_so() {
    let (document, solution) = axis_case("revolve", "na000000", [0.0, 0.0], [0.0, 1.0]);
    let error = check_axis_length(&document, &id("k7f2qx3m"), &solution).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(
        error.message(),
        "feature k7f2qx3m isn't a revolve or groove: it has no axis"
    );
    let error = check_axis_length(&document, &id("zzzzzzzz"), &solution).unwrap_err();
    assert_eq!(error.message(), "the document has no feature zzzzzzzz");
    // A solution without the line's ends isn't this sketch's: Silico's bug, said so.
    let mut solution = solution;
    solution.points.remove(&ElementId::new("pb000000").unwrap());
    let error = check_axis_length(&document, &id("r0000001"), &solution).unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal);
}

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 300,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0xa_c1e5),
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(config())]

    /// An axis line's ends pass exactly when their squared distance isn't 0. Here the ends
    /// are a few ulps apart in each coordinate: near 0 that's a few multiples of 5e-324,
    /// whose squares are 0; anywhere else in ±1e6 it's at least an ulp of a normal number
    /// far above 1e-161, whose square isn't.
    #[test]
    fn an_axis_passes_exactly_when_its_ends_differ(
        x in prop_oneof![Just(0.0), Just(-0.0), -1e6f64..1e6],
        y in prop_oneof![Just(0.0), Just(-0.0), -1e6f64..1e6],
        dx in prop_oneof![Just(0i64), Just(1), Just(-1), -3i64..3],
        dy in prop_oneof![Just(0i64), Just(1), Just(-1), -3i64..3],
    ) {
        // `steps` ulps from v, through zero as the next float either way.
        let step = |v: f64, steps: i64| {
            let mut v = v;
            for _ in 0..steps.unsigned_abs() {
                v = if steps > 0 { v.next_up() } else { v.next_down() };
            }
            v
        };
        let (bx, by) = (step(x, dx), step(y, dy));
        let (document, solution) = axis_case("revolve", "na000000", [x, y], [bx, by]);
        let result = check_axis_length(&document, &id("r0000001"), &solution);
        let apart = (bx - x).abs() > 1e-161 || (by - y).abs() > 1e-161;
        prop_assert_eq!(result.is_ok(), apart, "{:?} {:?}", [x, y], [bx, by]);
    }
}
