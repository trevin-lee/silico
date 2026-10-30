//! Hostile inputs for adding to a sketch (`document::sketch_edit::add`), from break-it. The
//! add's contract (docs/ports/sketch-edit.md, §2): the batch checked whole, refused with
//! every problem at the request's own field (`circles.cb000003.radius`, or an ID's
//! `circles.1.id`), nothing added; or the sketch with every member appended, which the gate
//! takes. Expected values are worked by hand from the gate's rules, or, in the property
//! tests, from the whole document's gate (`validate`) on the sketch appended by hand.

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use silico_core::document::sketch_edit::{Members, Refused, add};
use silico_core::document::{
    Document, ElementId, Operation, PROBLEMS_LISTED, Sketch, read, validate, write,
};

use crate::fixture::{ARC, CIRCLE, LINE, POINTS, circle, document, fields, line, point};

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

/// The members these lists hold, read as the document reads a sketch's lists.
fn members(points: &str, lines: &str, circles: &str, arcs: &str, constraints: &str) -> Members {
    let sketch = sketch_of(&doc(points, lines, circles, arcs, constraints)).clone();
    Members {
        points: sketch.points,
        lines: sketch.lines,
        circles: sketch.circles,
        arcs: sketch.arcs,
        constraints: sketch.constraints,
    }
}

/// `sketch` with `members` appended by hand, list by list: what the add claims to make.
fn appended(sketch: &Sketch, members: &Members) -> Sketch {
    let mut sketch = sketch.clone();
    sketch.points.extend(members.points.iter().cloned());
    sketch.lines.extend(members.lines.iter().cloned());
    sketch.circles.extend(members.circles.iter().cloned());
    sketch.arcs.extend(members.arcs.iter().cloned());
    sketch
        .constraints
        .extend(members.constraints.iter().cloned());
    sketch
}

fn with_sketch(document: &Document, sketch: &Sketch) -> Document {
    let mut document = document.clone();
    document.features[0].operation = Operation::Sketch(sketch.clone());
    document
}

fn refused(sketch: &Sketch, members: &Members) -> Refused {
    match add(sketch, members) {
        Ok(edited) => panic!("added: {:?}", edited.added),
        Err(refused) => refused,
    }
}

fn parameters(refused: &Refused) -> Vec<&str> {
    refused
        .errors
        .iter()
        .map(|e| e.parameter().unwrap_or_default())
        .collect()
}

/// The base sketch (pa (0, 0), pb (10, 0), pc (0, 10); line na from pa to pb; circle ca
/// about pa, radius 5; arc aa about pa from pb to pc), na horizontal.
fn base() -> Document {
    doc(
        POINTS,
        LINE,
        CIRCLE,
        ARC,
        r#"{"id": "ka000000", "type": "horizontal", "line": "na000000"}"#,
    )
}

// ---- a refusal caused by the batch, named at a member the batch didn't give

#[test]
fn an_on_added_onto_a_curve_an_existing_tangent_names_names_the_added_on() {
    // The sketch: na from pa (0, 0) to pb (10, 0); circle cd about pd (10, 5), radius 5, on
    // na's left, edge tangent to na (kt). The gate takes it: na and cd share no end. The
    // batch adds one `on`, kz, putting pb (na's end) on cd. The gate's rule "a curve ending
    // on another is tangent where it ends" now refuses, at the later of the pair, the
    // batch's `on`. (Broke, before the fix: at the sketch's own tangent,
    // `constraints.kt000001.b`, with only kt in its elements, which the request never gave.)
    let before = doc(
        &[
            point("pa000000", 0.0, 0.0),
            point("pb000000", 10.0, 0.0),
            point("pd000000", 10.0, 5.0),
        ]
        .join(", "),
        &line("na000000", "pa000000", "pb000000"),
        &circle("cd000000", "pd000000", 5.0),
        "",
        r#"{"id": "kt000001", "type": "tangent", "a": "na000000", "b": "cd000000", "side": "left"}"#,
    );
    validate(&before).unwrap();
    let batch = members(
        "",
        "",
        "",
        "",
        r#"{"id": "kz000001", "type": "on", "point": "pb000000", "curve": "cd000000"}"#,
    );
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(refused.total, 1, "{:?}", refused.errors);
    let error = &refused.errors[0];
    assert!(
        error.elements().iter().any(|e| e == "kz000001"),
        "the refusal doesn't name the member the request added: parameter {:?}, elements {:?}, {}",
        error.parameter(),
        error.elements(),
        error.message()
    );
}

