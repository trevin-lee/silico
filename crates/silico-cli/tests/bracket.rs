//! The bracket, M4.9's done test (docs/design/m4.9-mcp.md, *The bracket*): a scripted MCP
//! client starts the real `silico serve` in a temporary workspace and replays
//! `tests/bracket/session.jsonl`, a recorded agent session that designs the L-bracket of
//! `tests/bracket/spec.md`, rolls back one wrong edit, renders it, measures it, and
//! exports it, sending each line only after the reply to the one before.
//!
//! **Compared byte for byte:** every reply's `structuredContent` with the recording's, but
//! what's never compared: a PNG's bytes, `render`'s `png_bytes`, progress notifications,
//! and `initialize`'s `serverInfo.version` and the workspace's absolute path in its
//! `instructions` (the session uses only relative paths, so nothing else quotes it). Each
//! reply also holds to its tool's `outputSchema` in silico-mcp's committed `tools.json`.
//!
//! **Checked by hand,** never read from the recording: the expected values below are the
//! spec's dimensions worked out on paper (prisms, cylinders, the fillet's spandrel), each
//! derivation beside its assert. The 3MF is read here from its bytes and checked as M4.8
//! promises a printable mesh (closed, manifold, consistently oriented, no triangle with two
//! corners at one place, enclosing a positive volume), its genus that of a part with four
//! through holes, its chords counted by hand from OCCT's rule (126 points a rim and 33 an
//! arc of the fillet at 0.1 mm: 1,084 vertices and 2,180 triangles), and its volume the
//! part's and the chord segments' (about 0.512 mm³) within the coordinates' 32-bit
//! rounding; and
//! FreeCAD 1.1.3 (the oracle) reads it back, makes a solid of it, and finds that solid
//! valid, with the mesh's volume.
//!
//! **M5.1's timeline commands** on the same bracket and the real kernel: its timeline, its
//! pockets swapped (`move_feature`), the first removed and added back `before` the sketch
//! that followed it, each result the design's example and each volume by hand.
//!
//! **Recording.** With `SILICO_BRACKET_RECORD=1` set, the session is first driven live by
//! [`record`], which plays the agent (each request built from the replies before it, as an
//! agent reads them), and `session.jsonl` is written from the real replies; the replay and
//! every check below then run on what was written. Build the workers first, then record:
//!
//! ```text
//! pixi run cargo build -p silico-kernel -p silico-sketch -p silico-render --bins
//! SILICO_BRACKET_RECORD=1 pixi run cargo nextest run -p silico-cli -E 'test(bracket)'
//! ```
//!
//! It lives in silico-cli's tests, not silico-mcp's, so the `silico` binary it serves with
//! comes from Cargo (`CARGO_BIN_EXE_silico`), built for every run that runs it. Its workers
//! are the ones built beside it, as silico-cli's other tests take them (`pixi run test`
//! builds them).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/run.rs"]
mod run;
#[path = "common/serve.rs"]
mod serve;
#[path = "common/threemf.rs"]
mod threemf;
// The bracket's features as its session added them, which silico-core's timeline edits'
// tests share.
#[path = "../../silico-core/tests/common/bracket.rs"]
mod features;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::Receiver;

use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

use serve::{ask_line, exited, lines, serve_working};
use threemf::{model, printable};

/// π, as `std::f64::consts::PI`.
const PI: f64 = std::f64::consts::PI;

/// The spec's hole radius (an M5 clearance hole, 5.5 mm across) and fillet radius.
const HOLE_R: f64 = 2.75;
const FILLET_R: f64 = 5.0;

/// The export's tolerance, the session's (FreeCAD's default).
const TOLERANCE: f64 = 0.1;

fn session_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/bracket/session.jsonl")
}

/// Panics, naming `what`, unless `value` holds to `schema`.
fn holds(schema: &Value, value: &Value, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{what}: {errors:?}");
}

/// `silico serve` with the real workers in a temporary workspace of its own
/// ([`serve_working`]), its stderr the test's, its stdout's lines as they come.
struct Serve {
    _dir: tempfile::TempDir,
    work: PathBuf,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
}

impl Serve {
    fn start() -> Serve {
        let dir = tempfile::Builder::new()
            .prefix("silico-bracket-")
            .tempdir()
            .unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let work = root.join("work");
        std::fs::create_dir(&work).unwrap();
        let mut child = serve_working(&work, &root.join("cache-home"), Stdio::inherit());
        let stdin = child.stdin.take();
        let lines = lines(&mut child);
        Serve {
            _dir: dir,
            work,
            child: Some(child),
            stdin,
            lines,
        }
    }

    /// Sends `line`; then, if it's a request, gives the reply carrying its ID as written,
    /// every message before it (a progress notification, timing) skipped.
    fn send(&mut self, line: &str) -> Option<String> {
        let message: Value = serde_json::from_str(line).unwrap();
        let stdin = self.stdin.as_mut().unwrap();
        let Some(id) = message.get("id") else {
            writeln!(stdin, "{line}").unwrap();
            return None;
        };
        Some(ask_line(stdin, &self.lines, line, &id.to_string()))
    }

    /// Ends the server (its stdin closed), checking it exits 0.
    fn end(&mut self) {
        self.stdin = None;
        assert_eq!(exited(self.child.take().unwrap()).code(), Some(0));
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A reply to `tools/call`: its result, a tool's answer.
#[derive(Deserialize)]
struct Reply<'a> {
    #[serde(borrow)]
    result: Answer<'a>,
}

#[derive(Deserialize)]
struct Answer<'a> {
    #[serde(borrow, rename = "structuredContent")]
    structured: &'a RawValue,
    #[serde(rename = "isError")]
    is_error: bool,
}

/// `structuredContent`'s text as compared: `render`'s `png_bytes`, the size of an image a
/// GPU drew, never compared, its number said so.
fn compared(structured: &str) -> String {
    const KEY: &str = "\"png_bytes\": ";
    let Some(at) = structured.find(KEY) else {
        return structured.to_owned();
    };
    let start = at + KEY.len();
    let digits = structured[start..]
        .find(|c: char| !c.is_ascii_digit())
        .unwrap();
    assert!(digits > 0, "{structured}");
    format!(
        "{}\"not compared\"{}",
        &structured[..start],
        &structured[start + digits..]
    )
}

/// `initialize`'s result as the server writes it, but as compared: `serverInfo.version`,
/// the build's, and the workspace's absolute path in `instructions` put as `<version>` and
/// `<workspace>`.
fn compared_initialize(reply: &str, work: &Path) -> String {
    #[derive(Deserialize)]
    struct Initialized<'a> {
        #[serde(borrow)]
        result: &'a RawValue,
    }
    let text = serde_json::from_str::<Initialized>(reply)
        .unwrap()
        .result
        .get();
    let result: Value = serde_json::from_str(text).unwrap();
    let value = serde_json::to_string(&result["serverInfo"]["version"]).unwrap();
    // The key as the server writes it: compactly, from its structs.
    let version = format!("\"version\":{value}");
    assert!(text.contains(&version), "no serverInfo version in {text}");
    assert_eq!(text.matches(&version).count(), 1, "{text}");
    let masked = version.replace(&value, "\"<version>\"");
    let workspace = serde_json::to_string(work.to_str().unwrap()).unwrap();
    let workspace = workspace.trim_matches('"');
    assert!(text.contains(workspace), "{text}");
    text.replacen(&version, &masked, 1)
        .replace(workspace, "<workspace>")
}

