//! The sketch edits' tools (docs/design/m5.0-app.md, section 7, *The sketch edits*):
//! `sketch_add`, `sketch_remove`, `sketch_split`, `sketch_trim`, `sketch_unjoin`, and
//! `sketch_merge`, each listed, callable, and answering with the core's one-line envelope
//! byte for byte, its edits and its refusals alike. Expected values: the same calls on the
//! core's `Sessions::call` directly, in a workspace of their own; and the members added and
//! removed, by hand from the design's rules. Inside a transaction, so no rebuild starts; an
//! add or a remove asks the solver nothing, and an edit that writes geometry asks the fake
//! sketch solver for its own solve, on each side.

use std::sync::Arc;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Call, Sessions};
use silico_core::fake::{FakeKernel, FakeRenderer, FakeSketches};
use silico_core::sketch::SketchSolver;

use crate::client::{Client, HANG, Reply};

/// A sketch of a line from the origin, and a box: what the edits edit.
const LINE: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "b7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 3, "construction": false}], "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}], "circles": [], "arcs": [], "constraints": []}]}"#;

/// The calls, each `(name, arguments)`: a line added from the line's end, with a
/// coincidence-free horizontal; the first line removed, its start with it (no other curve
/// uses it) and its end kept (the new line's); and refusals: the origin, a point a line
/// uses, a box named as the sketch, a member that isn't an ID, and a stale revision.
const CALLS: [(&str, &str); 9] = [
    ("open", r#"{"path": "part.silico", "create": false}"#),
    ("begin", r#"{"session": "s1"}"#),
    (
        "sketch_add",
        r#"{"session": "s1", "revision": 1, "sketch": "k7f2qx3m", "points": [{"id": "p0000003", "x": 20, "y": 3, "construction": false}], "lines": [{"id": "n0000002", "from": "p0000002", "to": "p0000003", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "c0000001", "type": "horizontal", "line": "n0000002"}]}"#,
    ),
    (
        "sketch_remove",
        r#"{"session": "s1", "revision": 2, "sketch": "k7f2qx3m", "members": ["n0000001"]}"#,
    ),
    (
        "sketch_remove",
        r#"{"session": "s1", "revision": 3, "sketch": "k7f2qx3m", "members": ["origin"]}"#,
    ),
    (
        "sketch_remove",
        r#"{"session": "s1", "revision": 3, "sketch": "k7f2qx3m", "members": ["p0000002"]}"#,
    ),
    (
        "sketch_add",
        r#"{"session": "s1", "revision": 3, "sketch": "b7f2qx3m", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}"#,
    ),
    (
        "sketch_remove",
        r#"{"session": "s1", "revision": 3, "sketch": "k7f2qx3m", "members": [7]}"#,
    ),
    (
        "sketch_add",
        r#"{"session": "s1", "revision": 1, "sketch": "k7f2qx3m", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}"#,
    ),
];

/// The answer's `structuredContent` as written, checked to be `content`'s one text block's
/// text, byte for byte; and `isError`.
fn structured(line: &str) -> (String, bool) {
    let reply: Reply<'_> = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}"));
    let answer = reply.result;
    assert_eq!(answer.content.len(), 1, "{line}");
    assert_eq!(answer.content[0].kind, "text");
    assert_eq!(
        answer.content[0].text.as_deref(),
        Some(answer.structured.get())
    );
    (answer.structured.get().to_owned(), answer.is_error)
}

#[test]
fn each_sketch_edit_is_a_tool_whose_answer_is_the_cores_envelope_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let work = std::fs::canonicalize(dir.path()).unwrap().join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::write(work.join("part.silico"), LINE).unwrap();
    let secret = dir.path().join("secret");
    let sessions = Sessions::with_cache_opener(Arc::new(FakeKernel::new(2).unwrap()), move |c| {
        Cache::with_secret(c, &secret)
    })
    .rendering(FakeRenderer::new())
    .in_workspace(&work)
    .unwrap();
    let core: Vec<(String, bool)> = CALLS
        .iter()
        .map(|(name, arguments)| match sessions.call(name, arguments) {
            Call::Running(calling) => {
                let answered = calling.wait_timeout(HANG).ok().unwrap();
                (answered.text, !answered.ok)
            }
            Call::Unknown => panic!("{name}"),
        })
        .collect();
    let mut c = Client::initialized();
    c.file("part.silico", LINE);
    for (at, ((name, arguments), core)) in CALLS.iter().zip(&core).enumerate() {
        let id = at as u64 + 1;
        c.send_call(id, name, arguments);
        let (line, _) = c.reply_line(&id.to_string());
        let (text, is_error) = structured(&line);
        assert_eq!((&text, is_error), (&core.0, core.1), "{name} {arguments}");
    }
    // What each said, by hand.
    let envelope = |n: usize| serde_json::from_str::<Value>(&core[n].0).unwrap();
    assert_eq!(
        envelope(2)["result"],
        json!({"sketch": "k7f2qx3m", "added": ["p0000003", "n0000002", "c0000001"], "modified": [], "removed": [], "changed": true, "commit": null})
    );
    assert_eq!(
        envelope(3)["result"],
        json!({"sketch": "k7f2qx3m", "added": [], "modified": [], "removed": ["p0000001", "n0000001"], "changed": true, "commit": null})
    );
    let first = |n: usize| {
        let e = envelope(n);
        assert_eq!(e["ok"], false, "{e}");
        (
            e["errors"][0]["code"].as_str().unwrap().to_owned(),
            e["errors"][0]["parameter"].as_str().unwrap().to_owned(),
            e["errors"][0]["message"].as_str().unwrap().to_owned(),
        )
    };
    let refused = |code: &str, parameter: &str, message: &str| {
        (code.to_owned(), parameter.to_owned(), message.to_owned())
    };
    assert_eq!(
        first(4),
        refused(
            "invalid_input",
            "members.0",
            "origin is one of the sketch's fixed references, which can't be removed"
        )
    );
    assert_eq!(
        first(5),
        refused(
            "invalid_input",
            "members.0",
            "point p0000002 is used by line n0000002"
        )
    );
    assert_eq!(
        first(6),
        refused("invalid_input", "sketch", "b7f2qx3m is a box, not a sketch")
    );
    assert_eq!(
        first(7),
        refused(
            "invalid_input",
            "members.0",
            "members.0 must be a sketch member's ID, got 7"
        )
    );
    assert_eq!(
        first(8),
        refused(
            "conflict",
            "revision",
            "the edit was made against revision 1, and the session is at revision 3"
        )
    );
    // Nothing reached the file: the transaction holds the edits.
    assert_eq!(c.read_file("part.silico"), LINE);
}

#[test]
fn the_sketch_edit_tools_are_listed_as_edits() {
    let mut c = Client::initialized();
    let listed = c.request(1, "tools/list", "{}");
    let tools = listed["result"]["tools"].as_array().unwrap();
    for name in ["sketch_add", "sketch_remove"] {
        let tool = tools.iter().find(|t| t["name"] == name).unwrap();
        assert_eq!(
            (
                &tool["annotations"]["readOnlyHint"],
                &tool["annotations"]["destructiveHint"]
            ),
            (&json!(false), &json!(true)),
            "{name}"
        );
        assert_eq!(
            tool["inputSchema"]["required"],
            match name {
                "sketch_add" => json!([
                    "arcs",
                    "circles",
                    "constraints",
                    "lines",
                    "points",
                    "revision",
                    "session",
                    "sketch"
                ]),
                _ => json!(["members", "revision", "session", "sketch"]),
            },
            "{name}"
        );
    }
}

/// The L of a sketch's two lines meeting at `p0000002`, a lone point beside the corner
/// (4e-8 mm off it), and a box: what the geometry edits edit.
const ELL: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "b7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}, {"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 0, "construction": false}, {"id": "p0000003", "x": 10, "y": 10, "construction": false}], "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}, {"id": "n0000002", "from": "p0000002", "to": "p0000003", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "c0000001", "type": "horizontal", "line": "n0000001"}]}]}"#;

