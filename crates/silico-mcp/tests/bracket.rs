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
//! reply also holds to its tool's `outputSchema` in the committed `tools.json`.
//!
//! **Checked by hand,** never read from the recording: the expected values below are the
//! spec's dimensions worked out on paper (prisms, cylinders, the fillet's spandrel), each
//! derivation beside its assert. The 3MF is read here from its bytes and checked as M4.8
//! promises a printable mesh (closed, manifold, consistently oriented, no triangle with two
//! corners at one place, enclosing a positive volume), its genus that of a part with four
//! through holes, and its volume within the tolerance's bound of the hand calculation; and
//! FreeCAD 1.1.3 (the oracle) reads it back, makes a solid of it, and finds that solid
//! valid, with the mesh's volume.
//!
//! **Recording.** With `SILICO_BRACKET_RECORD=1` set, the session is first driven live by
//! [`record`], which plays the agent (each request built from the replies before it, as an
//! agent reads them), and `session.jsonl` is written from the real replies; the replay and
//! every check below then run on what was written. The `silico` binary and its workers are
//! the ones built beside this test program (`cargo build -p silico-cli -p silico-kernel
//! -p silico-sketch -p silico-render --bins`, which `pixi run test` does).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

use crate::contract::holds;

/// Longer than any one reply takes on a loaded machine: past it, the server hangs.
const REPLY: Duration = Duration::from_secs(240);

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

/// The `silico` binary built beside this test program (`target/<profile>/silico`), with its
/// workers next to it, where it finds them.
fn silico() -> PathBuf {
    let me = std::env::current_exe().unwrap();
    // target/<profile>/deps/all_mcp-<hash>
    let profile = me.parent().unwrap().parent().unwrap();
    let silico = profile.join(format!("silico{}", std::env::consts::EXE_SUFFIX));
    assert!(
        silico.is_file(),
        "{} isn't built: cargo build -p silico-cli -p silico-kernel -p silico-sketch -p silico-render --bins",
        silico.display()
    );
    silico
}

/// `silico serve` in a temporary workspace of its own, as an MCP client starts it: its
/// stdin and stdout piped, its stdout's lines as they come, its stderr kept for a failure's
/// message, its cache's secret in a cache home of its own.
struct Serve {
    _dir: tempfile::TempDir,
    work: PathBuf,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    stderr: Option<std::thread::JoinHandle<String>>,
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
        // A secret of its own, as silico would make one, so the run neither reads nor
        // writes the user's.
        let home = root.join("cache-home");
        std::fs::create_dir_all(home.join("silico")).unwrap();
        let mut secret = [0u8; 32];
        std::fs::File::open("/dev/urandom")
            .unwrap()
            .read_exact(&mut secret)
            .unwrap();
        std::fs::write(home.join("silico/cache-secret"), secret).unwrap();
        let mut child = Command::new(silico())
            .args(["serve", "--workspace"])
            .arg(&work)
            .env("XDG_CACHE_HOME", &home)
            .env_remove("SILICO_LOG")
            .env_remove("SILICO_KERNEL_WORKER")
            .env_remove("SILICO_SKETCH_WORKER")
            .env_remove("SILICO_RENDER_WORKER")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let mut stderr = child.stderr.take().unwrap();
        let stderr = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        let (to, lines) = mpsc::channel();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        std::thread::spawn(move || {
            for line in stdout.lines() {
                let Ok(line) = line else { return };
                if to.send(line).is_err() {
                    return;
                }
            }
        });
        Serve {
            _dir: dir,
            work,
            child: Some(child),
            stdin,
            lines,
            stderr: Some(stderr),
        }
    }

    /// Sends `line`; then, if it's a request, waits for the reply carrying its ID and gives
    /// it as written, every message before it (a progress notification, timing) skipped.
    fn send(&mut self, line: &str) -> Option<String> {
        let message: Value = serde_json::from_str(line).unwrap();
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
        let id = message.get("id")?;
        loop {
            let Ok(reply) = self.lines.recv_timeout(REPLY) else {
                panic!(
                    "no reply to {line} in {REPLY:?}; silico serve's stderr:\n{}",
                    self.kill()
                );
            };
            let parsed: Value = serde_json::from_str(&reply)
                .unwrap_or_else(|e| panic!("not JSON on stdout ({e}): {reply}"));
            assert_eq!(parsed["jsonrpc"], "2.0", "{reply}");
            if parsed.get("id") == Some(id) && parsed.get("method").is_none() {
                return Some(reply);
            }
            assert_eq!(parsed["method"], "notifications/progress", "{reply}");
        }
    }

    /// Kills the server, and gives its stderr.
    fn kill(&mut self) -> String {
        self.stdin = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.stderr
            .take()
            .map(|stderr| stderr.join().unwrap())
            .unwrap_or_default()
    }

    /// Ends the server (its stdin closed), checking it exits 0, and gives its stderr.
    fn end(&mut self) -> String {
        self.stdin = None;
        let mut child = self.child.take().unwrap();
        let (done, exited) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(child.wait());
        });
        let status = exited.recv_timeout(REPLY);
        let stderr = self.stderr.take().unwrap().join().unwrap();
        match status {
            Ok(Ok(status)) => assert_eq!(status.code(), Some(0), "{stderr}"),
            _ => panic!("silico serve didn't end in {REPLY:?}: {stderr}"),
        }
        stderr
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

