//! break-it: hostile input for M3.2's sketches in the document (`document::read`,
//! `document::write`, and the gate's sketch rules, `document::validate`), and the sketch's
//! outcome in a recompute. Expected values come from the design
//! (docs/design/m3.2-sketch-format.md), from
//! FreeCAD's own refusals (src/Mod/Sketcher/Gui/CommandConstraints.cpp), from the sketch
//! worker's own check (`silico_protocol::SketchSystem::check`, `within_reach`), or from hand
//! calculation. Tests marked `BREAKS:` were bug reports, each since fixed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::fixture;
use super::model::WAIT;

use super::within;

use std::sync::OnceLock;
use std::time::Duration;

use fixture::{ARC, CIRCLE, FEATURE, LINE, POINTS, constrained, document, fields, problems};
use proptest::prelude::*;
use silico_core::document::{
    Checked, ElementId, FeatureId, Operation, PROBLEMS_LISTED, Sketch, read, schema, validate,
    write,
};
use silico_core::{Error, ErrorCode};
use within::within;

fn read_error(text: &str) -> Error {
    read(text.as_bytes()).unwrap_err()
}

#[track_caller]
fn assert_refused_at(text: &str, parameter: &str) -> Error {
    let all = problems(text);
    let found = all
        .iter()
        .find(|e| e.parameter() == Some(parameter))
        .unwrap_or_else(|| panic!("nothing refused at {parameter}: {all:#?}"));
    assert_eq!(found.code(), ErrorCode::InvalidInput, "{found:?}");
    assert_eq!(found.feature(), Some(FEATURE), "{found:?}");
    found.clone()
}

// ---------------------------------------------------------------------------------------
// Constraints that force geometry of no size, which FreeCAD refuses when they're made.

#[test]
fn symmetric_of_a_lines_two_ends_about_the_line_itself() {
    // BREAKS: na runs from pa to pb; mirroring pa about the line through pa and pb gives pa
    // itself, so the constraint holds only with pb on pa: a line of no length, which the
    // sketch worker then refuses ("a line drawn with no length"). FreeCAD refuses it when
    // it's made: "Cannot add a symmetry constraint between a line and its end points!"
    // (CommandConstraints.cpp:3805, GeoId1 == GeoId2 == GeoId3). Past the GUI, FreeCAD
    // 1.1.3's solve (the PlaneGCS Silico's worker runs) says success and collapses the line
    // to (7.17157, 0)-(7.17157, 0): the next solve's worker refuses that line, blaming it,
    // not this constraint. The gate admits it.
    let text = constrained(
        r#"{"id": "ka000000", "type": "symmetric", "a": "pa000000", "b": "pb000000", "about": "na000000"}"#,
    );
    assert_refused_at(&text, "constraints.ka000000.about");
}

#[test]
fn midpoint_of_a_line_at_its_own_end() {
    // BREAKS: pa is na's start; midway along na at pa holds only with na of no length.
    // FreeCAD refuses a symmetry of a line's ends about one of its own end points:
    // "Cannot add a symmetry constraint between an element and its end points!"
    // (CommandConstraints.cpp:9839); FreeCAD 1.1.3's solve calls it conflicting. The gate
    // admits it.
    let text = constrained(
        r#"{"id": "ka000000", "type": "midpoint", "point": "pa000000", "line": "na000000"}"#,
    );
    assert_refused_at(&text, "constraints.ka000000.point");
}

#[test]
fn a_circles_own_center_on_the_circle() {
    // BREAKS: pa is ca's center; pa on ca holds only at radius 0, a circle of no size, which
    // the gate refuses when it's written (circles.<id>.radius) but not when a constraint forces
    // it. FreeCAD won't put a point of an element on the element itself (GeoIdVt ==
    // GeoIdCrv, CommandConstraints.cpp:5430: "because they are parts of the same element"),
    // and FreeCAD 1.1.3's solve calls it conflicting.
    let text = constrained(
        r#"{"id": "ka000000", "type": "on", "point": "pa000000", "curve": "ca000000"}"#,
    );
    assert_refused_at(&text, "constraints.ka000000.point");
}

#[test]
fn an_arcs_own_center_on_the_arc() {
    // BREAKS: as a circle's: pa is aa's center, and on its circle only at radius 0, where
    // its start is its center, which the gate refuses as drawn (arcs.<id>.start).
    let text = constrained(
        r#"{"id": "ka000000", "type": "on", "point": "pa000000", "curve": "aa000000"}"#,
    );
    assert_refused_at(&text, "constraints.ka000000.point");
}

// ---------------------------------------------------------------------------------------
// Sketches the gate admits that the sketch worker refuses: the design promises "a document
// the gate admits is one the worker can solve".

#[test]
fn a_line_between_two_points_drawn_at_the_same_place() {
    // BREAKS: two points, both at (3, 4), and a line from one to the other: drawn with no
    // length. The sketch worker refuses it before solving ("geometry 2 is a line drawn with
    // no length", SketchSystem::check: "the solver gets 0/0 for its direction; FreeCAD
    // can't draw one"), so the gate's "nothing is drawn with no size" misses it: it
    // compares the line's points by ID only.
    let text = document(&fields(
        r#"{"id": "pa000000", "x": 3, "y": 4, "construction": false}, {"id": "pb000000", "x": 3, "y": 4, "construction": false}"#,
        LINE,
        "",
        "",
        "",
    ));
    let error = assert_refused_at(&text, "lines.na000000.to");
    assert!(error.message().contains("na000000"), "{error:?}");
}

#[test]
fn an_arc_whose_radius_from_its_points_is_past_a_kilometre() {
    // BREAKS: every point is within a kilometre of the origin, as the gate requires, but
    // the arc's radius (center to start) is √((2e6)² + (2e6)²) = 2.83e6 mm. The radius is
    // one of the solver's values, and the worker takes only values within a kilometre
    // (`within_reach`, the same rule as a circle's radius, which the gate does check).
    // The radius is at least its run along x alone, 2e6 mm, which the worker refuses.
    let (cx, sx) = (-1e6f64, 1e6f64);
    assert!(!silico_protocol::within_reach(sx - cx));
    let text = document(&fields(
        r#"{"id": "pa000000", "x": -1000000, "y": -1000000, "construction": false}, {"id": "pb000000", "x": 1000000, "y": 1000000, "construction": false}, {"id": "pc000000", "x": 1000000, "y": -1000000, "construction": false}"#,
        "",
        "",
        ARC,
        "",
    ));
    assert_refused_at(&text, "arcs.aa000000.start");
}

#[test]
fn an_arc_with_a_radius_of_a_kilometre_exactly_is_admitted() {
    // The other side of the line above: center at the origin, start a kilometre away.
    assert!(silico_protocol::within_reach(1e6));
    let text = document(&fields(
        r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 1000000, "y": 0, "construction": false}, {"id": "pc000000", "x": 0, "y": 1000000, "construction": false}"#,
        "",
        "",
        ARC,
        "",
    ));
    assert!(problems(&text).is_empty(), "{:#?}", problems(&text));
}

// ---------------------------------------------------------------------------------------
// Errors.

