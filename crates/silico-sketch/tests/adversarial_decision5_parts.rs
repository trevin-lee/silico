//! /break-it on decision 5 per part (Fable, 2026-10-21): ε = max(1e-8 mm, 1e-9 × E,
//! 1e-9 mm² / max(T, 1e-5 mm)), E and T taken over the part of the sketch the dropped row
//! is in: its unknowns joined by every row sent (kept, dropped, or an arc's rules), the
//! origin's, axes', and dimensions' values fixed and joining nothing
//! (`SketchSystem::parts`, the drag's). T is the least d + (r1 ± r2) over the part's
//! circle-to-circle tangencies, those whose centres are under 1e-7 apart (squared under
//! 1e-14) left out, and counts as 1e-5 at the least.
//!
//! Found by break-it on the sketch-wide rule: E and T were the whole sketch's, so a
//! tangency between two construction circles in a corner, sharing no unknown with a metre
//! line, let the line's repeated angle be 9.9e-3 mm off at its far end and "met" (T = 1.01e-7,
//! ε 9.9e-3), where the line's own part allows 1e-6. PlaneGCS solves parts apart
//! (`System::initSolution`), so a kept row's slack never crosses them. And two equal
//! circles tangent inside are a double root (the row is d² alone), where DogLeg stops at the
//! first d under 1e-5, wherever that is from the start: T = d made ε 1.25e-4 to 1.82e-4 by
//! where the circle was drawn. A kept tangency leaves |d − (r1 ± r2)| within min(T,
//! 1e-10 / T) ≤ 1e-5 whatever T is, so T counts as 1e-5 at the least: ε at most 1e-4.
//!
//! Every case's dropped row is an `angle` from x_axis to a 1,000 mm line also held
//! `horizontal`: its gap is the angle in radians times 1,000 (the line's length, the only
//! lever; x_axis has none), a hand calculation.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::fixture;
use super::saves;

use fixture::{arc, circle_as, line, point};
use proptest::prelude::*;
use saves::{k, solved, text, worker};
use silico_core::ErrorCode;
use silico_core::document::{Checked, ElementId};
use silico_core::sketch::{SketchSolution, SketchWorker};

fn checked(text: &str) -> Checked {
    saves::checked(text).unwrap_or_else(|e| panic!("{e}"))
}

fn fix(id: usize, p: &str, x: f64, y: f64) -> [String; 2] {
    [
        k(
            id,
            &format!(
                r#""type": "distance_x", "a": "origin", "b": "{p}", "value": {x}, "driving": true"#
            ),
        ),
        k(
            id + 1,
            &format!(
                r#""type": "distance_y", "a": "origin", "b": "{p}", "value": {y}, "driving": true"#
            ),
        ),
    ]
}

