//! `silico serve` (docs/cli.md, docs/design/m4.9-mcp.md), the real binary on the real
//! kernel, started as an MCP client starts it, in a temporary workspace: it initializes,
//! lists its tools, makes a new document, closes it, and exits 0 when its stdin ends,
//! nothing on stdout but MCP's messages; it refuses a disk's root as its workspace, and
//! takes its options as every command does. And `/break-it` on it: every log on, garbage
//! and calls on stdin; its stdout closed under it; SIGTERM while its client never reads;
//! stdin closed mid-transaction; SIGTERM mid-commit. It never hangs, exits 0, and leaves
//! the document as it was or as committed, whole, with nothing beside it. Expected values:
//! MCP 2025-06-18's shapes and its stdio rule (nothing but messages on stdout), the tool
//! count from the design's command list, the note's rules, and the files as written here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/run.rs"]
mod run;
#[path = "common/serve.rs"]
mod serve;

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;

use run::{DEADLINE, output};
use serve::{BOX, INITIALIZE, ask, exited, json_rpc, lines, listed, serve, sigterm};

#[test]
fn serve_initializes_lists_its_tools_makes_a_document_and_ends_with_its_input() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = serve(dir.path(), None, Stdio::piped());
    let mut stderr = child.stderr.take().unwrap();
    let logs = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let mut stdin = child.stdin.take().unwrap();
    let lines = lines(&mut child);
    let init = ask(&mut stdin, &lines, INITIALIZE, "0");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "silico");
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let instructions = init["result"]["instructions"].as_str().unwrap();
    assert!(
        instructions.contains(&format!("The workspace is {}:", root.display())),
        "{instructions}"
    );
    writeln!(
        stdin,
        r#"{{"jsonrpc": "2.0", "method": "notifications/initialized"}}"#
    )
    .unwrap();
    let tools = ask(
        &mut stdin,
        &lines,
        r#"{"jsonrpc": "2.0", "id": 2, "method": "tools/list"}"#,
        "2",
    );
    let tools = tools["result"]["tools"].as_array().unwrap();
    // M4.1's 26 commands, M5.1's move_feature, suppress_feature, and timeline, M5.3's
    // drag_begin, drag_move, and drag_end, M5.5's meshes, M5.10's sketch, sketch_add,
    // sketch_remove, sketch_split, sketch_trim, sketch_unjoin, and sketch_merge, and M5.2's
    // undo and redo.
    assert_eq!(tools.len(), 42);
    assert_eq!(tools[1]["name"], "open");
    let opened = ask(
        &mut stdin,
        &lines,
        r#"{"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "open", "arguments": {"path": "new.silico", "create": true}}}"#,
        "3",
    );
    let envelope = &opened["result"]["structuredContent"];
    assert_eq!(envelope["ok"], true, "{opened}");
    assert_eq!(envelope["session"]["id"], "s1");
    assert_eq!(envelope["result"]["created"], true);
    assert!(root.join("new.silico").is_file());
    let closed = ask(
        &mut stdin,
        &lines,
        r#"{"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "close", "arguments": {"session": "s1"}}}"#,
        "4",
    );
    assert_eq!(closed["result"]["isError"], false, "{closed}");
    let version = ask(
        &mut stdin,
        &lines,
        r#"{"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "version"}}"#,
        "5",
    );
    assert_eq!(version["result"]["structuredContent"]["result"]["api"], 1);
    drop(stdin);
    assert_eq!(exited(child).code(), Some(0));
    let stderr = logs.join().unwrap();
    assert!(!stderr.contains("worker"), "{stderr}");
}

/// `silico ARGS`, run in `dir` with stdin empty: its exit code and stderr.
fn run_in(dir: &std::path::Path, args: &[&str]) -> (Option<i32>, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_silico"));
    command.args(args).current_dir(dir).stdin(Stdio::null());
    let out = output(command);
    assert!(out.stdout.is_empty(), "{args:?}");
    (out.status.code(), String::from_utf8(out.stderr).unwrap())
}

