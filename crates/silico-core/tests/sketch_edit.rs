//! The sketch edits' geometry and constraints (docs/ports/sketch-edit.md, §2, §5, §6, and
//! §9's tests): removing members, unjoining a shared point, merging points. Each result goes
//! through the validation gate, and each expected sketch is written by hand.

use silico_core::Error;
use silico_core::document::sketch_edit::{Edited, merge, remove, unjoin};
use silico_core::document::{Document, ElementId, Operation, Sketch, read, validate, write};

use crate::fixture::{document, fields, line, point};

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn ids(texts: &[&str]) -> Vec<ElementId> {
    texts.iter().map(|t| id(t)).collect()
}

/// The document of one sketch these lists make, read.
fn doc(points: &str, lines: &str, circles: &str, arcs: &str, constraints: &str) -> Document {
    read(document(&fields(points, lines, circles, arcs, constraints)).as_bytes()).unwrap()
}

fn sketch_of(document: &Document) -> &Sketch {
    match &document.features[0].operation {
        Operation::Sketch(sketch) => sketch,
        other => panic!("{other:?}"),
    }
}

/// `document` with its sketch replaced by the edit's, checked by the gate.
fn gated(document: &Document, edited: &Edited) -> Document {
    let mut document = document.clone();
    document.features[0].operation = Operation::Sketch(edited.sketch.clone());
    if let Err(invalid) = validate(&document) {
        panic!("the gate refused the edit: {:?}", invalid.errors());
    }
    document
}

/// The document's text, as a save writes it.
fn text(document: &Document) -> String {
    write(document).unwrap()
}

fn messages(errors: &[Error]) -> Vec<String> {
    errors.iter().map(|e| e.message().to_owned()).collect()
}

// Points along x: p0 (0, 0) to p4 (40, 0), and q0 (10, 10), q1 (10, -10) off the line.
const POINTS: &str = r#"{"id": "p0000000", "x": 0, "y": 0, "construction": false}, {"id": "p0000001", "x": 10, "y": 0, "construction": false}, {"id": "p0000002", "x": 20, "y": 0, "construction": false}, {"id": "p0000003", "x": 30, "y": 0, "construction": false}, {"id": "p0000004", "x": 40, "y": 0, "construction": false}, {"id": "q0000000", "x": 10, "y": 10, "construction": false}, {"id": "q0000001", "x": 10, "y": -10, "construction": false}"#;

/// Three lines meeting at p1: a from p0, b to p2, c up to q0.
fn joint() -> Document {
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000001", "p0000002"),
        line("c0000000", "p0000001", "q0000000"),
    ]
    .join(", ");
    doc(
        POINTS,
        &lines,
        "",
        "",
        r#"{"id": "k0000000", "type": "horizontal", "line": "b0000000"}, {"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 20, "driving": true}, {"id": "k0000002", "type": "vertical", "line": "c0000000"}"#,
    )
}

// ---- remove

#[test]
fn removing_a_curve_at_a_three_way_joint_leaves_the_others_sharing_the_point() {
    let before = joint();
    let edited = remove(sketch_of(&before), &ids(&["b0000000"])).unwrap();
    let after = gated(&before, &edited);
    let sketch = sketch_of(&after);
    // a and c still meet at p1; p2, used by b alone, goes with its constraint.
    assert_eq!(
        sketch
            .lines
            .iter()
            .map(|l| l.id.as_str())
            .collect::<Vec<_>>(),
        ["a0000000", "c0000000"]
    );
    assert_eq!(sketch.lines[0].to, id("p0000001"));
    assert_eq!(sketch.lines[1].from, id("p0000001"));
    assert!(!sketch.points.iter().any(|p| p.id == id("p0000002")));
    assert_eq!(
        edited.removed,
        ids(&["p0000002", "b0000000", "k0000000", "k0000001"])
    );
    assert_eq!((edited.added.len(), edited.modified.len()), (0, 0));
    // c's own constraint stays.
    assert_eq!(
        sketch
            .constraints
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["k0000002"]
    );
}

