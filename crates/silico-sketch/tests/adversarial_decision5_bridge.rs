//! /break-it on decision 5 per part, through the real sketch worker: a dropped row that
//! bridges two parts PlaneGCS solves apart. PlaneGCS solves the kept rows only
//! (`System::initSolution` partitions `clistR`, the rows less the redundant ones), so two
//! points each held from the origin alone are two parts to it, each solved exactly by its
//! linear rows. Silico joins them by the dropped row too (`SketchSystem::parts`: every row
//! sent joins, so which twin PlaneGCS drops changes nothing), and the bridged box spans the
//! distance between the two parts, which no kept row carries a direction along.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/saves.rs"]
mod saves;

use saves::{k, point, solved, text, worker};
use silico_core::ErrorCode;

fn checked(text: &str) -> silico_core::document::Checked {
    saves::checked(text).unwrap_or_else(|e| panic!("{e}"))
}

/// p1 held at the origin (k1, k2), p2 held at (0, `y`) (k3, k4), and a horizontal
/// distance from p1 to p2 of 5 nm, 5e-6 mm (k5): three rows on two x's, one of them 5 nm
/// from the other two. PlaneGCS calls one redundant (its error squared, 2.5e-11, is under
/// `convergenceRedundant`, 1e-10), not conflicting.
fn bridged(y: f64) -> silico_core::document::Checked {
    let points = [point("p0000001", 0.0, 0.0), point("p0000002", 0.0, y)];
    let constraints = [
        k(
            1,
            r#""type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true"#,
        ),
        k(
            2,
            r#""type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true"#,
        ),
        k(
            3,
            r#""type": "distance_x", "a": "origin", "b": "p0000002", "value": 0, "driving": true"#,
        ),
        k(
            4,
            &format!(
                r#""type": "distance_y", "a": "origin", "b": "p0000002", "value": {y}, "driving": true"#
            ),
        ),
        k(
            5,
            r#""type": "distance_x", "a": "p0000001", "b": "p0000002", "value": 0.000005, "driving": true"#,
        ),
    ];
    checked(&text(&points, &[], &[], &[], &constraints))
}

#[test]
fn a_5_nm_disagreement_bridging_two_rigid_points_ten_metres_apart_is_kept() {
    // SPEC: p2 at (0, 10000). The three x rows disagree by 5 nm; every row is linear, so
    // each of PlaneGCS's parts (p1's, p2's: the kept rows') is solved exactly and holds no
    // slack, and E for either is 0 (ε 1e-8 mm). The bridged part's box is (0, 0) to (0,
    // 10000), E 1e4, ε 1e-5: the 5 nm disagreement is kept, its gap reported. Under the same
    // rows with p2 at (0, 1) (the next test) it's refused. Decision 5's own reason for the
    // extent term (a direction's convergence carried along a lever) doesn't arise: no row
    // here has a direction. This is decision 5 as Fable decided it, per part
    // (docs/design/m3.3-solving.md): the dropped row joins its parts as a kept one would,
    // so the verdict doesn't hang on which twin PlaneGCS drops, and two sides' sum stays
    // within the union's window, never a refusal PlaneGCS's split would have avoided. Pinned
    // as SPEC for the M3 review.
    let mut w = worker();
    let s = solved(&bridged(10000.0), &mut w).unwrap_or_else(|e| panic!("{e}"));
    let gaps: Vec<f64> = s.redundant.values().copied().collect();
    assert_eq!(gaps.len(), 1, "{:?}", s.redundant);
    assert!((gaps[0] - 5e-6).abs() < 1e-12, "{gaps:?}");
}

#[test]
fn the_same_disagreement_bridging_points_a_millimetre_apart_is_refused() {
    // p2 at (0, 1): the bridged box is 1 mm, ε 1e-8, and the 5 nm is refused.
    let mut w = worker();
    let e = solved(&bridged(1.0), &mut w).unwrap_err();
    assert_eq!(e.code(), ErrorCode::OverConstrained, "{e}");
    assert!(e.message().contains("by up to 5"), "{e}");
    assert!(e.message().contains("past the 1e-8 mm"), "{e}");
}