#[test]
fn serve_refuses_the_root_of_the_disk_as_its_workspace() {
    for args in [
        &["serve", "--workspace", "/"][..],
        &["serve", "--workspace=/"],
        // Nor started in it.
        &["serve"],
    ] {
        let (code, stderr) = run_in(std::path::Path::new("/"), args);
        assert_eq!(code, Some(2), "{args:?}");
        assert!(
            stderr.starts_with("silico serve: the workspace can't be /, the root of a disk"),
            "{stderr}"
        );
    }
}

#[test]
fn serve_takes_its_options_as_every_command_does() {
    let dir = tempfile::tempdir().unwrap();
    // `--workspace=DIR` is `--workspace DIR`: the directory named, whatever follows `=`.
    let missing = dir.path().join("no=such");
    let (code, stderr) = run_in(
        dir.path(),
        &["serve", &format!("--workspace={}", missing.display())],
    );
    assert_eq!(code, Some(2));
    assert!(
        stderr.starts_with(&format!(
            "silico serve: the workspace {} can't be reached",
            missing.display()
        )),
        "{stderr}"
    );
    // Usage errors, each saying so, with the usage: an option it hasn't, `--workspace` with
    // no directory or given twice, and anything after `--`, which ends the options (so
    // `--workspace` there is no option, and serve takes no file).
    for (args, said) in [
        (
            &["serve", "--port", "80"][..],
            "silico serve: no option \"--port\"",
        ),
        (
            &["serve", "--workspace"],
            "silico: --workspace needs a directory",
        ),
        (
            &["serve", "--workspace", ".", "--workspace=."],
            "silico serve: --workspace is given once",
        ),
        (&["serve", "--", "--workspace", "."], "usage: silico check"),
        (&["serve", "."], "usage: silico check"),
        // And no other command has it.
        (
            &["check", "--workspace", ".", "part.silico"],
            "silico check: no option \"--workspace\"",
        ),
    ] {
        let (code, stderr) = run_in(dir.path(), args);
        assert_eq!(code, Some(2), "{args:?}");
        assert!(stderr.starts_with(said), "{args:?}: {stderr}");
        assert!(stderr.contains("usage: silico check"), "{args:?}: {stderr}");
    }
}

#[test]
fn serve_exits_2_when_its_stdin_cant_be_read() {
    // Its stdin a directory, which reading fails (EISDIR).
    let dir = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_silico"));
    command
        .args(["serve", "--workspace"])
        .arg(dir.path())
        .stdin(std::fs::File::open(dir.path()).unwrap());
    let out = output(command);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("silico serve: stdin couldn't be read: Is a directory"),
        "{stderr}"
    );
}

#[test]
fn serve_ends_on_sigterm_with_its_input_still_open() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = serve(dir.path(), None, Stdio::null());
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    // Answered, so it's serving (and its handler registered) before the signal.
    writeln!(stdin, r#"{{"jsonrpc": "2.0", "id": 1, "method": "ping"}}"#).unwrap();
    let mut pong = String::new();
    stdout.read_line(&mut pong).unwrap();
    assert_eq!(pong, "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n");
    sigterm(&child);
    assert_eq!(exited(child).code(), Some(0));
    drop(stdin);
}

