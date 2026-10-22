//! /break-it on decision 5's tolerance: a redundant constraint is kept when its gap (the
//! least distance a point must move for it to hold; an angle carried by the smaller of its
//! own curves' levers) is within ε = max(1e-8 mm, 1e-9 × E, 1e-9 mm² / max(T, 1e-5 mm)),
//! E the extent of the part of the sketch it's in, the diagonal of the box around the
//! part's points on the solution (Fable, 2026-10-20), and T the part's tightest
//! circle-to-circle tangency (Fable, 2026-10-21); a part is the sketch's unknowns joined by
//! every row sent (Fable, 2026-10-21, adversarial_decision5_parts.rs). And the load path
//! every solve takes: what a save keeps must be what its load gives.
//!
//! Found on the rule's first scale, the sketch's longest line or radius: that measured the
//! sketch's curves, not where its geometry is, so a gap a direction carries past a short
//! line's ends (a point on its extension, a circle tangent to it far along, two points
//! mirrored about it far off), which grows with the distance, was refused against 1e-8 mm
//! though FreeCAD solves the sketch. The extent covers every such distance. And it's the
//! extent of the constraint's part of the sketch: geometry no row joins to it (a
//! construction line a kilometre away, unconstrained) doesn't count, since no slack reaches
//! the constraint from there; geometry joined to it does, however far, so within a part how
//! far two dimensions may disagree still depends on what else is drawn, loud, with the gap
//! reported.
//!
//! Found next, fixed by T: PlaneGCS measures a tangency between circles or arcs in mm², as
//! d² − (r1 ± r2)² = (d − (r1 ± r2)) × T, T = d + (r1 ± r2), so a solve that holds it within
//! 1e-10 mm² holds the circles only within 1e-10 / T mm of tangent, and two circles of
//! radius 100 and 100.001 tangent inside (T 2 µm, slack 5e-8 mm) left a dimension
//! repeating their centres' distance 1.2e-8 off, refused against the floor though FreeCAD
//! solves the sketch. The extent can't bound it: the slack is on the row itself, in mm²,
//! and grows as T shrinks. ε's third term is ten times it, over every circle tangency in
//! the sketch, kept or dropped, except those PlaneGCS measures in millimetres, their
//! centres under 1e-7 apart. Those cases are drawn so the solver takes no step at all
//! (DogLeg returns a start whose rows are within 1e-10 untouched), so their gaps are hand
//! values and the solution is the drawing, bit for bit.
//!
//! Every case's constraints agree exactly or disagree by a known amount: Pythagorean
//! directions (3, 4, 5) and nanometre values, so the expected answer is a hand calculation.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../silico-core/tests/common/sketch.rs"]
mod fixture;
#[path = "common/saves.rs"]
mod saves;

use fixture::{circle, line, line_as, point};
use proptest::prelude::*;
use saves::{k, solved, text, worker, written_back};
use silico_core::ErrorCode;
use silico_core::document::{Checked, ElementId};
use silico_core::sketch::{SketchSolution, SketchSolver, SketchWorker};

/// `text`, read and gated.
fn checked(text: &str) -> Checked {
    saves::checked(text).unwrap_or_else(|e| panic!("{e}"))
}