#[test]
fn a_tangent_added_onto_curves_an_existing_on_joins_is_refused_at_the_added_tangent() {
    // The mirror of the above: the `on` is the sketch's, the tangent the batch's. The gate
    // refuses at the tangent, which is the request's: `constraints.kz000001.b`.
    let before = doc(
        &[
            point("pa000000", 0.0, 0.0),
            point("pb000000", 10.0, 0.0),
            point("pd000000", 10.0, 5.0),
        ]
        .join(", "),
        &line("na000000", "pa000000", "pb000000"),
        &circle("cd000000", "pd000000", 5.0),
        "",
        r#"{"id": "kt000001", "type": "on", "point": "pb000000", "curve": "cd000000"}"#,
    );
    validate(&before).unwrap();
    let batch = members(
        "",
        "",
        "",
        "",
        r#"{"id": "kz000001", "type": "tangent", "a": "na000000", "b": "cd000000", "side": "left"}"#,
    );
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(parameters(&refused), ["constraints.kz000001.b"]);
    assert_eq!(refused.errors[0].elements(), ["kz000001"]);
}

#[test]
fn a_tangent_on_and_an_edge_tangent_of_one_pair_are_refused_at_the_added_one_either_way() {
    // #258's pair through an edit: the sketch above's na and cd, one of the pair the
    // sketch's (kt000001) and the other the batch's, whose ID (ka000001) sorts before it, so
    // a refusal at the later in ID order would name the sketch's own constraint. The batch
    // appends, and the gate refuses the member the request gave.
    let pair = [
        (
            r#"{"id": "kt000001", "type": "tangent", "a": "na000000", "b": "cd000000", "side": "left"}"#,
            r#"{"id": "ka000001", "type": "tangent_on", "a": "na000000", "at": "pb000000", "b": "cd000000", "direction": "same"}"#,
            "constraints.ka000001.at",
        ),
        (
            r#"{"id": "kt000001", "type": "tangent_on", "a": "na000000", "at": "pb000000", "b": "cd000000", "direction": "same"}"#,
            r#"{"id": "ka000001", "type": "tangent", "a": "na000000", "b": "cd000000", "side": "left"}"#,
            "constraints.ka000001.b",
        ),
    ];
    for (held, added, at) in pair {
        let before = doc(
            &[
                point("pa000000", 0.0, 0.0),
                point("pb000000", 10.0, 0.0),
                point("pd000000", 10.0, 5.0),
            ]
            .join(", "),
            &line("na000000", "pa000000", "pb000000"),
            &circle("cd000000", "pd000000", 5.0),
            "",
            held,
        );
        validate(&before).unwrap();
        let refused = refused(sketch_of(&before), &members("", "", "", "", added));
        assert_eq!(parameters(&refused), [at]);
        assert_eq!(refused.errors[0].elements(), ["ka000001"]);
        assert_eq!(
            refused.errors[0].suggestion(),
            Some("remove the tangent: the tangent_on already makes them tangent at point pb000000")
        );
    }
}

// ---- the solver's limits

/// A sketch of one line, na from pa (0, 0) to pb (10, 0), with `n` driving lengths of 10 on
/// it: 4 unknowns, so n + 4 + 6 values.
fn lengths_on_one_line(n: usize) -> Document {
    let constraints: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"id": "k{i:07}", "type": "length", "line": "na000000", "value": 10, "driving": true}}"#
            )
        })
        .collect();
    doc(
        &[point("pa000000", 0.0, 0.0), point("pb000000", 10.0, 0.0)].join(", "),
        &line("na000000", "pa000000", "pb000000"),
        "",
        "",
        &constraints.join(", "),
    )
}

#[test]
fn a_driving_length_past_the_values_limit_is_refused_at_the_constraints_the_request_adds() {
    // 9,990 lengths: 4 unknowns + 9,990 dimensions + 6 fixed = 10,000 values, exactly the
    // solver's limit (MAX_SKETCH_VALUES), and 9,990 constraints, under its 10,000. The gate
    // takes it. One more driving length makes 10,001 values: refused, rightly, at the
    // request's `constraints`, the list that added the value. (Broke, before the fix: at
    // `points`, where the gate files every values count, a list the request left empty.)
    let before = lengths_on_one_line(9_990);
    validate(&before).unwrap();
    let batch = members(
        "",
        "",
        "",
        "",
        r#"{"id": "kz000001", "type": "length", "line": "na000000", "value": 10, "driving": true}"#,
    );
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(refused.total, 1, "{:?}", refused.errors);
    assert!(
        refused.errors[0].message().contains("10001 values"),
        "{}",
        refused.errors[0].message()
    );
    let parameter = refused.errors[0].parameter().unwrap_or_default();
    assert!(
        parameter.starts_with("constraints"),
        "refused at {parameter:?}, a list the request didn't add to"
    );
}

