//! `add_feature` of a sketch on a face that leaves out its plane's `x`, and `set_field` of a
//! sketch's `plane` (M5.9's part 3; docs/design/m4.1-command-api.md, section 5), on the fake
//! kernel. The fake is made to describe each box's six faces as the planes OCCT makes them
//! (docs/ports/attacher.md, *Worked frames*: Face1 facing −X, Face2 +X, Face3 −Y, Face4 +Y,
//! Face5 −Z, Face6 +Z), so each token expected is that table's, written out by hand; each
//! refusal's words are the gate's and `face_plane.rs`'s, written out by hand. The real
//! kernel's cases (a cube's six faces as OCCT describes them, a cylinder's side, a fuse, a
//! slanted side moved by an edit, and the repair of a broken face that pads the box's top)
//! are silico-kernel's tests/face_sketches.rs.

use super::model::WAIT;
use super::sweeps::{box_at, document, rectangle};

use std::sync::Arc;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Call, Sessions};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{Op, Primitive};
use silico_core::sketch::SketchSolver;
use silico_core::{Error, ErrorCode};
use silico_protocol::{Reply, Surface};

/// Each box face's outward normal, by its number from 1, as OCCT numbers a box's faces.
const BOX_NORMALS: [[f64; 3]; 6] = [
    [-1.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [0.0, -1.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, -1.0],
    [0.0, 0.0, 1.0],
];

/// The fake, each box's faces described as [`BOX_NORMALS`]' planes through the origin, and
/// every other shape's as the fake's own (no face a plane).
fn planar_boxes() -> FakeKernel {
    let kernel = FakeKernel::new(2).unwrap();
    kernel.answer_instead(|op| {
        let Op::DescribeAll { shape } = op else {
            return None;
        };
        let made = FakeKernel::describe(shape)?;
        if !(made.starts_with("box") || made.starts_with("translate(box")) {
            return None;
        }
        let mut description = FakeKernel::described(shape)?;
        assert_eq!(description.faces.len(), 6);
        for (face, normal) in description.faces.iter_mut().zip(BOX_NORMALS) {
            face.surface = Surface::Plane {
                normal,
                location: [0.0; 3],
            };
        }
        Some(Reply::DescribedAll(description))
    });
    kernel
}

fn cube() -> String {
    box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0)
}

/// A 4 × 2 rectangle from (1, 1), sketch `id`, its plane `plane` (any JSON).
fn sketch_on(id: &str, plane: Value) -> Value {
    let mut sketch: Value =
        serde_json::from_str(&rectangle(id, "xy", [1.0, 1.0], 4.0, 2.0)).unwrap();
    sketch["plane"] = plane;
    sketch
}

/// [`sketch_on`] the face `face`, its `x` left out.
fn without_x(id: &str, face: &str) -> Value {
    sketch_on(id, json!({"face": face, "reversed": false}))
}