/// One line of `session.jsonl`: what it's for, the JSON-RPC message the client sends, and
/// what's expected of its reply (a tool's `structuredContent` as compared, `initialize`'s
/// result as compared, or null for a notification).
#[derive(Deserialize)]
struct Line<'a> {
    note: String,
    #[serde(borrow)]
    send: &'a RawValue,
    #[serde(borrow)]
    expect: Option<&'a RawValue>,
}

/// What the replay saw: each tool called, by name, with its arguments and its answer.
struct Replayed {
    calls: Vec<(String, Value, Value)>,
    work: PathBuf,
    /// The PNG `render` sent beside its result, as base64 (never compared).
    images: Vec<String>,
    _serve: Serve,
}

impl Replayed {
    /// The answers (`structuredContent`) of every call of `tool`, in order.
    fn answers(&self, tool: &str) -> Vec<&Value> {
        self.calls
            .iter()
            .filter(|(name, _, _)| name == tool)
            .map(|(_, _, answer)| answer)
            .collect()
    }

    /// The arguments of every call of `tool`, in order.
    fn arguments(&self, tool: &str) -> Vec<&Value> {
        self.calls
            .iter()
            .filter(|(name, _, _)| name == tool)
            .map(|(_, arguments, _)| arguments)
            .collect()
    }
}

/// The committed `tools/list` reply's tools (silico-mcp's contract), by name.
fn tools() -> BTreeMap<String, Value> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../silico-mcp/tests/contract/tools.json");
    let list: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_owned(), t.clone()))
        .collect()
}

/// Replays `session.jsonl` on a new server, each line sent only after the reply to the one
/// before, every reply compared with the recording's.
fn replay() -> Replayed {
    let text = std::fs::read_to_string(session_file()).unwrap();
    let tools = tools();
    let mut serve = Serve::start();
    let mut calls = Vec::new();
    let mut images = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line: Line = serde_json::from_str(line).unwrap_or_else(|e| panic!("line {i}: {e}"));
        let step = format!("line {} ({})", i + 1, line.note);
        let sent: Value = serde_json::from_str(line.send.get()).unwrap();
        let reply = serve.send(line.send.get());
        let method = sent["method"].as_str().unwrap();
        match (method, reply) {
            ("initialize", Some(reply)) => {
                assert_eq!(
                    compared_initialize(&reply, &serve.work),
                    line.expect.unwrap().get(),
                    "{step}: the reply differs from the recording's"
                );
            }
            ("tools/call", Some(reply)) => {
                let answer: Reply =
                    serde_json::from_str(&reply).unwrap_or_else(|e| panic!("{step}: {e}: {reply}"));
                let structured = answer.result.structured.get();
                assert_eq!(
                    compared(structured),
                    line.expect.unwrap().get(),
                    "{step}: the reply differs from the recording's"
                );
                let envelope: Value = serde_json::from_str(structured).unwrap();
                assert_eq!(answer.result.is_error, envelope["ok"] == false, "{step}");
                let name = sent["params"]["name"].as_str().unwrap().to_owned();
                holds(&tools[&name]["outputSchema"], &envelope, &step);
                // The text block is the same envelope, as the design writes it.
                let whole: Value = serde_json::from_str(&reply).unwrap();
                let content = whole["result"]["content"].as_array().unwrap();
                assert_eq!(content[0]["type"], "text", "{step}");
                assert_eq!(content[0]["text"].as_str(), Some(structured), "{step}");
                for block in &content[1..] {
                    assert_eq!(block["type"], "image", "{step}");
                    assert_eq!(block["mimeType"], "image/png", "{step}");
                    images.push(block["data"].as_str().unwrap().to_owned());
                }
                calls.push((name, sent["params"]["arguments"].clone(), envelope));
            }
            (_, None) => assert!(line.expect.is_none(), "{step}: a notification has no reply"),
            (method, Some(reply)) => panic!("{step}: {method} isn't in the session: {reply}"),
        }
    }
    serve.end();
    let work = serve.work.clone();
    Replayed {
        calls,
        work,
        images,
        _serve: serve,
    }
}

// --- The session's requests, as the agent wrote them -------------------------------------

/// The L-profile on the XZ plane (the sketch's x along +X, its y along +Z): the base's
/// section 60 × 5 and the upright's 5 × 45 above it, one closed loop of six lines, fixed by
/// six horizontal and vertical constraints, its corner at the origin, and four lengths.
const PROFILE: &str = r#"{"id": "s0000001", "type": "sketch", "name": "Bracket profile", "plane": "xz", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 60, "y": 0, "construction": false}, {"id": "pa000003", "x": 60, "y": 5, "construction": false}, {"id": "pa000004", "x": 5, "y": 5, "construction": false}, {"id": "pa000005", "x": 5, "y": 50, "construction": false}, {"id": "pa000006", "x": 0, "y": 50, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000005", "construction": false}, {"id": "na000005", "from": "pa000005", "to": "pa000006", "construction": false}, {"id": "na000006", "from": "pa000006", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "horizontal", "line": "na000001"}, {"id": "ka000002", "type": "vertical", "line": "na000002"}, {"id": "ka000003", "type": "horizontal", "line": "na000003"}, {"id": "ka000004", "type": "vertical", "line": "na000004"}, {"id": "ka000005", "type": "horizontal", "line": "na000005"}, {"id": "ka000006", "type": "vertical", "line": "na000006"}, {"id": "ka000007", "type": "distance_x", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000008", "type": "distance_y", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000009", "type": "length", "line": "na000001", "value": 60, "driving": true}, {"id": "ka00000a", "type": "length", "line": "na000002", "value": 5, "driving": true}, {"id": "ka00000b", "type": "length", "line": "na000005", "value": 5, "driving": true}, {"id": "ka00000c", "type": "length", "line": "na000006", "value": 50, "driving": true}]}"#;

/// The pad of the profile, `length` along +Y: the XZ plane's normal is −Y, so `reversed`.
fn pad(length: u32) -> String {
    format!(
        r#"{{"id": "p0000001", "type": "pad", "name": "Legs", "sketch": "s0000001", "extent": {{"type": "one_side", "length": {length}, "reversed": true}}}}"#
    )
}

/// Two holes' circles, 5.5 across and 16 apart, the first's centre at `(x, y)` in the
/// sketch's coordinates and the second's `16` along `along` (`x` or `y`).
fn holes(id: &str, name: &str, plane: &str, (x, y): (u32, u32), along: &str) -> String {
    let (x2, y2, aligned) = match along {
        "x" => (x + 16, y, "horizontal_points"),
        _ => (x, y + 16, "vertical_points"),
    };
    let diameter = 2.0 * HOLE_R;
    format!(
        r#"{{"id": "{id}", "type": "sketch", "name": "{name}", "plane": "{plane}", "points": [{{"id": "pb000001", "x": {x}, "y": {y}, "construction": false}}, {{"id": "pb000002", "x": {x2}, "y": {y2}, "construction": false}}], "lines": [], "circles": [{{"id": "cb000001", "center": "pb000001", "radius": {HOLE_R}, "construction": false}}, {{"id": "cb000002", "center": "pb000002", "radius": {HOLE_R}, "construction": false}}], "arcs": [], "constraints": [{{"id": "kb000001", "type": "diameter", "curve": "cb000001", "value": {diameter}, "driving": true}}, {{"id": "kb000002", "type": "equal", "a": "cb000001", "b": "cb000002"}}, {{"id": "kb000003", "type": "distance_x", "a": "origin", "b": "pb000001", "value": {x}, "driving": true}}, {{"id": "kb000004", "type": "distance_y", "a": "origin", "b": "pb000001", "value": {y}, "driving": true}}, {{"id": "kb000005", "type": "{aligned}", "a": "pb000001", "b": "pb000002"}}, {{"id": "kb000006", "type": "distance_{along}", "a": "pb000001", "b": "pb000002", "value": 16, "driving": true}}]}}"#
    )
}