#[test]
fn a_point_past_the_unknowns_limit_is_refused_at_points() {
    // 511 points are 1,022 unknowns; two more make 1,026, past the 1,024 the solver takes
    // (MAX_SKETCH_UNKNOWNS). One more alone (1,024) is taken.
    let pts: Vec<String> = (0..511)
        .map(|i| point(&format!("p{i:07}"), f64::from(i), 0.0))
        .collect();
    let before = doc(&pts.join(", "), "", "", "", "");
    validate(&before).unwrap();
    let one = members(&point("pz000001", 0.0, 1.0), "", "", "", "");
    let edited = add(sketch_of(&before), &one).unwrap();
    assert_eq!(edited.added, ids(&["pz000001"]));
    let two = members(
        &[point("pz000001", 0.0, 1.0), point("pz000002", 0.0, 2.0)].join(", "),
        "",
        "",
        "",
        "",
    );
    let refused = refused(sketch_of(&before), &two);
    assert_eq!(parameters(&refused), ["points"]);
    assert!(
        refused.errors[0].message().contains("1026 unknowns"),
        "{}",
        refused.errors[0].message()
    );
}

// ---- IDs

#[test]
fn an_id_the_sketch_holds_given_twice_in_the_batch_is_refused_at_both_places() {
    // pa000000 is the sketch's point. The batch gives it to a point and again to a line:
    // the first at the request's ID path, `points.pa000000.id`; the second, a later holder
    // in the batch, at its place, `lines.0.id`; each naming the sketch's point as holder.
    let before = base();
    let batch = members(
        &point("pa000000", 20.0, 0.0),
        &line("pa000000", "pb000000", "pc000000"),
        "",
        "",
        "",
    );
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(parameters(&refused), ["points.pa000000.id", "lines.0.id"]);
    for error in &refused.errors {
        assert_eq!(
            error.message(),
            "id pa000000 is already the ID of point pa000000 (points.pa000000)"
        );
        assert_eq!(error.elements(), ["pa000000"]);
    }
    assert_eq!(refused.total, 2);
}

#[test]
fn an_id_a_constraint_holds_given_to_a_point_names_the_constraint() {
    let before = base();
    let batch = members(&point("ka000000", 20.0, 0.0), "", "", "", "");
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(parameters(&refused), ["points.ka000000.id"]);
    assert_eq!(
        refused.errors[0].message(),
        "id ka000000 is already the ID of constraint ka000000 (constraints.ka000000)"
    );
    assert_eq!(
        refused.errors[0].suggestion(),
        Some("give this point an ID of its own")
    );
}

#[test]
fn one_id_given_three_times_in_the_batch_is_refused_at_the_second_and_third_places() {
    // The first holder is the batch's own pz000001 (`points.pz000001`); the later two are
    // named by their places in their lists, `points.1` and `circles.0`.
    let before = base();
    let batch = members(
        &[point("pz000001", 20.0, 0.0), point("pz000001", 30.0, 0.0)].join(", "),
        "",
        &circle("pz000001", "pa000000", 2.0),
        "",
        "",
    );
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(parameters(&refused), ["points.1.id", "circles.0.id"]);
    for error in &refused.errors {
        assert_eq!(
            error.message(),
            "id pz000001 is already the ID of point pz000001 (points.pz000001), earlier in this request"
        );
    }
}

// ---- values at and past the gate's bounds, through the add

#[test]
fn a_point_a_kilometre_out_is_added_and_one_ulp_further_is_refused_at_its_coordinate() {
    let before = base();
    let at = members(
        &point("pz000001", 1_000_000.0, -1_000_000.0),
        "",
        "",
        "",
        "",
    );
    assert_eq!(
        add(sketch_of(&before), &at).unwrap().added,
        ids(&["pz000001"])
    );
    let past = members(
        &crate::fixture::point_debug("pz000001", 1_000_000f64.next_up(), 0.0),
        "",
        "",
        "",
        "",
    );
    let refused = refused(sketch_of(&before), &past);
    assert_eq!(parameters(&refused), ["points.pz000001.x"]);
    assert_eq!(refused.errors[0].elements(), ["pz000001"]);
}