/// Sessions on `kernel`, sketches solved as stored, with `features` opened as session s1.
struct Opened {
    _dir: tempfile::TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn opened_on(kernel: FakeKernel, features: &[String]) -> Opened {
    let dir = tempfile::tempdir().unwrap();
    let sketches = FakeSketches::new();
    let sessions = Sessions::with_stand_ins(
        Arc::new(kernel),
        |cache: &std::path::Path| Cache::unsealed(cache),
        move || Ok(Box::new(sketches.clone()) as Box<dyn SketchSolver>),
    );
    let path = dir.path().join("part.silico");
    let features: Vec<&str> = features.iter().map(String::as_str).collect();
    std::fs::write(&path, document(&features)).unwrap();
    let opened = Opened {
        _dir: dir,
        sessions,
    };
    let answer = opened.call(
        "open",
        &json!({"path": path.to_str().unwrap(), "create": false}),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    opened
}

impl Opened {
    fn call(&self, name: &str, arguments: &Value) -> Value {
        match self.sessions.call(name, &arguments.to_string()) {
            Call::Running(calling) => {
                let answered = calling
                    .wait_timeout(WAIT)
                    .unwrap_or_else(|_| panic!("{name} didn't answer in {WAIT:?}"));
                serde_json::from_str(&answered.text).unwrap()
            }
            Call::Unknown => panic!("no command {name}"),
        }
    }

    fn revision(&self) -> u64 {
        let status = self.call("status", &json!({"session": "s1"}));
        status["session"]["revision"].as_u64().unwrap()
    }

    fn text(&self) -> String {
        let text = self.call("text", &json!({"session": "s1", "feature": null}));
        text["result"]["text"].as_str().unwrap().to_owned()
    }

    /// `add_feature` of `feature` at the end (or before `before`), against the session's
    /// revision.
    fn add(&self, before: Option<&str>, feature: &Value) -> Value {
        let revision = self.revision();
        self.call(
            "add_feature",
            &json!({"session": "s1", "revision": revision, "before": before, "feature": feature}),
        )
    }

    /// `set_field` of the sketch `sketch`'s `field` to `value`.
    fn set(&self, sketch: &str, field: &str, value: Value) -> Value {
        let revision = self.revision();
        self.call(
            "set_field",
            &json!({"session": "s1", "revision": revision, "feature": sketch, "field": field, "value": value}),
        )
    }

    /// Each face's `default_x` as `elements` reports it, by name, of `feature`'s solid.
    fn default_xs(&self, feature: &str) -> Vec<(String, Value)> {
        let listed = self.call(
            "elements",
            &json!({"session": "s1", "model": "current", "feature": feature, "after": null, "limit": 100}),
        );
        assert_eq!(listed["ok"], true, "{listed}");
        listed["result"]["elements"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["kind"] == "face")
            .map(|e| {
                (
                    e["name"].as_str().unwrap().to_owned(),
                    e["default_x"].clone(),
                )
            })
            .collect()
    }

    /// That `answer` is a refusal whose first error is `error` (as the wire writes it), one
    /// of `total`, the session's revision and text as `revision` and `text`.
    #[track_caller]
    fn refused(&self, answer: &Value, error: &Value, revision: u64, text: &str) {
        assert_eq!(answer["ok"], false, "{answer}");
        assert_eq!(answer["errors"][0], *error, "{answer}");
        assert_eq!(self.revision(), revision, "the edit was made");
        assert_eq!(self.text(), text, "the edit was made");
    }
}

/// The error contract's object, as the wire writes it.
fn error(
    code: &str,
    message: &str,
    feature: &str,
    elements: &[&str],
    parameter: &str,
    suggestion: &str,
    candidates: &[&str],
) -> Value {
    json!({
        "code": code,
        "message": message,
        "feature": feature,
        "elements": elements,
        "elements_total": elements.len(),
        "parameter": parameter,
        "cause": null,
        "suggestion": suggestion,
        "candidates": candidates,
        "advice": null,
    })
}

/// F1's table for a box's six faces (docs/ports/attacher.md, *Worked frames*).
const TABLE: [(&str, &str); 6] = [
    ("Face1", "-y"),
    ("Face2", "+y"),
    ("Face3", "+x"),
    ("Face4", "-x"),
    ("Face5", "+x"),
    ("Face6", "+x"),
];

#[test]
fn a_sketch_added_on_each_face_of_a_cube_without_its_x_is_written_with_the_faces_default_x() {
    let f = opened_on(planar_boxes(), &[cube()]);
    for (i, (number, token)) in TABLE.iter().enumerate() {
        let sketch = format!("s000000{}", i + 1);
        let face = format!("b0000001.box.{number}");
        let revision = f.revision();
        let added = f.add(None, &without_x(&sketch, &face));
        assert_eq!(added["ok"], true, "{number}: {added}");
        // The token always written, in the plane's order: face, x, reversed.
        assert_eq!(
            added["result"]["feature"]["plane"],
            json!({"face": face, "x": token, "reversed": false}),
            "{number}"
        );
        assert_eq!(added["result"]["index"], i + 1);
        assert_eq!(f.revision(), revision + 1, "{number}");
        let written =
            format!(r#""plane": {{"face": "{face}", "x": "{token}", "reversed": false}}"#);
        assert!(f.text().contains(&written), "{number}: {}", f.text());
    }
    // Each is the face's default_x as `elements` reports it.
    let reported = f.default_xs("b0000001");
    let want: Vec<(String, Value)> = TABLE
        .iter()
        .map(|(number, token)| (format!("b0000001.box.{number}"), json!(token)))
        .collect();
    assert_eq!(reported, want);
    // A null x is the same as one left out.
    let null = sketch_on(
        "s0000009",
        json!({"face": "b0000001.box.Face2", "x": null, "reversed": true}),
    );
    let added = f.add(None, &null);
    assert_eq!(added["ok"], true, "{added}");
    assert_eq!(
        added["result"]["feature"]["plane"],
        json!({"face": "b0000001.box.Face2", "x": "+y", "reversed": true})
    );
    // An x given is kept as given, never the default.
    let given = sketch_on(
        "s000000a",
        json!({"face": "b0000001.box.Face2", "x": "-z", "reversed": false}),
    );
    let added = f.add(None, &given);
    assert_eq!(added["result"]["feature"]["plane"]["x"], "-z", "{added}");
}

#[test]
fn inside_a_transaction_the_face_is_found_in_the_current_documents_model_not_the_published_one() {
    // The second box is added inside the transaction: only the current document has it, so
    // the published model (the opened document's) holds no b0000002.box.Face2.
    let f = opened_on(planar_boxes(), &[cube()]);
    let published = f.call(
        "elements",
        &json!({"session": "s1", "model": "current", "feature": "b0000001", "after": null, "limit": 1}),
    );
    assert_eq!(published["ok"], true, "{published}");
    assert_eq!(f.call("begin", &json!({"session": "s1"}))["ok"], true);
    let second =
        serde_json::from_str::<Value>(&box_at("b0000002", [20.0, 0.0, 0.0], 1.0, 1.0, 1.0))
            .unwrap();
    assert_eq!(f.add(None, &second)["ok"], true);
    let added = f.add(None, &without_x("s0000001", "b0000002.box.Face2"));
    assert_eq!(added["ok"], true, "{added}");
    assert_eq!(added["result"]["feature"]["plane"]["x"], "+y");
    assert_eq!(added["result"]["commit"], Value::Null);
    // Rolled back, the document is as it was: the gate refuses a face of a feature it
    // hasn't.
    assert_eq!(f.call("rollback", &json!({"session": "s1"}))["ok"], true);
    let refused = f.add(None, &without_x("s0000001", "b0000002.box.Face2"));
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["errors"][0]["parameter"], "plane.face", "{refused}");
    assert!(
        refused["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("no feature b0000002 before this one"),
        "{refused}"
    );
}

#[test]
fn a_face_that_names_nothing_is_refused_as_broken_with_candidates_and_the_edit_isnt_made() {
    let f = opened_on(planar_boxes(), &[cube()]);
    let (revision, text) = (f.revision(), f.text());
    let answer = f.add(None, &without_x("s0000001", "b0000001.box.Face7"));
    f.refused(
        &answer,
        &error(
            "reference_broken",
            "plane.face, \"b0000001.box.Face7\", names no face of the solids live at the sketch's place: it isn't one of the faces they're made from, or an edit before this sketch removed or remade it",
            "s0000001",
            &["b0000001.box.Face7"],
            "feature.plane.face",
            "name the face as its feature now names it: the candidates are the likeliest, none taken for it",
            &["b0000001.box.Face6"],
        ),
        revision,
        &text,
    );
    assert_eq!(answer["errors"].as_array().unwrap().len(), 1);
}

#[test]
fn a_face_the_fake_types_no_plane_is_refused_as_not_planar_and_the_edit_isnt_made() {
    // The fake's own descriptions: no face is typed a plane, so there's no candidate.
    let f = opened_on(FakeKernel::new(2).unwrap(), &[cube()]);
    let (revision, text) = (f.revision(), f.text());
    let answer = f.add(None, &without_x("s0000001", "b0000001.box.Face6"));
    f.refused(
        &answer,
        &error(
            "reference_broken",
            "plane.face, \"b0000001.box.Face6\", names a face that isn't planar: OCCT types its surface a other, and a sketch sits only on a face typed a plane",
            "s0000001",
            &["b0000001.box.Face6"],
            "feature.plane.face",
            "name the face as its feature now names it: the candidates are the likeliest, none taken for it",
            &[],
        ),
        revision,
        &text,
    );
}

#[test]
fn an_edge_an_unstable_name_and_a_face_of_a_later_feature_are_the_gates_refusals() {
    // Refused by the gate before any model is asked for, in the gate's words, at the
    // document's field; the edit not made.
    let fuse = r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "Both", "base": "b0000001", "tool": "b0000002"}"#;
    let f = opened_on(
        planar_boxes(),
        &[
            cube(),
            box_at("b0000002", [20.0, 0.0, 0.0], 1.0, 1.0, 1.0),
            fuse.to_owned(),
        ],
    );
    let (revision, text) = (f.revision(), f.text());
    let face_advice = "write the face's name as Silico names it (feature.type.Face...), of a feature before this sketch";
    let cases = [
        (
            "b0000001.box.Edge3",
            "plane.face, \"b0000001.box.Edge3\", names an edge, not a face",
            face_advice,
        ),
        (
            "f0000001.fuse.Face3",
            "plane.face, \"f0000001.fuse.Face3\", names a face by a name that doesn't say which: a duplicate (~n) or an operation's element by its number",
            "name the face by how it came about, as Silico names it now",
        ),
        (
            "b0000001.box.Face6~1",
            "plane.face, \"b0000001.box.Face6~1\", names a face by a name that doesn't say which: a duplicate (~n) or an operation's element by its number",
            "name the face by how it came about, as Silico names it now",
        ),
    ];
    for (face, message, suggestion) in cases {
        let answer = f.add(None, &without_x("s0000001", face));
        f.refused(
            &answer,
            &error(
                "invalid_input",
                message,
                "s0000001",
                &[],
                "plane.face",
                suggestion,
                &[],
            ),
            revision,
            &text,
        );
    }
    // Added before the box its face is of: a cycle, which the name can't be read before.
    let answer = f.add(
        Some("b0000001"),
        &without_x("s0000001", "b0000001.box.Face6"),
    );
    assert_eq!(answer["ok"], false, "{answer}");
    let first = &answer["errors"][0];
    assert_eq!(first["parameter"], "plane.face", "{answer}");
    assert!(
        first["message"].as_str().unwrap().starts_with(
            "plane.face, \"b0000001.box.Face6\", isn't a face's name: no feature b0000001 before this one"
        ),
        "{answer}"
    );
    assert_eq!((f.revision(), f.text()), (revision, text));
}

#[test]
fn a_face_of_a_feature_that_failed_is_refused_naming_that_feature() {
    // The box's values refused by the kernel: it failed, so its faces' defaults aren't
    // known. Refused naming the box, with the way on: mend it, or give x.
    let kernel = planar_boxes();
    kernel.refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Box { .. }))
            .then(|| Error::new(ErrorCode::KernelFailed, "the box couldn't be made"))
    });
    let f = opened_on(kernel, &[cube()]);
    let (revision, text) = (f.revision(), f.text());
    let answer = f.add(None, &without_x("s0000001", "b0000001.box.Face6"));
    f.refused(
        &answer,
        &error(
            "invalid_input",
            "plane.face, \"b0000001.box.Face6\", is looked for in b0000001's solid, which didn't build, so the face's default x isn't known",
            "b0000001",
            &["b0000001.box.Face6"],
            "feature.plane.face",
            "mend b0000001 first (`check` says why it failed), or give plane.x to add the sketch all the same",
            &[],
        ),
        revision,
        &text,
    );
    // Given its x, it's added, and the rebuild blocks it on the box.
    let given = sketch_on(
        "s0000001",
        json!({"face": "b0000001.box.Face6", "x": "+x", "reversed": false}),
    );
    assert_eq!(f.add(None, &given)["ok"], true);
    assert_eq!(f.revision(), revision + 1);
}

