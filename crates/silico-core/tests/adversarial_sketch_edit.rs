//! Hostile inputs for the sketch edits (`document::sketch_edit`: remove, unjoin, merge),
//! from break-it. Each edit's contract: a refusal as data naming what's at fault, or a
//! sketch the gate takes. The expected sketches and lists are worked by hand, or, in the
//! property tests, by an independent diff of the sketch before and after.

use std::collections::BTreeMap;

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use silico_core::Error;
use silico_core::document::sketch_edit::{Edited, merge, remove, unjoin};
use silico_core::document::{
    Document, ElementId, Form, Operation, PROBLEMS_LISTED, Reference, Sketch, read, validate, write,
};

use crate::fixture::{arc, circle, document, fields, line, point};

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn ids(texts: &[&str]) -> Vec<ElementId> {
    texts.iter().map(|t| id(t)).collect()
}

fn doc(points: &str, lines: &str, circles: &str, arcs: &str, constraints: &str) -> Document {
    read(document(&fields(points, lines, circles, arcs, constraints)).as_bytes()).unwrap()
}

fn sketch_of(document: &Document) -> &Sketch {
    match &document.features[0].operation {
        Operation::Sketch(sketch) => sketch,
        other => panic!("{other:?}"),
    }
}

fn with_sketch(document: &Document, sketch: &Sketch) -> Document {
    let mut document = document.clone();
    document.features[0].operation = Operation::Sketch(sketch.clone());
    document
}

/// The gate's refusals of `sketch` in place of the document's, as text (none if it passes).
fn gate(document: &Document, sketch: &Sketch) -> Vec<String> {
    match validate(&with_sketch(document, sketch)) {
        Ok(_) => Vec::new(),
        Err(invalid) => invalid.errors().iter().map(|e| e.to_string()).collect(),
    }
}

fn messages(errors: &[Error]) -> Vec<String> {
    errors.iter().map(|e| e.message().to_owned()).collect()
}

/// A merge whose result would break one of the gate's rules for a constraint, refused as
/// the merge says it (FreeCAD's coincident command substitutes for some of these; the merge
/// substitutes only for an edge tangency at an end it makes shared, a `tangent_at`, M5.0's
/// open item 13, and refuses these): `message` word for word, `elements` the constraint and
/// what it names.
#[track_caller]
fn refused_as(result: Result<Edited, Error>, message: &str, elements: &[&str]) {
    let error = result.unwrap_err();
    assert_eq!(error.message(), message);
    assert_eq!(error.elements(), elements);
    assert_eq!(error.parameter(), Some("points"));
}

// p0 (0, 0) to p4 (40, 0) along x; q0 (10, 10), q1 (10, -10), q2 (20, 10) off it; p9
// (10, 0.00000004) touching p1, as ends `ends_not_joined` names (and --merge-touching
// merges) are.
const POINTS: &str = r#"{"id": "p0000000", "x": 0, "y": 0, "construction": false}, {"id": "p0000001", "x": 10, "y": 0, "construction": false}, {"id": "p0000002", "x": 20, "y": 0, "construction": false}, {"id": "p0000003", "x": 30, "y": 0, "construction": false}, {"id": "p0000004", "x": 40, "y": 0, "construction": false}, {"id": "q0000000", "x": 10, "y": 10, "construction": false}, {"id": "q0000001", "x": 10, "y": -10, "construction": false}, {"id": "q0000002", "x": 20, "y": 10, "construction": false}, {"id": "p0000009", "x": 10, "y": 0.00000004, "construction": false}"#;

// ------------------------------------------------------------------ merge

#[test]
fn merging_an_end_held_on_a_line_with_that_lines_own_end() {
    // A T drawn at a's end: b starts at p9, held on a, and p9 touches a's end p1, so
    // ends_not_joined names {p1, p9} and --merge-touching merges them. After the merge the
    // on holds a's own end on a, which the gate refuses ("part of a itself").
    // Broke, before the fix: merge returns Ok with `on` k0000000 holding p1 on a0000000, its own end; the
    // gate then refuses the sketch: "a on's point, p0000001, is part of a0000000 itself".
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000009", "q0000000"),
    ]
    .join(", ");
    let before = doc(
        POINTS,
        &lines,
        "",
        "",
        r#"{"id": "k0000000", "type": "on", "point": "p0000009", "curve": "a0000000"}"#,
    );
    assert_eq!(gate(&before, sketch_of(&before)), Vec::<String>::new());
    // Either point kept: the on holds it on a, whose end it is.
    for kept in ["p0000001", "p0000009"] {
        let other = if kept == "p0000001" {
            "p0000009"
        } else {
            "p0000001"
        };
        refused_as(
            merge(sketch_of(&before), &ids(&[kept, other])),
            &format!(
                "merging them would make constraint k0000000 hold {kept}, a point of a0000000, on a0000000 itself"
            ),
            &["k0000000", kept, "a0000000"],
        );
    }
}

#[test]
fn merging_a_tangent_on_end_with_an_end_of_its_b() {
    // a runs along y = 0 to p1 and ends there tangent on arc r, whose start p9 touches p1:
    // the arc about q0 (10, 10) from (10, 0) a quarter turn to (20, 10). Merged, a and r
    // share p1, so the tangent_on is one at a shared end, which the gate refuses ("make it
    // a tangent_at"). It's open item 13's case in its tangent_on form: FreeCAD makes a
    // coincidence at an endpoint-to-edge tangency an endpoint-to-endpoint one.
    // Broke, before the fix: merge returns Ok; the gate refuses "a tangent_on's at, point p0000001, is an
    // end of its b, r0000000, too: the curves meet there".
    let before = doc(
        POINTS,
        &line("a0000000", "p0000000", "p0000001"),
        "",
        &arc("r0000000", "q0000000", "p0000009", "q0000002"),
        r#"{"id": "k0000000", "type": "tangent_on", "a": "a0000000", "at": "p0000001", "b": "r0000000", "direction": "same"}"#,
    );
    assert_eq!(gate(&before, sketch_of(&before)), Vec::<String>::new());
    for kept in ["p0000001", "p0000009"] {
        let other = if kept == "p0000001" {
            "p0000009"
        } else {
            "p0000001"
        };
        refused_as(
            merge(sketch_of(&before), &ids(&[kept, other])),
            &format!(
                "merging them would make constraint k0000000 hold a0000000's end {kept}, a point of r0000000, tangent on r0000000: where curves meet, a tangency is a tangent_at"
            ),
            &["k0000000", "a0000000", kept, "r0000000"],
        );
    }
}

#[test]
fn merging_a_tangent_on_end_with_the_center_of_its_b() {
    // a ends at p1 tangent on circle c about q1; merging p1 into c's center puts the end
    // on its own circle's center (a tangent_on whose at is part of b).
    // Broke, before the fix: merge returns Ok; the gate refuses "a tangent_on's at, point q0000001, is
    // part of its b, c0000000, itself".
    let before = doc(
        POINTS,
        &line("a0000000", "p0000000", "p0000001"),
        &circle("c0000000", "q0000001", 10.0),
        "",
        r#"{"id": "k0000000", "type": "tangent_on", "a": "a0000000", "at": "p0000001", "b": "c0000000", "direction": "same"}"#,
    );
    assert_eq!(gate(&before, sketch_of(&before)), Vec::<String>::new());
    refused_as(
        merge(sketch_of(&before), &ids(&["q0000001", "p0000001"])),
        "merging them would make constraint k0000000 hold a0000000's end q0000001, a point of c0000000, tangent on c0000000: where curves meet, a tangency is a tangent_at",
        &["k0000000", "a0000000", "q0000001", "c0000000"],
    );
}

