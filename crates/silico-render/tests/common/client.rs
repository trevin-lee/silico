//! What the render client's tests share (`client.rs`, `adversarial_client.rs`): the workers,
//! a client of one, outcomes taken apart, the test worker's scenes, and watching processes.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use silico_core::render::{RenderClient, RenderOutcome, RenderSettings, RenderTicket, Rendered};
use silico_protocol::RenderScene;
use silico_render::bench::front_of_a_box;
use silico_render::test_scenes;

/// The one wait these tests allow for something they expect to happen (a worker to start or
/// end, a render to answer): long enough for a loaded machine, so only a real hang fails,
/// and it costs nothing when the event comes. Half nextest's 120 s, so a hang is said here.
pub const WAIT: Duration = Duration::from_secs(60);

/// A render's time no test reaches: set where a test checks that something comes promptly
/// (a drop, a cancel, a crash seen), so a client that waited out the render's time instead
/// fails at [`WAIT`] rather than passing, however loaded the machine. Ten times `WAIT`.
pub const UNREACHED: Duration = Duration::from_secs(600);

/// `f`'s result, if it returns within [`WAIT`]: for something that should be prompt and
/// would, wrong, block for a render's time ([`UNREACHED`]), which a test must fail at, not
/// wait out. Run on a thread of its own, left blocked if it never returns.
pub fn promptly<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(WAIT)
        .unwrap_or_else(|_| panic!("{what} didn't return in {WAIT:?}"))
}

pub const WORKER: &str = env!("CARGO_BIN_EXE_silico-render-worker");
pub const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-render-test-worker");

pub fn client(exe: &str, settings: RenderSettings) -> RenderClient {
    RenderClient::start(Path::new(exe), settings).unwrap()
}

pub fn drawn(outcome: RenderOutcome) -> Rendered {
    match outcome {
        RenderOutcome::Done(Ok(rendered)) => rendered,
        other => panic!("not drawn: {other:?}"),
    }
}

pub fn failed(outcome: RenderOutcome) -> silico_core::Error {
    match outcome {
        RenderOutcome::Done(Err(e)) => e,
        other => panic!("not failed: {other:?}"),
    }
}

/// The box from the front at one of the test worker's sizes (`test_scenes`).
pub fn at((width, height): (u32, u32)) -> RenderScene {
    front_of_a_box(width, height)
}

/// A scene the test worker hangs on (for 600 s), and one it aborts on.
pub fn hanging() -> RenderScene {
    at(test_scenes::HANGS)
}

pub fn crashing() -> RenderScene {
    at(test_scenes::CRASHES)
}

pub fn small() -> RenderScene {
    front_of_a_box(8, 8)
}

pub fn process_exists(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string()])
        .output()
        .unwrap()
        .status
        .success()
}

/// Whether `pid` is a zombie (exited, not yet reaped by the client).
pub fn zombie(pid: u32) -> bool {
    state(pid).is_some_and(|state| state.starts_with('Z'))
}

/// Whether `pid` has ended, every thread of it, so its parent can reap it: gone, or a
/// zombie with no thread left. On Linux a killed process's first thread is a zombie while
/// its others still end (`ps` says `Zl`, the `l` for more than one thread), and until they
/// have, `waitpid` doesn't give it to its parent: a render worker killed while idle, its
/// wgpu and lavapipe threads still ending, isn't yet ended to the client that asks.
pub fn ended(pid: u32) -> bool {
    match state(pid) {
        None => true,
        Some(state) => state.starts_with('Z') && !state.contains('l'),
    }
}

/// `ps`'s state of `pid` (`R`, `S`, `Z`, ... with its flags), or none if `ps` finds no such
/// process.
pub fn state(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

pub fn signal(pid: u32, sig: &str) {
    let ok = std::process::Command::new("kill")
        .args([sig, &pid.to_string()])
        .status()
        .unwrap()
        .success();
    assert!(ok, "kill {sig} {pid}");
}

/// Waits up to `limit` for `done`.
pub fn eventually(limit: Duration, done: impl Fn() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

/// The ticket's outcome, or None if it doesn't come within `limit`.
pub fn wait_within(ticket: RenderTicket, limit: Duration) -> Option<RenderOutcome> {
    ticket.wait_timeout(limit).ok()
}

/// A scratch directory of this test's own under the system's temporary directory.
pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "silico-adversarial-client-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