#[test]
fn a_face_of_a_box_that_crashed_the_worker_is_refused_with_the_fault() {
    // A kernel fault isn't the document's: refused with the fault itself, as a read of the
    // box's solid is, its suggestion saying how to try again.
    let kernel = planar_boxes();
    kernel.refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Box { .. }))
            .then(|| Error::new(ErrorCode::WorkerCrashed, "the kernel worker crashed"))
    });
    let f = opened_on(kernel, &[cube()]);
    let (revision, text) = (f.revision(), f.text());
    let answer = f.add(None, &without_x("s0000001", "b0000001.box.Face6"));
    assert_eq!(answer["ok"], false, "{answer}");
    let fault = &answer["errors"][0];
    assert_eq!(fault["code"], "worker_crashed", "{answer}");
    assert_eq!(fault["message"], "the kernel worker crashed");
    assert!(
        fault["suggestion"]
            .as_str()
            .unwrap()
            .contains("check with fresh"),
        "{answer}"
    );
    // The same fault a read of the box gives.
    let read = f.call(
        "elements",
        &json!({"session": "s1", "model": "current", "feature": "b0000001", "after": null, "limit": 1}),
    );
    assert_eq!(read["errors"][0], *fault);
    assert_eq!((f.revision(), f.text()), (revision, text));
}