#[test]
fn serve_writes_nothing_but_json_rpc_on_stdout_with_every_log_on() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("part.silico"), BOX).unwrap();
    let mut child = serve(dir.path(), Some("trace"), Stdio::piped());
    let mut stderr = child.stderr.take().unwrap();
    let logs = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stderr.read_to_end(&mut text);
        text
    });
    let mut stdin = child.stdin.take().unwrap();
    let lines = lines(&mut child);
    ask(&mut stdin, &lines, INITIALIZE, "0");
    // Garbage, then calls that reach the kernel, the cache, and the file system.
    for line in [
        &b"not json"[..],
        b"\xff\xfe",
        b"{\"jsonrpc\": \"2.0\", \"id\": 1, \"method\": \"tools/call\", \"params\": {\"name\": \"open\", \"arguments\": {\"path\": \"../x.silico\", \"create\": true}}}",
        b"{\"jsonrpc\": \"2.0\", \"id\": 2, \"method\": \"tools/call\", \"params\": {\"name\": \"nope\"}}",
    ] {
        stdin.write_all(line).unwrap();
        stdin.write_all(b"\n").unwrap();
    }
    let opened = ask(
        &mut stdin,
        &lines,
        r#"{"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "open", "arguments": {"path": "part.silico", "create": false}}}"#,
        "3",
    );
    assert_eq!(opened["result"]["isError"], false, "{opened}");
    // The open refused took s1, as each open takes the next ID when it's called.
    let session = opened["result"]["structuredContent"]["session"]["id"].clone();
    assert_eq!(session, "s2");
    let checked = ask(
        &mut stdin,
        &lines,
        r#"{"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "check", "arguments": {"session": "s2", "model": "current", "fresh": false}, "_meta": {"progressToken": 1}}}"#,
        "4",
    );
    assert_eq!(checked["result"]["isError"], false, "{checked}");
    let measured = ask(
        &mut stdin,
        &lines,
        r#"{"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "mass_properties", "arguments": {"session": "s2", "model": "current", "feature": "k7f2qx3m"}}}"#,
        "5",
    );
    assert_eq!(measured["result"]["isError"], false, "{measured}");
    // 40 × 20 × 10: 8,000 mm³, 2 × (800 + 400 + 200) = 2,800 mm², its centre half of each.
    assert_eq!(
        measured["result"]["structuredContent"]["result"]["bodies"],
        serde_json::json!([{"feature": "k7f2qx3m", "volume": 8000, "area": 2800, "center_of_mass": [20, 10, 5]}]),
        "{measured}"
    );
    drop(stdin);
    assert_eq!(exited(child).code(), Some(0));
    // Every line left on stdout is JSON-RPC too.
    while let Ok(line) = lines.recv_timeout(Duration::ZERO) {
        json_rpc(&line);
    }
    // The logs went to stderr.
    let logs = String::from_utf8_lossy(&logs.join().unwrap()).into_owned();
    assert!(logs.contains("serving MCP"), "{logs}");
}

#[test]
fn serve_ends_when_its_stdout_is_closed() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = serve(dir.path(), None, Stdio::null());
    let mut stdin = child.stdin.take().unwrap();
    // The client's end of stdout closed before anything is written: the first reply's write
    // fails (EPIPE, SIGPIPE being ignored), and serve ends.
    drop(child.stdout.take());
    for id in 0..1_000 {
        let ping = format!("{{\"jsonrpc\": \"2.0\", \"id\": {id}, \"method\": \"ping\"}}\n");
        if stdin.write_all(ping.as_bytes()).is_err() {
            break;
        }
    }
    let status = exited(child);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

#[test]
fn serve_ends_on_sigterm_while_its_client_never_reads() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = serve(dir.path(), None, Stdio::null());
    let mut stdin = child.stdin.take().unwrap();
    // Read the handshake's answer only, so the handler's registered; then never read again.
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    writeln!(stdin, "{INITIALIZE}").unwrap();
    let mut first = String::new();
    stdout.read_line(&mut first).unwrap();
    json_rpc(&first);
    // Far more pings than stdout's pipe and the server's queue hold, each with an ID of a
    // kilobyte, which its pong carries back: the writer blocks, then the reader, then this
    // thread's writes.
    let (wrote, written) = mpsc::channel();
    std::thread::spawn(move || {
        let pad = "p".repeat(1_000);
        for id in 0..100_000u64 {
            let ping =
                format!("{{\"jsonrpc\": \"2.0\", \"id\": \"{pad}{id}\", \"method\": \"ping\"}}\n");
            if stdin.write_all(ping.as_bytes()).is_err() {
                return;
            }
            if id == 150 {
                let _ = wrote.send(());
            }
        }
    });
    // 150 pongs of a kilobyte are more than stdout's pipe (64 KiB) holds, so the server's
    // writer is about to block, if it hasn't.
    written.recv_timeout(DEADLINE).unwrap();
    sigterm(&child);
    let status = exited(child);
    assert_eq!(status.code(), Some(0), "{status:?}");
    drop(stdout);
}

