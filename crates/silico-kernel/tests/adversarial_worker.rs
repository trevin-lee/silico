//! break-it: the worker client (silico_core::worker) and the kernel pool
//! (silico_core::pool), the M1.9 campaign. Workers that misbehave at the byte level
//! (floods of empty frames, replies cut short, bytes after the message, lengths past the
//! maximum), workers that never start, start and leave, or close their socket and linger;
//! starts that fail every way an executable can fail, or for want of descriptors; and
//! pools dropped in the middle of crashes, kills, floods, and advances, or started and
//! dropped from many threads at once.
//!
//! Like `pool.rs`, these watch this process's children, threads, and descriptors, so they
//! assume one test per process (nextest, which `pixi run test` uses).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/pool.rs"]
mod pool;
#[path = "common/process.rs"]
mod process;
#[path = "common/scripts.rs"]
mod scripts;

use std::collections::BTreeSet;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pool::{
    TEST_WORKER, WORKER, assert_reaped, done, kill9_and_wait, outcome, sleep, spin, state,
    wait_full, wait_no_children,
};
use process::{open_fds, scratch_dir, threads};
use rustix::process::{Resource, getrlimit, setrlimit};
use scripts::script;
use silico_core::kernel::{Blob, KernelRequest};
use silico_core::pool::{KernelPool, Outcome};
use silico_core::worker::Worker;
use silico_core::{Error, ErrorCode};
use silico_protocol::{
    DebugOp, Failure, Generation, MAX_FRAME_LEN, Op, Refusal, Reply, RequestId, Response,
    write_message,
};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

// ---------------------------------------------------------------------------------------
// A fake worker: this test binary, re-run as a worker by a wrapper script (a worker gets no
// environment of its own, so the script sets the mode). It answers as the production worker
// does, except that before answering an echo it writes what its mode says to the socket.

const FAKE_ENV: &str = "SILICO_ADVERSARIAL_WORKER_FAKE";

/// Not a test of its own: when run by [`fake_worker`]'s script, this test binary serves as a
/// kernel worker. Run normally, it does nothing.
#[test]
fn fake_worker_entry() {
    let Ok(mode) = std::env::var(FAKE_ENV) else {
        return;
    };
    let _ = silico_kernel::worker::run(move |id, generation, op, socket| {
        misbehave(&mode, id, generation, socket).map_err(|e| {
            Failure::new(Refusal::Internal {
                what: e.to_string(),
            })
        })?;
        silico_kernel::worker::handle(op)
    });
    std::process::exit(0);
}

/// Writes what `mode` says, before the real reply.
fn misbehave(
    mode: &str,
    id: RequestId,
    generation: Generation,
    socket: &mut dyn Write,
) -> std::io::Result<()> {
    let hang = || std::thread::sleep(Duration::from_secs(600));
    match mode {
        // 100,000 empty frames (each says only "started") before the reply.
        "empty-frames" => {
            for _ in 0..100_000 {
                socket.write_all(&0u32.to_le_bytes())?;
            }
        }
        // A header for 100 bytes, 10 of them, then the worker exits (code 0) or hangs.
        "partial-then-exit" | "partial-then-hang" => {
            socket.write_all(&100u32.to_le_bytes())?;
            socket.write_all(&[7; 10])?;
            socket.flush()?;
            if mode == "partial-then-exit" {
                std::process::exit(0);
            }
            hang();
        }
        // A valid reply with one byte more inside its frame.
        "trailing" => {
            let mut frame = Vec::new();
            let reply = Response {
                id,
                generation,
                result: Ok(Reply::Echo {
                    data: Blob::inline(vec![1]),
                }),
            };
            write_message(&mut frame, &reply).map_err(std::io::Error::other)?;
            let len = u32::from_le_bytes(frame[..4].try_into().unwrap()) + 1;
            frame[..4].copy_from_slice(&len.to_le_bytes());
            frame.push(0);
            socket.write_all(&frame)?;
        }
        // Eight bytes no message encodes to.
        "undecodable" => {
            socket.write_all(&8u32.to_le_bytes())?;
            socket.write_all(&[0xff; 8])?;
        }
        // A length one past the maximum, then nothing (the worker hangs).
        "over-max" => {
            socket.write_all(&(MAX_FRAME_LEN + 1).to_le_bytes())?;
            socket.flush()?;
            hang();
        }
        // Exactly the maximum, then the worker exits with nothing more.
        "max-then-exit" => {
            socket.write_all(&MAX_FRAME_LEN.to_le_bytes())?;
            socket.flush()?;
            std::process::exit(0);
        }
        // Answers, then exits on its own (code 3) a moment later, while idle.
        "exit-after-reply" => {
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(100));
                std::process::exit(3);
            });
        }
        _ => {}
    }
    socket.flush()
}

/// A worker executable that is this test binary in `mode` (see [`fake_worker_entry`]).
fn fake_worker(mode: &str) -> PathBuf {
    let me = std::env::current_exe().unwrap();
    script(
        &format!("fake-{mode}"),
        &format!(
            "{FAKE_ENV}={mode} exec '{}' --exact fake_worker_entry --nocapture --test-threads=1",
            me.display()
        ),
    )
}

/// Echoes `data` through `w`.
fn call_echo(w: &mut Worker, data: &[u8], timeout: Duration) -> Result<Vec<u8>, Error> {
    match w.call(
        Generation(1),
        Op::Echo {
            data: Blob::inline(data.to_vec()),
        },
        timeout,
    )? {
        Reply::Echo { data } => Ok(data.as_bytes().to_vec()),
        other => panic!("Echo replied {other:?}"),
    }
}