#[test]
fn hostile_face_planes_without_x_are_refused_where_the_reader_or_gate_refuses_them() {
    let f = opened_on(planar_boxes(), &[cube()]);
    let (revision, text) = (f.revision(), f.text());
    let megabyte = format!("b0000001.box.Face{}", "9".repeat(1 << 20));
    let cases: [(Value, &str); 8] = [
        // The face: a number, empty, missing, a megabyte long.
        (json!({"face": 5, "reversed": false}), "plane.face"),
        (json!({"face": "", "reversed": false}), "plane.face"),
        (json!({"reversed": false}), "plane.face"),
        (json!({"face": megabyte, "reversed": false}), "plane.face"),
        // Its reversed: missing, or not a boolean.
        (json!({"face": "b0000001.box.Face6"}), "plane.reversed"),
        (
            json!({"face": "b0000001.box.Face6", "reversed": "no"}),
            "plane.reversed",
        ),
        // A member the plane hasn't; an x that isn't a token (only null stands for none).
        (
            json!({"face": "b0000001.box.Face6", "reversed": false, "y": "+z"}),
            "plane.y",
        ),
        (
            json!({"face": "b0000001.box.Face6", "x": 7, "reversed": false}),
            "plane.x",
        ),
    ];
    for (plane, parameter) in cases {
        let shown = plane.to_string().chars().take(80).collect::<String>();
        // Each answered within `WAIT` (`call`'s), a megabyte's name too.
        let answer = f.add(None, &sketch_on("s0000001", plane));
        assert_eq!(answer["ok"], false, "{shown}: {answer}");
        let first = &answer["errors"][0];
        assert_eq!(first["parameter"], parameter, "{shown}: {answer}");
        assert_ne!(first["code"], "internal", "{shown}: {answer}");
        assert!(
            answer.to_string().len() < 20_000,
            "{shown}: the refusal echoes its input"
        );
    }
    // A pad given a face plane: no member of a pad's, never taken for a sketch's.
    let pad = json!({"type": "pad", "name": "P", "sketch": "b0000001", "plane": {"face": "b0000001.box.Face6", "reversed": false}, "extent": {"type": "one_side", "length": 1, "reversed": false}});
    let answer = f.add(None, &pad);
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(
        answer["errors"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["code"] != "internal"),
        "{answer}"
    );
    assert_eq!((f.revision(), f.text()), (revision, text.clone()));
    // replace_text takes the document as a file is read: x is required there.
    let with_sketch = text.replace(
        "\"features\": [",
        &format!(
            "\"features\": [{}, ",
            serde_json::to_string(&without_x("s0000001", "b0000001.box.Face6")).unwrap()
        ),
    );
    assert_ne!(with_sketch, text);
    let replaced = f.call(
        "replace_text",
        &json!({"session": "s1", "revision": revision, "text": with_sketch}),
    );
    assert_eq!(replaced["ok"], false, "{replaced}");
    assert_eq!(replaced["errors"][0]["parameter"], "plane.x", "{replaced}");
    assert_eq!((f.revision(), f.text()), (revision, text));
}

