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
    DebugOp, Failure, FailureKind, Generation, MAX_FRAME_LEN, Op, Reply, RequestId, Response,
    write_message,
};

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
        misbehave(&mode, id, generation, socket)
            .map_err(|e| Failure::new(FailureKind::Internal, e.to_string()))?;
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
    // call's 20 s.
    let (result, took, _) = echo_through("over-max", Duration::from_secs(20));
    let err = result.unwrap_err();
    assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
    assert!(err.cause().is_some_and(|c| c.contains("maximum")), "{err}");
    assert!(took < Duration::from_secs(3), "{took:?}");
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

#[test]
fn a_worker_that_closes_its_socket_mid_request_and_lingers_fails_within_its_timeout() {
    // The worker takes the request, closes its socket, and stays alive. The call's timeout
    // (300 ms) covers the whole call: waiting for the worker to exit, before it's killed,
    // stops at the call's deadline, not after a fixed grace of 2 s.
    let exe = script(
        "closes-and-lingers",
        "printf '\\0\\0\\0\\0' >&3\ndd bs=1 count=1 <&3 >/dev/null 2>&1\nexec 3>&-\nexec sleep 600",
    );
    let (err, took) = within(Duration::from_secs(10), move || {
        let mut w = Worker::spawn(&exe).unwrap();
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
    assert!(
        took < Duration::from_millis(1300),
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
    let exe = script(
        "child-holds-pipes",
        &format!(
            "printf '\\0\\0\\0\\0' >&3\ndd bs=1 count=1 <&3 >/dev/null 2>&1\nexec 3>&-\n\
             sleep 600 &\necho $! > '{}'\nexit 0",
            pidfile.display()
        ),
    );
    let (err, took) = within(Duration::from_secs(10), move || {
        let mut w = Worker::spawn(&exe).unwrap();
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
    assert!(
        took < Duration::from_millis(1300),
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
        assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{what}: {err}");
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
                assert_eq!(e.code(), ErrorCode::Internal, "{spare} spare: {e}");
                assert!(e.cause().is_some(), "{spare} spare: {e}");
                failures.push((spare, e));
            }
        }
        wait_no_children(&format!("{spare} spare"));
    }
    wait_fds(fds, "after every start");
    assert!(!failures.is_empty(), "no start ran out of descriptors");
    // Each says it's this process's shortage (`internal`, not `worker_crashed`, which would
    // blame a worker that never ran), and what to do about it.
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
    assert_eq!(err.code(), ErrorCode::Internal, "{err}");
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
    // one that ends before it starts is: about 3 launches in 8.5 s (0, 2, 5 s), not one a
    // second.
    let dir = scratch_dir("starts-then-exits");
    let log = dir.join("launches");
    let exe = script(
        "starts-then-exits",
        &format!(
            "echo x >> '{}'\nprintf '\\0\\0\\0\\0' >&3\nexit 3",
            log.display()
        ),
    );
    let pool = KernelPool::start_workers(&exe, 1).unwrap();
    std::thread::sleep(Duration::from_millis(8_500));
    drop(pool);
    let launches = std::fs::read_to_string(&log).unwrap().lines().count();
    assert!(
        (3..=4).contains(&launches),
        "{launches} launches in 8.5 s idle of a worker that exits on its own"
    );
    wait_no_children("after the drop");
}

/// A worker whose first launch says it has started, then closes its socket and lingers;
/// any later launch is the real test worker.
fn closes_its_socket_once(what: &str) -> PathBuf {
    let dir = scratch_dir(what);
    script(
        what,
        &format!(
            "if mkdir '{}' 2>/dev/null; then printf '\\0\\0\\0\\0' >&3; exec 3>&-; exec sleep 600; fi\n\
             exec '{TEST_WORKER}'",
            dir.join("once").display()
        ),
    )
}

#[test]
fn a_worker_that_closed_its_socket_is_replaced_before_its_next_request() {
    // A request 500 ms after the start, before any idle check: the worker, of no use, is
    // replaced before it's given the request, which runs on the replacement.
    let pool =
        KernelPool::start_workers(&closes_its_socket_once("closed-socket-request"), 1).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let result = done(pool.submit(Generation(1), pool::echo(&[1])));
    let held = pool.pids();
    drop(pool);
    assert_reaped(&held, "after the drop");
    wait_no_children("after the drop");
    assert_eq!(result.unwrap(), Blob::inline(vec![1]));
}

#[test]
fn an_idle_worker_that_closed_its_socket_is_replaced_and_reaped_while_idle() {
    // No request: the idle check, within a second or so, replaces the worker and reaps it.
    let pool = KernelPool::start_workers(&closes_its_socket_once("closed-socket-idle"), 1).unwrap();
    let first = pool.pids();
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
        let start = Instant::now();
        drop(pool);
        let took = start.elapsed();
        assert!(
            took < Duration::from_secs(3),
            "round {round}: the drop took {took:?}"
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
                        ErrorCode::WorkerCrashed | ErrorCode::ProtocolError | ErrorCode::Internal
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
    let pool = KernelPool::start().err().expect("a pool started");
    let worker = KernelWorker::start().err().expect("a worker started");
    for (what, err) in [("the pool", pool), ("a worker", worker)] {
        assert_eq!(
            err.code(),
            silico_core::ErrorCode::InvalidInput,
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
    // SILICO_KERNEL_WORKER empty, naming nothing, or naming a directory is refused as
    // invalid input naming the variable, the first thing to fix, as the default path, when
    // missing, says to set it.
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
/// `a_worker_named_by_a_bare_name_runs_the_one_in_the_working_directory` runs.
const RELATIVE_CHILD: &str = "SILICO_ADVERSARIAL_WORKER_RELATIVE_CHILD";

/// Not a test of its own: in the copy, starts a worker and a pool from the variable.
#[test]
fn relative_child_entry() {
    use silico_core::kernel::KernelWorker;
    if std::env::var_os(RELATIVE_CHILD).is_none() {
        return;
    }
    let worker = KernelWorker::start().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    drop(worker);
    let pool = KernelPool::start().unwrap();
    std::thread::sleep(Duration::from_millis(300));
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
    // and reaps every worker within seconds.
    let exe = script("most-workers", "exec sleep 600");
    let (started, pids, dropped) = within(Duration::from_secs(90), move || {
        let start = Instant::now();
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
        let start = Instant::now();
        drop(pool);
        (started, pids, start.elapsed())
    });
    assert_eq!(pids.len(), silico_core::pool::MAX_WORKERS);
    assert!(
        started < Duration::from_secs(30),
        "the start took {started:?}"
    );
    assert!(
        dropped < Duration::from_secs(10),
        "the drop took {dropped:?}"
    );
    assert_reaped(&pids, "after the drop");
    wait_no_children("after the drop");
}
