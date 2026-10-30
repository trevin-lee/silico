//! M3.2: sketches in the document (docs/design/m3.2-sketch-format.md). Each is read
//! exactly, written in canonical form with its lists sorted by ID, and checked by the gate.
//! Expected text and messages are written out by hand. Coordinates are checked bit for bit,
//! against floats written as literals.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::fixture;
use super::model::WAIT;

use fixture::{
    ARC, CIRCLE, FEATURE, LINE, POINTS, arc, circle, constrained, document, fields, line, point,
    problems,
};
use proptest::prelude::*;
use silico_core::document::{
    Coordinate, Direction, ElementId, Form, Operation, Plane, Reference, Sketch, TangentSide, read,
    validate, write,
};
use silico_core::{Error, ErrorCode};

/// The design's example, a 40 × 20 rectangle with a construction diagonal, in canonical
/// form: the feature holds objects, so it's written one member per line (M2.1's D7), and
/// each list is sorted by ID.
const EXAMPLE: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {
      "id": "k7f2qx3m",
      "type": "sketch",
      "v": 1,
      "name": "Base",
      "plane": "xy",
      "points": [
        {"id": "0a1b2c3d", "x": 0, "y": 0, "construction": false},
        {"id": "2p3q4r5s", "x": 0, "y": 20, "construction": false},
        {"id": "4e5f6g7h", "x": 40, "y": 0, "construction": false},
        {"id": "8j9k0m1n", "x": 40, "y": 20, "construction": false}
      ],
      "lines": [
        {"id": "6t7v8w9x", "from": "0a1b2c3d", "to": "4e5f6g7h", "construction": false},
        {"id": "a0b1c2d3", "from": "4e5f6g7h", "to": "8j9k0m1n", "construction": false},
        {"id": "e4f5g6h7", "from": "0a1b2c3d", "to": "8j9k0m1n", "construction": true},
        {"id": "j8k9m0n1", "from": "8j9k0m1n", "to": "2p3q4r5s", "construction": false},
        {"id": "p2q3r4s5", "from": "2p3q4r5s", "to": "0a1b2c3d", "construction": false}
      ],
      "circles": [],
      "arcs": [],
      "constraints": [
        {"id": "c0d1e2f3", "type": "horizontal", "line": "6t7v8w9x"},
        {"id": "g4h5j6k7", "type": "length", "line": "6t7v8w9x", "value": 40, "driving": true},
        {"id": "m8n9p0q1", "type": "distance_x", "a": "origin", "b": "0a1b2c3d", "value": 0, "driving": true},
        {"id": "r2s3t4v5", "type": "vertical", "line": "a0b1c2d3"},
        {"id": "w6x7y8z9", "type": "length", "line": "a0b1c2d3", "value": 20, "driving": true},
        {"id": "x0y1z2a3", "type": "horizontal", "line": "j8k9m0n1"},
        {"id": "y4z5a6b7", "type": "distance_y", "a": "origin", "b": "0a1b2c3d", "value": 0, "driving": true},
        {"id": "z8a9b0c1", "type": "vertical", "line": "p2q3r4s5"}
      ]
    }
  ]
}
"#;

fn sketch_of(text: &str) -> Sketch {
    let document = read(text.as_bytes()).unwrap();
    match &document.features[0].operation {
        Operation::Sketch(sketch) => sketch.clone(),
        other => panic!("{other:?}"),
    }
}

fn el(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn read_error(text: &str) -> Error {
    read(text.as_bytes()).unwrap_err()
}

/// The one problem the gate finds in `text`.
#[track_caller]
fn problem(text: &str) -> Error {
    let all = problems(text);
    assert_eq!(all.len(), 1, "{all:#?}");
    all[0].clone()
}

#[track_caller]
fn assert_error(error: &Error, parameter: &str, message: &str) {
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error:?}");
    assert_eq!(error.feature(), Some(FEATURE), "{error:?}");
    assert_eq!(error.parameter(), Some(parameter), "{error:?}");
    assert!(
        error.message().starts_with(message),
        "{:?}\nwanted it to start {message:?}",
        error.message()
    );
}

#[test]
fn the_base_sketchs_json_is_what_the_builders_write() {
    // The base sketch's lists are constants, as they're used where a `&str` must be; they're
    // held here to the builders every other sketch test writes with (the M3 review's QM4:
    // the builders had been copied, and the copies had drifted apart).
    let points = [
        point("pa000000", 0.0, 0.0),
        point("pb000000", 10.0, 0.0),
        point("pc000000", 0.0, 10.0),
    ];
    assert_eq!(POINTS, points.join(", "));
    assert_eq!(LINE, line("na000000", "pa000000", "pb000000"));
    assert_eq!(CIRCLE, circle("ca000000", "pa000000", 5.0));
    assert_eq!(ARC, arc("aa000000", "pa000000", "pb000000", "pc000000"));
}

#[test]
fn the_designs_example_reads_as_its_values_and_writes_back_byte_for_byte() {
    let document = read(EXAMPLE.as_bytes()).unwrap();
    assert!(validate(&document).is_ok());
    let sketch = sketch_of(EXAMPLE);
    assert_eq!(sketch.plane, Plane::Xy);
    assert_eq!(sketch.points.len(), 4);
    let corner = &sketch.points[3];
    assert_eq!(corner.id, el("8j9k0m1n"));
    assert_eq!((corner.x.mm(), corner.y.mm()), (40.0, 20.0));
    let diagonal = &sketch.lines[2];
    assert_eq!(
        (&diagonal.from, &diagonal.to, diagonal.construction),
        (&el("0a1b2c3d"), &el("8j9k0m1n"), true)
    );
    assert_eq!(
        sketch.constraints[2].form,
        Form::DistanceX {
            a: Reference::Origin,
            b: Reference::Element(el("0a1b2c3d")),
            value: silico_core::document::Length::from_nm(0),
            driving: true,
        }
    );
    assert_eq!(write(&document).unwrap(), EXAMPLE);
}