/// The canonical plane of a face, as the document writes it.
fn face_plane(face: &str, x: &str, reversed: bool) -> String {
    format!(r#""plane": {{"face": "{face}", "x": "{x}", "reversed": {reversed}}}"#)
}

#[test]
fn set_field_sets_a_sketchs_plane_whole_and_a_face_planes_members_each_written_as_given() {
    let f = opened_on(
        planar_boxes(),
        &[cube(), rectangle("s0000001", "xy", [1.0, 1.0], 4.0, 2.0)],
    );
    assert_eq!(f.call("begin", &json!({"session": "s1"}))["ok"], true);
    // The file as written isn't in canonical form, and an edit's text is: an edit of the
    // name first, so each text after is the one before it with only the plane changed.
    assert_eq!(f.set("s0000001", "name", json!("Top"))["ok"], true);
    let flat = f.text();
    let on_xy = r#""plane": "xy""#;
    assert!(flat.contains(on_xy), "{flat}");
    // Each step's text is the one before with the plane alone written anew.
    let steps: [(&str, Value, Value, String); 5] = [
        (
            "plane",
            json!({"face": "b0000001.box.Face6", "x": "+x", "reversed": false}),
            json!("xy"),
            face_plane("b0000001.box.Face6", "+x", false),
        ),
        (
            "plane.x",
            json!("+y"),
            json!("+x"),
            face_plane("b0000001.box.Face6", "+y", false),
        ),
        (
            "plane.face",
            json!("b0000001.box.Face5"),
            json!("b0000001.box.Face6"),
            face_plane("b0000001.box.Face5", "+y", false),
        ),
        (
            "plane.reversed",
            json!(true),
            json!(false),
            face_plane("b0000001.box.Face5", "+y", true),
        ),
        (
            "plane",
            json!("xy"),
            json!({"face": "b0000001.box.Face5", "x": "+y", "reversed": true}),
            on_xy.to_owned(),
        ),
    ];
    for (field, value, before, plane) in steps {
        let set = f.set("s0000001", field, value.clone());
        assert_eq!(set["ok"], true, "{field}: {set}");
        assert_eq!(set["result"]["before"], before, "{field}");
        assert_eq!(set["result"]["after"], value, "{field}");
        assert_eq!(set["result"]["changed"], true, "{field}");
        assert_eq!(f.text(), flat.replace(on_xy, &plane), "{field}");
    }
    assert_eq!(f.text(), flat);
}

#[test]
fn set_field_refuses_a_face_planes_member_of_an_origin_plane_and_a_malformed_plane() {
    let f = opened_on(
        planar_boxes(),
        &[cube(), rectangle("s0000001", "xy", [1.0, 1.0], 4.0, 2.0)],
    );
    let (revision, text) = (f.revision(), f.text());
    for member in ["x", "face", "reversed"] {
        let field = format!("plane.{member}");
        let answer = f.set("s0000001", &field, json!("+y"));
        f.refused(
            &answer,
            &json!({
                "code": "invalid_input",
                "message": format!("plane.{member} is a face plane's, and sketch s0000001 is on the origin plane xy, which has none"),
                "feature": "s0000001",
                "elements": [],
                "elements_total": 0,
                "parameter": "field",
                "cause": null,
                "suggestion": r#"set plane whole: to a face plane, {"face": "<its name>", "x": "+x", "reversed": false}, or to xy, xz, or yz"#,
                "candidates": [],
                "advice": null,
            }),
            revision,
            &text,
        );
    }
    // A face plane without x (only add_feature fills it in), with an x that isn't a token,
    // with a member it hasn't, and a plane of the wrong kind: each refused at its path.
    for (value, parameter) in [
        (
            json!({"face": "b0000001.box.Face6", "reversed": false}),
            "plane.x",
        ),
        (
            json!({"face": "b0000001.box.Face6", "x": null, "reversed": false}),
            "plane.x",
        ),
        (
            json!({"face": "b0000001.box.Face6", "x": "+w", "reversed": false}),
            "plane.x",
        ),
        (
            json!({"face": "b0000001.box.Face6", "x": "+x", "reversed": false, "y": 1}),
            "plane.y",
        ),
        (json!(5), "plane"),
        (json!("uv"), "plane"),
    ] {
        let answer = f.set("s0000001", "plane", value.clone());
        assert_eq!(answer["ok"], false, "{value}: {answer}");
        assert_eq!(
            answer["errors"][0]["parameter"], parameter,
            "{value}: {answer}"
        );
        assert_eq!(
            (f.revision(), f.text()),
            (revision, text.clone()),
            "{value}"
        );
    }
    // On a face plane: a member it hasn't is refused with its members among the fields.
    let set = f.set(
        "s0000001",
        "plane",
        json!({"face": "b0000001.box.Face6", "x": "+x", "reversed": false}),
    );
    assert_eq!(set["ok"], true, "{set}");
    let answer = f.set("s0000001", "plane.y", json!("+x"));
    assert_eq!(answer["ok"], false, "{answer}");
    assert_eq!(
        answer["errors"][0]["message"],
        "a sketch has no field plane.y"
    );
    let candidates: Vec<&str> = answer["errors"][0]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .filter(|c| c.starts_with("plane"))
        .collect();
    assert_eq!(
        candidates,
        ["plane", "plane.face", "plane.x", "plane.reversed"]
    );
    // And its x set to a value that isn't a token, refused at plane.x.
    let answer = f.set("s0000001", "plane.x", json!("+w"));
    assert_eq!(answer["ok"], false, "{answer}");
    assert_eq!(answer["errors"][0]["parameter"], "plane.x", "{answer}");
}