#[test]
fn merging_an_end_with_a_point_on_the_curve_its_line_is_tangent_to() {
    // a along y = 0 is tangent (left) to circle c about q0, radius 10, which it touches at
    // (10, 0); a lone point p9 there is held on c. Merged with a's end p1, the on holds
    // a's end on c beside an edge tangent of a and c, which the gate refuses ("make the
    // two one tangent_on"): FreeCAD's substituteConstraintCombinationsPointOnObject makes
    // that pair an endpoint-to-edge tangency.
    // Broke, before the fix: merge returns Ok; the gate refuses "a tangent's a0000000 ends at point
    // p0000001, which constraint k0000001 puts on c0000000".
    let before = doc(
        POINTS,
        &line("a0000000", "p0000000", "p0000001"),
        &circle("c0000000", "q0000000", 10.0),
        "",
        r#"{"id": "k0000000", "type": "tangent", "a": "a0000000", "b": "c0000000", "side": "left"}, {"id": "k0000001", "type": "on", "point": "p0000009", "curve": "c0000000"}"#,
    );
    assert_eq!(gate(&before, sketch_of(&before)), Vec::<String>::new());
    refused_as(
        merge(sketch_of(&before), &ids(&["p0000001", "p0000009"])),
        "merging them would make constraint k0000000 an edge tangency of a0000000 and c0000000 where k0000001 holds an end of one on the other: that's a tangent_on",
        &["k0000000", "k0000001", "a0000000", "c0000000"],
    );
}

#[test]
fn merging_an_end_with_a_point_on_the_curve_tangent_to_its_line_listed_first() {
    // The case before, its tangent listing the circle as a: the end held on a is the
    // line's, the tangent's b.
    let before = doc(
        POINTS,
        &line("a0000000", "p0000000", "p0000001"),
        &circle("c0000000", "q0000000", 10.0),
        "",
        r#"{"id": "k0000000", "type": "tangent", "a": "c0000000", "b": "a0000000", "side": "left"}, {"id": "k0000001", "type": "on", "point": "p0000009", "curve": "c0000000"}"#,
    );
    assert_eq!(gate(&before, sketch_of(&before)), Vec::<String>::new());
    refused_as(
        merge(sketch_of(&before), &ids(&["p0000001", "p0000009"])),
        "merging them would make constraint k0000000 an edge tangency of c0000000 and a0000000 where k0000001 holds an end of one on the other: that's a tangent_on",
        &["k0000000", "k0000001", "c0000000", "a0000000"],
    );
}

#[test]
fn merging_a_midpoint_with_an_end_of_its_line() {
    // q0 is held at a's midpoint; merged into a's start, the line's midpoint would be its
    // own end (a line of no length).
    // Broke, before the fix: merge returns Ok; the gate refuses "a midpoint's point, q0000000, is part of
    // a0000000 itself".
    let before = doc(
        POINTS,
        &line("a0000000", "p0000000", "p0000002"),
        "",
        "",
        r#"{"id": "k0000000", "type": "midpoint", "point": "q0000000", "line": "a0000000"}"#,
    );
    assert_eq!(gate(&before, sketch_of(&before)), Vec::<String>::new());
    refused_as(
        merge(sketch_of(&before), &ids(&["q0000000", "p0000000"])),
        "merging them would make constraint k0000000 hold q0000000, a point of a0000000, on a0000000 itself",
        &["k0000000", "q0000000", "a0000000"],
    );
}

#[test]
fn merging_a_point_line_distance_point_with_an_end_of_its_line() {
    // Broke, before the fix: merge returns Ok; the gate refuses "a point_line_distance's point, p0000001,
    // is part of a0000000 itself".
    let before = doc(
        POINTS,
        &line("a0000000", "p0000000", "p0000001"),
        "",
        "",
        r#"{"id": "k0000000", "type": "point_line_distance", "point": "q0000000", "line": "a0000000", "value": 10, "side": "left", "driving": true}"#,
    );
    assert_eq!(gate(&before, sketch_of(&before)), Vec::<String>::new());
    refused_as(
        merge(sketch_of(&before), &ids(&["p0000001", "q0000000"])),
        "merging them would make constraint k0000000 hold p0000001, a point of a0000000, on a0000000 itself",
        &["k0000000", "p0000001", "a0000000"],
    );
}

#[test]
fn merging_a_mirrored_point_with_an_end_of_the_mirror_line() {
    // q0 and q1 mirror each other about a (along y = 0); merging q0 into a's end makes
    // the symmetric mirror a's own end about a.
    // Broke, before the fix: merge returns Ok; the gate refuses "a symmetric's p0000001 is an end of
    // a0000000, so it mirrors onto itself".
    let before = doc(
        POINTS,
        &line("a0000000", "p0000000", "p0000001"),
        "",
        "",
        r#"{"id": "k0000000", "type": "symmetric", "a": "q0000000", "b": "q0000001", "about": "a0000000"}"#,
    );
    assert_eq!(gate(&before, sketch_of(&before)), Vec::<String>::new());
    refused_as(
        merge(sketch_of(&before), &ids(&["p0000001", "q0000000"])),
        "merging them would make constraint k0000000 mirror an end of a0000000 about a0000000 itself",
        &["k0000000", "p0000001", "q0000001", "a0000000"],
    );
    // Its other point, b, merged into the same end.
    refused_as(
        merge(sketch_of(&before), &ids(&["p0000001", "q0000001"])),
        "merging them would make constraint k0000000 mirror an end of a0000000 about a0000000 itself",
        &["k0000000", "q0000000", "p0000001", "a0000000"],
    );
}

#[test]
fn merging_a_point_on_a_circle_with_its_center() {
    // Broke, before the fix: merge returns Ok; the gate refuses "a on's point, p0000003, is part of
    // c0000000 itself".
    let before = doc(
        POINTS,
        "",
        &circle("c0000000", "p0000003", 10.0),
        "",
        r#"{"id": "k0000000", "type": "on", "point": "q0000000", "curve": "c0000000"}"#,
    );
    assert_eq!(gate(&before, sketch_of(&before)), Vec::<String>::new());
    refused_as(
        merge(sketch_of(&before), &ids(&["p0000003", "q0000000"])),
        "merging them would make constraint k0000000 hold p0000003, a point of c0000000, on c0000000 itself",
        &["k0000000", "p0000003", "c0000000"],
    );
}

