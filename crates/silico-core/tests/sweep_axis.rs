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
        redundant: BTreeSet::new(),
        algorithm: silico_protocol::Algorithm::DogLeg,
    };
    (document, solution)
}

#[test]
fn an_axis_whose_solved_ends_coincide_exactly_is_refused() {
    for type_name in ["revolve", "groove"] {
        for (pa, pb) in [
            ([0.0, 3.0], [0.0, 3.0]),
            ([-7.25, 1e-300], [-7.25, 1e-300]),
            // -0 and 0 are one place.
            ([0.0, -0.0], [-0.0, 0.0]),
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
fn an_axis_whose_ends_are_an_ulp_apart_passes() {
    let next = |v: f64| f64::from_bits(v.to_bits() + 1);
    for (pa, pb) in [
        ([0.0, 3.0], [0.0, next(3.0)]),
        ([0.0, 3.0], [next(0.0), 3.0]),
        ([1e6, -1e6], [next(1e6), -1e6]),
        ([0.0, 0.0], [0.0, 10.0]),
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

    /// An axis line's ends pass exactly when they're at two places.
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
        prop_assert_eq!(result.is_ok(), x != bx || y != by);
    }
}