#[test]
fn a_refusal_names_the_element_it_refuses() {
    // BREAKS: the design says each refusal names "the feature, the field
    // (`constraints.3.line`), and the element". The field was then a place in the list as
    // read, which `silico fmt` reorders (lists are written sorted by ID), so the element's
    // ID is what stays true; but a dimension out of range, or a point too far away, was
    // named only by its place: not in the error's elements, nor in its message. (Its field
    // names it by its ID now, decision 15; its elements still name it.)
    let text = document(&fields(
        r#"{"id": "pa000000", "x": 2000000, "y": 0, "construction": false}, {"id": "pb000000", "x": 10, "y": 0, "construction": false}"#,
        LINE,
        "",
        "",
        r#"{"id": "ka000000", "type": "length", "line": "na000000", "value": 0, "driving": true}"#,
    ));
    for (parameter, id) in [
        ("points.pa000000.x", "pa000000"),
        ("constraints.ka000000.value", "ka000000"),
    ] {
        let error = assert_refused_at(&text, parameter);
        assert!(
            error.elements().iter().any(|e| e == id) || error.message().contains(id),
            "{parameter} doesn't name {id}: {error:#?}"
        );
    }
}

#[test]
fn a_sketch_too_large_to_save_isnt_blamed_on_its_name_promptly() {
    // BREAKS: 30,000 points at the smallest float, written in the file as `5e-324` (6
    // bytes) and canonically in full (`0.000…5`, 326 bytes): the canonical text is ~21 MB,
    // past the 16 MiB a document may be, so `read` refuses it, naming the sketch, as it
    // should. But its suggestion is "make this feature's name or note shorter": the name is
    // "S", and there is no note. Before sketches, only a name or note could make one
    // feature that large.
    let points: Vec<String> = (0..30_000)
        .map(|i| format!(r#"{{"id": "p{i:07}", "x": 5e-324, "y": 5e-324, "construction": false}}"#))
        .collect();
    let text = document(&fields(&points.join(","), "", "", "", ""));
    assert!(text.len() < 3_000_000, "{}", text.len());
    let error = within(Duration::from_secs(20), move || read_error(&text));
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error:?}");
    assert_eq!(error.feature(), Some(FEATURE), "{error:?}");
    let suggestion = error.suggestion().unwrap_or_default();
    assert!(
        !suggestion.contains("name or note"),
        "a sketch of 30,000 points is told: {suggestion:?}"
    );
}

#[test]
fn hostile_text_isnt_echoed_whole_into_errors() {
    let huge = "q".repeat(1 << 20);
    let cases = [
        (
            document(&format!(
                "\"plane\": \"{huge}\", \"points\": [], \"lines\": [], \"circles\": [], \"arcs\": [], \"constraints\": []"
            )),
            "plane",
        ),
        (
            document(&fields(
                &format!(r#"{{"id": "{huge}", "x": 0, "y": 0, "construction": false}}"#),
                "",
                "",
                "",
                "",
            )),
            "points.0.id",
        ),
        (
            constrained(&format!(
                r#"{{"id": "ka000000", "type": "horizontal", "line": "{huge}"}}"#
            )),
            "constraints.ka000000.line",
        ),
        (
            constrained(&format!(r#"{{"id": "ka000000", "type": "{huge}"}}"#)),
            "constraints.ka000000.type",
        ),
        (
            constrained(&format!(
                r#"{{"id": "ka000000", "type": "tangent", "a": "na000000", "b": "ca000000", "side": "{huge}"}}"#
            )),
            "constraints.ka000000.side",
        ),
        (
            document(&fields(
                &format!(
                    r#"{{"id": "pa000000", "x": 1{}, "y": 0, "construction": false}}"#,
                    "0".repeat(1 << 20)
                ),
                "",
                "",
                "",
                "",
            )),
            "points.pa000000.x",
        ),
        (
            constrained(&format!(
                r#"{{"id": "ka000000", "type": "angle", "a": "na000000", "b": "x_axis", "value": 1{}, "driving": true}}"#,
                "0".repeat(1 << 20)
            )),
            "constraints.ka000000.value",
        ),
    ];
    for (text, parameter) in cases {
        let error = read_error(&text);
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{parameter}");
        assert_eq!(error.parameter(), Some(parameter), "{error:?}");
        let size = error.message().len() + error.suggestion().unwrap_or_default().len();
        assert!(size < 2000, "{parameter}: {size} bytes of error");
    }
}

#[test]
fn hostile_ids_and_references_are_refused_naming_the_field() {
    for id in [
        "PA000000", // upper case
        "pi000000", // i, l, o, u aren't Crockford's
        "pl000000",
        "po000000",
        "pu000000",
        "pa00000",   // 7
        "pa0000000", // 9
        "",
        "ééééééé",  // 8 bytes... and 7 chars
        "éééééééé", // 8 chars, 16 bytes
        "origin",
        "x_axis",
        "pa00000\\u0000",
        "pa000000 ",
        "\\u0070a000000", // "pa000000" escaped: the same text, so it reads
    ] {
        let text = document(&fields(
            &format!(r#"{{"id": "{id}", "x": 0, "y": 0, "construction": false}}"#),
            "",
            "",
            "",
            "",
        ));
        match read(text.as_bytes()) {
            Ok(document) => {
                assert_eq!(id, "\\u0070a000000", "{id:?} read");
                let Operation::Sketch(sketch) = &document.features[0].operation else {
                    panic!()
                };
                assert_eq!(sketch.points[0].id.as_str(), "pa000000");
            }
            Err(error) => {
                assert_eq!(error.code(), ErrorCode::InvalidInput, "{id:?}");
                assert_eq!(error.parameter(), Some("points.0.id"), "{id:?}: {error:?}");
            }
        }
    }
    for reference in [
        "ORIGIN", "Origin", "x-axis", "xaxis", " origin", "origin ", "", "z_axis", "y_axis2",
    ] {
        let text = constrained(&format!(
            r#"{{"id": "ka000000", "type": "on", "point": "pc000000", "curve": "{reference}"}}"#
        ));
        let error = read_error(&text);
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{reference:?}");
        assert_eq!(
            error.parameter(),
            Some("constraints.ka000000.curve"),
            "{reference:?}: {error:?}"
        );
    }
    // A feature's ID, the sketch's own, is no element of it.
    let error = assert_refused_at(
        &constrained(
            r#"{"id": "ka000000", "type": "on", "point": "pc000000", "curve": "k7f2qx3m"}"#,
        ),
        "constraints.ka000000.curve",
    );
    assert!(
        error.message().contains("isn't the ID of any element"),
        "{error:?}"
    );
    // A constraint naming itself, or another constraint.
    for curve in ["ka000000", "kb000000"] {
        let error = assert_refused_at(
            &constrained(&format!(
                r#"{{"id": "ka000000", "type": "on", "point": "pc000000", "curve": "{curve}"}}, {{"id": "kb000000", "type": "horizontal", "line": "na000000"}}"#
            )),
            "constraints.ka000000.curve",
        );
        assert!(error.message().contains("a constraint"), "{error:?}");
    }
}

#[test]
fn every_field_of_the_wrong_kind_is_refused_naming_it() {
    let point = |x: &str, construction: &str| {
        document(&fields(
            &format!(r#"{{"id": "pa000000", "x": {x}, "y": 0, "construction": {construction}}}"#),
            "",
            "",
            "",
            "",
        ))
    };
    let cases = [
        (
            document(
                "\"plane\": 1, \"points\": [], \"lines\": [], \"circles\": [], \"arcs\": [], \"constraints\": []",
            ),
            "plane",
        ),
        (
            document(
                "\"plane\": \"XY\", \"points\": [], \"lines\": [], \"circles\": [], \"arcs\": [], \"constraints\": []",
            ),
            "plane",
        ),
        (
            document(
                "\"plane\": \"xy\", \"points\": {}, \"lines\": [], \"circles\": [], \"arcs\": [], \"constraints\": []",
            ),
            "points",
        ),
        (
            document(
                "\"plane\": \"xy\", \"points\": [1], \"lines\": [], \"circles\": [], \"arcs\": [], \"constraints\": []",
            ),
            "points.0",
        ),
        (
            document(
                "\"plane\": \"xy\", \"points\": [[]], \"lines\": [], \"circles\": [], \"arcs\": [], \"constraints\": []",
            ),
            "points.0",
        ),
        (
            document(
                "\"plane\": \"xy\", \"points\": [null], \"lines\": [], \"circles\": [], \"arcs\": [], \"constraints\": []",
            ),
            "points.0",
        ),
        (point("\"1\"", "false"), "points.pa000000.x"),
        (point("null", "false"), "points.pa000000.x"),
        (point("true", "false"), "points.pa000000.x"),
        (point("0", "0"), "points.pa000000.construction"),
        (point("0", "\"false\""), "points.pa000000.construction"),
        (point("0", "null"), "points.pa000000.construction"),
        (
            constrained(r#"{"id": "ka000000", "type": 1, "line": "na000000"}"#),
            "constraints.ka000000.type",
        ),
        (
            constrained(r#"{"id": "ka000000", "type": "Horizontal", "line": "na000000"}"#),
            "constraints.ka000000.type",
        ),
        (
            constrained(r#"{"id": "ka000000", "type": "horizontal", "line": 7}"#),
            "constraints.ka000000.line",
        ),
        (
            constrained(r#"{"id": "ka000000", "type": "horizontal", "line": ["na000000"]}"#),
            "constraints.ka000000.line",
        ),
        (
            constrained(
                r#"{"id": "ka000000", "type": "length", "line": "na000000", "value": "10", "driving": true}"#,
            ),
            "constraints.ka000000.value",
        ),
        (
            constrained(
                r#"{"id": "ka000000", "type": "length", "line": "na000000", "value": 10, "driving": 1}"#,
            ),
            "constraints.ka000000.driving",
        ),
        (
            constrained(
                r#"{"id": "ka000000", "type": "angle", "a": "na000000", "b": "x_axis", "value": "30", "driving": true}"#,
            ),
            "constraints.ka000000.value",
        ),
        (
            constrained(
                r#"{"id": "ka000000", "type": "angle", "a": "na000000", "b": "x_axis", "value": 0.0000000001, "driving": true}"#,
            ),
            "constraints.ka000000.value",
        ),
        (
            constrained(
                r#"{"id": "ka000000", "type": "tangent", "a": "na000000", "b": "ca000000", "side": true}"#,
            ),
            "constraints.ka000000.side",
        ),
        (
            constrained(
                r#"{"id": "ka000000", "type": "tangent", "a": "na000000", "b": "ca000000", "side": "LEFT"}"#,
            ),
            "constraints.ka000000.side",
        ),
        (
            constrained(
                r#"{"id": "ka000000", "type": "tangent_at", "a": "na000000", "b": "aa000000", "at": "pb000000", "direction": "left"}"#,
            ),
            "constraints.ka000000.direction",
        ),
        (
            constrained(
                r#"{"id": "ka000000", "type": "point_line_distance", "point": "pc000000", "line": "na000000", "value": 1, "side": "outside", "driving": true}"#,
            ),
            "constraints.ka000000.side",
        ),
    ];
    for (text, parameter) in cases {
        let error = read_error(&text);
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{parameter}");
        assert_eq!(error.feature(), Some(FEATURE), "{parameter}: {error:?}");
        assert_eq!(error.parameter(), Some(parameter), "{error:?}");
        assert!(error.suggestion().is_some(), "{error:?}");
    }
}

#[test]
fn a_refused_angles_suggestion_says_how_to_write_it() {
    // Found by M2.6's tests review: no test read these suggestions. The largest angle is
    // worked out by hand: i64::MAX nanodegrees is 9223372036.854775807 degrees.
    let angle = |value: &str| {
        constrained(&format!(
            r#"{{"id": "ka000000", "type": "angle", "a": "na000000", "b": "x_axis", "value": {value}, "driving": true}}"#
        ))
    };
    for (value, says) in [
        (
            "0.0000000001",
            "round it to 9 decimals: angles are whole nanodegrees",
        ),
        (
            "9223372036.854775808",
            "an angle is at most 9223372036.854775807 degrees either way",
        ),
        (
            "-9223372036.854775808",
            "an angle is at most 9223372036.854775807 degrees either way",
        ),
        ("-0", "write 0"),
    ] {
        let error = read_error(&angle(value));
        assert_eq!(
            error.parameter(),
            Some("constraints.ka000000.value"),
            "{error:?}"
        );
        assert_eq!(error.suggestion(), Some(says), "{value}: {error:?}");
    }
}

#[test]
fn a_constraint_with_another_forms_keys_or_missing_its_own_is_refused() {
    for (constraint, parameter) in [
        // A value on a form with none.
        (
            r#"{"id": "ka000000", "type": "horizontal", "line": "na000000", "value": 1}"#,
            "constraints.ka000000.value",
        ),
        // A side on a distance with none.
        (
            r#"{"id": "ka000000", "type": "distance", "a": "pa000000", "b": "pb000000", "value": 1, "driving": true, "side": "left"}"#,
            "constraints.ka000000.side",
        ),
        // A tangent with no side: never decided from the geometry (D4).
        (
            r#"{"id": "ka000000", "type": "tangent", "a": "na000000", "b": "ca000000"}"#,
            "constraints.ka000000.side",
        ),
        // A dimension with no driving: always written.
        (
            r#"{"id": "ka000000", "type": "radius", "curve": "ca000000", "value": 5}"#,
            "constraints.ka000000.driving",
        ),
        // No type at all.
        (
            r#"{"id": "ka000000", "line": "na000000"}"#,
            "constraints.ka000000.type",
        ),
        // A key written twice.
        (
            r#"{"id": "ka000000", "type": "horizontal", "line": "na000000", "line": "na000000"}"#,
            "",
        ),
    ] {
        let error = read_error(&constrained(constraint));
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{constraint}");
        if !parameter.is_empty() {
            assert_eq!(
                error.parameter(),
                Some(parameter),
                "{constraint}: {error:?}"
            );
        }
    }
    // An unknown list beside a sketch's own, and one of its own left out.
    let error = read_error(&document(
        "\"plane\": \"xy\", \"points\": [], \"lines\": [], \"circles\": [], \"arcs\": [], \"ellipses\": [], \"constraints\": []",
    ));
    assert_eq!(error.parameter(), Some("ellipses"), "{error:?}");
    let error = read_error(&document(
        "\"plane\": \"xy\", \"points\": [], \"lines\": [], \"circles\": [], \"constraints\": []",
    ));
    assert_eq!(error.parameter(), Some("arcs"), "{error:?}");
}

// ---------------------------------------------------------------------------------------
// The gate's ranges, exactly at each edge and one step either side.

#[test]
fn ranges_hold_exactly_at_their_edges() {
    let next_up = |v: f64| f64::from_bits(v.to_bits() + 1);
    // A point's coordinate: within a kilometre, inclusive, either way.
    for (x, ok) in [
        (1e6, true),
        (-1e6, true),
        (next_up(1e6), false),
        (-next_up(1e6), false),
        (f64::MAX, false),
        (-0.0, true),
        (f64::from_bits(1), true),
    ] {
        let text = document(&fields(
            &format!(r#"{{"id": "pa000000", "x": {x}, "y": {x}, "construction": false}}"#),
            "",
            "",
            "",
            "",
        ));
        let all = problems(&text);
        if ok {
            assert!(all.is_empty(), "{x}: {all:#?}");
        } else {
            // Both coordinates, each named.
            let parameters: Vec<_> = all.iter().filter_map(|e| e.parameter()).collect();
            assert_eq!(
                parameters,
                ["points.pa000000.x", "points.pa000000.y"],
                "{x}"
            );
        }
    }
    // A circle's radius: above 0, at most a kilometre.
    for (radius, ok) in [
        (1e6, true),
        (next_up(1e6), false),
        (0.0, false),
        (-0.0, false),
        (-f64::from_bits(1), false),
        (1e-6, true),
    ] {
        let text = document(&fields(
            POINTS,
            "",
            &format!(
                r#"{{"id": "ca000000", "center": "pa000000", "radius": {radius}, "construction": false}}"#
            ),
            "",
            "",
        ));
        assert_eq!(problems(&text).is_empty(), ok, "radius {radius}");
    }
    // Dimensions: whole nanometres and nanodegrees, so one step is 0.000001 mm or
    // 0.000000001°.
    let dimension = |form: &str, value: &str| {
        let constraint = match form {
            "length" => format!(
                r#"{{"id": "ka000000", "type": "length", "line": "na000000", "value": {value}, "driving": true}}"#
            ),
            "distance_x" => format!(
                r#"{{"id": "ka000000", "type": "distance_x", "a": "pa000000", "b": "pb000000", "value": {value}, "driving": true}}"#
            ),
            "diameter" => format!(
                r#"{{"id": "ka000000", "type": "diameter", "curve": "ca000000", "value": {value}, "driving": true}}"#
            ),
            "angle" => format!(
                r#"{{"id": "ka000000", "type": "angle", "a": "na000000", "b": "x_axis", "value": {value}, "driving": true}}"#
            ),
            _ => unreachable!(),
        };
        problems(&constrained(&constraint)).is_empty()
    };
    for (form, value, ok) in [
        ("length", "0.000001", true),
        ("length", "0", false),
        ("length", "-0.000001", false),
        ("length", "1000000", true),
        ("length", "1000000.000001", false),
        ("distance_x", "0", true),
        ("distance_x", "-1000000", true),
        ("distance_x", "1000000", true),
        ("distance_x", "-1000000.000001", false),
        ("distance_x", "1000000.000001", false),
        ("diameter", "2000000", true),
        ("diameter", "2000000.000001", false),
        ("diameter", "0.000001", true),
        ("angle", "360", true),
        ("angle", "-360", true),
        ("angle", "0", true),
        ("angle", "360.000000001", false),
        ("angle", "-360.000000001", false),
        ("angle", "9223372036.854775807", false),
        ("angle", "-9223372036.854775807", false),
    ] {
        assert_eq!(dimension(form, value), ok, "{form} {value}");
    }
}

// ---------------------------------------------------------------------------------------
// What each place takes, every kind tried in every place of every form, against the
// design's table (D4, with D6's fixed references).

/// Each candidate for a place: its text and its kind.
const CANDIDATES: [(&str, &str); 7] = [
    ("pb000000", "point"),
    ("na000000", "line"),
    ("ca000000", "circle"),
    ("aa000000", "arc"),
    ("kz000000", "constraint"),
    ("origin", "origin"),
    ("x_axis", "axis"),
];

/// A place in a form: its key, the value that fills it in the valid form, and the kinds
/// it takes.
type Place = (&'static str, &'static str, &'static [&'static str]);

/// Each form: a valid constraint with `{key}` for each place, the values that fill the
/// places in it, and what each place takes, from the design's table.
fn forms() -> Vec<(&'static str, Vec<Place>)> {
    const POINT: &[&str] = &["point", "origin"];
    const LINE: &[&str] = &["line"];
    const LINE_OR_AXIS: &[&str] = &["line", "axis"];
    const CURVE: &[&str] = &["line", "circle", "arc", "axis"];
    const ROUND: &[&str] = &["circle", "arc"];
    const EQUAL: &[&str] = &["line", "circle", "arc"];
    const ENDED: &[&str] = &["line", "arc"];
    const SHARED: &[&str] = &["point"];
    const ABOUT: &[&str] = &["line", "point", "origin", "axis"];
    vec![
        (
            r#"{"id": "ka000000", "type": "horizontal", "line": "{line}"}"#,
            vec![("line", "na000000", LINE)],
        ),
        (
            r#"{"id": "ka000000", "type": "vertical", "line": "{line}"}"#,
            vec![("line", "na000000", LINE)],
        ),
        (
            r#"{"id": "ka000000", "type": "horizontal_points", "a": "{a}", "b": "{b}"}"#,
            vec![("a", "pc000000", POINT), ("b", "pa000000", POINT)],
        ),
        (
            r#"{"id": "ka000000", "type": "vertical_points", "a": "{a}", "b": "{b}"}"#,
            vec![("a", "pc000000", POINT), ("b", "pa000000", POINT)],
        ),
        (
            r#"{"id": "ka000000", "type": "length", "line": "{line}", "value": 3, "driving": true}"#,
            vec![("line", "na000000", LINE)],
        ),
        (
            r#"{"id": "ka000000", "type": "distance", "a": "{a}", "b": "{b}", "value": 3, "driving": true}"#,
            vec![("a", "pc000000", POINT), ("b", "pa000000", POINT)],
        ),
        (
            r#"{"id": "ka000000", "type": "distance_x", "a": "{a}", "b": "{b}", "value": 3, "driving": true}"#,
            vec![("a", "pc000000", POINT), ("b", "pa000000", POINT)],
        ),
        (
            r#"{"id": "ka000000", "type": "distance_y", "a": "{a}", "b": "{b}", "value": 3, "driving": true}"#,
            vec![("a", "pc000000", POINT), ("b", "pa000000", POINT)],
        ),
        (
            r#"{"id": "ka000000", "type": "point_line_distance", "point": "{point}", "line": "{line}", "value": 3, "side": "left", "driving": true}"#,
            vec![
                ("point", "pc000000", POINT),
                ("line", "na000000", LINE_OR_AXIS),
            ],
        ),
        (
            r#"{"id": "ka000000", "type": "on", "point": "{point}", "curve": "{curve}"}"#,
            vec![("point", "pc000000", POINT), ("curve", "na000000", CURVE)],
        ),
        (
            r#"{"id": "ka000000", "type": "parallel", "a": "{a}", "b": "{b}"}"#,
            vec![
                ("a", "y_axis", LINE_OR_AXIS),
                ("b", "na000000", LINE_OR_AXIS),
            ],
        ),
        (
            r#"{"id": "ka000000", "type": "perpendicular", "a": "{a}", "b": "{b}"}"#,
            vec![
                ("a", "y_axis", LINE_OR_AXIS),
                ("b", "na000000", LINE_OR_AXIS),
            ],
        ),
        (
            r#"{"id": "ka000000", "type": "angle", "a": "{a}", "b": "{b}", "value": 30, "driving": true}"#,
            vec![
                ("a", "y_axis", LINE_OR_AXIS),
                ("b", "na000000", LINE_OR_AXIS),
            ],
        ),
        (
            r#"{"id": "ka000000", "type": "radius", "curve": "{curve}", "value": 3, "driving": true}"#,
            vec![("curve", "ca000000", ROUND)],
        ),
        (
            r#"{"id": "ka000000", "type": "diameter", "curve": "{curve}", "value": 3, "driving": true}"#,
            vec![("curve", "ca000000", ROUND)],
        ),
        (
            r#"{"id": "ka000000", "type": "equal", "a": "{a}", "b": "{b}"}"#,
            vec![("a", "ca000000", EQUAL), ("b", "aa000000", EQUAL)],
        ),
        (
            r#"{"id": "ka000000", "type": "tangent", "a": "{a}", "b": "{b}", "side": "left"}"#,
            vec![("a", "y_axis", CURVE), ("b", "ca000000", CURVE)],
        ),
        (
            r#"{"id": "ka000000", "type": "tangent_on", "a": "{a}", "at": "{at}", "b": "{b}", "direction": "same"}"#,
            vec![
                ("a", "na000000", ENDED),
                ("at", "pb000000", SHARED),
                ("b", "ca000000", CURVE),
            ],
        ),
        (
            r#"{"id": "ka000000", "type": "tangent_at", "a": "{a}", "b": "{b}", "at": "{at}", "direction": "same"}"#,
            vec![
                ("a", "na000000", ENDED),
                ("b", "aa000000", ENDED),
                ("at", "pb000000", SHARED),
            ],
        ),
        (
            r#"{"id": "ka000000", "type": "symmetric", "a": "{a}", "b": "{b}", "about": "{about}"}"#,
            vec![
                ("a", "pc000000", POINT),
                ("b", "pa000000", POINT),
                ("about", "y_axis", ABOUT),
            ],
        ),
        (
            r#"{"id": "ka000000", "type": "midpoint", "point": "{point}", "line": "{line}"}"#,
            vec![("point", "pc000000", POINT), ("line", "na000000", LINE)],
        ),
    ]
}

#[test]
fn every_kind_in_every_place_is_taken_exactly_where_the_design_says() {
    let other = r#"{"id": "kz000000", "type": "horizontal", "line": "na000000"}"#;
    let forms = forms();
    assert_eq!(forms.len(), silico_core::document::CONSTRAINT_TYPES.len());
    for (template, places) in &forms {
        let filled = |vary: &str, with: &str| {
            let mut text = (*template).to_owned();
            for (key, base, _) in places {
                let value = if *key == vary { with } else { base };
                text = text.replace(&format!("{{{key}}}"), value);
            }
            text
        };
        // The base fits.
        let base = constrained(&format!("{}, {other}", filled("", "")));
        assert!(
            problems(&base).is_empty(),
            "{template}: {:#?}",
            problems(&base)
        );
        for (key, _, takes) in places {
            let field = format!("constraints.ka000000.{key}");
            for (candidate, kind) in CANDIDATES {
                let text = constrained(&format!("{}, {other}", filled(key, candidate)));
                let all = problems(&text);
                for e in &all {
                    assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
                    assert_eq!(e.feature(), Some(FEATURE), "{e:?}");
                    assert!(
                        e.parameter()
                            .is_some_and(|p| p.starts_with("constraints.ka000000")),
                        "{e:?}"
                    );
                }
                let refused_kind = all.iter().any(|e| {
                    e.parameter() == Some(field.as_str()) && e.message().contains(", but a")
                });
                assert_eq!(
                    refused_kind,
                    !takes.contains(&kind),
                    "{key} = {candidate} ({kind}) in {template}: {all:#?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Size.

#[test]
fn a_huge_sketch_of_nothing_but_problems_is_checked_promptly() {
    // 120,000 constraints, each naming nothing that exists: read and checked within a few
    // seconds of CPU, every problem counted, a hundred listed.
    let constraints: Vec<String> = (0..120_000)
        .map(|i| {
            format!(r#"{{"id": "k{i:07}", "type": "distance", "a": "zzzzzzzz", "b": "yyyyyyyy", "value": 1, "driving": true}}"#)
        })
        .collect();
    let text = document(&fields("", "", "", "", &constraints.join(",")));
    let invalid = within(Duration::from_secs(20), move || {
        validate(&read(text.as_bytes()).unwrap()).unwrap_err()
    });
    assert_eq!(invalid.errors().len(), PROBLEMS_LISTED);
    // Two dangling references each, and two sizes: 120,000 constraints past 10,000, and
    // their 120,000 values (each a dimension's) and the origin's and axes' six past 10,000.
    assert_eq!(invalid.total(), 2 * 120_000 + 2);
}

#[test]
fn ten_thousand_elements_sharing_one_id_are_each_named_once_promptly() {
    let points: Vec<String> = (0..10_000)
        .map(|i| format!(r#"{{"id": "pa000000", "x": {i}, "y": 0, "construction": false}}"#))
        .collect();
    let text = document(&fields(&points.join(","), "", "", "", ""));
    let invalid = within(Duration::from_secs(10), move || {
        validate(&read(text.as_bytes()).unwrap()).unwrap_err()
    });
    // 9,999 duplicates, and 20,000 unknowns past 1024, 20,006 values past 10,000, and
    // 10,003 pieces of geometry (the origin and the axes with them) past 10,000.
    assert_eq!(invalid.total(), 9_999 + 3);
    assert_eq!(invalid.errors()[0].parameter(), Some("points.1.id"));
}

// ---------------------------------------------------------------------------------------
// Recompute.

fn rebuilt(text: &str) -> (std::sync::Arc<silico_core::recompute::Model>, Vec<String>) {
    use silico_core::fake::{FakeKernel, FakeSketches};
    use silico_core::recompute::{Recompute, RunOutcome};
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    let checked = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    let model = match engine.start(checked).wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the recompute didn't finish"),
    };
    let calls = engine
        .kernel()
        .calls()
        .iter()
        .map(|c| c.op.name().to_owned())
        .collect();
    (model, calls)
}

fn sketch_feature(id: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "S", {}}}"#,
        fields(POINTS, LINE, CIRCLE, ARC, "")
    )
}

fn doc(features: &[String]) -> String {
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{}]}}",
        features.join(", ")
    )
}

#[test]
fn a_document_of_sketches_alone_asks_the_kernel_nothing() {
    use silico_core::recompute::FeatureOutcome;
    let text = doc(&[sketch_feature("s0000000"), sketch_feature("s0000001")]);
    let (model, calls) = rebuilt(&text);
    assert!(calls.is_empty(), "{calls:?}");
    assert!(model.bodies().is_empty());
    assert_eq!(model.errors().count(), 0);
    assert!(model.drift().is_empty());
    for id in ["s0000000", "s0000001"] {
        assert!(
            matches!(
                model.outcome(&FeatureId::new(id).unwrap()),
                Some(FeatureOutcome::Sketch(_))
            ),
            "{id}"
        );
    }
    // Saved as rebuilt, the sketches have no fingerprint, and the document opens and passes.
    let saved = write(&model.saved()).unwrap();
    assert!(!saved.contains("fingerprint"), "{saved}");
    assert!(validate(&read(saved.as_bytes()).unwrap()).is_ok());
}

#[test]
fn sketches_between_solids_change_nothing_about_them() {
    use silico_core::recompute::FeatureOutcome;
    let boxed = |id: &str, x: i32| {
        format!(
            r#"{{"id": "{id}", "type": "box", "v": 1, "name": "B", "position": [{x}, 0, 0], "length": 1, "width": 2, "height": {}}}"#,
            3 + x
        )
    };
    // A sketch first, between the two boxes, between the boxes and their fuse, and last;
    // the fuse names its boxes past two sketches.
    let text = doc(&[
        sketch_feature("s0000000"),
        boxed("b0000000", 0),
        sketch_feature("s0000001"),
        boxed("b0000001", 5),
        sketch_feature("s0000002"),
        r#"{"id": "f0000000", "type": "fuse", "v": 1, "name": "F", "base": "b0000000", "tool": "b0000001"}"#.to_owned(),
        sketch_feature("s0000003"),
    ]);
    let (model, calls) = rebuilt(&text);
    let id = |t: &str| FeatureId::new(t).unwrap();
    assert_eq!(model.bodies(), [id("f0000000")]);
    assert_eq!(model.errors().count(), 0);
    for s in ["s0000000", "s0000001", "s0000002", "s0000003"] {
        assert!(
            matches!(model.outcome(&id(s)), Some(FeatureOutcome::Sketch(_))),
            "{s}: {:?}",
            model.outcome(&id(s))
        );
        assert_eq!(model.fingerprint(&id(s)), None);
    }
    for b in ["b0000000", "b0000001", "f0000000"] {
        assert!(
            matches!(model.outcome(&id(b)), Some(FeatureOutcome::Built { .. })),
            "{b}: {:?}",
            model.outcome(&id(b))
        );
    }
    // Two boxes, one translate, one fuse, and the three measured: each summarized and
    // described in one request.
    let mut sorted = calls.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        [
            "Describe",
            "Describe",
            "Describe",
            "Fuse",
            "MakeBox",
            "MakeBox",
            "Summarize",
            "Summarize",
            "Summarize",
            "Translate"
        ],
        "{calls:?}"
    );
}

#[test]
fn a_boolean_naming_a_sketch_is_refused_whichever_operand_it_is() {
    let boxed = r#"{"id": "b0000000", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 1, "width": 2, "height": 3}"#;
    for (base, tool, field) in [
        ("s0000000", "b0000000", "base"),
        ("b0000000", "s0000000", "tool"),
    ] {
        let text = doc(&[
            sketch_feature("s0000000"),
            boxed.to_owned(),
            format!(
                r#"{{"id": "f0000000", "type": "cut", "v": 1, "name": "F", "base": "{base}", "tool": "{tool}"}}"#
            ),
        ]);
        let errors = validate(&read(text.as_bytes()).unwrap()).unwrap_err();
        let error = &errors.errors()[0];
        assert_eq!(errors.total(), 1, "{errors:#?}");
        assert_eq!(error.feature(), Some("f0000000"));
        assert_eq!(error.parameter(), Some(field));
        assert!(error.message().contains("makes no solid"), "{error:?}");
    }
    // Both operands the same sketch: each named, neither as consumed twice.
    let text = doc(&[
        sketch_feature("s0000000"),
        r#"{"id": "f0000000", "type": "common", "v": 1, "name": "F", "base": "s0000000", "tool": "s0000000"}"#.to_owned(),
    ]);
    let errors = validate(&read(text.as_bytes()).unwrap()).unwrap_err();
    let parameters: Vec<_> = errors
        .errors()
        .iter()
        .filter_map(|e| e.parameter())
        .collect();
    assert_eq!(parameters, ["base", "tool"], "{errors:#?}");
}

// ---------------------------------------------------------------------------------------
// Properties over random hostile sketches.

fn validator() -> &'static jsonschema::Validator {
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        let schema: serde_json::Value = serde_json::from_str(&schema().unwrap()).unwrap();
        jsonschema::validator_for(&schema).unwrap()
    })
}

/// IDs few enough to collide across kinds and with themselves, and one nothing has.
const IDS: [&str; 8] = [
    "pa000000", "pb000000", "pc000000", "na000000", "ca000000", "aa000000", "ka000000", "zzzzzzzz",
];

fn id() -> impl Strategy<Value = &'static str> {
    prop::sample::select(&IDS[..])
}

fn reference() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        6 => id(),
        1 => prop::sample::select(&["origin", "x_axis", "y_axis"][..]),
    ]
}

/// Coordinates at the edges: signed zeros, the smallest floats, a kilometre and a step
/// either side, the largest float, and some ordinary values.
fn coordinate() -> impl Strategy<Value = String> {
    let next_up = |v: f64| f64::from_bits(v.to_bits() + 1);
    let edges = vec![
        0.0,
        -0.0,
        f64::from_bits(1),
        -f64::from_bits(1),
        f64::MIN_POSITIVE,
        1e-7,
        1e6,
        -1e6,
        next_up(1e6),
        -next_up(1e6),
        f64::MAX,
        -f64::MAX,
        0.1 + 0.2,
        10.0,
    ];
    prop_oneof![
        3 => prop::sample::select(edges).prop_map(|v: f64| format!("{v:e}")),
        1 => (-2e6f64..2e6).prop_map(|v| format!("{v}")),
    ]
}

/// Lengths at the edges, in whole nanometres.
fn length() -> impl Strategy<Value = &'static str> {
    prop::sample::select(
        &[
            "0",
            "0.000001",
            "-0.000001",
            "1",
            "1000000",
            "1000000.000001",
            "-1000000",
            "2000000",
            "2000000.000001",
            "9223372036854.775807",
        ][..],
    )
}

fn angle() -> impl Strategy<Value = &'static str> {
    prop::sample::select(
        &[
            "0",
            "360",
            "-360",
            "360.000000001",
            "-360.000000001",
            "90",
            "0.000000001",
            "9223372036.854775807",
        ][..],
    )
}

fn boolean() -> impl Strategy<Value = &'static str> {
    prop::sample::select(&["true", "false"][..])
}

