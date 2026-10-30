//! A client of the server, for tests: the server serving on a thread, on sessions on a fake
//! kernel drawing with a fake renderer, in a workspace of their own; its input a pipe the
//! test writes lines to, its output lines the test reads, failing if one doesn't come.

use std::io::{BufReader, PipeWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use silico_core::cache::Cache;
use silico_core::command::Sessions;
use silico_core::fake::{FakeKernel, FakeRenderer, FakeSketches};
use silico_core::sketch::SketchSolver;
use silico_core::kernel::{Op, Primitive};
use silico_mcp::{Ended, Server, Stop};
use tempfile::TempDir;

/// Longer than anything here takes on a loaded machine: past it, the server hangs.
pub const HANG: Duration = Duration::from_secs(60);

/// A 40 × 20 × 10 box.
pub const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

/// `check`'s request of s1's current model.
pub const CHECK: &str = r#"{"session": "s1", "model": "current", "fresh": false}"#;

/// A reply to `tools/call`, its result a tool's answer.
#[derive(Deserialize)]
pub struct Reply<'a> {
    #[serde(borrow)]
    pub result: Answer<'a>,
}

/// A tool's answer, its `structuredContent` as written.
#[derive(Deserialize)]
pub struct Answer<'a> {
    pub content: Vec<Block>,
    #[serde(borrow, rename = "structuredContent")]
    pub structured: &'a RawValue,
    #[serde(rename = "isError")]
    pub is_error: bool,
}

/// One of an answer's content blocks.
#[derive(Deserialize)]
pub struct Block {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: Option<String>,
}

/// A JSON-RPC error's code, ID, and its data's first error.
pub fn rpc_error(message: &Value) -> (i64, Value, Value) {
    assert!(message.get("result").is_none(), "{message}");
    (
        message["error"]["code"].as_i64().unwrap(),
        message["id"].clone(),
        message["error"]["data"]["errors"][0].clone(),
    )
}

pub fn is_box(op: &Op) -> bool {
    matches!(op, Op::MakePrimitive(Primitive::Box { .. }))
}

/// The output's end: each line written sent to the test, or, once `fail` is set, every
/// write failing as a closed pipe's does; or, while `stall` says so, every write blocked,
/// as a client's that stopped reading is.
struct Lines {
    line: Vec<u8>,
    to: Sender<Vec<u8>>,
    fail: Arc<AtomicBool>,
    stall: Arc<Stall>,
}

/// Whether writes block, and whether one is blocked.
#[derive(Default)]
struct Stall {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}

/// Writes go on again, a blocked one first.
fn release(stall: &Stall) {
    let mut state = stall.state.lock().unwrap();
    *state = (false, false);
    stall.changed.notify_all();
}

