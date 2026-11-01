//! M5.9's part 3 as MCP tools: `add_feature` of a sketch on a face that leaves out its
//! plane's `x`, the core writing the face's `default_x` (M4.1 section 5), and `set_field` of
//! the plane, each called by its tool and answered with the core's envelope byte for byte,
//! holding to its tool's schemas in the committed `tools.json`. The fake kernel describes a
//! box's six faces as OCCT's box's planes (docs/ports/attacher.md, *Worked frames*), so each
//! token expected is that table's, written out by hand; sketches are solved as stored.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Call, Sessions};
use silico_core::fake::{FakeKernel, FakeRenderer, FakeSketches};
use silico_core::kernel::{Op, Surface};
use silico_core::sketch::SketchSolver;
use silico_protocol::Reply;

use crate::client::{Client, HANG, Reply as ToolReply};
use crate::contract::holds;

/// A 10 mm cube at the origin.
const CUBE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "b0000001", "type": "box", "v": 1, "name": "Cube", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}]}"#;

/// `kernel` describing each box's faces as planes, by number, as OCCT's box's: Face1 facing
/// −X, Face2 +X, Face3 −Y, Face4 +Y, Face5 −Z, Face6 +Z.
fn planar_boxes(kernel: &FakeKernel) {
    kernel.answer_instead(|op| {
        let Op::DescribeAll { shape } = op else {
            return None;
        };
        if !FakeKernel::describe(shape)?.starts_with("box") {
            return None;
        }
        let mut description = FakeKernel::described(shape)?;
        let normals = [
            [-1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, -1.0],
            [0.0, 0.0, 1.0],
        ];
        for (face, normal) in description.faces.iter_mut().zip(normals) {
            face.surface = Surface::Plane {
                normal,
                location: [0.0; 3],
            };
        }
        Some(Reply::DescribedAll(description))
    });
}