#[test]
fn merging_far_points_into_a_line_of_no_length_is_refused_naming_the_line() {
    // A lone point z drawn exactly where b ends; merging b's start into z (kept, at its own
    // place) leaves b from (20, 0) to (20, 0). The merge places the point, so it's the
    // gate that refuses, naming b: survived, the gate is the backstop here.
    let points = format!("{POINTS}, {}", point("z0000000", 20.0, 0.0));
    let before = doc(
        &points,
        &line("b0000000", "p0000001", "p0000002"),
        "",
        "",
        "",
    );
    match merge(sketch_of(&before), &ids(&["z0000000", "p0000001"])) {
        Err(error) => assert!(error.elements().iter().any(|e| e == "b0000000")),
        Ok(edited) => {
            let refused = gate(&before, &edited.sketch);
            assert_eq!(refused.len(), 1, "{refused:#?}");
            assert!(
                refused[0].contains("line b0000000 is 0 mm long"),
                "{refused:#?}"
            );
        }
    }
}

#[test]
fn merging_a_symmetrics_about_point_with_one_it_mirrors_is_refused() {
    let before = doc(
        POINTS,
        "",
        "",
        "",
        r#"{"id": "k0000000", "type": "symmetric", "a": "q0000000", "b": "q0000001", "about": "p0000001"}"#,
    );
    let error = merge(sketch_of(&before), &ids(&["p0000001", "q0000000"])).unwrap_err();
    assert_eq!(
        error.message(),
        "merging them would make constraint k0000000 name point p0000001 twice"
    );
    assert_eq!(error.elements(), ["k0000000", "p0000001"]);
}

#[test]
fn merging_an_arcs_two_ends_is_refused_naming_the_points_merged() {
    // The design's errors name the points merged ("merging pa000004 and pa000005 would
    // give line na000004 one point at both ends", docs/design/m5.0-app.md, sketch_merge).
    // Broke, before the fix: the message is "merging them would give arc r0000000 one point twice",
    // naming neither point, and `elements` lists the arc's center q0000000, which no one
    // asked to merge, beside its two ends.
    let before = doc(
        POINTS,
        "",
        "",
        &arc("r0000000", "q0000000", "p0000001", "q0000002"),
        "",
    );
    let error = merge(sketch_of(&before), &ids(&["p0000001", "q0000002"])).unwrap_err();
    assert!(
        error.message().contains("p0000001") && error.message().contains("q0000002"),
        "{}",
        error.message()
    );
    assert_eq!(error.elements(), ["p0000001", "q0000002", "r0000000"]);
}

#[test]
fn a_refused_merge_leaves_the_sketch_as_it_was_and_the_next_edit_works() {
    let before = doc(
        POINTS,
        &line("a0000000", "p0000000", "p0000001"),
        "",
        "",
        "",
    );
    let sketch = sketch_of(&before).clone();
    merge(&sketch, &ids(&["p0000000", "p0000001"])).unwrap_err();
    assert_eq!(&sketch, sketch_of(&before));
    let edited = merge(&sketch, &ids(&["p0000001", "p0000009"])).unwrap();
    assert_eq!(gate(&before, &edited.sketch), Vec::<String>::new());
    assert_eq!(edited.removed, ids(&["p0000009"]));
}

#[test]
fn merging_every_lone_point_of_a_full_sketch_is_prompt() {
    // 512 points, the most a sketch's 1,024 unknowns allow, merged into the first.
    let points: Vec<String> = (0..512)
        .map(|i| point(&format!("m{i:07}"), f64::from(i), 0.0))
        .collect();
    let before = doc(&points.join(", "), "", "", "", "");
    let named: Vec<ElementId> = sketch_of(&before)
        .points
        .iter()
        .map(|p| p.id.clone())
        .collect();
    let start = std::time::Instant::now();
    let edited = merge(sketch_of(&before), &named).unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(edited.sketch.points.len(), 1);
    assert_eq!(edited.removed.len(), 511);
}

// ------------------------------------------------------------------ remove

#[test]
fn removing_an_unknown_member_named_many_times_lists_at_most_the_problems_listed() {
    // Every other list of problems the core refuses lists the first PROBLEMS_LISTED and
    // counts the rest (document::validate, a transaction's features).
    // Broke, before the fix: remove returns one error for each of the 100,000 places, unbounded.
    let before = doc(POINTS, "", "", "", "");
    let members = vec![id("zzzzzzzz"); 100_000];
    let refused = remove(sketch_of(&before), &members).unwrap_err();
    assert_eq!((refused.errors.len(), refused.total), (1, 1));
    assert_eq!(refused.errors[0].parameter(), Some("members.0"));
    // 150 unknown IDs, each once: the first 100 listed, in the request's order, all counted.
    let members: Vec<ElementId> = (0..150).map(|i| id(&format!("z{i:07}"))).collect();
    let refused = remove(sketch_of(&before), &members).unwrap_err();
    assert_eq!(PROBLEMS_LISTED, 100);
    assert_eq!((refused.errors.len(), refused.total), (100, 150));
    assert_eq!(refused.errors[99].parameter(), Some("members.99"));
    assert_eq!(
        refused.errors[99].message(),
        "the sketch has no member z0000099"
    );
}

#[test]
fn removing_a_used_point_named_twice_refuses_it_once() {
    // A member named twice is taken once (decision 7), so it's refused once.
    // Broke, before the fix: two identical refusals but for their parameter, members.0 and members.1.
    let before = doc(
        POINTS,
        &line("a0000000", "p0000000", "p0000001"),
        "",
        "",
        "",
    );
    let errors = remove(sketch_of(&before), &ids(&["p0000001", "p0000001"]))
        .unwrap_err()
        .errors;
    assert_eq!(
        messages(&errors),
        ["point p0000001 is used by line a0000000"]
    );
    // Named where it's first named.
    assert_eq!(errors[0].parameter(), Some("members.0"));
}

#[test]
fn removing_a_point_a_line_and_an_arc_use_names_them_as_curves() {
    let before = doc(
        POINTS,
        &line("a0000000", "p0000000", "p0000001"),
        "",
        &arc("r0000000", "q0000000", "p0000001", "q0000002"),
        "",
    );
    let errors = remove(sketch_of(&before), &ids(&["p0000001"]))
        .unwrap_err()
        .errors;
    assert_eq!(
        messages(&errors),
        ["point p0000001 is used by curves a0000000 and r0000000"]
    );
    assert_eq!(errors[0].elements(), ["p0000001", "a0000000", "r0000000"]);
    assert_eq!(errors[0].elements_total(), 3);
}

#[test]
fn removing_a_hub_point_names_at_most_the_problems_listed_and_counts_the_rest() {
    // A hub where 511 lines meet (the most a sketch's unknowns allow): the refusal lists
    // the first PROBLEMS_LISTED elements and counts them all, as ends_not_joined and every
    // sweep refusal do (the catalog's hub case).
    // Broke, before the fix: elements lists all 512 and the message joins all 511 line IDs (about 6,700
    // characters), with elements_total 512.
    let mut points = vec![point("h0000000", 0.0, 0.0)];
    let mut lines = Vec::new();
    for i in 0..511 {
        // A 32-wide grid above the hub, 1 mm apart.
        points.push(point(
            &format!("m{i:07}"),
            f64::from(i % 32),
            f64::from(i / 32 + 1),
        ));
        lines.push(line(&format!("n{i:07}"), "h0000000", &format!("m{i:07}")));
    }
    let before = doc(&points.join(", "), &lines.join(", "), "", "", "");
    assert_eq!(gate(&before, sketch_of(&before)), Vec::<String>::new());
    let errors = remove(sketch_of(&before), &ids(&["h0000000"]))
        .unwrap_err()
        .errors;
    assert_eq!(errors.len(), 1);
    let error = &errors[0];
    assert_eq!(error.elements_total(), 512);
    // The point, then the first 99 lines.
    assert_eq!(error.elements().len(), 100);
    assert_eq!(error.elements()[0], "h0000000");
    assert_eq!(error.elements()[99], "n0000098");
    assert_eq!(
        error.message(),
        "point h0000000 is used by lines n0000000, n0000001, n0000002 and 508 more"
    );
}

