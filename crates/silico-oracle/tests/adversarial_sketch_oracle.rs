//! /break-it on M3.3: the sketches that broke Silico's solve, run through headless FreeCAD
//! 1.1.3 too (the oracle), to tell Silico's own breaks from behavior both share. Needs the
//! oracle environment (`pixi run install-oracle`, or SILICO_FREECADCMD) and the sketch
//! worker beside the test binary.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../silico-core/tests/common/sketch.rs"]
mod fixture;

use fixture::{arc, line, point};
use silico_core::document::{Operation, Sketch, read};
use silico_core::sketch::{SketchWorker, Translation};
use silico_oracle::sketch::{
    FreecadSolved, Ours, SketchCase, compare, freecad_sketch, freecad_solve, silico_solve,
};

fn sketch(points: &[String], lines: &[String], arcs: &[String], constraints: &[String]) -> Sketch {
    let text = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{{\"id\": \"s0000001\", \"type\": \"sketch\", \"v\": 1, \"name\": \"S\", \"plane\": \"xy\", \"points\": [{}], \"lines\": [{}], \"circles\": [], \"arcs\": [{}], \"constraints\": [{}]}}]}}",
        points.join(", "),
        lines.join(", "),
        arcs.join(", "),
        constraints.join(", ")
    );
    match &read(text.as_bytes()).unwrap().features[0].operation {
        Operation::Sketch(s) => s.clone(),
        other => panic!("{other:?}"),
    }
}

fn c(s: &str) -> String {
    s.to_owned()
}

/// Both sides' answers for `s`, and how they differ.
fn both(s: &Sketch) -> (Ours, FreecadSolved, Vec<String>) {
    let translation = Translation::new(s).unwrap();
    let model = freecad_sketch(s, &translation).unwrap();
    let theirs = freecad_solve(std::slice::from_ref(&model))
        .unwrap()
        .remove(0);
    let case = SketchCase {
        name: "case".to_owned(),
        note: String::new(),
        sketch: s.clone(),
    };
    let ours = silico_solve(&case, &mut SketchWorker::start().unwrap()).unwrap();
    let how = compare(&model, &ours, &theirs);
    (ours, theirs, how)
}

fn symmetric_arc(end_deg: f64) -> Sketch {
    let r = 10.0;
    let (a0, a1) = (30f64.to_radians(), end_deg.to_radians());
    sketch(
        &[
            point("pc000000", 0.0, 0.0),
            point("pa000000", r * libm::cos(a0), r * libm::sin(a0)),
            point("pb000000", r * libm::cos(a1), r * libm::sin(a1)),
        ],
        &[],
        &[arc("aa000000", "pc000000", "pa000000", "pb000000")],
        &[
            c(r#"{"id": "k0000001", "type": "on", "point": "pc000000", "curve": "y_axis"}"#),
            c(
                r#"{"id": "k0000002", "type": "symmetric", "a": "pa000000", "b": "pb000000", "about": "y_axis"}"#,
            ),
        ],
    )
}

#[test]
fn freecad_too_leaves_a_half_circles_symmetric_ends_unmirrored() {
    // The spec question behind adversarial_document's half-circle break: FreeCAD 1.1.3's
    // Sketch::addSymmetricConstraint sends the same single midpoint row. This records what
    // FreeCAD does; it passes when FreeCAD shares the bug (the solve "succeeds" with a and b
    // where they were drawn, not mirrored about the y axis).
    for end in [210.0, 270.0] {
        // Under-constrained (3 degrees of freedom), so the two land at different places; only
        // whether FreeCAD mirrors the ends is compared.
        let (_, theirs, _) = both(&symmetric_arc(end));
        let (a, b) = (theirs.points["pa000000"], theirs.points["pb000000"]);
        assert_eq!(theirs.code, 0, "{theirs:?}");
        // Mirrored about the y axis would be a.y = b.y; FreeCAD leaves them far apart, as
        // Silico does (the two sides agree).
        assert!(
            (a[1] - b[1]).abs() > 1.0,
            "drawn to {end}°, FreeCAD mirrored them: a {a:?} b {b:?}"
        );
    }
}

#[test]
fn a_sloppy_but_consistent_redundant_rectangle_solves_where_freecad_refuses_it() {
    // adversarial_document's proptest's smallest realistic failure: a 1.157364 × 23.846104
    // rectangle at the origin, p3 drawn 0.32 left, p4 0.043 up, with three agreeing
    // redundancies on top of a full definition. FreeCAD calls nine constraints conflicting
    // (-4, over-constrained, 3 degrees of freedom below none): PlaneGCS diagnoses on the
    // drawing. Silico did too, until M3.3's break-it: a sketch called over-constrained that
    // the solver solved anyway is diagnosed again from its solution, where only one
    // agreeing redundancy is left, and it solves, as SolidWorks would, to the analytic
    // rectangle. The difference is FreeCAD's, on purpose.
    let (w, h): (f64, f64) = (1.157364, 23.846104);
    let m = w.min(h);
    let s = sketch(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", w, 0.0),
            point("p0000003", w - 0.2758551592300838 * m, h),
            point("p0000004", 0.0, h + 0.037252241646966326 * m),
        ],
        &[
            line("n0000001", "p0000001", "p0000002"),
            line("n0000002", "p0000002", "p0000003"),
            line("n0000003", "p0000003", "p0000004"),
            line("n0000004", "p0000004", "p0000001"),
        ],
        &[],
        &[
            c(
                r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            ),
            c(
                r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            ),
            c(r#"{"id": "k0000003", "type": "horizontal", "line": "n0000001"}"#),
            c(r#"{"id": "k0000004", "type": "vertical", "line": "n0000002"}"#),
            c(r#"{"id": "k0000005", "type": "horizontal", "line": "n0000003"}"#),
            c(r#"{"id": "k0000006", "type": "vertical", "line": "n0000004"}"#),
            format!(
                r#"{{"id": "k0000007", "type": "length", "line": "n0000001", "value": {w}, "driving": true}}"#
            ),
            format!(
                r#"{{"id": "k0000008", "type": "length", "line": "n0000002", "value": {h}, "driving": true}}"#
            ),
            c(r#"{"id": "k0000009", "type": "parallel", "a": "n0000001", "b": "n0000003"}"#),
            c(r#"{"id": "k0000010", "type": "perpendicular", "a": "n0000001", "b": "n0000002"}"#),
            c(r#"{"id": "k0000011", "type": "equal", "a": "n0000002", "b": "n0000004"}"#),
        ],
    );
    let (ours, theirs, how) = both(&s);
    assert_eq!(theirs.code, -4, "{theirs:?}");
    let solution = ours.outcome.as_ref().unwrap();
    for (p, want) in [
        ("p0000002", [w, 0.0]),
        ("p0000003", [w, h]),
        ("p0000004", [0.0, h]),
    ] {
        let got = solution
            .point(&silico_core::document::ElementId::new(p).unwrap())
            .unwrap();
        assert!(
            (got[0] - want[0]).abs() < 1e-8 && (got[1] - want[1]).abs() < 1e-8,
            "{p}: {got:?}"
        );
    }
    assert!(
        how.iter().any(|h| h.starts_with("FreeCAD's solve gave -4")),
        "{how:?}"
    );
}

#[test]
fn a_square_template_redimensioned_to_a_hundred_metres_by_ten_microns_matches_freecad() {
    // Drawn as a true 10 × 10 square with agreeing redundancies (horizontal and vertical on
    // all four sides, parallel, perpendicular, equal), then dimensioned 100,000 × 0.01.
    // Found by break-it, fixed: Silico's diagnosis on the drawing (shared points, no
    // coincidences) gave degrees of freedom -3 and six conflicting constraints; FreeCAD's
    // gives 0 and one redundant, k0000003. Diagnosed again from the solution its solver
    // reached anyway (Translation::solve), Silico's is FreeCAD's.
    let p = |id: &str, x: f64, y: f64| point(id, x, y);
    let s = sketch(
        &[
            p("p0000001", 0.0, 0.0),
            p("p0000002", 10.0, 0.0),
            p("p0000003", 10.0, 10.0),
            p("p0000004", 0.0, 10.0),
        ],
        &[
            line("n0000001", "p0000001", "p0000002"),
            line("n0000002", "p0000002", "p0000003"),
            line("n0000003", "p0000003", "p0000004"),
            line("n0000004", "p0000004", "p0000001"),
        ],
        &[],
        &[
            c(
                r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            ),
            c(
                r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            ),
            c(r#"{"id": "k0000003", "type": "horizontal", "line": "n0000001"}"#),
            c(r#"{"id": "k0000004", "type": "vertical", "line": "n0000002"}"#),
            c(r#"{"id": "k0000005", "type": "horizontal", "line": "n0000003"}"#),
            c(r#"{"id": "k0000006", "type": "vertical", "line": "n0000004"}"#),
            c(
                r#"{"id": "k0000007", "type": "length", "line": "n0000001", "value": 100000, "driving": true}"#,
            ),
            c(
                r#"{"id": "k0000008", "type": "length", "line": "n0000002", "value": 0.01, "driving": true}"#,
            ),
            c(r#"{"id": "k0000009", "type": "parallel", "a": "n0000001", "b": "n0000003"}"#),
            c(r#"{"id": "k0000010", "type": "perpendicular", "a": "n0000001", "b": "n0000002"}"#),
            c(r#"{"id": "k0000011", "type": "equal", "a": "n0000002", "b": "n0000004"}"#),
        ],
    );
    let (ours, theirs, how) = both(&s);
    assert!(how.is_empty(), "{how:?}\nours {ours:?}\ntheirs {theirs:?}");
}