fn constraint() -> impl Strategy<Value = String> {
    (
        id(),
        0usize..21,
        [reference(), reference(), reference()],
        length(),
        angle(),
        boolean(),
        prop::sample::select(&["left", "right", "outside", "inside"][..]),
        prop::sample::select(&["same", "opposite"][..]),
    )
        .prop_map(|(id, form, [a, b, c], length, angle, driving, side, direction)| {
            let line_side = if side == "outside" || side == "inside" {
                "left"
            } else {
                side
            };
            let head = format!(r#"{{"id": "{id}", "type": "{}""#, silico_core::document::CONSTRAINT_TYPES[form]);
            let rest = match form {
                0 | 1 => format!(r#""line": "{a}""#),
                2 | 3 | 10 | 11 | 15 => format!(r#""a": "{a}", "b": "{b}""#),
                4 => format!(r#""line": "{a}", "value": {length}, "driving": {driving}"#),
                5..=7 => format!(
                    r#""a": "{a}", "b": "{b}", "value": {length}, "driving": {driving}"#
                ),
                8 => format!(
                    r#""point": "{a}", "line": "{b}", "value": {length}, "side": "{line_side}", "driving": {driving}"#
                ),
                9 => format!(r#""point": "{a}", "curve": "{b}""#),
                12 => format!(
                    r#""a": "{a}", "b": "{b}", "value": {angle}, "driving": {driving}"#
                ),
                13 | 14 => format!(r#""curve": "{a}", "value": {length}, "driving": {driving}"#),
                16 => format!(r#""a": "{a}", "b": "{b}", "side": "{side}""#),
                17 => format!(
                    r#""a": "{a}", "at": "{c}", "b": "{b}", "direction": "{direction}""#
                ),
                18 => format!(
                    r#""a": "{a}", "b": "{b}", "at": "{c}", "direction": "{direction}""#
                ),
                19 => format!(r#""a": "{a}", "b": "{b}", "about": "{c}""#),
                20 => format!(r#""point": "{a}", "line": "{b}""#),
                _ => unreachable!(),
            };
            format!("{head}, {rest}}}")
        })
}

fn hostile_sketch() -> impl Strategy<Value = String> {
    let point = (id(), coordinate(), coordinate(), boolean()).prop_map(|(id, x, y, c)| {
        format!(r#"{{"id": "{id}", "x": {x}, "y": {y}, "construction": {c}}}"#)
    });
    let line = (id(), id(), id(), boolean()).prop_map(|(id, from, to, c)| {
        format!(r#"{{"id": "{id}", "from": "{from}", "to": "{to}", "construction": {c}}}"#)
    });
    let circle = (id(), id(), coordinate(), boolean()).prop_map(|(id, center, r, c)| {
        format!(r#"{{"id": "{id}", "center": "{center}", "radius": {r}, "construction": {c}}}"#)
    });
    let arc = (id(), id(), id(), id(), boolean()).prop_map(|(id, center, start, end, c)| {
        format!(
            r#"{{"id": "{id}", "center": "{center}", "start": "{start}", "end": "{end}", "construction": {c}}}"#
        )
    });
    (
        prop::sample::select(&["xy", "xz", "yz"][..]),
        prop::collection::vec(point, 0..6),
        prop::collection::vec(line, 0..4),
        prop::collection::vec(circle, 0..3),
        prop::collection::vec(arc, 0..3),
        prop::collection::vec(constraint(), 0..6),
    )
        .prop_map(|(plane, points, lines, circles, arcs, constraints)| {
            document(&format!(
                "\"plane\": \"{plane}\", \"points\": [{}], \"lines\": [{}], \"circles\": [{}], \"arcs\": [{}], \"constraints\": [{}]",
                points.join(", "),
                lines.join(", "),
                circles.join(", "),
                arcs.join(", "),
                constraints.join(", ")
            ))
        })
}

/// The gate's problems, as code, feature, and parameter.
fn verdict(document: &silico_core::document::Document) -> Vec<(ErrorCode, String, String)> {
    match validate(document) {
        Ok(()) => Vec::new(),
        Err(invalid) => invalid
            .errors()
            .iter()
            .map(|e| {
                (
                    e.code(),
                    e.feature().unwrap_or_default().to_owned(),
                    e.parameter().unwrap_or_default().to_owned(),
                )
            })
            .collect(),
    }
}

/// The sketch's list fields a parameter may name: `list`, `list.m`, or `list.m.key`, where
/// the member `m` is an ID or a place, from 0 with no leading zeros (decision 15).
fn names_a_field(parameter: &str) -> bool {
    let mut parts = parameter.split('.');
    let list = parts.next().unwrap_or_default();
    let member = parts.next();
    let key = parts.next();
    let place = |m: &str| m.parse::<usize>().is_ok_and(|n| n.to_string() == m);
    ["points", "lines", "circles", "arcs", "constraints"].contains(&list)
        && member.is_none_or(|m| place(m) || ElementId::new(m).is_some())
        && key
            .is_none_or(|k| !k.is_empty() && k.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        && parts.next().is_none()
        || parameter == "fingerprint"
}

fn sketch_in(document: &silico_core::document::Document) -> &Sketch {
    match &document.features[0].operation {
        Operation::Sketch(sketch) => sketch,
        other => panic!("{other:?}"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 3000,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0xb4ea_c175),
        ..ProptestConfig::default()
    })]

    /// Any sketch the reader reads: the gate never panics, refuses only as `invalid_input`
    /// naming the sketch and one of its fields, decides the same after a save and reopen
    /// (which sorts every list), and the save is canonical and taken by the schema.
    #[test]
    fn hostile_sketches_are_judged_the_same_saved_and_reopened(text in hostile_sketch()) {
        let document = match read(text.as_bytes()) {
            Ok(document) => document,
            // A coordinate past the largest float isn't generated; everything else reads.
            Err(error) => return Err(TestCaseError::fail(format!("{error:?}\n{text}"))),
        };
        let first = verdict(&document);
        for (code, feature, parameter) in &first {
            prop_assert_eq!(*code, ErrorCode::InvalidInput);
            prop_assert_eq!(feature.as_str(), FEATURE);
            prop_assert!(names_a_field(parameter), "{}", parameter);
        }
        prop_assert_eq!(&verdict(&document), &first, "the gate isn't deterministic");

        let saved = write(&document).unwrap();
        let reopened = read(saved.as_bytes()).unwrap();
        prop_assert_eq!(write(&reopened).unwrap(), saved.clone(), "not canonical");
        // The same verdict, but for where each element named by its place sits in its list.
        let unplaced = |v: &[(ErrorCode, String, String)]| {
            let mut v: Vec<String> = v
                .iter()
                .map(|(_, _, p)| {
                    // Only a place moves with a sort: a member named by its ID keeps it.
                    let mut parts: Vec<&str> = p.split('.').collect();
                    if parts.len() > 1 && ElementId::new(parts[1]).is_none() {
                        parts[1] = "#";
                    }
                    parts.join(".")
                })
                .collect();
            v.sort();
            v
        };
        prop_assert_eq!(unplaced(&verdict(&reopened)), unplaced(&first), "{}", saved);
        // Each list held sorted by ID now, with the same elements.
        let (a, b) = (sketch_in(&document), sketch_in(&reopened));
        prop_assert_eq!(a.plane, b.plane);
        let mut ids: Vec<_> = a.elements().map(|(id, kind)| (kind, id.clone())).collect();
        let mut reopened_ids: Vec<_> = b.elements().map(|(id, kind)| (kind, id.clone())).collect();
        prop_assert!(reopened_ids.windows(2).all(|w| w[0].0 != w[1].0 || w[0].1 <= w[1].1));
        ids.sort();
        reopened_ids.sort();
        prop_assert_eq!(ids, reopened_ids);
        let json: serde_json::Value = serde_json::from_str(&saved).unwrap();
        prop_assert!(validator().is_valid(&json), "the schema refuses {}", saved);
    }
}

// ---------------------------------------------------------------------------------------
// Every sketch the types can hold is written as text the reader reads back as it.

mod constructed {
    use super::*;
    use silico_core::document::{
        Angle, Coordinate, Direction, Document, ElementId, Feature, FeatureVersion, Form,
        FormatVersion, Length, LineSide, Name, NamingScheme, Plane, Reference, SketchArc,
        SketchCircle, SketchConstraint, SketchLine, SketchPoint, TangentSide, Units,
    };

    fn element() -> impl Strategy<Value = ElementId> {
        // Few enough to collide.
        prop::sample::select(&["00000000", "zzzzzzzz", "a0000000", "a0000001"][..])
            .prop_map(|t| ElementId::new(t).unwrap())
    }

    fn reference() -> impl Strategy<Value = Reference> {
        prop_oneof![
            3 => element().prop_map(Reference::Element),
            1 => Just(Reference::Origin),
            1 => Just(Reference::XAxis),
            1 => Just(Reference::YAxis),
        ]
    }

    fn coordinate() -> impl Strategy<Value = Coordinate> {
        prop_oneof![
            any::<u64>().prop_map(f64::from_bits),
            prop::sample::select(vec![
                0.0,
                -0.0,
                f64::MAX,
                -f64::MAX,
                f64::from_bits(1),
                -f64::from_bits(1)
            ]),
        ]
        .prop_filter_map("finite", Coordinate::new)
    }

    /// Any length or angle a document holds, in nanometres or nanodegrees: at most i64::MAX
    /// either way (i64::MIN, which only code can make, the reader refuses).
    fn i64_edge() -> impl Strategy<Value = i64> {
        prop_oneof![
            any::<i64>().prop_filter("at most i64::MAX either way", |&n| n != i64::MIN),
            prop::sample::select(vec![-i64::MAX, -1, 0, 1, i64::MAX]),
        ]
    }

    fn form() -> impl Strategy<Value = Form> {
        (
            0usize..20,
            [reference(), reference(), reference()],
            i64_edge(),
            any::<bool>(),
            0usize..4,
            any::<bool>(),
        )
            .prop_map(|(n, [a, b, c], value, driving, side, flag)| {
                let length = Length::from_nm(value);
                let angle = Angle::from_ndeg(value);
                let line_side = if flag {
                    LineSide::Left
                } else {
                    LineSide::Right
                };
                let tangent_side = [
                    TangentSide::Left,
                    TangentSide::Right,
                    TangentSide::Outside,
                    TangentSide::Inside,
                ][side];
                let direction = if flag {
                    Direction::Same
                } else {
                    Direction::Opposite
                };
                match n {
                    0 => Form::Horizontal { line: a },
                    1 => Form::Vertical { line: a },
                    2 => Form::HorizontalPoints { a, b },
                    3 => Form::VerticalPoints { a, b },
                    4 => Form::Length {
                        line: a,
                        value: length,
                        driving,
                    },
                    5 => Form::Distance {
                        a,
                        b,
                        value: length,
                        driving,
                    },
                    6 => Form::DistanceX {
                        a,
                        b,
                        value: length,
                        driving,
                    },
                    7 => Form::DistanceY {
                        a,
                        b,
                        value: length,
                        driving,
                    },
                    8 => Form::PointLineDistance {
                        point: a,
                        line: b,
                        value: length,
                        side: line_side,
                        driving,
                    },
                    9 => Form::On { point: a, curve: b },
                    10 => Form::Parallel { a, b },
                    11 => Form::Perpendicular { a, b },
                    12 => Form::Angle {
                        a,
                        b,
                        value: angle,
                        driving,
                    },
                    13 => Form::Radius {
                        curve: a,
                        value: length,
                        driving,
                    },
                    14 => Form::Diameter {
                        curve: a,
                        value: length,
                        driving,
                    },
                    15 => Form::Equal { a, b },
                    16 => Form::Tangent {
                        a,
                        b,
                        side: tangent_side,
                    },
                    17 => Form::TangentAt {
                        a,
                        b,
                        at: c,
                        direction,
                    },
                    18 => Form::Symmetric { a, b, about: c },
                    _ => Form::Midpoint { point: a, line: b },
                }
            })
    }

    fn sketch() -> impl Strategy<Value = Sketch> {
        let point = (element(), coordinate(), coordinate(), any::<bool>()).prop_map(
            |(id, x, y, construction)| SketchPoint {
                id,
                x,
                y,
                construction,
            },
        );
        let line = (element(), element(), element(), any::<bool>()).prop_map(
            |(id, from, to, construction)| SketchLine {
                id,
                from,
                to,
                construction,
            },
        );
        let circle = (element(), element(), coordinate(), any::<bool>()).prop_map(
            |(id, center, radius, construction)| SketchCircle {
                id,
                center,
                radius,
                construction,
            },
        );
        let arc = (element(), element(), element(), element(), any::<bool>()).prop_map(
            |(id, center, start, end, construction)| SketchArc {
                id,
                center,
                start,
                end,
                construction,
            },
        );
        let constraint = (element(), form()).prop_map(|(id, form)| SketchConstraint { id, form });
        (
            prop::sample::select(Plane::ALL.to_vec()),
            prop::collection::vec(point, 0..5),
            prop::collection::vec(line, 0..4),
            prop::collection::vec(circle, 0..3),
            prop::collection::vec(arc, 0..3),
            prop::collection::vec(constraint, 0..8),
        )
            .prop_map(
                |(plane, points, lines, circles, arcs, constraints)| Sketch {
                    plane,
                    points,
                    lines,
                    circles,
                    arcs,
                    constraints,
                },
            )
    }

    fn holding(sketch: Sketch) -> Document {
        Document {
            silico: FormatVersion,
            units: Units,
            naming: NamingScheme,
            features: vec![Feature {
                id: FeatureId::new(FEATURE).unwrap(),
                operation: Operation::Sketch(sketch),
                version: FeatureVersion,
                name: Name::new("S").unwrap(),
                note: None,
                fingerprint: None,
            }],
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 3000,
            failure_persistence: None,
            rng_seed: proptest::test_runner::RngSeed::Fixed(0x0c0_5c7ed),
            ..ProptestConfig::default()
        })]

        /// `read(write(d))` is `d` with each list sorted by ID (stably), for any sketch the
        /// types hold, the gate's verdict on it aside.
        #[test]
        fn any_sketch_written_reads_back_as_itself_sorted(sketch in sketch()) {
            let mut sorted = sketch.clone();
            sorted.points.sort_by(|a, b| a.id.cmp(&b.id));
            sorted.lines.sort_by(|a, b| a.id.cmp(&b.id));
            sorted.circles.sort_by(|a, b| a.id.cmp(&b.id));
            sorted.arcs.sort_by(|a, b| a.id.cmp(&b.id));
            sorted.constraints.sort_by(|a, b| a.id.cmp(&b.id));
            let text = write(&holding(sketch)).unwrap();
            let back = match read(text.as_bytes()) {
                Ok(document) => document,
                Err(error) => return Err(TestCaseError::fail(format!("{error:?}\n{text}"))),
            };
            prop_assert_eq!(back, holding(sorted), "{}", text);
            let _ = validate(&read(text.as_bytes()).unwrap());
        }
    }
}