/// A pocket through everything of `base`, up from its sketch (the pocket's direction is
/// against the sketch's normal, so `reversed`: the XY plane's +Z, the YZ plane's +X).
fn pocket(id: &str, name: &str, base: &str, sketch: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "pocket", "name": "{name}", "base": "{base}", "sketch": "{sketch}", "extent": {{"type": "through_all", "reversed": true}}}}"#
    )
}

const INITIALIZE: &str = r#"{"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "bracket-agent", "version": "1"}}}"#;
const INITIALIZED: &str = r#"{"jsonrpc": "2.0", "method": "notifications/initialized"}"#;
const MASS: &str = r#"{"session": "s1", "model": "current", "feature": null}"#;
const CHECK: &str = r#"{"session": "s1", "model": "current", "fresh": false}"#;
const SESSION: &str = r#"{"session": "s1"}"#;

/// The agent, live: drives `serve` through the session, building each request from the
/// replies before it as an agent reads them (the revision its edit is made against, the
/// names `query` gives), and gives `session.jsonl`'s lines.
struct Recorder {
    serve: Serve,
    id: u64,
    lines: Vec<String>,
    revision: u64,
}

impl Recorder {
    /// Calls `tool` on `arguments`, recording the line and its answer; gives the answer.
    fn call(&mut self, note: &str, tool: &str, arguments: &str) -> Value {
        self.id += 1;
        let send = format!(
            r#"{{"jsonrpc": "2.0", "id": {}, "method": "tools/call", "params": {{"name": "{tool}", "arguments": {arguments}}}}}"#,
            self.id
        );
        let reply = self.serve.send(&send).unwrap();
        let answer: Reply = serde_json::from_str(&reply).unwrap();
        let structured = answer.result.structured.get();
        let envelope: Value = serde_json::from_str(structured).unwrap();
        assert_eq!(envelope["ok"], true, "{note}: {structured}");
        if let Some(revision) = envelope["session"]["revision"].as_u64() {
            self.revision = revision;
        }
        self.push(note, &send, Some(&compared(structured)));
        envelope
    }

    fn push(&mut self, note: &str, send: &str, expect: Option<&str>) {
        let note = serde_json::to_string(note).unwrap();
        self.lines.push(format!(
            r#"{{"note": {note}, "send": {send}, "expect": {}}}"#,
            expect.unwrap_or("null")
        ));
    }

    /// `add_feature` of `feature` (JSON text), against the session's revision.
    fn add(&mut self, note: &str, feature: &str) -> Value {
        let arguments = format!(
            r#"{{"session": "s1", "revision": {}, "before": null, "feature": {feature}}}"#,
            self.revision
        );
        self.call(note, "add_feature", &arguments)
    }
}

/// The agent's session, live, as `session.jsonl`'s lines.
fn record() -> Vec<String> {
    let mut r = Recorder {
        serve: Serve::start(),
        id: 0,
        lines: Vec::new(),
        revision: 0,
    };
    let reply = r.serve.send(INITIALIZE).unwrap();
    let expect = compared_initialize(&reply, &r.serve.work);
    r.push("initialize", INITIALIZE, Some(&expect));
    assert!(r.serve.send(INITIALIZED).is_none());
    r.push("initialized", INITIALIZED, None);
    r.call(
        "open: a new document for the bracket",
        "open",
        r#"{"path": "bracket.silico", "create": true}"#,
    );
    r.call("begin a transaction", "begin", SESSION);
    r.add("the L-profile, on the XZ plane", PROFILE);
    r.add("the wrong edit: the legs padded 4 deep, not 40", &pad(4));
    r.call("check the model: it builds, nothing to see", "check", CHECK);
    r.call(
        "measure it: 2,100 mm³, not the spec's 21,000 for the legs",
        "mass_properties",
        MASS,
    );
    r.call("roll the wrong edit back", "rollback", SESSION);
    r.call("begin again", "begin", SESSION);
    r.add("the L-profile again", PROFILE);
    r.add("the legs, padded 40 wide", &pad(40));
    r.call("measure the legs: 21,000 mm³", "mass_properties", MASS);
    r.add(
        "the base's holes, on the XY plane",
        &holes("s0000002", "Base holes", "xy", (40, 12), "y"),
    );
    r.add(
        "pocket them up through the base",
        &pocket("h0000001", "Base holes", "p0000001", "s0000002"),
    );
    r.add(
        "the upright's holes, on the YZ plane",
        &holes("s0000003", "Upright holes", "yz", (12, 35), "x"),
    );
    r.add(
        "pocket them through the upright",
        &pocket("h0000002", "Upright holes", "h0000001", "s0000003"),
    );
    r.call("measure the holes' cut", "mass_properties", MASS);
    let lines = r.call(
        "find the straight edges 40 long: the inside corner is the one at x = 5, z = 5",
        "query",
        r#"{"session": "s1", "model": "current", "feature": null, "kind": "edge", "type": "line", "stable": true, "facing": null, "measures": [{"measure": "length", "at_least": 40, "at_most": 40}], "order": null, "limit": 10}"#,
    );
    let corner = lines["result"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["start"] == serde_json::json!([5, 0, 5]))
        .unwrap()["name"]
        .clone();
    r.add(
        "fillet the inside corner, radius 5",
        &format!(
            r#"{{"id": "f0000001", "type": "fillet", "name": "Inside corner", "base": "h0000002", "edges": [{corner}], "radius": {FILLET_R}}}"#
        ),
    );
    r.call("commit: the file is saved", "commit", SESSION);
    r.call("check the saved model", "check", CHECK);
    r.call("measure the bracket", "mass_properties", MASS);
    let face = r.call(
        "find the base's end face: the smallest planar face facing +X",
        "query",
        r#"{"session": "s1", "model": "current", "feature": null, "kind": "face", "type": "plane", "stable": null, "facing": {"direction": [1, 0, 0], "within": 0}, "measures": [], "order": {"by": "area", "descending": false}, "limit": 1}"#,
    );
    let end = face["result"]["elements"][0]["name"].clone();
    let rims = r.call(
        "find the holes' rims: circles of radius 2.75",
        "query",
        &format!(
            r#"{{"session": "s1", "model": "current", "feature": null, "kind": "edge", "type": "circle", "stable": null, "facing": null, "measures": [{{"measure": "radius", "at_least": {HOLE_R}, "at_most": {HOLE_R}}}], "order": null, "limit": 8}}"#
        ),
    );
    let rim = rims["result"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["axis"]["point"] == serde_json::json!([40, 12, 5]))
        .unwrap()["name"]
        .clone();
    r.call(
        "measure the hole at (40, 12) from the base's end",
        "distance",
        &format!(
            r#"{{"session": "s1", "model": "current", "elements": [{rim}, {end}], "feature": null}}"#
        ),
    );
    r.call(
        "render it from the isometric corner, the measured rim and face highlighted",
        "render",
        &format!(
            r#"{{"session": "s1", "model": "current", "view": "top_front_right", "camera": null, "width": 800, "height": 600, "highlight": [{rim}, {end}], "to": "bracket.png", "replace": false}}"#
        ),
    );
    r.call(
        "export it for printing",
        "export",
        &format!(
            r#"{{"session": "s1", "format": "3mf", "path": "bracket.3mf", "replace": false, "features": null, "tolerance": {TOLERANCE}}}"#
        ),
    );
    r.call("close the session", "close", SESSION);
    r.serve.end();
    r.lines
}

