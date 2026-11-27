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
use silico_core::kernel::Op;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::timeline::{Excepted, timeline};
use silico_protocol::{Primitive, Reply, Surface};
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
  "naming": 6,
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

/// The fake answering every whole description as `faces` says of each face (by the shape's
/// expression, its number from 1), the fake's own edges and vertices kept.
fn described_as(engine: &Recompute<FakeKernel>, faces: fn(&str, usize) -> Surface) {
    engine.kernel().answer_instead(move |op| {
        let Op::DescribeAll { shape } = op else {
            return None;
        };
        let made = FakeKernel::describe(shape)?;
        let mut description = FakeKernel::described(shape)?;
        for (i, face) in description.faces.iter_mut().enumerate() {
            face.surface = faces(&made, i + 1);
        }
        Some(Reply::DescribedAll(description))
    });
}

#[test]
fn a_faces_refusal_waits_for_every_live_solid_so_its_candidates_are_the_same_however_slow_a_branch()
{
    // s1 is on b1's top, which the fake is made to describe as no plane (b1 has one planar
    // face, its Face1); the rest of every solid is planar (normal +Z, through z = 5). An
    // unrelated branch, b2 padded from s3 on its top into p3, is live at s1's place too, and
    // p3 waits a round for s3's face. s1's not-planar refusal, whose candidates are read from
    // every live solid, waits for p3, so they're b1's Face1 and then four of p3's planar faces
    // by name, whatever round or speed b2's make has: here held up to 80 ms each time.
    fn surfaces(made: &str, face: usize) -> Surface {
        let plane = Surface::Plane {
            normal: [0.0, 0.0, 1.0],
            location: [0.0, 0.0, 5.0],
        };
        if made.starts_with("box") && face != 1 {
            Surface::Other
        } else {
            plane
        }
    }
    let features = [
        cube(),
        box_at("b0000002", [20.0, 0.0, 0.0], 5.0, 5.0, 5.0),
        rectangle("s0000003", "xy", [21.0, 1.0], 2.0, 2.0).replace(
            r#""plane": "xy""#,
            &format!(r#""plane": {}"#, face("b0000002.box.Face6", "+x", false)),
        ),
        pad("p0000003", Some("b0000002"), "s0000003", &one_side("1")),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
    ];
    let mut seen = Vec::new();
    for delay in [0u64, 30, 80] {
        let engine = engine();
        described_as(&engine, surfaces);
        let hold = engine
            .kernel()
            .hold(|op| matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx == 5.0));
        let run = engine.start(checked_document(&strs(&features)));
        std::thread::sleep(std::time::Duration::from_millis(delay));
        hold.release();
        let model = match run.wait_timeout(WAIT) {
            Ok(Ok(RunOutcome::Published(model))) => model,
            other => panic!("{:?}", other.map(|r| r.map(|_| ())).ok()),
        };
        assert!(matches!(
            model.outcome(&id("p0000003")),
            Some(FeatureOutcome::Built { .. })
        ));
        let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("s0000001")) else {
            panic!("{:?}", model.outcome(&id("s0000001")));
        };
        let candidates = errors[0].candidates().to_vec();
        assert_eq!(candidates.len(), 5, "{candidates:?}");
        assert_eq!(candidates[0], "b0000001.box.Face1");
        let p3 = model.element_map(&id("p0000003")).unwrap();
        for name in &candidates[1..] {
            let parsed =
                silico_core::naming::Name::parse(name, &|f| model_maker(&model, f)).unwrap();
            assert!(p3.element(&parsed).is_some(), "{name} isn't p3's");
        }
        seen.push(candidates);
    }
    assert!(seen.windows(2).all(|w| w[0] == w[1]), "{seen:?}");
}

/// What the feature `id` of `model`'s document is, as a name's parser asks.
fn model_maker(
    model: &Model,
    id: &silico_core::document::FeatureId,
) -> Option<silico_core::naming::Maker> {
    model
        .document()
        .features
        .iter()
        .find(|f| &f.id == id)
        .map(|f| silico_core::naming::Maker::of(&f.operation))
}

