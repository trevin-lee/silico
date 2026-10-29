//! Adding members to a sketch (`document::sketch_edit::add`; docs/ports/sketch-edit.md, §2,
//! *Add a point, a curve, a constraint*, and §9's tests; docs/design/m5.0-app.md,
//! `sketch_add`). Each expected sketch, path, and order is written by hand from the design's
//! rules, or from FreeCAD's behavior as the note records it.

use silico_core::document::sketch_edit::{Edited, Members, Refused, add};
use silico_core::document::{
    Direction, Document, ElementId, Form, Operation, Sketch, TangentSide, read, validate, write,
};

use crate::bracket::S0000002;
use crate::fixture::{ARC, CIRCLE, LINE, POINTS, document, fields};

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn ids(texts: &[&str]) -> Vec<ElementId> {
    texts.iter().map(|t| id(t)).collect()
}

fn sketch_of(document: &Document) -> &Sketch {
    match &document.features[0].operation {
        Operation::Sketch(sketch) => sketch,
        other => panic!("{other:?}"),
    }
}

/// A document of the one feature `feature`, read.
fn doc_of(feature: &str) -> Document {
    read(
        format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{feature}]}}"#
        )
        .as_bytes(),
    )
    .unwrap()
}

/// The fixture's base sketch (pa (0, 0), pb (10, 0), pc (0, 10); line na from pa to pb;
/// circle ca about pa; arc aa about pa from pb to pc), with na horizontal.
fn base() -> Document {
    read(
        document(&fields(
            POINTS,
            LINE,
            CIRCLE,
            ARC,
            r#"{"id": "ka000000", "type": "horizontal", "line": "na000000"}"#,
        ))
        .as_bytes(),
    )
    .unwrap()
}

/// The members these lists hold, as a request gives them: read as the document reads a
/// sketch's lists (whose references are the gate's to check, not the reader's).
fn members(points: &str, lines: &str, circles: &str, arcs: &str, constraints: &str) -> Members {
    let read =
        read(document(&fields(points, lines, circles, arcs, constraints)).as_bytes()).unwrap();
    let sketch = sketch_of(&read).clone();
    Members {
        points: sketch.points,
        lines: sketch.lines,
        circles: sketch.circles,
        arcs: sketch.arcs,
        constraints: sketch.constraints,
    }
}

/// `document` with its sketch replaced by the edit's, checked by the whole gate.
fn gated(document: &Document, edited: &Edited) -> Document {
    let mut document = document.clone();
    document.features[0].operation = Operation::Sketch(edited.sketch.clone());
    if let Err(invalid) = validate(&document) {
        panic!("the gate refused the edit: {:?}", invalid.errors());
    }
    document
}

/// The refusal's errors, each as (parameter, message).
fn said(refused: &Refused) -> Vec<(String, String)> {
    refused
        .errors
        .iter()
        .map(|e| {
            (
                e.parameter().unwrap_or_default().to_owned(),
                e.message().to_owned(),
            )
        })
        .collect()
}

fn parameters(refused: &Refused) -> Vec<&str> {
    refused
        .errors
        .iter()
        .map(|e| e.parameter().unwrap_or_default())
        .collect()
}

fn refused(sketch: &Sketch, members: &Members) -> Refused {
    match add(sketch, members) {
        Ok(edited) => panic!("added: {:?}", edited.added),
        Err(refused) => refused,
    }
}

// ---- the design's example