/// Runs `f` on another thread, failing the test if it takes longer than `limit`: a hang
/// fails here rather than at nextest's two minutes.
fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit)
        .unwrap_or_else(|_| panic!("still running after {limit:?}"))
}

/// Echoes once through a fresh fake worker in `mode`: the error, how long the call took,
/// and the worker's process ID.
fn echo_through(mode: &str, timeout: Duration) -> (Result<Vec<u8>, Error>, Duration, u32) {
    let exe = fake_worker(mode);
    let mode = mode.to_owned();
    within(Duration::from_secs(30), move || {
        let mut w = Worker::spawn(&exe).unwrap();
        let pid = w.pid();
        let start = Instant::now();
        let result = call_echo(&mut w, &[1, 2, 3], timeout);
        let took = start.elapsed();
        if result.is_err() {
            assert!(w.is_stopped(), "{mode}: a failed worker wasn't stopped");
            // Stopped means reaped: no process has its ID.
            assert_eq!(state(pid), None, "{mode}: the worker wasn't reaped");
            // And a later call fails at once, without touching the socket.
            let again = call_echo(&mut w, &[4], timeout).unwrap_err();
            assert_eq!(again.code(), ErrorCode::WorkerCrashed, "{mode}: {again}");
        }
        (result, took, pid)
    })
}

// ---------------------------------------------------------------------------------------
// Replies that misbehave at the byte level

#[test]
fn a_flood_of_empty_frames_before_the_reply_still_gets_the_reply() {
    let (result, _, _) = echo_through("empty-frames", Duration::from_secs(20));
    assert_eq!(result.unwrap(), [1, 2, 3]);
}

#[test]
fn a_reply_cut_short_by_an_exit_is_a_crash_naming_the_exit() {
    let (result, took, _) = echo_through("partial-then-exit", Duration::from_secs(20));
    let err = result.unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    assert!(
        err.cause()
            .is_some_and(|c| c.contains("exited with code 0")),
        "{err}"
    );
    assert!(err.suggestion().is_some(), "{err}");
    assert!(took < Duration::from_secs(5), "{took:?}");
}

#[test]
fn a_reply_cut_short_by_a_worker_that_then_hangs_times_out() {
    let (result, took, _) = echo_through("partial-then-hang", Duration::from_millis(500));
    let err = result.unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerTimeout, "{err}");
    assert!(took < Duration::from_secs(3), "{took:?}");
}

#[test]
fn a_reply_with_a_byte_after_the_message_is_a_protocol_error() {
    let (result, _, _) = echo_through("trailing", Duration::from_secs(20));
    let err = result.unwrap_err();
    assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
    assert!(
        err.suggestion()
            .is_some_and(|s| s.starts_with("this is a bug in Silico's kernel worker")),
        "{err}"
    );
}

#[test]
fn a_reply_no_message_encodes_to_is_a_protocol_error() {
    let (result, _, _) = echo_through("undecodable", Duration::from_secs(20));
    let err = result.unwrap_err();
    assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
    assert!(err.suggestion().is_some(), "{err}");
}

#[test]
fn a_reply_longer_than_the_maximum_is_refused_without_waiting_for_it() {
    // The worker hangs after the header: refusing it must not wait for 512 MiB, or for the
    // call's timeout, longer than the 30 s echo_through gives the whole call (a refusal that
    // waited would fail there, or as a timeout, not a protocol error).
    let (result, _, _) = echo_through("over-max", Duration::from_secs(120));
    let err = result.unwrap_err();
    assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
    assert!(err.cause().is_some_and(|c| c.contains("maximum")), "{err}");
}

#[test]
fn a_reply_promising_the_maximum_then_ending_is_a_crash() {
    let (result, took, _) = echo_through("max-then-exit", Duration::from_secs(20));
    let err = result.unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    assert!(took < Duration::from_secs(5), "{took:?}");
}

#[test]
fn a_worker_that_answers_then_exits_on_its_own_fails_the_next_call_as_its_install() {
    let exe = fake_worker("exit-after-reply");
    within(Duration::from_secs(30), move || {
        let mut w = Worker::spawn(&exe).unwrap();
        assert_eq!(
            call_echo(&mut w, &[1], Duration::from_secs(20)).unwrap(),
            [1]
        );
        std::thread::sleep(Duration::from_millis(500));
        let err = call_echo(&mut w, &[2], Duration::from_secs(20)).unwrap_err();
        assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
        assert!(
            err.cause()
                .is_some_and(|c| c.contains("exited with code 3")),
            "{err}"
        );
        assert!(
            err.suggestion()
                .is_some_and(|s| s.starts_with("check Silico's install")),
            "{err}"
        );
    });
}

// ---------------------------------------------------------------------------------------
// Workers that never start, or close their socket and linger

#[test]
fn a_worker_that_never_starts_doesnt_have_its_request_blamed() {
    // A worker that never says it has started (here it sleeps; on macOS, a binary held at
    // `_dyld_start` while it's assessed, for 17 s and more) fails its first request as a
    // timeout, but not one that blames the request ("simplify the operation"): it never had
    // the request to work on.
    let exe = script("never-starts", "exec sleep 600");
    let err = within(Duration::from_secs(10), move || {
        let mut w = Worker::spawn(&exe).unwrap();
        call_echo(&mut w, &[1], Duration::from_millis(500)).unwrap_err()
    });
    assert_eq!(err.code(), ErrorCode::WorkerTimeout, "{err}");
    assert!(
        err.cause().is_some_and(|c| c.contains("hadn't started")),
        "{err}"
    );
    assert!(
        err.suggestion()
            .is_some_and(|s| s.starts_with("try the request again")),
        "a worker that never started blamed the request: {err}"
    );
}