/// Every face the fake describes is a plane, of normal +Z through z = 10: so every sketch on
/// a face is placed, and padded.
fn planar(_: &str, _: usize) -> Surface {
    Surface::Plane {
        normal: [0.0, 0.0, 1.0],
        location: [0.0, 0.0, 10.0],
    }
}

/// Whether `done` comes to hold within the tests' one wait: looked at until it does, so a
/// test fails here only by the wait running out.
fn within_wait(done: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        if done() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// How many requests `engine`'s kernel was asked that `picks` picks.
fn asked_count(engine: &Recompute<FakeKernel>, picks: impl Fn(&Op) -> bool) -> usize {
    engine
        .kernel()
        .asked()
        .iter()
        .filter(|call| picks(&call.op))
        .count()
}

fn is_prism(op: &Op) -> bool {
    matches!(op, Op::Prism { .. } | Op::PrismThroughAll { .. })
}

/// Whether `op` makes the 5 mm box, `b0000002`.
fn is_small_box(op: &Op) -> bool {
    matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx == 5.0)
}

/// `features` rebuilt on a fresh engine whose every face is planar, `b0000002`'s make held
/// until `prisms` prisms are asked for, or the tests' wait runs out: whether they were while
/// it was held, the model published once it's let go, the rounds the rebuild planned, and
/// the engine.
fn rebuilt_holding_the_small_box(
    features: &[String],
    prisms: usize,
) -> (bool, std::sync::Arc<Model>, usize, Recompute<FakeKernel>) {
    let held = engine();
    described_as(&held, planar);
    let hold = held.kernel().hold(is_small_box);
    let run = held.start(checked_document(&strs(features)));
    let progress = run.progress();
    let asked = within_wait(|| asked_count(&held, is_prism) >= prisms);
    hold.release();
    let model = match run.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        other => panic!("{:?}", other.map(|r| r.map(|_| ())).ok()),
    };
    (asked, model, progress.rounds(), held)
}

/// The document as a save writes it.
fn saved_text(model: &Model) -> String {
    write(&model.saved()).unwrap()
}

#[test]
fn a_face_sketchs_pad_is_built_while_an_unrelated_branch_is_still_building() {
    // M5's milestone review, A13-M1: s1 is on b1's top, and p1 pads it; b2, a box that
    // nothing of s1 is made from, is held mid-make. p1's prism is asked for while b2 is still
    // held, as the attacher note's rounds have it ("a slow or failed branch it doesn't come
    // from never delays ... a sketch that resolves"): before, s1 was placed only once every
    // step of its round had settled, b2's make among them, so p1 waited for b2.
    let features = [
        cube(),
        box_at("b0000002", [20.0, 0.0, 0.0], 5.0, 5.0, 5.0),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
        pad("p0000001", Some("b0000001"), "s0000001", &one_side("3")),
    ];
    let (while_held, model, rounds, held) = rebuilt_holding_the_small_box(&features, 1);
    assert!(
        while_held,
        "p1's prism wasn't asked for while b2's make was held"
    );
    assert!(matches!(
        model.outcome(&id("p0000001")),
        Some(FeatureOutcome::Built { .. })
    ));
    // Each step asked for once, the prism no more for having been asked ahead of its round;
    // and the rounds as before: one placing s1, one finding none waiting.
    assert_eq!(asked_count(&held, is_prism), 1);
    assert_eq!(asked_count(&held, is_small_box), 1);
    assert_eq!(rounds, 2);
    // The model published is the one a rebuild with nothing held publishes, byte for byte
    // as a save writes it.
    let unheld = engine();
    described_as(&unheld, planar);
    assert_eq!(
        saved_text(&model),
        saved_text(&published(&unheld, &strs(&features)))
    );
}

