//! M3.2's gate, rule by rule: each test fails on a wrong version of a rule that the other
//! sketch tests pass (found by the review's mutations), and on each rule added by the review.
//! Counts and limits are worked out by hand from the solver's limits.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use silico_core::document::{Operation, Plane, read, validate, write};
use silico_core::{Error, ErrorCode};

const FEATURE: &str = "k7f2qx3m";

fn document(fields: &str) -> String {
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{{\"id\": \"{FEATURE}\", \"type\": \"sketch\", \"v\": 1, \"name\": \"S\", {fields}}}]}}"
    )
}
fn fields(points: &str, lines: &str, circles: &str, arcs: &str, constraints: &str) -> String {
    format!(
        "\"plane\": \"xy\", \"points\": [{points}], \"lines\": [{lines}], \"circles\": [{circles}], \"arcs\": [{arcs}], \"constraints\": [{constraints}]"
    )
}
const POINTS: &str = r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 10, "y": 0, "construction": false}, {"id": "pc000000", "x": 0, "y": 10, "construction": false}"#;
const LINE: &str =
    r#"{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}"#;
const LINE2: &str =
    r#"{"id": "nb000000", "from": "pb000000", "to": "pc000000", "construction": false}"#;
const CIRCLE: &str =
    r#"{"id": "ca000000", "center": "pa000000", "radius": 5, "construction": false}"#;
const ARC: &str = r#"{"id": "aa000000", "center": "pa000000", "start": "pb000000", "end": "pc000000", "construction": false}"#;

fn constrained(c: &str) -> String {
    document(&fields(POINTS, &format!("{LINE}, {LINE2}"), CIRCLE, ARC, c))
}
fn problems(text: &str) -> Vec<Error> {
    match validate(&read(text.as_bytes()).unwrap()) {
        Ok(()) => Vec::new(),
        Err(invalid) => invalid.errors().to_vec(),
    }
}
#[track_caller]
fn only(text: &str, parameter: &str, message: &str) {
    let all = problems(text);
    assert_eq!(all.len(), 1, "{all:#?}");
    assert_eq!(all[0].code(), ErrorCode::InvalidInput);
    assert_eq!(all[0].parameter(), Some(parameter), "{all:#?}");
    assert!(all[0].message().contains(message), "{all:#?}");
}
#[track_caller]
fn none(text: &str) {
    let all = problems(text);
    assert!(all.is_empty(), "{all:#?}");
}

#[test]
fn each_plane_reads_as_its_own_value() {
    for (text, plane) in [("xy", Plane::Xy), ("xz", Plane::Xz), ("yz", Plane::Yz)] {
        let doc = document(&fields("", "", "", "", "")).replace("\"xy\"", &format!("\"{text}\""));
        let d = read(doc.as_bytes()).unwrap();
        match &d.features[0].operation {
            Operation::Sketch(s) => assert_eq!(s.plane, plane, "{text}"),
            other => panic!("{other:?}"),
        }
        assert!(
            write(&d)
                .unwrap()
                .contains(&format!("\"plane\": \"{text}\""))
        );
    }
}

#[test]
fn an_arcs_radius_past_a_kilometre_along_y_is_refused() {
    let text = document(&fields(
        r#"{"id": "pa000000", "x": 0, "y": -1000000, "construction": false}, {"id": "pb000000", "x": 0, "y": 1000000, "construction": false}, {"id": "pc000000", "x": 1000000, "y": -1000000, "construction": false}"#,
        "",
        "",
        ARC,
        "",
    ));
    only(&text, "arcs.0.start", "more than the 1000000 mm");
}

#[test]
fn an_arcs_start_or_end_drawn_at_its_center_is_refused() {
    let pd = r#"{"id": "pd000000", "x": 0, "y": 0, "construction": false}"#;
    for (key, arc) in [
        (
            "start",
            r#"{"id": "aa000000", "center": "pa000000", "start": "pd000000", "end": "pc000000", "construction": false}"#,
        ),
        (
            "end",
            r#"{"id": "aa000000", "center": "pa000000", "start": "pb000000", "end": "pd000000", "construction": false}"#,
        ),
    ] {
        let text = document(&fields(&format!("{POINTS}, {pd}"), "", "", arc, ""));
        only(
            &text,
            &format!("arcs.0.{key}"),
            "is too close to its center at (0, 0) for its radius to be anything but 0",
        );
    }
}