#[test]
fn a_started_worker_that_times_out_blames_the_request() {
    // The other side of the one above: a worker that has started (it answered) and then
    // runs past the timeout was working on the request.
    let err = within(Duration::from_secs(10), || {
        let mut w = Worker::spawn(Path::new(TEST_WORKER)).unwrap();
        call_echo(&mut w, &[1], Duration::from_secs(20)).unwrap();
        w.call(
            Generation(1),
            Op::Debug(DebugOp::Sleep { millis: 60_000 }),
            Duration::from_millis(300),
        )
        .unwrap_err()
    });
    assert_eq!(err.code(), ErrorCode::WorkerTimeout, "{err}");
    assert!(
        err.suggestion()
            .is_some_and(|s| s.starts_with("simplify the operation")),
        "{err}"
    );
    assert!(err.cause().is_none(), "{err}");
}

/// Waits up to ten seconds for a stand-in worker to make `marker`, so a test's timeout
/// counts from there, not from the script's start, which a loaded machine can slow.
fn wait_for(marker: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "{} never made", marker.display());
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The graces a crashed call gave its worker to exit before killing it, as it says them:
/// what it decided from the call's deadline, which a loaded machine can't change.
#[derive(Clone, Default)]
struct Graces(Arc<Mutex<Vec<u64>>>);

impl<S: Subscriber> Layer<S> for Graces {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        struct Grace(Option<u64>);
        impl Visit for Grace {
            fn record_u64(&mut self, field: &Field, value: u64) {
                if field.name() == "grace_ms" {
                    self.0 = Some(value);
                }
            }
            fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
        }
        let mut grace = Grace(None);
        event.record(&mut grace);
        if let Some(ms) = grace.0 {
            self.0.lock().unwrap().push(ms);
        }
    }
}

impl Graces {
    /// Installs the subscriber that records them, for this test's process.
    fn record() -> Graces {
        let graces = Graces::default();
        tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(graces.clone()),
        )
        .expect("one test a process, as nextest runs them");
        graces
    }

    fn all(&self) -> Vec<u64> {
        self.0.lock().unwrap().clone()
    }
}

#[test]
fn a_worker_that_closes_its_socket_mid_request_and_lingers_fails_within_its_timeout() {
    // The worker takes the request, closes its socket, and stays alive. The call's timeout
    // (300 ms) covers the whole call: waiting for the worker to exit, before it's killed,
    // stops at the call's deadline, not after a fixed grace of 2 s: the grace it decides is
    // what's left of the 300 ms (at least 100 ms), not 2000.
    let started = scratch_dir("closes-and-lingers").join("started");
    let exe = script(
        "closes-and-lingers",
        &format!(
            "printf '\\0\\0\\0\\0' >&3\ntouch '{}'\ndd bs=1 count=1 <&3 >/dev/null 2>&1\n\
             exec 3>&-\nexec sleep 600",
            started.display()
        ),
    );
    let graces = Graces::record();
    let (err, took) = within(Duration::from_secs(20), move || {
        let mut w = Worker::spawn(&exe).unwrap();
        wait_for(&started);
        let start = Instant::now();
        let err = call_echo(&mut w, &[1], Duration::from_millis(300)).unwrap_err();
        (err, start.elapsed())
    });
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    assert!(
        err.cause()
            .is_some_and(|c| c.starts_with("it stopped responding and was killed")),
        "{err}"
    );
    assert!(err.suggestion().is_some(), "{err}");
    let graces = graces.all();
    assert!(
        matches!(graces[..], [grace] if (100..=300).contains(&grace)),
        "a call allowed 300 ms gave its worker {graces:?} ms to exit: {err}"
    );
    // And waited no more than it said: a fixed grace (2 s) anywhere it waits, for the exit,
    // its replies, or its log, would take at least that. This one is a bound on time, with
    // 1.7 s of room: telling 300 ms from 2 s needs one.
    assert!(
        took < Duration::from_secs(2),
        "a call allowed 300 ms took {took:?}: {err}"
    );
}

#[test]
fn a_worker_whose_child_holds_its_pipes_fails_within_its_timeout() {
    // The worker takes the request and exits, but a process it started holds its log open:
    // waiting to read it to its end stops at the call's deadline too. (One that held the
    // socket too would keep the call waiting for a reply, until its timeout.)
    let dir = scratch_dir("child-holds-pipes");
    let pidfile = dir.join("pid");
    let started = dir.join("started");
    let exe = script(
        "child-holds-pipes",
        &format!(
            "printf '\\0\\0\\0\\0' >&3\ntouch '{}'\ndd bs=1 count=1 <&3 >/dev/null 2>&1\n\
             exec 3>&-\nsleep 600 &\necho $! > '{}'\nexit 0",
            started.display(),
            pidfile.display()
        ),
    );
    let graces = Graces::record();
    let (err, took) = within(Duration::from_secs(20), move || {
        let mut w = Worker::spawn(&exe).unwrap();
        wait_for(&started);
        let start = Instant::now();
        let err = call_echo(&mut w, &[1], Duration::from_millis(300)).unwrap_err();
        (err, start.elapsed())
    });
    if let Ok(pid) = std::fs::read_to_string(&pidfile) {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", pid.trim()])
            .status();
    }
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    let graces = graces.all();
    assert!(
        matches!(graces[..], [grace] if (100..=300).contains(&grace)),
        "a call allowed 300 ms gave its worker {graces:?} ms to exit: {err}"
    );
    // And waited no more than it said: a fixed grace (2 s) anywhere it waits, for the exit,
    // its replies, or its log, would take at least that. This one is a bound on time, with
    // 1.7 s of room: telling 300 ms from 2 s needs one.
    assert!(
        took < Duration::from_secs(2),
        "a call allowed 300 ms took {took:?}: {err}"
    );
}