#[test]
fn serve_rolls_back_a_transaction_when_its_stdin_closes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("part.silico"), BOX).unwrap();
    let mut child = serve(dir.path(), None, Stdio::null());
    let mut stdin = child.stdin.take().unwrap();
    let lines = lines(&mut child);
    ask(&mut stdin, &lines, INITIALIZE, "0");
    for (id, name, arguments) in [
        (1, "open", r#"{"path": "part.silico", "create": false}"#),
        (2, "begin", r#"{"session": "s1"}"#),
        (
            3,
            "set_field",
            r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}"#,
        ),
    ] {
        let reply = ask(
            &mut stdin,
            &lines,
            &format!(
                r#"{{"jsonrpc": "2.0", "id": {id}, "method": "tools/call", "params": {{"name": "{name}", "arguments": {arguments}}}}}"#
            ),
            &id.to_string(),
        );
        assert_eq!(reply["result"]["isError"], false, "{reply}");
    }
    // A check running as the client goes away.
    writeln!(stdin, r#"{{"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {{"name": "check", "arguments": {{"session": "s1", "model": "current", "fresh": false}}}}}}"#).unwrap();
    drop(stdin);
    assert_eq!(exited(child).code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("part.silico")).unwrap(),
        BOX
    );
    assert_eq!(listed(dir.path()), ["part.silico"]);
}

