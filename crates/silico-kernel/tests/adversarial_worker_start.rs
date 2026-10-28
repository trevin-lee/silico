//! break-it for the start code (worker_unavailable against worker_crashed): how a worker that
//! never said it started, or can't be started at all, is reported, by the core's worker
//! client (`Worker`), the sketch worker (`SketchWorker`), and the kernel pool. Expected
//! values come from the codes' own contract (error.rs): `worker_unavailable` is a worker
//! that never reached its first frame (Silico's install or environment, not the request),
//! `worker_crashed` one that had started; and from POSIX's exec rules for what can be run.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::scripts;
use crate::wait::WAIT;

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use scripts::script;
use silico_core::kernel::{Blob, KernelRequest};
use silico_core::pool::KernelPool;
use silico_core::sketch::{SketchSolver, SketchWorker};
use silico_core::worker::Worker;
use silico_core::{Error, ErrorCode};
use silico_protocol::{Algorithm, Generation, Op, Reply, SketchSystem};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

/// A first frame, as `silico_protocol::write_started` writes it: an empty frame.
const STARTED: &str = "printf '\\0\\0\\0\\0' >&3";

fn echo(w: &mut Worker, timeout: Duration) -> Result<Vec<u8>, Error> {
    match w.call(
        Generation(1),
        Op::Echo {
            data: Blob::inline(vec![1, 2, 3]),
        },
        timeout,
    )? {
        Reply::Echo { data } => Ok(data.as_bytes().to_vec()),
        other => panic!("Echo replied {other:?}"),
    }
}

fn empty_system() -> SketchSystem {
    SketchSystem {
        values: vec![],
        unknowns: vec![],
        geometry: vec![],
        constraints: vec![],
        algorithms: vec![Algorithm::DogLeg],
    }
}