#[test]
fn a_sketch_on_a_face_of_a_face_sketchs_pad_and_its_pad_are_built_while_an_unrelated_branch_is_still_building()
 {
    // A level deeper: s2 is on p1's top, and p2 pads it onto p1. Both prisms are asked for
    // while b2's make is held: before, each level of nested sketches waited for every step
    // of its round, b2's make in the first.
    let features = [
        cube(),
        box_at("b0000002", [20.0, 0.0, 0.0], 5.0, 5.0, 5.0),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
        pad("p0000001", Some("b0000001"), "s0000001", &one_side("3")),
        on_face(
            "s0000002",
            &face("p0000001.pad.Face(G s0000001.sketch.Face1)", "+x", false),
        ),
        pad("p0000002", Some("p0000001"), "s0000002", &one_side("1")),
    ];
    let (while_held, model, rounds, held) = rebuilt_holding_the_small_box(&features, 2);
    assert!(
        while_held,
        "p1's and p2's prisms weren't both asked for while b2's make was held"
    );
    for built in ["p0000001", "p0000002"] {
        assert!(
            matches!(
                model.outcome(&id(built)),
                Some(FeatureOutcome::Built { .. })
            ),
            "{built}: {:?}",
            model.outcome(&id(built))
        );
    }
    assert_eq!(asked_count(&held, is_prism), 2);
    // One round a level, and one more finding none waiting.
    assert_eq!(rounds, 3);
    let unheld = engine();
    described_as(&unheld, planar);
    assert_eq!(
        saved_text(&model),
        saved_text(&published(&unheld, &strs(&features)))
    );
}

/// The box, an unrelated 5 mm box, a sketch on the box's top, and its pad onto the box.
fn beside_a_small_box() -> Vec<String> {
    vec![
        cube(),
        box_at("b0000002", [20.0, 0.0, 0.0], 5.0, 5.0, 5.0),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
        pad("p0000001", Some("b0000001"), "s0000001", &one_side("3")),
    ]
}

#[test]
fn a_pad_placed_ahead_whose_prism_the_kernel_refuses_fails_as_it_would_in_its_round() {
    // Break-it, A13-M1: the step run ahead of its round fails. The pad fails with the
    // kernel's refusal, naming it, as a rebuild with nothing held has it, byte for byte; the
    // prism is asked once, not again by the round that plans it; and the rest is built.
    fn refusing(engine: &Recompute<FakeKernel>) {
        engine.kernel().refuse(|op| {
            is_prism(op).then(|| silico_core::Error::new(ErrorCode::InvalidInput, "no prism"))
        });
    }
    let features = beside_a_small_box();
    let held = engine();
    described_as(&held, planar);
    refusing(&held);
    let hold = held.kernel().hold(is_small_box);
    let run = held.start(checked_document(&strs(&features)));
    assert!(
        within_wait(|| asked_count(&held, is_prism) == 1),
        "the prism wasn't asked for while b2's make was held"
    );
    hold.release();
    let model = match run.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        other => panic!("{:?}", other.map(|r| r.map(|_| ())).ok()),
    };
    let Some(FeatureOutcome::Failed(error)) = model.outcome(&id("p0000001")) else {
        panic!("{:?}", model.outcome(&id("p0000001")));
    };
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(error.feature(), Some("p0000001"));
    assert!(error.message().contains("no prism"), "{error:?}");
    for built in ["b0000001", "b0000002"] {
        assert!(
            matches!(
                model.outcome(&id(built)),
                Some(FeatureOutcome::Built { .. })
            ),
            "{built}"
        );
    }
    assert_eq!(asked_count(&held, is_prism), 1);
    let unheld = engine();
    described_as(&unheld, planar);
    refusing(&unheld);
    assert_eq!(
        saved_text(&model),
        saved_text(&published(&unheld, &strs(&features)))
    );
}