#[test]
fn serve_on_sigterm_mid_commit_leaves_the_document_whole() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("part.silico"), BOX).unwrap();
    let mut child = serve(dir.path(), None, Stdio::null());
    let mut stdin = child.stdin.take().unwrap();
    let lines = lines(&mut child);
    ask(&mut stdin, &lines, INITIALIZE, "0");
    for (id, name, arguments) in [
        (1, "open", r#"{"path": "part.silico", "create": false}"#),
        (2, "begin", r#"{"session": "s1"}"#),
        (
            3,
            "set_field",
            r#"{"session": "s1", "revision": 1, "feature": "k7f2qx3m", "field": "length", "value": 41}"#,
        ),
    ] {
        let reply = ask(
            &mut stdin,
            &lines,
            &format!(
                r#"{{"jsonrpc": "2.0", "id": {id}, "method": "tools/call", "params": {{"name": "{name}", "arguments": {arguments}}}}}"#
            ),
            &id.to_string(),
        );
        assert_eq!(reply["result"]["isError"], false, "{reply}");
    }
    // The commit sent, and SIGTERM at once: whichever wins, the file is whole.
    writeln!(stdin, r#"{{"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {{"name": "commit", "arguments": {{"session": "s1", "path": null, "replace": false}}}}}}"#).unwrap();
    sigterm(&child);
    assert_eq!(exited(child).code(), Some(0));
    let text = std::fs::read_to_string(dir.path().join("part.silico")).unwrap();
    if text != BOX {
        let document: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(document["features"][0]["length"], 41, "{text}");
    }
    assert_eq!(listed(dir.path()), ["part.silico"]);
    drop(stdin);
}

/// A directory `bin/` holding a copy of `silico` and, linked from the build's, each of
/// `workers`: an install with only those workers beside it. Its resolved `bin/`.
fn install(root: &std::path::Path, workers: &[&str]) -> std::path::PathBuf {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let bin = std::fs::canonicalize(&bin).unwrap();
    let built = std::path::Path::new(env!("CARGO_BIN_EXE_silico"));
    std::fs::copy(built, bin.join("silico")).unwrap();
    for worker in workers {
        let from = built.with_file_name(worker);
        assert!(from.is_file(), "{} isn't built", from.display());
        std::os::unix::fs::symlink(from, bin.join(worker)).unwrap();
    }
    bin
}

/// All of `pipe`, read on a thread of its own, given once it ends: received under
/// [`DEADLINE`], so a server that doesn't end fails the test, saying so, rather than hanging.
fn read_all(pipe: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (to, read) = mpsc::channel();
    std::thread::spawn(move || {
        let mut pipe = pipe;
        let mut text = String::new();
        let _ = pipe.read_to_string(&mut text);
        let _ = to.send(text);
    });
    read
}

/// How `command`, a `silico serve` that shouldn't start, ends, and what it said on stdout
/// and stderr, sent an `initialize` and then its input's end, as a client starts one (a
/// server that did start would answer, then end).
fn refused_start(mut command: Command) -> (Option<i32>, String, String) {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let _ = writeln!(stdin, "{INITIALIZE}");
    drop(stdin);
    let stdout = read_all(child.stdout.take().unwrap());
    let stderr = read_all(child.stderr.take().unwrap());
    let stdout = stdout
        .recv_timeout(DEADLINE)
        .expect("serve's stdout didn't end: it's still running");
    let stderr = stderr
        .recv_timeout(DEADLINE)
        .expect("serve's stderr didn't end: it's still running");
    (exited(child).code(), stdout, stderr)
}

/// `silico serve` as `exe`, in the workspace `dir`, with no worker named by a variable.
fn serve_command(exe: &std::path::Path, dir: &std::path::Path) -> Command {
    let mut command = Command::new(exe);
    command
        .args(["serve", "--workspace"])
        .arg(dir)
        .env_remove("SILICO_KERNEL_WORKER")
        .env_remove("SILICO_SKETCH_WORKER")
        .env_remove("SILICO_RENDER_WORKER")
        .env_remove("SILICO_STEP_WORKER")
        .env_remove("SILICO_LOG");
    command
}

#[test]
fn serve_without_a_kernel_worker_doesnt_start_and_says_where_it_looked() {
    // Nothing would build: a broken install, said before the handshake, never a server that
    // answers every read with its fault.
    let root = tempfile::tempdir().unwrap();
    let bin = install(
        root.path(),
        &[
            "silico-sketch-worker",
            "silico-render-worker",
            "silico-step-worker",
        ],
    );
    let workspace = root.path().join("models");
    std::fs::create_dir(&workspace).unwrap();
    let (code, stdout, stderr) = refused_start(serve_command(&bin.join("silico"), &workspace));
    assert_eq!(code, Some(2), "{stderr}");
    assert_eq!(stdout, "", "no handshake");
    let said = format!(
        "silico serve: there's no kernel worker it can use, so nothing can be built: worker_unavailable: no kernel worker in {}: Silico looks for silico-kernel-worker beside its own executable, {}",
        bin.display(),
        bin.join("silico").display()
    );
    assert!(stderr.starts_with(&said), "{said}\nin {stderr}");
    assert!(
        stderr.contains(&format!("or put it in {}", bin.display())),
        "{stderr}"
    );
    assert!(listed(&workspace).is_empty());
}

#[test]
fn serve_with_a_kernel_worker_variable_naming_nothing_doesnt_start() {
    let dir = tempfile::tempdir().unwrap();
    let mut command = serve_command(
        std::path::Path::new(env!("CARGO_BIN_EXE_silico")),
        dir.path(),
    );
    command.env("SILICO_KERNEL_WORKER", "/nowhere");
    let (code, stdout, stderr) = refused_start(command);
    assert_eq!(code, Some(2), "{stderr}");
    assert_eq!(stdout, "", "no handshake");
    assert!(
        stderr.starts_with(
            "silico serve: there's no kernel worker it can use, so nothing can be built: worker_unavailable: SILICO_KERNEL_WORKER names \"/nowhere\", which isn't a file"
        ),
        "{stderr}"
    );
}

#[test]
fn serve_with_only_a_kernel_worker_doesnt_start_and_names_each_missing_one() {
    // All four workers or none (M4's close, overturning #199): a missing sketch, STEP, or
    // render worker is a broken install, said before the handshake, each in one message
    // naming where it looked, rather than met at the command that first needs it.
    let root = tempfile::tempdir().unwrap();
    let bin = install(root.path(), &["silico-kernel-worker"]);
    let workspace = root.path().join("models");
    std::fs::create_dir(&workspace).unwrap();
    let (code, stdout, stderr) = refused_start(serve_command(&bin.join("silico"), &workspace));
    assert_eq!(code, Some(2), "{stderr}");
    assert_eq!(stdout, "", "no handshake");
    let said = [
        (
            "silico serve: there's no sketch worker, so no sketch can be solved: worker_unavailable: no sketch worker in",
            "silico-sketch-worker",
        ),
        (
            "silico serve: there's no STEP worker, so nothing can be exported as STEP: worker_unavailable: no STEP worker in",
            "silico-step-worker",
        ),
        (
            "silico serve: there's no render worker, so nothing can be rendered: worker_unavailable: no render worker in",
            "silico-render-worker",
        ),
    ];
    let lines: Vec<&str> = stderr.lines().collect();
    for (message, worker) in said {
        let line = lines
            .iter()
            .find(|line| line.starts_with(message))
            .unwrap_or_else(|| panic!("{message}\nnot in {stderr}"));
        assert!(
            line.contains(&format!(
                "Silico looks for {worker} beside its own executable"
            )),
            "{line}"
        );
        assert!(line.contains(&bin.display().to_string()), "{line}");
    }
    // The kernel worker is there: nothing is said of it.
    assert!(!stderr.contains("kernel worker"), "{stderr}");
    assert!(listed(&workspace).is_empty());
}

#[test]
fn serve_missing_any_one_other_worker_alone_doesnt_start_and_names_it() {
    // Each of the sketch, STEP, and render workers missing on its own, the other three
    // there: exit 2 before the handshake, and only that worker said, where it was looked for.
    let all = [
        "silico-kernel-worker",
        "silico-sketch-worker",
        "silico-render-worker",
        "silico-step-worker",
    ];
    for (missing, said) in [
        (
            "silico-sketch-worker",
            "silico serve: there's no sketch worker, so no sketch can be solved: worker_unavailable: no sketch worker in",
        ),
        (
            "silico-step-worker",
            "silico serve: there's no STEP worker, so nothing can be exported as STEP: worker_unavailable: no STEP worker in",
        ),
        (
            "silico-render-worker",
            "silico serve: there's no render worker, so nothing can be rendered: worker_unavailable: no render worker in",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let there: Vec<&str> = all.iter().copied().filter(|w| *w != missing).collect();
        let bin = install(root.path(), &there);
        let workspace = root.path().join("models");
        std::fs::create_dir(&workspace).unwrap();
        let (code, stdout, stderr) = refused_start(serve_command(&bin.join("silico"), &workspace));
        assert_eq!(code, Some(2), "{missing}: {stderr}");
        assert_eq!(stdout, "", "{missing}: no handshake");
        let lines: Vec<&str> = stderr
            .lines()
            .filter(|line| line.starts_with("silico serve: "))
            .collect();
        assert_eq!(lines.len(), 1, "{missing}: {stderr}");
        assert!(lines[0].starts_with(said), "{missing}: {}", lines[0]);
        assert!(
            lines[0].contains(&format!(
                "Silico looks for {missing} beside its own executable"
            )),
            "{}",
            lines[0]
        );
        assert!(
            lines[0].contains(&bin.display().to_string()),
            "{}",
            lines[0]
        );
        assert!(listed(&workspace).is_empty());
    }
}

#[test]
fn serve_without_any_worker_names_all_four_kernel_first() {
    let root = tempfile::tempdir().unwrap();
    let bin = install(root.path(), &[]);
    let workspace = root.path().join("models");
    std::fs::create_dir(&workspace).unwrap();
    let (code, stdout, stderr) = refused_start(serve_command(&bin.join("silico"), &workspace));
    assert_eq!(code, Some(2), "{stderr}");
    assert_eq!(stdout, "", "no handshake");
    let firsts: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("silico serve: there's no "))
        .map(|line| line.split(',').next().unwrap())
        .collect();
    assert_eq!(
        firsts,
        [
            "silico serve: there's no kernel worker it can use",
            "silico serve: there's no sketch worker",
            "silico serve: there's no STEP worker",
            "silico serve: there's no render worker",
        ],
        "{stderr}"
    );
}

#[test]
fn serve_with_a_kernel_worker_it_cant_read_doesnt_start() {
    // A worker there that can't be read is no worker it can use: refused as one missing.
    let root = tempfile::tempdir().unwrap();
    let bin = install(root.path(), &[]);
    let worker = bin.join("silico-kernel-worker");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_BIN_EXE_silico")).with_file_name("silico-kernel-worker"),
        &worker,
    )
    .unwrap();
    std::fs::set_permissions(&worker, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
    let workspace = root.path().join("models");
    std::fs::create_dir(&workspace).unwrap();
    let (code, stdout, stderr) = refused_start(serve_command(&bin.join("silico"), &workspace));
    assert_eq!(code, Some(2), "{stderr}");
    assert_eq!(stdout, "", "no handshake");
    let said = format!(
        "silico serve: there's no kernel worker it can use, so nothing can be built: worker_unavailable: couldn't read the kernel worker {}",
        worker.display()
    );
    assert!(stderr.starts_with(&said), "{said}\nin {stderr}");
}

#[test]
fn serve_gives_a_kernel_faults_advice_as_data_in_the_models_fault() {
    // The contract lows' `advice`, from a real fault: a kernel worker that segfaults once
    // the request reaches it (worker.rs's `crash_advice`: a fault signal with the request
    // is the values', which would crash it again). `check`'s envelope gives the fault once,
    // in `model.fault`, its advice `change_the_values` as data beside the session's words.
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("models");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("box.silico"), BOX).unwrap();
    let kernel = root.path().join("segfaulting-kernel");
    std::fs::write(
        &kernel,
        "#!/bin/sh\nprintf '\\0\\0\\0\\0' >&3\nhead -c 1 <&3 >/dev/null\nkill -SEGV $$\nexec sleep 600\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&kernel, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_silico"))
        .args(["serve", "--workspace"])
        .arg(&workspace)
        .env("SILICO_KERNEL_WORKER", &kernel)
        .env("XDG_CACHE_HOME", root.path().join("cache-home"))
        .env_remove("SILICO_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let lines = lines(&mut child);
    ask(&mut stdin, &lines, INITIALIZE, "0");
    let opened = ask(
        &mut stdin,
        &lines,
        r#"{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "open", "arguments": {"path": "box.silico", "create": false}}}"#,
        "1",
    );
    assert_eq!(
        opened["result"]["structuredContent"]["ok"], true,
        "{opened}"
    );
    let checked = ask(
        &mut stdin,
        &lines,
        r#"{"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "check", "arguments": {"session": "s1", "model": "current", "fresh": true}}}"#,
        "2",
    );
    let fault = &checked["result"]["structuredContent"]["model"]["fault"];
    assert_eq!(fault["code"], "worker_crashed", "{checked}");
    assert_eq!(fault["advice"], "change_the_values", "{checked}");
    drop(stdin);
    assert_eq!(exited(child).code(), Some(0));
}