#[test]
fn the_designs_example_adds_a_hole_equal_to_the_first() {
    // docs/design/m5.0-app.md, `sketch_add`: the bracket's base holes, a third added.
    let before = doc_of(S0000002);
    let batch = members(
        r#"{"id": "pb000003", "x": 40, "y": 20, "construction": false}"#,
        "",
        r#"{"id": "cb000003", "center": "pb000003", "radius": 2.75, "construction": false}"#,
        "",
        r#"{"id": "kb000007", "type": "equal", "a": "cb000001", "b": "cb000003"}"#,
    );
    let edited = add(sketch_of(&before), &batch).unwrap();
    assert_eq!(edited.added, ids(&["pb000003", "cb000003", "kb000007"]));
    assert!(edited.modified.is_empty() && edited.removed.is_empty());
    // As a save writes it: each list sorted by ID, the new members where their IDs fall.
    let expected = doc_of(
        r#"{"id": "s0000002", "type": "sketch", "v": 1, "name": "Base holes", "plane": "xy", "points": [{"id": "pb000001", "x": 40, "y": 12, "construction": false}, {"id": "pb000002", "x": 40, "y": 28, "construction": false}, {"id": "pb000003", "x": 40, "y": 20, "construction": false}], "lines": [], "circles": [{"id": "cb000001", "center": "pb000001", "radius": 2.75, "construction": false}, {"id": "cb000002", "center": "pb000002", "radius": 2.75, "construction": false}, {"id": "cb000003", "center": "pb000003", "radius": 2.75, "construction": false}], "arcs": [], "constraints": [{"id": "kb000001", "type": "diameter", "curve": "cb000001", "value": 5.5, "driving": true}, {"id": "kb000002", "type": "equal", "a": "cb000001", "b": "cb000002"}, {"id": "kb000003", "type": "distance_x", "a": "origin", "b": "pb000001", "value": 40, "driving": true}, {"id": "kb000004", "type": "distance_y", "a": "origin", "b": "pb000001", "value": 12, "driving": true}, {"id": "kb000005", "type": "vertical_points", "a": "pb000001", "b": "pb000002"}, {"id": "kb000006", "type": "distance_y", "a": "pb000001", "b": "pb000002", "value": 16, "driving": true}, {"id": "kb000007", "type": "equal", "a": "cb000001", "b": "cb000003"}]}"#,
    );
    assert_eq!(
        write(&gated(&before, &edited)).unwrap(),
        write(&expected).unwrap()
    );
}

// ---- a batch of each kind

#[test]
fn a_batch_of_every_kind_is_added_whole_and_listed_by_kind_each_in_the_order_given() {
    // Each kind given out of ID order, so an `added` sorted by ID would differ: by kind
    // (points, lines, circles, arcs, constraints), then as given (the design's common rules:
    // the order the edit made them).
    let before = base();
    let batch = members(
        r#"{"id": "pz000002", "x": 20, "y": 0, "construction": false}, {"id": "pz000001", "x": 25, "y": 0, "construction": false}, {"id": "pz000003", "x": 20, "y": 5, "construction": false}"#,
        r#"{"id": "nz000002", "from": "pb000000", "to": "pz000002", "construction": false}, {"id": "nz000001", "from": "pz000003", "to": "pc000000", "construction": false}"#,
        r#"{"id": "cz000001", "center": "pz000002", "radius": 2, "construction": false}"#,
        r#"{"id": "az000001", "center": "pz000002", "start": "pz000001", "end": "pz000003", "construction": false}"#,
        r#"{"id": "kz000002", "type": "horizontal", "line": "nz000002"}, {"id": "kz000001", "type": "radius", "curve": "az000001", "value": 5, "driving": true}"#,
    );
    let edited = add(sketch_of(&before), &batch).unwrap();
    assert_eq!(
        edited.added,
        ids(&[
            "pz000002", "pz000001", "pz000003", "nz000002", "nz000001", "cz000001", "az000001",
            "kz000002", "kz000001",
        ])
    );
    assert!(edited.modified.is_empty() && edited.removed.is_empty());
    let after = gated(&before, &edited);
    let sketch = sketch_of(&after);
    let had = sketch_of(&before);
    // The sketch's own members are as they were, and each new one as given.
    assert_eq!(sketch.points[..3], had.points[..]);
    assert_eq!(sketch.points[3..], batch.points[..]);
    assert_eq!(sketch.lines[..1], had.lines[..]);
    assert_eq!(sketch.lines[1..], batch.lines[..]);
    assert_eq!(sketch.circles[..1], had.circles[..]);
    assert_eq!(sketch.circles[1..], batch.circles[..]);
    assert_eq!(sketch.arcs[..1], had.arcs[..]);
    assert_eq!(sketch.arcs[1..], batch.arcs[..]);
    assert_eq!(sketch.constraints[..1], had.constraints[..]);
    assert_eq!(sketch.constraints[1..], batch.constraints[..]);
}