#[test]
fn removing_every_member_leaves_an_empty_sketch_the_gate_takes() {
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000001", "p0000002"),
    ]
    .join(", ");
    let before = doc(
        POINTS,
        &lines,
        &circle("c0000000", "p0000003", 5.0),
        &arc("r0000000", "q0000000", "p0000001", "q0000002"),
        r#"{"id": "k0000000", "type": "tangent_at", "a": "a0000000", "b": "b0000000", "at": "p0000001", "direction": "same"}, {"id": "k0000001", "type": "radius", "curve": "c0000000", "value": 5, "driving": true}"#,
    );
    let all: Vec<ElementId> = sketch_of(&before)
        .elements()
        .map(|(id, _)| id.clone())
        .collect();
    // Listed backwards: constraints, then curves, then the points they use.
    let backwards: Vec<ElementId> = all.iter().rev().cloned().collect();
    let edited = remove(sketch_of(&before), &backwards).unwrap();
    let sketch = &edited.sketch;
    // By kind, then by ID: p0000009 among the points before q0000000.
    let mut all = all;
    all[5..9].rotate_right(1);
    assert!(
        sketch.points.is_empty()
            && sketch.lines.is_empty()
            && sketch.circles.is_empty()
            && sketch.arcs.is_empty()
            && sketch.constraints.is_empty()
    );
    assert_eq!(edited.removed, all);
    assert_eq!(gate(&before, sketch), Vec::<String>::new());
}

#[test]
fn removing_nothing_changes_nothing() {
    let before = doc(
        POINTS,
        &line("a0000000", "p0000000", "p0000001"),
        "",
        "",
        "",
    );
    let edited = remove::<ElementId>(sketch_of(&before), &[]).unwrap();
    assert_eq!(&edited.sketch, sketch_of(&before));
    assert!(edited.added.is_empty() && edited.modified.is_empty() && edited.removed.is_empty());
}

#[test]
fn removing_a_line_keeps_its_end_an_arc_centres_on() {
    // r is centred on p1, a's end: p1 stays, and so does a distance on it.
    let before = doc(
        POINTS,
        &line("a0000000", "p0000000", "p0000001"),
        "",
        &arc("r0000000", "p0000001", "p0000002", "q0000000"),
        r#"{"id": "k0000000", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 10, "driving": true}"#,
    );
    let edited = remove(sketch_of(&before), &ids(&["a0000000"])).unwrap();
    assert_eq!(edited.removed, ids(&["p0000000", "a0000000"]));
    assert_eq!(edited.sketch.constraints.len(), 1);
    assert_eq!(gate(&before, &edited.sketch), Vec::<String>::new());
}

#[test]
fn removing_the_line_of_a_d_removes_both_its_tangent_ats_and_keeps_the_arc() {
    // A D: line a from p0 to p2, arc r about p1 from p2 round to p0, joined at both ends
    // by tangent_ats.
    let before = doc(
        POINTS,
        &line("a0000000", "p0000000", "p0000002"),
        "",
        &arc("r0000000", "p0000001", "p0000002", "p0000000"),
        r#"{"id": "k0000000", "type": "tangent_at", "a": "a0000000", "b": "r0000000", "at": "p0000000", "direction": "same"}, {"id": "k0000001", "type": "tangent_at", "a": "a0000000", "b": "r0000000", "at": "p0000002", "direction": "same"}, {"id": "k0000002", "type": "radius", "curve": "r0000000", "value": 10, "driving": true}"#,
    );
    let edited = remove(sketch_of(&before), &ids(&["a0000000"])).unwrap();
    assert_eq!(edited.removed, ids(&["a0000000", "k0000000", "k0000001"]));
    let points: Vec<&str> = edited.sketch.points.iter().map(|p| p.id.as_str()).collect();
    assert!(points.contains(&"p0000000") && points.contains(&"p0000002"));
    assert_eq!(gate(&before, &edited.sketch), Vec::<String>::new());
}

#[test]
fn removing_a_symmetrics_about_point_removes_the_symmetric_and_nothing_else() {
    let before = doc(
        POINTS,
        "",
        "",
        "",
        r#"{"id": "k0000000", "type": "symmetric", "a": "q0000000", "b": "q0000001", "about": "p0000001"}, {"id": "k0000001", "type": "distance", "a": "q0000000", "b": "q0000001", "value": 20, "driving": true}"#,
    );
    let edited = remove(sketch_of(&before), &ids(&["p0000001"])).unwrap();
    assert_eq!(edited.removed, ids(&["p0000001", "k0000000"]));
    assert_eq!(gate(&before, &edited.sketch), Vec::<String>::new());
}

#[test]
fn a_refused_remove_names_every_problem_at_once() {
    // An unknown member and a used point, judged whole: both refused.
    let before = doc(
        POINTS,
        &line("a0000000", "p0000000", "p0000001"),
        "",
        "",
        "",
    );
    let errors = remove(sketch_of(&before), &ids(&["p0000000", "zzzzzzzz"]))
        .unwrap_err()
        .errors;
    // In the request's order.
    let parameters: Vec<Option<&str>> = errors.iter().map(|e| e.parameter()).collect();
    assert_eq!(parameters, [Some("members.0"), Some("members.1")]);
    assert_eq!(
        messages(&errors),
        [
            "point p0000000 is used by line a0000000",
            "the sketch has no member zzzzzzzz"
        ]
    );
    // The other way round, the other way round.
    let errors = remove(sketch_of(&before), &ids(&["zzzzzzzz", "p0000000"]))
        .unwrap_err()
        .errors;
    let parameters: Vec<Option<&str>> = errors.iter().map(|e| e.parameter()).collect();
    assert_eq!(parameters, [Some("members.0"), Some("members.1")]);
    assert_eq!(
        messages(&errors),
        [
            "the sketch has no member zzzzzzzz",
            "point p0000000 is used by line a0000000"
        ]
    );
}

// ------------------------------------------------------------------ unjoin