#[test]
fn a_tangent_at_naming_the_removed_curve_goes_with_it_whatever_the_constraints_order() {
    // Two lines joined at p1 by a tangent_at, listed first, then last, beside a horizontal
    // (FreeCAD 1.1.3's result depends on that order: f8f159c4d3).
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000001", "p0000002"),
    ]
    .join(", ");
    let tangent = r#"{"id": "k0000009", "type": "tangent_at", "a": "a0000000", "b": "b0000000", "at": "p0000001", "direction": "same"}"#;
    let horizontal = r#"{"id": "k0000001", "type": "horizontal", "line": "a0000000"}"#;
    let first = doc(POINTS, &lines, "", "", &format!("{tangent}, {horizontal}"));
    let last = doc(POINTS, &lines, "", "", &format!("{horizontal}, {tangent}"));
    let one = remove(sketch_of(&first), &ids(&["b0000000"])).unwrap();
    let other = remove(sketch_of(&last), &ids(&["b0000000"])).unwrap();
    assert_eq!(text(&gated(&first, &one)), text(&gated(&last, &other)));
    assert_eq!(one.removed, ids(&["p0000002", "b0000000", "k0000009"]));
    // a keeps its end p1, and its horizontal.
    let kept = sketch_of(&gated(&first, &one)).clone();
    assert_eq!(kept.lines[0].to, id("p0000001"));
    assert_eq!(kept.constraints.len(), 1);
}

#[test]
fn removing_one_of_two_lines_removes_their_angle_and_a_symmetry_about_it() {
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000002", "p0000003"),
    ]
    .join(", ");
    let before = doc(
        POINTS,
        &lines,
        "",
        "",
        r#"{"id": "k0000000", "type": "angle", "a": "a0000000", "b": "b0000000", "value": 0, "driving": false}, {"id": "k0000001", "type": "symmetric", "a": "q0000000", "b": "q0000001", "about": "b0000000"}"#,
    );
    let edited = remove(sketch_of(&before), &ids(&["b0000000"])).unwrap();
    gated(&before, &edited);
    assert_eq!(
        edited.removed,
        ids(&["p0000002", "p0000003", "b0000000", "k0000000", "k0000001"])
    );
}

#[test]
fn a_lone_point_stays_unless_named_and_a_named_one_takes_its_constraints() {
    // q0 and q1 are used by no curve before the edit: removing a leaves them; naming q0
    // removes it and the distance on it.
    let before = doc(
        POINTS,
        &line("a0000000", "p0000000", "p0000001"),
        "",
        "",
        r#"{"id": "k0000000", "type": "distance", "a": "q0000000", "b": "q0000001", "value": 20, "driving": true}"#,
    );
    let curve_only = remove(sketch_of(&before), &ids(&["a0000000"])).unwrap();
    assert_eq!(
        curve_only.removed,
        ids(&["p0000000", "p0000001", "a0000000"])
    );
    let sketch = sketch_of(&gated(&before, &curve_only)).clone();
    assert!(sketch.points.iter().any(|p| p.id == id("q0000000")));
    let point = remove(sketch_of(&before), &ids(&["q0000000"])).unwrap();
    gated(&before, &point);
    assert_eq!(point.removed, ids(&["q0000000", "k0000000"]));
}

#[test]
fn a_batch_equals_its_members_removed_one_by_one_and_takes_a_member_named_twice_once() {
    let before = joint();
    let batch = remove(
        sketch_of(&before),
        &ids(&["k0000002", "b0000000", "b0000000"]),
    )
    .unwrap();
    let first = remove(sketch_of(&before), &ids(&["b0000000"])).unwrap();
    let then = remove(&first.sketch, &ids(&["k0000002"])).unwrap();
    assert_eq!(text(&gated(&before, &batch)), text(&gated(&before, &then)));
    assert_eq!(
        batch.removed,
        ids(&["p0000002", "b0000000", "k0000000", "k0000001", "k0000002"])
    );
}