#[test]
fn curves_through_points_at_the_largest_finite_coordinates_are_refused_without_overflowing() {
    // Points at ±f64::MAX: each coordinate past a kilometre, and a line, a circle, and an
    // arc drawn through them, whose lengths overflow to infinity. Refused at each field of
    // the request, no panic, nothing reported of no size (infinity isn't small), and no
    // NaN. (The arc's radius is reported as "inf mm": 2√2 · f64::MAX has no f64.)
    let before = base();
    let batch = members(
        &[
            crate::fixture::point_debug("pz000001", f64::MAX, -f64::MAX),
            crate::fixture::point_debug("pz000002", -f64::MAX, f64::MAX),
        ]
        .join(", "),
        &line("nz000001", "pz000001", "pz000002"),
        &circle("cz000001", "pz000001", 1e300),
        r#"{"id": "az000001", "center": "pz000001", "start": "pz000002", "end": "pa000000", "construction": false}"#,
        "",
    );
    let refused = refused(sketch_of(&before), &batch);
    let got = parameters(&refused);
    for want in [
        "points.pz000001.x",
        "points.pz000001.y",
        "points.pz000002.x",
        "points.pz000002.y",
        "circles.cz000001.radius",
        "arcs.az000001.start",
    ] {
        assert!(got.contains(&want), "{want} not in {got:?}");
    }
    for error in &refused.errors {
        assert!(
            !error.message().contains("NaN") && !error.message().contains("1e-7 mm"),
            "{}",
            error.message()
        );
        let p = error.parameter().unwrap_or_default();
        assert!(p.contains("z00000"), "{p} isn't a member the request gave");
    }
}

#[test]
fn a_new_line_between_two_new_points_drawn_at_one_place_is_refused_at_its_end() {
    // Two distinct points at (20, 0): a line of no size by value, not by ID.
    let before = base();
    let batch = members(
        &[point("pz000001", 20.0, 0.0), point("pz000002", 20.0, 0.0)].join(", "),
        &line("nz000001", "pz000001", "pz000002"),
        "",
        "",
        "",
    );
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(parameters(&refused), ["lines.nz000001.to"]);
    let elements: BTreeSet<&str> = refused.errors[0]
        .elements()
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(
        elements,
        BTreeSet::from(["nz000001", "pz000001", "pz000002"])
    );
}

#[test]
fn a_new_point_onto_an_existing_points_place_ending_a_line_to_it_is_refused() {
    // pz at pb's place (10, 0), a new line from pb to pz: no size by value.
    let before = base();
    let batch = members(
        &point("pz000001", 10.0, 0.0),
        &line("nz000001", "pb000000", "pz000001"),
        "",
        "",
        "",
    );
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(parameters(&refused), ["lines.nz000001.to"]);
}

#[test]
fn a_constraint_naming_only_the_origin_and_an_axis_is_refused_at_its_path() {
    // FreeCAD's negative indices (the axes, 7d8b1f30c3's range check): a constraint naming
    // only the sketch's fixed references constrains nothing in it (3d7871fca5).
    let before = base();
    let batch = members(
        "",
        "",
        "",
        "",
        r#"{"id": "kz000001", "type": "on", "point": "origin", "curve": "x_axis"}"#,
    );
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(parameters(&refused), ["constraints.kz000001"]);
    assert_eq!(refused.errors[0].elements(), ["kz000001"]);
}

#[test]
fn a_constraint_naming_a_point_where_a_line_goes_is_refused_naming_what_it_is() {
    let before = base();
    let batch = members(
        "",
        "",
        "",
        "",
        r#"{"id": "kz000001", "type": "horizontal", "line": "pa000000"}"#,
    );
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(parameters(&refused), ["constraints.kz000001.line"]);
    assert!(
        refused.errors[0].message().contains("point"),
        "{}",
        refused.errors[0].message()
    );
}