// ---------------------------------------------------------------------------------------
// Starts that fail

#[test]
fn every_way_a_worker_cant_be_started_says_what_to_try() {
    // An executable that can't be started at all fails `Worker::spawn` with a code, a cause,
    // and what to try, as every other worker failure does (`every_failure_says_what_to_try`
    // in worker.rs); so does the pool's start, and a request while its binary is missing.
    let dir = scratch_dir("cant-start");
    let not_executable = dir.join("not-executable");
    std::fs::write(&not_executable, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&not_executable, std::fs::Permissions::from_mode(0o644)).unwrap();
    let no_interpreter = dir.join("no-interpreter");
    std::fs::write(&no_interpreter, "#!/nonexistent/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&no_interpreter, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cases = [
        ("missing", dir.join("missing")),
        ("a directory", dir.clone()),
        ("not executable", not_executable),
        ("an interpreter that's missing", no_interpreter),
        ("an empty path", PathBuf::new()),
    ];
    let mut bad = Vec::new();
    for (what, exe) in &cases {
        let err = match Worker::spawn(exe) {
            Ok(_) => panic!("{what}: started"),
            Err(e) => e,
        };
        assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{what}: {err}");
        assert!(
            err.message().contains(&*exe.to_string_lossy()),
            "{what}: {err}"
        );
        assert!(err.cause().is_some(), "{what}: {err}");
        if err.suggestion().is_none() {
            bad.push(format!("{what}: {err}"));
        }
    }
    // A pool whose binary is missing at start, or goes missing under it.
    let missing = dir.join("missing");
    match KernelPool::start_workers(&missing, 2) {
        Ok(_) => panic!("a pool of a missing binary started"),
        Err(e) if e.suggestion().is_none() => bad.push(format!("a pool's start: {e}")),
        Err(_) => {}
    }
    let exe = dir.join("worker");
    std::os::unix::fs::symlink(TEST_WORKER, &exe).unwrap();
    let pool = KernelPool::start_workers(&exe, 1).unwrap();
    std::fs::remove_file(&exe).unwrap();
    kill9_and_wait(pool.pids()[0]);
    let err = done(pool.submit(Generation(1), sleep(1))).unwrap_err();
    if err.suggestion().is_none() {
        bad.push(format!("a request after the binary went: {err}"));
    }
    assert!(
        bad.is_empty(),
        "errors with no suggestion:\n{}",
        bad.join("\n")
    );
}

/// Runs `f` with this process allowed only `spare` descriptors more than it has open, and
/// puts the limit back after.
fn short_of_descriptors<T>(spare: u64, f: impl FnOnce() -> T) -> T {
    let saved = getrlimit(Resource::Nofile);
    let open = open_fds() as u64;
    setrlimit(
        Resource::Nofile,
        rustix::process::Rlimit {
            current: Some(open + spare),
            maximum: saved.maximum,
        },
    )
    .unwrap();
    let result = f();
    setrlimit(Resource::Nofile, saved).unwrap();
    result
}