#[test]
fn freecads_box_is_added_in_one_batch_its_joints_shared_points() {
    // TestSketcherSolver.testBoxCase's CreateBoxSketchSet: four lines added one by one, four
    // Coincidents joining them, Horizontal on 0 and 2, Vertical on 1 and 3, and lengths of 81.370787
    // on line 1 and 187.573036 on line 0. In Silico one batch: the four joints are four
    // shared points, so six constraints, not ten. The add doesn't solve, so the lengths
    // differ from the drawing (168.662926 and 90.157303) until the edit's solve.
    let before = read(document(&fields("", "", "", "", "")).as_bytes()).unwrap();
    let batch = members(
        r#"{"id": "p0000000", "x": -99.230339, "y": 36.960674, "construction": false}, {"id": "p0000001", "x": 69.432587, "y": 36.960674, "construction": false}, {"id": "p0000002", "x": 69.432587, "y": -53.196629, "construction": false}, {"id": "p0000003", "x": -99.230339, "y": -53.196629, "construction": false}"#,
        r#"{"id": "n0000000", "from": "p0000000", "to": "p0000001", "construction": false}, {"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}, {"id": "n0000002", "from": "p0000002", "to": "p0000003", "construction": false}, {"id": "n0000003", "from": "p0000003", "to": "p0000000", "construction": false}"#,
        "",
        "",
        r#"{"id": "k0000004", "type": "horizontal", "line": "n0000000"}, {"id": "k0000005", "type": "horizontal", "line": "n0000002"}, {"id": "k0000006", "type": "vertical", "line": "n0000001"}, {"id": "k0000007", "type": "vertical", "line": "n0000003"}, {"id": "k0000008", "type": "length", "line": "n0000001", "value": 81.370787, "driving": true}, {"id": "k0000009", "type": "length", "line": "n0000000", "value": 187.573036, "driving": true}"#,
    );
    let edited = add(sketch_of(&before), &batch).unwrap();
    assert_eq!(
        edited.added,
        ids(&[
            "p0000000", "p0000001", "p0000002", "p0000003", "n0000000", "n0000001", "n0000002",
            "n0000003", "k0000004", "k0000005", "k0000006", "k0000007", "k0000008", "k0000009",
        ])
    );
    let after = gated(&before, &edited);
    let sketch = sketch_of(&after);
    assert_eq!((sketch.points.len(), sketch.constraints.len()), (4, 6));
    // Each corner is two lines' end and start: line i ends where line i + 1 starts.
    for i in 0..4 {
        assert_eq!(
            sketch.lines[i].to,
            sketch.lines[(i + 1) % 4].from,
            "corner {i}"
        );
    }
}

// ---- shared points

#[test]
fn a_line_from_a_point_the_sketch_has_shares_it_with_that_points_curves() {
    // pb (10, 0) is na's end and aa's start. A line from it is joined to both by sharing it
    // (M3.2's D2), where FreeCAD adds a line with its own start and a Coincident: one new
    // point, not two, and no constraint.
    let before = base();
    let batch = members(
        r#"{"id": "pz000001", "x": 20, "y": 0, "construction": false}"#,
        r#"{"id": "nz000001", "from": "pb000000", "to": "pz000001", "construction": false}"#,
        "",
        "",
        "",
    );
    let edited = add(sketch_of(&before), &batch).unwrap();
    assert_eq!(edited.added, ids(&["pz000001", "nz000001"]));
    let after = gated(&before, &edited);
    let sketch = sketch_of(&after);
    assert_eq!(sketch.points.len(), 4);
    assert_eq!(sketch.constraints.len(), 1);
    let users: Vec<&str> = sketch
        .lines
        .iter()
        .filter(|l| l.from == id("pb000000") || l.to == id("pb000000"))
        .map(|l| l.id.as_str())
        .chain(
            sketch
                .arcs
                .iter()
                .filter(|a| a.start == id("pb000000") || a.end == id("pb000000"))
                .map(|a| a.id.as_str()),
        )
        .collect();
    assert_eq!(users, ["na000000", "nz000001", "aa000000"]);
}

