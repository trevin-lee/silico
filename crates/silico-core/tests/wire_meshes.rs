//! `meshes`' wire types (docs/design/m5.0-app.md, section 7, `meshes`, M5.5; section 2's
//! numbers, A5): the design's request and result byte for byte, read and written back, held
//! to the committed snapshot; a result written from values given by hand, its numbers as A5
//! gives them; a held body's answer; `held` given back as a result wrote it; and every field
//! of a request refused by its path. Expected values are the design's text, the error
//! contract's words, and decimals worked by hand.

use std::path::PathBuf;
use std::sync::Arc;

use silico_core::command::meshes::{Held, MeshElement, MeshedBody, Meshes, MeshesRead};
use silico_core::command::render::AnsweredCamera;
use silico_core::command::wire::meshes::MeshesRequest;
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::command::{ModelChoice, Problems, SessionId};
use silico_core::document::FeatureId;
use silico_core::document::json::{Node, Value, read};
use silico_core::naming::{Maker, Name, OpCode};
use silico_core::recompute::Key;
use silico_protocol::{Camera, Mesh, Projection};

fn contract(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract")
        .join(name)
}

fn committed(name: &str) -> String {
    let path = contract(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn example(name: &str) -> String {
    committed(&format!("commands/examples/meshes/{name}.json"))
}

/// The design's JSON blocks between the headings `from` and `to`, in order.
fn design(from: &str, to: &str) -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/m5.0-app.md");
    let doc = std::fs::read_to_string(&path).unwrap();
    let section = &doc[doc.find(from).unwrap()..doc.find(to).unwrap()];
    section
        .split("```json\n")
        .skip(1)
        .map(|block| block.split("\n```").next().unwrap().to_owned())
        .collect()
}

/// `value` as the API writes it, and a newline, as each example file holds it.
fn written<T: ToWire>(value: &T) -> String {
    write(&value.to_wire(), Layout::Canonical).unwrap() + "\n"
}

/// `value` on one line, as MCP sends it.
fn one_line<T: ToWire>(value: &T) -> String {
    write(&value.to_wire(), Layout::OneLine).unwrap()
}

fn parsed(text: &str) -> Node {
    read(text.as_bytes()).unwrap()
}

fn schema_of(part: &str) -> serde_json::Value {
    let schema: serde_json::Value =
        serde_json::from_str(&committed("commands/meshes.json")).unwrap();
    schema[part].clone()
}

fn holds(schema: &serde_json::Value, text: &str, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{what}: {errors:?}");
}

fn fails(schema: &serde_json::Value, text: &str, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    assert!(!validator.is_valid(&value), "{what} held");
}

fn fid(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn name(text: &str) -> Arc<Name> {
    let maker = |f: &FeatureId| {
        Some(Maker::By(if f.as_str().starts_with('s') {
            OpCode::Sketch
        } else {
            OpCode::Pad
        }))
    };
    Arc::new(Name::parse(text, &maker).unwrap())
}

/// The design's camera: orthographic, 80 mm high, its up the unit vector the example writes.
fn designs_camera() -> Camera {
    Camera {
        projection: Projection::Orthographic { view_height: 80.0 },
        eye: [100.0, -50.0, 95.0],
        target: [30.0, 20.0, 25.0],
        up: [
            -0.408_248_290_463_863_13,
            0.408_248_290_463_863_13,
            0.816_496_580_927_725_8,
        ],
    }
}

const DESIGNS_KEY: &str = "3f9a0c5e1d7b2a4c6e8f0a1b3c5d7e9f1a2b4c6d8e0f1a3b5c7d9e1f2a4b6c8d";

/// The request's every reason refused, each `(parameter, message)`.
fn refused(text: &str) -> Vec<(String, String)> {
    let problems = MeshesRequest::read(&parsed(text)).unwrap_err();
    said(&problems)
}

fn said(problems: &Problems) -> Vec<(String, String)> {
    problems
        .errors()
        .iter()
        .map(|e| {
            (
                e.parameter().unwrap_or("").to_owned(),
                e.message().to_owned(),
            )
        })
        .collect()
}

/// A well-formed request but for `field`, given `value` (JSON text).
fn with(field: &str, value: &str) -> String {
    let fields = [
        ("session", r#""s1""#),
        ("model", r#""current""#),
        ("view", r#""front""#),
        ("camera", "null"),
        ("width", "100"),
        ("height", "100"),
        ("features", "null"),
        ("held", "[]"),
    ];
    let members: Vec<String> = fields
        .iter()
        .map(|(key, v)| format!(r#""{key}": {}"#, if *key == field { value } else { v }))
        .collect();
    format!("{{{}}}", members.join(", "))
}

#[test]
fn the_meshes_examples_are_the_designs_text_and_the_commands_refusals_and_no_other() {
    let blocks = design("### `meshes` (M5.5; decision 6)", "### `sketch` (M5.10)");
    assert_eq!(blocks.len(), 2);
    for (block, name) in blocks.iter().zip(["request", "result"]) {
        assert_eq!(format!("{block}\n"), example(name), "{name}");
    }
    let mut files: Vec<String> = std::fs::read_dir(contract("commands/examples/meshes"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    files.sort();
    assert_eq!(
        files,
        [
            "refused-suppressed.json",
            "refused-too-long-bodies.json",
            "refused-too-long-body.json",
            "request.json",
            "result.json",
        ]
    );
    // Each refusal holds to the error contract.
    let envelope: serde_json::Value =
        serde_json::from_str(&committed("commands/envelope.json")).unwrap();
    let error = serde_json::json!({"$defs": envelope["$defs"], "$ref": "#/$defs/Error"});
    for name in [
        "refused-suppressed",
        "refused-too-long-bodies",
        "refused-too-long-body",
    ] {
        holds(&error, &example(name), name);
    }
}

#[test]
fn the_designs_request_is_read_exactly_written_back_and_holds_to_its_schema() {
    let text = example("request");
    let request = MeshesRequest::read(&parsed(&text)).unwrap();
    assert_eq!(
        request,
        MeshesRequest {
            session: SessionId::parse("s1").unwrap(),
            model: ModelChoice::Published,
            meshes: Meshes {
                view: None,
                camera: Some(designs_camera()),
                width: 1600,
                height: 1000,
                features: None,
                held: vec![],
            },
        }
    );
    // Its up read from its digits, each the f64 nearest.
    let camera = request.meshes.camera.as_ref().unwrap();
    assert_eq!(camera.up[2].to_bits(), 0.816_496_580_927_725_8f64.to_bits());
    assert_eq!(written(&request), text);
    holds(&schema_of("request"), &text, "the design's");
}

#[test]
fn a_request_from_each_cube_edge_is_read_as_that_view_and_holds_to_its_schema() {
    use silico_core::views::NamedView;
    let schema = schema_of("request");
    for (name, view) in [
        ("top_front", NamedView::TopFront),
        ("top_back", NamedView::TopBack),
        ("top_left", NamedView::TopLeft),
        ("top_right", NamedView::TopRight),
        ("bottom_front", NamedView::BottomFront),
        ("bottom_back", NamedView::BottomBack),
        ("bottom_left", NamedView::BottomLeft),
        ("bottom_right", NamedView::BottomRight),
        ("front_left", NamedView::FrontLeft),
        ("front_right", NamedView::FrontRight),
        ("back_left", NamedView::BackLeft),
        ("back_right", NamedView::BackRight),
    ] {
        let text = format!(
            "{{\n  \"session\": \"s1\",\n  \"model\": \"current\",\n  \"view\": \"{name}\",\n  \"camera\": null,\n  \"width\": 100,\n  \"height\": 100,\n  \"features\": null,\n  \"held\": []\n}}\n"
        );
        let request = MeshesRequest::read(&parsed(&text)).unwrap();
        assert_eq!(request.meshes.view, Some(view), "{name}");
        assert_eq!(written(&request), text, "{name}");
        holds(&schema, &text, name);
    }
    // Its faces the other way round, a word no view is: refused, naming `view`.
    let text = r#"{"session": "s1", "model": "current", "view": "front_top", "camera": null, "width": 100, "height": 100, "features": null, "held": []}"#;
    let refused = MeshesRequest::read(&parsed(text)).unwrap_err();
    let fields: Vec<Option<&str>> = refused.errors().iter().map(|e| e.parameter()).collect();
    assert_eq!(fields, [Some("view")]);
    assert_eq!(refused.errors()[0].candidates().len(), 26);
}

#[test]
fn a_request_naming_features_and_held_meshes_is_read_as_it_says_and_written_back() {
    let key = "00ff".repeat(16);
    let text = format!(
        r#"{{
  "session": "s3",
  "model": "current",
  "view": "top_front_right",
  "camera": null,
  "width": 1,
  "height": 4096,
  "features": ["k7f2qx3m", "m3tq8v0z"],
  "held": [
    {{"key": "{key}", "deflection": 0.03125}},
    {{"key": "{DESIGNS_KEY}", "deflection": 0.004630340016196244}}
  ]
}}
"#
    );
    let request = MeshesRequest::read(&parsed(&text)).unwrap();
    let m = &request.meshes;
    assert_eq!(request.model, ModelChoice::Current);
    assert_eq!(m.view, Some(silico_core::views::NamedView::TopFrontRight));
    assert_eq!((m.width, m.height), (1, 4096));
    assert_eq!(m.features, Some(vec![fid("k7f2qx3m"), fid("m3tq8v0z")]));
    // Each key's 32 bytes, by hand: 00 ff repeated.
    assert_eq!(m.held.len(), 2);
    assert_eq!(m.held[0].key, Key::parse(&key).unwrap());
    assert_eq!(m.held[0].key.to_string(), key);
    assert_eq!(m.held[0].deflection.to_bits(), 0.03125f64.to_bits());
    assert_eq!(m.held[1].key.to_string(), DESIGNS_KEY);
    assert_eq!(
        m.held[1].deflection.to_bits(),
        0.004_630_340_016_196_244f64.to_bits()
    );
    assert_eq!(written(&request), text);
    holds(&schema_of("request"), &text, "features and held");
    // The schema refuses what the reader does: a key not of 64 lowercase hex digits, a
    // deflection not positive, and a field render has but meshes hasn't.
    for (field, value) in [
        (
            "held",
            format!(r#"[{{"key": "{}", "deflection": 1}}]"#, "A".repeat(64)),
        ),
        ("held", format!(r#"[{{"key": "{key}", "deflection": 0}}]"#)),
        (
            "held",
            format!(r#"[{{"key": "{key}", "deflection": 1, "level": 1}}]"#),
        ),
    ] {
        fails(&schema_of("request"), &with(field, &value), &value);
    }
    fails(
        &schema_of("request"),
        &with("held", r#"[], "highlight": []"#),
        "highlight",
    );
}

/// The design's example body, by its values: its arrays cut to one triangle, one edge, and
/// two vertices.
fn designs_body() -> MeshedBody {
    MeshedBody {
        feature: fid("f0000001"),
        body: Some(1),
        key: Key::parse(DESIGNS_KEY).unwrap(),
        // 80 mm over 1,000 pixels: a half pixel of 0.04, whose power of two is 2⁻⁵.
        level: 0.03125,
        deflection: 0.03125,
        bounds: Some([[0.0, 0.0, 0.0], [60.0, 40.0, 50.0]]),
        serves: [0.03125, 0.0625],
        held: false,
        mesh: Some(Arc::new(Mesh {
            positions: vec![[0.0, 0.0, 0.0], [60.0, 0.0, 0.0], [60.0, 40.0, 0.0]],
            normals: vec![[0.0, 0.0, -1.0]; 3],
            triangles: vec![[0, 2, 1]],
            face_triangles: vec![1],
            valid: None,
            edge_points: vec![[0.0, 0.0, 0.0], [60.0, 0.0, 0.0]],
            edge_point_counts: vec![2],
            vertex_points: vec![[0.0, 0.0, 0.0], [60.0, 0.0, 0.0]],
        })),
        faces: vec![MeshElement {
            name: name("p0000001.pad.Face(G s0000001.sketch.Edge.na000001)"),
            stable: true,
            count: Some(1),
        }],
        edges: vec![MeshElement {
            name: name("p0000001.pad.Edge(M s0000001.sketch.Edge.na000001)"),
            stable: true,
            count: Some(2),
        }],
        vertices: ["pa000001", "pa000002"]
            .map(|point| MeshElement {
                name: name(&format!(
                    "p0000001.pad.Vertex(M s0000001.sketch.Vertex.{point})"
                )),
                stable: true,
                count: None,
            })
            .to_vec(),
    }
}

fn read_of(bodies: Vec<MeshedBody>) -> MeshesRead {
    MeshesRead {
        camera: AnsweredCamera::of(&designs_camera()).unwrap(),
        bodies,
        coarser: "height",
    }
}

#[test]
fn the_designs_result_is_written_byte_for_byte_from_its_values_and_holds_to_its_schema() {
    let text = example("result");
    assert_eq!(written(&read_of(vec![designs_body()])), text);
    holds(&schema_of("result"), &text, "the design's");
    // Not a request, and not a result with a field left out.
    fails(&schema_of("result"), &example("request"), "a request");
    let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
    value["bodies"][0].as_object_mut().unwrap().remove("serves");
    fails(&schema_of("result"), &value.to_string(), "no serves");
}

#[test]
fn a_bodys_numbers_are_as_a5_gives_them() {
    // Coordinates as a coordinate is written but -0 as 0, never rounded to a length; normals
    // the shortest decimal of their f32; levels as coordinates. Each decimal by hand:
    // 0.1 + 0.2 is 0.30000000000000004 in f64; 1e-12 written out in full; the f32 nearest
    // 0.1 is 0.100000001490116..., whose shortest round trip is 0.1, and 1/3's is 0.33333334;
    // 2⁻²⁰ is 0.00000095367431640625 exactly. A vertex's point is written as an edge's
    // point is: -0 as 0, -1e-12 in full.
    // 2⁻²⁰, exactly.
    let level = 0.000_000_953_674_316_406_25;
    let body = MeshedBody {
        feature: fid("k7f2qx3m"),
        body: None,
        key: Key::parse(&"0".repeat(64)).unwrap(),
        level,
        deflection: 0.004_630_340_016_196_244,
        bounds: Some([[-0.0, -1.5, 0.1 + 0.2], [1e-12, 0.0, 12_345.678_901_234_5]]),
        serves: [0.0, 2.0 * 0.003_906_25],
        held: false,
        mesh: Some(Arc::new(Mesh {
            positions: vec![[-0.0, 0.1 + 0.2, 1e-12]],
            normals: vec![[-0.0, 0.1, 1.0 / 3.0]],
            triangles: vec![[0, 4_294_967_295, 7]],
            face_triangles: vec![1],
            valid: None,
            edge_points: vec![[-0.0, -0.0, -2.5]],
            edge_point_counts: vec![1],
            vertex_points: vec![[-0.0, 0.1 + 0.2, -1e-12]],
        })),
        faces: vec![MeshElement {
            name: name("k7f2qx3m.pad.Face1"),
            stable: false,
            count: Some(1),
        }],
        edges: vec![MeshElement {
            name: name("k7f2qx3m.pad.Edge1"),
            stable: false,
            count: Some(1),
        }],
        vertices: vec![MeshElement {
            name: name("k7f2qx3m.pad.Vertex1"),
            stable: false,
            count: None,
        }],
    };
    assert_eq!(
        one_line(&body),
        format!(
            r#"{{"feature": "k7f2qx3m", "body": null, "key": "{}", "level": 0.00000095367431640625, "deflection": 0.004630340016196244, "box": [[0, -1.5, 0.30000000000000004], [0.000000000001, 0, 12345.6789012345]], "serves": [0, 0.0078125], "held": false, "positions": [[0, 0.30000000000000004, 0.000000000001]], "normals": [[0, 0.1, 0.33333334]], "triangles": [[0, 4294967295, 7]], "faces": [{{"name": "k7f2qx3m.pad.Face1", "stable": false, "triangles": 1}}], "edge_points": [[0, 0, -2.5]], "edges": [{{"name": "k7f2qx3m.pad.Edge1", "stable": false, "points": 1}}], "vertex_points": [[0, 0.30000000000000004, -0.000000000001]], "vertices": [{{"name": "k7f2qx3m.pad.Vertex1", "stable": false}}]}}"#,
            "0".repeat(64)
        )
    );
    // A body with no box: null.
    let unmeasured = MeshedBody {
        bounds: None,
        ..body
    };
    let text = one_line(&read_of(vec![unmeasured]));
    assert!(text.contains(r#""box": null"#), "{text}");
    holds(&schema_of("result"), &text, "no box");
}

#[test]
fn a_held_body_is_named_with_no_geometry_its_counts_null() {
    let held = MeshedBody {
        held: true,
        mesh: None,
        faces: vec![MeshElement {
            count: None,
            ..designs_body().faces[0].clone()
        }],
        edges: vec![MeshElement {
            count: None,
            ..designs_body().edges[0].clone()
        }],
        ..designs_body()
    };
    let text = written(&read_of(vec![held]));
    assert_eq!(
        text,
        format!(
            r#"{{
  "camera": {{"projection": "orthographic", "eye": [100, -50, 95], "target": [30, 20, 25], "up": [-0.40824829046386313, 0.40824829046386313, 0.8164965809277258], "view_height": 80, "field_of_view": null}},
  "bodies": [
    {{
      "feature": "f0000001",
      "body": 1,
      "key": "{DESIGNS_KEY}",
      "level": 0.03125,
      "deflection": 0.03125,
      "box": [[0, 0, 0], [60, 40, 50]],
      "serves": [0.03125, 0.0625],
      "held": true,
      "positions": [],
      "normals": [],
      "triangles": [],
      "faces": [
        {{"name": "p0000001.pad.Face(G s0000001.sketch.Edge.na000001)", "stable": true, "triangles": null}}
      ],
      "edge_points": [],
      "edges": [
        {{"name": "p0000001.pad.Edge(M s0000001.sketch.Edge.na000001)", "stable": true, "points": null}}
      ],
      "vertex_points": [],
      "vertices": [
        {{"name": "p0000001.pad.Vertex(M s0000001.sketch.Vertex.pa000001)", "stable": true}},
        {{"name": "p0000001.pad.Vertex(M s0000001.sketch.Vertex.pa000002)", "stable": true}}
      ]
    }}
  ]
}}
"#
        )
    );
    holds(&schema_of("result"), &text, "held");
}

/// The members of `node`, an object, by key.
fn member<'a>(node: &'a Node, key: &str) -> &'a Node {
    match &node.value {
        Value::Object(members) => &members.iter().find(|m| m.key == key).unwrap().value,
        _ => panic!("not an object"),
    }
}

fn item(node: &Node, at: usize) -> &Node {
    match &node.value {
        Value::Array(items) => &items[at],
        _ => panic!("not an array"),
    }
}

#[test]
fn held_given_back_as_a_result_wrote_it_reads_as_the_bodys_key_and_deflection() {
    // A floor and a level of awkward decimals: each written, then read back from the text
    // the result holds (the reader's own number text, never through another float), equal
    // to the bit.
    for deflection in [
        0.004_630_340_016_196_244,
        0.000_000_953_674_316_406_25,
        0.1 + 0.2,
        f64::MIN_POSITIVE,
        1e300,
    ] {
        let body = MeshedBody {
            deflection,
            ..designs_body()
        };
        let text = one_line(&read_of(vec![body.clone()]));
        let result = parsed(&text);
        let written = item(member(&result, "bodies"), 0);
        let number = |key: &str| match &member(written, key).value {
            Value::Number(text) | Value::String(text) => text.clone(),
            _ => panic!("{key}"),
        };
        let request = with(
            "held",
            &format!(
                r#"[{{"key": "{}", "deflection": {}}}]"#,
                number("key"),
                number("deflection")
            ),
        );
        let read = MeshesRequest::read(&parsed(&request)).unwrap();
        assert_eq!(read.meshes.held.len(), 1);
        let Held {
            key,
            deflection: back,
        } = read.meshes.held[0];
        assert_eq!(key, body.key, "{deflection}");
        assert_eq!(back.to_bits(), deflection.to_bits(), "{deflection}");
    }
}

#[test]
fn each_field_of_a_request_is_refused_by_its_path_saying_what_it_must_be() {
    let key = "0".repeat(64);
    let cases: Vec<(String, Vec<(&str, String)>)> = vec![
        (
            r#"{"session": "s1", "model": "current", "view": "front", "camera": null, "width": 100, "height": 100, "features": null}"#.to_owned(),
            vec![("held", "held is missing".to_owned())],
        ),
        (
            with("held", "null"),
            vec![("held", "held must be a list, got null".to_owned())],
        ),
        (
            with("held", "[1]"),
            vec![("held.0", "held.0 must be an object, got a number".to_owned())],
        ),
        (
            with("held", r#"[{"key": "abc", "deflection": 1}]"#),
            vec![(
                "held.0.key",
                r#"held.0.key must be a mesh's key, 64 lowercase hex digits, got "abc""#
                    .to_owned(),
            )],
        ),
        (
            // 63 digits, 65, an upper-case digit, a letter past f, a number.
            with(
                "held",
                &format!(
                    r#"[{{"key": "{short}", "deflection": 1}}, {{"key": "{key}0", "deflection": 1}}, {{"key": "A{short}", "deflection": 1}}, {{"key": "g{short}", "deflection": 1}}, {{"key": 7, "deflection": 1}}]"#,
                    short = &key[1..],
                ),
            ),
            vec![
                ("held.0.key", String::new()),
                ("held.1.key", String::new()),
                ("held.2.key", String::new()),
                ("held.3.key", String::new()),
                (
                    "held.4.key",
                    "held.4.key must be a mesh's key, 64 lowercase hex digits, got 7".to_owned(),
                ),
            ],
        ),
        (
            with(
                "held",
                &format!(
                    r#"[{{"key": "{key}", "deflection": 0}}, {{"key": "{key}", "deflection": -0}}, {{"key": "{key}", "deflection": -1}}, {{"key": "{key}", "deflection": 1e999}}, {{"key": "{key}", "deflection": "0.1"}}, {{"key": "{key}", "deflection": null}}]"#
                ),
            ),
            vec![
                ("held.0.deflection", "held.0.deflection must be a positive number, got 0".to_owned()),
                ("held.1.deflection", "held.1.deflection must be a positive number, got -0".to_owned()),
                ("held.2.deflection", "held.2.deflection must be a positive number, got -1".to_owned()),
                ("held.3.deflection", "held.3.deflection must be a positive number, got 1e999".to_owned()),
                ("held.4.deflection", r#"held.4.deflection must be a positive number, got "0.1""#.to_owned()),
                ("held.5.deflection", "held.5.deflection must be a positive number, got null".to_owned()),
            ],
        ),
        (
            with("held", &format!(r#"[{{"key": "{key}"}}]"#)),
            vec![("held.0.deflection", "held.0.deflection is missing".to_owned())],
        ),
        (
            with("held", &format!(r#"[{{"key": "{key}", "deflection": 1, "level": 1}}]"#)),
            vec![("held.0.level", "held.0.level isn't a field of this request".to_owned())],
        ),
        (
            with("features", r#""k7f2qx3m""#),
            vec![(
                "features",
                r#"features must be a list of features' IDs or null, got "k7f2qx3m""#.to_owned(),
            )],
        ),
        (
            with("features", r#"["k7f2qx3m", "K7F2QX3M", 1]"#),
            vec![
                ("features.1", r#"features.1 must be a feature's ID, got "K7F2QX3M""#.to_owned()),
                ("features.2", "features.2 must be a feature's ID, got 1".to_owned()),
            ],
        ),
        (
            with("width", "-1"),
            vec![(
                "width",
                "width must be a whole number from 0 to 4294967295, got -1".to_owned(),
            )],
        ),
        (
            with("height", "1.5"),
            vec![(
                "height",
                "height must be a whole number from 0 to 4294967295, got 1.5".to_owned(),
            )],
        ),
        (
            with("width", "4294967296"),
            vec![(
                "width",
                "width must be a whole number from 0 to 4294967295, got 4294967296".to_owned(),
            )],
        ),
        (
            with("view", r#""sideways""#),
            vec![("view", String::new())],
        ),
        (
            with("model", r#""latest""#),
            vec![("model", "model must be current or published, got latest".to_owned())],
        ),
        (
            with("session", r#""1""#),
            vec![("session", r#"session must be a session's ID, s1, s2, ..., got "1""#.to_owned())],
        ),
        (
            with(
                "camera",
                r#"{"projection": "orthographic", "eye": [0, -1, 0], "target": [0, 0, 0], "up": [0, 0, 1], "view_height": null, "field_of_view": null}"#,
            ),
            vec![(
                "camera.view_height",
                "camera.view_height must be given for an orthographic camera, got null".to_owned(),
            )],
        ),
        // Render's fields aren't meshes'.
        (
            with("held", r#"[], "highlight": [], "to": null, "replace": false"#),
            vec![
                ("highlight", "highlight isn't a field of this request".to_owned()),
                ("to", "to isn't a field of this request".to_owned()),
                ("replace", "replace isn't a field of this request".to_owned()),
            ],
        ),
    ];
    for (text, want) in cases {
        let got = refused(&text);
        assert_eq!(got.len(), want.len(), "{text}: {got:?}");
        for ((parameter, message), (want_parameter, want_message)) in got.iter().zip(&want) {
            assert_eq!(parameter, want_parameter, "{text}");
            if !want_message.is_empty() {
                assert_eq!(message, want_message, "{text}");
            }
        }
    }
    // The size isn't the reader's to refuse: 0 and 4097 read, for the command to refuse,
    // naming each (session_meshes.rs).
    let read = MeshesRequest::read(&parsed(&with("width", "0"))).unwrap();
    assert_eq!(read.meshes.width, 0);
    let read = MeshesRequest::read(&parsed(&with("height", "4097"))).unwrap();
    assert_eq!(read.meshes.height, 4097);
}

#[test]
fn a_deflection_is_read_correctly_rounded_from_any_number_form() {
    // Each the f64 nearest its decimal: 2⁻⁵ however it's written, and one past an f64's
    // digits rounded to it.
    let key = "0".repeat(64);
    for form in [
        "0.03125",
        "3.125e-2",
        "3.125E-2",
        "0.0312500000000000000000000001",
        "312.5e-4",
    ] {
        let text = with(
            "held",
            &format!(r#"[{{"key": "{key}", "deflection": {form}}}]"#),
        );
        let read = MeshesRequest::read(&parsed(&text)).unwrap();
        assert_eq!(
            read.meshes.held[0].deflection.to_bits(),
            0.03125f64.to_bits(),
            "{form}"
        );
    }
    // The smallest subnormal is positive: taken. Under it, rounded to 0: refused.
    let text = with(
        "held",
        &format!(r#"[{{"key": "{key}", "deflection": 5e-324}}]"#),
    );
    let read = MeshesRequest::read(&parsed(&text)).unwrap();
    assert_eq!(read.meshes.held[0].deflection.to_bits(), 1);
    let text = with(
        "held",
        &format!(r#"[{{"key": "{key}", "deflection": 1e-400}}]"#),
    );
    assert_eq!(
        refused(&text),
        [(
            "held.0.deflection".to_owned(),
            "held.0.deflection must be a positive number, got 1e-400".to_owned()
        )]
    );
}

#[test]
fn a_hundred_thousand_held_meshes_are_read_and_their_refusals_listed_to_a_hundred() {
    // `held` has no bound but the request's size: 100,000 read, each as given.
    let entries: Vec<String> = (0..100_000u32)
        .map(|i| format!(r#"{{"key": "{:064x}", "deflection": {}}}"#, i, i + 1))
        .collect();
    let text = with("held", &format!("[{}]", entries.join(", ")));
    let read = MeshesRequest::read(&parsed(&text)).unwrap();
    assert_eq!(read.meshes.held.len(), 100_000);
    assert_eq!(
        read.meshes.held[99_999].key.to_string(),
        format!("{:064x}", 99_999)
    );
    assert_eq!(read.meshes.held[99_999].deflection, 100_000.0);
    // 2,000 refusals, two a mesh: the first hundred listed, every one counted.
    let bad: Vec<&str> = (0..1000)
        .map(|_| r#"{"key": "x", "deflection": 0}"#)
        .collect();
    let problems =
        MeshesRequest::read(&parsed(&with("held", &format!("[{}]", bad.join(", "))))).unwrap_err();
    assert_eq!(problems.errors().len(), 100);
    assert_eq!(problems.total(), 2000);
    assert_eq!(
        problems.errors()[99].parameter(),
        Some("held.49.deflection")
    );
}

#[test]
fn a_held_mesh_given_a_key_twice_is_refused_by_the_reader() {
    // Silico's reader refuses a key given twice, wherever it is.
    let key = "0".repeat(64);
    let text = with(
        "held",
        &format!(r#"[{{"key": "{key}", "key": "{key}", "deflection": 1}}]"#),
    );
    assert!(read(text.as_bytes()).is_err());
}