/// The metre line: n0000003 from p0000005, held at (0, 500) (k1, k2), to p0000006 at
/// (1000, 500), horizontal (k3), 1,000 long (k4), and at `degrees` to x_axis (k5, the row
/// PlaneGCS drops). Its gap is `degrees` in radians × 1000.
fn metre_line(degrees: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let points = vec![
        point("p0000005", 0.0, 500.0),
        point("p0000006", 1000.0, 500.0),
    ];
    let mut constraints = fix(1, "p0000005", 0.0, 500.0).to_vec();
    constraints.push(k(3, r#""type": "horizontal", "line": "n0000003""#));
    constraints.push(k(
        4,
        r#""type": "length", "line": "n0000003", "value": 1000, "driving": true"#,
    ));
    constraints.push(k(
        5,
        &format!(
            r#""type": "angle", "a": "x_axis", "b": "n0000003", "value": {degrees}, "driving": true"#
        ),
    ));
    let lines = vec![line("n0000003", "p0000005", "p0000006")];
    (points, lines, constraints)
}

/// The metre line, and in their own corner, sharing nothing with it, two construction
/// circles of radius 10: c0000001 about p0000001, held at the origin, its radius held (k6
/// to k8), and c0000002 about p0000002, drawn at (`x`, 0) and held on y = 0 (k9), tangent
/// to c0000001 inside (k10), and its radius held too if `both` (k11).
fn metre_line_beside_circles(degrees: &str, x: f64, both: bool) -> Checked {
    let (mut points, lines, mut c) = metre_line(degrees);
    points.push(point("p0000001", 0.0, 0.0));
    points.push(point("p0000002", x, 0.0));
    c.extend(fix(6, "p0000001", 0.0, 0.0));
    c.push(k(
        8,
        r#""type": "radius", "curve": "c0000001", "value": 10, "driving": true"#,
    ));
    c.push(k(
        9,
        r#""type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true"#,
    ));
    c.push(k(
        10,
        r#""type": "tangent", "a": "c0000001", "b": "c0000002", "side": "inside""#,
    ));
    if both {
        c.push(k(
            11,
            r#""type": "radius", "curve": "c0000002", "value": 10, "driving": true"#,
        ));
    }
    let circles = [
        circle_as("c0000001", "p0000001", 10.0, true),
        circle_as("c0000002", "p0000002", 10.0, true),
    ];
    checked(&text(&points, &lines, &circles, &[], &c))
}

fn gap_of(s: &SketchSolution, id: &str) -> f64 {
    s.redundant[&ElementId::new(id).unwrap()]
}

fn at(s: &SketchSolution, id: &str) -> [f64; 2] {
    s.point(&ElementId::new(id).unwrap()).unwrap()
}

/// 0.000567° in radians × 1000: the metre line's far end off by 9.896e-3 mm.
const NEAR_TEN_MICROMETRES: &str = "0.000567";
/// 0.000008594° × 1000: 1.4999e-4 mm.
const FIFTEEN_HUNDREDTHS_OF_A_MICROMETRE: &str = "0.000008594";

// --- The baseline: the metre line alone --------------------------------------------------

#[test]
fn a_metre_lines_angle_a_hundredth_of_a_millimetre_off_conflicts_alone() {
    // E = 1000 (the line's ends), ε = 1e-6: the far end 9.9e-3 off is refused, naming the
    // angle, and so is 1.5e-4.
    let mut w = worker();
    for degrees in [NEAR_TEN_MICROMETRES, FIFTEEN_HUNDREDTHS_OF_A_MICROMETRE] {
        let (points, lines, c) = metre_line(degrees);
        let e = solved(&checked(&text(&points, &lines, &[], &[], &c)), &mut w).unwrap_err();
        assert_eq!(
            (e.code(), e.elements()),
            (ErrorCode::OverConstrained, &["k0000005".to_owned()][..]),
            "{degrees}: {e}"
        );
        assert!(
            e.message()
                .contains("(k0000005), past the 1.0000000000000002e-6 mm"),
            "{e}"
        );
        assert!(e.message().contains("of the part's 1e3 mm extent"), "{e}");
    }
}

// --- Two construction circles about 100 nm apart, in another part -------------------------

/// The refusal every case of the metre line beside circles of another part gives: the
/// angle named, against the line's own part, (0, 500) to (1000, 500), with no tangency.
fn refused_by_the_lines_part(e: &silico_core::Error, what: &str) {
    assert_eq!(
        (e.code(), e.elements()),
        (ErrorCode::OverConstrained, &["k0000005".to_owned()][..]),
        "{what}: {e}"
    );
    assert!(
        e.message().contains("past the 1.0000000000000002e-6 mm"),
        "{what}: {e}"
    );
    assert!(
        e.message().contains("of the part's 1e3 mm extent"),
        "{what}: {e}"
    );
    assert!(!e.message().contains("tangency"), "{what}: {e}");
}

#[test]
fn construction_circles_about_100_nm_apart_leave_the_metre_line_refused() {
    // Found by break-it, fixed by parts: the circles' centres 0.99e-7 apart (squared 9.8e-15,
    // PlaneGCS's millimetre form, no T) or 1.01e-7 (squared 1.02e-14, the mm² form, T 1.01e-7
    // and, sketch-wide, ε 9.9e-3, which kept the angle 9.896e-3 off). The circles share no
    // unknown with the line: the angle's part is the line's two points, E 1000, ε 1e-6, from
    // both drawings, with c2's radius held and free.
    let mut w = worker();
    for x in [0.99e-7, 1.01e-7] {
        for both in [false, true] {
            let doc = metre_line_beside_circles(NEAR_TEN_MICROMETRES, x, both);
            let e = solved(&doc, &mut w).unwrap_err();
            refused_by_the_lines_part(&e, &format!("{x:e}, both radii held: {both}"));
        }
    }
}

#[test]
fn an_arc_101_nm_off_its_circles_centre_leaves_the_metre_line_refused() {
    // As above, with an arc for the second circle (a quarter about (1.01e-7, 0) from its
    // east point to its north one): its rules join its nine values, but nothing joins them
    // to the line. Refused, naming k0000005, against the line's part.
    let x = 1.01e-7;
    let (mut points, lines, mut c) = metre_line(NEAR_TEN_MICROMETRES);
    points.push(point("p0000001", 0.0, 0.0));
    points.push(point("p0000002", x, 0.0));
    points.push(point("p0000003", x + 10.0, 0.0));
    points.push(point("p0000004", x, 10.0));
    c.extend(fix(6, "p0000001", 0.0, 0.0));
    c.push(k(
        8,
        r#""type": "radius", "curve": "c0000001", "value": 10, "driving": true"#,
    ));
    c.push(k(
        9,
        r#""type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true"#,
    ));
    c.push(k(
        10,
        r#""type": "tangent", "a": "c0000001", "b": "a0000001", "side": "inside""#,
    ));
    let doc = checked(&text(
        &points,
        &lines,
        &[circle_as("c0000001", "p0000001", 10.0, true)],
        &[arc("a0000001", "p0000002", "p0000003", "p0000004")],
        &c,
    ));
    let mut w = worker();
    let e = solved(&doc, &mut w).unwrap_err();
    refused_by_the_lines_part(&e, "the arc");
}

// --- Equal circles tangent inside: T is where the solver stopped ----------------------------

#[test]
fn equal_circles_tangent_inside_in_another_part_leave_the_metre_line_refused_from_every_start() {
    // Found by break-it, fixed by parts: two construction circles of radius 10 held tangent
    // inside, the second drawn at (x0, 0). The row is d² − 0², a double root, so DogLeg
    // stops at the first d with d² ≤ 1e-10: 5.5e-6 from x0 = 0.3, 8.0e-6 from 0.65, 7.0e-6
    // from 1, and sketch-wide, ε = 1e-9 / d kept the angle 1.4999e-4 off from 0.3 alone.
    // The angle shares nothing with the circles: ε 1e-6, refused from every start.
    let mut w = worker();
    for x0 in [0.3, 0.65, 1.0] {
        let doc = metre_line_beside_circles(FIFTEEN_HUNDREDTHS_OF_A_MICROMETRE, x0, true);
        let e = solved(&doc, &mut w).unwrap_err();
        refused_by_the_lines_part(&e, &format!("x0 {x0}"));
    }
}

/// The metre line, and joined to it, two construction circles of radius 10: c0000001
/// about p0000005, the line's start (0, 500), its radius held (k6), and c0000002 about
/// p0000002, drawn at (`x0`, 500) and held on y = 500 (k7), tangent to c0000001 inside
/// (k8), and its radius held too if `both` (k9). Every point of the part is on y = 500 and
/// within x 0 to 1000: E = 1000.
fn metre_line_joined_to_circles(degrees: &str, x0: f64, both: bool) -> Checked {
    let (mut points, lines, mut c) = metre_line(degrees);
    points.push(point("p0000002", x0, 500.0));
    c.push(k(
        6,
        r#""type": "radius", "curve": "c0000001", "value": 10, "driving": true"#,
    ));
    c.push(k(
        7,
        r#""type": "distance_y", "a": "origin", "b": "p0000002", "value": 500, "driving": true"#,
    ));
    c.push(k(
        8,
        r#""type": "tangent", "a": "c0000001", "b": "c0000002", "side": "inside""#,
    ));
    if both {
        c.push(k(
            9,
            r#""type": "radius", "curve": "c0000002", "value": 10, "driving": true"#,
        ));
    }
    let circles = [
        circle_as("c0000001", "p0000005", 10.0, true),
        circle_as("c0000002", "p0000002", 10.0, true),
    ];
    checked(&text(&points, &lines, &circles, &[], &c))
}

/// Where DogLeg left the double root: the centres' distance, d, on the solution, checked
/// to be the mm² form's landing (d² within the solver's 1e-10, and d at least the 1e-7
/// cutoff, so T counts and is floored to 1e-5, not skipped).
fn landed(s: &SketchSolution) -> f64 {
    let c1 = at(s, "p0000005");
    let c2 = at(s, "p0000002");
    let (dx, dy) = (c2[0] - c1[0], c2[1] - c1[1]);
    let d = (dx * dx + dy * dy).sqrt();
    assert!(d * d <= 1e-10 && d >= 1e-7, "d {d:e}");
    d
}

#[test]
fn equal_circles_tangent_inside_joined_to_the_metre_line_give_a_tenth_of_a_micrometre_from_every_start()
 {
    // The same circles joined to the line (c1 about its start): the part is the line and
    // both circles, E 1000, and T = d, where DogLeg stopped, under 1e-5 from every start,
    // so it counts as 1e-5: ε = 1e-9 / 1e-5 = 1e-4 from every start. The angle 5.157e-6°
    // (a gap of 9.0007e-5 by hand) is kept, its gap reported; 6.303e-6° (1.10008e-4) and
    // 8.594e-6° (1.4999e-4, which the uncapped 1e-9 / d kept from x0 = 0.3) are refused,
    // naming the angle. With c2's radius free, its gradient at r2 = r1 is 0, so it stays;
    // 1.4999e-4 is refused from every start too.
    let mut w = worker();
    for x0 in [0.3, 0.65, 1.0] {
        let s = solved(
            &metre_line_joined_to_circles("0.000005157", x0, true),
            &mut w,
        )
        .unwrap_or_else(|e| panic!("x0 {x0}: {e}"));
        let d = landed(&s);
        let gap = gap_of(&s, "k0000005");
        assert!(
            (9.000e-5..=9.001e-5).contains(&gap),
            "x0 {x0}, d {d:e}: {gap:e}"
        );
        for (degrees, both) in [
            ("0.000006303", true),
            (FIFTEEN_HUNDREDTHS_OF_A_MICROMETRE, true),
            (FIFTEEN_HUNDREDTHS_OF_A_MICROMETRE, false),
        ] {
            let what = format!("x0 {x0}, {degrees}°, both radii held: {both}");
            let e = solved(&metre_line_joined_to_circles(degrees, x0, both), &mut w).unwrap_err();
            assert_eq!(
                (e.code(), e.elements()),
                (ErrorCode::OverConstrained, &["k0000005".to_owned()][..]),
                "{what}: {e}"
            );
            assert!(e.message().contains("past the 1e-4 mm"), "{what}: {e}");
            assert!(
                e.message().contains("(1e-5 mm at the least)"),
                "{what}: {e}"
            );
            assert!(
                e.message().contains("of the part's 1e3 mm extent"),
                "{what}: {e}"
            );
        }
    }
}

#[test]
fn equal_circles_tangent_inside_land_a_few_nanometres_apart_and_stay_on_load() {
    // The landing itself (passes): d² ≤ 1e-10 and the save's load takes no further step
    // (d² is already under the convergence), so the save and the load agree. Hand: from
    // x0 = 1e-5, one halving: 5e-6 exactly.
    let mut w = worker();
    let doc = metre_line_beside_circles("0", 1e-5, true);
    let s = solved(&doc, &mut w).unwrap();
    assert_eq!(
        s.point(&ElementId::new("p0000002").unwrap()),
        Some([5e-6, 0.0])
    );
    let l = solved(&saves::written_back(&doc, &s), &mut w).unwrap();
    assert_eq!(l.points, s.points);
}

// --- Near the cutoff, every side, every scale: Ok means tangent -----------------------------

thread_local!(static W: std::cell::RefCell<SketchWorker> = std::cell::RefCell::new(worker()));

fn near_cutoff() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0),
        Just(1e-7),
        Just(0.99e-7),
        Just(1.01e-7),
        (0.9e-7f64..1.1e-7),
        (0.0f64..1e-6),
        (1e-6f64..1e-3),
        (1e-3f64..30.0),
    ]
}