// ---- refusals

#[test]
fn a_batch_with_bad_members_is_refused_whole_each_problem_at_its_id_path() {
    // One good point among them; every problem is named at once, in the gate's order (each
    // list in turn), at the request's own path, and nothing is added.
    let before = base();
    let batch = members(
        r#"{"id": "pz000001", "x": 20, "y": 0, "construction": false}"#,
        r#"{"id": "nz000001", "from": "pz000001", "to": "pz100001", "construction": false}"#,
        r#"{"id": "cz000001", "center": "pz000001", "radius": 0, "construction": false}"#,
        "",
        r#"{"id": "kz000001", "type": "length", "line": "na000000", "value": 0, "driving": true}"#,
    );
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(
        parameters(&refused),
        [
            "lines.nz000001.to",
            "circles.cz000001.radius",
            "constraints.kz000001.value"
        ]
    );
    assert_eq!(refused.total, 3);
    // The line's missing end has the new point, a character away, as its only candidate:
    // the base's points are three away.
    assert_eq!(refused.errors[0].candidates(), ["pz000001"]);
    assert_eq!(refused.errors[0].elements(), ["nz000001"]);
    assert_eq!(refused.errors[1].elements(), ["cz000001"]);
    assert_eq!(refused.errors[2].elements(), ["kz000001"]);
}

#[test]
fn a_list_with_one_constraint_naming_no_member_adds_none_of_it() {
    // FreeCAD 1.1.3, the note's probe: `addConstraint([Horizontal(0), Horizontal(5)])` on a
    // sketch of one line adds neither. Here the good one is a horizontal of a line of the
    // same batch, and the bad one names a line no member holds.
    let before = base();
    let batch = members(
        r#"{"id": "pz000001", "x": 20, "y": 0, "construction": false}"#,
        r#"{"id": "nz000001", "from": "pb000000", "to": "pz000001", "construction": false}"#,
        "",
        "",
        r#"{"id": "kz000001", "type": "horizontal", "line": "nz000001"}, {"id": "kz000002", "type": "horizontal", "line": "qq999999"}"#,
    );
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(parameters(&refused), ["constraints.kz000002.line"]);
    assert_eq!(refused.total, 1);
    assert!(refused.errors[0].candidates().is_empty());
}

#[test]
fn an_id_the_sketch_holds_is_refused_at_the_requests_field_naming_its_holder() {
    // M4.1 section 2: at the field the request named (`circles.ca000000.id`), never at the
    // sketch's untouched member, with the member holding it in the message and elements.
    // A point given the line's ID too, whatever its kind. The gate isn't run, so the bad
    // radius beside them isn't reported until the IDs are mended.
    let before = base();
    let batch = members(
        r#"{"id": "na000000", "x": 20, "y": 0, "construction": false}"#,
        "",
        r#"{"id": "ca000000", "center": "pa000000", "radius": 7, "construction": false}, {"id": "cz000001", "center": "pa000000", "radius": 0, "construction": false}"#,
        "",
        "",
    );
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(
        said(&refused),
        [
            (
                "points.na000000.id".to_owned(),
                "id na000000 is already the ID of line na000000 (lines.na000000)".to_owned()
            ),
            (
                "circles.ca000000.id".to_owned(),
                "id ca000000 is already the ID of circle ca000000 (circles.ca000000)".to_owned()
            ),
        ]
    );
    assert_eq!(refused.total, 2);
    assert_eq!(refused.errors[0].elements(), ["na000000"]);
    assert_eq!(
        refused.errors[0].suggestion(),
        Some("give this point an ID of its own")
    );
}