#[test]
fn unjoining_one_of_two_concentric_circles() {
    let before = doc(
        POINTS,
        "",
        &[
            circle("c0000000", "p0000003", 5.0),
            circle("c0000001", "p0000003", 8.0),
        ]
        .join(", "),
        "",
        r#"{"id": "k0000000", "type": "distance_x", "a": "origin", "b": "p0000003", "value": 30, "driving": true}, {"id": "k0000001", "type": "radius", "curve": "c0000001", "value": 8, "driving": true}"#,
    );
    let edited = unjoin(
        sketch_of(&before),
        &id("p0000003"),
        &id("c0000001"),
        id("z0000000"),
    )
    .unwrap();
    assert_eq!(gate(&before, &edited.sketch), Vec::<String>::new());
    let c1 = edited
        .sketch
        .circles
        .iter()
        .find(|c| c.id == id("c0000001"))
        .unwrap();
    assert_eq!(c1.center, id("z0000000"));
    assert_eq!(edited.sketch.circles[0].center, id("p0000003"));
    assert_eq!(edited.sketch.constraints.len(), 2);
    assert_eq!(edited.added, ids(&["z0000000"]));
    assert_eq!(edited.modified, ids(&["c0000001"]));
    assert!(edited.removed.is_empty());
}

#[test]
fn unjoining_one_end_of_a_d_removes_only_that_ends_tangent_at() {
    let before = doc(
        POINTS,
        &line("a0000000", "p0000000", "p0000002"),
        "",
        &arc("r0000000", "p0000001", "p0000002", "p0000000"),
        r#"{"id": "k0000000", "type": "tangent_at", "a": "a0000000", "b": "r0000000", "at": "p0000000", "direction": "same"}, {"id": "k0000001", "type": "tangent_at", "a": "r0000000", "b": "a0000000", "at": "p0000002", "direction": "same"}"#,
    );
    let edited = unjoin(
        sketch_of(&before),
        &id("p0000000"),
        &id("a0000000"),
        id("z0000000"),
    )
    .unwrap();
    assert_eq!(gate(&before, &edited.sketch), Vec::<String>::new());
    assert_eq!(edited.removed, ids(&["k0000000"]));
    assert_eq!(edited.sketch.lines[0].from, id("z0000000"));
    assert_eq!(edited.sketch.lines[0].to, id("p0000002"));
    // The arc's own use of p0 is untouched.
    assert_eq!(edited.sketch.arcs[0].end, id("p0000000"));
}

#[test]
fn unjoining_at_a_three_way_joint_keeps_the_others_tangent_at_and_tangent_on() {
    // a, b, and c meet at p1; a tangent_at joins a and b there, and c ends there tangent
    // on circle e. Unjoining b drops the tangent_at (b's), and keeps c's tangent_on.
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000001", "p0000002"),
        line("c0000000", "q0000001", "p0000001"),
    ]
    .join(", ");
    let before = doc(
        POINTS,
        &lines,
        &circle("e0000000", "q0000000", 10.0),
        "",
        r#"{"id": "k0000000", "type": "tangent_at", "a": "a0000000", "b": "b0000000", "at": "p0000001", "direction": "same"}, {"id": "k0000001", "type": "tangent_on", "a": "c0000000", "at": "p0000001", "b": "e0000000", "direction": "same"}, {"id": "k0000002", "type": "tangent_at", "a": "a0000000", "b": "c0000000", "at": "p0000001", "direction": "same"}"#,
    );
    assert_eq!(gate(&before, sketch_of(&before)), Vec::<String>::new());
    let edited = unjoin(
        sketch_of(&before),
        &id("p0000001"),
        &id("b0000000"),
        id("z0000000"),
    )
    .unwrap();
    assert_eq!(gate(&before, &edited.sketch), Vec::<String>::new());
    assert_eq!(edited.removed, ids(&["k0000000"]));
    assert_eq!(edited.modified, ids(&["b0000000"]));
    let kept: Vec<&str> = edited
        .sketch
        .constraints
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(kept, ["k0000001", "k0000002"]);
}

#[test]
fn unjoining_into_the_points_own_id_or_the_curves_is_refused() {
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000001", "p0000002"),
    ]
    .join(", ");
    let before = doc(POINTS, &lines, "", "", "");
    for taken in ["p0000001", "b0000000"] {
        let error = unjoin(
            sketch_of(&before),
            &id("p0000001"),
            &id("b0000000"),
            id(taken),
        )
        .unwrap_err();
        assert_eq!(
            (error.message(), error.parameter()),
            (
                format!("the sketch already has a member {taken}").as_str(),
                Some("ids.0")
            )
        );
    }
}

#[test]
fn unjoining_names_a_curve_that_is_a_constraint_or_a_point_as_no_curve() {
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000001", "p0000002"),
    ]
    .join(", ");
    let before = doc(
        POINTS,
        &lines,
        "",
        "",
        r#"{"id": "k0000000", "type": "horizontal", "line": "a0000000"}"#,
    );
    for not_a_curve in ["k0000000", "p0000002"] {
        let error = unjoin(
            sketch_of(&before),
            &id("p0000001"),
            &id(not_a_curve),
            id("z0000000"),
        )
        .unwrap_err();
        assert_eq!(error.parameter(), Some("curve"));
        assert_eq!(
            error.message(),
            format!("the sketch has no curve {not_a_curve}")
        );
    }
    // A point that's a curve: refused naming `point`.
    let error = unjoin(
        sketch_of(&before),
        &id("a0000000"),
        &id("b0000000"),
        id("z0000000"),
    )
    .unwrap_err();
    assert_eq!(error.parameter(), Some("point"));
    assert_eq!(error.message(), "the sketch has no point a0000000");
}

#[test]
fn unjoining_then_merging_back_gives_the_sketch_again() {
    // With no tangent_at at the point (which the unjoin removes for good), merging the new
    // point back into the old gives the original sketch, its tangent_on moved back too.
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000001", "p0000002"),
    ]
    .join(", ");
    let before = doc(
        POINTS,
        &lines,
        &circle("c0000000", "q0000001", 10.0),
        "",
        r#"{"id": "k0000000", "type": "tangent_on", "a": "b0000000", "at": "p0000001", "b": "c0000000", "direction": "same"}, {"id": "k0000001", "type": "length", "line": "b0000000", "value": 10, "driving": true}"#,
    );
    let apart = unjoin(
        sketch_of(&before),
        &id("p0000001"),
        &id("b0000000"),
        id("z0000000"),
    )
    .unwrap();
    let back = merge(&apart.sketch, &ids(&["p0000001", "z0000000"])).unwrap();
    assert!(back.sketch.canonically_eq(sketch_of(&before)));
    assert_eq!(
        write(&with_sketch(&before, &back.sketch)).unwrap(),
        write(&before).unwrap()
    );
    assert_eq!(back.modified, ids(&["b0000000", "k0000000"]));
    assert_eq!(back.removed, ids(&["z0000000"]));
}

// ------------------------------------------------------------------ property tests

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x5e7c_ed17),
        ..ProptestConfig::default()
    }
}

/// What a random sketch is made of: indices into its points, taken modulo their count.
#[derive(Debug, Clone)]
struct Recipe {
    /// Distinct places on a 10 mm grid, so no curve is of no size.
    places: Vec<(i32, i32)>,
    lines: Vec<(usize, usize)>,
    circles: Vec<(usize, u8)>,
    arcs: Vec<(usize, usize, usize)>,
    /// A form, three references, and a choice (a side or direction).
    constraints: Vec<(u8, usize, usize, usize, u8)>,
}