#[test]
fn a_point_and_every_curve_that_uses_it_go_together() {
    // Judged whole: the curves first, then the point, which no curve then uses.
    let before = joint();
    let edited = remove(
        sketch_of(&before),
        &ids(&["p0000001", "a0000000", "b0000000", "c0000000"]),
    )
    .unwrap();
    let sketch = sketch_of(&gated(&before, &edited)).clone();
    assert!(sketch.lines.is_empty());
    assert_eq!(
        sketch
            .points
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        ["p0000003", "p0000004", "q0000001"]
    );
}

#[test]
fn a_point_a_curve_still_uses_is_refused_naming_the_curves() {
    let before = joint();
    let errors = remove(
        sketch_of(&before),
        &ids(&["k0000000", "p0000001", "a0000000"]),
    )
    .unwrap_err();
    assert_eq!(
        messages(&errors),
        ["point p0000001 is used by lines b0000000 and c0000000"]
    );
    let error = &errors[0];
    assert_eq!(error.parameter(), Some("members.1"));
    assert_eq!(error.elements(), ["p0000001", "b0000000", "c0000000"]);
    assert_eq!(
        error.suggestion(),
        Some("remove the curves that use it with it, or unjoin it first")
    );
}

#[test]
fn a_member_the_sketch_hasnt_is_refused_and_the_origin_cant_be_named() {
    let before = joint();
    let errors = remove(sketch_of(&before), &ids(&["b0000000", "zzzzzzzz"])).unwrap_err();
    assert_eq!(messages(&errors), ["the sketch has no member zzzzzzzz"]);
    assert_eq!(errors[0].parameter(), Some("members.1"));
    // The fixed references aren't members: no ID names them (FreeCAD 2a401ee895, 83d14b785e).
    for fixed in ["origin", "x_axis", "y_axis"] {
        assert_eq!(ElementId::new(fixed), None);
    }
}

// ---- unjoin

#[test]
fn unjoining_moves_one_curve_to_a_new_point_and_leaves_the_others_joined() {
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000001", "p0000002"),
        line("c0000000", "p0000001", "q0000000"),
    ]
    .join(", ");
    let before = doc(
        POINTS,
        &lines,
        "",
        "",
        r#"{"id": "k0000000", "type": "length", "line": "b0000000", "value": 10, "driving": true}, {"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 10, "driving": true}, {"id": "k0000002", "type": "tangent_at", "a": "a0000000", "b": "b0000000", "at": "p0000001", "direction": "same"}"#,
    );
    let edited = unjoin(
        sketch_of(&before),
        &id("p0000001"),
        &id("b0000000"),
        id("p0000009"),
    )
    .unwrap();
    let sketch = sketch_of(&gated(&before, &edited)).clone();
    // b runs from the new point, at p1's coordinates; a and c still share p1.
    let b = sketch
        .lines
        .iter()
        .find(|l| l.id == id("b0000000"))
        .unwrap();
    assert_eq!(b.from, id("p0000009"));
    let new = sketch
        .points
        .iter()
        .find(|p| p.id == id("p0000009"))
        .unwrap();
    assert_eq!(
        (new.x.mm(), new.y.mm(), new.construction),
        (10.0, 0.0, false)
    );
    assert_eq!(sketch.lines[0].to, id("p0000001"));
    assert_eq!(sketch.lines[2].from, id("p0000001"));
    // b keeps its length; p1's distance stays with p1; the tangent_at at p1 goes.
    let constraints: Vec<&str> = sketch.constraints.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(constraints, ["k0000000", "k0000001"]);
    assert_eq!(edited.added, ids(&["p0000009"]));
    assert_eq!(edited.modified, ids(&["b0000000"]));
    assert_eq!(edited.removed, ids(&["k0000002"]));
}