fn at(s: &SketchSolution, id: &str) -> [f64; 2] {
    s.point(&ElementId::new(id).unwrap()).unwrap()
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

/// A 5 mm line n0000001 from p0000001, held at (1, 1), to p0000002 at (4, 5): its length and
/// its run (3) fix it, its rise (4) only through the length's square root. `jitter` is how
/// far p0000002 is drawn off (x, y).
fn short_line(jitter: [f64; 2]) -> (Vec<String>, Vec<String>, Vec<String>) {
    let points = vec![
        point("p0000001", 1.0, 1.0),
        point("p0000002", 4.0 + jitter[0], 5.0 + jitter[1]),
    ];
    let lines = vec![line("n0000001", "p0000001", "p0000002")];
    let mut constraints = fix(1, "p0000001", 1.0, 1.0).to_vec();
    constraints.push(k(
        3,
        r#""type": "length", "line": "n0000001", "value": 5, "driving": true"#,
    ));
    constraints.push(k(
        4,
        r#""type": "distance_x", "a": "p0000001", "b": "p0000002", "value": 3, "driving": true"#,
    ));
    (points, lines, constraints)
}

/// What a solve of `doc` gives, then what its load gives (solved, written through the file,
/// read, solved again): both must be Ok, at the same bits.
fn save_and_load(doc: &Checked, w: &mut dyn SketchSolver) -> Result<SketchSolution, String> {
    let s = solved(doc, w).map_err(|e| {
        format!(
            "refused on save: {:?} {e} / elements {:?}",
            e.code(),
            e.elements()
        )
    })?;
    let l = solved(&written_back(doc, &s), w).map_err(|e| {
        format!(
            "solved on save, refused on load: {:?} {e} / elements {:?}",
            e.code(),
            e.elements()
        )
    })?;
    if l.points != s.points || l.redundant.keys().ne(s.redundant.keys()) {
        return Err(format!(
            "the load differs: saved {:?} {:?}, loaded {:?} {:?}",
            s.points, s.redundant, l.points, l.redundant
        ));
    }
    Ok(s)
}

// --- A far point on a short line's extension ------------------------------------------------

/// The short line, and p0000003 at (600001, 800001): on the line's extension exactly
/// ((1, 1) + 200000 × (3, 4)), held there by its own x and y, and constrained onto the line
/// too. Four constraints for three unknowns' worth of freedom: one is redundant, and all
/// agree exactly.
fn far_point_on_line(jitter: [f64; 2]) -> Checked {
    let (mut points, lines, mut constraints) = short_line(jitter);
    points.push(point("p0000003", 600001.0, 800001.0));
    constraints.extend(fix(5, "p0000003", 600001.0, 800001.0));
    constraints.push(k(
        7,
        r#""type": "on", "point": "p0000003", "curve": "n0000001""#,
    ));
    checked(&text(&points, &lines, &[], &[], &constraints))
}

#[test]
fn far_point_on_a_short_lines_extension_drawn_exactly_is_kept() {
    // The baseline: drawn on its solution, nothing moves, so nothing is off by more than
    // rounding (the line's direction from exact points; 1e6 × 1e-16 is 1e-10 mm).
    let mut w = worker();
    let s = save_and_load(&far_point_on_line([0.0, 0.0]), &mut w).unwrap();
    assert_eq!(at(&s, "p0000003"), [600001.0, 800001.0]);
    assert_eq!(s.redundant.len(), 1, "{:?}", s.redundant);
}

#[test]
fn far_point_on_a_short_lines_extension_drawn_off_by_ten_nanometres_is_kept() {
    // Found by break-it, fixed by the extent: p0000002 drawn 1e-5 mm off its (4, 5): one
    // Newton step leaves the line's length within the solver's convergence (1e-10) but not
    // at 5 exactly, so its direction is off by about 1e-12 rad. The far point, a kilometre
    // along, is then about 1e-6 mm off the line: PlaneGCS calls the constraint redundant
    // (its error² under 1e-10), and so would FreeCAD, but the longest line's ε was 1e-8 mm,
    // and the sketch was refused as over-constrained, though its constraints agree exactly.
    // Its extent is 1,000,000 mm (the box (1, 1) to (600001, 800001)), its ε 1e-3: kept.
    let mut w = worker();
    let mut broke = Vec::new();
    for jitter in [
        [0.0, 1e-5],
        [0.0, -1e-5],
        [1e-5, 0.0],
        [0.0, 3e-6],
        [0.0, 3e-5],
        [2e-5, -2e-5],
    ] {
        match save_and_load(&far_point_on_line(jitter), &mut w) {
            Ok(s) => {
                let p = at(&s, "p0000003");
                if p != [600001.0, 800001.0] {
                    broke.push(format!("{jitter:?}: the far point moved to {p:?}"));
                }
            }
            Err(e) => broke.push(format!("{jitter:?}: {e}")),
        }
    }
    assert!(broke.is_empty(), "{}", broke.join("\n"));
}

// One worker per test thread for the properties: a worker's start costs more than a solve.
thread_local!(static W: std::cell::RefCell<SketchWorker> = std::cell::RefCell::new(worker()));

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, failure_persistence: None, ..ProptestConfig::default() })]

    /// The far point's case, drawn off by up to 1e-3 mm, biased to the convergence edge
    /// (1e-5 and below), and placed at any whole multiple of (3, 4) along the line, out to
    /// a kilometre.
    #[test]
    fn a_far_point_on_a_short_lines_extension_agrees_wherever_it_is_drawn(
        jx in prop_oneof![Just(0.0), -1e-5f64..1e-5, -1e-3f64..1e-3],
        jy in prop_oneof![Just(0.0), -1e-5f64..1e-5, -1e-3f64..1e-3],
        along in prop_oneof![1u32..10, 1000u32..200_000],
    ) {
        let (mut points, lines, mut constraints) = short_line([jx, jy]);
        let (x, y) = (1.0 + 3.0 * f64::from(along), 1.0 + 4.0 * f64::from(along));
        points.push(point("p0000003", x, y));
        constraints.extend(fix(5, "p0000003", x, y));
        constraints.push(k(7, r#""type": "on", "point": "p0000003", "curve": "n0000001""#));
        let doc = checked(&text(&points, &lines, &[], &[], &constraints));
        let s = W.with(|w| save_and_load(&doc, &mut *w.borrow_mut()));
        // Found by break-it, fixed as above: the first failure was 5380 × 5 mm (27 m) along,
        // the line's end drawn 8.3e-6 mm high: a gap of 1.0000076e-8 mm against the longest
        // line's ε 1e-8. Its extent's ε is 2.69e-5.
        prop_assert!(s.is_ok(), "{} along: {}", along, s.unwrap_err());
    }
}

// --- A small circle tangent to the line far along ------------------------------------------

#[test]
fn far_circle_tangent_to_a_short_line_is_kept() {
    // Found by break-it, fixed by the extent (as the far point): a circle of radius 5 whose
    // centre is held at (600001, 800001) + 5 × (−4, 3)/5 = (599997, 800004), left of the
    // line's extension and tangent to it exactly. The longest line or radius was 5 mm, ε
    // 1e-8 mm; the tangency's gap is the line's direction error times a kilometre, under
    // 4e-11 rad × 1e6 = 4e-5 (hand). The extent is √(599996² + 800003²) = 1,000,000.0000125
    // (the box (1, 1) to (599997, 800004)), ε 1.0e-3: kept.
    let mut w = worker();
    let mut broke = Vec::new();
    for jitter in [[0.0, 0.0], [0.0, 1e-5], [0.0, -1e-5], [1e-5, 1e-5]] {
        let (mut points, lines, mut constraints) = short_line(jitter);
        points.push(point("p0000003", 599997.0, 800004.0));
        constraints.extend(fix(5, "p0000003", 599997.0, 800004.0));
        constraints.push(k(
            7,
            r#""type": "radius", "curve": "c0000001", "value": 5, "driving": true"#,
        ));
        constraints.push(k(
            8,
            r#""type": "tangent", "a": "n0000001", "b": "c0000001", "side": "left""#,
        ));
        let doc = checked(&text(
            &points,
            &lines,
            &[circle("c0000001", "p0000003", 5.0)],
            &[],
            &constraints,
        ));
        if let Err(e) = save_and_load(&doc, &mut w) {
            broke.push(format!("{jitter:?}: {e}"));
        }
    }
    assert!(broke.is_empty(), "{}", broke.join("\n"));
}

/// The far tangent circle, its radius drawn 5 and given as `radius`: the tangency, from a
/// centre held 5 off the line's extension, disagrees with it by `radius` − 5.
fn far_circle_of_radius(radius: &str) -> Checked {
    let (mut points, lines, mut constraints) = short_line([0.0, 0.0]);
    points.push(point("p0000003", 599997.0, 800004.0));
    constraints.extend(fix(5, "p0000003", 599997.0, 800004.0));
    constraints.push(k(
        7,
        &format!(r#""type": "radius", "curve": "c0000001", "value": {radius}, "driving": true"#),
    ));
    constraints.push(k(
        8,
        r#""type": "tangent", "a": "n0000001", "b": "c0000001", "side": "left""#,
    ));
    checked(&text(
        &points,
        &lines,
        &[circle("c0000001", "p0000003", 5.0)],
        &[],
        &constraints,
    ))
}

#[test]
fn far_circle_tangent_to_a_short_line_keeps_a_disagreement_under_the_extents_tolerance() {
    // ε at a kilometre's extent is 1.0e-3 (as above), but only gaps under PlaneGCS's own
    // 1e-5 reach it: past that PlaneGCS calls the rows conflicting (the radius given as
    // 5.0005 is refused so, "the sketch's constraints conflict"). So the extent's side is
    // pinned here, and its refusing side by the core's stand-in tests: the radius given as
    // 5.000005, drawn on its solution so the line's direction is exact, a gap of 5e-6, is
    // kept and reported, where the longest line or radius (5 mm, ε 1e-8) refused it.
    let mut w = worker();
    let s = solved(&far_circle_of_radius("5.000005"), &mut w).unwrap_or_else(|e| panic!("{e}"));
    let gaps: Vec<f64> = s.redundant.values().copied().collect();
    assert!(
        gaps.len() == 1 && (4.9e-6..=5.1e-6).contains(&gaps[0]),
        "{:?}",
        s.redundant
    );
    let e = solved(&far_circle_of_radius("5.0005"), &mut w).unwrap_err();
    assert_eq!(e.code(), ErrorCode::OverConstrained, "{e}");
    assert!(
        e.message().starts_with("the sketch's constraints conflict"),
        "{e}"
    );
}

// --- Two far points mirrored about the short line -------------------------------------------

#[test]
fn far_points_symmetric_about_a_short_line_are_kept() {
    // Kept since the extent (the box (1, 1) to (800001, 936001), √(800000² + 936000²) =
    // 1,231,298.5, ε 1.23e-3). Held (the solver drops one of the points' own dimensions
    // here, not the symmetry): (800001, 600001) and (352001, 936001) mirror each other
    // about the line through (1, 1) along (3, 4): the reflection [[−7, 24], [24, 7]] / 25 of
    // (800000, 600000) is (352000, 936000), exactly. Both held by their x and y, and
    // symmetric about the line too: two redundant rows, both met exactly.
    let mut w = worker();
    let mut broke = Vec::new();
    for jitter in [[0.0, 0.0], [0.0, 1e-5], [0.0, -1e-5], [1e-5, 1e-5]] {
        let (mut points, lines, mut constraints) = short_line(jitter);
        points.push(point("p0000003", 800001.0, 600001.0));
        points.push(point("p0000004", 352001.0, 936001.0));
        constraints.extend(fix(5, "p0000003", 800001.0, 600001.0));
        constraints.extend(fix(7, "p0000004", 352001.0, 936001.0));
        constraints.push(k(
            9,
            r#""type": "symmetric", "a": "p0000003", "b": "p0000004", "about": "n0000001""#,
        ));
        let doc = checked(&text(&points, &lines, &[], &[], &constraints));
        match save_and_load(&doc, &mut w) {
            Ok(s) => {
                if at(&s, "p0000003") != [800001.0, 600001.0]
                    || at(&s, "p0000004") != [352001.0, 936001.0]
                {
                    broke.push(format!("{jitter:?}: moved {:?}", s.points));
                }
            }
            Err(e) => broke.push(format!("{jitter:?}: {e}")),
        }
    }
    assert!(broke.is_empty(), "{}", broke.join("\n"));
}

// --- E from geometry the constraint has nothing to do with --------------------------------

/// A 10 mm horizontal line from (1, 1), its length given twice: 10 and `second` (a distance
/// between its ends), and, if `far`, a 1 km construction line 900 m below, unconstrained.
fn twice_dimensioned(second: f64, length: f64, far: bool) -> Checked {
    twice_dimensioned_beside(second, length, far.then_some(1e6), false)
}

/// [`twice_dimensioned`], the construction line `far` long, if any, and if `joined`, its
/// ends dimensioned from p1 and from each other (k6 to k9), in whole nanometres: one part.
fn twice_dimensioned_beside(second: f64, length: f64, far: Option<f64>, joined: bool) -> Checked {
    let mut points = vec![
        point("p0000001", 1.0, 1.0),
        point("p0000002", 1.0 + length, 1.0),
    ];
    let mut lines = vec![line("n0000001", "p0000001", "p0000002")];
    if let Some(far) = far {
        points.push(point("p0000003", -500000.0, -900000.0));
        points.push(point("p0000004", -500000.0 + far, -900000.0));
        lines.push(line_as("n0000002", "p0000003", "p0000004", true));
    }
    let mut constraints = fix(1, "p0000001", 1.0, 1.0).to_vec();
    constraints.push(k(3, r#""type": "horizontal", "line": "n0000001""#));
    constraints.push(k(
        4,
        &format!(r#""type": "length", "line": "n0000001", "value": {length}, "driving": true"#),
    ));
    constraints.push(k(
        5,
        &format!(
            r#""type": "distance", "a": "p0000001", "b": "p0000002", "value": {second}, "driving": true"#
        ),
    ));
    if let (Some(far), true) = (far, joined) {
        for (n, kind, a, b, value) in [
            (6, "distance_x", "p0000001", "p0000003", -500001.0),
            (7, "distance_y", "p0000001", "p0000003", -900001.0),
            (8, "distance_x", "p0000003", "p0000004", far),
            (9, "distance_y", "p0000003", "p0000004", 0.0),
        ] {
            constraints.push(k(
                n,
                &format!(
                    r#""type": "{kind}", "a": "{a}", "b": "{b}", "value": {value}, "driving": true"#
                ),
            ));
        }
    }
    checked(&text(&points, &lines, &[], &[], &constraints))
}

#[test]
fn two_lengths_five_nanometres_apart_conflict_on_a_small_sketch() {
    // The rule's own side: 10 and 10.000005 on a sketch whose longest line is 10 mm:
    // ε = 1e-8 mm, the gap 5e-6 mm. Refused as over-constrained, naming the one dropped.
    let mut w = worker();
    let e = solved(&twice_dimensioned(10.000005, 10.0, false), &mut w).unwrap_err();
    assert_eq!(e.code(), ErrorCode::OverConstrained, "{e}");
    assert!(!e.elements().is_empty(), "{e}");
}

#[test]
fn an_unrelated_construction_line_leaves_two_lengths_five_micrometres_apart_conflicting() {
    // Found by break-it, fixed by parts: the same 10 mm line, dimensioned 10 and 10.000005,
    // beside an unconstrained construction line 1 km long and 900 m away. Sketch-wide, the
    // box (-500000, -900000) to (500000, 1) gave E = √(1,000,000² + 900,001²) = 1,345,363
    // mm and ε 1.3e-3, so the 5e-6 mm gap was "met" and whether two dimensions conflict
    // depended on what else was drawn. No row joins the construction line to the 10 mm one:
    // the length's part is the line's two points, E 10, ε 1e-8: refused, as without it.
    let mut w = worker();
    let e = solved(&twice_dimensioned(10.000005, 10.0, true), &mut w).unwrap_err();
    assert_eq!(e.code(), ErrorCode::OverConstrained, "{e}");
    assert_eq!(e.elements().len(), 1, "{e}");
    assert!(e.message().contains("of the part's 1e1 mm extent"), "{e}");
}

#[test]
fn an_unrelated_construction_line_leaves_a_one_and_a_two_nanometre_length_conflicting() {
    // Found by break-it, fixed by parts: a line 1 nm long dimensioned 1 nm and 2 nm, a 100%
    // disagreement, beside the kilometre construction line, unconstrained (sketch-wide, ε
    // 1.3e-3 kept it, "met within 1e-6 mm"): its part is its own, E 1e-6, ε the floor:
    // refused, as without the construction line.
    let mut w = worker();
    for far in [true, false] {
        let e = solved(&twice_dimensioned(0.000002, 0.000001, far), &mut w).unwrap_err();
        assert_eq!(e.code(), ErrorCode::OverConstrained, "{far}: {e}");
        assert!(e.message().contains("past the 1e-8 mm"), "{far}: {e}");
    }
}

#[test]
fn a_construction_line_joined_to_two_lengths_five_micrometres_apart_lets_them_agree() {
    // RECORDED, what remains within a part: the kilometre construction line's ends
    // dimensioned from p1 (-500001, -900001) and from each other (1000000, 0), so it and
    // the 10 mm line are one part, E = √(1,000,000² + 900,001²) = 1,345,363 mm, ε 1.3e-3:
    // the 5e-6 mm disagreement is kept, its gap reported. The exact rule (decision 5's
    // dependence coefficients) would refuse it.
    let mut w = worker();
    let s = solved(
        &twice_dimensioned_beside(10.000005, 10.0, Some(1e6), true),
        &mut w,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let gaps: Vec<f64> = s.redundant.values().copied().collect();
    assert_eq!(gaps.len(), 1, "{:?}", s.redundant);
    assert!((gaps[0] - 5e-6).abs() < 1e-9, "{gaps:?}");
}

/// A 10 mm horizontal line from (1, 1), its length given twice, 10 and `second`, and a
/// construction line beside it on y = 1, from (12, 1) to (`to`, 1), joined to it: held
/// horizontal (k6) and its start 1 right of the line's end (k7). Every point on y = 1 and
/// one part, so the extent is `to` − 1.
fn twice_dimensioned_in_a_row(second: f64, to: f64) -> Checked {
    let points = [
        point("p0000001", 1.0, 1.0),
        point("p0000002", 11.0, 1.0),
        point("p0000003", 12.0, 1.0),
        point("p0000004", to, 1.0),
    ];
    let lines = [
        line("n0000001", "p0000001", "p0000002"),
        line_as("n0000002", "p0000003", "p0000004", true),
    ];
    let mut constraints = fix(1, "p0000001", 1.0, 1.0).to_vec();
    constraints.push(k(3, r#""type": "horizontal", "line": "n0000001""#));
    constraints.push(k(
        4,
        r#""type": "length", "line": "n0000001", "value": 10, "driving": true"#,
    ));
    constraints.push(k(
        5,
        &format!(
            r#""type": "distance", "a": "p0000001", "b": "p0000002", "value": {second}, "driving": true"#
        ),
    ));
    constraints.push(k(6, r#""type": "horizontal", "line": "n0000002""#));
    constraints.push(k(
        7,
        r#""type": "distance_x", "a": "p0000002", "b": "p0000003", "value": 1, "driving": true"#,
    ));
    checked(&text(&points, &lines, &[], &[], &constraints))
}

#[test]
fn two_lengths_a_nanometre_apart_conflict_within_a_metres_extent_and_agree_past_it() {
    // The rule's edge: 10 and 10.000001 (a gap of 1.00000008e-6 mm, 10.000001's rounding)
    // on a sketch whose points span 999 mm (the construction line to x 1000: ε 9.99e-7),
    // refused, and 1001 mm (to x 1002: ε 1.001e-6), kept with the gap reported.
    let mut w = worker();
    let e = solved(&twice_dimensioned_in_a_row(10.000001, 1000.0), &mut w).unwrap_err();
    assert_eq!(e.code(), ErrorCode::OverConstrained, "{e}");
    assert!(
        e.message().contains("of the part's 9.99e2 mm extent"),
        "{e}"
    );
    let s = solved(&twice_dimensioned_in_a_row(10.000001, 1002.0), &mut w)
        .unwrap_or_else(|e| panic!("{e}"));
    let gaps: Vec<f64> = s.redundant.values().copied().collect();
    assert_eq!(gaps.len(), 1, "{:?}", s.redundant);
    assert!((0.99e-6..=1.01e-6).contains(&gaps[0]), "{gaps:?}");
}

// --- Lines of no size to speak of -----------------------------------------------------------

/// A line from the origin (held on both axes) to (len, 0), horizontal, at `angle` degrees
/// to x_axis as well, as its file writes it.
fn tiny_line_text(len: f64, angle: f64) -> String {
    let points = vec![point("p0000001", 0.0, 0.0), point("p0000002", len, 0.0)];
    let lines = vec![line("n0000001", "p0000001", "p0000002")];
    let constraints = vec![
        k(1, r#""type": "on", "point": "p0000001", "curve": "x_axis""#),
        k(2, r#""type": "on", "point": "p0000001", "curve": "y_axis""#),
        k(3, r#""type": "horizontal", "line": "n0000001""#),
        k(
            4,
            &format!(
                r#""type": "angle", "a": "x_axis", "b": "n0000001", "value": {angle}, "driving": true"#
            ),
        ),
    ];
    text(&points, &lines, &[], &[], &constraints)
}

/// [`tiny_line_text`], gated.
fn tiny_line(len: f64, angle: f64) -> Checked {
    checked(&tiny_line_text(len, angle))
}

/// That the gate refuses [`tiny_line_text`]: its line is 1e-7 mm long or less, no size to
/// the kernel (the gate's floor), named by its field.
#[track_caller]
fn too_small_to_draw(len: f64, angle: f64) {
    let refused = saves::checked(&tiny_line_text(len, angle)).unwrap_err();
    assert!(
        refused.contains("parameter: Some(\"lines.0.to\")")
            && refused.contains(&format!("line n0000001 is {len:e} mm long")),
        "{len:e}: {refused}"
    );
}

#[test]
fn tiny_lines_horizontal_and_at_no_angle_agree() {
    // Agreeing redundancy at every scale the gate takes: above its floor, 1e-7 mm. At or
    // under it (1e-12 now among them), the gate refuses the line before any solve.
    let mut w = worker();
    for len in [1e-7, 1e-12, 1e-50, 1e-100, 1e-150] {
        too_small_to_draw(len, 0.0);
    }
    for len in [1.0, 1e-6] {
        let s = solved(&tiny_line(len, 0.0), &mut w)
            .unwrap_or_else(|e| panic!("{len}: {e} {:?}", e.elements()));
        assert_eq!(at(&s, "p0000002")[1], 0.0, "{len}");
        assert!(
            s.redundant.values().all(|&g| g == 0.0),
            "{len}: {:?}",
            s.redundant
        );
    }
}

#[test]
fn a_point_715_mm_along_a_5_mm_lines_extension_is_kept() {
    // Found by break-it, fixed by the extent: the property's minimal case, at part scale. The
    // 5 mm line from (1, 1) to (4, 5) drawn with its end (2.4943317280471605e-5,
    // -1.7720700385223786e-5) off, and p0000003 at (430, 573) = (1, 1) + 143 × (3, 4), on
    // the line exactly. Refused as over-constrained by the longest line's scale: k0000007
    // (the point on the line) "disagrees by 1.0034e-8 mm, past this sketch's 1e-8 mm". The
    // solver left the line's direction about 1.4e-11 rad off, 7e-11 mm at its own end (its
    // 5 mm lever) and 1e-8 at the point's 715 mm. The box (1, 1) to (430, 573) is 429 ×
    // 572, E = 715 (143 × 5), ε 7.15e-7: kept, its gap under the hand bound, 4e-11 rad (the
    // line's direction, held by its length and run, each to 1e-10) × 715 = 3e-8.
    let (mut points, lines, mut constraints) =
        short_line([2.4943317280471605e-5, -1.7720700385223786e-5]);
    points.push(point("p0000003", 430.0, 573.0));
    constraints.extend(fix(5, "p0000003", 430.0, 573.0));
    constraints.push(k(
        7,
        r#""type": "on", "point": "p0000003", "curve": "n0000001""#,
    ));
    let doc = checked(&text(&points, &lines, &[], &[], &constraints));
    let mut w = worker();
    let s = save_and_load(&doc, &mut w).unwrap();
    assert_eq!(at(&s, "p0000003"), [430.0, 573.0]);
    let gaps: Vec<f64> = s.redundant.values().copied().collect();
    assert!(gaps.len() == 1 && gaps[0] <= 3e-8, "{:?}", s.redundant);
}

/// The 715 mm case, p0000003 drawn and held at (430, `y`) instead of on the line.
fn far_point_held_at(y: f64) -> Checked {
    let (mut points, lines, mut constraints) =
        short_line([2.4943317280471605e-5, -1.7720700385223786e-5]);
    points.push(point("p0000003", 430.0, y));
    constraints.extend(fix(5, "p0000003", 430.0, y));
    constraints.push(k(
        7,
        r#""type": "on", "point": "p0000003", "curve": "n0000001""#,
    ));
    checked(&text(&points, &lines, &[], &[], &constraints))
}

#[test]
fn a_point_715_mm_along_held_a_micrometre_off_is_kept_and_two_off_is_refused() {
    // ε pinned both ways at 7.15e-7 (E 715, as above). p0000003 held (0, δ) off the line,
    // which runs along (3, 4)/5: its distance from the line is 0.6 δ (hand), give or take
    // the line's own slack there, 3e-8 (as above). At δ = 1e-6 a gap of 6e-7 ± 3e-8, kept
    // and reported; at δ = 2e-6, 1.2e-6 ± 3e-8, refused, naming the point on the
    // line (k0000007, the row PlaneGCS drops).
    let mut w = worker();
    let s = save_and_load(&far_point_held_at(573.000001), &mut w).unwrap();
    assert_eq!(at(&s, "p0000003"), [430.0, 573.000001]);
    let gaps: Vec<f64> = s.redundant.values().copied().collect();
    assert!(
        gaps.len() == 1 && (5.7e-7..=6.3e-7).contains(&gaps[0]),
        "{:?}",
        s.redundant
    );
    let e = solved(&far_point_held_at(573.000002), &mut w).unwrap_err();
    assert_eq!(
        (e.code(), e.elements()),
        (ErrorCode::OverConstrained, &["k0000007".to_owned()][..]),
        "{e}"
    );
    let worst: f64 = e
        .message()
        .split("by up to ")
        .nth(1)
        .and_then(|m| m.split(' ').next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(f64::NAN);
    assert!((1.17e-6..=1.23e-6).contains(&worst), "{e}");
    assert!(e.message().contains("(k0000007), past the 7.150"), "{e}");
    assert!(e.message().contains("of the part's 7.1500"), "{e}");
    assert!(e.message().contains("e2 mm extent"), "{e}");
}

#[test]
fn tiny_lines_name_one_redundant_constraint_and_one_freedom() {
    // Below about 1e-13 mm PlaneGCS said dofs 3, and named k0000002 (p0000001 on y_axis)
    // redundant beside the angle: the angle's Jacobian row scales as 1/length, so the rank
    // check, relative to the largest pivot, took the unit rows for zero, and a point held on
    // both axes was called free (1.1.3 does the same). The gate's floor, a drawn size above
    // 1e-7 mm (the M3 review, round 1), now refuses such a line before any solve. Above it,
    // p0000001 held on both axes (2 rows, 2 unknowns), the line horizontal and at 0° (2
    // rows, one unknown: the end's y): one constraint redundant, the end's x free.
    let mut w = worker();
    for len in [1.0, 1e-6] {
        let s = solved(&tiny_line(len, 0.0), &mut w).unwrap();
        assert_eq!((s.dofs, s.redundant.len()), (1, 1), "{len:e} mm: {s:?}");
    }
    // The gate's floor (the M3 review, round 1) now refuses these before PlaneGCS sees them:
    // the 1e-12 arm, which PlaneGCS counted right, and the arms it miscounted.
    for len in [1e-12, 1e-50, 1e-100, 1e-150] {
        too_small_to_draw(len, 0.0);
    }
}

#[test]
fn tiny_lines_horizontal_and_at_thirty_degrees_conflict() {
    // A line can't be horizontal and at 30° to x. On a 1 mm line it's refused; the question
    // is where that stops. At 1e-8 mm and below the dropped constraint's gap (the rise, or
    // 0.52 rad × the length) is under 1e-8 mm, decision 5's ε, and PlaneGCS's own check is
    // under 1e-5: a sketch claiming both is kept, "met". The gate's floor (1e-7 mm, the M3
    // review) now refuses 1e-8 and under before any solve; above it, each is refused.
    for len in [1e-8, 1e-12, 1e-100, 1e-150] {
        too_small_to_draw(len, 30.0);
    }
    let mut w = worker();
    let mut kept = Vec::new();
    for len in [1.0, 1e-3, 1e-6] {
        match solved(&tiny_line(len, 30.0), &mut w) {
            Ok(s) => kept.push(format!(
                "{len} mm: Ok, end at {:?}, redundant {:?}",
                at(&s, "p0000002"),
                s.redundant
            )),
            Err(e) => assert!(
                matches!(e.code(), ErrorCode::OverConstrained),
                "{len}: {:?} {e}",
                e.code()
            ),
        }
    }
    // Survives: every length is refused, so no line is kept horizontal and at 30° at once.
    assert!(kept.is_empty(), "{}", kept.join("\n"));
}

#[test]
fn a_one_nanometre_line_solved_from_any_start_is_kept_clear_of_the_floor() {
    // The format reviewer's case (M3, round 1): a line from the origin with a driving
    // length of 1 nm, the smallest the file states, at 30° to x, drawn from four starts.
    // A solve lands on 1e-6 only to rounding (9.999999999999997e-7 from three of them),
    // which a floor at 1 nm would refuse; the gate's floor, 1e-7 mm, is ten times under it.
    let mut w = worker();
    for start in [
        (1.3e-6, 2e-7),
        (7e-7, 4e-7),
        (1.1e-6, -3e-7),
        (9.3e-7, 1e-8),
    ] {
        let points = vec![
            point("pa000001", 0.0, 0.0),
            point("pa000002", start.0, start.1),
        ];
        let lines = vec![line("na000001", "pa000001", "pa000002")];
        let mut constraints = fix(1, "pa000001", 0.0, 0.0).to_vec();
        constraints.push(k(
            3,
            r#""type": "length", "line": "na000001", "value": 0.000001, "driving": true"#,
        ));
        constraints.push(k(
            4,
            r#""type": "angle", "a": "x_axis", "b": "na000001", "value": 30, "driving": true"#,
        ));
        let doc = checked(&text(&points, &lines, &[], &[], &constraints));
        let s = save_and_load(&doc, &mut w).unwrap_or_else(|e| panic!("{start:?}: {e}"));
        let [x, y] = at(&s, "pa000002");
        let length = libm::hypot(x, y);
        assert!((length - 1e-6).abs() <= 1e-21, "{start:?}: {length:e}");
    }
}

#[test]
fn two_lines_a_nanodegree_apart_joined_are_refused_naming_the_joining_line() {
    // Two 1 mm lines from the origin, one along x and one at 1e-9° to it, their far ends
    // joined by a third line, drawn wide open (to (1, 0.5)). The solve draws the third
    // 2 sin(θ/2) long, θ = 1e-9° = 1.745e-11 rad: 1.745e-11 mm, under the kernel's 1e-7
    // mm, so the solve's geometry is one the file can't hold: an invalid result, naming the
    // joining line and its ends, with the gate's advice.
    let points = vec![
        point("p0000001", 0.0, 0.0),
        point("p0000002", 1.0, 0.0),
        point("p0000003", 1.0, 0.5),
    ];
    let lines = vec![
        line("n0000001", "p0000001", "p0000002"),
        line("n0000002", "p0000001", "p0000003"),
        line("n0000003", "p0000002", "p0000003"),
    ];
    let mut constraints = fix(1, "p0000001", 0.0, 0.0).to_vec();
    constraints.push(k(3, r#""type": "horizontal", "line": "n0000001""#));
    constraints.push(k(
        4,
        r#""type": "length", "line": "n0000001", "value": 1, "driving": true"#,
    ));
    constraints.push(k(
        5,
        r#""type": "length", "line": "n0000002", "value": 1, "driving": true"#,
    ));
    constraints.push(k(
        6,
        r#""type": "angle", "a": "x_axis", "b": "n0000002", "value": 0.000000001, "driving": true"#,
    ));
    let doc = checked(&text(&points, &lines, &[], &[], &constraints));
    let e = solved(&doc, &mut worker()).unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidResult, "{e}");
    assert_eq!(e.elements(), ["n0000003", "p0000002", "p0000003"], "{e}");
    assert!(
        e.message()
            .starts_with("the sketch solves to geometry its file can't hold: line n0000003 is "),
        "{e}"
    );
    let length: f64 = e
        .message()
        .split("line n0000003 is ")
        .nth(1)
        .and_then(|m| m.split(' ').next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(f64::NAN);
    // The solve meets the angle to its convergence, 1e-10 rad, on a 1 mm lever: 1e-10 mm
    // either way of the hand value (here 1.607e-11), far under the floor.
    assert!((length - 1.745e-11).abs() <= 1e-10, "{e}");
    assert_eq!(
        e.suggestion(),
        Some("change the constraints that draw it this small: draw the line's ends further apart")
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, failure_persistence: None, ..ProptestConfig::default() })]

    /// The far point at part scale: a 5 mm reference line, the point on its extension at most
    /// a metre out (a whole multiple of (3, 4)), the line's end drawn off by up to 0.1 mm,
    /// biased to the solver's convergence edge.
    #[test]
    // Found by break-it, fixed by the extent: at 143 × 5 mm (715 mm) along, the end drawn
    // (2.49e-5, -1.77e-5) off, refused, "disagree by up to 1.0034e-8 mm, past this sketch's
    // 1e-8 mm" (pinned above). At `along` = a, E = 5a and ε = 5e-9 × a, against a hand
    // bound of 2e-10 × a: the same 25× margin at every a.
    fn a_point_on_a_short_lines_extension_within_a_metre_agrees(
        jx in prop_oneof![Just(0.0), -3e-5f64..3e-5, -1e-1f64..1e-1],
        jy in prop_oneof![Just(0.0), -3e-5f64..3e-5, -1e-1f64..1e-1],
        along in 2u32..=200,
    ) {
        let (mut points, lines, mut constraints) = short_line([jx, jy]);
        let (x, y) = (1.0 + 3.0 * f64::from(along), 1.0 + 4.0 * f64::from(along));
        points.push(point("p0000003", x, y));
        constraints.extend(fix(5, "p0000003", x, y));
        constraints.push(k(7, r#""type": "on", "point": "p0000003", "curve": "n0000001""#));
        let doc = checked(&text(&points, &lines, &[], &[], &constraints));
        let s = W.with(|w| save_and_load(&doc, &mut *w.borrow_mut()));
        prop_assert!(s.is_ok(), "{} along: {}", along, s.unwrap_err());
    }
}

// --- A circle tangency, in square millimetres ------------------------------------------------

/// Circle c0000001 of radius 100 about p0000001, held at the origin (k1, k2, k3), and circle
/// c0000002 of radius `r_b` about p0000002, drawn at (`x`, 0) and held on y = 0 (k4, k5):
/// then `then`, numbered from k6. Every point and radius is drawn where the constraints
/// hold it, so the kept rows are met as drawn.
fn tangent_circles(r_b: &str, x: f64, then: &[&str]) -> Checked {
    let points = [point("p0000001", 0.0, 0.0), point("p0000002", x, 0.0)];
    let mut constraints = fix(1, "p0000001", 0.0, 0.0).to_vec();
    constraints.push(k(
        3,
        r#""type": "radius", "curve": "c0000001", "value": 100, "driving": true"#,
    ));
    constraints.push(k(
        4,
        &format!(r#""type": "radius", "curve": "c0000002", "value": {r_b}, "driving": true"#),
    ));
    constraints.push(k(
        5,
        r#""type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true"#,
    ));
    constraints.extend(then.iter().enumerate().map(|(i, form)| k(6 + i, form)));
    checked(&text(
        &points,
        &[],
        &[
            circle("c0000001", "p0000001", 100.0),
            circle("c0000002", "p0000002", r_b.parse().unwrap()),
        ],
        &[],
        &constraints,
    ))
}

const TANGENT_INSIDE: &str =
    r#""type": "tangent", "a": "c0000001", "b": "c0000002", "side": "inside""#;

/// p1 to p2, `value` apart.
fn apart(value: &str) -> String {
    format!(
        r#""type": "distance", "a": "p0000001", "b": "p0000002", "value": {value}, "driving": true"#
    )
}

/// [`tangent_circles`] given `tangent` inside (k6), and p2 held on a line n0000001 (k11)
/// as well: the line from p0000003, held at (`x`, −`h`) (k7, k8), to p0000004, held at
/// (`x` + 1e-6, 1 − `h`) (k9, k10). The line passes y = 0 at `x` + 1e-6 × `h`, so p2 at
/// (`x`, 0) is 1e-6 × `h` / √(1 + 1e-12) off it (hand): the point on the line is the row
/// PlaneGCS drops, and its gap is a fraction of a nanometre the file can't write as a
/// dimension. The kept rows hold p2 at (d, 0), d = `r_b` − 100, by the tangency.
fn tangent_circles_and_a_line(r_b: &str, x: f64, h: f64) -> Checked {
    let (x1, y0, y1) = (x + 1e-6, -h, 1.0 - h);
    let points = [
        point("p0000001", 0.0, 0.0),
        point("p0000002", x, 0.0),
        point("p0000003", x, y0),
        point("p0000004", x1, y1),
    ];
    let mut constraints = fix(1, "p0000001", 0.0, 0.0).to_vec();
    constraints.push(k(
        3,
        r#""type": "radius", "curve": "c0000001", "value": 100, "driving": true"#,
    ));
    constraints.push(k(
        4,
        &format!(r#""type": "radius", "curve": "c0000002", "value": {r_b}, "driving": true"#),
    ));
    constraints.push(k(
        5,
        r#""type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true"#,
    ));
    constraints.push(k(6, TANGENT_INSIDE));
    constraints.extend(fix(7, "p0000003", x, y0));
    constraints.extend(fix(9, "p0000004", x1, y1));
    constraints.push(k(
        11,
        r#""type": "on", "point": "p0000002", "curve": "n0000001""#,
    ));
    checked(&text(
        &points,
        &[line("n0000001", "p0000003", "p0000004")],
        &[
            circle("c0000001", "p0000001", 100.0),
            circle("c0000002", "p0000002", r_b.parse().unwrap()),
        ],
        &[],
        &constraints,
    ))
}

/// The one redundant constraint `s` names, and its gap.
fn one_redundant(s: &SketchSolution) -> (String, f64) {
    let all: Vec<(String, f64)> = s
        .redundant
        .iter()
        .map(|(id, &gap)| (id.as_str().to_owned(), gap))
        .collect();
    match &all[..] {
        [one] => one.clone(),
        _ => panic!("{all:?}"),
    }
}

/// `s` solved where `doc`'s sketch is drawn, bit for bit: the solver took no step.
fn as_drawn(s: &SketchSolution, points: &[(&str, [f64; 2])], radii: [f64; 2]) {
    for &(id, p) in points {
        assert_eq!(at(s, id), p, "{id}");
    }
    for (id, r) in ["c0000001", "c0000002"].into_iter().zip(radii) {
        assert_eq!(s.radius(&ElementId::new(id).unwrap()), Some(r), "{id}");
    }
}

#[test]
fn nearly_concentric_circles_keep_a_distance_the_tangencys_square_millimetres_hold() {
    // W1, the finding (Fable, 2026-10-21): radii 100 and 100.001 inside, so the tangency
    // holds the centres 0.001 apart, and the same 0.001 given again as a distance (k7, the
    // highest tag, the row PlaneGCS drops). p2 drawn at (0.00100004, 0): every kept row is
    // met as drawn but the tangency, whose residual is d² − s² = (4e-8)(0.00200004) = 8e-11
    // mm², under DogLeg's 1e-10, so it takes no step and the solution is the drawing. The
    // distance's gap is 4e-8; T = 0.00100004 + 0.001 = 0.00200004, ε = 1e-9 / T = 5.0e-7:
    // kept. The floor alone, 1e-8, refused it, though FreeCAD solves the sketch.
    let mut w = worker();
    let x = 0.00100004;
    let s = save_and_load(
        &tangent_circles("100.001", x, &[TANGENT_INSIDE, &apart("0.001")]),
        &mut w,
    )
    .unwrap();
    as_drawn(
        &s,
        &[("p0000001", [0.0, 0.0]), ("p0000002", [x, 0.0])],
        [100.0, 100.001],
    );
    let (id, gap) = one_redundant(&s);
    assert_eq!(id, "k0000007");
    assert!((gap - 4e-8).abs() <= 1e-16, "{gap:e}");
}

#[test]
fn a_tangency_of_a_thirty_second_of_a_millimetre_keeps_3_point_2e_minus_8_and_no_more() {
    // W2 and W3, ε pinned both ways: radii 100 and 100.015625 (100 + 2⁻⁶, exact), the
    // centres 0.015625 apart, so d = |r1 − r2| = 2⁻⁶ exactly, T = 2⁻⁵, ε = 1e-9 × 32 =
    // 3.2e-8 exactly (E is about 1, the line's; 1e-9). p2 held on a line 3e-8 off: kept,
    // its gap 3e-8; 3.4e-8 off: refused, naming the point on the line. 1e-9 / d or 1e-9 /
    // s would be 6.4e-8 and keep 3.4e-8; the floor, 1e-8, refuses 3e-8.
    let mut w = worker();
    let x = 0.015625;
    let s = save_and_load(&tangent_circles_and_a_line("100.015625", x, 0.03), &mut w).unwrap();
    as_drawn(
        &s,
        &[("p0000001", [0.0, 0.0]), ("p0000002", [x, 0.0])],
        [100.0, 100.015625],
    );
    let (id, gap) = one_redundant(&s);
    assert_eq!(id, "k0000011");
    assert!((gap - 3e-8).abs() <= 1e-16, "{gap:e}");
    let e = solved(&tangent_circles_and_a_line("100.015625", x, 0.034), &mut w).unwrap_err();
    assert_eq!(
        (e.code(), e.elements()),
        (ErrorCode::OverConstrained, &["k0000011".to_owned()][..]),
        "{e}"
    );
    assert!(e.message().contains("by up to 3.4"), "{e}");
    assert!(
        e.message().contains("past the 3.2e-8 mm of the part"),
        "{e}"
    );
    assert!(
        e.message().contains(
            "1e-9 mm² over the 3.125e-2 mm of its tightest circle tangency (1e-5 mm at the least), or 1e-8 mm at least"
        ),
        "{e}"
    );
}

#[test]
fn a_tangency_of_two_micrometres_keeps_four_tenths_of_a_nanometre_and_not_six() {
    // W4, the finding's window: radii 100 and 100.001, p2 at (0.001, 0). s = 100.001 − 100
    // = 0.0010000000000047748 in doubles, so the tangency's residual is about 1e-17 and no
    // step is taken. ε = 1e-9 / (0.001 + s) = 5.0e-7. p2 held on a line 4e-7 off: kept,
    // its gap 4e-7; 6e-7 off: refused, naming the point on the line.
    let mut w = worker();
    let s = save_and_load(&tangent_circles_and_a_line("100.001", 0.001, 0.4), &mut w).unwrap();
    as_drawn(
        &s,
        &[("p0000001", [0.0, 0.0]), ("p0000002", [0.001, 0.0])],
        [100.0, 100.001],
    );
    let (id, gap) = one_redundant(&s);
    assert_eq!(id, "k0000011");
    assert!((gap - 4e-7).abs() <= 1e-15, "{gap:e}");
    let e = solved(&tangent_circles_and_a_line("100.001", 0.001, 0.6), &mut w).unwrap_err();
    assert_eq!(
        (e.code(), e.elements()),
        (ErrorCode::OverConstrained, &["k0000011".to_owned()][..]),
        "{e}"
    );
    assert!(e.message().contains("past the 4.99999"), "{e}");
    assert!(
        e.message().contains("of its tightest circle tangency"),
        "{e}"
    );
}

#[test]
fn a_dropped_tangency_counts_toward_the_tolerance_as_a_kept_one_does() {
    // W5: every tangency counts, kept or dropped, so a renumbering can't change ε. Radii 100
    // and 100.0004, the distance between the centres given first (k6) and the tangency
    // last (k7), so PlaneGCS drops the tangency. p2 drawn at the distance, so no row moves.
    // The tangency's gap is the distance less 0.0004 (s = 100.0004 − 100 =
    // 0.00040000000000190994 in doubles): 0.000401 gives 1e-6, T = 0.000801, ε = 1.248e-6:
    // kept; 0.000402 gives 2e-6, T = 0.000802, ε = 1.247e-6: refused, naming the tangency.
    // Counting kept tangencies only, ε would be the floor and refuse the first.
    let mut w = worker();
    let s = save_and_load(
        &tangent_circles("100.0004", 0.000401, &[&apart("0.000401"), TANGENT_INSIDE]),
        &mut w,
    )
    .unwrap();
    as_drawn(
        &s,
        &[("p0000001", [0.0, 0.0]), ("p0000002", [0.000401, 0.0])],
        [100.0, 100.0004],
    );
    let (id, gap) = one_redundant(&s);
    assert_eq!(id, "k0000007");
    assert!((gap - 1e-6).abs() <= 1e-14, "{gap:e}");
    let e = solved(
        &tangent_circles("100.0004", 0.000402, &[&apart("0.000402"), TANGENT_INSIDE]),
        &mut w,
    )
    .unwrap_err();
    assert_eq!(
        (e.code(), e.elements()),
        (ErrorCode::OverConstrained, &["k0000007".to_owned()][..]),
        "{e}"
    );
    assert!(
        e.message().contains("of its tightest circle tangency"),
        "{e}"
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, failure_persistence: None, ..ProptestConfig::default() })]

    /// A nanometre against a circle tangency: radii 100 and 100 + s inside, p2 drawn at
    /// (s, 0), and the distance between the centres given as s + 1 nm, s a whole number of
    /// nanometres. Drawn as the kept rows hold it, so no row moves and the gap is 1e-6.
    /// From s = 1 µm (T 2e-3, ε at most 5.0e-7; E = s, 1e-9 × E at most 1e-8) the
    /// nanometre conflicts; to s = 0.4 µm (T 8e-4, ε at least 1.25e-6) it's kept, reported.
    #[test]
    fn a_nanometre_conflicts_with_a_circle_tangency_of_a_micrometre_or_more(
        n in prop_oneof![1_000u32..=10_000_000, 1u32..=400],
    ) {
        let s = f64::from(n) / 1e6;
        let r_b = format!("{}", f64::from(100_000_000 + n) / 1e6);
        let d = format!("{}", f64::from(n + 1) / 1e6);
        let doc = tangent_circles(&r_b, s, &[TANGENT_INSIDE, &apart(&d)]);
        if n >= 1_000 {
            let e = W.with(|w| solved(&doc, &mut *w.borrow_mut()))
                .expect_err("a micrometre or more");
            prop_assert_eq!(
                (e.code(), e.elements()),
                (ErrorCode::OverConstrained, &["k0000007".to_owned()][..]),
                "{}: {}", s, e
            );
        } else {
            let sol = W.with(|w| save_and_load(&doc, &mut *w.borrow_mut()))
                .unwrap_or_else(|e| panic!("{s}: {e}"));
            prop_assert_eq!(at(&sol, "p0000002"), [s, 0.0]);
            let (id, gap) = one_redundant(&sol);
            prop_assert_eq!(id, "k0000007");
            prop_assert!((gap - 1e-6).abs() <= 1e-12, "{}: {:e}", s, gap);
        }
    }
}