#[test]
fn a_rebuild_cancelled_while_a_step_placed_ahead_runs_stops_and_the_next_builds_it() {
    // Break-it, A13-M1: the pad's prism, run ahead of its round, is held, and so is b2; the
    // rebuild is cancelled. It ends superseded within the wait, holding no thread on the
    // held steps; once they're let go, a rebuild of the same document publishes the pad.
    let features = beside_a_small_box();
    let engine = engine();
    described_as(&engine, planar);
    let small = engine.kernel().hold(is_small_box);
    let prism = engine.kernel().hold(is_prism);
    let run = engine.start(checked_document(&strs(&features)));
    assert!(within_wait(|| asked_count(&engine, is_prism) == 1));
    engine.cancel();
    match run.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Superseded)) => {}
        Ok(other) => panic!("not superseded: {other:?}"),
        Err(_) => panic!("the cancelled rebuild didn't end"),
    }
    small.release();
    prism.release();
    let model = published(&engine, &strs(&features));
    assert!(matches!(
        model.outcome(&id("p0000001")),
        Some(FeatureOutcome::Built { .. })
    ));
}

#[test]
fn a_rebuild_again_of_a_face_sketchs_pad_built_ahead_asks_the_kernel_nothing() {
    // Break-it, A13-M1: what ran ahead of its round is kept as the round's own steps are,
    // so a second rebuild of the same document, on the same engine, finds every step and
    // measure kept: it asks nothing, in the same two rounds, and publishes the same model.
    let features = beside_a_small_box();
    let (while_held, first, _, engine) = rebuilt_holding_the_small_box(&features, 1);
    assert!(while_held);
    let asked = engine.kernel().asked().len();
    let run = engine.start(checked_document(&strs(&features)));
    let progress = run.progress();
    let again = match run.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        other => panic!("{:?}", other.map(|r| r.map(|_| ())).ok()),
    };
    assert_eq!(engine.kernel().asked().len(), asked);
    assert_eq!(progress.rounds(), 2);
    assert_eq!(saved_text(&again), saved_text(&first));
}

#[test]
fn while_an_unrelated_branch_is_held_the_rebuild_says_the_sketches_placed_ahead_built_and_counts_their_pads()
 {
    // Break-it, A13-M1: `status.rebuild` while b2 is held and both levels' pads have run
    // ahead of their rounds. Each sketch placed ahead reads built (it solved), each pad
    // built, b2 running; the steps counted include the pads', and `done` is under them by
    // b2's make and the step after it (its translate), nothing else. Once let go, every
    // feature is built, the count done.
    let features = [
        cube(),
        box_at("b0000002", [20.0, 0.0, 0.0], 5.0, 5.0, 5.0),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
        pad("p0000001", Some("b0000001"), "s0000001", &one_side("3")),
        on_face(
            "s0000002",
            &face("p0000001.pad.Face(G s0000001.sketch.Face1)", "+x", false),
        ),
        pad("p0000002", Some("p0000001"), "s0000002", &one_side("1")),
    ];
    let engine = engine();
    described_as(&engine, planar);
    let hold = engine.kernel().hold(is_small_box);
    let run = engine.start(checked_document(&strs(&features)));
    let progress = run.progress();
    // p2's every step built and measured: its fuse asked, and its solid described.
    let fuses = |op: &Op| matches!(op, Op::Boolean { .. });
    let describes = |op: &Op| matches!(op, Op::DescribeAll { .. });
    assert!(within_wait(|| {
        let now = progress.now();
        asked_count(&engine, fuses) == 2
            && asked_count(&engine, describes) == 3
            && now.features.get(&id("p0000002")) == Some(silico_core::recompute::StepState::Built)
    }));
    let held = progress.now();
    use silico_core::recompute::StepState::{Built, Queued, Running};
    for (feature, state) in [
        ("b0000001", Built),
        ("s0000001", Built),
        ("p0000001", Built),
        ("s0000002", Built),
        ("p0000002", Built),
    ] {
        assert_eq!(
            held.features.get(&id(feature)),
            Some(state),
            "{feature}: {held:?}"
        );
    }
    assert!(
        matches!(held.features.get(&id("b0000002")), Some(Running | Queued)),
        "{held:?}"
    );
    // The steps: b1's make; b2's make and translate; the pads' faces, one step, as the two
    // sketches are the same rectangle on the same frame (every face the plane z = 10, x
    // along +X: docs/ports/attacher.md, *The key*); and each pad's prism and fuse. All but
    // b2's two settled.
    assert_eq!(held.steps, 8, "{held:?}");
    assert_eq!(held.done, 6, "{held:?}");
    assert!(!held.ended);
    hold.release();
    match run.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(_))) => {}
        other => panic!("{:?}", other.map(|r| r.map(|_| ())).ok()),
    }
    let done = progress.now();
    assert_eq!(
        (done.done, done.steps, done.ended),
        (8, 8, true),
        "{done:?}"
    );
    assert!(
        done.features.iter().all(|(_, state)| state == Built),
        "{done:?}"
    );
}