#[test]
fn an_id_given_twice_in_the_batch_is_refused_at_the_later_members_place() {
    // The later holder is named by its place in its list, as the gate names one (M4.1
    // section 2): the second point at `points.1`, and a constraint given a point's ID at
    // `constraints.0`, though it's the first constraint.
    let before = base();
    let batch = members(
        r#"{"id": "pz000001", "x": 20, "y": 0, "construction": false}, {"id": "pz000001", "x": 30, "y": 0, "construction": false}, {"id": "pz000002", "x": 40, "y": 0, "construction": false}"#,
        "",
        "",
        "",
        r#"{"id": "pz000002", "type": "horizontal", "line": "na000000"}"#,
    );
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(
        said(&refused),
        [
            (
                "points.1.id".to_owned(),
                "id pz000001 is already the ID of point pz000001 (points.pz000001), earlier in this request".to_owned()
            ),
            (
                "constraints.0.id".to_owned(),
                "id pz000002 is already the ID of point pz000002 (points.pz000002), earlier in this request".to_owned()
            ),
        ]
    );
    assert_eq!(refused.total, 2);
}

#[test]
fn a_constraint_may_name_a_member_of_its_own_batch() {
    // FreeCAD needs the geometry added first (a constraint's index must be in range); here
    // one batch holds both, checked as the sketch it makes.
    let before = base();
    let batch = members(
        r#"{"id": "pz000001", "x": 30, "y": 0, "construction": false}"#,
        "",
        r#"{"id": "cz000001", "center": "pz000001", "radius": 5, "construction": false}"#,
        "",
        r#"{"id": "kz000001", "type": "equal", "a": "ca000000", "b": "cz000001"}, {"id": "kz000002", "type": "on", "point": "pz000001", "curve": "na000000"}"#,
    );
    let edited = add(sketch_of(&before), &batch).unwrap();
    assert_eq!(
        edited.added,
        ids(&["pz000001", "cz000001", "kz000001", "kz000002"])
    );
    gated(&before, &edited);
}

#[test]
fn an_edge_tangent_between_curves_that_share_an_end_is_refused_not_converted() {
    // The tangency substitution (FreeCAD 180d722a53, 620188073e) is the gate's rule here: a
    // new line from pb, which aa starts at, edge tangent to aa, is refused at its constraint.
    let before = base();
    let batch = members(
        r#"{"id": "pz000001", "x": 20, "y": 0, "construction": false}"#,
        r#"{"id": "nz000001", "from": "pb000000", "to": "pz000001", "construction": false}"#,
        "",
        "",
        r#"{"id": "kz000001", "type": "tangent", "a": "aa000000", "b": "nz000001", "side": "left"}"#,
    );
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(parameters(&refused), ["constraints.kz000001.b"]);
    assert_eq!(refused.total, 1);
}

// ---- what's kept as given

#[test]
fn construction_is_kept_on_every_member_points_too() {
    // FreeCAD 3db4633f9d stopped forcing new points to construction; D7: as given, member by
    // member (FreeCAD's list form takes one flag for the whole list).
    let before = base();
    let batch = members(
        r#"{"id": "pz000001", "x": 20, "y": 0, "construction": true}, {"id": "pz000002", "x": 20, "y": 10, "construction": false}"#,
        r#"{"id": "nz000001", "from": "pz000001", "to": "pz000002", "construction": true}"#,
        r#"{"id": "cz000001", "center": "pz000002", "radius": 3, "construction": true}"#,
        "",
        "",
    );
    let edited = add(sketch_of(&before), &batch).unwrap();
    let after = gated(&before, &edited);
    let sketch = sketch_of(&after);
    let point = |p: &str| sketch.points.iter().find(|q| q.id == id(p)).unwrap();
    assert!(point("pz000001").construction);
    assert!(!point("pz000002").construction);
    assert!(sketch.lines[1].construction);
    assert!(sketch.circles[1].construction);
}