#[test]
fn unjoining_moves_a_tangent_on_of_the_detached_curve_to_its_new_point() {
    // b ends at p1 tangent on a circle about q1 of radius 10 that p1 lies on; a shares p1.
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000001", "p0000002"),
    ]
    .join(", ");
    let before = doc(
        POINTS,
        &lines,
        r#"{"id": "c0000000", "center": "q0000001", "radius": 10, "construction": false}"#,
        "",
        r#"{"id": "k0000000", "type": "tangent_on", "a": "b0000000", "at": "p0000001", "b": "c0000000", "direction": "same"}"#,
    );
    let edited = unjoin(
        sketch_of(&before),
        &id("p0000001"),
        &id("b0000000"),
        id("p0000009"),
    )
    .unwrap();
    let sketch = sketch_of(&gated(&before, &edited)).clone();
    let at = match &sketch.constraints[0].form {
        silico_core::document::Form::TangentOn { at, .. } => at.to_string(),
        other => panic!("{other:?}"),
    };
    assert_eq!(at, "p0000009");
    assert_eq!(edited.modified, ids(&["b0000000", "k0000000"]));
}

#[test]
fn unjoining_a_point_no_other_curve_shares_or_a_curve_that_doesnt_use_it_is_refused() {
    let before = joint();
    let alone = unjoin(
        sketch_of(&before),
        &id("p0000000"),
        &id("a0000000"),
        id("p0000009"),
    )
    .unwrap_err();
    assert_eq!(
        alone.message(),
        "point p0000000 is used by no curve but line a0000000, so there's nothing to unjoin"
    );
    assert_eq!(alone.parameter(), Some("point"));
    let elsewhere = unjoin(
        sketch_of(&before),
        &id("p0000001"),
        &id("q0000000"),
        id("p0000009"),
    )
    .unwrap_err();
    assert_eq!(elsewhere.parameter(), Some("curve"));
    let missing = unjoin(
        sketch_of(&before),
        &id("p0000002"),
        &id("a0000000"),
        id("p0000009"),
    )
    .unwrap_err();
    assert_eq!(
        missing.message(),
        "line a0000000 doesn't use point p0000002"
    );
    let taken = unjoin(
        sketch_of(&before),
        &id("p0000001"),
        &id("b0000000"),
        id("q0000000"),
    )
    .unwrap_err();
    assert_eq!(taken.parameter(), Some("ids.0"));
}

// ---- merge

/// Two lines whose ends touch: a from p0 to p1, b from p9 (10, 0.00000004) to p2.
fn touching(constraints: &str) -> Document {
    let points = format!("{POINTS}, {}", point("p0000009", 10.0, 0.00000004));
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000009", "p0000002"),
    ]
    .join(", ");
    doc(&points, &lines, "", "", constraints)
}

#[test]
fn merging_keeps_the_first_point_where_it_is_and_points_the_others_users_to_it() {
    // The ends carry a horizontal_points to another point, an on to another curve, and a
    // lock from the origin: still merged (1.1.3 reports 0 missing for each).
    let before = touching(
        r#"{"id": "k0000000", "type": "horizontal_points", "a": "p0000009", "b": "p0000003"}, {"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 10, "driving": true}"#,
    );
    let edited = merge(sketch_of(&before), &ids(&["p0000001", "p0000009"])).unwrap();
    let sketch = sketch_of(&gated(&before, &edited)).clone();
    assert!(!sketch.points.iter().any(|p| p.id == id("p0000009")));
    let kept = sketch
        .points
        .iter()
        .find(|p| p.id == id("p0000001"))
        .unwrap();
    assert_eq!((kept.x.mm(), kept.y.mm()), (10.0, 0.0));
    assert_eq!(sketch.lines[1].from, id("p0000001"));
    let points_of = match &sketch.constraints[0].form {
        silico_core::document::Form::HorizontalPoints { a, b } => (a.to_string(), b.to_string()),
        other => panic!("{other:?}"),
    };
    assert_eq!(points_of, ("p0000001".to_owned(), "p0000003".to_owned()));
    assert_eq!(edited.modified, ids(&["b0000000", "k0000000"]));
    assert_eq!(edited.removed, ids(&["p0000009"]));
    assert!(edited.added.is_empty());
}