impl Write for Lines {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut state = self.stall.state.lock().unwrap();
        while state.0 {
            state.1 = true;
            self.stall.changed.notify_all();
            state = self.stall.changed.wait(state).unwrap();
        }
        drop(state);
        if self.fail.load(Ordering::SeqCst) {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        for &b in bytes {
            if b == b'\n' {
                let _ = self.to.send(std::mem::take(&mut self.line));
            } else {
                self.line.push(b);
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        Ok(())
    }
}

pub struct Client {
    _dir: TempDir,
    pub work: PathBuf,
    pub kernel: Arc<FakeKernel>,
    input: Option<PipeWriter>,
    lines: Receiver<Vec<u8>>,
    fail: Arc<AtomicBool>,
    stall: Arc<Stall>,
    server: Option<JoinHandle<Ended>>,
    pub stop: Stop,
}

impl Client {
    /// A server, not yet initialized.
    pub fn start() -> Client {
        let (reader, input) = std::io::pipe().unwrap();
        let mut client = Client::reading(BufReader::new(reader));
        client.input = Some(input);
        client
    }

    /// A server, not yet initialized, reading `reader` (the client's own writes fail).
    pub fn reading(reader: impl std::io::BufRead + Send + 'static) -> Client {
        Client::reading_solving(reader, None)
    }

    /// A server, initialized, its sketches solved by `solver`, the fake sketch solver, as
    /// the core's own tests solve them.
    pub fn initialized_solving(solver: FakeSketches) -> Client {
        let (reader, input) = std::io::pipe().unwrap();
        let mut client = Client::reading_solving(BufReader::new(reader), Some(solver));
        client.input = Some(input);
        client.initialize();
        client
    }

    /// A server, not yet initialized, reading `reader`, its sketches solved by `solver` if
    /// it's given, else by the sketch worker.
    fn reading_solving(
        reader: impl std::io::BufRead + Send + 'static,
        solver: Option<FakeSketches>,
    ) -> Client {
        let dir = tempfile::tempdir().unwrap();
        let work = std::fs::canonicalize(dir.path()).unwrap().join("work");
        std::fs::create_dir(&work).unwrap();
        let secret = dir.path().join("secret");
        let kernel = Arc::new(FakeKernel::new(2).unwrap());
        let cache = move |cache: &std::path::Path| Cache::with_secret(cache, &secret);
        let sessions = match solver {
            None => Sessions::with_cache_opener(Arc::clone(&kernel), cache),
            Some(solver) => Sessions::with_stand_ins(Arc::clone(&kernel), cache, move || {
                Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>)
            }),
        }
        .rendering(FakeRenderer::new())
        .in_workspace(&work)
        .unwrap();
        let server = Server::new(sessions).unwrap();
        let stop = server.stopper();
        let (to, lines) = mpsc::channel();
        let fail = Arc::new(AtomicBool::new(false));
        let stall = Arc::new(Stall::default());
        let output = Lines {
            line: Vec::new(),
            to,
            fail: Arc::clone(&fail),
            stall: Arc::clone(&stall),
        };
        let server = std::thread::spawn(move || server.serve(reader, output));
        Client {
            _dir: dir,
            work,
            kernel,
            input: None,
            lines,
            fail,
            stall,
            server: Some(server),
            stop,
        }
    }

    /// A server, initialized.
    pub fn initialized() -> Client {
        let mut client = Client::start();
        client.initialize();
        client
    }

    /// The handshake.
    fn initialize(&mut self) {
        let reply = self.request(0, "initialize", r#"{"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}"#);
        assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
        self.send(r#"{"jsonrpc": "2.0", "method": "notifications/initialized"}"#);
    }

    /// A server, initialized, with `part.silico` ([`BOX`]) open as s1 (by the call `1`).
    pub fn opened() -> Client {
        let mut client = Client::initialized();
        client.file("part.silico", BOX);
        client.open(1, "part.silico");
        client
    }

    /// `name` in the workspace, holding `text`.
    pub fn file(&self, name: &str, text: &str) {
        std::fs::write(self.work.join(name), text).unwrap();
    }

    pub fn read_file(&self, name: &str) -> String {
        std::fs::read_to_string(self.work.join(name)).unwrap()
    }

    /// Writes `bytes` and a newline.
    pub fn send_bytes(&mut self, bytes: &[u8]) {
        let input = self.input.as_mut().unwrap();
        input.write_all(bytes).unwrap();
        input.write_all(b"\n").unwrap();
    }

    pub fn send(&mut self, line: &str) {
        self.send_bytes(line.as_bytes());
    }

    /// The next line written, as written.
    pub fn line(&mut self) -> String {
        match self.lines.recv_timeout(HANG) {
            Ok(line) => String::from_utf8(line).unwrap(),
            Err(e) => panic!("no line in {HANG:?}: {e}"),
        }
    }

    /// The next line written, if one comes within `wait` (only to show none came, where
    /// something later is known to come after it).
    pub fn line_within(&mut self, wait: Duration) -> Option<String> {
        match self.lines.recv_timeout(wait) {
            Ok(line) => Some(String::from_utf8(line).unwrap()),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => None,
        }
    }

    /// The next message written.
    pub fn message(&mut self) -> Value {
        let line = self.line();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"))
    }

    /// The reply to the request `id` (JSON text), its line, and every other message
    /// written before it (notifications, other replies), in order.
    pub fn reply_line(&mut self, id: &str) -> (String, Vec<Value>) {
        let id: Value = serde_json::from_str(id).unwrap();
        let mut before = Vec::new();
        loop {
            let line = self.line();
            let message: Value = serde_json::from_str(&line).unwrap();
            if message.get("id") == Some(&id) && message.get("method").is_none() {
                return (line, before);
            }
            before.push(message);
        }
    }

    /// Sends the request `method` with `params` (JSON text) as `id`, and gives its reply.
    pub fn request(&mut self, id: u64, method: &str, params: &str) -> Value {
        self.send(&format!(
            r#"{{"jsonrpc": "2.0", "id": {id}, "method": "{method}", "params": {params}}}"#
        ));
        let (line, _) = self.reply_line(&id.to_string());
        serde_json::from_str(&line).unwrap()
    }

    /// Sends `tools/call` of `name` on `arguments` (JSON text) as `id`, not waiting.
    pub fn send_call(&mut self, id: u64, name: &str, arguments: &str) {
        self.send_call_as(&id.to_string(), name, arguments);
    }

    /// Sends `tools/call` of `name` on `arguments` (JSON text) as `id` (JSON text), not
    /// waiting.
    pub fn send_call_as(&mut self, id: &str, name: &str, arguments: &str) {
        self.send(&format!(
            r#"{{"jsonrpc": "2.0", "id": {id}, "method": "tools/call", "params": {{"name": "{name}", "arguments": {arguments}}}}}"#
        ));
    }

    /// `tools/call` of `name` on `arguments` (JSON text) as `id`: its reply's result.
    pub fn call(&mut self, id: u64, name: &str, arguments: &str) -> Value {
        self.send_call(id, name, arguments);
        let (line, _) = self.reply_line(&id.to_string());
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert!(reply.get("result").is_some(), "{line}");
        reply["result"].clone()
    }

    /// `tools/call`'s answer's envelope, `structuredContent`.
    pub fn envelope(&mut self, id: u64, name: &str, arguments: &str) -> Value {
        self.call(id, name, arguments)["structuredContent"].clone()
    }

    /// Opens `name` (in the workspace) as a session, which must open.
    pub fn open(&mut self, id: u64, name: &str) -> String {
        let envelope = self.envelope(
            id,
            "open",
            &format!(r#"{{"path": "{name}", "create": false}}"#),
        );
        assert_eq!(envelope["ok"], true, "{envelope}");
        envelope["session"]["id"].as_str().unwrap().to_owned()
    }

    /// Closes the server's input.
    pub fn close_input(&mut self) {
        self.input = None;
    }

    /// Every write to the output fails from now, as a closed pipe's does.
    pub fn close_output(&self) {
        self.fail.store(true, Ordering::SeqCst);
    }

    /// Every write to the output blocks from now, as a client's that stopped reading does.
    pub fn stall_output(&self) {
        self.stall.state.lock().unwrap().0 = true;
    }

    /// Writes to the output go on again, a blocked one first.
    pub fn release_output(&self) {
        release(&self.stall);
    }

    /// What lets writes to the output go on again, from another thread.
    pub fn releaser(&self) -> impl FnOnce() + Send + 'static {
        let stall = Arc::clone(&self.stall);
        move || release(&stall)
    }

    /// Whether `serve` has returned.
    pub fn has_ended(&self) -> bool {
        self.server.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Waits until a write is blocked, failing if none is within [`HANG`].
    pub fn wait_stalled(&self) {
        let state = self.stall.state.lock().unwrap();
        let (state, _) = self
            .stall
            .changed
            .wait_timeout_while(state, HANG, |s| !s.1)
            .unwrap();
        assert!(state.1, "no write blocked");
    }

    /// The server's input, to write to from another thread; the client's own writes fail
    /// after.
    pub fn take_input(&mut self) -> PipeWriter {
        self.input.take().unwrap()
    }

    /// How the server ended, failing if it doesn't within [`HANG`].
    pub fn ended(&mut self) -> Ended {
        let server = self.server.take().unwrap();
        let (done, ended) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(server.join());
        });
        match ended.recv_timeout(HANG) {
            Ok(Ok(ended)) => ended,
            Ok(Err(_)) => panic!("the server panicked"),
            Err(_) => panic!("the server didn't end in {HANG:?}"),
        }
    }
}

/// `count` requests' held by the fake kernel's box, started, failing if they don't.
pub fn running(kernel: &FakeKernel, count: usize) {
    assert!(kernel.wait_running(count, HANG), "nothing ran");
}