#[test]
fn lists_are_read_in_any_order_and_written_sorted_by_id() {
    // The example's points and constraints reversed, and its lines rotated.
    let mut text = EXAMPLE.to_owned();
    for (a, b) in [
        (
            r#"{"id": "0a1b2c3d", "x": 0, "y": 0, "construction": false}"#,
            r#"{"id": "8j9k0m1n", "x": 40, "y": 20, "construction": false}"#,
        ),
        (
            r#"{"id": "c0d1e2f3", "type": "horizontal", "line": "6t7v8w9x"}"#,
            r#"{"id": "z8a9b0c1", "type": "vertical", "line": "p2q3r4s5"}"#,
        ),
    ] {
        text = text.replace(a, "SWAP").replace(b, a).replace("SWAP", b);
    }
    assert_ne!(text, EXAMPLE);
    let document = read(text.as_bytes()).unwrap();
    let sketch = sketch_of(&text);
    // Held as written.
    assert_eq!(sketch.points[0].id, el("8j9k0m1n"));
    assert_eq!(sketch.constraints[0].id, el("z8a9b0c1"));
    assert_eq!(write(&document).unwrap(), EXAMPLE);
}

#[test]
fn every_constraint_form_reads_and_writes_back_in_schema_order() {
    // One of each, with every choice written, in canonical form (each list sorted by ID).
    let constraints = [
        r#"{"id": "k0000000", "type": "horizontal", "line": "na000000"}"#,
        r#"{"id": "k0000001", "type": "vertical", "line": "na000000"}"#,
        r#"{"id": "k0000002", "type": "horizontal_points", "a": "pa000000", "b": "pb000000"}"#,
        r#"{"id": "k0000003", "type": "vertical_points", "a": "origin", "b": "pc000000"}"#,
        r#"{"id": "k0000004", "type": "length", "line": "na000000", "value": 10, "driving": true}"#,
        r#"{"id": "k0000005", "type": "distance", "a": "pa000000", "b": "pc000000", "value": 10.000001, "driving": false}"#,
        r#"{"id": "k0000006", "type": "distance_x", "a": "pb000000", "b": "pa000000", "value": -10, "driving": true}"#,
        r#"{"id": "k0000007", "type": "distance_y", "a": "origin", "b": "pc000000", "value": 10, "driving": true}"#,
        r#"{"id": "k0000008", "type": "point_line_distance", "point": "pc000000", "line": "na000000", "value": 10, "side": "left", "driving": true}"#,
        r#"{"id": "k0000009", "type": "on", "point": "pc000000", "curve": "y_axis"}"#,
        r#"{"id": "k000000a", "type": "parallel", "a": "na000000", "b": "x_axis"}"#,
        r#"{"id": "k000000b", "type": "perpendicular", "a": "na000000", "b": "y_axis"}"#,
        r#"{"id": "k000000c", "type": "angle", "a": "x_axis", "b": "na000000", "value": -0.000000001, "driving": true}"#,
        r#"{"id": "k000000d", "type": "radius", "curve": "ca000000", "value": 5, "driving": true}"#,
        r#"{"id": "k000000e", "type": "diameter", "curve": "aa000000", "value": 20, "driving": false}"#,
        r#"{"id": "k000000f", "type": "equal", "a": "ca000000", "b": "aa000000"}"#,
        r#"{"id": "k000000g", "type": "tangent", "a": "na000000", "b": "cb000000", "side": "right"}"#,
        r#"{"id": "k000000h", "type": "tangent_on", "a": "na000000", "at": "pb000000", "b": "ca000000", "direction": "same"}"#,
        r#"{"id": "k000000j", "type": "tangent_at", "a": "na000000", "b": "aa000000", "at": "pb000000", "direction": "opposite"}"#,
        r#"{"id": "k000000k", "type": "symmetric", "a": "pb000000", "b": "pc000000", "about": "origin"}"#,
        r#"{"id": "k000000m", "type": "midpoint", "point": "pc000000", "line": "na000000"}"#,
    ];
    let text = format!(
        r#"{{
  "silico": 1,
  "units": {{"length": "mm", "resolution": "1nm"}},
  "naming": 5,
  "features": [
    {{
      "id": "k7f2qx3m",
      "type": "sketch",
      "v": 1,
      "name": "S",
      "plane": "yz",
      "points": [
        {pa},
        {pb},
        {pc}
      ],
      "lines": [
        {LINE}
      ],
      "circles": [
        {CIRCLE},
        {cb}
      ],
      "arcs": [
        {ARC}
      ],
      "constraints": [
        {}
      ]
    }}
  ]
}}
"#,
        constraints.join(",\n        "),
        pa = r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}"#,
        pb = r#"{"id": "pb000000", "x": 10, "y": 0, "construction": false}"#,
        pc = r#"{"id": "pc000000", "x": 0, "y": 10, "construction": false}"#,
        // A second circle for the edge tangent, about pc: the tangent_on already holds na's
        // end on ca, and an edge tangent of that pair would repeat it (#258).
        cb = r#"{"id": "cb000000", "center": "pc000000", "radius": 5, "construction": false}"#,
    );
    let document = read(text.as_bytes()).unwrap();
    assert_eq!(write(&document).unwrap(), text);
    let sketch = sketch_of(&text);
    let types: Vec<&str> = sketch
        .constraints
        .iter()
        .map(|c| c.form.type_name())
        .collect();
    assert_eq!(types, silico_core::document::CONSTRAINT_TYPES);
    assert_eq!(
        sketch.constraints[16].form,
        Form::Tangent {
            a: Reference::Element(el("na000000")),
            b: Reference::Element(el("cb000000")),
            side: TangentSide::Right,
        }
    );
    assert_eq!(
        sketch.constraints[17].form,
        Form::TangentOn {
            a: Reference::Element(el("na000000")),
            at: Reference::Element(el("pb000000")),
            b: Reference::Element(el("ca000000")),
            direction: Direction::Same,
        }
    );
    // Each word reads as its own value.
    let Form::PointLineDistance { side, .. } = sketch.constraints[8].form else {
        panic!("{:?}", sketch.constraints[8].form)
    };
    assert_eq!(side, silico_core::document::LineSide::Left);
    assert_eq!(
        sketch.constraints[18].form,
        Form::TangentAt {
            a: Reference::Element(el("na000000")),
            b: Reference::Element(el("aa000000")),
            at: Reference::Element(el("pb000000")),
            direction: Direction::Opposite,
        }
    );
    // The schema takes it too.
    let schema: serde_json::Value =
        serde_json::from_str(&silico_core::document::schema().unwrap()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&serde_json::from_str(&text).unwrap()));
    // And the gate.
    assert!(validate(&document).is_ok(), "{:#?}", validate(&document));
}