// --- Hand calculations ---------------------------------------------------------------------

/// A body's mass properties, summed from parts: a box, a cylinder, or the fillet's
/// spandrel, each of its volume (negative for a hole) and its centroid.
#[derive(Default)]
struct Sum {
    volume: f64,
    moment: [f64; 3],
}

impl Sum {
    fn add(&mut self, volume: f64, centroid: [f64; 3]) {
        self.volume += volume;
        for (m, c) in self.moment.iter_mut().zip(centroid) {
            *m += volume * c;
        }
    }

    /// An axis-aligned box from `min` to `max`.
    fn cuboid(&mut self, min: [f64; 3], max: [f64; 3]) {
        let volume = (max[0] - min[0]) * (max[1] - min[1]) * (max[2] - min[2]);
        self.add(volume, [0, 1, 2].map(|i| (min[i] + max[i]) / 2.0));
    }

    fn centre(&self) -> [f64; 3] {
        self.moment.map(|m| m / self.volume)
    }
}

/// The legs, `width` along y: the base [0, 60] × [0, w] × [0, 5] and the upright
/// [0, 5] × [0, w] × [5, 50] on it.
fn legs(width: f64) -> Sum {
    let mut sum = Sum::default();
    sum.cuboid([0.0, 0.0, 0.0], [60.0, width, 5.0]);
    sum.cuboid([0.0, 0.0, 5.0], [5.0, width, 50.0]);
    sum
}

/// One hole: a cylinder of radius 2.75 through a 5 mm leg, π · 2.75² · 5 = 37.8125π.
const HOLE: f64 = PI * HOLE_R * HOLE_R * 5.0;

/// The legs less the four holes: the base's at x = 40, y = 12 and 28, z from 0 to 5; the
/// upright's at z = 35, y = 12 and 28, x from 0 to 5.
fn drilled() -> Sum {
    let mut sum = legs(40.0);
    for y in [12.0, 28.0] {
        sum.add(-HOLE, [40.0, y, 2.5]);
        sum.add(-HOLE, [2.5, y, 35.0]);
    }
    sum
}

/// The bracket: the drilled legs and the fillet's spandrel in the inside corner, the
/// square [5, 10] × [5, 10] (x, z) less the quarter disc of radius 5 about (10, 10), 40
/// along y. Its section is r²(1 − π/4); its centroid is d from the corner on each axis,
/// d = (r²·r/2 − (πr²/4)(r − 4r/(3π))) / (r²(1 − π/4)) = r(10 − 3π)/(12 − 3π) (the square's
/// moment less the quarter disc's, whose centroid is 4r/(3π) from its centre).
fn bracket() -> Sum {
    let mut sum = drilled();
    let r = FILLET_R;
    let d = r * (10.0 - 3.0 * PI) / (12.0 - 3.0 * PI);
    sum.add(r * r * (1.0 - PI / 4.0) * 40.0, [5.0 + d, 20.0, 5.0 + d]);
    sum
}

/// The measure's bound on a volume or area by hand: OCCT's integration to a relative 1e-9
/// (docs/ports/measure.md, `MEASURE_PRECISION`), then one rounding to 6 decimals.
fn measured(what: &str, got: &Value, hand: f64) {
    let got = got.as_f64().unwrap_or_else(|| panic!("{what}: {got}"));
    let bound = 1e-9 * hand.abs() + 5e-7;
    assert!(
        (got - hand).abs() <= bound,
        "{what}: measured {got}, by hand {hand} (bound {bound})"
    );
}

/// A centre of mass against the hand's, to the measure's bound (the moments and the volume
/// each to a relative 1e-9, the centre their quotient) and the nanometre it's rounded to.
fn centred(what: &str, got: &Value, hand: [f64; 3]) {
    for (i, h) in hand.into_iter().enumerate() {
        let g = got[i].as_f64().unwrap();
        let bound = 2e-9 * h.abs() + 5e-7;
        assert!(
            (g - h).abs() <= bound,
            "{what}[{i}]: measured {g}, by hand {h} (bound {bound})"
        );
    }
}

fn mass(answer: &Value) -> &Value {
    let bodies = answer["result"]["bodies"].as_array().unwrap();
    assert_eq!(bodies.len(), 1, "{answer}");
    &bodies[0]
}

// --- The export's mesh, by hand -------------------------------------------------------------

/// How many chords OCCT cuts an arc of `angle` radians and radius `r` into, meshing at the
/// tolerance `t` with FreeCAD's angular deflection `a` = min(0.1, 5t + 0.005) (FreeCAD's
/// `defaultAngularDeflection`, docs/ports/mesh-export.md, *Same as FreeCAD*): an edge is
/// meshed at half the face's deflections (`BRepMesh_CurveTessellator::init`, 0.5 × the
/// angular and the linear), and a circle in equal steps of at most min(2 acos(1 − (t/2)/r),
/// a/2) (`GCPnts_TangentialDeflection::PerformCircular` and `ArcAngularStep`), so in
/// ⌈angle / step⌉. Here the angle is the smaller, which the assert holds: so it's the
/// angle's rule alone, whatever the worker takes off the tolerance for rounding.
fn chords(angle: f64, r: f64, t: f64) -> usize {
    let a = (5.0 * t + 0.005).min(0.1);
    let linear = 2.0 * libm::acos(1.0 - t / 2.0 / r);
    assert!(linear > a / 2.0, "the linear rule binds at r {r}, t {t}");
    (angle / (a / 2.0)).ceil() as usize
}

/// How many of `vertices` lie on the circle of radius `r` about `centre` in the plane
/// `axis` = `at` (0, 1, 2 for x, y, z), and within `within` of it (a box about the arc).
fn on_circle(
    vertices: &[[f64; 3]],
    axis: usize,
    at: f64,
    centre: [f64; 3],
    r: f64,
    within: impl Fn(&[f64; 3]) -> bool,
) -> usize {
    vertices
        .iter()
        .filter(|v| v[axis] == at && within(v))
        .filter(|v| {
            let d: f64 = (0..3)
                .filter(|&i| i != axis)
                .map(|i| (v[i] - centre[i]) * (v[i] - centre[i]))
                .sum();
            (d.sqrt() - r).abs() < 1e-5
        })
        .count()
}