/// The calls, each `(name, arguments)`: the open's rebuild waited for; a split of the first
/// line at its middle; its second
/// piece unjoined from the corner, and merged back; that piece trimmed (nothing cuts it but
/// its neighbours, at its ends: removed whole); and refusals: a split off the line, an
/// unjoin of a point one curve uses, a merge of a line's two ends, and a malformed trim.
const GEOMETRY_CALLS: [(&str, &str); 11] = [
    ("open", r#"{"path": "part.silico", "create": false}"#),
    (
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    ),
    ("begin", r#"{"session": "s1"}"#),
    (
        "sketch_split",
        r#"{"session": "s1", "revision": 1, "sketch": "k7f2qx3m", "curve": "n0000001", "at": [[5, 0]], "ids": ["p0000004", "n0000003", "c0000002"]}"#,
    ),
    (
        "sketch_unjoin",
        r#"{"session": "s1", "revision": 2, "sketch": "k7f2qx3m", "point": "p0000002", "curve": "n0000003", "ids": ["p0000005"]}"#,
    ),
    (
        "sketch_merge",
        r#"{"session": "s1", "revision": 3, "sketch": "k7f2qx3m", "points": ["p0000002", "p0000005"]}"#,
    ),
    (
        "sketch_trim",
        r#"{"session": "s1", "revision": 4, "sketch": "k7f2qx3m", "curve": "n0000003", "pick": [7, 0], "ids": []}"#,
    ),
    (
        "sketch_split",
        r#"{"session": "s1", "revision": 5, "sketch": "k7f2qx3m", "curve": "n0000002", "at": [[11, 5]], "ids": null}"#,
    ),
    (
        "sketch_unjoin",
        r#"{"session": "s1", "revision": 5, "sketch": "k7f2qx3m", "point": "p0000003", "curve": "n0000002", "ids": null}"#,
    ),
    (
        "sketch_merge",
        r#"{"session": "s1", "revision": 5, "sketch": "k7f2qx3m", "points": ["p0000001", "p0000004"]}"#,
    ),
    (
        "sketch_trim",
        r#"{"session": "s1", "revision": 5, "sketch": "k7f2qx3m", "curve": "n0000002", "pick": [10], "ids": null}"#,
    ),
];

#[test]
fn each_geometry_edit_is_a_tool_whose_answer_is_the_cores_envelope_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let work = std::fs::canonicalize(dir.path()).unwrap().join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::write(work.join("part.silico"), ELL).unwrap();
    let secret = dir.path().join("secret");
    let solver = FakeSketches::new();
    let solving = solver.clone();
    let sessions = Sessions::with_stand_ins(
        Arc::new(FakeKernel::new(2).unwrap()),
        move |c: &std::path::Path| Cache::with_secret(c, &secret),
        move || Ok(Box::new(solving.clone()) as Box<dyn SketchSolver>),
    )
    .rendering(FakeRenderer::new())
    .in_workspace(&work)
    .unwrap();
    let core: Vec<(String, bool)> = GEOMETRY_CALLS
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
    c.file("part.silico", ELL);
    for (at, ((name, arguments), core)) in GEOMETRY_CALLS.iter().zip(&core).enumerate() {
        let id = at as u64 + 1;
        c.send_call(id, name, arguments);
        let (line, _) = c.reply_line(&id.to_string());
        let (text, is_error) = structured(&line);
        assert_eq!((&text, is_error), (&core.0, core.1), "{name} {arguments}");
    }
    // What each said, by hand.
    let envelope = |n: usize| serde_json::from_str::<Value>(&core[n].0).unwrap();
    assert_eq!(
        envelope(3)["result"],
        json!({"sketch": "k7f2qx3m", "pieces": ["n0000001", "n0000003"], "added": ["p0000004", "n0000003", "c0000002"], "modified": ["n0000001"], "removed": [], "references": [], "changed": true, "commit": null})
    );
    assert_eq!(
        envelope(4)["result"],
        json!({"sketch": "k7f2qx3m", "added": ["p0000005"], "modified": ["n0000003"], "removed": [], "changed": true, "commit": null})
    );
    assert_eq!(
        envelope(5)["result"],
        json!({"sketch": "k7f2qx3m", "added": [], "modified": ["n0000003"], "removed": ["p0000005"], "changed": true, "commit": null})
    );
    assert_eq!(
        envelope(6)["result"],
        json!({"sketch": "k7f2qx3m", "pieces": [], "added": [], "modified": [], "removed": ["n0000003", "c0000002"], "references": [], "changed": true, "commit": null})
    );
    // The core's side asked its solver once for the open's rebuild, which `check` waited for,
    // and once for each edit's solve: each clean, so none asked for the sketch's solve
    // before it.
    assert_eq!(solver.requests().len(), 5);
    let first = |n: usize| {
        let e = envelope(n);
        assert_eq!(e["ok"], false, "{e}");
        (
            e["errors"][0]["parameter"].as_str().unwrap().to_owned(),
            e["errors"][0]["message"].as_str().unwrap().to_owned(),
        )
    };
    assert_eq!(
        first(7),
        (
            "at.0".to_owned(),
            "the split position is 1 mm from line n0000002, farther than 1e-7 mm".to_owned()
        )
    );
    assert_eq!(
        first(8),
        (
            "point".to_owned(),
            "point p0000003 is used by no curve but line n0000002, so there's nothing to unjoin"
                .to_owned()
        )
    );
    assert_eq!(
        first(9),
        (
            "points".to_owned(),
            "merging p0000001 and p0000004 would give line n0000001 one point at both ends"
                .to_owned()
        )
    );
    assert_eq!(
        first(10),
        (
            "pick".to_owned(),
            "pick must be a position, two numbers [x, y] in millimetres, got 1 of them".to_owned()
        )
    );
    // Nothing reached the file: the transaction holds the edits.
    assert_eq!(c.read_file("part.silico"), ELL);
}

/// The join's calls: the open's rebuild waited for; a split of the first line at (5, 0);
/// its join at the split point, back as it was; and refusals: the corner, which no
/// tangent_at holds on one line, a point that isn't the sketch's, and a malformed point.
const JOIN_CALLS: [(&str, &str); 8] = [
    ("open", r#"{"path": "part.silico", "create": false}"#),
    (
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    ),
    ("begin", r#"{"session": "s1"}"#),
    (
        "sketch_split",
        r#"{"session": "s1", "revision": 1, "sketch": "k7f2qx3m", "curve": "n0000001", "at": [[5, 0]], "ids": ["p0000004", "n0000003", "c0000002"]}"#,
    ),
    (
        "sketch_join",
        r#"{"session": "s1", "revision": 2, "sketch": "k7f2qx3m", "point": "p0000004"}"#,
    ),
    (
        "sketch_join",
        r#"{"session": "s1", "revision": 3, "sketch": "k7f2qx3m", "point": "p0000002"}"#,
    ),
    (
        "sketch_join",
        r#"{"session": "s1", "revision": 3, "sketch": "k7f2qx3m", "point": "p0000009"}"#,
    ),
    (
        "sketch_join",
        r#"{"session": "s1", "revision": 3, "sketch": "k7f2qx3m", "point": 5}"#,
    ),
];

#[test]
fn the_join_is_a_tool_whose_answer_is_the_cores_envelope_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let work = std::fs::canonicalize(dir.path()).unwrap().join("work");
    std::fs::create_dir(&work).unwrap();
    std::fs::write(work.join("part.silico"), ELL).unwrap();
    let secret = dir.path().join("secret");
    let solver = FakeSketches::new();
    let solving = solver.clone();
    let sessions = Sessions::with_stand_ins(
        Arc::new(FakeKernel::new(2).unwrap()),
        move |c: &std::path::Path| Cache::with_secret(c, &secret),
        move || Ok(Box::new(solving.clone()) as Box<dyn SketchSolver>),
    )
    .rendering(FakeRenderer::new())
    .in_workspace(&work)
    .unwrap();
    let core: Vec<(String, bool)> = JOIN_CALLS
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
    c.file("part.silico", ELL);
    for (at, ((name, arguments), core)) in JOIN_CALLS.iter().zip(&core).enumerate() {
        let id = at as u64 + 1;
        c.send_call(id, name, arguments);
        let (line, _) = c.reply_line(&id.to_string());
        let (text, is_error) = structured(&line);
        assert_eq!((&text, is_error), (&core.0, core.1), "{name} {arguments}");
    }
    // What each said, by hand: the split's pieces joined, n0000003, p0000004, and the
    // tangent_at c0000002 removed.
    let envelope = |n: usize| serde_json::from_str::<Value>(&core[n].0).unwrap();
    assert_eq!(
        envelope(4)["result"],
        json!({"sketch": "k7f2qx3m", "added": [], "modified": ["n0000001"], "removed": ["p0000004", "n0000003", "c0000002"], "references": [], "changed": true, "commit": null})
    );
    assert_eq!(envelope(4)["session"]["revision"], 3);
    let first = |n: usize| {
        let e = envelope(n);
        assert_eq!(e["ok"], false, "{e}");
        (
            e["errors"][0]["parameter"].as_str().unwrap().to_owned(),
            e["errors"][0]["message"].as_str().unwrap().to_owned(),
        )
    };
    assert_eq!(
        first(5),
        (
            "point".to_owned(),
            "lines n0000001 and n0000002 meet at point p0000002, but no tangent_at (same) there holds them on one line".to_owned()
        )
    );
    assert_eq!(
        first(6),
        (
            "point".to_owned(),
            "the sketch has no point p0000009".to_owned()
        )
    );
    assert_eq!(
        first(7),
        (
            "point".to_owned(),
            "point must be a point's ID, as the sketch holds it, got 5".to_owned()
        )
    );
    // The open's rebuild, the split's solve, and the join's, each clean.
    assert_eq!(solver.requests().len(), 3);
    // Nothing reached the file: the transaction holds the edits.
    assert_eq!(c.read_file("part.silico"), ELL);
}

#[test]
fn the_geometry_edit_tools_are_listed_as_edits() {
    let mut c = Client::initialized();
    let listed = c.request(1, "tools/list", "{}");
    let tools = listed["result"]["tools"].as_array().unwrap();
    for (name, required) in [
        (
            "sketch_split",
            json!(["at", "curve", "ids", "revision", "session", "sketch"]),
        ),
        (
            "sketch_trim",
            json!(["curve", "ids", "pick", "revision", "session", "sketch"]),
        ),
        (
            "sketch_unjoin",
            json!(["curve", "ids", "point", "revision", "session", "sketch"]),
        ),
        (
            "sketch_merge",
            json!(["points", "revision", "session", "sketch"]),
        ),
        (
            "sketch_join",
            json!(["point", "revision", "session", "sketch"]),
        ),
    ] {
        let tool = tools.iter().find(|t| t["name"] == name).unwrap();
        assert_eq!(
            (
                &tool["annotations"]["readOnlyHint"],
                &tool["annotations"]["destructiveHint"]
            ),
            (&json!(false), &json!(true)),
            "{name}"
        );
        assert_eq!(tool["inputSchema"]["required"], required, "{name}");
    }
}