/// A point at (x, y), as written, read back as the f64 it holds.
fn coordinate_of(x: &str) -> f64 {
    let text = document(&fields(
        &format!(r#"{{"id": "pa000000", "x": {x}, "y": 0, "construction": false}}"#),
        "",
        "",
        "",
        "",
    ));
    sketch_of(&text).points[0].x.mm()
}

/// How a coordinate of `value` is written.
fn written(value: f64) -> String {
    Coordinate::new(value).unwrap().to_string()
}

#[test]
fn coordinates_are_read_correctly_rounded_and_written_in_full() {
    // The float nearest each decimal: 0.1 + 0.2 is the float after 0.3's.
    assert_eq!(coordinate_of("0.1").to_bits(), 0.1f64.to_bits());
    assert_eq!(
        coordinate_of("0.30000000000000004").to_bits(),
        (0.1f64 + 0.2).to_bits()
    );
    assert_ne!(0.3f64.to_bits(), (0.1f64 + 0.2).to_bits());
    assert_eq!(written(0.1 + 0.2), "0.30000000000000004");
    assert_eq!(written(0.3), "0.3");
    // Exponents read; plain decimals written.
    assert_eq!(coordinate_of("1e-7").to_bits(), 1e-7f64.to_bits());
    assert_eq!(written(1e-7), "0.0000001");
    assert_eq!(coordinate_of("1.05E1"), 10.5);
    assert_eq!(written(10.5), "10.5");
    assert_eq!(written(40.0), "40");
    assert_eq!(written(1e6), "1000000");
    // Minus zero is a float of its own, read and written as one.
    assert_eq!(coordinate_of("-0").to_bits(), (-0.0f64).to_bits());
    assert_eq!(written(-0.0), "-0");
    assert_ne!(Coordinate::new(-0.0), Coordinate::new(0.0));
    // The smallest float, and a decimal far finer than it, which rounds to 0.
    assert_eq!(
        coordinate_of("5e-324").to_bits(),
        f64::from_bits(1).to_bits()
    );
    assert_eq!(coordinate_of("1e-400").to_bits(), 0.0f64.to_bits());
    // Not finite.
    assert_eq!(Coordinate::new(f64::NAN), None);
    assert_eq!(Coordinate::new(f64::INFINITY), None);
}

#[test]
fn a_coordinate_past_the_largest_float_is_refused() {
    let text = document(&fields(
        r#"{"id": "pa000000", "x": 1e400, "y": 0, "construction": false}"#,
        "",
        "",
        "",
        "",
    ));
    let error = read_error(&text);
    assert_error(
        &error,
        "points.pa000000.x",
        "points.pa000000.x 1e400 is too large, at line 1",
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 2000,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x5ce7c4),
        ..ProptestConfig::default()
    })]

    #[test]
    fn every_finite_coordinate_reads_back_as_the_same_bits(bits in any::<u64>()) {
        let value = f64::from_bits(bits);
        prop_assume!(value.is_finite());
        let text = written(value);
        prop_assert!(!text.contains(['e', 'E']), "{text}");
        prop_assert_eq!(coordinate_of(&text).to_bits(), bits, "{}", text);
    }
}