/// Waits up to five seconds for this process's open descriptors to fall to `fds`.
fn wait_fds(fds: usize, context: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while open_fds() > fds {
        assert!(
            Instant::now() < deadline,
            "{context}: descriptors {fds} → {}",
            open_fds()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_start_short_of_descriptors_fails_cleanly_at_every_step() {
    // The app at its descriptor limit, with 0 to 15 to spare: each start runs out at another
    // of its steps (the socket pair, the log pipe, /dev/null, `spawn`'s own pipe, the reply
    // thread's copy of the socket). Each fails with a structured error saying what to try,
    // or starts; either way no process or descriptor is left behind.
    let fds = open_fds();
    let mut failures = Vec::new();
    for spare in 0..16 {
        let result = short_of_descriptors(spare, || Worker::spawn(Path::new(TEST_WORKER)));
        match result {
            Ok(w) => drop(w),
            Err(e) => {
                assert_eq!(e.code(), ErrorCode::ResourceExhausted, "{spare} spare: {e}");
                assert!(e.cause().is_some(), "{spare} spare: {e}");
                failures.push((spare, e));
            }
        }
        wait_no_children(&format!("{spare} spare"));
    }
    wait_fds(fds, "after every start");
    assert!(!failures.is_empty(), "no start ran out of descriptors");
    // Each says it's this process's shortage (`resource_exhausted`, not `worker_unavailable`
    // or `worker_crashed`, which would blame the worker or its install), and what to do
    // about it.
    let unadvised: Vec<String> = failures
        .iter()
        .filter(|(_, e)| {
            !e.suggestion()
                .is_some_and(|s| s.contains("too many files open"))
        })
        .map(|(spare, e)| format!("{spare} spare: {e}"))
        .collect();
    assert!(
        unadvised.is_empty(),
        "starts out of descriptors with no suggestion:\n{}",
        unadvised.join("\n")
    );
}

#[test]
fn a_pool_short_of_descriptors_fails_its_request_cleanly_and_recovers() {
    // The pool's worker dies while the app is out of descriptors: the request that finds it
    // dead can't start another and fails, structured; once descriptors are back, the next
    // request runs and the pool is full again, with nothing left behind.
    let pool = pool::pool(TEST_WORKER, 1);
    let first = pool.pids();
    kill9_and_wait(first[0]);
    let err = short_of_descriptors(0, || done(pool.submit(Generation(1), sleep(1)))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ResourceExhausted, "{err}");
    assert!(err.cause().is_some(), "{err}");
    done(pool.submit(Generation(1), sleep(1))).unwrap();
    let held = wait_full(&pool, 1, &first.iter().copied().collect());
    drop(pool);
    assert_reaped(&held, "after the drop");
    wait_no_children("after the drop");
}

// ---------------------------------------------------------------------------------------
// The pool and workers that start, then leave or go quiet

#[test]
fn an_idle_slot_whose_worker_starts_then_exits_on_its_own_backs_off() {
    // A worker that says it has started and then exits on its own (code 3) while idle is
    // the install's, as its crash advice says, so it's started again less and less often, as
    // one that ends before it starts is: launches at about 0, 2, 5, and 10 s, not one a
    // second. Each gap is longer than the wait before it (1, 2, and 4 s), counted from the
    // idle check that sees the exit, after the launch was logged: so however late a loaded
    // machine runs the checks.
    let dir = scratch_dir("starts-then-exits");
    let log = dir.join("launches");
    let exe = script(
        "starts-then-exits",
        &format!(
            "perl -MTime::HiRes=time -e 'print time, \"\\n\"' >> '{}'\n\
             printf '\\0\\0\\0\\0' >&3\nexit 3",
            log.display()
        ),
    );
    let pool = KernelPool::start_workers(&exe, 1).unwrap();
    // A hang guard: the fourth launch comes at about 10 s.
    let deadline = Instant::now() + Duration::from_secs(60);
    let times = loop {
        let times: Vec<f64> = std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .map(|l| l.trim().parse().unwrap())
            .collect();
        if times.len() >= 4 {
            break times;
        }
        assert!(Instant::now() < deadline, "launches at {times:?}");
        std::thread::sleep(Duration::from_millis(5));
    };
    drop(pool);
    let gaps: Vec<f64> = times.windows(2).map(|w| w[1] - w[0]).collect();
    for (k, wait) in [1.0, 2.0, 4.0].into_iter().enumerate() {
        assert!(gaps[k] > wait, "gap {k} not over {wait} s: gaps {gaps:?}");
    }
    wait_no_children("after the drop");
}

/// A worker whose first launch says it has started, closes its socket, makes the marker
/// returned, and lingers; any later launch is the real test worker.
fn closes_its_socket_once(what: &str) -> (PathBuf, PathBuf) {
    let dir = scratch_dir(what);
    let closed = dir.join("closed");
    let exe = script(
        what,
        &format!(
            "if mkdir '{}' 2>/dev/null; then printf '\\0\\0\\0\\0' >&3; exec 3>&-; \
             touch '{}'; exec sleep 600; fi\nexec '{TEST_WORKER}'",
            dir.join("once").display(),
            closed.display()
        ),
    );
    (exe, closed)
}

#[test]
fn a_worker_that_closed_its_socket_is_replaced_before_its_next_request() {
    // A request as soon as the worker has closed its socket, before an idle check (a
    // second after the start) would replace it: the worker, of no use, is replaced before
    // it's given the request, which runs on the replacement.
    let (exe, closed) = closes_its_socket_once("closed-socket-request");
    let pool = KernelPool::start_workers(&exe, 1).unwrap();
    wait_for(&closed);
    // Time for the core to read the socket's end; well short of an idle check.
    std::thread::sleep(Duration::from_millis(100));
    let result = done(pool.submit(Generation(1), pool::echo(&[1])));
    let held = pool.pids();
    drop(pool);
    assert_reaped(&held, "after the drop");
    wait_no_children("after the drop");
    assert_eq!(result.unwrap(), Blob::inline(vec![1]));
}

#[test]
fn an_idle_worker_that_closed_its_socket_is_replaced_and_reaped_while_idle() {
    // No request: once the worker has closed its socket, the idle check, within a second or
    // so, replaces the worker and reaps it.
    let (exe, closed) = closes_its_socket_once("closed-socket-idle");
    let pool = KernelPool::start_workers(&exe, 1).unwrap();
    let first = pool.pids();
    wait_for(&closed);
    let deadline = Instant::now() + Duration::from_secs(3);
    let held = loop {
        let pids = pool.pids();
        if pids.len() == 1 && pids[0] != first[0] {
            break pids;
        }
        assert!(Instant::now() < deadline, "still {pids:?}");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(state(first[0]), None, "the lingering worker wasn't reaped");
    drop(pool);
    assert_reaped(&held, "after the drop");
    wait_no_children("after the drop");
}

// ---------------------------------------------------------------------------------------
// Dropping the pool in the middle of everything

/// A small deterministic sequence, so every run hits the same moments.
fn next(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *seed >> 33
}

enum Sent {
    Echo(Vec<u8>, silico_core::pool::Pending<Blob>),
    Other(&'static str, silico_core::pool::Pending<()>),
}

#[test]
fn dropping_the_pool_amid_crashes_kills_floods_and_advances_leaves_nothing() {
    // 20 rounds. In each, a pool of 3 takes a flood of echoes, sleeps, segfaults, aborts,
    // and garbage at whatever generation it's on, while another thread advances it and a
    // third SIGKILLs its workers, with the stop-after varied; then it's dropped at a moment
    // that differs each round. The drop ends at once, every request ends (its own bytes, a
    // structured failure, superseded, or "the pool stopped"), and no process, thread, or
    // descriptor outlives the pools.
    drop(pool::pool(TEST_WORKER, 1));
    std::thread::sleep(Duration::from_millis(200));
    let (fds, threads_before) = (open_fds(), threads());
    let mut seed = 7u64;
    for round in 0..20u64 {
        let pool = pool::pool(TEST_WORKER, 3);
        pool.set_stop_after(Duration::from_millis(next(&mut seed) % 40));
        let stop = AtomicBool::new(false);
        let run_for = Duration::from_millis(30 + next(&mut seed) % 300);
        let mut killed = BTreeSet::new();
        let sent = std::thread::scope(|scope| {
            let (pool, stop) = (&pool, &stop);
            let submitter = scope.spawn(move || {
                let mut seed = round;
                let mut sent = Vec::new();
                let mut n = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    let g = Generation(pool.generation().0 + next(&mut seed) % 2);
                    let kind = next(&mut seed) % 10;
                    sent.push(match kind {
                        0 => Sent::Other(
                            "segfault",
                            pool.submit(g, KernelRequest::debug(DebugOp::Segfault)),
                        ),
                        1 => Sent::Other(
                            "abort",
                            pool.submit(g, KernelRequest::debug(DebugOp::Abort)),
                        ),
                        2 => Sent::Other(
                            "garbage",
                            pool.submit(g, KernelRequest::debug(DebugOp::Garbage)),
                        ),
                        3..=5 => Sent::Other("sleep", pool.submit(g, sleep(next(&mut seed) % 30))),
                        _ => {
                            let bytes = n.to_le_bytes().to_vec();
                            Sent::Echo(bytes.clone(), pool.submit(g, pool::echo(&bytes)))
                        }
                    });
                    n += 1;
                    spin(Duration::from_micros(next(&mut seed) % 400));
                }
                sent
            });
            let advancer = scope.spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    pool.advance(Generation(pool.generation().0 + 1));
                    std::thread::sleep(Duration::from_millis(7));
                }
            });
            let start = Instant::now();
            let mut seed = round + 100;
            while start.elapsed() < run_for {
                let pids = pool.pids();
                if !pids.is_empty() && next(&mut seed).is_multiple_of(3) {
                    let pid = pids[(next(&mut seed) as usize) % pids.len()];
                    let _ = std::process::Command::new("kill")
                        .args(["-KILL", &pid.to_string()])
                        .status();
                    killed.insert(pid);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            stop.store(true, Ordering::Relaxed);
            advancer.join().unwrap();
            submitter.join().unwrap()
        });
        let mut held = pool.pids();
        held.extend(killed);
        // Nothing in flight here runs long (sleeps of at most 30 ms, a stop-after of at most
        // 40 ms; crashes, aborts, and garbage end at once), so a drop that waited for what
        // was running would still end within milliseconds: what it mustn't do is wait for
        // something that never comes (a thread or the stopper left asleep, a worker left
        // running). So the wait for the drop is a hang guard, which a loaded machine's slow
        // starts and reaps don't reach.
        let (dropped, returned) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(pool);
            let _ = dropped.send(());
        });
        assert!(
            returned.recv_timeout(Duration::from_secs(30)).is_ok(),
            "round {round}: the drop hung"
        );
        assert_reaped(&held, &format!("round {round}"));
        for s in sent {
            let failure = match s {
                Sent::Echo(bytes, p) => match outcome(p) {
                    Outcome::Done(Ok(blob)) => {
                        assert_eq!(blob, Blob::inline(bytes), "round {round}");
                        None
                    }
                    Outcome::Done(Err(e)) => Some(("echo", e)),
                    Outcome::Superseded => None,
                },
                Sent::Other(what, p) => match outcome(p) {
                    Outcome::Done(Ok(())) => {
                        assert_eq!(what, "sleep", "round {round}: {what} succeeded");
                        None
                    }
                    Outcome::Done(Err(e)) => Some((what, e)),
                    Outcome::Superseded => None,
                },
            };
            if let Some((what, e)) = failure {
                assert!(
                    matches!(
                        e.code(),
                        ErrorCode::WorkerCrashed
                            | ErrorCode::WorkerUnavailable
                            | ErrorCode::ProtocolError
                            | ErrorCode::Internal
                    ),
                    "round {round}: {what}: {e}"
                );
                assert!(e.suggestion().is_some(), "round {round}: {what}: {e}");
            }
        }
        wait_no_children(&format!("round {round}"));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while (open_fds() > fds || threads() > threads_before) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(open_fds() <= fds, "descriptors: {fds} → {}", open_fds());
    assert!(
        threads() <= threads_before,
        "threads: {threads_before} → {}",
        threads()
    );
}

#[test]
fn pools_started_and_dropped_from_many_threads_at_once_leave_nothing() {
    // Six threads each start a pool of four, run a request on it, and drop it, five times:
    // their starts contend for the one start lock. None hangs, every request runs, and
    // nothing outlives the pools.
    let fds = open_fds();
    within(Duration::from_secs(60), || {
        for round in 0..5 {
            std::thread::scope(|scope| {
                for t in 0..6u8 {
                    scope.spawn(move || {
                        let pool = pool::pool(WORKER, 4);
                        assert_eq!(
                            done(pool.submit(Generation(0), pool::echo(&[t, round]))).unwrap(),
                            Blob::inline(vec![t, round])
                        );
                    });
                }
            });
        }
    });
    wait_no_children("after every pool");
    wait_fds(fds, "after every pool");
}

// ---------------------------------------------------------------------------------------
// Where the worker is found, and how many

/// Set in a copy of this test binary that `a_worker_named_by_an_unusable_variable_says_so`
/// runs, with `SILICO_KERNEL_WORKER` set (the environment can't be changed in-process).
const START_CHILD: &str = "SILICO_ADVERSARIAL_WORKER_START_CHILD";

/// Not a test of its own: in the copy, starting a pool and a worker must fail naming the
/// variable that pointed at nothing usable.
#[test]
fn start_child_entry() {
    use silico_core::kernel::{KERNEL_WORKER_ENV, KernelWorker};
    if std::env::var_os(START_CHILD).is_none() {
        return;
    }
    // A pool that couldn't be opened is still one: each request fails with why.
    let pool =
        done(KernelPool::start().submit(Generation(1), sleep(0))).expect_err("a pool started");
    let worker = KernelWorker::start().err().expect("a worker started");
    for (what, err) in [("the pool", pool), ("a worker", worker)] {
        assert_eq!(
            err.code(),
            silico_core::ErrorCode::WorkerUnavailable,
            "{what}: {err}"
        );
        let says = |text: Option<&str>| text.is_some_and(|t| t.contains(KERNEL_WORKER_ENV));
        assert!(
            says(Some(err.message())) || says(err.suggestion()),
            "{what}: {err}"
        );
    }
}

#[test]
fn a_worker_named_by_an_unusable_variable_says_so() {
    // SILICO_KERNEL_WORKER empty, naming nothing, or naming a directory is refused as a
    // worker that can't be had (worker_unavailable), naming the variable, the first thing
    // to fix, as the default path, when missing, says to set it.
    use silico_core::kernel::KERNEL_WORKER_ENV;
    let me = std::env::current_exe().unwrap();
    let mut bad = Vec::new();
    for value in [
        "",
        "/nonexistent/silico-kernel-worker",
        "not-here/worker",
        "/",
    ] {
        let out = std::process::Command::new(&me)
            .args([
                "--exact",
                "start_child_entry",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(START_CHILD, "1")
            .env(KERNEL_WORKER_ENV, value)
            .output()
            .unwrap();
        if !out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout) + String::from_utf8_lossy(&out.stderr);
            let said = text
                .lines()
                .filter(|l| l.contains(": ") && (l.contains("the pool") || l.contains("a worker")))
                .collect::<Vec<_>>()
                .join("\n");
            bad.push(format!("{KERNEL_WORKER_ENV}={value:?}:\n{said}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// Set in a copy of this test binary that
/// `a_pool_started_before_its_worker_was_built_finds_it_on_a_later_request` runs, with
/// `SILICO_KERNEL_WORKER` naming where the worker will be.
const LATER_CHILD: &str = "SILICO_ADVERSARIAL_WORKER_LATER_CHILD";

/// Not a test of its own: in the copy, a pool started with no worker where the variable
/// points refuses each request, saying why its last look failed; once the worker is put
/// there, the next requests run, the pool's and a client's made before it opened, eight at
/// once opening it once; the pool then has the worker's identity, and stops superseded
/// requests as an opened pool does.
#[test]
fn later_child_entry() {
    use silico_core::kernel::{KERNEL_WORKER_ENV, Kernel as _};
    if std::env::var_os(LATER_CHILD).is_none() {
        return;
    }
    let exe = PathBuf::from(std::env::var_os(KERNEL_WORKER_ENV).unwrap());
    let pool = KernelPool::start();
    let client = pool.client();
    let built = |identity: &str| identity.starts_with("kernel worker sha256:");
    // Nothing there yet: refused, as a worker that can't be had, and no slot.
    let missing =
        done(pool.submit(Generation(1), sleep(0))).expect_err("a pool with no worker ran");
    assert_eq!(missing.code(), ErrorCode::WorkerUnavailable, "{missing}");
    assert_eq!(pool.refused(), Some(missing.clone()));
    assert_eq!((pool.size(), pool.pids()), (0, Vec::new()));
    assert!(
        !built(pool.identity()) && !built(client.identity()),
        "{}",
        pool.identity()
    );
    // Something there it can't use (a file it may not read): the next look says so.
    std::fs::write(&exe, format!("#!/bin/sh\nexec '{TEST_WORKER}'\n")).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o000)).unwrap();
    let unreadable =
        done(client.submit(Generation(0), sleep(0))).expect_err("an unreadable worker ran");
    assert_ne!(unreadable.message(), missing.message(), "{unreadable}");
    assert_eq!(pool.refused(), Some(unreadable));
    // Half put in place (a copy under way): it doesn't answer, so the pool isn't opened on
    // it, and goes on looking.
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(&exe, "#!/bin/sh\nexi").unwrap();
    let half = done(pool.submit(Generation(1), sleep(0))).expect_err("a half-copied worker ran");
    assert_eq!(pool.refused(), Some(half));
    assert_eq!(pool.size(), 0);
    assert!(!built(pool.identity()), "{}", pool.identity());
    std::fs::write(&exe, format!("#!/bin/sh\nexec '{TEST_WORKER}'\n")).unwrap();
    // The worker built after the pool started: eight requests at once find it, and run,
    // and the pool opens once (each starts a worker of its own at most, none lost).
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                barrier.wait();
                done(pool.submit(Generation(1), sleep(50))).unwrap();
            });
        }
    });
    assert_eq!(pool.refused(), None);
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    assert_eq!(pool.size(), cores.min(silico_core::pool::MAX_WORKERS));
    let pids = pool.pids();
    let distinct: BTreeSet<u32> = pids.iter().copied().collect();
    assert!(pids.len() <= 8 && distinct.len() == pids.len(), "{pids:?}");
    // The worker's identity now, the pool's and a client's made before it opened alike.
    assert!(built(pool.identity()), "{}", pool.identity());
    assert_eq!(client.identity(), pool.identity());
    // A client made before the pool opened is served by it too, and its superseded request
    // is stopped by the stopper the open started, long before the request would end.
    done(client.submit(Generation(0), sleep(0))).unwrap();
    // Superseded before it's due (a second's stop-after, 100 ms in), so only the stopper,
    // not the advance itself, can stop it.
    pool.set_stop_after(Duration::from_secs(1));
    let long = client.submit(Generation(1), sleep(60_000));
    std::thread::sleep(Duration::from_millis(100));
    let asked = Instant::now();
    client.advance(Generation(2));
    assert!(matches!(outcome(long), Outcome::Superseded));
    assert!(
        asked.elapsed() < Duration::from_secs(20),
        "{:?}",
        asked.elapsed()
    );
    // Dropping it stops every worker it started.
    drop(client);
    drop(pool);
    wait_no_children("after the pool opened late");
}