#[test]
fn a_fresh_rebuild_asks_again_for_the_steps_a_face_sketch_placed_ahead_makes() {
    // PR #614's review (tests): a rebuild reading nothing kept builds every step again,
    // those it runs ahead of their rounds too. After a rebuild on the same engine, a fresh
    // one asks for the pad's prism again, while b2 is held.
    let features = beside_a_small_box();
    let (while_held, _, _, engine) = rebuilt_holding_the_small_box(&features, 1);
    assert!(while_held);
    assert_eq!(asked_count(&engine, is_prism), 1);
    let hold = engine.kernel().hold(is_small_box);
    let run = engine.start_fresh(checked_document(&strs(&features)));
    let asked_ahead = within_wait(|| asked_count(&engine, is_prism) >= 2);
    hold.release();
    match run.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(_))) => {}
        other => panic!("{:?}", other.map(|r| r.map(|_| ())).ok()),
    }
    assert!(
        asked_ahead,
        "the fresh rebuild's prism wasn't asked while b2 was held"
    );
    assert_eq!(asked_count(&engine, is_prism), 2);
}

#[test]
fn a_sketch_on_a_face_is_placed_ahead_only_from_its_own_solid_once_built_never_from_its_maker() {
    // PR #614's review (tests): s1 names b1's top, but b1 is consumed by a fillet before s1,
    // so its face is looked for in the fillet's solid, not the box's. The fake describes each
    // solid's faces as planes through a height of its own (its expression's length), so a
    // frame taken from the box would be another than the fillet's. The fillet's step is
    // held: the pad's prism isn't asked for while it is, though the box is built and
    // described; once it's let go, s1's frame is the fillet's top, through the fillet's
    // height, and the model is the one a rebuild with nothing held publishes.
    use std::sync::{Arc, Mutex};
    fn by_solid(engine: &Recompute<FakeKernel>, heights: &Arc<Mutex<Vec<(String, f64)>>>) {
        let heights = Arc::clone(heights);
        engine.kernel().answer_instead(move |op| {
            let Op::DescribeAll { shape } = op else {
                return None;
            };
            let made = FakeKernel::describe(shape)?;
            let height = made.len() as f64;
            heights.lock().unwrap().push((made.clone(), height));
            let mut description = FakeKernel::described(shape)?;
            for face in &mut description.faces {
                face.surface = Surface::Plane {
                    normal: [0.0, 0.0, 1.0],
                    location: [0.0, 0.0, height],
                };
            }
            Some(Reply::DescribedAll(description))
        });
    }
    let fillet = r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "F", "base": "b0000001", "edges": ["b0000001.box.Edge1"], "radius": 1}"#;
    let features = [
        cube(),
        box_at("b0000002", [20.0, 0.0, 0.0], 5.0, 5.0, 5.0),
        fillet.to_owned(),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
        pad("p0000001", Some("f0000001"), "s0000001", &one_side("3")),
    ];
    let is_fillet = |op: &Op| matches!(op, Op::Fillet { .. });
    let heights = Arc::new(Mutex::new(Vec::new()));
    let held = engine();
    by_solid(&held, &heights);
    let hold = held.kernel().hold(is_fillet);
    let run = held.start(checked_document(&strs(&features)));
    // The fillet held, and both boxes built and described: all the look ahead could see.
    let boxes_described = || {
        heights
            .lock()
            .unwrap()
            .iter()
            .filter(|(made, _)| !made.contains("fillet"))
            .count()
            >= 2
    };
    assert!(within_wait(
        || asked_count(&held, is_fillet) == 1 && boxes_described()
    ));
    assert_eq!(
        asked_count(&held, is_prism),
        0,
        "the pad's prism was asked for while the fillet its sketch's face is in was held"
    );
    hold.release();
    let model = match run.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        other => panic!("{:?}", other.map(|r| r.map(|_| ())).ok()),
    };
    assert!(matches!(
        model.outcome(&id("p0000001")),
        Some(FeatureOutcome::Built { .. })
    ));
    let fillet_height = heights
        .lock()
        .unwrap()
        .iter()
        .find(|(made, _)| made.starts_with("fillet"))
        .map(|(_, height)| *height)
        .unwrap();
    let box_height = heights
        .lock()
        .unwrap()
        .iter()
        .find(|(made, _)| made.starts_with("box"))
        .map(|(_, height)| *height)
        .unwrap();
    assert_ne!(fillet_height, box_height);
    let frame = model.frame(&id("s0000001")).unwrap();
    assert_eq!(frame.origin, [0.0, 0.0, fillet_height], "{frame:?}");
    // The prism asked for once, on the fillet's frame: none run ahead on another.
    assert_eq!(asked_count(&held, is_prism), 1);
    let unheld = engine();
    by_solid(&unheld, &Arc::new(Mutex::new(Vec::new())));
    assert_eq!(
        saved_text(&model),
        saved_text(&published(&unheld, &strs(&features)))
    );
}