#[test]
fn tangent_at_an_end_of_a_alone_is_refused_naming_b() {
    // pa is na's start; aa runs from pb to pc (pa is its center, not an end).
    only(
        &constrained(
            r#"{"id": "k0000000", "type": "tangent_at", "a": "na000000", "b": "aa000000", "at": "pa000000", "direction": "same"}"#,
        ),
        "constraints.0.at",
        "isn't an end of its b, aa000000",
    );
}

#[test]
fn two_rounds_tangent_outside_or_inside_are_admitted() {
    for side in ["outside", "inside"] {
        none(&constrained(&format!(
            r#"{{"id": "k0000000", "type": "tangent", "a": "aa000000", "b": "ca000000", "side": "{side}"}}"#
        )));
    }
}

#[test]
fn two_lines_equal_are_admitted() {
    none(&constrained(
        r#"{"id": "k0000000", "type": "equal", "a": "na000000", "b": "nb000000"}"#,
    ));
}

#[test]
fn symmetric_about_its_b_is_refused_and_about_a_line_through_one_end_admitted() {
    only(
        &constrained(
            r#"{"id": "k0000000", "type": "symmetric", "a": "pb000000", "b": "pc000000", "about": "pc000000"}"#,
        ),
        "constraints.0.about",
        "is one of the points it mirrors",
    );
    // FreeCAD refuses only both ends of the line (CommandConstraints.cpp, GeoId1 == GeoId2 == GeoId3).
    none(&constrained(
        r#"{"id": "k0000000", "type": "symmetric", "a": "pa000000", "b": "pc000000", "about": "na000000"}"#,
    ));
}

#[test]
fn an_arcs_own_end_on_the_arc_is_refused() {
    for p in ["pb000000", "pc000000"] {
        only(
            &constrained(&format!(
                r#"{{"id": "k0000000", "type": "on", "point": "{p}", "curve": "aa000000"}}"#
            )),
            "constraints.0.point",
            "is part of aa000000 itself",
        );
    }
}

