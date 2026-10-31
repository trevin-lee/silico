//! M5.9: sketches on faces in the document, the gate, the readers of a feature's stored
//! names, and the rebuild on the fake kernel (docs/ports/attacher.md, *How a rebuild places
//! it (M5.9)*). The canonical text and every message are written out by hand from the note
//! and the format's rules (docs/design/m2.1-document-format.md, D7); the real kernel's
//! frames and solids are silico-kernel's tests/face_sketches.rs.

use super::model::WAIT;
use super::sweeps;

use silico_core::ErrorCode;
use silico_core::command::outline;
use silico_core::document::{
    AxisToken, ElementName, FacePlane, Operation, Plane, chain, read, suppression, validate, write,
};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::timeline::{Excepted, timeline};
use sweeps::*;

/// A face plane's object, as the file writes it.
fn face(name: &str, x: &str, reversed: bool) -> String {
    format!(r#"{{"face": "{name}", "x": "{x}", "reversed": {reversed}}}"#)
}

/// A 4 × 2 rectangle from (1, 1) on the face plane `plane` (its text), sketch `id`.
fn on_face(id: &str, plane: &str) -> String {
    rectangle(id, "xy", [1.0, 1.0], 4.0, 2.0)
        .replace(r#""plane": "xy""#, &format!(r#""plane": {plane}"#))
}

/// The 10 mm box `b0000001` at the origin.
fn cube() -> String {
    box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0)
}

/// The canonical text of a box and an empty sketch on its top face, written by hand: the
/// sketch holds an object, its plane, so it's written a member to a line (D7); the plane's
/// object holds no object, so it's on one line, its keys in the format's order.
const DOCUMENT: &str = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "b0000001", "type": "box", "v": 1, "name": "Box", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10},
    {
      "id": "s0000001",
      "type": "sketch",
      "v": 1,
      "name": "Top",
      "plane": {"face": "b0000001.box.Face6", "x": "-y", "reversed": true},
      "points": [],
      "lines": [],
      "circles": [],
      "arcs": [],
      "constraints": []
    }
  ]
}
"#;