/// A 1 × 1 square sketch `id` on the plane `plane` (its text).
fn sketch(id: &str, plane: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "sketch", "name": "Square", "plane": {plane}, "points": [{{"id": "pa000001", "x": 1, "y": 1, "construction": false}}, {{"id": "pa000002", "x": 2, "y": 1, "construction": false}}, {{"id": "pa000003", "x": 2, "y": 2, "construction": false}}], "lines": [{{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}}, {{"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}}, {{"id": "na000003", "from": "pa000003", "to": "pa000001", "construction": false}}], "circles": [], "arcs": [], "constraints": []}}"#
    )
}

fn calls() -> Vec<(&'static str, String)> {
    let add = |revision: u64, feature: String| {
        format!(
            r#"{{"session": "s1", "revision": {revision}, "before": null, "feature": {feature}}}"#
        )
    };
    vec![
        ("open", r#"{"path": "part.silico", "create": false}"#.to_owned()),
        // Face2, +X: `+y`; Face4, +Y, its x null: `-x`.
        (
            "add_feature",
            add(
                1,
                sketch(
                    "s0000001",
                    r#"{"face": "b0000001.box.Face2", "reversed": false}"#,
                ),
            ),
        ),
        (
            "add_feature",
            add(
                2,
                sketch(
                    "s0000002",
                    r#"{"face": "b0000001.box.Face4", "x": null, "reversed": true}"#,
                ),
            ),
        ),
        // Face7: no such face, refused with Face6, its nearest by number.
        (
            "add_feature",
            add(
                3,
                sketch(
                    "s0000003",
                    r#"{"face": "b0000001.box.Face7", "reversed": false}"#,
                ),
            ),
        ),
        // The first sketch moved to the top, by set_field of its plane, then its x alone.
        (
            "set_field",
            r#"{"session": "s1", "revision": 3, "feature": "s0000001", "field": "plane", "value": {"face": "b0000001.box.Face6", "x": "+x", "reversed": false}}"#
                .to_owned(),
        ),
        (
            "set_field",
            r#"{"session": "s1", "revision": 4, "feature": "s0000001", "field": "plane.x", "value": "-y"}"#
                .to_owned(),
        ),
        // A face plane without x, which only add_feature fills in.
        (
            "set_field",
            r#"{"session": "s1", "revision": 5, "feature": "s0000001", "field": "plane", "value": {"face": "b0000001.box.Face6", "reversed": false}}"#
                .to_owned(),
        ),
    ]
}

/// The committed `tools.json`'s tools, by name.
fn tool(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/contract/tools.json");
    let tools: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("no tool {name}"))
        .clone()
}

#[test]
fn a_face_sketch_added_without_x_and_its_plane_set_answer_as_the_core_does_and_hold_to_their_tools()
{
    let calls = calls();
    // The core's own answers, on sessions of their own.
    let dir = tempfile::tempdir().unwrap();
    let work = std::fs::canonicalize(dir.path()).unwrap().join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::write(work.join("part.silico"), CUBE).unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    planar_boxes(&kernel);
    let sessions = Sessions::with_stand_ins(
        kernel,
        move |c: &std::path::Path| Cache::with_secret(c, &secret),
        || Ok(Box::new(FakeSketches::new()) as Box<dyn SketchSolver>),
    )
    .rendering(FakeRenderer::new())
    .in_workspace(&work)
    .unwrap();
    let core: Vec<(String, bool)> = calls
        .iter()
        .map(|(name, arguments)| match sessions.call(name, arguments) {
            Call::Running(calling) => {
                let answered = calling.wait_timeout(HANG).ok().unwrap();
                (answered.text, !answered.ok)
            }
            Call::Unknown => panic!("{name}"),
        })
        .collect();
    let mut c = Client::initialized_solving(FakeSketches::new());
    planar_boxes(&c.kernel);
    c.file("part.silico", CUBE);
    let mut answers = Vec::new();
    for (at, ((name, arguments), core)) in calls.iter().zip(&core).enumerate() {
        let id = at as u64 + 1;
        c.send_call(id, name, arguments);
        let (line, _) = c.reply_line(&id.to_string());
        let reply: ToolReply<'_> =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
        let answer = reply.result;
        assert_eq!(answer.content.len(), 1, "{line}");
        assert_eq!(
            answer.content[0].text.as_deref(),
            Some(answer.structured.get())
        );
        let text = answer.structured.get().to_owned();
        assert_eq!(
            (&text, answer.is_error),
            (&core.0, core.1),
            "call {at}: {name}"
        );
        let envelope: Value = serde_json::from_str(&text).unwrap();
        holds(&tool(name)["outputSchema"], &envelope, name);
        holds(
            &tool(name)["inputSchema"],
            &serde_json::from_str(arguments).unwrap(),
            name,
        );
        answers.push(envelope);
    }
    let ran: Vec<bool> = answers.iter().map(|a| a["ok"] == true).collect();
    assert_eq!(ran, [true, true, true, false, true, true, false]);
    assert_eq!(
        answers[1]["result"]["feature"]["plane"],
        json!({"face": "b0000001.box.Face2", "x": "+y", "reversed": false})
    );
    assert_eq!(
        answers[2]["result"]["feature"]["plane"],
        json!({"face": "b0000001.box.Face4", "x": "-x", "reversed": true})
    );
    let broken = &answers[3]["errors"][0];
    assert_eq!(broken["code"], "reference_broken");
    assert_eq!(broken["parameter"], "plane.face");
    assert_eq!(broken["candidates"], json!(["b0000001.box.Face6"]));
    assert_eq!(answers[3]["session"]["revision"], 3);
    assert_eq!(
        answers[4]["result"]["after"],
        json!({"face": "b0000001.box.Face6", "x": "+x", "reversed": false})
    );
    assert_eq!(answers[5]["result"]["before"], "+x");
    assert_eq!(answers[5]["result"]["after"], "-y");
    assert_eq!(answers[6]["errors"][0]["parameter"], "plane.x");
    // The file holds each as the last edit wrote it.
    let file = c.read_file("part.silico");
    assert!(
        file.contains(r#""plane": {"face": "b0000001.box.Face6", "x": "-y", "reversed": false}"#),
        "{file}"
    );
    assert!(
        file.contains(r#""plane": {"face": "b0000001.box.Face4", "x": "-x", "reversed": true}"#),
        "{file}"
    );
}