/// What FreeCAD 1.1.3 reads from the 3MF at `path`: the mesh's own checks, and the shape
/// made from it (`Part.Shape.makeShapeFromMesh`, then `Part.makeSolid`, as FreeCAD's
/// *Part from mesh* does), whether it's a valid solid, and its volume.
fn freecad_reads(path: &Path) -> Value {
    silico_oracle::freecad::script(&format!(
        r#"
import json, Mesh, Part
m = Mesh.Mesh({path:?})
shape = Part.Shape()
shape.makeShapeFromMesh(m.Topology, 0.01)
solid = Part.makeSolid(shape)
box = solid.BoundBox
print("SILICO_ORACLE " + json.dumps({{
    "mesh_solid": m.isSolid(),
    "non_manifold": m.hasNonManifolds(),
    "misoriented": m.countNonUniformOrientedFacets(),
    "points": m.CountPoints,
    "facets": m.CountFacets,
    "type": solid.ShapeType,
    "valid": solid.isValid(),
    "closed": solid.isClosed(),
    "shells": len(solid.Shells),
    "faces": len(solid.Faces),
    "volume": solid.Volume,
    "box": [box.XMin, box.YMin, box.ZMin, box.XMax, box.YMax, box.ZMax],
}}))
"#,
        path = path.to_str().unwrap()
    ))
    .unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn the_bracket_session_replays_measures_as_by_hand_and_its_3mf_reads_back_in_the_oracle_as_a_solid()
{
    if std::env::var_os("SILICO_BRACKET_RECORD").is_some() {
        let mut text = record().join("\n");
        text.push('\n');
        std::fs::write(session_file(), text).unwrap();
    }
    let run = replay();

    // The wrong edit: the L-profile's section is 60 · 5 + 5 · 45 = 525 mm², its perimeter
    // 60 + 5 + 55 + 45 + 5 + 50 = 220 mm; padded 4: 2,100 mm³, 2 · 525 + 220 · 4 = 1,930
    // mm², its centre 2 along y (the pad goes +Y from the XZ plane, reversed from its −Y
    // normal). It builds: `check` has nothing to say; the volume is what's wrong.
    let masses = run.answers("mass_properties");
    assert_eq!(masses.len(), 4);
    let wrong = mass(masses[0]);
    let thin = legs(4.0);
    measured("the wrong pad's volume", &wrong["volume"], thin.volume);
    assert_eq!(thin.volume, 2100.0);
    measured(
        "the wrong pad's area",
        &wrong["area"],
        2.0 * 525.0 + 220.0 * 4.0,
    );
    centred(
        "the wrong pad's centre",
        &wrong["center_of_mass"],
        thin.centre(),
    );
    let checks = run.answers("check");
    assert_eq!(checks.len(), 2);
    for check in &checks {
        assert_eq!(check["result"]["problems_total"], 0, "{check}");
        assert_eq!(check["result"]["warnings_total"], 0, "{check}");
    }
    // Rolled back to the transaction's base, the empty document: the file never held it.
    let begins = run.answers("begin");
    let rollback = run.answers("rollback")[0];
    assert_eq!(
        rollback["result"]["restored"], begins[0]["result"]["base"],
        "{rollback}"
    );
    assert_eq!(rollback["session"]["transaction"], false);

    // The legs, 40 wide: 12,000 + 9,000 = 21,000 mm³, 2 · 525 + 220 · 40 = 9,850 mm², the
    // centre (9,562.5 / 525, 20, 6,937.5 / 525).
    let right = mass(masses[1]);
    let full = legs(40.0);
    assert_eq!(full.volume, 21000.0);
    measured("the legs' volume", &right["volume"], full.volume);
    measured("the legs' area", &right["area"], 9850.0);
    centred("the legs' centre", &right["center_of_mass"], full.centre());

    // The four holes: 4 · 37.8125π = 151.25π mm³ less; each takes two discs of π · 2.75²
    // from its leg's faces and adds a wall of 2π · 2.75 · 5, so 4 · (27.5π − 15.125π) =
    // 49.5π mm² more.
    let holed = mass(masses[2]);
    let drilled = drilled();
    measured(
        "the drilled legs' volume",
        &holed["volume"],
        21000.0 - 151.25 * PI,
    );
    measured(
        "the drilled legs' volume (sum)",
        &holed["volume"],
        drilled.volume,
    );
    measured("the drilled legs' area", &holed["area"], 9850.0 + 49.5 * PI);
    centred(
        "the drilled legs' centre",
        &holed["center_of_mass"],
        drilled.centre(),
    );

    // The corner the agent filleted: of the six edges 40 long (one along y at each of the
    // profile's six corners), the one at x = 5, z = 5.
    let queries = run.answers("query");
    assert_eq!(queries.len(), 3);
    assert_eq!(queries[0]["result"]["matched"], 6, "{}", queries[0]);
    let fillets: Vec<&Value> = run
        .arguments("add_feature")
        .into_iter()
        .map(|a| &a["feature"])
        .filter(|f| f["type"] == "fillet")
        .collect();
    assert_eq!(fillets.len(), 1);
    let fillet = fillets[0];
    assert_eq!(fillet["radius"], FILLET_R);
    let edge = queries[0]["result"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == fillet["edges"][0])
        .unwrap();
    let ends = [&edge["start"], &edge["end"]]
        .map(|p| serde_json::from_value::<[f64; 3]>(p.clone()).unwrap());
    let mut ends = ends.to_vec();
    ends.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(ends, [[5.0, 0.0, 5.0], [5.0, 40.0, 5.0]], "{edge}");

    // Committed: the four solids' fingerprints written.
    let commit = run.answers("commit")[0];
    assert_eq!(commit["result"]["written"], true);
    assert_eq!(
        commit["result"]["refingerprinted"],
        serde_json::json!(["p0000001", "h0000001", "h0000002", "f0000001"])
    );

    // The bracket. Volume: 21,000 − 151.25π + the fillet's (1 − π/4) · 5² · 40 =
    // 22,000 − 401.25π ≈ 20,739.4359 mm³. Area: the fillet takes 5 · 40 from each leg's
    // inner face and adds a quarter cylinder, (π/2) · 5 · 40 = 100π, and each end face
    // gains the spandrel, 25(1 − π/4): 9,850 + 49.5π − 400 + 100π + 50 − 12.5π =
    // 9,500 + 137π ≈ 9,930.3982 mm². Centre: the parts' moments over the volume
    // ([`bracket`]): about (18.0196, 20, 13.0140).
    let done = mass(masses[3]);
    let hand = bracket();
    let volume = 22000.0 - 401.25 * PI;
    assert!((hand.volume - volume).abs() < 1e-9, "{}", hand.volume);
    measured("the bracket's volume", &done["volume"], volume);
    measured("the bracket's area", &done["area"], 9500.0 + 137.0 * PI);
    centred(
        "the bracket's centre",
        &done["center_of_mass"],
        hand.centre(),
    );

    // The saved document's last fingerprint: the box [0, 0, 0]–[60, 40, 50]; 13 faces (the
    // L-prism's 8, a wall for each hole, the fillet's), 33 edges (the prism's 18, each hole
    // two rims and a seam, the fillet's two lines and two arcs for the corner's one), and
    // 22 vertices (the prism's 12, each hole's two on its seam, the fillet's four for the
    // corner's two).
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(run.work.join("bracket.silico")).unwrap())
            .unwrap();
    let features = saved["features"].as_array().unwrap();
    let last = &features.last().unwrap()["fingerprint"];
    assert_eq!(last["bbox"], serde_json::json!([0, 0, 0, 60, 40, 50]));
    assert_eq!(last["faces"], 13);
    assert_eq!(last["edges"], 33);
    assert_eq!(last["vertices"], 22);
    measured("the saved volume", &last["volume"], volume);

    // The base's end face, x = 60: 40 × 5 = 200 mm², facing +X, centred (60, 20, 2.5); the
    // other face facing +X is the upright's inner face, larger.
    assert_eq!(queries[1]["result"]["matched"], 2);
    let end = &queries[1]["result"]["elements"][0];
    measured("the end face's area", &end["area"], 200.0);
    assert_eq!(end["normal"], serde_json::json!([1, 0, 0]));
    assert_eq!(end["centroid"], serde_json::json!([60, 20, 2.5]));
    // Eight rims, two for each hole, each of radius 2.75; the one measured at (40, 12).
    assert_eq!(queries[2]["result"]["matched"], 8);
    let distance = run.answers("distance")[0];
    let measured_pair = &run.arguments("distance")[0]["elements"];
    assert_eq!(measured_pair[1], end["name"]);
    let rim = queries[2]["result"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == measured_pair[0])
        .unwrap();
    measured("the rim's radius", &rim["radius"], HOLE_R);
    assert_eq!(rim["axis"]["point"], serde_json::json!([40, 12, 5]));
    // The hole's centre is 60 − 40 = 20 from the end face; its rim's nearest point,
    // (42.75, 12, 5), 20 − 2.75 = 17.25.
    measured("the rim's distance", &distance["result"]["distance"], 17.25);
    measured(
        "the hole's centre's distance",
        &distance["result"]["center_to_element"],
        20.0,
    );
    assert_eq!(
        distance["result"]["from"],
        serde_json::json!([42.75, 12, 5])
    );
    assert_eq!(distance["result"]["to"], serde_json::json!([60, 12, 5]));

    // The render: 800 × 600, as asked, written to bracket.png and sent beside the result
    // (its bytes never compared); from the top front right, looking at the box's centre
    // (30, 20, 25) along (−1, 1, −1), up (−1, 1, 2)/√6.
    let render = &run.answers("render")[0]["result"];
    assert_eq!(render["width"], 800);
    assert_eq!(render["height"], 600);
    assert_eq!(render["path"], "bracket.png");
    let png = std::fs::read(run.work.join("bracket.png")).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(&png[12..16], b"IHDR");
    let be = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
    assert_eq!((be(16), be(20)), (800, 600));
    assert_eq!(run.images.len(), 1);
    let camera = &render["camera"];
    assert_eq!(camera["target"], serde_json::json!([30, 20, 25]));
    let eye: [f64; 3] = serde_json::from_value(camera["eye"].clone()).unwrap();
    let away = [eye[0] - 30.0, 20.0 - eye[1], eye[2] - 25.0];
    assert!(
        away[0] > 0.0 && away.iter().all(|a| (a - away[0]).abs() < 1e-6),
        "{eye:?}"
    );
    let up: [f64; 3] = serde_json::from_value(camera["up"].clone()).unwrap();
    let sixth = (1.0f64 / 6.0).sqrt();
    for (u, h) in up.into_iter().zip([-sixth, sixth, 2.0 * sixth]) {
        assert!((u - h).abs() < 1e-12, "{up:?}");
    }

    // The export: one object, the bracket's last feature, its counts and its volume those
    // of the mesh in the file, which is printable as M4.8 promises.
    let export = &run.answers("export")[0]["result"];
    let archive = std::fs::read(run.work.join("bracket.3mf")).unwrap();
    assert_eq!(export["bytes"], archive.len());
    let meshes = model(&archive);
    assert_eq!(meshes.len(), 1, "one object, the bracket's");
    let mesh = &meshes[0];
    assert_eq!(mesh.name, "Inside corner");
    let objects = export["objects"].as_array().unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0]["feature"], "f0000001");
    assert_eq!(objects[0]["triangles"], mesh.triangles.len());
    assert_eq!(objects[0]["vertices"], mesh.vertices.len());
    let mesh_volume = printable(mesh);
    measured("the export's volume", &objects[0]["volume"], mesh_volume);
    // Meshed at the tolerance asked, by hand from OCCT's rule ([`chords`]): at 0.1 mm each
    // rim a circle of 126 chords (2π over 0.05 rad), so 126 points, and each of the
    // fillet's two quarter arcs 32 chords (π/2 over 0.05), 33 points. (At 0.01 mm they'd
    // be 229 and 58.)
    let rim = chords(2.0 * PI, HOLE_R, TOLERANCE);
    let arc = chords(PI / 2.0, FILLET_R, TOLERANCE);
    assert_eq!((rim, arc), (126, 32));
    let any = |_: &[f64; 3]| true;
    for y in [12.0, 28.0] {
        for at in [0.0, 5.0] {
            let v = &mesh.vertices;
            assert_eq!(
                on_circle(v, 2, at, [40.0, y, at], HOLE_R, any),
                rim,
                "base, y {y}"
            );
            assert_eq!(
                on_circle(v, 0, at, [at, y, 35.0], HOLE_R, any),
                rim,
                "upright, y {y}"
            );
        }
    }
    let corner = |v: &[f64; 3]| v[0] <= 10.0 && v[2] <= 10.0;
    for y in [0.0, 40.0] {
        let points = on_circle(&mesh.vertices, 1, y, [10.0, y, 10.0], FILLET_R, corner);
        assert_eq!(points, arc + 1, "the fillet's arc at y {y}");
    }
    // Nothing else is a vertex but the profile's five corners left at each end: the
    // walls and the fillet are meshed with no nodes inside them (a cylinder's splitter
    // places none, docs/ports/mesh-export.md), so 8 · 126 + 2 · 33 + 2 · 5 = 1,084; and,
    // closed and of genus 4 (four through holes), V − E + F = 2 − 2 · 4 with E = 3F/2, so
    // F = 2V + 12 = 2,180.
    let vertices = 8 * rim + 2 * (arc + 1) + 2 * 5;
    assert_eq!(mesh.vertices.len(), vertices);
    assert_eq!(mesh.triangles.len(), 2 * vertices + 12);
    // The mesh's box is the part's, [0, 0, 0]–[60, 40, 50], exactly: its corners are the
    // B-rep's vertices, whole numbers in 32 bits too.
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for p in &mesh.vertices {
        for i in 0..3 {
            low[i] = low[i].min(p[i]);
            high[i] = high[i].max(p[i]);
        }
    }
    assert_eq!((low, high), ([0.0; 3], [60.0, 40.0, 50.0]));
    // The mesh's volume, by hand. Every vertex is on the B-rep, on the rims and arcs
    // counted above, so each hole's wall is the prism on its rim's 126 chords and the
    // fillet's the prism on its arc's 32: each curved face concave to the material, the
    // mesh holds the part's material and, past each arc, the segments between it and its
    // chords. A hole's prism is short of its cylinder by h(πr² − (n/2)r² sin(2π/n)); a
    // chord across θ cuts off (r²/2)(θ − sin θ), under r²θ³/12. So the mesh encloses
    // V + 4 · 5 · (π · 2.75² − 63 · 2.75² sin(2π/126)) + 40 · 32 · (25/2)(θ − sin θ) at
    // θ = π/64, about V + 0.197 + 0.315 = V + 0.512 mm³, to what rounding each coordinate
    // to 32 bits moves it: a vertex by at most √3 · 60 · 2⁻²⁴, the volume by at most that
    // times the surface (under 10,000 mm²), ε ≈ 0.062 mm³ either way.
    let n = rim as f64;
    let hole_short = HOLE_R * HOLE_R * (PI - n / 2.0 * libm::sin(2.0 * PI / n));
    let theta = PI / 2.0 / arc as f64;
    let spandrel = arc as f64 * FILLET_R * FILLET_R / 2.0 * (theta - libm::sin(theta));
    assert!(spandrel <= arc as f64 * FILLET_R * FILLET_R * theta * theta * theta / 12.0);
    let enclosed = volume + 4.0 * 5.0 * hole_short + 40.0 * spandrel;
    let rounding = 10_000.0 * 3f64.sqrt() * 60.0 / 16_777_216.0;
    assert!(
        (mesh_volume - enclosed).abs() <= rounding,
        "the mesh encloses {mesh_volume} mm³; by hand {enclosed} (the part's {volume}), to {rounding}"
    );

    // FreeCAD reads it back as a solid: the mesh closed, manifold, and oriented by its own
    // checks, with the file's points and facets; the shape made of it a valid, closed solid
    // of one shell and a face a triangle, its box the part's, and its volume the mesh's.
    let read = freecad_reads(&run.work.join("bracket.3mf"));
    assert_eq!(read["mesh_solid"], true, "{read}");
    assert_eq!(read["non_manifold"], false, "{read}");
    assert_eq!(read["misoriented"], 0, "{read}");
    assert_eq!(read["points"], mesh.vertices.len(), "{read}");
    assert_eq!(read["facets"], mesh.triangles.len(), "{read}");
    assert_eq!(read["type"], "Solid", "{read}");
    assert_eq!(read["valid"], true, "{read}");
    assert_eq!(read["closed"], true, "{read}");
    assert_eq!(read["shells"], 1, "{read}");
    assert_eq!(read["faces"], mesh.triangles.len(), "{read}");
    let box_: [f64; 6] = serde_json::from_value(read["box"].clone()).unwrap();
    for (got, want) in box_.into_iter().zip([0.0, 0.0, 0.0, 60.0, 40.0, 50.0]) {
        assert!((got - want).abs() < 1e-9, "{read}");
    }
    let theirs = read["volume"].as_f64().unwrap();
    assert!(
        (theirs - mesh_volume).abs() <= 1e-9 * mesh_volume,
        "FreeCAD's solid {theirs} mm³, the mesh {mesh_volume}"
    );
}