#[test]
fn a_bad_curve_and_a_constraint_on_it_are_both_refused_at_their_own_paths() {
    // A circle of radius 0 and a radius on it: the circle's own fault, and none for the
    // constraint, which names a circle the batch gives.
    let before = base();
    let batch = members(
        "",
        "",
        r#"{"id": "cz000001", "center": "pb000000", "radius": 0, "construction": false}"#,
        "",
        r#"{"id": "kz000001", "type": "radius", "curve": "cz000001", "value": 3, "driving": true}"#,
    );
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(parameters(&refused), ["circles.cz000001.radius"]);
}

#[test]
fn more_than_a_hundred_gate_problems_are_all_counted_and_a_hundred_listed() {
    // 150 circles of radius 0 about pa: past the gate's listing, counted whole, listed in
    // the batch's order. 150 circles are 150 unknowns, under the solver's 1,024.
    let before = base();
    let circles: Vec<String> = (0..150)
        .map(|i| {
            format!(
                r#"{{"id": "c{i:07}", "center": "pa000000", "radius": 0, "construction": false}}"#
            )
        })
        .collect();
    let batch = members("", "", &circles.join(", "), "", "");
    let refused = refused(sketch_of(&before), &batch);
    assert_eq!(refused.total, 150);
    assert_eq!(refused.errors.len(), PROBLEMS_LISTED);
    assert_eq!(
        refused.errors[0].parameter(),
        Some("circles.c0000000.radius")
    );
    assert_eq!(
        refused.errors[99].parameter(),
        Some("circles.c0000099.radius")
    );
}

// ---- no corruption, and the same answer each time

#[test]
fn a_refused_batch_leaves_the_sketch_for_the_next_add_and_the_answers_repeat() {
    let before = base();
    let sketch = sketch_of(&before).clone();
    let bad = members(
        &point("pz000001", 20.0, 0.0),
        &line("nz000001", "pz000001", "pz000001"),
        "",
        "",
        "",
    );
    let first = refused(&sketch, &bad);
    assert_eq!(first, refused(&sketch, &bad));
    assert_eq!(&sketch, sketch_of(&before));
    let good = members(
        &point("pz000001", 20.0, 0.0),
        &line("nz000001", "pb000000", "pz000001"),
        "",
        "",
        "",
    );
    let edited = add(&sketch, &good).unwrap();
    assert_eq!(edited, add(&sketch, &good).unwrap());
    validate(&with_sketch(&before, &edited.sketch)).unwrap();
    // And added to again: an add's result is a sketch the next add takes.
    let more = members(
        "",
        "",
        "",
        "",
        r#"{"id": "kz000001", "type": "horizontal", "line": "nz000001"}"#,
    );
    let again = add(&edited.sketch, &more).unwrap();
    assert_eq!(again.added, ids(&["kz000001"]));
    validate(&with_sketch(&before, &again.sketch)).unwrap();
}

#[test]
fn a_batch_added_in_two_halves_is_the_batch_added_whole() {
    // Geometry first, then constraints on it (FreeCAD's two calls): the same sketch as one
    // batch, and `added` the two halves' in turn.
    let before = base();
    let geometry = members(
        &[point("pz000001", 20.0, 0.0), point("pz000002", 20.0, 10.0)].join(", "),
        &line("nz000001", "pz000001", "pz000002"),
        &circle("cz000001", "pz000002", 3.0),
        "",
        "",
    );
    let constraints = members(
        "",
        "",
        "",
        "",
        r#"{"id": "kz000001", "type": "vertical", "line": "nz000001"}, {"id": "kz000002", "type": "equal", "a": "ca000000", "b": "cz000001"}"#,
    );
    let whole = Members {
        constraints: constraints.constraints.clone(),
        ..geometry.clone()
    };
    let one = add(sketch_of(&before), &whole).unwrap();
    let first = add(sketch_of(&before), &geometry).unwrap();
    let second = add(&first.sketch, &constraints).unwrap();
    assert_eq!(one.sketch, second.sketch);
    let both: Vec<ElementId> = first.added.iter().chain(&second.added).cloned().collect();
    assert_eq!(one.added, both);
}

// ---- property tests

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x0add_0ba7),
        ..ProptestConfig::default()
    }
}