#[test]
fn a_sketch_on_a_face_reads_as_its_three_values_and_writes_back_byte_for_byte() {
    let document = read(DOCUMENT.as_bytes()).unwrap();
    let Operation::Sketch(sketch) = &document.features[1].operation else {
        panic!("{:?}", document.features[1]);
    };
    assert_eq!(
        sketch.plane,
        Plane::Face(FacePlane {
            face: ElementName::new("b0000001.box.Face6").unwrap(),
            x: AxisToken::MinusY,
            reversed: true,
        })
    );
    assert_eq!(write(&document).unwrap(), DOCUMENT);
    // Its keys in another order read the same, and write back in the format's.
    let shuffled = DOCUMENT.replace(
        r#"{"face": "b0000001.box.Face6", "x": "-y", "reversed": true}"#,
        r#"{"reversed": true, "x": "-y", "face": "b0000001.box.Face6"}"#,
    );
    assert_eq!(
        write(&read(shuffled.as_bytes()).unwrap()).unwrap(),
        DOCUMENT
    );
    // Every token reads back as itself, and `false` is written too.
    for token in AxisToken::ALL {
        let text = DOCUMENT
            .replace(r#""x": "-y""#, &format!(r#""x": "{}""#, token.word()))
            .replace(r#""reversed": true"#, r#""reversed": false"#);
        assert_eq!(
            write(&read(text.as_bytes()).unwrap()).unwrap(),
            text,
            "{token}"
        );
    }
}

#[test]
fn each_malformed_face_plane_is_refused_naming_its_parameter() {
    let plane = r#"{"face": "b0000001.box.Face6", "x": "-y", "reversed": true}"#;
    let cases: [(&str, &str, &str); 10] = [
        (
            "7",
            "plane",
            "plane is a number, not an origin plane's word (xy, xz, or yz) or a face plane",
        ),
        (
            "[]",
            "plane",
            "plane is an array, not an origin plane's word (xy, xz, or yz) or a face plane",
        ),
        (
            r#"{"x": "-y", "reversed": true}"#,
            "plane.face",
            "plane has no face",
        ),
        (
            r#"{"face": "b0000001.box.Face6", "reversed": true}"#,
            "plane.x",
            "plane has no x",
        ),
        (
            r#"{"face": "b0000001.box.Face6", "x": "-y"}"#,
            "plane.reversed",
            "plane has no reversed",
        ),
        (
            r#"{"face": "b0000001.box.Face6", "x": "-y", "reversed": true, "y": "+z"}"#,
            "plane.y",
            "plane has a key it can't have: \"y\"",
        ),
        (
            r#"{"face": 6, "x": "-y", "reversed": true}"#,
            "plane.face",
            "plane.face is a number, not a face's name, a string",
        ),
        (
            r#"{"face": "", "x": "-y", "reversed": true}"#,
            "plane.face",
            "plane.face is empty",
        ),
        (
            r#"{"face": "b0000001.box.Face6", "x": "y", "reversed": true}"#,
            "plane.x",
            "plane.x \"y\" isn't an axis",
        ),
        (
            r#"{"face": "b0000001.box.Face6", "x": "-y", "reversed": "yes"}"#,
            "plane.reversed",
            "plane.reversed is a string, not true or false",
        ),
    ];
    for (form, parameter, message) in cases {
        let text = DOCUMENT.replace(plane, form);
        let error = read(text.as_bytes()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{form}");
        assert_eq!(error.feature(), Some("s0000001"), "{form}");
        assert_eq!(error.parameter(), Some(parameter), "{form}");
        assert!(
            error.message().starts_with(&format!("{message}, at line ")),
            "{form}: {}",
            error.message()
        );
    }
    // A key twice is the reader's to refuse, as any object's.
    let twice = DOCUMENT.replace(
        plane,
        r#"{"face": "b0000001.box.Face6", "x": "-y", "x": "+y", "reversed": true}"#,
    );
    assert_eq!(
        read(twice.as_bytes()).unwrap_err().code(),
        ErrorCode::InvalidInput
    );
    // A word that isn't a plane still says so.
    let word = DOCUMENT.replace(plane, r#""face""#);
    assert!(
        read(word.as_bytes())
            .unwrap_err()
            .message()
            .starts_with("plane \"face\" isn't a sketch plane")
    );
}

#[test]
fn the_gate_refuses_a_face_that_isnt_a_faces_stable_name_of_a_feature_before_the_sketch() {
    let fuse = r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "Both", "base": "b0000001", "tool": "b0000002"}"#;
    let second = box_at("b0000002", [20.0, 0.0, 0.0], 1.0, 1.0, 1.0);
    let face_advice = "write the face's name as Silico names it (feature.type.Face...), of a feature before this sketch";
    // Not a name.
    let e = problem(&[
        &cube(),
        &on_face("s0000001", &face("nonsense", "+x", false)),
    ]);
    assert_eq!(e.parameter(), Some("plane.face"));
    assert_eq!(e.feature(), Some("s0000001"));
    assert!(
        e.message()
            .starts_with("plane.face, \"nonsense\", isn't a face's name: "),
        "{}",
        e.message()
    );
    assert_eq!(e.suggestion(), Some(face_advice));
    // An edge.
    let e = problem(&[
        &cube(),
        &on_face("s0000001", &face("b0000001.box.Edge3", "+x", false)),
    ]);
    assert_refused(
        &e,
        "s0000001",
        "plane.face",
        "plane.face, \"b0000001.box.Edge3\", names an edge, not a face",
        face_advice,
    );
    // A vertex.
    let e = problem(&[
        &cube(),
        &on_face("s0000001", &face("b0000001.box.Vertex1", "+x", false)),
    ]);
    assert_refused(
        &e,
        "s0000001",
        "plane.face",
        "plane.face, \"b0000001.box.Vertex1\", names a vertex, not a face",
        face_advice,
    );
    // An operation's element by its number: unstable (N3).
    let e = problem(&[
        &cube(),
        &second,
        fuse,
        &on_face("s0000001", &face("f0000001.fuse.Face3", "+x", false)),
    ]);
    assert_refused(
        &e,
        "s0000001",
        "plane.face",
        "plane.face, \"f0000001.fuse.Face3\", names a face by a name that doesn't say which: a duplicate (~n) or an operation's element by its number",
        "name the face by how it came about, as Silico names it now",
    );
    // A duplicate: unstable too.
    let e = problem(&[
        &cube(),
        &on_face("s0000001", &face("b0000001.box.Face6~1", "+x", false)),
    ]);
    assert_eq!(e.parameter(), Some("plane.face"));
    assert!(
        e.message().contains("by a name that doesn't say which"),
        "{}",
        e.message()
    );
    // A feature after the sketch: a cycle, which the name can't be read before.
    let e = problem(&[
        &on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
        &cube(),
    ]);
    assert_eq!(e.parameter(), Some("plane.face"));
    assert!(
        e.message().starts_with(
            "plane.face, \"b0000001.box.Face6\", isn't a face's name: no feature b0000001 before this one"
        ),
        "{}",
        e.message()
    );
    // Itself: no feature before it.
    let e = problem(&[
        &cube(),
        &on_face("s0000001", &face("s0000001.sketch.Face1", "+x", false)),
    ]);
    assert_eq!(e.parameter(), Some("plane.face"));
    // A box named as a cut made it.
    let e = problem(&[
        &cube(),
        &on_face("s0000001", &face("b0000001.cut.Face3", "+x", false)),
    ]);
    assert_refused(
        &e,
        "s0000001",
        "plane.face",
        "plane.face, \"b0000001.cut.Face3\", names Face3 of a cut, but b0000001 is a box: its elements are another's",
        "name the face as b0000001's solid names it now (a box's elements are `b0000001.box.` and their numbers)",
    );
    // A face of a feature before it, stably named: taken.
    checked(&[
        &cube(),
        &on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
    ])
    .unwrap();
}

#[test]
fn a_fillets_edges_are_refused_by_the_gate_as_before() {
    // The edges' messages are the face's code shared: they read as they did.
    let fillet = r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "F", "base": "b0000001", "edges": ["b0000001.box.Face6", "b0000001.box.Edge1", "b0000001.box.Edge1"], "radius": 1}"#;
    let invalid = checked(&[&cube(), fillet]).unwrap_err();
    let messages: Vec<&str> = invalid.errors().iter().map(|e| e.message()).collect();
    assert_eq!(
        messages,
        [
            "edges.0, \"b0000001.box.Face6\", names a face, not an edge",
            "edges.2 names the edge edges.1 names",
        ]
    );
    assert_eq!(
        invalid.errors()[0].suggestion(),
        Some(
            "write the edge's name as Silico names it (feature.type.Edge(...)), of a feature before this one"
        )
    );
}

/// The box, a sketch on its top face, and a pad of it on the box.
fn padded() -> Vec<String> {
    vec![
        cube(),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
        pad("p0000001", Some("b0000001"), "s0000001", &one_side("3")),
    ]
}

fn strs(features: &[String]) -> Vec<&str> {
    features.iter().map(String::as_str).collect()
}

#[test]
fn suppressing_the_faces_feature_suppresses_the_sketch_and_the_gate_refuses_one_left_on() {
    let features = padded();
    let document = checked_document(&strs(&features));
    let suppressed = suppression::suppress(&document, &id("b0000001")).unwrap();
    assert_eq!(
        suppressed.suppressed,
        [id("b0000001"), id("s0000001"), id("p0000001")]
    );
    // The box alone suppressed: the sketch is a dependent left on.
    let flagged = features[0].replace(
        r#""name": "Box", "#,
        r#""name": "Box", "suppressed": true, "#,
    );
    let invalid = checked(&[&flagged, &features[1], &features[2]]).unwrap_err();
    let e = &invalid.errors()[0];
    assert_eq!(e.feature(), Some("s0000001"));
    assert_eq!(e.parameter(), Some("plane.face"));
    assert_eq!(
        e.message(),
        "s0000001 isn't suppressed, but plane.face names an element of b0000001, which is suppressed: a feature that depends on a suppressed one is suppressed with it"
    );
    // Unsuppressing the sketch unsuppresses the face's feature.
    let all = checked_document(&[
        &flagged,
        &features[1].replace(
            r#""name": "Sketch", "#,
            r#""name": "Sketch", "suppressed": true, "#,
        ),
    ]);
    let back = suppression::unsuppress(&all, &id("s0000001")).unwrap();
    assert_eq!(back.unsuppressed, [id("b0000001"), id("s0000001")]);
}

#[test]
fn the_timeline_gives_a_face_sketch_the_faces_feature_as_its_input() {
    let features = padded();
    let document = checked_document(&strs(&features));
    let drawn = timeline(document.document(), None, &Excepted::default());
    let inputs: Vec<Vec<String>> = drawn
        .features
        .iter()
        .map(|e| e.inputs.iter().map(|i| i.to_string()).collect())
        .collect();
    assert_eq!(
        inputs,
        [
            vec![],
            vec!["b0000001".to_owned()],
            vec!["b0000001".to_owned(), "s0000001".to_owned()]
        ]
    );
    // A modified name's own feature is its source's: a fuse keeps the box's top as
    // b0000001.box.Face6, and the sketch on it names the box.
    let second = box_at("b0000002", [20.0, 0.0, 0.0], 1.0, 1.0, 1.0);
    let fuse = r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "Both", "base": "b0000001", "tool": "b0000002"}"#;
    let sketch = on_face("s0000001", &face("b0000001.box.Face6", "+x", false));
    let fused = checked_document(&[&cube(), &second, fuse, &sketch]);
    let drawn = timeline(fused.document(), None, &Excepted::default());
    assert_eq!(drawn.features[3].inputs, [id("b0000001")]);
}

#[test]
fn an_edit_of_the_faces_feature_reaches_the_sketch_and_a_pad_of_it_with_no_base() {
    // The pad stands on the box's top as a body of its own: only the face links it to the
    // box, so an edit of the box reaches it through the sketch (its fingerprint is written
    // anew at the commit), as one of its base would.
    let features = [
        cube(),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
        pad("p0000001", None, "s0000001", &one_side("3")),
    ];
    let document = read(sweeps::document(&strs(&features)).as_bytes()).unwrap();
    let reached = silico_core::document::with_users(&document, &[id("b0000001")].into());
    assert_eq!(
        reached,
        [id("b0000001"), id("s0000001"), id("p0000001")].into()
    );
}

#[test]
fn removing_the_faces_feature_is_refused_naming_the_sketch_and_moving_the_sketch_before_it_by_the_gate()
 {
    let features = padded();
    let document = checked_document(&strs(&features));
    let e = chain::remove(&document, &id("b0000001")).unwrap_err();
    assert_eq!(e.code(), ErrorCode::InvalidInput);
    assert_eq!(e.feature(), Some("b0000001"));
    assert_eq!(e.parameter(), Some("feature"));
    assert_eq!(
        e.message(),
        "s0000001 names b0000001's elements, so b0000001 can't be removed"
    );
    assert_eq!(
        e.suggestion(),
        Some("remove s0000001 first, or set its plane.face to another feature's face")
    );
    // The sketch before the box: the chain moves it, and the gate refuses what's left.
    let sketch_alone = checked_document(&strs(&features[..2]));
    let moved = chain::move_feature(&sketch_alone, &id("s0000001"), Some(&id("b0000001"))).unwrap();
    let invalid = validate(&moved.document).unwrap_err();
    assert_eq!(invalid.errors()[0].parameter(), Some("plane.face"));
    assert!(
        invalid.errors()[0]
            .message()
            .contains("isn't a face's name: no feature b0000001 before this one"),
        "{}",
        invalid.errors()[0].message()
    );
}

#[test]
fn the_outline_says_a_face_planes_axis_and_flip_and_its_face_on_a_line_of_its_own() {
    let document = read(DOCUMENT.as_bytes()).unwrap();
    let text = outline(&document);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[1..3],
        [
            r#"s0000001 "Top" sketch plane.x -y, plane.reversed true"#,
            r#"  face "b0000001.box.Face6""#,
        ],
        "{text}"
    );
}