#[test]
fn a_merge_that_collapses_a_curve_directly_or_through_a_chain_is_refused() {
    // TestCoincidentCommandGui's collapse cases: a's two ends, and a's ends with a third
    // point between them in the list.
    let before = touching("");
    for names in [
        &["p0000000", "p0000001"][..],
        &["p0000000", "q0000000", "p0000001"],
    ] {
        let error = merge(sketch_of(&before), &ids(names)).unwrap_err();
        assert_eq!(
            error.message(),
            "merging them would give line a0000000 one point twice"
        );
        assert_eq!(error.elements(), ["p0000000", "p0000001", "a0000000"]);
    }
}

#[test]
fn a_merge_that_makes_a_constraint_name_one_point_twice_is_refused() {
    let before = touching(
        r#"{"id": "k0000000", "type": "distance", "a": "p0000001", "b": "p0000009", "value": 1, "driving": true}"#,
    );
    let error = merge(sketch_of(&before), &ids(&["p0000001", "p0000009"])).unwrap_err();
    assert_eq!(
        error.message(),
        "merging them would make constraint k0000000 name point p0000001 twice"
    );
    assert_eq!(error.elements(), ["k0000000", "p0000001"]);
}

#[test]
fn a_merge_that_leaves_an_edge_tangent_at_a_shared_end_is_refused() {
    // a line and an arc whose ends touch, an edge tangent between them: after the merge
    // they share an end, where only a tangent_at may hold (decision 10; open item 13 of
    // the M5.0 design would convert it instead, pending the maintainer).
    let points = format!("{POINTS}, {}", point("p0000009", 10.0, 0.00000004));
    let before = doc(
        &points,
        &line("a0000000", "p0000000", "p0000001"),
        "",
        r#"{"id": "r0000000", "center": "q0000000", "start": "p0000009", "end": "p0000002", "construction": false}"#,
        r#"{"id": "k0000000", "type": "tangent", "a": "a0000000", "b": "r0000000", "side": "left"}"#,
    );
    let error = merge(sketch_of(&before), &ids(&["p0000001", "p0000009"])).unwrap_err();
    assert!(
        error
            .message()
            .starts_with("merging them would leave tangent k0000000"),
        "{error}"
    );
    assert_eq!(error.elements(), ["k0000000", "a0000000", "r0000000"]);
}

#[test]
fn a_point_named_twice_fewer_than_two_or_one_the_sketch_hasnt_is_refused() {
    let before = touching("");
    let twice = merge(sketch_of(&before), &ids(&["p0000001", "p0000001"])).unwrap_err();
    assert_eq!(
        twice.message(),
        "point p0000001 is named twice: it's already one point"
    );
    assert_eq!(twice.parameter(), Some("points.1"));
    let one = merge(sketch_of(&before), &ids(&["p0000001"])).unwrap_err();
    assert_eq!(one.message(), "a merge takes two or more points");
    let none = merge(sketch_of(&before), &[]).unwrap_err();
    assert_eq!(none.message(), "a merge takes two or more points");
    let missing = merge(sketch_of(&before), &ids(&["p0000001", "zzzzzzzz"])).unwrap_err();
    assert_eq!(
        (missing.message(), missing.parameter()),
        ("the sketch has no point zzzzzzzz", Some("points.1"))
    );
    let a_line = merge(sketch_of(&before), &ids(&["p0000001", "a0000000"])).unwrap_err();
    assert_eq!(a_line.parameter(), Some("points.1"));
}

#[test]
fn an_edit_leaves_the_sketch_it_was_given_as_it_was() {
    let before = joint();
    let sketch = sketch_of(&before).clone();
    let _ = remove(&sketch, &ids(&["b0000000"])).unwrap();
    let _ = unjoin(&sketch, &id("p0000001"), &id("b0000000"), id("p0000009")).unwrap();
    assert_eq!(&sketch, sketch_of(&before));
}