/// The bases a batch is added to: the fixture's (a horizontal on na), and one with an edge
/// tangent and an `on` the batch can collide with.
fn bases() -> [Document; 2] {
    [
        base(),
        doc(
            &[
                point("pa000000", 0.0, 0.0),
                point("pb000000", 10.0, 0.0),
                point("pc000000", 0.0, 10.0),
                point("pd000000", 10.0, 5.0),
            ]
            .join(", "),
            &line("na000000", "pa000000", "pb000000"),
            &circle("ca000000", "pd000000", 5.0),
            r#"{"id": "aa000000", "center": "pa000000", "start": "pb000000", "end": "pc000000", "construction": false}"#,
            r#"{"id": "ka000000", "type": "tangent", "a": "na000000", "b": "ca000000", "side": "left"}, {"id": "kb000000", "type": "on", "point": "pc000000", "curve": "ca000000"}"#,
        ),
    ]
}

/// An ID for the `n`th member of a batch: fresh (its kind's letter, `z`, then `n`), or,
/// for a pick under 4, one the sketch holds or an earlier member's.
fn member_id(kind: char, n: usize, pick: u8) -> String {
    match pick {
        0 => "pa000000".to_owned(),
        1 => "na000000".to_owned(),
        2 => "ka000000".to_owned(),
        3 if n > 0 => format!("{kind}z{:06}", n - 1),
        _ => format!("{kind}z{n:06}"),
    }
}

/// Coordinates: mostly on a grid, an eighth of the time at an edge (1e-8, a kilometre, and
/// half a millimetre past it).
fn coordinate(pick: usize) -> f64 {
    const GRID: [f64; 6] = [0.0, 5.0, 10.0, 20.0, -7.5, 15.0];
    const EDGE: [f64; 4] = [1e-8, 1_000_000.0, 1_000_000.5, -1e-8];
    if pick % 8 == 7 {
        EDGE[(pick / 8) % EDGE.len()]
    } else {
        GRID[pick % GRID.len()]
    }
}

/// Radii: mostly sizable, an eighth of the time 0, at the tolerance, just past it, or
/// past a kilometre.
fn radius(pick: usize) -> f64 {
    const EDGE: [f64; 4] = [0.0, 1e-7, 1.1e-7, 1_000_000.5];
    if pick % 8 == 7 {
        EDGE[(pick / 8) % EDGE.len()]
    } else {
        [2.0, 3.0, 5.0, 7.5][pick % 4]
    }
}

/// What a batch is made of, as picks: each reference an index into the members the sketch
/// and the batch hold, or, for a pick of 240 or more, one no member holds.
#[derive(Debug, Clone)]
struct Batch {
    points: Vec<(u8, usize, usize)>,
    lines: Vec<(u8, usize, usize)>,
    circles: Vec<(u8, usize, usize)>,
    arcs: Vec<(u8, usize, usize, usize)>,
    constraints: Vec<(u8, u8, usize, usize, usize, usize)>,
}

fn batch() -> impl Strategy<Value = Batch> {
    // ID picks: fresh 39 times in 40.
    let idp = prop_oneof![39 => 4u8..8, 1 => 0u8..4];
    let r = 0usize..256;
    (
        prop::collection::vec((idp.clone(), 0usize..64, 0usize..64), 0..5),
        prop::collection::vec((idp.clone(), r.clone(), r.clone()), 0..4),
        prop::collection::vec((idp.clone(), r.clone(), 0usize..64), 0..3),
        prop::collection::vec((idp.clone(), r.clone(), r.clone(), r.clone()), 0..3),
        prop::collection::vec((idp, 0u8..9, r.clone(), r.clone(), r, 0usize..8), 0..6),
    )
        .prop_map(|(points, lines, circles, arcs, constraints)| Batch {
            points,
            lines,
            circles,
            arcs,
            constraints,
        })
}

/// The member a pick names in `pool`, or `missing` for a pick of 240 or more.
fn pick<'a>(pool: &'a [String], missing: &'a str, pick: usize) -> &'a str {
    if pick >= 240 || pool.is_empty() {
        missing
    } else {
        pool[pick % pool.len()].as_str()
    }
}

impl Batch {
    fn points(&self) -> Vec<String> {
        let mut pool: Vec<String> = ["pa000000", "pb000000", "pc000000"]
            .map(str::to_owned)
            .to_vec();
        pool.extend((0..self.points.len()).map(|n| format!("pz{n:06}")));
        pool
    }

    fn curves(&self) -> Vec<String> {
        let mut pool: Vec<String> = ["na000000", "ca000000", "aa000000", "x_axis"]
            .map(str::to_owned)
            .to_vec();
        pool.extend((0..self.lines.len()).map(|n| format!("nz{n:06}")));
        pool.extend((0..self.circles.len()).map(|n| format!("cz{n:06}")));
        pool.extend((0..self.arcs.len()).map(|n| format!("az{n:06}")));
        pool
    }

