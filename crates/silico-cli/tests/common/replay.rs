//! A recorded MCP session, replayed and recorded (the bracket's, M4.9's done test, and the
//! plate's, M6.5's): `silico serve` with the real workers in a temporary workspace, each line
//! of a session's `.jsonl` sent only after the reply to the one before, every reply compared
//! with the recording's byte for byte and held to its tool's `outputSchema` in silico-mcp's
//! committed `tools.json`; and the recorder, which plays an agent live and writes the lines.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::Receiver;

use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

use super::serve::{ask_line, exited, lines, serve_working};

/// Panics, naming `what`, unless `value` holds to `schema`.
pub fn holds(schema: &Value, value: &Value, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{what}: {errors:?}");
}

/// `silico serve` with the real workers in a temporary workspace of its own
/// ([`serve_working`]), its stderr the test's, its stdout's lines as they come.
pub struct Serve {
    pub _dir: tempfile::TempDir,
    pub work: PathBuf,
    pub child: Option<Child>,
    pub stdin: Option<ChildStdin>,
    pub lines: Receiver<String>,
}

impl Serve {
    pub fn start() -> Serve {
        let dir = tempfile::Builder::new()
            .prefix("silico-session-")
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
    pub fn send(&mut self, line: &str) -> Option<String> {
        let message: Value = serde_json::from_str(line).unwrap();
        let stdin = self.stdin.as_mut().unwrap();
        let Some(id) = message.get("id") else {
            writeln!(stdin, "{line}").unwrap();
            return None;
        };
        Some(ask_line(stdin, &self.lines, line, &id.to_string()))
    }

    /// Ends the server (its stdin closed), checking it exits 0.
    pub fn end(&mut self) {
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
pub struct Reply<'a> {
    #[serde(borrow)]
    pub result: Answer<'a>,
}

#[derive(Deserialize)]
pub struct Answer<'a> {
    #[serde(borrow, rename = "structuredContent")]
    pub structured: &'a RawValue,
    #[serde(rename = "isError")]
    pub is_error: bool,
}

/// `structuredContent`'s text as compared: `render`'s `png_bytes`, the size of an image a
/// GPU drew, never compared, its number said so.
pub fn compared(structured: &str) -> String {
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
pub fn compared_initialize(reply: &str, work: &Path) -> String {
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
pub struct Line<'a> {
    pub note: String,
    #[serde(borrow)]
    pub send: &'a RawValue,
    #[serde(borrow)]
    pub expect: Option<&'a RawValue>,
}

/// What the replay saw: each tool called, by name, with its arguments and its answer.
pub struct Replayed {
    pub calls: Vec<(String, Value, Value)>,
    pub work: PathBuf,
    /// The PNG `render` sent beside its result, as base64 (never compared).
    pub images: Vec<String>,
    pub _serve: Serve,
}

impl Replayed {
    /// The answers (`structuredContent`) of every call of `tool`, in order.
    pub fn answers(&self, tool: &str) -> Vec<&Value> {
        self.calls
            .iter()
            .filter(|(name, _, _)| name == tool)
            .map(|(_, _, answer)| answer)
            .collect()
    }

    /// The arguments of every call of `tool`, in order.
    pub fn arguments(&self, tool: &str) -> Vec<&Value> {
        self.calls
            .iter()
            .filter(|(name, _, _)| name == tool)
            .map(|(_, arguments, _)| arguments)
            .collect()
    }
}

/// The committed `tools/list` reply's tools (silico-mcp's contract), by name.
pub fn tools() -> BTreeMap<String, Value> {
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
pub fn replay(session: &Path) -> Replayed {
    let text = std::fs::read_to_string(session).unwrap();
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

/// The agent, live: drives `serve` through the session, building each request from the
/// replies before it as an agent reads them (the revision its edit is made against, the
/// names `query` gives), and gives `session.jsonl`'s lines.
pub struct Recorder {
    pub serve: Serve,
    pub id: u64,
    pub lines: Vec<String>,
    pub revision: u64,
}

impl Recorder {
    /// Calls `tool` on `arguments`, recording the line and its answer; gives the answer.
    pub fn call(&mut self, note: &str, tool: &str, arguments: &str) -> Value {
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

    pub fn push(&mut self, note: &str, send: &str, expect: Option<&str>) {
        let note = serde_json::to_string(note).unwrap();
        self.lines.push(format!(
            r#"{{"note": {note}, "send": {send}, "expect": {}}}"#,
            expect.unwrap_or("null")
        ));
    }

    /// `add_feature` of `feature` (JSON text), against the session's revision.
    pub fn add(&mut self, note: &str, feature: &str) -> Value {
        let arguments = format!(
            r#"{{"session": "s1", "revision": {}, "before": null, "feature": {feature}}}"#,
            self.revision
        );
        self.call(note, "add_feature", &arguments)
    }
}