fn radius() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(10.0),
        Just(10.000001),
        Just(9.999999),
        Just(0.000001),
        Just(1e-9),
        (1e-6f64..20.0),
    ]
}

/// Circle c1 of radius 10 at the origin, held; c2 of radius `r2` (held) drawn at (`x`, 0)
/// and held on y = 0; tangent outside if `outside`, else inside.
fn two_circles(x: f64, r2: f64, outside: bool) -> Checked {
    let points = [point("p0000001", 0.0, 0.0), point("p0000002", x, 0.0)];
    let mut c = fix(1, "p0000001", 0.0, 0.0).to_vec();
    c.push(k(
        3,
        r#""type": "radius", "curve": "c0000001", "value": 10, "driving": true"#,
    ));
    c.push(k(
        4,
        &format!(r#""type": "radius", "curve": "c0000002", "value": {r2}, "driving": true"#),
    ));
    c.push(k(
        5,
        r#""type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true"#,
    ));
    let side = if outside { "outside" } else { "inside" };
    c.push(k(
        6,
        &format!(r#""type": "tangent", "a": "c0000001", "b": "c0000002", "side": "{side}""#),
    ));
    checked(&text(
        &points,
        &[],
        &[
            circle_as("c0000001", "p0000001", 10.0, true),
            circle_as("c0000002", "p0000002", r2, true),
        ],
        &[],
        &c,
    ))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, failure_persistence: None, ..ProptestConfig::default() })]

    /// Whatever the drawing, near the cutoff or not, tiny or equal radii, inside or outside:
    /// a solve is the same twice; an Ok's kept tangency really holds (outside: centres apart
    /// and d within 1e-6 of r1 + r2; inside: d² − (r1 − r2)² within 1e-9 mm², or radii
    /// equal within 1e-9 where PlaneGCS measures r1 − r2); and a refusal is a structured
    /// one naming elements.
    #[test]
    fn a_tangency_near_the_concentric_cutoff_is_held_or_refused_by_name(
        x in near_cutoff(),
        r2 in radius(),
        outside in any::<bool>(),
    ) {
        // The radius must be a whole number of nanometres to be held by a dimension.
        let r2 = (r2 * 1e6).round().max(1.0) / 1e6;
        let doc = two_circles(x, r2, outside);
        let first = W.with(|w| solved(&doc, &mut *w.borrow_mut()));
        let again = W.with(|w| solved(&doc, &mut *w.borrow_mut()));
        match (&first, &again) {
            (Ok(a), Ok(b)) => prop_assert_eq!(saves::said(a), saves::said(b)),
            (Err(a), Err(b)) => prop_assert_eq!(a.to_string(), b.to_string()),
            _ => prop_assert!(false, "differs: {:?} / {:?}", first.is_ok(), again.is_ok()),
        }
        match first {
            Ok(s) => {
                let c = s.point(&ElementId::new("p0000002").unwrap()).unwrap();
                let r = s.radius(&ElementId::new("c0000002").unwrap()).unwrap();
                let d = (c[0] * c[0] + c[1] * c[1]).sqrt();
                if outside {
                    prop_assert!(d * d >= 1e-14, "outside, concentric: d {:e}", d);
                    prop_assert!((d - (10.0 + r)).abs() <= 1e-6,
                        "outside, not touching: d {:e}, r2 {:e}", d, r);
                } else if d * d >= 1e-14 {
                    prop_assert!((d * d - (10.0 - r) * (10.0 - r)).abs() <= 1e-9,
                        "inside, not touching: d {:e}, r2 {:e}", d, r);
                } else {
                    prop_assert!((10.0 - r).abs() <= 1e-9, "inside, concentric: r2 {:e}", r);
                }
            }
            Err(e) => {
                prop_assert!(
                    matches!(e.code(), ErrorCode::OverConstrained | ErrorCode::InvalidResult
                        | ErrorCode::SolveFailed),
                    "{:?} {}", e.code(), e
                );
                prop_assert!(!e.elements().is_empty() || e.code() == ErrorCode::SolveFailed,
                    "no elements: {}", e);
            }
        }
    }
}