// E9: each arc is three unknowns.
#[test]
fn arcs_count_three_unknowns_each() {
    // 3 points (6) and 681 arcs (2043): 2049, past 2048.
    let arcs: Vec<String> = (0..681).map(|i| format!(r#"{{"id": "a{i:07}", "center": "pa000000", "start": "pb000000", "end": "pc000000", "construction": false}}"#)).collect();
    only(
        &document(&fields(POINTS, "", "", &arcs.join(","), "")),
        "points",
        "the sketch has 2049 unknowns",
    );
}

// E9: each arc's rules count as a constraint.
#[test]
fn arcs_count_as_constraints() {
    // 680 arcs and 9,321 constraints: 10,001.
    let arcs: Vec<String> = (0..680).map(|i| format!(r#"{{"id": "a{i:07}", "center": "pa000000", "start": "pb000000", "end": "pc000000", "construction": false}}"#)).collect();
    let cs: Vec<String> = (0..9321)
        .map(|i| format!(r#"{{"id": "k{i:07}", "type": "horizontal", "line": "na000000"}}"#))
        .collect();
    only(
        &document(&fields(POINTS, LINE, "", &arcs.join(","), &cs.join(","))),
        "constraints",
        "the sketch has 10001 constraints",
    );
}

// E9: each arc is four rows; each midpoint two.
#[test]
fn arcs_and_midpoints_count_their_rows_in_the_jacobian() {
    // 680 arcs (2046 unknowns), 8,000 symmetric (16,000 rows) and the arcs' 2,720:
    // 18,720 × 2046 = 38,301,120, past 2^25; without the arcs' rows, 32,736,000.
    let arcs: Vec<String> = (0..680).map(|i| format!(r#"{{"id": "a{i:07}", "center": "pa000000", "start": "pb000000", "end": "pc000000", "construction": false}}"#)).collect();
    let cs: Vec<String> = (0..8000).map(|i| format!(r#"{{"id": "k{i:07}", "type": "symmetric", "a": "pb000000", "b": "pc000000", "about": "pa000000"}}"#)).collect();
    only(
        &document(&fields(POINTS, "", "", &arcs.join(","), &cs.join(","))),
        "constraints",
        "38301120",
    );
    // 1024 points (2048 unknowns) and 8,193 midpoints, two rows each: 33,558,528.
    let points: Vec<String> = (0..1024)
        .map(|i| format!(r#"{{"id": "p{i:07}", "x": {i}, "y": 0, "construction": false}}"#))
        .collect();
    let line = r#"{"id": "n0000000", "from": "p0000000", "to": "p0000001", "construction": false}"#;
    let cs: Vec<String> = (0..8193).map(|i| format!(r#"{{"id": "k{i:07}", "type": "midpoint", "point": "p0000002", "line": "n0000000"}}"#)).collect();
    only(
        &document(&fields(&points.join(","), line, "", "", &cs.join(","))),
        "constraints",
        "33558528",
    );
}

#[test]
fn the_schema_refuses_sketches_the_reader_refuses_by_shape() {
    let schema: serde_json::Value =
        serde_json::from_str(&silico_core::document::schema().unwrap()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let base = constrained(
        r#"{"id": "k0000000", "type": "horizontal", "line": "na000000"}, {"id": "k0000001", "type": "length", "line": "na000000", "value": 3, "driving": true}"#,
    );
    assert!(read(base.as_bytes()).is_ok());
    assert!(validator.is_valid(&serde_json::from_str(&base).unwrap()));
    let mut admitted = Vec::new();
    for (from, to) in [
        (
            r#""line": "na000000"}"#,
            r#""line": "na000000", "value": 3}"#,
        ),
        (r#", "driving": true}"#, "}"),
        (r#""type": "horizontal""#, r#""type": "coincident""#),
        (r#""plane": "xy""#, r#""plane": "XY""#),
        (r#""x": 10, "#, r#""x": 10, "z": 0, "#),
        (r#""x": 10, "#, ""),
        (r#""id": "pa000000""#, r#""id": "PA000000""#),
        (r#""from": "pa000000""#, r#""from": "origin""#),
        (
            r#""construction": false}, {"id": "pb"#,
            r#""construction": 0}, {"id": "pb"#,
        ),
        (r#""value": 3"#, r#""value": "3""#),
        (r#""line": "na000000"}"#, r#""line": "x-axis"}"#),
        (r#""radius": 5, "#, ""),
        (r#", "arcs": ["#, r#", "arcz": ["#),
    ] {
        let text = base.replacen(from, to, 1);
        assert_ne!(text, base, "{from}");
        assert!(read(text.as_bytes()).is_err(), "reader takes {to}");
        if validator.is_valid(&serde_json::from_str(&text).unwrap()) {
            admitted.push(to);
        }
    }
    assert!(
        admitted.is_empty(),
        "the schema takes what the reader refuses: {admitted:#?}"
    );
}

#[test]
fn a_tangent_between_curves_sharing_an_end_is_a_tangent_at() {
    // The line ends at pb, where the arc starts: FreeCAD makes that a tangency at the point
    // (180d722a53); an edge tangency there is redundant where it holds.
    only(
        &constrained(
            r#"{"id": "k0000000", "type": "tangent", "a": "na000000", "b": "aa000000", "side": "left"}"#,
        ),
        "constraints.0.b",
        "a tangent's na000000 and aa000000 share an end, point pb000000",
    );
    let error = &problems(&constrained(
        r#"{"id": "k0000000", "type": "tangent", "a": "na000000", "b": "aa000000", "side": "left"}"#,
    ))[0];
    assert_eq!(
        error.suggestion(),
        Some("make it a tangent_at, at point pb000000")
    );
    // A circle has no ends, so a line through its center can be tangent to it.
    none(&constrained(
        r#"{"id": "k0000000", "type": "tangent", "a": "na000000", "b": "ca000000", "side": "left"}"#,
    ));
}

#[test]
fn a_point_line_distance_from_the_lines_own_end_is_refused() {
    only(
        &constrained(
            r#"{"id": "k0000000", "type": "point_line_distance", "point": "pb000000", "line": "na000000", "value": 3, "side": "left", "driving": true}"#,
        ),
        "constraints.0.point",
        "a point_line_distance's point, pb000000, is part of na000000 itself",
    );
}

#[test]
fn reference_dimensions_take_what_a_solve_can_measure() {
    // Driving: above 0 and at most a kilometre. Reference: from 0, and up to 3 km, past the
    // 2√2 km (2828427.124746... mm) two points within a kilometre can be apart.
    for (constraint, problem) in [
        (
            r#"{"id": "k0000000", "type": "distance", "a": "pa000000", "b": "pb000000", "value": 0, "driving": false}"#,
            None,
        ),
        (
            r#"{"id": "k0000000", "type": "point_line_distance", "point": "pc000000", "line": "na000000", "value": 0, "side": "left", "driving": false}"#,
            None,
        ),
        (
            r#"{"id": "k0000000", "type": "distance", "a": "pa000000", "b": "pb000000", "value": 2828427.124747, "driving": false}"#,
            None,
        ),
        (
            r#"{"id": "k0000000", "type": "length", "line": "na000000", "value": 3000000, "driving": false}"#,
            None,
        ),
        (
            r#"{"id": "k0000000", "type": "length", "line": "na000000", "value": 3000000.000001, "driving": false}"#,
            Some("a reference length's value is 3000000.000001 mm, but it's from 0 to 3000000 mm"),
        ),
        (
            r#"{"id": "k0000000", "type": "radius", "curve": "ca000000", "value": -0.000001, "driving": false}"#,
            Some("a reference radius's value is -0.000001 mm, but it's from 0 to 3000000 mm"),
        ),
        (
            r#"{"id": "k0000000", "type": "distance_x", "a": "pa000000", "b": "pb000000", "value": -3000000, "driving": false}"#,
            None,
        ),
        (
            r#"{"id": "k0000000", "type": "distance_x", "a": "pa000000", "b": "pb000000", "value": -3000000.000001, "driving": false}"#,
            Some(
                "a reference distance_x's value is -3000000.000001 mm, but it's at most 3000000 mm either way",
            ),
        ),
        (
            r#"{"id": "k0000000", "type": "distance", "a": "pa000000", "b": "pb000000", "value": 0, "driving": true}"#,
            Some("a distance's value is 0 mm, but it's above 0 and at most 1000000 mm"),
        ),
    ] {
        match problem {
            None => none(&constrained(constraint)),
            Some(message) => only(&constrained(constraint), "constraints.0.value", message),
        }
    }
}

#[test]
fn geometry_the_solver_measures_as_no_size_is_refused() {
    // 1e-200 squared underflows to 0: to the solver, a line of no length and an arc of no
    // radius, though the points are at different places.
    let tiny = r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 0, "y": 1e-200, "construction": false}, {"id": "pc000000", "x": 1e-200, "y": 0, "construction": false}, {"id": "pd000000", "x": 0, "y": 10, "construction": false}"#;
    only(
        &document(&fields(
            tiny,
            r#"{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}"#,
            "",
            "",
            "",
        )),
        "lines.0.to",
        "line na000000's ends, points pa000000 at (0, 0) and pb000000 at (0, 0.",
    );
    only(
        &document(&fields(
            tiny,
            "",
            "",
            r#"{"id": "aa000000", "center": "pa000000", "start": "pc000000", "end": "pd000000", "construction": false}"#,
            "",
        )),
        "arcs.0.start",
        "is too close to its center at (0, 0) for its radius to be anything but 0 to the solver",
    );
    // A line from 0 to -0 is the same place: no length.
    only(
        &document(&fields(
            r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": -0, "y": 0, "construction": false}"#,
            r#"{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}"#,
            "",
            "",
            "",
        )),
        "lines.0.to",
        "too close for its length to be anything but 0",
    );
}

#[test]
fn an_arc_whose_start_and_end_are_drawn_at_one_place_is_refused() {
    let points = r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 10, "y": 0, "construction": false}, {"id": "pc000000", "x": 10, "y": 0, "construction": false}"#;
    only(
        &document(&fields(points, "", "", ARC, "")),
        "arcs.0.end",
        "arc aa000000's start and end, points pb000000 and pc000000, are both drawn at (10, 0)",
    );
}