#[test]
fn a_pool_started_before_its_worker_was_built_finds_it_on_a_later_request() {
    // An app started before its kernel worker was built: its pool refuses at first, and
    // finds the worker on a later request instead of keeping the error (#196's M5.4).
    use silico_core::kernel::KERNEL_WORKER_ENV;
    let dir = scratch_dir("later");
    let exe = dir.join("silico-kernel-worker");
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "later_child_entry",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(LATER_CHILD, "1")
        .env(KERNEL_WORKER_ENV, &exe)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Set in a copy of this test binary that
/// `a_worker_named_by_a_bare_name_runs_the_one_in_the_working_directory` runs.
const RELATIVE_CHILD: &str = "SILICO_ADVERSARIAL_WORKER_RELATIVE_CHILD";

/// Not a test of its own: in the copy, starts a worker and a pool from the variable, and
/// has each answer a request.
#[test]
fn relative_child_entry() {
    use silico_core::kernel::KernelWorker;
    if std::env::var_os(RELATIVE_CHILD).is_none() {
        return;
    }
    // Each answers a request, so its script has run before it's dropped.
    let mut worker = KernelWorker::start().unwrap();
    worker
        .run(KernelRequest::debug(DebugOp::Sleep { millis: 0 }))
        .unwrap();
    drop(worker);
    let pool = KernelPool::start();
    done(pool.submit(Generation(1), sleep(0))).unwrap();
    drop(pool);
}

#[test]
fn a_worker_named_by_a_bare_name_runs_the_one_in_the_working_directory() {
    // SILICO_KERNEL_WORKER=silico-kernel-worker, with one in the working directory and
    // another on PATH: the one checked, in the working directory, is the one run, by a
    // worker and by a pool alike.
    use silico_core::kernel::KERNEL_WORKER_ENV;
    let dir = scratch_dir("bare-name");
    let marker = dir.join("ran");
    for place in ["cwd", "bin"] {
        std::fs::create_dir_all(dir.join(place)).unwrap();
        let exe = dir.join(place).join("silico-kernel-worker");
        std::fs::write(
            &exe,
            format!(
                "#!/bin/sh\necho {place} >> '{}'\nexec '{TEST_WORKER}'\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "relative_child_entry",
            "--nocapture",
            "--test-threads=1",
        ])
        .current_dir(dir.join("cwd"))
        .env(RELATIVE_CHILD, "1")
        .env(KERNEL_WORKER_ENV, "silico-kernel-worker")
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", dir.join("bin").display()),
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let ran = std::fs::read_to_string(&marker).unwrap();
    assert!(
        !ran.is_empty() && ran.lines().all(|l| l == "cwd"),
        "ran: {ran:?}"
    );
}

#[test]
fn a_pool_of_the_most_workers_starts_and_stops_cleanly() {
    // MAX_WORKERS slots of a stand-in worker that never starts: 1024 processes and three
    // threads each in the app. The pool starts (or fails, structured), and its drop kills
    // and reaps every worker. Bounded by this process's CPU time, not the wall clock: on a
    // machine other work saturates, the wall clock measures the machine (1024 starts took
    // 35–76 s on CI under mutants shards, against 6 s alone), while what starting and
    // dropping cost here is the pool's. Measured on an M-series Mac: about 4 s to start and
    // 0.5 s to drop, 5 s and 0.4 s with the machine at a load of 43. On Linux each start
    // forks this whole process (the socket's descriptor is passed in the child before exec),
    // growing by three threads a worker, so it costs far more: 32 s on the CI runner with
    // six mutants shards running. The bounds are about four times what was measured, per
    // platform. The wall clock bounds
    // only a hang, at 9 minutes (nextest allows this test 10: .config/nextest.toml): so a
    // start or drop that waits on each worker is caught once the wait is about half a
    // second (a handshake's timeout, say), not a brief sleep, which the CPU time doesn't see.
    let exe = script("most-workers", "exec sleep 600");
    let (started, pids, dropped) = within(Duration::from_secs(540), move || {
        let start = cpu_time::ProcessTime::now();
        let pool = KernelPool::start_workers(&exe, silico_core::pool::MAX_WORKERS);
        let started = start.elapsed();
        let pool = match pool {
            Ok(pool) => pool,
            Err(e) => {
                assert!(e.suggestion().is_some() || e.cause().is_some(), "{e}");
                return (started, Vec::new(), Duration::ZERO);
            }
        };
        let pids = pool.pids();
        let start = cpu_time::ProcessTime::now();
        drop(pool);
        (started, pids, start.elapsed())
    });
    assert_eq!(pids.len(), silico_core::pool::MAX_WORKERS);
    let most = if cfg!(target_os = "linux") { 120 } else { 30 };
    assert!(
        started < Duration::from_secs(most),
        "the start took {started:?} of CPU"
    );
    assert!(
        dropped < Duration::from_secs(5),
        "the drop took {dropped:?} of CPU"
    );
    assert_reaped(&pids, "after the drop");
    wait_no_children("after the drop");
}