fn recipe() -> impl Strategy<Value = Recipe> {
    let grid: Vec<(i32, i32)> = (0..5)
        .flat_map(|x| (0..5).map(move |y| (x * 10, y * 10)))
        .collect();
    (
        Just(grid).prop_shuffle(),
        3usize..=7,
        prop::collection::vec((0usize..8, 0usize..8), 0..=6),
        prop::collection::vec((0usize..8, 1u8..=4), 0..=2),
        prop::collection::vec((0usize..8, 0usize..8, 0usize..8), 0..=2),
        prop::collection::vec(
            (0u8..21, 0usize..32, 0usize..32, 0usize..32, 0u8..4),
            0..=14,
        ),
    )
        .prop_map(|(grid, n, lines, circles, arcs, constraints)| Recipe {
            places: grid[..n].to_vec(),
            lines,
            circles,
            arcs,
            constraints,
        })
}

/// A recipe's constraint as JSON, its references taken from pools by what each place
/// takes (points with the origin; curves with the axes; anything), so the gate takes a
/// fair share of them.
fn constraint_json(
    n: usize,
    (form, r1, r2, r3, choice): (u8, usize, usize, usize, u8),
    points: &[String],
    curves: &[String],
    any: &[String],
    ends: &[(String, String)],
    shared: &[(String, String, String)],
) -> String {
    let p = |r: usize| points[r % points.len()].clone();
    let c = |r: usize| curves[r % curves.len()].clone();
    let a = |r: usize| any[r % any.len()].clone();
    let side = ["left", "right"][usize::from(choice % 2)];
    let tside = ["left", "right", "outside", "inside"][usize::from(choice % 4)];
    let dir = ["same", "opposite"][usize::from(choice % 2)];
    let id = format!("k{n:07}");
    let body = match form {
        0 => format!(r#""type": "horizontal", "line": "{}""#, c(r1)),
        1 => format!(r#""type": "vertical", "line": "{}""#, c(r1)),
        2 => format!(
            r#""type": "horizontal_points", "a": "{}", "b": "{}""#,
            p(r1),
            p(r2)
        ),
        3 => format!(
            r#""type": "vertical_points", "a": "{}", "b": "{}""#,
            p(r1),
            p(r2)
        ),
        4 => format!(
            r#""type": "length", "line": "{}", "value": 10, "driving": true"#,
            c(r1)
        ),
        5 => format!(
            r#""type": "distance", "a": "{}", "b": "{}", "value": 10, "driving": true"#,
            p(r1),
            p(r2)
        ),
        6 => format!(
            r#""type": "distance_x", "a": "{}", "b": "{}", "value": 10, "driving": true"#,
            p(r1),
            p(r2)
        ),
        7 => format!(
            r#""type": "distance_y", "a": "{}", "b": "{}", "value": 10, "driving": true"#,
            p(r1),
            p(r2)
        ),
        8 => format!(
            r#""type": "point_line_distance", "point": "{}", "line": "{}", "value": 5, "side": "{side}", "driving": true"#,
            p(r1),
            c(r2)
        ),
        9 => format!(
            r#""type": "on", "point": "{}", "curve": "{}""#,
            p(r1),
            c(r2)
        ),
        10 => format!(r#""type": "parallel", "a": "{}", "b": "{}""#, c(r1), c(r2)),
        11 => format!(
            r#""type": "perpendicular", "a": "{}", "b": "{}""#,
            c(r1),
            c(r2)
        ),
        12 => format!(
            r#""type": "angle", "a": "{}", "b": "{}", "value": 30, "driving": false"#,
            c(r1),
            c(r2)
        ),
        13 => format!(
            r#""type": "radius", "curve": "{}", "value": 5, "driving": true"#,
            c(r1)
        ),
        14 => format!(
            r#""type": "diameter", "curve": "{}", "value": 10, "driving": true"#,
            c(r1)
        ),
        15 => format!(r#""type": "equal", "a": "{}", "b": "{}""#, c(r1), c(r2)),
        16 => format!(
            r#""type": "tangent", "a": "{}", "b": "{}", "side": "{tside}""#,
            c(r1),
            c(r2)
        ),
        // A curve's end, mostly, as a tangent_on's at must be.
        17 if !ends.is_empty() && choice < 3 => {
            let (a, at) = &ends[r1 % ends.len()];
            format!(
                r#""type": "tangent_on", "a": "{a}", "at": "{at}", "b": "{}", "direction": "{dir}""#,
                c(r3)
            )
        }
        17 => format!(
            r#""type": "tangent_on", "a": "{}", "at": "{}", "b": "{}", "direction": "{dir}""#,
            c(r1),
            p(r2),
            c(r3)
        ),
        // Two curves that share an end, mostly, as a tangent_at's must.
        18 if !shared.is_empty() && choice < 3 => {
            let (a, b, at) = &shared[r1 % shared.len()];
            format!(
                r#""type": "tangent_at", "a": "{a}", "b": "{b}", "at": "{at}", "direction": "{dir}""#
            )
        }
        18 => format!(
            r#""type": "tangent_at", "a": "{}", "b": "{}", "at": "{}", "direction": "{dir}""#,
            c(r1),
            c(r2),
            p(r3)
        ),
        19 => format!(
            r#""type": "symmetric", "a": "{}", "b": "{}", "about": "{}""#,
            p(r1),
            p(r2),
            a(r3)
        ),
        _ => format!(
            r#""type": "midpoint", "point": "{}", "line": "{}""#,
            p(r1),
            c(r2)
        ),
    };
    format!(r#"{{"id": "{id}", {body}}}"#)
}

/// The recipe's sketch, every constraint the gate refuses (alone, with those before it)
/// left out; none if its geometry is refused.
fn build(recipe: &Recipe) -> Option<Document> {
    let n = recipe.places.len();
    let pid = |i: usize| format!("p{:07}", i % n);
    let points: Vec<String> = recipe
        .places
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| point(&pid(i), f64::from(x), f64::from(y)))
        .collect();
    let mut lines = Vec::new();
    let mut curve_ids = Vec::new();
    for (i, &(a, b)) in recipe.lines.iter().enumerate() {
        if a % n != b % n {
            let id = format!("n{i:07}");
            lines.push(line(&id, &pid(a), &pid(b)));
            curve_ids.push(id);
        }
    }
    let mut circles = Vec::new();
    for (i, &(c, r)) in recipe.circles.iter().enumerate() {
        let id = format!("c{i:07}");
        circles.push(circle(&id, &pid(c), f64::from(r) * 2.5));
        curve_ids.push(id);
    }
    let mut arcs = Vec::new();
    for (i, &(c, s, e)) in recipe.arcs.iter().enumerate() {
        let (c, s, e) = (c % n, s % n, e % n);
        if c != s && c != e && s != e {
            let id = format!("r{i:07}");
            arcs.push(arc(&id, &pid(c), &pid(s), &pid(e)));
            curve_ids.push(id);
        }
    }
    let mut point_pool: Vec<String> = (0..n).map(pid).collect();
    point_pool.push("origin".to_owned());
    let mut curve_pool = curve_ids.clone();
    curve_pool.extend(["x_axis".to_owned(), "y_axis".to_owned()]);
    let any: Vec<String> = point_pool.iter().chain(&curve_pool).cloned().collect();
    // Each line's and arc's ends, and each pair of them sharing one.
    let ends: Vec<(String, String)> = recipe
        .lines
        .iter()
        .enumerate()
        .filter(|(_, (a, b))| a % n != b % n)
        .flat_map(|(i, &(a, b))| [(format!("n{i:07}"), pid(a)), (format!("n{i:07}"), pid(b))])
        .chain(
            recipe
                .arcs
                .iter()
                .enumerate()
                .filter(|(_, (c, s, e))| c % n != s % n && c % n != e % n && s % n != e % n)
                .flat_map(|(i, &(_, s, e))| {
                    [(format!("r{i:07}"), pid(s)), (format!("r{i:07}"), pid(e))]
                }),
        )
        .collect();
    let mut shared = Vec::new();
    for (x, (a, p)) in ends.iter().enumerate() {
        for (b, q) in &ends[x + 1..] {
            if a != b && p == q {
                shared.push((a.clone(), b.clone(), p.clone()));
            }
        }
    }
    let text = |constraints: &[String]| {
        document(&fields(
            &points.join(", "),
            &lines.join(", "),
            &circles.join(", "),
            &arcs.join(", "),
            &constraints.join(", "),
        ))
    };
    let geometry = read(text(&[]).as_bytes()).ok()?;
    validate(&geometry).ok()?;
    let mut kept: Vec<String> = Vec::new();
    for (k, &c) in recipe.constraints.iter().enumerate() {
        let json = constraint_json(k, c, &point_pool, &curve_pool, &any, &ends, &shared);
        let mut trial = kept.clone();
        trial.push(json);
        if let Ok(document) = read(text(&trial).as_bytes())
            && validate(&document).is_ok()
        {
            kept = trial;
        }
    }
    read(text(&kept).as_bytes()).ok()
}

/// Every member of a sketch by ID: its kind's place in the format's order, and what it
/// holds.
fn members(sketch: &Sketch) -> BTreeMap<String, (u8, String)> {
    let mut all = BTreeMap::new();
    for p in &sketch.points {
        all.insert(p.id.to_string(), (0, format!("{p:?}")));
    }
    for l in &sketch.lines {
        all.insert(l.id.to_string(), (1, format!("{l:?}")));
    }
    for c in &sketch.circles {
        all.insert(c.id.to_string(), (2, format!("{c:?}")));
    }
    for a in &sketch.arcs {
        all.insert(a.id.to_string(), (3, format!("{a:?}")));
    }
    for c in &sketch.constraints {
        all.insert(c.id.to_string(), (4, format!("{c:?}")));
    }
    all
}

/// What an edit added, modified, and removed, worked out from the sketches alone, each by
/// kind and then by ID.
fn diff(before: &Sketch, after: &Sketch) -> [Vec<String>; 3] {
    let (b, a) = (members(before), members(after));
    let sorted = |mut v: Vec<(u8, String)>| {
        v.sort();
        v.into_iter().map(|(_, id)| id).collect::<Vec<_>>()
    };
    let added = a
        .iter()
        .filter(|(id, _)| !b.contains_key(*id))
        .map(|(id, (k, _))| (*k, id.clone()))
        .collect();
    let modified = a
        .iter()
        .filter(|(id, held)| b.get(*id).is_some_and(|was| was != *held))
        .map(|(id, (k, _))| (*k, id.clone()))
        .collect();
    let removed = b
        .iter()
        .filter(|(id, _)| !a.contains_key(*id))
        .map(|(id, (k, _))| (*k, id.clone()))
        .collect();
    [sorted(added), sorted(modified), sorted(removed)]
}

fn lists(edited: &Edited) -> [Vec<String>; 3] {
    let text = |v: &[ElementId]| v.iter().map(|id| id.to_string()).collect::<Vec<_>>();
    [
        text(&edited.added),
        text(&edited.modified),
        text(&edited.removed),
    ]
}

/// Every reference of the sketch names one of its members, or a fixed reference: what any
/// edit must keep, whatever else the gate holds it to.
fn dangling(sketch: &Sketch) -> Vec<String> {
    let all = members(sketch);
    let has = |id: &ElementId| all.contains_key(id.as_str());
    let mut missing = Vec::new();
    for l in &sketch.lines {
        for p in [&l.from, &l.to] {
            if !has(p) {
                missing.push(format!("{}: {p}", l.id));
            }
        }
    }
    for c in &sketch.circles {
        if !has(&c.center) {
            missing.push(format!("{}: {}", c.id, c.center));
        }
    }
    for a in &sketch.arcs {
        for p in [&a.center, &a.start, &a.end] {
            if !has(p) {
                missing.push(format!("{}: {p}", a.id));
            }
        }
    }
    for c in &sketch.constraints {
        for r in references(&c.form) {
            if let Reference::Element(e) = r
                && !has(e)
            {
                missing.push(format!("{}: {e}", c.id));
            }
        }
    }
    missing
}

/// A form's references, read here by hand (the core's own list is the crate's).
fn references(form: &Form) -> Vec<&Reference> {
    match form {
        Form::Horizontal { line } | Form::Vertical { line } => vec![line],
        Form::HorizontalPoints { a, b }
        | Form::VerticalPoints { a, b }
        | Form::Distance { a, b, .. }
        | Form::DistanceX { a, b, .. }
        | Form::DistanceY { a, b, .. }
        | Form::Parallel { a, b }
        | Form::Perpendicular { a, b }
        | Form::Angle { a, b, .. }
        | Form::Equal { a, b }
        | Form::Tangent { a, b, .. } => vec![a, b],
        Form::Length { line, .. } => vec![line],
        Form::PointLineDistance { point, line, .. } | Form::Midpoint { point, line } => {
            vec![point, line]
        }
        Form::On { point, curve } => vec![point, curve],
        Form::Radius { curve, .. } | Form::Diameter { curve, .. } => vec![curve],
        Form::TangentOn { a, at, b, .. } | Form::TangentAt { a, b, at, .. } => vec![a, at, b],
        Form::Symmetric { a, b, about } => vec![a, b, about],
    }
}

/// The sketch with its lists reversed: the same sketch, as its canonical text writes it.
fn reversed(sketch: &Sketch) -> Sketch {
    let mut s = sketch.clone();
    s.points.reverse();
    s.lines.reverse();
    s.circles.reverse();
    s.arcs.reverse();
    s.constraints.reverse();
    s
}

fn all_ids(sketch: &Sketch) -> Vec<ElementId> {
    sketch.elements().map(|(id, _)| id.clone()).collect()
}

proptest! {
    #![proptest_config(config(1500))]

    /// A remove of any members: refused, or a sketch the gate takes whose lists are the
    /// diff's, the same however the sketch's lists and the request are ordered.
    #[test]
    fn any_remove_is_refused_or_gated_and_says_what_changed(
        recipe in recipe(),
        picks in prop::collection::vec(0usize..64, 0..6),
        unknown in any::<bool>(),
    ) {
        let Some(before) = build(&recipe) else { return Ok(()) };
        let sketch = sketch_of(&before);
        let all = all_ids(sketch);
        prop_assume!(!all.is_empty());
        let mut named: Vec<ElementId> = picks.iter().map(|&i| all[i % all.len()].clone()).collect();
        if unknown {
            named.push(id("zzzzzzzz"));
        }
        match remove(sketch, &named) {
            Err(refused) => {
                prop_assert!(!refused.errors.is_empty());
                prop_assert!(refused.total >= refused.errors.len() as u64);
                for e in &refused.errors {
                    prop_assert!(e.parameter().is_some_and(|p| p.starts_with("members.")), "{e:?}");
                }
            }
            Ok(edited) => {
                prop_assert!(!unknown);
                prop_assert_eq!(gate(&before, &edited.sketch), Vec::<String>::new());
                prop_assert_eq!(dangling(&edited.sketch), Vec::<String>::new());
                prop_assert_eq!(lists(&edited), diff(sketch, &edited.sketch));
                for id in &named {
                    prop_assert!(edited.removed.contains(id), "{id} named but kept");
                }
                let mut backwards = named.clone();
                backwards.reverse();
                let other = remove(&reversed(sketch), &backwards).unwrap();
                prop_assert!(other.sketch.canonically_eq(&edited.sketch));
                prop_assert_eq!(lists(&other), lists(&edited));
            }
        }
    }

    /// A batch of curves, constraints, and lone points removes what they'd remove one by
    /// one (decision 7, FreeCAD 41c91574cf), each skipped once it's gone.
    #[test]
    fn a_remove_batch_equals_its_members_one_by_one(
        recipe in recipe(),
        picks in prop::collection::vec(0usize..64, 1..6),
    ) {
        let Some(before) = build(&recipe) else { return Ok(()) };
        let sketch = sketch_of(&before);
        let used: Vec<&ElementId> = sketch.lines.iter().flat_map(|l| [&l.from, &l.to])
            .chain(sketch.circles.iter().map(|c| &c.center))
            .chain(sketch.arcs.iter().flat_map(|a| [&a.center, &a.start, &a.end]))
            .collect();
        let candidates: Vec<ElementId> = all_ids(sketch).into_iter()
            .filter(|id| !used.contains(&id))
            .collect();
        prop_assume!(!candidates.is_empty());
        let named: Vec<ElementId> = picks.iter().map(|&i| candidates[i % candidates.len()].clone()).collect();
        let batch = remove(sketch, &named).unwrap();
        let mut current = sketch.clone();
        for id in &named {
            if all_ids(&current).contains(id) {
                current = remove(&current, std::slice::from_ref(id)).unwrap().sketch;
            }
        }
        prop_assert!(batch.sketch.canonically_eq(&current));
    }

    /// An unjoin of any point from any curve: refused naming its parameter, or a sketch the
    /// gate takes whose lists are the diff's; and merging the new point back gives the
    /// sketch again, less the tangent_ats the unjoin removed.
    #[test]
    fn any_unjoin_is_refused_or_gated_and_merges_back(
        recipe in recipe(),
        point in 0usize..16,
        curve in 0usize..16,
        taken in 0u8..8,
    ) {
        let Some(before) = build(&recipe) else { return Ok(()) };
        let sketch = sketch_of(&before);
        let curves: Vec<ElementId> = sketch.lines.iter().map(|l| l.id.clone())
            .chain(sketch.circles.iter().map(|c| c.id.clone()))
            .chain(sketch.arcs.iter().map(|a| a.id.clone()))
            .collect();
        prop_assume!(!curves.is_empty());
        let p = sketch.points[point % sketch.points.len()].id.clone();
        let c = curves[curve % curves.len()].clone();
        // Now and then the new ID is one the sketch has.
        let new = if taken == 0 { p.clone() } else { id("z0000000") };
        match unjoin(sketch, &p, &c, new.clone()) {
            Err(error) => {
                prop_assert!(matches!(error.parameter(), Some("point" | "curve" | "ids.0")), "{error:?}");
            }
            Ok(edited) => {
                prop_assert_eq!(gate(&before, &edited.sketch), Vec::<String>::new());
                prop_assert_eq!(dangling(&edited.sketch), Vec::<String>::new());
                prop_assert_eq!(lists(&edited), diff(sketch, &edited.sketch));
                let back = merge(&edited.sketch, &[p.clone(), new]).unwrap();
                let mut expected = sketch.clone();
                expected.constraints.retain(|k| !edited.removed.contains(&k.id));
                prop_assert!(back.sketch.canonically_eq(&expected), "{:#?}", back.sketch);
                for k in &edited.removed {
                    let form = &sketch.constraints.iter().find(|x| &x.id == k).unwrap().form;
                    let is_tangent_at = matches!(form, Form::TangentAt { .. });
                    prop_assert!(is_tangent_at);
                }
                let other = unjoin(&reversed(sketch), &p, &c, id("z0000000")).unwrap();
                prop_assert!(other.sketch.canonically_eq(&edited.sketch));
            }
        }
    }

    /// A merge of any points keeps what any edit must: nothing dangling, the first point
    /// where it was, the others gone, and lists that are the diff's.
    #[test]
    fn any_merge_leaves_nothing_dangling_and_says_what_changed(
        recipe in recipe(),
        picks in prop::collection::vec(0usize..16, 0..5),
    ) {
        let Some(before) = build(&recipe) else { return Ok(()) };
        let sketch = sketch_of(&before);
        let named: Vec<ElementId> = picks.iter().map(|&i| sketch.points[i % sketch.points.len()].id.clone()).collect();
        match merge(sketch, &named) {
            Err(error) => {
                prop_assert!(error.parameter().is_some_and(|p| p.starts_with("points")), "{error:?}");
            }
            Ok(edited) => {
                prop_assert_eq!(dangling(&edited.sketch), Vec::<String>::new());
                prop_assert_eq!(lists(&edited), diff(sketch, &edited.sketch));
                let kept = sketch.points.iter().find(|p| p.id == named[0]).unwrap();
                prop_assert!(edited.sketch.points.contains(kept));
                for other in &named[1..] {
                    prop_assert!(!edited.sketch.points.iter().any(|p| &p.id == other));
                }
                let other = merge(&reversed(sketch), &named).unwrap();
                prop_assert!(other.sketch.canonically_eq(&edited.sketch));
            }
        }
    }

    /// A merge of any points: refused, or a sketch the gate takes (the edit's contract).
    // Broke, before the fix: merges whose re-pointed constraints break the gate's rules come back Ok (an
    // `on`, `midpoint`, `point_line_distance`, or `symmetric` onto a curve's own point, a
    // `tangent_on` at a now-shared end, an `on` beside an edge `tangent`): the targeted
    // tests above, merging_*, are each minimal case.
    #[test]
    fn any_merge_is_refused_or_gated(
        recipe in recipe(),
        picks in prop::collection::vec(0usize..16, 2..5),
    ) {
        let Some(before) = build(&recipe) else { return Ok(()) };
        let sketch = sketch_of(&before);
        let named: Vec<ElementId> = picks.iter().map(|&i| sketch.points[i % sketch.points.len()].id.clone()).collect();
        if let Ok(edited) = merge(sketch, &named) {
            prop_assert_eq!(gate(&before, &edited.sketch), Vec::<String>::new());
        }
    }
}