#[test]
fn a_sketch_read_wrongly_is_refused_naming_its_field() {
    let p = |x: &str| format!(r#"{{"id": "pa000000", "x": {x}, "y": 0, "construction": false}}"#);
    let cases: Vec<(String, &str, &str)> = vec![
        (
            document(&fields("", "", "", "", "").replace("\"xy\"", "\"XY\"")),
            "plane",
            "plane \"XY\" isn't a sketch plane",
        ),
        (
            document(&fields("", "", "", "", "").replace("\"points\": []", "\"points\": {}")),
            "points",
            "points is an object, not an array of points",
        ),
        (
            document(&fields(&p("\"1\""), "", "", "", "")),
            "points.pa000000.x",
            "points.pa000000.x is a string, not a number in millimetres",
        ),
        (
            document(&fields(
                r#"{"id": "pa000000", "x": 0, "construction": false}"#,
                "",
                "",
                "",
                "",
            )),
            "points.pa000000.y",
            "points.pa000000 has no y",
        ),
        (
            document(&fields(
                r#"{"id": "pa000000", "x": 0, "y": 0, "z": 0, "construction": false}"#,
                "",
                "",
                "",
                "",
            )),
            "points.pa000000.z",
            "points.pa000000 has a key it can't have: \"z\"",
        ),
        (
            document(&fields(
                r#"{"id": "pa000000", "x": 0, "y": 0, "construction": 0}"#,
                "",
                "",
                "",
                "",
            )),
            "points.pa000000.construction",
            "points.pa000000.construction is a number, not true or false",
        ),
        (
            document(&fields(
                r#"{"id": "PA000000", "x": 0, "y": 0, "construction": false}"#,
                "",
                "",
                "",
                "",
            )),
            "points.0.id",
            "points.0.id \"PA000000\" isn't a sketch element's ID",
        ),
        (
            document(&fields(
                "",
                r#"{"id": "na000000", "from": "origin", "to": "pb000000", "construction": false}"#,
                "",
                "",
                "",
            )),
            "lines.na000000.from",
            "lines.na000000.from \"origin\" isn't a sketch element's ID",
        ),
        (
            constrained(
                r#"{"id": "k0000000", "type": "coincident", "a": "pa000000", "b": "pb000000"}"#,
            ),
            "constraints.k0000000.type",
            "\"coincident\" isn't a constraint type",
        ),
        (
            constrained(r#"{"id": "k0000000", "type": "horizontal", "line": "x-axis"}"#),
            "constraints.k0000000.line",
            "constraints.k0000000.line \"x-axis\" isn't a sketch element's ID, origin, x_axis, or y_axis",
        ),
        (
            constrained(
                r#"{"id": "k0000000", "type": "tangent", "a": "na000000", "b": "ca000000", "side": "above"}"#,
            ),
            "constraints.k0000000.side",
            "constraints.k0000000.side \"above\" isn't one of left, right, outside, inside",
        ),
        (
            constrained(
                r#"{"id": "k0000000", "type": "length", "line": "na000000", "value": "=w", "driving": true}"#,
            ),
            "constraints.k0000000.value",
            "constraints.k0000000.value is a string, not a length in millimetres",
        ),
        (
            constrained(
                r#"{"id": "k0000000", "type": "angle", "a": "na000000", "b": "x_axis", "value": 0.0000000001, "driving": true}"#,
            ),
            "constraints.k0000000.value",
            "constraints.k0000000.value 0.0000000001 has more than 9 decimals",
        ),
        (
            constrained(r#"{"id": "k0000000", "type": "length", "line": "na000000", "value": 10}"#),
            "constraints.k0000000.driving",
            "constraints.k0000000 has no driving",
        ),
        (
            constrained(
                r#"{"id": "k0000000", "type": "horizontal", "line": "na000000", "value": 10}"#,
            ),
            "constraints.k0000000.value",
            "constraints.k0000000 has a key it can't have: \"value\"",
        ),
    ];
    for (text, parameter, message) in cases {
        let error = read_error(&text);
        assert_error(&error, parameter, message);
    }
}

#[test]
fn ids_are_unique_within_a_sketch_whatever_their_kind() {
    let error = problem(&constrained(
        r#"{"id": "na000000", "type": "horizontal", "line": "na000000"}"#,
    ));
    assert_error(
        &error,
        "constraints.0.id",
        "id na000000 is already the ID of line na000000 (lines.na000000)",
    );
    // The same ID in another sketch is another element.
    let one = document(&fields(POINTS, LINE, "", "", ""));
    let other = r#"{"id": "k7f2qx3n", "type": "sketch", "v": 1, "name": "T", "plane": "xz", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []}"#;
    let two = format!("{}, {other}]}}", one.strip_suffix("]}").unwrap());
    assert!(problems(&two).is_empty(), "{:#?}", problems(&two));
}

#[test]
fn a_curves_points_must_be_points_of_its_sketch() {
    let cases = [
        (
            document(&fields(
                POINTS,
                r#"{"id": "na000000", "from": "pa000000", "to": "pz000000", "construction": false}"#,
                "",
                "",
                "",
            )),
            "lines.na000000.to",
            "lines.na000000.to pz000000 isn't the ID of any element of this sketch",
        ),
        (
            document(&fields(
                POINTS,
                LINE,
                r#"{"id": "ca000000", "center": "na000000", "radius": 5, "construction": false}"#,
                "",
                "",
            )),
            "circles.ca000000.center",
            "circles.ca000000.center na000000 is a line, not a point",
        ),
        (
            document(&fields(
                POINTS,
                r#"{"id": "na000000", "from": "pa000000", "to": "pa000000", "construction": false}"#,
                "",
                "",
                "",
            )),
            "lines.na000000.to",
            "line na000000 goes from point pa000000 to itself, so it has no length",
        ),
        (
            document(&fields(
                POINTS,
                "",
                "",
                r#"{"id": "aa000000", "center": "pa000000", "start": "pa000000", "end": "pc000000", "construction": false}"#,
                "",
            )),
            "arcs.aa000000.start",
            "arc aa000000's start is its center, point pa000000, so it has no radius",
        ),
        (
            document(&fields(
                POINTS,
                "",
                "",
                r#"{"id": "aa000000", "center": "pa000000", "start": "pb000000", "end": "pb000000", "construction": false}"#,
                "",
            )),
            "arcs.aa000000.end",
            "arc aa000000 starts and ends at point pb000000, so it has no sweep",
        ),
    ];
    for (text, parameter, message) in cases {
        assert_error(&problem(&text), parameter, message);
    }
}

#[test]
fn coincident_ends_that_would_collapse_a_line_leave_it_running_from_a_point_to_itself() {
    // TestCoincidentCommandGui.py's collapse fixtures, in Silico's terms: with shared points,
    // making two ends coincident is giving them one point. Three lines in a chain, the first
    // one's end made the third one's start, the two ends either side of the middle line
    // (test_transitive_coincidence_cannot_collapse_intermediate_line), leave the middle line
    // running from that point to itself; a line's own ends made one
    // (test_coincident_endpoints_of_same_edge_are_rejected, FreeCAD #20119) is the same. The
    // gate refuses it, naming the line, where FreeCAD's solve fails.
    let text = document(&fields(
        r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 10, "y": 0, "construction": false}, {"id": "pd000000", "x": 30, "y": 0, "construction": false}"#,
        r#"{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}, {"id": "nb000000", "from": "pb000000", "to": "pb000000", "construction": false}, {"id": "nc000000", "from": "pb000000", "to": "pd000000", "construction": false}"#,
        "",
        "",
        "",
    ));
    let error = problem(&text);
    assert_error(
        &error,
        "lines.nb000000.to",
        "line nb000000 goes from point pb000000 to itself, so it has no length",
    );
    assert!(
        error.elements().iter().any(|e| e == "nb000000"),
        "{error:?}"
    );
}

#[test]
fn an_arc_whose_ends_disagree_about_its_radius_is_the_solves_not_the_gates() {
    // Start 10 from the center, end 20: a hand edit the re-solve mends (D2, arcs).
    let text = document(&fields(
        r#"{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 10, "y": 0, "construction": false}, {"id": "pc000000", "x": 0, "y": 20, "construction": false}"#,
        "",
        "",
        ARC,
        "",
    ));
    assert!(problems(&text).is_empty(), "{:#?}", problems(&text));
}

#[test]
fn sizes_and_places_past_a_kilometre_or_at_zero_are_refused() {
    let far = document(&fields(
        r#"{"id": "pa000000", "x": 1000000.0000000001, "y": -1000000, "construction": false}"#,
        "",
        "",
        "",
        "",
    ));
    assert_error(
        &problem(&far),
        "points.pa000000.x",
        "points.pa000000.x is 1000000.0000000001 mm, farther from the sketch's origin than the 1000000 mm (a kilometre) a point may be",
    );
    for (radius, message) in [
        (
            "0",
            "circle ca000000's radius is 0 mm: arcs and circles have a radius above 1e-7 mm, as the kernel measures them",
        ),
        (
            "-1",
            "circle ca000000's radius is -1 mm: arcs and circles have a radius above 1e-7 mm, as the kernel measures them",
        ),
        (
            "1000001",
            "circle ca000000's radius is 1000001 mm, more than the 1000000 mm (a kilometre) the solver takes",
        ),
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
        assert_error(&problem(&text), "circles.ca000000.radius", message);
    }
}

#[test]
fn a_dimension_out_of_range_is_refused() {
    let cases = [
        (
            r#"{"id": "k0000000", "type": "length", "line": "na000000", "value": 0, "driving": true}"#,
            "a length's value is 0 mm, but it must be above 0 and at most 1000000 mm",
        ),
        (
            r#"{"id": "k0000000", "type": "distance", "a": "pa000000", "b": "pb000000", "value": -1, "driving": false}"#,
            "a reference distance's value is -1 mm, but it must be from 0 to 3000000 mm",
        ),
        (
            r#"{"id": "k0000000", "type": "radius", "curve": "ca000000", "value": 0, "driving": true}"#,
            "a radius's value is 0 mm, but it must be above 0 and at most 1000000 mm",
        ),
        (
            r#"{"id": "k0000000", "type": "diameter", "curve": "ca000000", "value": 2000000.000001, "driving": true}"#,
            "a diameter's value is 2000000.000001 mm, but it must be above 0 and at most 2000000 mm",
        ),
        (
            r#"{"id": "k0000000", "type": "point_line_distance", "point": "pc000000", "line": "na000000", "value": 0, "side": "left", "driving": true}"#,
            "a point_line_distance's value is 0 mm, but it must be above 0 and at most 1000000 mm",
        ),
        (
            r#"{"id": "k0000000", "type": "distance_x", "a": "pa000000", "b": "pb000000", "value": -1000000.000001, "driving": true}"#,
            "a distance_x's value is -1000000.000001 mm, but it must be at most 1000000 mm either way",
        ),
        (
            r#"{"id": "k0000000", "type": "angle", "a": "na000000", "b": "x_axis", "value": -360.000000001, "driving": true}"#,
            "an angle's value is -360.000000001 degrees, but it must be at most 360 either way",
        ),
    ];
    for (constraint, message) in cases {
        assert_error(
            &problem(&constrained(constraint)),
            "constraints.k0000000.value",
            message,
        );
    }
    // The edges of each range pass.
    for constraint in [
        r#"{"id": "k0000000", "type": "length", "line": "na000000", "value": 0.000001, "driving": true}"#,
        r#"{"id": "k0000000", "type": "diameter", "curve": "ca000000", "value": 2000000, "driving": true}"#,
        r#"{"id": "k0000000", "type": "distance_y", "a": "pa000000", "b": "pb000000", "value": -1000000, "driving": true}"#,
        r#"{"id": "k0000000", "type": "distance_x", "a": "pa000000", "b": "pb000000", "value": 0, "driving": true}"#,
        r#"{"id": "k0000000", "type": "angle", "a": "na000000", "b": "x_axis", "value": 360, "driving": true}"#,
    ] {
        assert!(
            problems(&constrained(constraint)).is_empty(),
            "{constraint}"
        );
    }
}

#[test]
fn every_reference_must_name_an_element_its_place_takes() {
    let cases = [
        (
            r#"{"id": "k0000000", "type": "horizontal", "line": "pz000000"}"#,
            "constraints.k0000000.line",
            "constraints.k0000000.line pz000000 isn't the ID of any element of this sketch",
        ),
        (
            r#"{"id": "k0000000", "type": "horizontal", "line": "x_axis"}"#,
            "constraints.k0000000.line",
            "constraints.k0000000.line x_axis is an axis, but a horizontal's line is a line of this sketch",
        ),
        (
            r#"{"id": "k0000000", "type": "length", "line": "ca000000", "value": 1, "driving": true}"#,
            "constraints.k0000000.line",
            "constraints.k0000000.line ca000000 is a circle, but a length's line is a line of this sketch",
        ),
        (
            r#"{"id": "k0000000", "type": "distance", "a": "na000000", "b": "pb000000", "value": 1, "driving": true}"#,
            "constraints.k0000000.a",
            "constraints.k0000000.a na000000 is a line, but a distance's a is a point of this sketch, or origin",
        ),
        (
            r#"{"id": "k0000000", "type": "distance", "a": "x_axis", "b": "pb000000", "value": 1, "driving": true}"#,
            "constraints.k0000000.a",
            "constraints.k0000000.a x_axis is an axis, but a distance's a is a point of this sketch, or origin",
        ),
        (
            r#"{"id": "k0000000", "type": "on", "point": "pc000000", "curve": "origin"}"#,
            "constraints.k0000000.curve",
            "constraints.k0000000.curve origin is the origin, but a on's curve is a line, circle, or arc of this sketch, x_axis, or y_axis",
        ),
        (
            r#"{"id": "k0000000", "type": "radius", "curve": "na000000", "value": 1, "driving": true}"#,
            "constraints.k0000000.curve",
            "constraints.k0000000.curve na000000 is a line, but a radius's curve is a circle or arc of this sketch",
        ),
        (
            r#"{"id": "k0000000", "type": "parallel", "a": "na000000", "b": "ca000000"}"#,
            "constraints.k0000000.b",
            "constraints.k0000000.b ca000000 is a circle, but a parallel's b is a line of this sketch, x_axis, or y_axis",
        ),
        (
            r#"{"id": "k0000000", "type": "midpoint", "point": "pc000000", "line": "aa000000"}"#,
            "constraints.k0000000.line",
            "constraints.k0000000.line aa000000 is a arc, but a midpoint's line is a line of this sketch",
        ),
        (
            r#"{"id": "k0000000", "type": "symmetric", "a": "pb000000", "b": "pc000000", "about": "ca000000"}"#,
            "constraints.k0000000.about",
            "constraints.k0000000.about ca000000 is a circle, but a symmetric's about is a line or point of this sketch, origin, x_axis, or y_axis",
        ),
        (
            r#"{"id": "k0000000", "type": "horizontal", "line": "k0000001"}, {"id": "k0000001", "type": "vertical", "line": "na000000"}"#,
            "constraints.k0000000.line",
            "constraints.k0000000.line k0000001 is a constraint, but a horizontal's line is a line of this sketch",
        ),
        (
            r#"{"id": "k0000000", "type": "tangent_at", "a": "ca000000", "b": "aa000000", "at": "pb000000", "direction": "same"}"#,
            "constraints.k0000000.a",
            "constraints.k0000000.a ca000000 is a circle, but a tangent_at's a is a line or arc of this sketch",
        ),
    ];
    for (constraints, parameter, message) in cases {
        assert_error(&problem(&constrained(constraints)), parameter, message);
    }
}

#[test]
fn a_form_that_needs_two_elements_or_a_fitting_pair_gets_them() {
    let cases = [
        (
            r#"{"id": "k0000000", "type": "parallel", "a": "na000000", "b": "na000000"}"#,
            "constraints.k0000000.b",
            "a parallel's a and b are both na000000: it needs two different elements",
        ),
        (
            r#"{"id": "k0000000", "type": "distance", "a": "pa000000", "b": "pa000000", "value": 1, "driving": true}"#,
            "constraints.k0000000.b",
            "a distance's a and b are both pa000000: it needs two different elements",
        ),
        (
            r#"{"id": "k0000000", "type": "symmetric", "a": "pb000000", "b": "pc000000", "about": "pb000000"}"#,
            "constraints.k0000000.about",
            "a symmetric's about, pb000000, is one of the points it mirrors",
        ),
        (
            r#"{"id": "k0000000", "type": "equal", "a": "na000000", "b": "ca000000"}"#,
            "constraints.k0000000.b",
            "an equal takes two lines, or two circles or arcs",
        ),
        (
            r#"{"id": "k0000000", "type": "tangent", "a": "na000000", "b": "x_axis", "side": "left"}"#,
            "constraints.k0000000.b",
            "a tangent takes a line and a circle or arc, or two circles or arcs: two lines can't be tangent",
        ),
        (
            r#"{"id": "k0000000", "type": "tangent", "a": "na000000", "b": "ca000000", "side": "outside"}"#,
            "constraints.k0000000.side",
            "a tangent's side for these curves is left or right",
        ),
        (
            r#"{"id": "k0000000", "type": "tangent", "a": "aa000000", "b": "ca000000", "side": "left"}"#,
            "constraints.k0000000.side",
            "a tangent's side for these curves is outside or inside",
        ),
        (
            r#"{"id": "k0000000", "type": "tangent_at", "a": "na000000", "b": "aa000000", "at": "pc000000", "direction": "same"}"#,
            "constraints.k0000000.at",
            "a tangent_at's at, point pc000000, isn't an end of its a, na000000",
        ),
    ];
    for (constraint, parameter, message) in cases {
        assert_error(&problem(&constrained(constraint)), parameter, message);
    }
    // An axis is a line a circle can be tangent to.
    assert!(problems(&constrained(
        r#"{"id": "k0000000", "type": "tangent", "a": "x_axis", "b": "ca000000", "side": "left"}"#
    ))
    .is_empty());
}

#[test]
fn a_constraint_on_the_fixed_references_alone_is_refused() {
    // FreeCAD's 3d7871fca5: a dimension between the origin and an axis constrains nothing.
    // And an angle between the axes alone is the one measure decision 5 would find not a
    // number (the axes have no lever), so none reaches a solve.
    for (constraint, what) in [
        (
            r#"{"id": "k0000000", "type": "angle", "a": "x_axis", "b": "y_axis", "value": 90, "driving": true}"#,
            "angle",
        ),
        (
            r#"{"id": "k0000000", "type": "on", "point": "origin", "curve": "x_axis"}"#,
            "on",
        ),
        (
            r#"{"id": "k0000000", "type": "perpendicular", "a": "x_axis", "b": "y_axis"}"#,
            "perpendicular",
        ),
        (
            r#"{"id": "k0000000", "type": "parallel", "a": "x_axis", "b": "y_axis"}"#,
            "parallel",
        ),
    ] {
        assert_error(
            &problem(&constrained(constraint)),
            "constraints.k0000000",
            &format!(
                "this {what} names only the sketch's fixed references, so it constrains nothing in the sketch"
            ),
        );
    }
}

#[test]
fn a_sketch_makes_no_solid_for_a_boolean_or_a_fingerprint() {
    let sketch = r#"{"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}"#;
    let block = r#"{"id": "b0000000", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#;
    let cut = r#"{"id": "c0000000", "type": "cut", "v": 1, "name": "C", "base": "b0000000", "tool": "k7f2qx3m"}"#;
    let text = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{sketch}, {block}, {cut}]}}"
    );
    let error = problem(&text);
    assert_eq!(error.feature(), Some("c0000000"));
    assert_eq!(error.parameter(), Some("tool"));
    assert_eq!(
        error.message(),
        "tool k7f2qx3m is k7f2qx3m (\"S\"), a sketch, which makes no solid for a cut to use"
    );
    let fingerprinted = document(&fields("", "", "", "", "")).replace(
        "\"constraints\": []",
        "\"constraints\": [], \"fingerprint\": {\"volume\": 1, \"area\": 1, \"faces\": 1, \"edges\": 1, \"vertices\": 1}",
    );
    assert_error(
        &problem(&fingerprinted),
        "fingerprint",
        "a sketch has a fingerprint, but it makes no solid to measure",
    );
}