fn kill(pid: u32, signal: &str) {
    assert!(
        Command::new("kill")
            .args([&format!("-{signal}"), &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
}

/// A scratch directory of the test's own.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("adv-start-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

// --- The sketch worker ------------------------------------------------------------------

#[test]
fn a_sketch_worker_that_exits_at_launch_is_unavailable_however_late_the_solve_comes() {
    // Found by break-it: a sketch worker that exits as it starts (a missing library, an
    // exit at launch: never a first frame) was worker_unavailable when the solve's request
    // found it dying, but worker_crashed when the solve found it already gone: a solve begun
    // with a stopper (as the engine begins every one) checked `has_exited` first and worded
    // that as a crash (sketch.rs `solve`), never asking whether it had started. Here the
    // solve comes 300 ms after the start.
    let exe = script("sketch-exits-at-launch", "exit 3");
    let mut solver = SketchWorker::start_worker(&exe).unwrap();
    let _stopper = solver.stopper().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let err = solver.solve(empty_system()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
}

#[test]
fn a_sketch_worker_that_exits_at_launch_gets_one_code_every_time() {
    // Found by break-it (as above): the same exiting binary, solved at once and solved late,
    // got two codes. One input, one answer.
    let exe = script("sketch-exits-at-launch-twice", "exit 3");
    let mut codes = BTreeSet::new();
    for wait in [0, 300] {
        let mut solver = SketchWorker::start_worker(&exe).unwrap();
        let _stopper = solver.stopper().unwrap();
        std::thread::sleep(Duration::from_millis(wait));
        codes.insert(solver.solve(empty_system()).unwrap_err().code().as_str());
    }
    assert_eq!(
        codes.into_iter().collect::<Vec<_>>(),
        ["worker_unavailable"],
        "one binary, one code"
    );
}

#[test]
fn a_sketch_worker_whose_first_frame_comes_after_it_exited_had_started() {
    // Found by mutants: the solve's wait for what an ended worker sent before it ended could
    // be dropped (its deadline put in the past) and every test passed. Here the worker exits
    // at once while a process it started sends its first frame later: it had started, so
    // the solve waits for that frame, as a call to it would (`Worker::crashed`), and calls
    // the end a crash.
    //
    // The timing, against CI's stalls: the child waits for `go`, which the test makes just
    // before it calls the solve, then sends its frame 0.2 s later. So the frame comes about
    // 0.2 s into the solve's wait, which lasts up to EXIT_GRACE, 2 s; the mutant's wait
    // ends at once, before it. A stall before `go` changes nothing (the child waits for
    // it). Only a stall in the microseconds between `go` and the wait's start could let the
    // frame be read first (the mutant then passes, a missed catch); only one of over 1.8 s
    // in the child within the wait would fail the right code, as the product's own 2 s
    // bound would.
    let temp = tempfile::Builder::new()
        .prefix("silico-sketch-first-frame-")
        .tempdir()
        .unwrap();
    let go = temp.path().join("go");
    let exe = script(
        "sketch-first-frame-late",
        &format!(
            "( while [ ! -e '{}' ]; do sleep 0.005; done; sleep 0.2; {STARTED} ) &\nexit 0",
            go.display()
        ),
    );
    let mut solver = SketchWorker::start_worker(&exe).unwrap();
    let _stopper = solver.stopper().unwrap();
    // Until the worker itself has exited (a zombie, or gone); its child waits for `go`.
    let pid = solver.pid().to_string();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let ps = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&ps.stdout).trim().to_owned();
        if stat.is_empty() || stat.starts_with('Z') {
            break;
        }
        assert!(Instant::now() < deadline, "the worker didn't exit");
        std::thread::sleep(Duration::from_millis(5));
    }
    std::fs::write(&go, "").unwrap();
    let err = solver.solve(empty_system()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
}

// --- Ended before its first frame --------------------------------------------------------

/// Ways a worker ends before its first frame: how it's told, its script, and the signal
/// sent it from outside, if any.
const UNSTARTED: [(&str, &str, Option<&str>); 7] = [
    ("SIGTERM from outside", "exec sleep 600", Some("TERM")),
    ("SIGINT from outside", "exec sleep 600", Some("INT")),
    ("SIGHUP from outside", "exec sleep 600", Some("HUP")),
    ("SIGKILL from outside", "exec sleep 600", Some("KILL")),
    (
        "its socket closed at launch",
        "exec 3>&-\nexec sleep 600",
        None,
    ),
    ("an exit at launch", "exit 3", None),
    ("a fault at launch", "kill -SEGV $$", None),
];

/// Each way's error, after checking it's worker_unavailable.
fn unstarted_errors(tag: &str) -> Vec<(&'static str, Error)> {
    UNSTARTED
        .iter()
        .map(|&(what, body, signal)| {
            let exe = script(&format!("{tag}-{}", what.replace(' ', "-")), body);
            let mut w = Worker::spawn(&exe).unwrap();
            if let Some(signal) = signal {
                kill(w.pid(), signal);
            }
            let err = echo(&mut w, WAIT).unwrap_err();
            assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{what}: {err}");
            (what, err)
        })
        .collect()
}

#[test]
fn ended_before_the_first_frame_is_unavailable_and_its_advice_says_what_ended_it() {
    // Found by break-it: worker_unavailable's doc said no retry helps, while a worker
    // stopped from outside as it started is advised to try again (crash_advice, keyed on
    // how it ended). The decision keeps that advice (a one-off kill at launch is worth a
    // retry, Fable's residual), and the doc now says so. Pinned here: stopped from outside,
    // or gone quiet, try again; an exit or a fault of its own, check the install.
    let again = "try the request again";
    let install = "check Silico's install";
    let wrong: Vec<String> = unstarted_errors("advice")
        .into_iter()
        .filter(|(what, err)| {
            let want = match *what {
                "an exit at launch" | "a fault at launch" => install,
                _ => again,
            };
            !err.suggestion().unwrap_or_default().starts_with(want)
        })
        .map(|(what, err)| format!("{what}: {err}"))
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn an_unavailable_worker_isnt_said_to_have_crashed_during_the_request() {
    // Found by break-it: every worker_unavailable from a worker that ended unstarted said
    // "the kernel worker crashed during Echo" (crashed() kept its message whatever the
    // code): the message named the code it was moved away from, and blamed the request.
    let wrong: Vec<String> = unstarted_errors("message")
        .into_iter()
        .filter(|(_, err)| err.message().contains("crashed during"))
        .map(|(what, err)| format!("{what}: {err}"))
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// --- Said it started, then ended ---------------------------------------------------------

#[test]
fn the_start_code_follows_the_first_frame_every_time() {
    // The code is the first frame's to decide, read before the report is built: a worker
    // that sent it and ended at once is a crash every time, one that didn't (or sent only
    // part of it) is unavailable every time, however its end races the call.
    let ends = [
        ("exit", "exit 3"),
        ("fault", "kill -SEGV $$"),
        ("sigkill", "kill -KILL $$"),
        ("closes and lingers", "exec 3>&-\nexec sleep 600"),
    ];
    for (what, end) in ends {
        for (said, prefix, want) in [
            ("started", STARTED, ErrorCode::WorkerCrashed),
            ("never", ":", ErrorCode::WorkerUnavailable),
            (
                "half a frame",
                "printf '\\0\\0' >&3",
                ErrorCode::WorkerUnavailable,
            ),
        ] {
            let exe = script(
                &format!("ends-{}-{}", what.replace(' ', "-"), said.replace(' ', "-")),
                &format!("{prefix}\n{end}"),
            );
            // A lingering worker costs its grace (up to 2 s) a run.
            let runs = if what == "closes and lingers" { 3 } else { 12 };
            for run in 0..runs {
                let mut w = Worker::spawn(&exe).unwrap();
                let err = echo(&mut w, WAIT).unwrap_err();
                assert_eq!(err.code(), want, "{what}, {said}, run {run}: {err}");
                // Stopped: the next call fails at once, never hangs or answers.
                let next = echo(&mut w, WAIT).unwrap_err();
                assert!(w.is_stopped(), "{what}, {said}: {next}");
            }
        }
    }
}

#[test]
fn a_first_frame_that_isnt_a_frame_of_this_build_is_a_structured_error() {
    // Another protocol's worker (a first frame that isn't empty and doesn't decode, or a
    // length past the limit) said something, so it ran: protocol_error, and stopped.
    for (what, body) in [
        (
            "garbage",
            "printf '\\005\\0\\0\\0hello' >&3\nexec sleep 600",
        ),
        (
            "too long",
            "printf '\\377\\377\\377\\377' >&3\nexec sleep 600",
        ),
    ] {
        let exe = script(&format!("first-frame-{}", what.replace(' ', "-")), body);
        let mut w = Worker::spawn(&exe).unwrap();
        let lateness = crate::lateness::Lateness::start();
        let started = Instant::now();
        let err = echo(&mut w, WAIT).unwrap_err();
        assert!(
            matches!(
                err.code(),
                ErrorCode::ProtocolError | ErrorCode::WorkerUnavailable
            ),
            "{what}: {err}"
        );
        let bound = crate::lateness::allowance(Duration::from_secs(5), lateness.most());
        assert!(started.elapsed() < bound, "{what}: {err}");
        assert!(w.is_stopped(), "{what}");
    }
}

// --- Executables that can't be started ---------------------------------------------------

#[test]
fn every_executable_that_cant_be_started_is_unavailable_naming_it() {
    let dir = scratch("cant-start");
    let empty = dir.join("empty");
    std::fs::write(&empty, "").unwrap();
    chmod(&empty, 0o755);
    let crlf = dir.join("crlf");
    std::fs::write(&crlf, "#!/bin/sh\r\nexit 0\r\n").unwrap();
    chmod(&crlf, 0o755);
    let no_interpreter = dir.join("no-interpreter");
    std::fs::write(&no_interpreter, "#!/nonexistent/interpreter\n").unwrap();
    chmod(&no_interpreter, 0o755);
    let not_executable = dir.join("not-executable");
    std::fs::write(&not_executable, "#!/bin/sh\nexit 0\n").unwrap();
    chmod(&not_executable, 0o644);
    let directory = dir.join("a-directory");
    std::fs::create_dir(&directory).unwrap();
    let fifo = dir.join("fifo");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    chmod(&fifo, 0o755);
    let dangling = dir.join("dangling");
    std::os::unix::fs::symlink(dir.join("nothing"), &dangling).unwrap();
    let loop_link = dir.join("loop");
    std::os::unix::fs::symlink(&loop_link, &loop_link).unwrap();
    let garbage = dir.join("garbage");
    std::fs::write(&garbage, [0x7f, b'E', b'L', b'F', 0, 0, 0, 0]).unwrap();
    chmod(&garbage, 0o755);
    for (what, path) in [
        ("an empty file", empty),
        ("a script with CRLF line ends", crlf),
        ("a missing interpreter", no_interpreter),
        ("mode 0644", not_executable),
        ("a directory", directory),
        ("a FIFO", fifo),
        ("a dangling symlink", dangling),
        ("a symlink loop", loop_link),
        ("a truncated ELF header", garbage),
        ("/dev/null", PathBuf::from("/dev/null")),
    ] {
        let lateness = crate::lateness::Lateness::start();
        let started = Instant::now();
        let result = Worker::spawn(&path).and_then(|mut w| echo(&mut w, WAIT));
        let err = result.unwrap_err();
        assert!(
            started.elapsed()
                < crate::lateness::allowance(Duration::from_secs(10), lateness.most()),
            "{what}: took {:?}",
            started.elapsed()
        );
        assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{what}: {err}");
        assert!(err.suggestion().is_some(), "{what}: {err}");
        assert!(
            err.message().contains(&path.display().to_string()) || err.cause().is_some(),
            "{what}: {err}"
        );
    }
}

#[test]
fn a_pool_of_an_executable_that_isnt_a_file_is_unavailable_at_once() {
    // Found once, as the pool opens: hashed only if it's a regular file, so a FIFO or a
    // device never blocks or reads without end.
    let dir = scratch("pool-not-a-file");
    let fifo = dir.join("fifo");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let zero = dir.join("zero");
    std::os::unix::fs::symlink("/dev/zero", &zero).unwrap();
    let directory = dir.join("a-directory");
    std::fs::create_dir(&directory).unwrap();
    for (what, path) in [
        ("a FIFO", fifo),
        ("a link to /dev/zero", zero),
        ("a directory", directory),
        ("a missing file", dir.join("missing")),
    ] {
        let lateness = crate::lateness::Lateness::start();
        let started = Instant::now();
        let err = KernelPool::start_on_demand(&path, 1).err().unwrap();
        let bound = crate::lateness::allowance(Duration::from_secs(5), lateness.most());
        assert!(started.elapsed() < bound, "{what}");
        assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{what}: {err}");
        assert!(err.suggestion().is_some(), "{what}: {err}");
    }
}

// --- Recovery: a fixed install works again ------------------------------------------------

fn pool_echo(pool: &KernelPool) -> Result<Blob, Error> {
    pool.submit(Generation(1), KernelRequest::echo(Blob::inline(vec![7, 7])))
        .wait_timeout(Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("no outcome"))
        .done()
        .expect("superseded")
}

/// Kills the pool's workers, and gives them time to be gone.
fn kill_workers(pool: &KernelPool) {
    for pid in pool.pids() {
        kill(pid, "KILL");
    }
    std::thread::sleep(Duration::from_millis(300));
}

#[test]
fn a_worker_made_unrunnable_under_a_pool_is_unavailable_and_runs_again_once_fixed() {
    // A copy, not a link: a link's mode is the shared file's, every test's worker.
    let dir = scratch("unrunnable-then-fixed");
    let exe = dir.join("silico-kernel-worker");
    std::fs::copy(WORKER, &exe).unwrap();
    let pool = KernelPool::start_on_demand(&exe, 1).unwrap();
    pool_echo(&pool).unwrap();
    // Its executable made unrunnable, and its worker gone: the next request's start fails.
    chmod(&exe, 0o644);
    kill_workers(&pool);
    let err = pool_echo(&pool).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
    // Fixed: the next request runs, at once (a request starts a worker whatever the wait).
    chmod(&exe, 0o755);
    let lateness = crate::lateness::Lateness::start();
    let started = Instant::now();
    let ok = pool_echo(&pool);
    assert!(ok.is_ok(), "after the fix: {:?}", ok.err());
    assert!(
        started.elapsed() < crate::lateness::allowance(Duration::from_secs(20), lateness.most())
    );
}

#[test]
fn another_build_under_a_pool_is_unavailable_and_the_first_put_back_runs_again() {
    let dir = scratch("replaced-then-restored");
    let exe = dir.join("silico-kernel-worker");
    let kept = dir.join("kept");
    // Copies, not links: the scratch may be on another device than the build (CI keeps it in
    // memory), and a copy has the same bytes, so it's the same build.
    std::fs::copy(WORKER, &exe).unwrap();
    std::fs::copy(WORKER, &kept).unwrap();
    let pool = KernelPool::start_on_demand(&exe, 1).unwrap();
    pool_echo(&pool).unwrap();
    // Another build at the path, and the worker gone.
    std::fs::remove_file(&exe).unwrap();
    std::fs::write(&exe, format!("#!/bin/sh\n{STARTED}\nexec sleep 600\n")).unwrap();
    chmod(&exe, 0o755);
    kill_workers(&pool);
    let err = pool_echo(&pool).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
    assert!(err.message().contains("changed since it started"), "{err}");
    // The first build put back (another inode, the same bytes): it's the pool's build.
    std::fs::rename(&kept, &exe).unwrap();
    let ok = pool_echo(&pool);
    assert!(ok.is_ok(), "the build put back: {:?}", ok.err());
}