fn published(engine: &Recompute<FakeKernel>, features: &[&str]) -> std::sync::Arc<Model> {
    match engine.start(checked_document(features)).wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish"),
    }
}

fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new())
}

#[test]
fn on_the_fake_a_face_typed_no_plane_breaks_the_sketch_and_blocks_its_pad() {
    // The fake describes every face as a surface of type other, never a plane: the sketch's
    // face is found, and refused as not planar, with no planar face to offer.
    let features = padded();
    let model = published(&engine(), &strs(&features));
    let Some(FeatureOutcome::Broken { errors, total }) = model.outcome(&id("s0000001")) else {
        panic!("{:?}", model.outcome(&id("s0000001")));
    };
    assert_eq!(*total, 1);
    let e = &errors[0];
    assert_eq!(e.code(), ErrorCode::ReferenceBroken);
    assert_eq!(e.feature(), Some("s0000001"));
    assert_eq!(e.parameter(), Some("plane.face"));
    assert_eq!(e.elements(), ["b0000001.box.Face6"]);
    assert!(e.candidates().is_empty(), "{:?}", e.candidates());
    assert_eq!(
        e.message(),
        "plane.face, \"b0000001.box.Face6\", names a face that isn't planar: OCCT types its surface a other, and a sketch sits only on a face typed a plane"
    );
    assert_eq!(model.frame(&id("s0000001")), None);
    assert_eq!(
        model.outcome(&id("p0000001")),
        Some(&FeatureOutcome::Blocked { on: id("s0000001") })
    );
    assert!(matches!(
        model.outcome(&id("b0000001")),
        Some(FeatureOutcome::Built { .. })
    ));
    assert_eq!(model.error_count(), 1);
}