#[test]
fn a_sketch_larger_than_the_solver_takes_is_refused() {
    // 513 points: 1,026 unknowns, past the solver's 1,024.
    let points: Vec<String> = (0..513)
        .map(|i| format!(r#"{{"id": "p{i:07}", "x": {i}, "y": 0, "construction": false}}"#))
        .collect();
    let text = document(&fields(&points.join(", "), "", "", "", ""));
    assert_error(
        &problem(&text),
        "points",
        "the sketch has 1026 unknowns (two for each point, one for each circle, three for each arc), more than the 1024 the sketch solver takes",
    );
    // Advice a single profile past the limit can follow too (round 3's tests lens).
    assert_eq!(
        problem(&text).suggestion(),
        Some(
            "split the sketch into smaller sketches: a single profile past this is more work than the sketch solver takes"
        )
    );
    // 512 points: 1,024 unknowns, exactly the limit.
    let text = document(&fields(&points[..512].join(", "), "", "", "", ""));
    assert!(problems(&text).is_empty());
    // And a circle about one of them: 1,025, one past it.
    let circle = r#"{"id": "c0000000", "center": "p0000000", "radius": 1, "construction": false}"#;
    let text = document(&fields(&points[..512].join(", "), "", circle, "", ""));
    assert_error(
        &problem(&text),
        "points",
        "the sketch has 1025 unknowns (two for each point, one for each circle, three for each arc), more than the 1024 the sketch solver takes",
    );
}

#[test]
fn every_problem_in_a_sketch_is_reported_at_once_in_field_order() {
    let text = document(&fields(
        r#"{"id": "pa000000", "x": 2000000, "y": 0, "construction": false}"#,
        r#"{"id": "na000000", "from": "pa000000", "to": "pa000000", "construction": false}"#,
        "",
        "",
        r#"{"id": "na000000", "type": "length", "line": "na000000", "value": 0, "driving": true}"#,
    ));
    let parameters: Vec<String> = problems(&text)
        .iter()
        .map(|e| e.parameter().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        parameters,
        [
            "points.pa000000.x",
            "lines.na000000.to",
            "constraints.0.id",
            "constraints.0.value"
        ]
    );
}

// Paths to a sketch's members (decision 15, docs/design/m4.1-command-api.md): by ID, but by
// place for a member with no usable ID. Each ID and place below is the fixture's, by hand.

#[test]
fn the_reader_names_a_member_by_its_id_wherever_the_id_is_written() {
    // The id written last, after the field refused: found by looking ahead in the member,
    // not by its place (3, after POINTS's three).
    let fourth = |member: &str| document(&fields(&format!("{POINTS}, {member}"), "", "", "", ""));
    let cases = [
        (
            fourth(r#"{"x": "1", "y": 0, "construction": false, "id": "pd000000"}"#),
            "points.pd000000.x",
            "points.pd000000.x is a string, not a number in millimetres",
        ),
        (
            fourth(r#"{"x": 0, "construction": false, "id": "pd000000"}"#),
            "points.pd000000.y",
            "points.pd000000 has no y",
        ),
        (
            fourth(r#"{"x": 0, "y": 0, "z": 0, "construction": false, "id": "pd000000"}"#),
            "points.pd000000.z",
            "points.pd000000 has a key it can't have: \"z\"",
        ),
        (
            constrained(
                r#"{"type": "length", "line": "na000000", "value": "=w", "driving": true, "id": "k0000000"}"#,
            ),
            "constraints.k0000000.value",
            "constraints.k0000000.value is a string, not a length in millimetres",
        ),
    ];
    for (text, parameter, message) in cases {
        assert_error(&read_error(&text), parameter, message);
    }
}

#[test]
fn a_member_with_no_id_is_named_by_its_place_in_its_own_list() {
    // The second line: lines.1, its place in lines, not in the sketch (4, after the three
    // points and the first line).
    let second = |member: &str| document(&fields(POINTS, &format!("{LINE}, {member}"), "", "", ""));
    let cases = [
        (
            second(r#"{"from": "pa000000", "to": "pc000000", "construction": false}"#),
            "lines.1.id",
            "lines.1 has no id",
        ),
        (second("7"), "lines.1", "lines.1 is a number, not an object"),
        (second("{}"), "lines.1.id", "lines.1 has no id"),
    ];
    for (text, parameter, message) in cases {
        assert_error(&read_error(&text), parameter, message);
    }
}

#[test]
fn a_member_whose_id_isnt_an_id_is_named_by_its_place() {
    // Never by the text its id holds, however it's written, and wherever.
    let second = |member: &str| document(&fields(POINTS, &format!("{LINE}, {member}"), "", "", ""));
    let cases = [
        (
            second(
                r#"{"id": "NB000000", "from": "pa000000", "to": "pc000000", "construction": false}"#,
            ),
            "lines.1.id",
            "lines.1.id \"NB000000\" isn't a sketch element's ID",
        ),
        (
            second(r#"{"from": "pa000000", "to": 7, "construction": false, "id": "nb00000"}"#),
            "lines.1.id",
            "lines.1.id \"nb00000\" isn't a sketch element's ID",
        ),
        (
            second(r#"{"id": 7, "from": "pa000000", "to": "pc000000", "construction": false}"#),
            "lines.1.id",
            "lines.1.id is a number, not a string",
        ),
    ];
    for (text, parameter, message) in cases {
        assert_error(&read_error(&text), parameter, message);
    }
}

#[test]
fn a_repeated_id_names_its_first_holder_alone_and_each_later_one_by_place() {
    // The gate. Circle pb000000 takes a point's ID: the point keeps it, the circle is
    // circles.0. Two constraints k0000000, each of length 0: the first is
    // constraints.k0000000, the second constraints.1, its duplicate refused at
    // constraints.1.id.
    let both = r#"{"id": "k0000000", "type": "length", "line": "na000000", "value": 0, "driving": true}, {"id": "k0000000", "type": "length", "line": "na000000", "value": 0, "driving": true}"#;
    let text = document(&fields(
        POINTS,
        LINE,
        &circle("pb000000", "pa000000", 0.0),
        "",
        both,
    ));
    let all = problems(&text);
    let parameters: Vec<&str> = all.iter().filter_map(|e| e.parameter()).collect();
    assert_eq!(
        parameters,
        [
            "circles.0.id",
            "circles.0.radius",
            "constraints.k0000000.value",
            "constraints.1.id",
            "constraints.1.value"
        ],
        "{all:#?}"
    );
    assert_eq!(
        all[0].message(),
        "id pb000000 is already the ID of point pb000000 (points.pb000000)"
    );
    assert_eq!(
        all[3].message(),
        "id k0000000 is already the ID of constraint k0000000 (constraints.k0000000)"
    );

    // The reader, the same: a later holder refused by place, in its list or another.
    let cases = [
        (
            document(&fields(
                &format!(
                    r#"{POINTS}, {{"id": "pa000000", "x": "1", "y": 0, "construction": false}}"#
                ),
                "",
                "",
                "",
                "",
            )),
            "points.3.x",
            "points.3.x is a string, not a number in millimetres",
        ),
        (
            document(&fields(
                POINTS,
                r#"{"id": "pb000000", "from": "pa000000", "to": 7, "construction": false}"#,
                "",
                "",
                "",
            )),
            "lines.0.to",
            "lines.0.to is a number, not a string",
        ),
        // The first holder, refused, is by its ID.
        (
            document(&fields(
                r#"{"id": "pa000000", "x": 0, "y": 0, "construction": 0}, {"id": "pa000000", "x": 0, "y": 0, "construction": false}"#,
                "",
                "",
                "",
                "",
            )),
            "points.pa000000.construction",
            "points.pa000000.construction is a number, not true or false",
        ),
    ];
    for (text, parameter, message) in cases {
        assert_error(&read_error(&text), parameter, message);
    }
}

#[test]
fn an_array_of_anything_but_a_sketchs_members_is_named_by_place() {
    // Objects with an id where an array holds no sketch members: a fillet's edges, a box's
    // position. Each is named by its place, never by that id.
    let sketch = document(&fields(POINTS, LINE, "", "", ""));
    let with = |feature: &str| format!("{}, {feature}]}}", sketch.strip_suffix("]}").unwrap());
    let cases = [
        (
            with(
                r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "R", "base": "b0000000", "edges": ["b0000000.box.Edge1", {"id": "pa000000"}], "radius": 1}"#,
            ),
            "f0000001",
            "edges.1",
        ),
        (
            with(
                r#"{"id": "b0000000", "type": "box", "v": 1, "name": "B", "position": [0, 0, {"id": "pa000000"}], "length": 1, "width": 1, "height": 1}"#,
            ),
            "b0000000",
            "position.2",
        ),
    ];
    for (text, feature, parameter) in cases {
        let error = read_error(&text);
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{error:?}");
        assert_eq!(error.feature(), Some(feature), "{error:?}");
        assert_eq!(error.parameter(), Some(parameter), "{error:?}");
    }
}

#[test]
fn a_members_path_is_its_own_id_whatever_order_its_list_is_written_in() {
    // k0000001, of length 0, written before k0000000: its path is its own ID as written,
    // and again once the save sorts it after k0000000 (never the ID at its place).
    let text = constrained(
        r#"{"id": "k0000001", "type": "length", "line": "na000000", "value": 0, "driving": true}, {"id": "k0000000", "type": "horizontal", "line": "na000000"}"#,
    );
    assert_error(
        &problem(&text),
        "constraints.k0000001.value",
        "a length's value is 0 mm",
    );
    let saved = write(&read(text.as_bytes()).unwrap()).unwrap();
    assert!(
        saved.find("k0000000").unwrap() < saved.find("k0000001").unwrap(),
        "{saved}"
    );
    assert_error(
        &problem(&saved),
        "constraints.k0000001.value",
        "a length's value is 0 mm",
    );
}

#[test]
fn the_reader_takes_the_lists_in_the_formats_order() {
    // Circles before arcs: an arc holding a circle's ID is its later holder, by place.
    let text = document(&fields(
        POINTS,
        "",
        CIRCLE,
        r#"{"id": "ca000000", "center": "pa000000", "start": 7, "end": "pc000000", "construction": false}"#,
        "",
    ));
    assert_error(
        &read_error(&text),
        "arcs.0.start",
        "arcs.0.start is a number, not a string",
    );
    // Points before constraints, whatever order the file writes the lists in: a
    // constraint holding a point's ID is its later holder, by place.
    let text = document(&format!(
        r#""plane": "xy", "constraints": [{{"id": "pa000000", "type": "length", "line": "na000000", "value": "1", "driving": true}}], "points": [{POINTS}], "lines": [{LINE}], "circles": [], "arcs": []"#
    ));
    assert_error(
        &read_error(&text),
        "constraints.0.value",
        "constraints.0.value is a string, not a length in millimetres",
    );
}

#[test]
fn the_ids_a_sketch_holds_are_its_own_for_its_paths() {
    // A second sketch with a point pa000000, as the first sketch has: it holds the ID in
    // its own sketch, so it's named by it, by the reader and by the gate.
    let second = |x: &str| {
        let one = document(&fields(POINTS, "", "", "", ""));
        let other = format!(
            r#"{{"id": "k7f2qx3n", "type": "sketch", "v": 1, "name": "T", "plane": "xz", "points": [{{"id": "pa000000", "x": {x}, "y": 0, "construction": false}}], "lines": [], "circles": [], "arcs": [], "constraints": []}}"#
        );
        format!("{}, {other}]}}", one.strip_suffix("]}").unwrap())
    };
    let error = read_error(&second("\"1\""));
    assert_eq!(error.feature(), Some("k7f2qx3n"), "{error:?}");
    assert_eq!(error.parameter(), Some("points.pa000000.x"), "{error:?}");
    let all = problems(&second("2000000"));
    let [error] = &all[..] else {
        panic!("{all:#?}")
    };
    assert_eq!(error.feature(), Some("k7f2qx3n"), "{error:?}");
    assert_eq!(error.parameter(), Some("points.pa000000.x"), "{error:?}");
}

#[test]
fn a_document_with_a_sketch_rebuilds_its_solids_and_the_sketch_makes_none() {
    use silico_core::document::{Checked, FeatureId};
    use silico_core::fake::FakeKernel;
    use silico_core::recompute::{FeatureOutcome, Recompute, RunOutcome};
    let block = r#"{"id": "b0000000", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 1, "width": 2, "height": 3}"#;
    let sketch = EXAMPLE
        .split_once("\"features\": [\n")
        .and_then(|(_, rest)| rest.rsplit_once("\n  ]"))
        .map(|(feature, _)| feature)
        .unwrap();
    let text = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"features\": [{sketch}, {block}]}}"
    );
    let engine = Recompute::new(FakeKernel::new(2).unwrap())
        .with_sketches(silico_core::fake::FakeSketches::new());
    let checked = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    let model = match engine.start(checked).wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the recompute didn't finish"),
    };
    let id = |t: &str| FeatureId::new(t).unwrap();
    assert!(matches!(
        model.outcome(&id("k7f2qx3m")),
        Some(FeatureOutcome::Sketch(_))
    ));
    assert!(matches!(
        model.outcome(&id("b0000000")),
        Some(FeatureOutcome::Built { .. })
    ));
    assert_eq!(model.bodies(), [id("b0000000")]);
    assert_eq!(model.errors().count(), 0);
    assert_eq!(model.fingerprint(&id("k7f2qx3m")), None);
    // The kernel was asked for the box alone (and to measure it).
    let calls: Vec<&str> = engine
        .kernel()
        .calls()
        .iter()
        .map(|c| c.op.name())
        .collect();
    // The box made, then measured: summarized, and described in one request.
    assert_eq!(calls, ["MakeBox", "Summarize", "Describe"]);
}