#[test]
fn an_edit_of_an_unrelated_branch_takes_the_steps_a_look_ahead_plans_from_what_the_engine_keeps() {
    // Mutants, PR #614: a rebuild after an edit of b2 only has b2's steps to run, so it
    // looks ahead, and places s1 ahead on the box it keeps; the pad's steps it plans then are
    // taken from what the engine keeps, as the round's own are, not asked for again. The
    // edited box is made again; the prism, once in all.
    let features = beside_a_small_box();
    let engine = engine();
    described_as(&engine, planar);
    published(&engine, &strs(&features));
    assert_eq!(asked_count(&engine, is_prism), 1);
    let mut edited = features.clone();
    edited[1] = box_at("b0000002", [20.0, 0.0, 0.0], 6.0, 6.0, 6.0);
    let model = published(&engine, &strs(&edited));
    assert!(matches!(
        model.outcome(&id("p0000001")),
        Some(FeatureOutcome::Built { .. })
    ));
    let bigger = |op: &Op| matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx == 6.0);
    assert_eq!(asked_count(&engine, bigger), 1);
    assert_eq!(asked_count(&engine, is_prism), 1);
}

#[test]
fn a_face_found_past_the_models_reach_is_never_placed_ahead_so_its_pad_asks_nothing() {
    // Mutants, PR #614: every face the fake describes is the plane z = 1.5e6, within a
    // frame's reach (√3·1e6) but placing the sketch's rectangle past 1e6 mm, the model's:
    // the sketch is refused there, naming `plane`, at its round's end, and never placed
    // ahead, so its pad's prism is never asked for.
    fn far(_: &str, _: usize) -> Surface {
        Surface::Plane {
            normal: [0.0, 0.0, 1.0],
            location: [0.0, 0.0, 1.5e6],
        }
    }
    let engine = engine();
    described_as(&engine, far);
    let model = published(&engine, &strs(&beside_a_small_box()));
    let Some(FeatureOutcome::Failed(error)) = model.outcome(&id("s0000001")) else {
        panic!("{:?}", model.outcome(&id("s0000001")));
    };
    assert_eq!(error.parameter(), Some("plane"), "{error:?}");
    assert_eq!(
        model.outcome(&id("p0000001")),
        Some(&FeatureOutcome::Blocked { on: id("s0000001") })
    );
    assert_eq!(asked_count(&engine, is_prism), 0);
}