#[test]
fn a_sketch_on_a_face_no_feature_sweeps_is_still_resolved_and_its_broken_face_reported() {
    // Face7 of a box names nothing: its nearest by number, Face6, is the candidate.
    let model = published(
        &engine(),
        &[
            &cube(),
            &on_face("s0000001", &face("b0000001.box.Face7", "+x", false)),
        ],
    );
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("s0000001")) else {
        panic!("{:?}", model.outcome(&id("s0000001")));
    };
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
    assert_eq!(errors[0].candidates(), ["b0000001.box.Face6"]);
    // The sketch is solved all the same: its diagnosis is the model's.
    assert!(model.diagnosed(&id("s0000001")).is_some());
}

#[test]
fn a_sketch_on_a_face_of_a_pad_blocked_by_its_own_sketch_is_blocked_on_that_sketch() {
    // Two rounds' worth: s2 is on the top of p1, whose sketch s1 is on the box's top. On
    // the fake s1 is refused (not planar), so p1 is blocked on it, and s2, whose face only
    // p1's solid could hold, is blocked on s1 too, and p2 with it.
    let features = [
        cube(),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
        pad("p0000001", Some("b0000001"), "s0000001", &one_side("3")),
        on_face(
            "s0000002",
            &face("p0000001.pad.Face(G s0000001.sketch.Face1)", "+x", false),
        ),
        pad("p0000002", Some("p0000001"), "s0000002", &one_side("1")),
    ];
    let model = published(&engine(), &strs(&features));
    let s1 = id("s0000001");
    assert!(matches!(
        model.outcome(&s1),
        Some(FeatureOutcome::Broken { .. })
    ));
    for blocked in ["p0000001", "s0000002", "p0000002"] {
        assert_eq!(
            model.outcome(&id(blocked)),
            Some(&FeatureOutcome::Blocked { on: s1.clone() }),
            "{blocked}"
        );
    }
}

#[test]
fn a_sketch_on_a_face_is_never_blocked_by_a_solid_its_name_couldnt_be_in() {
    // An unrelated body, p2 on b2, blocked on its own broken face sketch, s2, comes before
    // s1, on the box's top: s1's face could only be in what's built from b1, so p2's
    // failure doesn't block it. On the fake s1 is refused for its own face, as not planar.
    let features = [
        cube(),
        box_at("b0000002", [20.0, 0.0, 0.0], 5.0, 5.0, 5.0),
        on_face("s0000002", &face("b0000002.box.Face7", "+x", false)),
        pad("p0000002", Some("b0000002"), "s0000002", &one_side("1")),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
    ];
    let model = published(&engine(), &strs(&features));
    assert_eq!(
        model.outcome(&id("p0000002")),
        Some(&FeatureOutcome::Blocked { on: id("s0000002") })
    );
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("s0000001")) else {
        panic!("{:?}", model.outcome(&id("s0000001")));
    };
    assert!(
        errors[0].message().contains("isn't planar"),
        "{}",
        errors[0].message()
    );
}
