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
fn a_tangent_at_of_two_curves_survives_removing_a_third_at_their_joint_whatever_the_order() {
    // FreeCAD 358942b771: deleting a third curve at a joint lost the tangencies there; and
    // 1.1.3's result depends on the constraints' order (f8f159c4d3). a, b, d meet at p1, a
    // tangent_at joins a and b, another a and d; removing d takes only d's.
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000001", "p0000002"),
        line("d0000000", "p0000001", "q0000000"),
    ]
    .join(", ");
    let ab = r#"{"id": "k0000001", "type": "tangent_at", "a": "a0000000", "b": "b0000000", "at": "p0000001", "direction": "same"}"#;
    let ad = r#"{"id": "k0000002", "type": "tangent_at", "a": "a0000000", "b": "d0000000", "at": "p0000001", "direction": "same"}"#;
    for constraints in [format!("{ad}, {ab}"), format!("{ab}, {ad}")] {
        let before = doc(POINTS, &lines, "", "", &constraints);
        let edited = remove(sketch_of(&before), &ids(&["d0000000"])).unwrap();
        let sketch = sketch_of(&gated(&before, &edited)).clone();
        assert_eq!(
            (sketch.lines[0].to.clone(), sketch.lines[1].from.clone()),
            (id("p0000001"), id("p0000001"))
        );
        assert_eq!(
            sketch
                .constraints
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["k0000001"]
        );
        assert_eq!(edited.removed, ids(&["q0000000", "d0000000", "k0000002"]));
    }
}

#[test]
fn a_symmetry_about_a_point_another_curve_still_uses_stays() {
    // FreeCAD 51cf34a596: a symmetric's third reference, its about, counts as the others do.
    // q0 and q1 mirror each other about p1, which a and b share; removing b leaves p1 to a.
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
        r#"{"id": "k0000000", "type": "symmetric", "a": "q0000000", "b": "q0000001", "about": "p0000001"}"#,
    );
    let edited = remove(sketch_of(&before), &ids(&["b0000000"])).unwrap();
    let sketch = sketch_of(&gated(&before, &edited)).clone();
    assert_eq!(edited.removed, ids(&["p0000002", "b0000000"]));
    assert_eq!(
        sketch
            .constraints
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["k0000000"]
    );
}