/// `initialize`'s result as compared: `serverInfo.version`, the build's, and the
/// workspace's absolute path in `instructions` put as `<version>` and `<workspace>`.
fn compared_initialize(reply: &str, work: &Path) -> Value {
    let mut reply: Value = serde_json::from_str(reply).unwrap();
    let result = &mut reply["result"];
    assert!(result["serverInfo"]["version"].is_string(), "{result}");
    result["serverInfo"]["version"] = Value::from("<version>");
    let instructions = result["instructions"].as_str().unwrap();
    let workspace = work.to_str().unwrap();
    assert!(instructions.contains(workspace), "{instructions}");
    result["instructions"] = Value::from(instructions.replace(workspace, "<workspace>"));
    reply["result"].take()
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

/// The committed `tools/list` reply's tools, by name.
fn tools() -> BTreeMap<String, Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/contract/tools.json");
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
                let expected: Value = serde_json::from_str(line.expect.unwrap().get()).unwrap();
                assert_eq!(compared_initialize(&reply, &serve.work), expected, "{step}");
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
    format!(
        r#"{{"id": "{id}", "type": "sketch", "name": "{name}", "plane": "{plane}", "points": [{{"id": "pb000001", "x": {x}, "y": {y}, "construction": false}}, {{"id": "pb000002", "x": {x2}, "y": {y2}, "construction": false}}], "lines": [], "circles": [{{"id": "cb000001", "center": "pb000001", "radius": 2.75, "construction": false}}, {{"id": "cb000002", "center": "pb000002", "radius": 2.75, "construction": false}}], "arcs": [], "constraints": [{{"id": "kb000001", "type": "diameter", "curve": "cb000001", "value": 5.5, "driving": true}}, {{"id": "kb000002", "type": "equal", "a": "cb000001", "b": "cb000002"}}, {{"id": "kb000003", "type": "distance_x", "a": "origin", "b": "pb000001", "value": {x}, "driving": true}}, {{"id": "kb000004", "type": "distance_y", "a": "origin", "b": "pb000001", "value": {y}, "driving": true}}, {{"id": "kb000005", "type": "{aligned}", "a": "pb000001", "b": "pb000002"}}, {{"id": "kb000006", "type": "distance_{along}", "a": "pb000001", "b": "pb000002", "value": 16, "driving": true}}]}}"#
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
            r#"{{"session": "s1", "revision": {}, "feature": {feature}}}"#,
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
    let expect = compared_initialize(&reply, &r.serve.work).to_string();
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
            r#"{{"id": "f0000001", "type": "fillet", "name": "Inside corner", "base": "h0000002", "edges": [{corner}], "radius": 5}}"#
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
        r#"{"session": "s1", "model": "current", "feature": null, "kind": "edge", "type": "circle", "stable": null, "facing": null, "measures": [{"measure": "radius", "at_least": 2.75, "at_most": 2.75}], "order": null, "limit": 8}"#,
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
        r#"{"session": "s1", "format": "3mf", "path": "bracket.3mf", "replace": false, "features": null, "tolerance": 0.1}"#,
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

// --- The 3MF, read from its bytes -----------------------------------------------------------

/// The archive's entries (stored, as Silico writes them): each name and its data, from the
/// local headers (APPNOTE 6.3.3, 4.3.7).
fn entries(archive: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let u16_at = |at: usize| usize::from(u16::from_le_bytes([archive[at], archive[at + 1]]));
    let u32_at = |at: usize| u32::from_le_bytes(archive[at..at + 4].try_into().unwrap());
    let mut out = BTreeMap::new();
    let mut at = 0;
    while u32_at(at) == 0x0403_4b50 {
        assert_eq!(u16_at(at + 8), 0, "stored");
        let size = u32_at(at + 18) as usize;
        assert_eq!(u32_at(at + 22) as usize, size);
        let (name_len, extra) = (u16_at(at + 26), u16_at(at + 28));
        let name = String::from_utf8(archive[at + 30..at + 30 + name_len].to_vec()).unwrap();
        let data = at + 30 + name_len + extra;
        out.insert(name, archive[data..data + size].to_vec());
        at = data + size;
    }
    out
}

/// A tag's attribute `name`, from the tag's text after its name.
fn attribute(tag: &str, name: &str) -> String {
    let tag = format!(" {tag}");
    let key = format!(" {name}=\"");
    let at = tag
        .find(&key)
        .unwrap_or_else(|| panic!("no {name} in {tag}"))
        + key.len();
    tag[at..].split('"').next().unwrap().to_owned()
}

/// The 3MF model's one object: its type, name, vertices, and triangles, as written.
struct Object {
    kind: String,
    name: String,
    vertices: Vec<[f64; 3]>,
    triangles: Vec<[usize; 3]>,
}

fn object(model: &str) -> Object {
    let header = &model[model.find("<model ").unwrap()..];
    assert_eq!(attribute(header, "unit"), "millimeter");
    let objects: Vec<&str> = model.split("<object ").skip(1).collect();
    assert_eq!(objects.len(), 1, "one object, the bracket's");
    let object = objects[0];
    let vertices = object
        .split("<vertex ")
        .skip(1)
        .map(|tag| ["x", "y", "z"].map(|k| attribute(tag, k).parse::<f32>().unwrap().into()))
        .collect();
    let triangles = object
        .split("<triangle ")
        .skip(1)
        .map(|tag| ["v1", "v2", "v3"].map(|k| attribute(tag, k).parse().unwrap()))
        .collect();
    Object {
        kind: attribute(object, "type"),
        name: attribute(object, "name"),
        vertices,
        triangles,
    }
}

/// Checks `o` is printable as M4.8 promises (docs/ports/mesh-export.md, *A printable
/// mesh*), from its triangles alone: no triangle with a repeated index or two corners at
/// one place; every directed edge on exactly one triangle and its reverse on exactly one
/// other (closed, manifold, consistently oriented); and the volume it encloses, which it
/// gives, positive (facing outward).
fn printable(o: &Object) -> f64 {
    let mut directed: BTreeMap<(usize, usize), u32> = BTreeMap::new();
    for &[a, b, c] in &o.triangles {
        assert!(a != b && b != c && a != c, "a repeated index: {a} {b} {c}");
        let (p, q, s) = (o.vertices[a], o.vertices[b], o.vertices[c]);
        assert!(
            p != q && q != s && p != s,
            "two corners at one place: {a} {b} {c}"
        );
        for edge in [(a, b), (b, c), (c, a)] {
            *directed.entry(edge).or_default() += 1;
        }
    }
    for (&(a, b), &count) in &directed {
        assert_eq!(count, 1, "edge {a}→{b} on {count} triangles one way");
        assert_eq!(
            directed.get(&(b, a)),
            Some(&1),
            "edge {a}→{b} without its reverse: open, or misoriented"
        );
    }
    let volume = o
        .triangles
        .iter()
        .map(|&[a, b, c]| {
            let (a, b, c) = (o.vertices[a], o.vertices[b], o.vertices[c]);
            a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0])
        })
        .sum::<f64>()
        / 6.0;
    assert!(volume > 0.0, "the mesh faces inward: {volume}");
    volume
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
    let fillet = run.arguments("add_feature")[8]["feature"].clone();
    assert_eq!(fillet["type"], "fillet");
    assert_eq!(fillet["radius"], 5);
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
    // of the mesh in the file.
    let export = &run.answers("export")[0]["result"];
    let archive = std::fs::read(run.work.join("bracket.3mf")).unwrap();
    assert_eq!(export["bytes"], archive.len());
    let entries = entries(&archive);
    let model = String::from_utf8(entries["3D/3dmodel.model"].clone()).unwrap();
    let mesh = object(&model);
    assert_eq!(mesh.kind, "model");
    assert_eq!(mesh.name, "Inside corner");
    let objects = export["objects"].as_array().unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0]["feature"], "f0000001");
    assert_eq!(objects[0]["triangles"], mesh.triangles.len());
    assert_eq!(objects[0]["vertices"], mesh.vertices.len());
    let mesh_volume = printable(&mesh);
    measured("the export's volume", &objects[0]["volume"], mesh_volume);
    // Closed and of genus 4 (four through holes): V − E + F = 2 − 2 · 4, each edge on two
    // triangles, E = 3F/2.
    let (v, f) = (mesh.vertices.len() as i64, mesh.triangles.len() as i64);
    assert_eq!(2 * v - f, 2 * (2 - 2 * 4), "V {v}, F {f}");
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
    // The mesh's volume against the hand's, within the tolerance's bound. Every vertex is
    // on the B-rep, and each curved face (the holes' walls, the fillet) is concave to the
    // material, so its triangles are chords inside the cylinder: the mesh holds all the
    // part's material and more, by at most the area between each arc and its chords, along
    // the face. An arc of radius r cut in chords each subtending at most a (FreeCAD's
    // angular deflection, min(0.1, 5t + 0.005) = 0.1 rad at t = 0.1 mm, which the export
    // ports) leaves a sagitta s ≤ r(1 − cos(a/2)) ≤ r·a²/8, and the area between the arc
    // and its chords is at most the arc's length × s. So the excess is at most each curved
    // face's area × its s: the holes' walls 4 · 2π · 2.75 · 5 = 110π mm² at 2.75 · 0.01/8,
    // the fillet's 100π mm² at 5 · 0.01/8, 1.003125π ≈ 3.15 mm³ in all; under the
    // tolerance's own bound, the curved area × t = 21π. Rounding each coordinate to 32 bits
    // moves a vertex at most √3 · 60 · 2⁻²⁴, which moves the volume by at most that times
    // the surface (under 10,000 mm²): ε ≈ 0.062 mm³, either way.
    let angle = (5.0 * TOLERANCE + 0.005).min(0.1);
    let sagitta = |r: f64| (r * angle * angle / 8.0).min(TOLERANCE);
    let excess = 110.0 * PI * sagitta(HOLE_R) + 100.0 * PI * sagitta(FILLET_R);
    let rounding = 10_000.0 * 3f64.sqrt() * 60.0 / 16_777_216.0;
    assert!(
        mesh_volume >= volume - rounding && mesh_volume <= volume + excess + rounding,
        "the mesh encloses {mesh_volume} mm³; by hand {volume}, so from {} to {}",
        volume - rounding,
        volume + excess + rounding
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