    fn members(&self) -> Members {
        let points = self.points();
        let curves = self.curves();
        let p = |i: usize| pick(&points, "pq999999", i);
        // A second pick, moved off the first's member one time in two (a curve from a
        // point to itself is refused whatever else it is).
        let other = |first: usize, i: usize| {
            if i.is_multiple_of(2)
                && first < 240
                && i < 240
                && first % points.len() == i % points.len()
            {
                i + 1
            } else {
                i
            }
        };
        let c = |i: usize| pick(&curves, "nq999999", i);
        let pts: Vec<String> = self
            .points
            .iter()
            .enumerate()
            .map(|(n, &(idp, x, y))| {
                crate::fixture::point_debug(&member_id('p', n, idp), coordinate(x), coordinate(y))
            })
            .collect();
        let lines: Vec<String> = self
            .lines
            .iter()
            .enumerate()
            .map(|(n, &(idp, a, b))| line(&member_id('n', n, idp), p(a), p(other(a, b))))
            .collect();
        let circles: Vec<String> = self
            .circles
            .iter()
            .enumerate()
            .map(|(n, &(idp, a, r))| {
                format!(
                    r#"{{"id": "{}", "center": "{}", "radius": {:?}, "construction": false}}"#,
                    member_id('c', n, idp),
                    p(a),
                    radius(r)
                )
            })
            .collect();
        let arcs: Vec<String> = self
            .arcs
            .iter()
            .enumerate()
            .map(|(n, &(idp, a, b, e))| {
                crate::fixture::arc(
                    &member_id('a', n, idp),
                    p(a),
                    p(other(a, b)),
                    p(other(b, other(a, e))),
                )
            })
            .collect();
        let constraints: Vec<String> = self
            .constraints
            .iter()
            .enumerate()
            .map(|(n, &(idp, kind, x, y, z, v))| {
                let k = member_id('k', n, idp);
                let value = [3.0, 10.0, 0.0, 2_000_000.0, 2.75, 81.370787, 0.5, 0.001][v];
                let side = ["left", "right", "outside", "inside"][v % 4];
                let direction = ["same", "opposite"][v % 2];
                let (cx, cy, pz) = (c(x), c(y), p(z));
                match kind {
                    0 => format!(r#"{{"id": "{k}", "type": "horizontal", "line": "{cx}"}}"#),
                    1 => format!(r#"{{"id": "{k}", "type": "on", "point": "{}", "curve": "{cy}"}}"#, p(x)),
                    2 => format!(r#"{{"id": "{k}", "type": "tangent", "a": "{cx}", "b": "{cy}", "side": "{side}"}}"#),
                    3 => format!(r#"{{"id": "{k}", "type": "tangent_at", "a": "{cx}", "b": "{cy}", "at": "{pz}", "direction": "{direction}"}}"#),
                    4 => format!(r#"{{"id": "{k}", "type": "tangent_on", "a": "{cx}", "at": "{pz}", "b": "{cy}", "direction": "{direction}"}}"#),
                    5 => format!(r#"{{"id": "{k}", "type": "length", "line": "{cx}", "value": {value:?}, "driving": true}}"#),
                    6 => format!(r#"{{"id": "{k}", "type": "equal", "a": "{cx}", "b": "{cy}"}}"#),
                    7 => format!(r#"{{"id": "{k}", "type": "radius", "curve": "{cx}", "value": {value:?}, "driving": true}}"#),
                    _ => format!(r#"{{"id": "{k}", "type": "symmetric", "a": "{}", "b": "{pz}", "about": "{cy}"}}"#, p(x)),
                }
            })
            .collect();
        members(
            &pts.join(", "),
            &lines.join(", "),
            &circles.join(", "),
            &arcs.join(", "),
            &constraints.join(", "),
        )
    }
}

/// Each member of `members` with its kind's list, in the format's order.
fn by_kind(members: &Members) -> Vec<(&'static str, &ElementId)> {
    let mut all = Vec::new();
    all.extend(members.points.iter().map(|m| ("points", &m.id)));
    all.extend(members.lines.iter().map(|m| ("lines", &m.id)));
    all.extend(members.circles.iter().map(|m| ("circles", &m.id)));
    all.extend(members.arcs.iter().map(|m| ("arcs", &m.id)));
    all.extend(members.constraints.iter().map(|m| ("constraints", &m.id)));
    all
}

proptest! {
    #![proptest_config(config(4000))]

    /// Any batch on either base: Ok exactly when the whole document's gate takes the sketch
    /// appended by hand, and then that sketch, `added` by kind as given; refused for its
    /// IDs, exactly one `.id` error per member whose ID the sketch or an earlier member
    /// holds (counted here independently); refused otherwise with the document gate's
    /// errors, at the same paths; the same answer twice.
    #[test]
    fn any_batch_is_the_gates_verdict_on_the_sketch_it_makes(
        which in 0usize..2,
        batch in batch(),
    ) {
        let before = &bases()[which];
        let sketch = sketch_of(before);
        let members = batch.members();
        let made = appended(sketch, &members);
        let gate = validate(&with_sketch(before, &made));
        let result = add(sketch, &members);
        prop_assert_eq!(&result, &add(sketch, &members));

        // Independently: each member's ID held by the sketch or an earlier member.
        let held: BTreeSet<&ElementId> = sketch.elements().map(|(id, _)| id).collect();
        let mut seen: BTreeMap<&ElementId, usize> = BTreeMap::new();
        let mut collisions = 0u64;
        for (_, member) in by_kind(&members) {
            let earlier = seen.entry(member).or_insert(0);
            if held.contains(member) || *earlier > 0 {
                collisions += 1;
            }
            *earlier += 1;
        }

        match result {
            Ok(edited) => {
                prop_assert!(gate.is_ok(), "added, but the gate refuses: {:?}", gate.err().map(|e| e.errors().to_vec()));
                prop_assert_eq!(collisions, 0);
                prop_assert_eq!(&edited.sketch, &made);
                let want: Vec<ElementId> = by_kind(&members).into_iter().map(|(_, id)| id.clone()).collect();
                prop_assert_eq!(&edited.added, &want);
                prop_assert!(edited.modified.is_empty() && edited.removed.is_empty());
                // And the document as a save writes it reads back as the same sketch.
                let text = write(&with_sketch(before, &edited.sketch)).unwrap();
                let back = read(text.as_bytes()).unwrap();
                validate(&back).unwrap();
            }
            Err(refused) => {
                prop_assert!(gate.is_err(), "refused, but the gate takes it: {:?}", refused.errors);
                prop_assert!(refused.total >= 1);
                prop_assert_eq!(refused.errors.len() as u64, refused.total.min(PROBLEMS_LISTED as u64));
                if collisions > 0 {
                    prop_assert_eq!(refused.total, collisions);
                    for error in &refused.errors {
                        let p = error.parameter().unwrap_or_default();
                        prop_assert!(p.ends_with(".id"), "{}", p);
                    }
                } else {
                    let theirs: Vec<String> = gate.err().unwrap().errors().iter().map(|e| e.parameter().unwrap_or_default().to_owned()).collect();
                    let ours: Vec<String> = refused.errors.iter().map(|e| e.parameter().unwrap_or_default().to_owned()).collect();
                    prop_assert_eq!(ours, theirs);
                }
            }
        }
    }

    /// On the fixture's base (no edge tangent or `on` for a batch to collide with), every
    /// refusal is at a member the request gave: its path's member an ID the batch gives
    /// (or, for an ID given again, a place), and that member among the error's elements.
    #[test]
    fn every_refusal_on_the_plain_base_is_at_a_member_the_request_gave(batch in batch()) {
        let before = base();
        let sketch = sketch_of(&before);
        let members = batch.members();
        let given: BTreeSet<&str> = by_kind(&members).into_iter().map(|(_, id)| id.as_str()).collect();
        if let Err(refused) = add(sketch, &members) {
            for error in &refused.errors {
                let p = error.parameter().unwrap_or_default();
                let mut parts = p.split('.');
                let list = parts.next().unwrap_or_default();
                let Some(member) = parts.next() else {
                    // The solver's size limits, at a list.
                    prop_assert!(["points", "lines", "constraints"].contains(&list), "{}", p);
                    continue;
                };
                let by_place = member.parse::<usize>().is_ok() && p.ends_with(".id");
                prop_assert!(by_place || given.contains(member), "{} names no member the request gave: {}", p, error.message());
                if !by_place {
                    prop_assert!(error.elements().iter().any(|e| e == member), "{} {:?}", p, error.elements());
                }
            }
        }
    }
}