#[test]
fn a_removed_lines_midpoint_becomes_its_ends_symmetry_while_they_stay() {
    // FreeCAD stores a midpoint as a Symmetric of the line's ends about the point, so its
    // line's delete leaves it while neighbours keep the ends (the sketch-edit oracle,
    // remove_line_with_midpoint, 1.1.3). a runs p0 to p1, b p1 to p3, c p3 to p4; the lone
    // p2 at (20, 0) is b's midpoint. Removing b leaves p1 to a and p3 to c: p2 is still
    // midway between them, as a symmetric of p1 and p3 about p2, its ID kept.
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000001", "p0000003"),
        line("c0000000", "p0000003", "p0000004"),
    ]
    .join(", ");
    let midpoint =
        r#"{"id": "k0000000", "type": "midpoint", "point": "p0000002", "line": "b0000000"}"#;
    let before = doc(POINTS, &lines, "", "", midpoint);
    let edited = remove(sketch_of(&before), &ids(&["b0000000"])).unwrap();
    let after = gated(&before, &edited);
    assert_eq!(edited.removed, ids(&["b0000000"]));
    assert_eq!(edited.modified, ids(&["k0000000"]));
    assert!(text(&after).contains(
        r#"{"id": "k0000000", "type": "symmetric", "a": "p0000001", "b": "p0000003", "about": "p0000002"}"#
    ));
    // Without c, p3 goes with b, and the midpoint with it: no symmetric of an end gone.
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000001", "p0000003"),
    ]
    .join(", ");
    let before = doc(POINTS, &lines, "", "", midpoint);
    let edited = remove(sketch_of(&before), &ids(&["b0000000"])).unwrap();
    gated(&before, &edited);
    assert_eq!(edited.removed, ids(&["p0000003", "b0000000", "k0000000"]));
    assert!(edited.modified.is_empty());
    // With the point named too, it goes, and the midpoint with it.
    let before = doc(
        POINTS,
        &[
            line("a0000000", "p0000000", "p0000001"),
            line("b0000000", "p0000001", "p0000003"),
            line("c0000000", "p0000003", "p0000004"),
        ]
        .join(", "),
        "",
        "",
        midpoint,
    );
    let edited = remove(sketch_of(&before), &ids(&["b0000000", "p0000002"])).unwrap();
    gated(&before, &edited);
    assert_eq!(edited.removed, ids(&["p0000002", "b0000000", "k0000000"]));
    assert!(edited.modified.is_empty());
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
    .unwrap_err()
    .errors;
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
    let errors = remove(sketch_of(&before), &ids(&["b0000000", "zzzzzzzz"]))
        .unwrap_err()
        .errors;
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
fn unjoining_a_construction_point_gives_a_construction_point() {
    let points = POINTS.replace(
        r#""id": "p0000001", "x": 10, "y": 0, "construction": false"#,
        r#""id": "p0000001", "x": 10, "y": 0, "construction": true"#,
    );
    assert_ne!(points, POINTS);
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000001", "p0000002"),
    ]
    .join(", ");
    let before = doc(&points, &lines, "", "", "");
    let edited = unjoin(
        sketch_of(&before),
        &id("p0000001"),
        &id("b0000000"),
        id("p0000009"),
    )
    .unwrap();
    let sketch = sketch_of(&gated(&before, &edited)).clone();
    let new = sketch
        .points
        .iter()
        .find(|p| p.id == id("p0000009"))
        .unwrap();
    assert_eq!(
        (new.x.mm(), new.y.mm(), new.construction),
        (10.0, 0.0, true)
    );
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
    assert_eq!(alone.elements(), ["p0000000", "a0000000"]);
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
    assert_eq!(missing.parameter(), Some("curve"));
    assert_eq!(missing.elements(), ["p0000002", "a0000000"]);
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
    // §9's ends carrying a horizontal_points to another point, an on to another curve (d,
    // up x = 10 through them), and a lock from the origin, here beside a symmetric about the
    // merged end too: still merged (1.1.3 reports 0 missing for each, c036128aa0).
    let points = format!("{POINTS}, {}", point("p0000009", 10.0, 0.00000004));
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000009", "p0000002"),
        line("d0000000", "q0000000", "q0000001"),
    ]
    .join(", ");
    let before = doc(
        &points,
        &lines,
        "",
        "",
        r#"{"id": "k0000000", "type": "horizontal_points", "a": "p0000009", "b": "p0000003"}, {"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 10, "driving": true}, {"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000003", "type": "on", "point": "p0000009", "curve": "d0000000"}, {"id": "k0000004", "type": "symmetric", "a": "p0000000", "b": "p0000002", "about": "p0000009"}"#,
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
    use silico_core::document::Form;
    let points_of: Vec<Vec<String>> = sketch
        .constraints
        .iter()
        .map(|c| match &c.form {
            Form::HorizontalPoints { a, b } => vec![a.to_string(), b.to_string()],
            Form::DistanceX { a, b, .. } | Form::DistanceY { a, b, .. } => {
                vec![a.to_string(), b.to_string()]
            }
            Form::On { point, curve } => vec![point.to_string(), curve.to_string()],
            Form::Symmetric { a, b, about } => {
                vec![a.to_string(), b.to_string(), about.to_string()]
            }
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        points_of,
        [
            vec!["p0000001", "p0000003"],
            vec!["origin", "p0000001"],
            vec!["origin", "p0000001"],
            vec!["p0000001", "d0000000"],
            vec!["p0000000", "p0000002", "p0000001"],
        ]
    );
    assert_eq!(
        edited.modified,
        ids(&["b0000000", "k0000000", "k0000003", "k0000004"])
    );
    assert_eq!(edited.removed, ids(&["p0000009"]));
    assert!(edited.added.is_empty());
}

#[test]
fn a_merge_beside_edge_tangents_to_a_curve_that_doesnt_end_there_is_made() {
    // a and b touch at p1 and p9, each tangent to circle c about q0 of radius 10 (y = 0 is
    // tangent to it), listed once as a and once as b: after the merge a and b share an end,
    // c none, so each tangent stays an edge tangency.
    let points = format!("{POINTS}, {}", point("p0000009", 10.0, 0.00000004));
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000009", "p0000002"),
    ]
    .join(", ");
    let before = doc(
        &points,
        &lines,
        r#"{"id": "c0000000", "center": "q0000000", "radius": 10, "construction": false}"#,
        "",
        r#"{"id": "k0000000", "type": "tangent", "a": "a0000000", "b": "c0000000", "side": "left"}, {"id": "k0000001", "type": "tangent", "a": "c0000000", "b": "b0000000", "side": "left"}"#,
    );
    let edited = merge(sketch_of(&before), &ids(&["p0000001", "p0000009"])).unwrap();
    let sketch = sketch_of(&gated(&before, &edited)).clone();
    assert_eq!(sketch.lines[1].from, id("p0000001"));
    assert_eq!(sketch.constraints, sketch_of(&before).constraints);
    assert_eq!(edited.modified, ids(&["b0000000"]));
    assert_eq!(edited.removed, ids(&["p0000009"]));
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
            "merging p0000000 and p0000001 would give line a0000000 one point twice"
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

/// Line a0 from p0 (0, 0) to p1 (10, 0), and `arc` about q0 (10, 10), radius 10, with p9 at
/// (10, 4e-8) one of its ends, held edge tangent to a0 by k0 (`a` and `b` as `pair` names
/// them): a0 touches the circle at (10, 0), its foot from q0.
fn tangent_touching(arc: &str, pair: [&str; 2]) -> Document {
    let points = format!(
        "{POINTS}, {}, {}, {}",
        point("p0000009", 10.0, 0.00000004),
        point("p0000007", 0.0, 10.0),
        point("p0000008", 20.0, 10.0)
    );
    doc(
        &points,
        &line("a0000000", "p0000000", "p0000001"),
        "",
        arc,
        &format!(
            r#"{{"id": "k0000000", "type": "tangent", "a": "{}", "b": "{}", "side": "left"}}"#,
            pair[0], pair[1]
        ),
    )
}

#[test]
fn a_merge_that_leaves_an_edge_tangent_at_a_shared_end_makes_it_a_tangent_at_as_drawn() {
    // FreeCAD's coincident command substitutes an endpoint tangency (620188073e; open item
    // 13). Merged at p1, the path along a0 into (10, 0) runs +x. An arc from p9 (270°)
    // round to (20, 10) leaves it +x too, counter-clockwise at the circle's foot: smooth,
    // `same`, whichever of the two k0 names first (read back along r0, it arrives −x and
    // a0 leaves −x). An arc from (0, 10) (180°) round to p9 arrives +x, so leaving along it
    // runs back −x: a cusp, `opposite`. The constraint keeps its ID.
    let from_p9 = r#"{"id": "r0000000", "center": "q0000000", "start": "p0000009", "end": "p0000008", "construction": false}"#;
    let to_p9 = r#"{"id": "r0000000", "center": "q0000000", "start": "p0000007", "end": "p0000009", "construction": false}"#;
    for (arc, pair, direction) in [
        (from_p9, ["a0000000", "r0000000"], "same"),
        (from_p9, ["r0000000", "a0000000"], "same"),
        (to_p9, ["a0000000", "r0000000"], "opposite"),
        (to_p9, ["r0000000", "a0000000"], "opposite"),
    ] {
        let before = tangent_touching(arc, pair);
        if let Err(invalid) = validate(&before) {
            panic!("the gate refused the sketch before: {:?}", invalid.errors());
        }
        let edited = merge(sketch_of(&before), &ids(&["p0000001", "p0000009"])).unwrap();
        let after = text(&gated(&before, &edited));
        assert!(
            after.contains(&format!(
                r#"{{"id": "k0000000", "type": "tangent_at", "a": "{}", "b": "{}", "at": "p0000001", "direction": "{direction}"}}"#,
                pair[0], pair[1]
            )),
            "{pair:?}: {after}"
        );
        assert!(!after.contains(r#""type": "tangent""#), "{after}");
        assert_eq!(edited.added, Vec::<ElementId>::new());
        assert_eq!(edited.modified, ids(&["r0000000", "k0000000"]));
        assert_eq!(edited.removed, ids(&["p0000009"]));
    }
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
    assert_eq!(twice.elements(), ["p0000001"]);
    let one = merge(sketch_of(&before), &ids(&["p0000001"])).unwrap_err();
    assert_eq!(
        (one.message(), one.parameter()),
        ("a merge takes two or more points", Some("points"))
    );
    let none = merge(sketch_of(&before), &[]).unwrap_err();
    assert_eq!(
        (none.message(), none.parameter()),
        ("a merge takes two or more points", Some("points"))
    );
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

// ---- the mutants run's: curves that don't end at the kept point

/// Lines a from p0 to p1 and b from p9 (touching p1) to p2, and `more` beside them.
fn touching_with(
    points: &[String],
    lines: &[String],
    circles: &str,
    arcs: &str,
    constraints: &str,
) -> Document {
    let all = std::iter::once(POINTS.to_owned())
        .chain(std::iter::once(point("p0000009", 10.0, 0.00000004)))
        .chain(points.iter().cloned())
        .collect::<Vec<_>>()
        .join(", ");
    let lines = [
        line("a0000000", "p0000000", "p0000001"),
        line("b0000000", "p0000009", "p0000002"),
    ]
    .into_iter()
    .chain(lines.iter().cloned())
    .collect::<Vec<_>>()
    .join(", ");
    doc(&all, &lines, circles, arcs, constraints)
}

#[test]
fn an_edge_tangent_of_a_curve_ending_at_the_merged_point_with_one_that_doesnt_is_made() {
    // The mutants run's (line 350's arcs, 344's lines): a, which ends at the kept p1,
    // tangent to an arc r far off, about q1, from t1 (20, -10) to t2 (10, -20), which
    // doesn't end there: merged, a and r share no end, so the tangent stays an edge one.
    let before = touching_with(
        &[
            point("t0000001", 20.0, -10.0),
            point("t0000002", 10.0, -20.0),
        ],
        &[],
        "",
        &crate::fixture::arc("r0000000", "q0000001", "t0000001", "t0000002"),
        r#"{"id": "k0000000", "type": "tangent", "a": "a0000000", "b": "r0000000", "side": "right"}"#,
    );
    let edited = merge(sketch_of(&before), &ids(&["p0000001", "p0000009"])).unwrap();
    gated(&before, &edited);
    assert_eq!(edited.modified, ids(&["b0000000"]));
    assert_eq!(edited.removed, ids(&["p0000009"]));
    // The other way round: an arc r starting at p9, merged into the kept p1, tangent to
    // a line d far off, from q0 (10, 10) to t1 (20, 10), which doesn't end there.
    let before = touching_with(
        &[
            point("t0000001", 20.0, 10.0),
            point("t0000002", 20.0, -10.0),
        ],
        &[line("d0000000", "q0000000", "t0000001")],
        "",
        &crate::fixture::arc("r0000000", "q0000001", "p0000009", "t0000002"),
        r#"{"id": "k0000000", "type": "tangent", "a": "d0000000", "b": "r0000000", "side": "right"}"#,
    );
    let edited = merge(sketch_of(&before), &ids(&["p0000001", "p0000009"])).unwrap();
    gated(&before, &edited);
    assert_eq!(edited.modified, ids(&["b0000000", "r0000000"]));
}

#[test]
fn an_edge_tangent_beside_an_on_of_another_point_on_its_curve_is_made() {
    // The mutants run's (line 487): a tangent to the circle c about q0 of radius 10, and a
    // lone point s held on c, at (20, 10), no end of a: merged, nothing holds an end of one
    // on the other, so the tangent stays an edge one.
    let before = touching_with(
        &[point("s0000000", 20.0, 10.0)],
        &[],
        &crate::fixture::circle("c0000000", "q0000000", 10.0),
        "",
        r#"{"id": "k0000000", "type": "tangent", "a": "a0000000", "b": "c0000000", "side": "left"}, {"id": "k0000001", "type": "on", "point": "s0000000", "curve": "c0000000"}"#,
    );
    let edited = merge(sketch_of(&before), &ids(&["p0000001", "p0000009"])).unwrap();
    gated(&before, &edited);
    assert_eq!(edited.modified, ids(&["b0000000"]));
}