// --- M5.1's timeline commands on the real kernel ---------------------------------------------

/// silico-core's committed example `name` (`move_feature/result`), read as JSON.
fn core_example(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../silico-core/tests/contract/commands/examples")
        .join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

/// A tool's answer's envelope, through `serve`.
fn called(serve: &mut Serve, id: u64, tool: &str, arguments: &str) -> Value {
    let send = format!(
        r#"{{"jsonrpc": "2.0", "id": {id}, "method": "tools/call", "params": {{"name": "{tool}", "arguments": {arguments}}}}}"#
    );
    let reply = serve.send(&send).unwrap();
    let answer: Reply = serde_json::from_str(&reply).unwrap();
    let envelope: Value = serde_json::from_str(answer.result.structured.get()).unwrap();
    assert_eq!(
        answer.result.is_error,
        envelope["ok"] == false,
        "{tool}: {envelope}"
    );
    holds(&tools()[tool]["outputSchema"], &envelope, tool);
    envelope
}

#[test]
fn the_bracket_s_timeline_edits_through_serve_measure_as_by_hand() {
    // The design's M5.1 examples on the real kernel (docs/design/m5.0-app.md, section 7):
    // the bracket, formatted so each feature stores its fingerprint, then, inside a
    // transaction, its timeline, the pockets swapped by `move_feature`, the first removed,
    // and it added back before s0000003; each result the design's, each volume by hand.
    use features::*;
    let mut serve = Serve::start();
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        [
            S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001
        ]
        .join(", ")
    );
    std::fs::write(serve.work.join("bracket.silico"), text).unwrap();
    serve.send(INITIALIZE).unwrap();
    assert!(serve.send(INITIALIZED).is_none());
    let mut id = 0;
    let mut call = |serve: &mut Serve, tool: &str, arguments: &str| {
        id += 1;
        called(serve, id, tool, arguments)
    };
    let opened = call(
        &mut serve,
        "open",
        r#"{"path": "bracket.silico", "create": false}"#,
    );
    assert_eq!(opened["ok"], true, "{opened}");
    let formatted = call(
        &mut serve,
        "format",
        r#"{"session": "s1", "features": null, "write": true}"#,
    );
    assert_eq!(formatted["result"]["written"], true, "{formatted}");
    let volume = |serve: &mut Serve, call: &mut dyn FnMut(&mut Serve, &str, &str) -> Value| {
        let answer = call(
            serve,
            "mass_properties",
            r#"{"session": "s1", "model": "current", "feature": null}"#,
        );
        mass(&answer)["volume"].clone()
    };
    // All built: the legs, less the four holes, and the fillet's spandrel, 20,739.435948.
    let whole = bracket().volume;
    measured("the bracket", &volume(&mut serve, &mut call), whole);
    let begun = call(&mut serve, "begin", r#"{"session": "s1"}"#);
    let at = begun["session"]["revision"].as_u64().unwrap();

    // The timeline: every feature healthy, as the design's example lists them, the fillet
    // the one body.
    let timeline = call(
        &mut serve,
        "timeline",
        r#"{"session": "s1", "model": "current"}"#,
    );
    let example = core_example("timeline/result");
    assert_eq!(
        timeline["result"]["features"], example["features"],
        "{timeline}"
    );
    assert_eq!(timeline["result"]["bodies"], example["bodies"]);
    assert_eq!(timeline["result"]["revision"], at);
    assert_eq!(timeline["result"]["model_revision"], at);
    assert_eq!(timeline["result"]["problems"], 0);

    // The pockets swapped: the design's result, word for word; the cuts commute, so the
    // volume is the same.
    let moved = call(
        &mut serve,
        "move_feature",
        &format!(
            r#"{{"session": "s1", "revision": {at}, "feature": "h0000001", "before": "f0000001"}}"#
        ),
    );
    assert_eq!(
        moved["result"],
        core_example("move_feature/result"),
        "{moved}"
    );
    measured("the pockets swapped", &volume(&mut serve, &mut call), whole);
    // Rebuilt on their new bases, each feature still healthy: the edits' reach is excepted
    // from drift until the commit writes it.
    let timeline = call(
        &mut serve,
        "timeline",
        r#"{"session": "s1", "model": "current"}"#,
    );
    for entry in timeline["result"]["features"].as_array().unwrap() {
        assert_eq!(entry["health"], "healthy", "{entry}");
    }
    let rolled = call(&mut serve, "rollback", r#"{"session": "s1"}"#);
    let at = rolled["session"]["revision"].as_u64().unwrap();
    call(&mut serve, "begin", r#"{"session": "s1"}"#);

    // The first pocket removed: h0000002 on the pad, so two holes fewer, 20,977.019.
    let removed = call(
        &mut serve,
        "remove_feature",
        &format!(r#"{{"session": "s1", "revision": {at}, "feature": "h0000001"}}"#),
    );
    assert_eq!(
        removed["result"]["rewired"],
        core_example("remove_feature/result-rewired")["rewired"]
    );
    assert_eq!(removed["result"]["index"], 3);
    let one_pocket = whole + 2.0 * HOLE;
    measured(
        "the first pocket removed",
        &volume(&mut serve, &mut call),
        one_pocket,
    );
    // Added back before s0000003, its fingerprint left out: h0000002's base rewired back,
    // and the volume as before.
    let mut back = removed["result"]["feature"].clone();
    back.as_object_mut().unwrap().remove("fingerprint");
    let added = call(
        &mut serve,
        "add_feature",
        &format!(
            r#"{{"session": "s1", "revision": {}, "before": "s0000003", "feature": {back}}}"#,
            at + 1
        ),
    );
    assert_eq!(added["result"]["index"], 3, "{added}");
    assert_eq!(
        added["result"]["rewired"],
        serde_json::json!([{"feature": "h0000002", "field": "base", "from": "p0000001", "to": "h0000001"}])
    );
    measured("the pocket back", &volume(&mut serve, &mut call), whole);
    call(&mut serve, "rollback", r#"{"session": "s1"}"#);
    call(&mut serve, "close", r#"{"session": "s1"}"#);
    serve.end();
}

#[test]
fn the_bracket_s_suppressions_through_serve_measure_as_by_hand() {
    // The design's suppression cases on the real kernel (docs/design/m5.0-app.md, section
    // 7's `suppress_feature` and the test plan's *Suppress*), each inside a transaction from
    // the formatted bracket, rolled back after it; each result the design's, and each
    // volume by hand: the legs (21,000), a hole (π · 2.75² · 5), the fillet's spandrel.
    use features::*;
    let mut serve = Serve::start();
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        [
            S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001
        ]
        .join(", ")
    );
    std::fs::write(serve.work.join("bracket.silico"), text).unwrap();
    serve.send(INITIALIZE).unwrap();
    assert!(serve.send(INITIALIZED).is_none());
    let mut id = 0;
    let mut call = |serve: &mut Serve, tool: &str, arguments: &str| {
        id += 1;
        called(serve, id, tool, arguments)
    };
    let opened = call(
        &mut serve,
        "open",
        r#"{"path": "bracket.silico", "create": false}"#,
    );
    assert_eq!(opened["ok"], true, "{opened}");
    call(
        &mut serve,
        "format",
        r#"{"session": "s1", "features": null, "write": true}"#,
    );
    // Fresh: every step built again, never read from the cache (the test plan's).
    let cleared = call(&mut serve, "cache_clear", r#"{"session": "s1"}"#);
    assert_eq!(cleared["ok"], true, "{cleared}");
    let volume = |serve: &mut Serve, call: &mut dyn FnMut(&mut Serve, &str, &str) -> Value| {
        let checked = call(
            serve,
            "check",
            r#"{"session": "s1", "model": "current", "fresh": true}"#,
        );
        assert_eq!(checked["ok"], true, "{checked}");
        let answer = call(
            serve,
            "mass_properties",
            r#"{"session": "s1", "model": "current", "feature": null}"#,
        );
        answer["result"]["bodies"].clone()
    };
    let whole = bracket().volume;
    let one_pocket = whole + 2.0 * HOLE;
    let no_fillet = drilled().volume;
    let no_pockets = legs(40.0).volume + bracket().volume - drilled().volume;
    // By hand: 21,000 + 25 (1 − π/4) · 40.
    assert!((no_pockets - (21000.0 + 25.0 * (1.0 - PI / 4.0) * 40.0)).abs() < 1e-9);

    // Each case: the features suppressed, by one command each, and what's left.
    let cases: [(&[&str], Option<f64>); 6] = [
        // The first hole: the part, h0000002 on the pad (decision 3), 20,977.019.
        (&["h0000001"], Some(one_pocket)),
        (&["h0000002"], Some(one_pocket)),
        // The fillet: the drilled legs, 20,524.834111.
        (&["f0000001"], Some(no_fillet)),
        // Both pockets: 21,214.602.
        (&["h0000001", "h0000002"], Some(no_pockets)),
        // The base holes' sketch: h0000001 with it, 20,977.019.
        (&["s0000002"], Some(one_pocket)),
        // The pad: all four solids; no body.
        (&["p0000001"], None),
    ];
    for (suppressed, hand) in cases {
        let begun = call(&mut serve, "begin", r#"{"session": "s1"}"#);
        let mut at = begun["session"]["revision"].as_u64().unwrap();
        for feature in suppressed {
            let done = call(
                &mut serve,
                "suppress_feature",
                &format!(
                    r#"{{"session": "s1", "revision": {at}, "feature": "{feature}", "suppressed": true}}"#
                ),
            );
            assert_eq!(done["result"]["changed"], true, "{done}");
            at = done["session"]["revision"].as_u64().unwrap();
        }
        let bodies = volume(&mut serve, &mut call);
        match hand {
            Some(hand) => {
                let bodies = bodies.as_array().unwrap();
                assert_eq!(bodies.len(), 1, "{suppressed:?}: {bodies:?}");
                measured(
                    &format!("{suppressed:?} suppressed"),
                    &bodies[0]["volume"],
                    hand,
                );
            }
            None => assert_eq!(bodies, serde_json::json!([]), "{suppressed:?}"),
        }
        call(&mut serve, "rollback", r#"{"session": "s1"}"#);
    }

    // The design's examples on the real kernel: the sketch, then h0000001 unsuppressed with
    // it, the whole bracket again.
    let begun = call(&mut serve, "begin", r#"{"session": "s1"}"#);
    let at = begun["session"]["revision"].as_u64().unwrap();
    let done = call(
        &mut serve,
        "suppress_feature",
        &format!(
            r#"{{"session": "s1", "revision": {at}, "feature": "s0000002", "suppressed": true}}"#
        ),
    );
    assert_eq!(
        done["result"],
        core_example("suppress_feature/result-sketch")
    );
    let back = call(
        &mut serve,
        "suppress_feature",
        &format!(
            r#"{{"session": "s1", "revision": {}, "feature": "h0000001", "suppressed": false}}"#,
            at + 1
        ),
    );
    assert_eq!(
        back["result"],
        core_example("suppress_feature/result-unsuppress")
    );
    let bodies = volume(&mut serve, &mut call);
    measured("unsuppressed again", &bodies[0]["volume"], whole);
    call(&mut serve, "rollback", r#"{"session": "s1"}"#);
    call(&mut serve, "close", r#"{"session": "s1"}"#);
    serve.end();
}

#[test]
fn a_suppressed_fillet_changes_the_shape_and_unsuppressed_gives_it_back() {
    // FreeCAD's TestSuppressed.py::testSuppressChangesShape (main), ported with its values
    // by hand where FreeCAD asserts only that they differ and come back: a 10 mm cube with a
    // 1 mm fillet of one edge, 1000 − (1 − π/4) · 1² · 10 = 997.853982; suppressed, the cube,
    // 1000; unsuppressed, 997.853982 again (FreeCAD 1.1.3, observed: 997.8539816339743,
    // 999.9999999999998, 997.8539816339743).
    let mut serve = Serve::start();
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k0000001", "type": "box", "v": 1, "name": "Cube", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}, {"id": "f0000001", "type": "fillet", "v": 1, "name": "Fillet", "base": "k0000001", "edges": ["k0000001.box.Edge1"], "radius": 1}]}"#;
    std::fs::write(serve.work.join("cube.silico"), text).unwrap();
    serve.send(INITIALIZE).unwrap();
    assert!(serve.send(INITIALIZED).is_none());
    let mut id = 0;
    let mut call = |serve: &mut Serve, tool: &str, arguments: &str| {
        id += 1;
        called(serve, id, tool, arguments)
    };
    call(
        &mut serve,
        "open",
        r#"{"path": "cube.silico", "create": false}"#,
    );
    let volume = |serve: &mut Serve, call: &mut dyn FnMut(&mut Serve, &str, &str) -> Value| {
        let answer = call(
            serve,
            "mass_properties",
            r#"{"session": "s1", "model": "current", "feature": null}"#,
        );
        mass(&answer)["volume"].clone()
    };
    let filleted = 1000.0 - (1.0 - PI / 4.0) * 10.0;
    measured("filleted", &volume(&mut serve, &mut call), filleted);
    // Outside a transaction, each its own commit.
    let off = call(
        &mut serve,
        "suppress_feature",
        r#"{"session": "s1", "revision": 1, "feature": "f0000001", "suppressed": true}"#,
    );
    assert_eq!(off["result"]["suppressed"], serde_json::json!(["f0000001"]));
    measured("suppressed", &volume(&mut serve, &mut call), 1000.0);
    let rev = off["session"]["revision"].as_u64().unwrap();
    let on = call(
        &mut serve,
        "suppress_feature",
        &format!(
            r#"{{"session": "s1", "revision": {rev}, "feature": "f0000001", "suppressed": false}}"#
        ),
    );
    assert_eq!(
        on["result"]["unsuppressed"],
        serde_json::json!(["f0000001"])
    );
    measured("unsuppressed", &volume(&mut serve, &mut call), filleted);
    call(&mut serve, "close", r#"{"session": "s1"}"#);
    serve.end();
}