#[test]
fn side_and_direction_are_as_given_even_against_the_drawing() {
    // D4: never decided from geometry, where FreeCAD main records a tangent's side from the
    // drawing (setOrientation, f432d1c6a7, #29007's testCircleLineTangentOriented) and an end
    // tangency's kind (AutoLockTangencyAndPerpty). The circle about (5, 3) of radius 3 lies
    // on na's left (na runs +x, so its left is +y), and is given `right`. nz runs on from
    // na's end, straight on, which is `same` where the point ends one and starts the other,
    // and is given `opposite`.
    let before = base();
    let batch = members(
        r#"{"id": "pz000001", "x": 5, "y": 3, "construction": false}, {"id": "pz000002", "x": 20, "y": 0, "construction": false}"#,
        r#"{"id": "nz000001", "from": "pb000000", "to": "pz000002", "construction": false}"#,
        r#"{"id": "cz000001", "center": "pz000001", "radius": 3, "construction": false}"#,
        "",
        r#"{"id": "kz000001", "type": "tangent", "a": "na000000", "b": "cz000001", "side": "right"}, {"id": "kz000002", "type": "tangent_at", "a": "na000000", "b": "nz000001", "at": "pb000000", "direction": "opposite"}"#,
    );
    let edited = add(sketch_of(&before), &batch).unwrap();
    let after = gated(&before, &edited);
    let form = |k: &str| {
        &sketch_of(&after)
            .constraints
            .iter()
            .find(|c| c.id == id(k))
            .unwrap()
            .form
    };
    assert!(matches!(
        form("kz000001"),
        Form::Tangent {
            side: TangentSide::Right,
            ..
        }
    ));
    assert!(matches!(
        form("kz000002"),
        Form::TangentAt {
            direction: Direction::Opposite,
            ..
        }
    ));
}

#[test]
fn an_empty_batch_adds_nothing_and_leaves_the_sketch_as_it_was() {
    // FreeCAD's empty lists add nothing (1.1.3: both return an empty tuple). Its command's
    // `changed` is false: the sketch is the one it was given.
    let before = base();
    let edited = add(sketch_of(&before), &Members::default()).unwrap();
    assert_eq!(&edited.sketch, sketch_of(&before));
    assert!(edited.added.is_empty() && edited.modified.is_empty() && edited.removed.is_empty());
}

#[test]
fn a_conflicting_constraint_is_added_for_the_edits_solve_to_refuse() {
    // The gate doesn't solve: a second length on na, 12 beside a 10, is added, as FreeCAD's
    // addConstraint keeps one (1.1.3: status -3). The edit's command solves after the gate
    // and refuses it as data (M3.3); silico-sketch's tests/sketch_edit.rs solves this sketch
    // with the real worker.
    let before = read(
        document(&fields(
            POINTS,
            LINE,
            "",
            "",
            r#"{"id": "ka000001", "type": "length", "line": "na000000", "value": 10, "driving": true}"#,
        ))
        .as_bytes(),
    )
    .unwrap();
    let batch = members(
        "",
        "",
        "",
        "",
        r#"{"id": "kz000001", "type": "length", "line": "na000000", "value": 12, "driving": true}"#,
    );
    let edited = add(sketch_of(&before), &batch).unwrap();
    assert_eq!(edited.added, ids(&["kz000001"]));
    gated(&before, &edited);
}

#[test]
fn many_bad_ids_are_all_counted_and_the_first_hundred_listed() {
    // Every problem counted, as `remove`'s are: 150 points each given an ID the sketch holds
    // (the base's pa000000), the first 100 listed.
    let before = base();
    let points: Vec<String> = (0..150)
        .map(|i| format!(r#"{{"id": "pa000000", "x": {i}, "y": 1, "construction": false}}"#))
        .collect();
    let batch = members(&points.join(", "), "", "", "", "");
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(refused.total, 150);
    assert_eq!(refused.errors.len(), 100);
    assert_eq!(refused.errors[0].parameter(), Some("points.pa000000.id"));
    assert_eq!(refused.errors[1].parameter(), Some("points.1.id"));
    assert_eq!(refused.errors[99].parameter(), Some("points.99.id"));
}
