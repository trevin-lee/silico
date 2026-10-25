//! M0.7: the core's worker client against the real workers. A box built and measured
//! through the worker comes back right; a worker that crashes, hangs, or replies with
//! garbage becomes a structured error and is reaped, and the test process carries on.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/imports.rs"]
mod imports;
#[path = "common/numbers.rs"]
mod numbers;
#[path = "common/scripts.rs"]
mod scripts;

use numbers::relative_error;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use scripts::script;
use silico_core::worker::Worker;
use silico_core::{Error, ErrorCode};
use silico_protocol::{
    Blob, DebugOp, Generation, Misbehavior, Op, OutOfTurn, Primitive, Reply, ShapeData,
    ShapeSummary,
};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");
const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-test-worker");
const TIMEOUT: Duration = Duration::from_secs(30);

fn worker() -> Worker {
    Worker::spawn(Path::new(WORKER)).unwrap()
}

fn test_worker() -> Worker {
    Worker::spawn(Path::new(TEST_WORKER)).unwrap()
}

/// Whether a process with this ID exists (zombies included).
fn process_exists(pid: u32) -> bool {
    Command::new("ps")
        .args(["-p", &pid.to_string()])
        .output()
        .unwrap()
        .status
        .success()
}

fn make_box(w: &mut Worker, dx: f64, dy: f64, dz: f64) -> Result<ShapeData, Error> {
    match w.call(
        Generation(1),
        Op::MakePrimitive(Primitive::Box { dx, dy, dz }),
        TIMEOUT,
    )? {
        Reply::Shape(shape) => Ok(shape),
        other => panic!("MakeBox replied {other:?}"),
    }
}

#[test]
fn box_volume_through_worker() {
    let mut w = worker();
    let brep = make_box(&mut w, 10.0, 20.0, 30.0).unwrap();
    match w
        .call(Generation(1), Op::Summarize { shape: brep }, TIMEOUT)
        .unwrap()
    {
        Reply::Summary(ShapeSummary { volume, center, .. }) => {
            // Analytic: 10 × 20 × 30, centered at (5, 10, 15).
            assert!(relative_error(volume, 6000.0) <= 1e-9, "volume {volume}");
            for (got, want) in center.into_iter().zip([5.0, 10.0, 15.0]) {
                assert!(relative_error(got, want) <= 1e-9, "center {center:?}");
            }
        }
        other => panic!("Summarize replied {other:?}"),
    }
}

#[test]
fn shapes_are_stateless_across_workers() {
    // A shape built by one worker is measured by another: workers keep nothing.
    let brep = make_box(&mut worker(), 1.0, 2.0, 3.0).unwrap();
    let mut other = worker();
    match other
        .call(Generation(1), Op::Summarize { shape: brep }, TIMEOUT)
        .unwrap()
    {
        Reply::Summary(ShapeSummary { volume, .. }) => assert!(relative_error(volume, 6.0) <= 1e-9),
        other => panic!("{other:?}"),
    }
}

#[test]
fn kernel_and_input_errors_are_structured_and_the_worker_lives_on() {
    let mut w = worker();
    let err = make_box(&mut w, -1.0, 1.0, 1.0).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert!(err.message().contains("box dx"), "{err}");

    let err = make_box(&mut w, 1.0, 0.0, 1.0).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert!(err.message().contains("box dy"), "{err}");

    // OCCT's own failure: bytes with a good checksum that aren't a B-rep get past the
    // checksum to OCCT, which can't read them. The cause is OCCT's, and there's always a
    // suggestion.
    let garbage = b"not a shape, but checksummed".to_vec();
    let mut brep = garbage.clone();
    brep.extend(crc32fast::hash(&garbage).to_le_bytes());
    let err = w
        .call(
            Generation(1),
            Op::Summarize {
                shape: ShapeData::unchecked(Blob::inline(brep)),
            },
            TIMEOUT,
        )
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
    assert_eq!(err.message(), "Summarize failed");
    assert_eq!(
        err.cause(),
        Some("Storage_StreamTypeMismatchError"),
        "{err}"
    );
    assert_eq!(
        err.suggestion(),
        Some("change the operation's values: the kernel can't build it from these"),
        "{err}"
    );

    let err = w
        .call(
            Generation(1),
            Op::Summarize {
                shape: ShapeData::unchecked(Blob::inline(b"not a shape".to_vec())),
            },
            TIMEOUT,
        )
        .unwrap_err();
    // Refused by the B-rep's checksum before OCCT reads it.
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert!(err.message().contains("corrupted"), "{err}");

    assert!(!w.is_stopped());
    assert!(make_box(&mut w, 1.0, 1.0, 1.0).is_ok());
}

#[test]
fn a_sketch_solve_or_drag_is_refused_as_another_workers() {
    let mut w = worker();
    let system = silico_protocol::SketchSystem {
        values: vec![],
        unknowns: vec![],
        geometry: vec![],
        constraints: vec![],
        algorithms: vec![silico_protocol::Algorithm::DogLeg],
    };
    for (op, name) in [
        (Op::Solve(system.clone()), "Solve"),
        (
            Op::DragBegin(silico_protocol::DragBegin {
                system,
                point: silico_protocol::PointOf::Point(silico_protocol::Geometry(0)),
            }),
            "DragBegin",
        ),
        (Op::DragMove { to: [1.0, 2.0] }, "DragMove"),
        (Op::DragEnd, "DragEnd"),
    ] {
        let err = w.call(Generation(1), op, TIMEOUT).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Internal, "{err}");
        assert_eq!(
            err.message(),
            format!("the kernel worker couldn't do {name}")
        );
        assert_eq!(
            err.cause(),
            Some(format!("{name} is answered by another kind of worker, not this one").as_str())
        );
    }
    assert!(make_box(&mut w, 1.0, 1.0, 1.0).is_ok());
}

#[test]
fn production_worker_refuses_debug_ops() {
    let mut w = worker();
    for op in [
        Op::Debug(DebugOp::Sleep { millis: 1 }),
        Op::Debug(DebugOp::Abort),
        Op::Debug(DebugOp::Segfault),
        Op::Debug(DebugOp::Garbage),
        Op::Debug(DebugOp::EnableFpTraps),
        Op::Debug(DebugOp::ReplyOutOfTurn(OutOfTurn::WrongId)),
        Op::Debug(DebugOp::Misbehave(Misbehavior::WrongReply)),
    ] {
        let name = op.name();
        let err = w.call(Generation(1), op, TIMEOUT).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Internal, "{name}: {err}");
        assert!(
            err.cause().unwrap().contains("test operation"),
            "{name}: {err}"
        );
    }
    // Still alive and answering.
    assert!(matches!(
        w.call(Generation(1), Op::Echo { data: Blob::inline(vec![1]) }, TIMEOUT).unwrap(),
        Reply::Echo { data } if data.as_bytes() == [1]
    ));
}

fn assert_crashed(op: Op, signal: &str) {
    let mut w = test_worker();
    let pid = w.pid();
    assert!(process_exists(pid), "worker {pid} isn't running");
    let err = w.call(Generation(1), op, TIMEOUT).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    assert!(err.cause().unwrap().contains(signal), "{err}");
    assert!(w.is_stopped());
    assert!(!process_exists(pid), "worker {pid} wasn't reaped");
    // A stopped worker refuses further calls at once, saying to start a new one.
    let err = w
        .call(
            Generation(1),
            Op::Echo {
                data: Blob::inline(vec![]),
            },
            TIMEOUT,
        )
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    assert_eq!(err.suggestion(), Some("start a new kernel worker"));
}

#[test]
fn segfault_becomes_error() {
    assert_crashed(Op::Debug(DebugOp::Segfault), "SIGSEGV");
}

#[test]
fn abort_becomes_error() {
    assert_crashed(Op::Debug(DebugOp::Abort), "SIGABRT");
}

#[test]
fn hang_times_out() {
    let mut w = test_worker();
    let pid = w.pid();
    assert!(process_exists(pid), "worker {pid} isn't running");
    let start = Instant::now();
    let err = w
        .call(
            Generation(1),
            Op::Debug(DebugOp::Sleep { millis: 60_000 }),
            Duration::from_secs(1),
        )
        .unwrap_err();
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(err.code(), ErrorCode::WorkerTimeout, "{err}");
    assert!(!process_exists(pid), "worker {pid} still exists");
}

#[test]
fn garbage_reply_is_error() {
    let mut w = test_worker();
    let pid = w.pid();
    assert!(process_exists(pid), "worker {pid} isn't running");
    let err = w
        .call(Generation(1), Op::Debug(DebugOp::Garbage), TIMEOUT)
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
    assert!(!process_exists(pid), "worker {pid} still exists");
}

#[test]
fn a_crash_report_carries_the_workers_last_output() {
    // The test worker logs 25 lines before aborting; the report keeps the last 20.
    let mut w = test_worker();
    let err = w
        .call(Generation(1), Op::Debug(DebugOp::Abort), TIMEOUT)
        .unwrap_err();
    let cause = err.cause().unwrap();
    for line in 6..=25 {
        assert!(
            cause.contains(&format!("line {line}\n")) || cause.ends_with(&format!("line {line}")),
            "line {line} missing: {cause}"
        );
    }
    assert!(
        !cause.contains("line 5\n"),
        "more than 20 lines kept: {cause}"
    );
}

#[test]
fn a_reply_out_of_turn_is_a_protocol_error() {
    // The worker first sends a reply with the previous request's id, or with this request's
    // id but another generation; either way the client stops it.
    for (out_of_turn, cause) in [
        (
            OutOfTurn::WrongId,
            "reply for request 0 (generation 1), expected 1 (generation 1)",
        ),
        (
            OutOfTurn::WrongGeneration,
            "reply for request 1 (generation 2), expected 1 (generation 1)",
        ),
    ] {
        let mut w = test_worker();
        let pid = w.pid();
        assert!(process_exists(pid), "worker {pid} isn't running");
        assert!(echo(&mut w, vec![], TIMEOUT).is_ok());
        let err = w
            .call(
                Generation(1),
                Op::Debug(DebugOp::ReplyOutOfTurn(out_of_turn)),
                TIMEOUT,
            )
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
        assert_eq!(err.cause(), Some(cause), "{err}");
        assert!(!process_exists(pid), "worker {pid} still exists");
    }
}

#[test]
fn a_worker_given_something_other_than_a_socket_refuses_to_run() {
    use command_fds::{CommandFdExt, FdMapping};
    let file = std::fs::File::open(WORKER).unwrap();
    let mut cmd = Command::new(WORKER);
    cmd.env("SILICO_WORKER_FD", "3");
    cmd.fd_mappings(vec![FdMapping {
        parent_fd: file.into(),
        child_fd: 3,
    }])
    .unwrap();
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("isn't a socket"),
        "{out:?}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn fp_traps_switched_on_by_a_request_fail_it() {
    let mut w = test_worker();
    let err = w
        .call(Generation(1), Op::Debug(DebugOp::EnableFpTraps), TIMEOUT)
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::Internal, "{err}");
    assert!(
        err.cause().unwrap().contains("floating-point traps are on"),
        "{err}"
    );
    // The worker switched them back off and carries on.
    assert!(make_box(&mut w, 1.0, 1.0, 1.0).is_ok());
}

#[test]
#[cfg(target_os = "macos")]
fn occt_doesnt_switch_fp_traps_on_on_macos() {
    // OCCT's OSD::SetFloatingSignal(true) doesn't set the trap-enable bits on macOS (it
    // doesn't write FPCR, which some Apple silicon would keep: the sketch worker's tests
    // write it directly), so the request succeeds and traps stay off.
    let mut w = test_worker();
    assert!(matches!(
        w.call(Generation(1), Op::Debug(DebugOp::EnableFpTraps), TIMEOUT)
            .unwrap(),
        Reply::Done
    ));
}

#[test]
fn fp_trap_check_fails_when_traps_are_on() {
    // In-process: the check the worker runs at startup and after every request, which a
    // worker whose output is a result must run (ARCHITECTURE.md, Deterministic kernel, 7).
    assert!(silico_kernel::worker::PLATFORM.traps.check().is_some());
    assert!(silico_kernel::worker::check_fp_traps().is_ok());
    silico_kernel::test_ops::enable_fp_traps().unwrap();
    let on = silico_kernel::worker::check_fp_traps();
    silico_kernel::disable_fp_traps().unwrap();
    if cfg!(target_os = "linux") {
        assert!(on.unwrap_err().contains("floating-point traps are on"));
    } else {
        assert!(on.is_ok(), "OCCT doesn't switch traps on on macOS");
    }
    assert!(silico_kernel::worker::check_fp_traps().is_ok());
}

#[test]
fn a_worker_not_started_by_the_core_refuses_to_run() {
    for env in [None, Some("x"), Some("7")] {
        let mut cmd = Command::new(WORKER);
        cmd.env_remove("SILICO_WORKER_FD");
        if let Some(value) = env {
            cmd.env("SILICO_WORKER_FD", value);
        }
        let out = cmd.output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{env:?}: {out:?}");
    }
}

#[test]
fn no_zombies() {
    for i in 0..100 {
        let mut w = test_worker();
        let op = match i % 3 {
            0 => Op::Debug(DebugOp::Segfault),
            1 => Op::Debug(DebugOp::Abort),
            _ => Op::Debug(DebugOp::Sleep { millis: 10_000 }),
        };
        assert!(
            w.call(Generation(1), op, Duration::from_millis(200))
                .is_err()
        );
    }
    // Healthy workers dropped while idle are reaped too.
    for _ in 0..10 {
        drop(worker());
    }
    let children = Command::new("pgrep")
        .args(["-P", &std::process::id().to_string()])
        .output()
        .unwrap();
    assert!(
        children.stdout.is_empty(),
        "child processes left: {}",
        String::from_utf8_lossy(&children.stdout)
    );
}

#[test]
fn workers_import_no_platform_math() {
    for exe in [WORKER, TEST_WORKER] {
        let out = Command::new("bash")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../third_party/occt/scripts/check-math-imports.sh"
            ))
            .arg(exe)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{exe}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn test_binary_imports_no_platform_math() {
    imports::assert_this_binary_imports_no_platform_math();
}

/// A stand-in worker: a shell script, written to this test's temporary directory.
fn echo(w: &mut Worker, data: Vec<u8>, timeout: Duration) -> Result<Vec<u8>, Error> {
    match w.call(
        Generation(1),
        Op::Echo {
            data: Blob::inline(data),
        },
        timeout,
    )? {
        Reply::Echo { data } => Ok(data.as_bytes().to_vec()),
        other => panic!("Echo replied {other:?}"),
    }
}

#[test]
fn a_corrupted_brep_is_never_read_as_a_shape() {
    // OCCT reads many corrupted B-reps as some other shape (a wrong volume, with no error),
    // so every B-rep carries a checksum, checked before OCCT sees it.
    let brep = silico_kernel::to_brep(&silico_kernel::make_box(10.0, 20.0, 30.0).unwrap()).unwrap();
    assert!(silico_kernel::from_brep(&brep).is_ok());
    for at in 0..brep.len() {
        let mut flipped = brep.clone();
        flipped[at] ^= 0xff;
        let err = silico_kernel::from_brep(&flipped)
            .err()
            .unwrap_or_else(|| panic!("byte {at} flipped was read"));
        assert_eq!(
            err.kind(),
            silico_kernel::KernelErrorKind::InvalidInput,
            "byte {at}: {err}"
        );
    }
    for len in [0, 3, 4, brep.len() / 2, brep.len() - 1] {
        assert!(
            silico_kernel::from_brep(&brep[..len]).is_err(),
            "truncated to {len} was read"
        );
    }
}

#[test]
fn a_request_over_the_frame_limit_is_invalid_input_and_the_worker_lives_on() {
    let mut w = worker();
    let too_large = vec![0; silico_protocol::MAX_FRAME_LEN as usize + 1];
    // Refused before sending, not by waiting out the timeout, which a refusal that waited
    // can't come before: so the timeout is the bound, and long, since encoding the 512 MiB
    // request takes about 0.2 s alone and over a second on a loaded machine (under cargo
    // mutants), and more as the machine's memory runs short.
    let timeout = Duration::from_secs(90);
    let start = Instant::now();
    let err = echo(&mut w, too_large, timeout).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert!(!w.is_stopped());
    assert!(start.elapsed() < timeout, "{:?}", start.elapsed());
    assert_eq!(echo(&mut w, vec![1, 2], TIMEOUT).unwrap(), [1, 2]);
}

#[test]
fn large_blobs_cross_byte_for_byte() {
    // Bigger than the socket buffers, with a pattern that shows any reordering or loss.
    let data: Vec<u8> = (0..9 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    assert!(echo(&mut worker(), data.clone(), TIMEOUT).unwrap() == data);
}

#[test]
fn the_generation_goes_to_the_worker_and_back() {
    // The worker echoes the generation; the client checks it, so a wrong one is an error.
    let mut w = worker();
    for generation in [0, 42, u64::MAX] {
        assert!(
            w.call(
                Generation(generation),
                Op::Echo {
                    data: Blob::inline(vec![])
                },
                TIMEOUT
            )
            .is_ok()
        );
    }
}

#[test]
fn a_worker_that_cant_start_is_a_structured_error() {
    let err = Worker::spawn(Path::new("/nonexistent/silico-kernel-worker"))
        .err()
        .unwrap();
    assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
}

#[test]
fn a_worker_killed_while_idle_is_reported_on_the_next_call() {
    // Answered once first, so it had started: its death is a crash (worker_crashed). Killed
    // at once, it could be killed before its first frame, which is worker_unavailable (the
    // test below).
    let mut w = worker();
    echo(&mut w, vec![1], TIMEOUT).unwrap();
    let pid = w.pid();
    assert!(
        Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    let err = echo(&mut w, vec![], TIMEOUT).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    assert!(err.cause().unwrap().contains("SIGKILL"), "{err}");
}

#[test]
fn a_worker_killed_before_its_first_frame_is_unavailable_not_crashed() {
    // The M3 review's start-code decision: a process that ends before it says it started
    // was never a worker of this build, whatever ended it (here a SIGKILL from outside, as a
    // dyld abort or an exit at launch end one); worker_unavailable, with the crash's cause.
    // A stand-in that never sends its first frame makes the order certain.
    let exe = script("never-started", "exec sleep 600");
    let mut w = Worker::spawn(&exe).unwrap();
    let pid = w.pid();
    assert!(
        Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    let err = echo(&mut w, vec![], TIMEOUT).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
    assert!(err.cause().unwrap().contains("SIGKILL"), "{err}");
}

#[test]
fn output_that_isnt_utf8_doesnt_hide_the_last_words() {
    let exe = script(
        "non-utf8",
        "printf 'before\\n\\377\\n' >&2; printf 'LAST WORDS\\n' >&2; sleep 0.2; exit 7",
    );
    let err = echo(&mut Worker::spawn(&exe).unwrap(), vec![], TIMEOUT).unwrap_err();
    let cause = err.cause().unwrap();
    assert!(
        cause.contains("exited with code 7") && cause.contains("LAST WORDS"),
        "{err}"
    );
}

#[test]
fn a_crash_report_waits_for_the_last_line() {
    // The worker exits at once, but a child it leaves behind holds the log pipe open and
    // writes the last line 300 ms later. The report must wait for the pipe to close.
    let exe = script(
        "late-line",
        "(exec 3>&-; sleep 0.3; echo 'FINAL LINE' >&2) & exit 7",
    );
    let err = echo(&mut Worker::spawn(&exe).unwrap(), vec![], TIMEOUT).unwrap_err();
    let cause = err.cause().unwrap();
    assert!(cause.contains("exited with code 7"), "{err}");
    assert!(cause.ends_with("FINAL LINE"), "{err}");
}

/// How long the timeout tests wait for a call that should have given up before failing:
/// long enough that only a call its timeout didn't end reaches it, however loaded the
/// machine.
const HANG: Duration = Duration::from_secs(60);

/// Echoes 32 MiB through a worker started from `exe`, with a timeout of 5 s, on another
/// thread: it must time out, no sooner than its timeout, and within twice it (room for a
/// loaded machine, where a send whose wait ignored its deadline, or overran it by a
/// multiple, would take far longer). Kills the worker and fails the test if the call
/// hasn't ended after [`HANG`].
fn echo_32_mib_with_a_timeout(exe: std::path::PathBuf, what: &str) {
    let timeout = Duration::from_secs(5);
    let (started, pid) = std::sync::mpsc::channel();
    let (done, result) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut w = Worker::spawn(&exe).unwrap();
        let _ = started.send(w.pid());
        let start = Instant::now();
        let err = echo(&mut w, vec![0; 32 * 1024 * 1024], timeout).unwrap_err();
        let _ = done.send((err, start.elapsed()));
    });
    let pid = pid.recv_timeout(HANG).expect("the worker never started");
    let Ok((err, elapsed)) = result.recv_timeout(HANG) else {
        // The call is stuck with the worker: end it, so nothing outlives the test.
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status();
        panic!("{what}: the call was still blocked after {HANG:?}");
    };
    assert_eq!(err.code(), ErrorCode::WorkerTimeout, "{what}: {err}");
    assert!(
        elapsed >= timeout && elapsed < timeout * 2,
        "{what}: gave up after {elapsed:?}, with a timeout of {timeout:?}"
    );
}

#[test]
fn the_timeout_covers_sending() {
    // A worker that never reads: a request bigger than the socket buffers can't be sent,
    // and the call must still give up when its timeout runs out.
    echo_32_mib_with_a_timeout(script("never-reads", "exec sleep 600"), "never reads");
}

#[test]
fn the_timeout_covers_a_worker_that_reads_slowly() {
    // Each write gets through a little at a time, so a per-write timeout never fires; the
    // call's timeout must still cover the whole send. A timeout per write would let the
    // send run on as long as the worker reads, 16 KiB every half second: about 17 minutes
    // for the 32 MiB. The worker stops once its socket is closed.
    echo_32_mib_with_a_timeout(
        script(
            "slow-reader",
            "while :; do\n\
             n=$(dd bs=16384 count=1 <&3 2>/dev/null | wc -c)\n\
             [ \"$n\" -eq 0 ] && exit 0\n\
             sleep 0.5\n\
             done",
        ),
        "reads slowly",
    );
}

#[test]
fn a_timeout_of_weeks_is_waited_out_in_pieces() {
    // 32 MiB doesn't fit the socket's buffers, so the send waits for room; a wait longer
    // than poll takes (about 24.8 days on macOS) must not fail the call.
    let mut w = worker();
    let data = vec![7u8; 32 * 1024 * 1024];
    let echoed = echo(&mut w, data.clone(), Duration::from_secs(30 * 24 * 60 * 60)).unwrap();
    // Not assert_eq!: a failure would print megabytes.
    assert!(echoed == data);
}

#[test]
fn a_timeout_too_long_for_the_clock_means_no_deadline() {
    // More than the socket's buffers hold, so the send has to wait, with no deadline.
    let data: Vec<u8> = (0..9 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    // Not assert_eq!: a failure would print megabytes.
    assert!(echo(&mut worker(), data.clone(), Duration::MAX).unwrap() == data);
}

/// Set in a copy of this test binary that `the_variables_a_worker_needs_reach_it` runs.
const ENV_CHILD: &str = "SILICO_TEST_WORKER_ENV_DUMP";

#[test]
fn the_variables_a_worker_needs_reach_it() {
    // Runs this test binary again, with each allowed variable set to a value of its own,
    // and the copy (below) starts a worker that writes its environment to a file.
    let dump = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("worker-env-values-{}.txt", std::process::id()));
    let wanted = [
        ("SILICO_LOG", "silico=trace"),
        ("RUST_BACKTRACE", "full"),
        ("RUST_LIB_BACKTRACE", "0"),
        ("PATH", "/usr/bin:/bin:/silico-test-path"),
    ];
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a_worker_started_with_set_variables_dumps_them",
            "--nocapture",
        ])
        .env(ENV_CHILD, &dump)
        .envs(wanted)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&dump).unwrap();
    std::fs::remove_file(&dump).unwrap();
    for (name, value) in wanted {
        assert!(
            text.lines().any(|l| l == format!("{name}={value}")),
            "the worker didn't get {name}={value}:\n{text}"
        );
    }
}

#[test]
fn a_worker_started_with_set_variables_dumps_them() {
    // Only does anything in the copy that the test above runs.
    let Some(dump) = std::env::var_os(ENV_CHILD) else {
        return;
    };
    let exe = script(
        "dump-env-values",
        &format!("/usr/bin/env > '{}'; exit 0", Path::new(&dump).display()),
    );
    assert!(echo(&mut Worker::spawn(&exe).unwrap(), vec![], TIMEOUT).is_err());
}

#[test]
fn a_worker_gets_only_the_variables_it_needs() {
    // OCCT reads variables such as CSF_MeshAlgo, so the app's environment mustn't reach a
    // worker. The test process has cargo's variables; the worker must not.
    assert!(std::env::var_os("CARGO_MANIFEST_DIR").is_some());
    let dump = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("worker-env-{}.txt", std::process::id()));
    let exe = script(
        "dump-env",
        &format!("/usr/bin/env > '{}'; exit 0", dump.display()),
    );
    assert!(echo(&mut Worker::spawn(&exe).unwrap(), vec![], TIMEOUT).is_err());
    let text = std::fs::read_to_string(&dump).unwrap();
    std::fs::remove_file(&dump).unwrap();
    let names: Vec<&str> = text
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(n, _)| n)
        .collect();
    assert!(names.contains(&"SILICO_WORKER_FD"), "{names:?}");
    // What the core passes on, and what the shell running the script sets itself.
    let allowed = [
        "SILICO_WORKER_FD",
        "SILICO_LOG",
        "RUST_BACKTRACE",
        "RUST_LIB_BACKTRACE",
        "PATH",
        "PWD",
        "OLDPWD",
        "SHLVL",
        "_",
    ];
    let extra: Vec<&&str> = names.iter().filter(|n| !allowed.contains(n)).collect();
    assert!(extra.is_empty(), "the worker got {extra:?}");
}

/// Waits until the process has exited (it's a zombie, or gone).
fn wait_until_exited(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let out = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || stat.trim_start().starts_with('Z') {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "process {pid} didn't exit: {stat}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_request_to_a_worker_that_has_exited_is_a_crash() {
    // The worker is gone before the request is written, so the write itself fails. It
    // answered once first, so it had started: a crash (killed before its first frame, it
    // would be worker_unavailable).
    let mut w = worker();
    echo(&mut w, vec![1], TIMEOUT).unwrap();
    let pid = w.pid();
    assert!(
        Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    wait_until_exited(pid);
    let err = echo(&mut w, vec![0; 64], TIMEOUT).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    let cause = err.cause().unwrap();
    assert!(
        cause.contains("SIGKILL") && cause.contains("sending the request failed"),
        "{err}"
    );
}

#[test]
fn the_trap_check_rejects_any_trap_bit() {
    // The decision itself, on every platform (some Apple silicon can't switch traps on).
    use silico_kernel::worker::check_fp_trap_mask;
    assert!(check_fp_trap_mask(0).is_ok());
    for bit in [0x100, 0x200, 0x400, 0x800, 0x1000, 0x8000, 0x1, 0x20] {
        let err = check_fp_trap_mask(bit).unwrap_err();
        assert!(err.contains(&format!("{bit:#x}")), "{err}");
    }
}

/// Collects this process's `tracing` output.
#[derive(Clone, Default)]
struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn worker_logs_keep_their_level_and_carry_the_calls_trace() {
    let logs = Captured::default();
    let writer = logs.clone();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .init();
    let mut w = test_worker();
    let pid = w.pid();
    assert!(
        w.call(Generation(1), Op::Debug(DebugOp::Abort), TIMEOUT)
            .is_err()
    );
    // Other tests in this process log too (under `cargo test` they share the subscriber),
    // so only this worker's lines count: its process ID is on each of them.
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    // The core's span for the call names its trace.
    let trace = text
        .split("worker_call{trace=")
        .skip(1)
        .find_map(|rest| {
            let (trace, rest) = rest.split_once(' ')?;
            rest.starts_with(&format!("pid={pid} ")).then_some(trace)
        })
        .unwrap_or_else(|| panic!("no worker_call span for {pid} in:\n{text}"));
    // The worker's warning comes back at WARN, from the worker, with the same trace.
    let forwarded = text
        .lines()
        .find(|l| l.contains("aborting, as asked: line 25") && l.contains(&format!("pid={pid}")))
        .unwrap_or_else(|| panic!("worker warning not forwarded:\n{text}"));
    assert!(forwarded.contains("WARN"), "{forwarded}");
    assert!(forwarded.contains("silico::worker"), "{forwarded}");
    assert!(forwarded.contains(&format!("trace={trace}")), "{forwarded}");
}

#[test]
fn a_shape_carries_its_validity_and_workers_take_its_word() {
    // The worker that makes a shape checks it once and says; later requests carry that
    // answer, and the summary reports it rather than checking the whole shape again.
    let mut w = worker();
    let b = make_box(&mut w, 10.0, 20.0, 30.0).unwrap();
    assert_eq!(b.valid(), Some(true), "a box is valid");
    let summarize = |w: &mut Worker, shape: ShapeData| match w
        .call(Generation(1), Op::Summarize { shape }, TIMEOUT)
        .unwrap()
    {
        Reply::Summary(s) => s,
        other => panic!("Summarize replied {other:?}"),
    };
    assert!(summarize(&mut w, b.clone()).valid);
    let said_invalid = ShapeData::checked(b.brep().clone(), false);
    assert!(!summarize(&mut w, said_invalid).valid);
    // A shape no worker has checked (from a file, say) is checked: an open box isn't
    // valid, whatever else it is, and a box is.
    let open = silico_kernel::to_brep(&silico_kernel::test_ops::open_box().unwrap()).unwrap();
    assert!(!summarize(&mut w, ShapeData::unchecked(Blob::inline(open.clone()))).valid);
    // And a fillet on it is judged knowing that: Edge5, away from the missing face, is
    // accepted, its result still invalid, as the input was.
    match w
        .call(
            Generation(1),
            Op::Fillet {
                shape: ShapeData::unchecked(Blob::inline(open)),
                edges: vec![5],
                radius: 2.0,
            },
            TIMEOUT,
        )
        .unwrap()
    {
        Reply::Built(rounded) => assert_eq!(rounded.shape.valid(), Some(false)),
        other => panic!("Fillet replied {other:?}"),
    }
    assert!(summarize(&mut w, ShapeData::unchecked(b.brep().clone())).valid);
    // A fillet's reply says whether its result is valid.
    match w
        .call(
            Generation(1),
            Op::Fillet {
                shape: b,
                edges: vec![1],
                radius: 2.0,
            },
            TIMEOUT,
        )
        .unwrap()
    {
        Reply::Built(rounded) => assert_eq!(rounded.shape.valid(), Some(true)),
        other => panic!("Fillet replied {other:?}"),
    }
}

#[test]
fn a_fillet_reply_says_when_its_result_is_invalid() {
    // Told its input is invalid, the worker applies only FreeCAD's changed-faces check and
    // returns the broken second fillet (a "solid" larger than the box); its reply must say
    // it isn't valid, and so must its summary.
    let mut w = worker();
    let fillet = |w: &mut Worker, shape: ShapeData, edges: Vec<u32>, radius: f64| match w
        .call(
            Generation(1),
            Op::Fillet {
                shape,
                edges,
                radius,
            },
            TIMEOUT,
        )
        .unwrap()
    {
        Reply::Built(b) => b.shape,
        other => panic!("Fillet replied {other:?}"),
    };
    let b = make_box(&mut w, 10.0, 20.0, 30.0).unwrap();
    let once = fillet(&mut w, b, vec![1, 3, 5], 3.0);
    assert_eq!(once.valid(), Some(true));
    let said_invalid = ShapeData::checked(once.brep().clone(), false);
    let twice = fillet(&mut w, said_invalid, vec![5], 5.0);
    let fresh = silico_kernel::from_brep(twice.brep().as_bytes()).unwrap();
    assert!(!fresh.is_valid().unwrap(), "the result really is invalid");
    assert_eq!(
        twice.valid(),
        Some(false),
        "an invalid result was reported valid"
    );
    match w
        .call(Generation(1), Op::Summarize { shape: twice }, TIMEOUT)
        .unwrap()
    {
        Reply::Summary(s) => assert!(!s.valid, "summarized as valid"),
        other => panic!("{other:?}"),
    }
}

/// Set in a copy of this test binary that `an_empty_silico_log_leaves_workers_warning` runs.
const EMPTY_LOG_CHILD: &str = "SILICO_TEST_EMPTY_LOG_CHILD";

#[test]
fn an_empty_silico_log_leaves_workers_warning() {
    // An empty SILICO_LOG is the default, warn, in workers as in the app; parsed as a
    // filter it would be "nothing", and a crash report would lose the worker's last lines.
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a_worker_under_an_empty_silico_log_still_warns",
            "--nocapture",
        ])
        .env(EMPTY_LOG_CHILD, "1")
        .env("SILICO_LOG", "")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("1 passed"), "{stdout}");
}

#[test]
fn a_worker_under_an_empty_silico_log_still_warns() {
    // Only does anything in the copy that the test above runs.
    if std::env::var_os(EMPTY_LOG_CHILD).is_none() {
        return;
    }
    let err = test_worker()
        .call(Generation(1), Op::Debug(DebugOp::Abort), TIMEOUT)
        .unwrap_err();
    let cause = err.cause().unwrap();
    assert!(cause.contains("aborting, as asked: line 25"), "{cause}");
}

#[test]
fn every_failure_says_what_to_try() {
    // A crash, a reply out of turn or unreadable, and each refusal carry a suggestion; a
    // fillet radius too small for OCCT is refused before it's sent, not answered with
    // advice to make it smaller still.
    use silico_core::kernel::{KernelRequest, KernelWorker, Primitive};
    use silico_protocol::Misbehavior;
    let test = |op| {
        let mut w = KernelWorker::start_worker(Path::new(TEST_WORKER)).unwrap();
        w.run(KernelRequest::debug(op)).unwrap_err()
    };
    for (what, err) in [
        ("a segfault", test(DebugOp::Segfault)),
        ("an abort", test(DebugOp::Abort)),
        ("garbage", test(DebugOp::Garbage)),
        (
            "out of turn",
            test(DebugOp::ReplyOutOfTurn(silico_protocol::OutOfTurn::WrongId)),
        ),
        ("a wrong reply", {
            let mut w = KernelWorker::start_worker(Path::new(TEST_WORKER)).unwrap();
            w.run(KernelRequest::debug(DebugOp::Misbehave(
                Misbehavior::WrongReply,
            )))
            .unwrap();
            w.make(Primitive::Sphere { radius: 1.0 }).unwrap_err()
        }),
    ] {
        assert!(err.suggestion().is_some(), "{what}: {err}");
    }
    // Each kind of failure gets its own advice: a worker that misbehaved is Silico's bug, not
    // the request's; a test operation a production worker refuses, Silico's own; a refusal
    // that says no more, the value it names.
    let bug = "this is a bug in Silico's kernel worker, not in the request";
    for (what, err, says) in [
        ("garbage", test(DebugOp::Garbage), bug),
        (
            "out of turn",
            test(DebugOp::ReplyOutOfTurn(silico_protocol::OutOfTurn::WrongId)),
            bug,
        ),
        (
            "a test operation on a production worker",
            {
                let mut w = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
                w.run(KernelRequest::debug(DebugOp::Abort)).unwrap_err()
            },
            "this is a bug in Silico, not in the request",
        ),
        (
            "a refusal with no advice of its own",
            {
                let mut w = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
                let block = w.make(Primitive::Sphere { radius: 1.0 }).unwrap();
                w.tessellate(&block, 0.0).unwrap_err()
            },
            "change the value the message names",
        ),
    ] {
        assert!(err.suggestion().unwrap().starts_with(says), "{what}: {err}");
    }
    // A crash's advice follows how the worker ended: its own crash is the values'; a kill
    // from outside is worth a retry; an exit of its own is its install. SIGSEGV and SIGBUS
    // sent from outside don't end a worker once Rust's runtime has started (its handler for
    // stack overflows takes them, and returns), so a segfault is the worker's own.
    let mid_request = |signal: &'static str| {
        let mut w = KernelWorker::start_worker(Path::new(TEST_WORKER)).unwrap();
        let pid = w.pid();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let _ = Command::new("kill")
                .args([format!("-{signal}"), pid.to_string()])
                .status();
        });
        w.run(KernelRequest::debug(DebugOp::Sleep { millis: 10_000 }))
            .unwrap_err()
    };
    // Sent once the worker has started (it has answered), then waited on until the worker
    // has ended (a zombie, or gone), so the request can't reach it.
    let while_idle = |signal: &str| {
        let mut w = KernelWorker::start_worker(Path::new(TEST_WORKER)).unwrap();
        w.run(KernelRequest::debug(DebugOp::Sleep { millis: 0 }))
            .unwrap();
        let pid = w.pid().to_string();
        let _ = Command::new("kill")
            .args([format!("-{signal}"), pid.clone()])
            .status();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            let ps = Command::new("ps")
                .args(["-o", "stat=", "-p", &pid])
                .output()
                .unwrap();
            let stat = String::from_utf8_lossy(&ps.stdout).trim().to_owned();
            if stat.is_empty() || stat.starts_with('Z') {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        w.make(Primitive::Sphere { radius: 1.0 }).unwrap_err()
    };
    for (what, err, says) in [
        (
            "a segfault",
            test(DebugOp::Segfault),
            "change the operation's values",
        ),
        (
            "an abort",
            test(DebugOp::Abort),
            "change the operation's values",
        ),
        (
            "a SIGILL",
            mid_request("ILL"),
            "change the operation's values",
        ),
        (
            "a SIGFPE",
            mid_request("FPE"),
            "change the operation's values",
        ),
        (
            "a SIGTRAP",
            mid_request("TRAP"),
            "change the operation's values",
        ),
        (
            "a SIGSYS",
            mid_request("SYS"),
            "change the operation's values",
        ),
        ("a Ctrl-C", mid_request("INT"), "try the request again"),
        ("a hangup", mid_request("HUP"), "try the request again"),
        ("a SIGUSR1", mid_request("USR1"), "try the request again"),
        (
            "a kill from outside",
            mid_request("KILL"),
            "try the request once more",
        ),
        (
            "a SIGTERM from outside",
            mid_request("TERM"),
            "try the request again",
        ),
        (
            "a worker that faulted while idle, before the request",
            while_idle("ILL"),
            "try the request again",
        ),
        (
            "a worker killed partway through reading a large request",
            {
                // Says it has started (an empty frame), takes 1 MiB of the 64 MiB request,
                // then is killed, as the system kills a worker that runs out of memory
                // reading it.
                let exe = script(
                    "killed-reading",
                    "printf '\\0\\0\\0\\0' >&3\nhead -c 1048576 <&3 >/dev/null\nkill -KILL $$",
                );
                let mut w = KernelWorker::start_worker(&exe).unwrap();
                let big = ShapeData::unchecked(Blob::inline(vec![b'x'; 64 * 1024 * 1024]));
                w.summarize(&big).unwrap_err()
            },
            "try the request once more",
        ),
        (
            "a worker killed while idle, before the request",
            while_idle("KILL"),
            "try the request again",
        ),
        (
            "a worker that crashed as it started",
            {
                // As dyld aborts a worker whose library is missing: before it says it has
                // started, whatever the request.
                let exe = script("aborts-at-start", "kill -ABRT $$");
                let mut w = KernelWorker::start_worker(&exe).unwrap();
                w.make(Primitive::Sphere { radius: 1.0 }).unwrap_err()
            },
            "check Silico's install",
        ),
        (
            "an exit of its own",
            {
                let exe = script("exits", "echo 'dyld: Library not loaded' >&2\nexit 3");
                let mut w = KernelWorker::start_worker(&exe).unwrap();
                w.make(Primitive::Sphere { radius: 1.0 }).unwrap_err()
            },
            "check Silico's install",
        ),
    ] {
        // A worker that ended before its first frame was never one of this build: it can't
        // be had. Every other ran, and crashed.
        let unstarted = ["a worker that crashed as it started", "an exit of its own"];
        let want = if unstarted.contains(&what) {
            ErrorCode::WorkerUnavailable
        } else {
            ErrorCode::WorkerCrashed
        };
        assert_eq!(err.code(), want, "{what}: {err}");
        assert!(err.suggestion().unwrap().starts_with(says), "{what}: {err}");
    }
    let mut w = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    let block = w
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    for err in [
        w.fillet(&block, &[13], 1.0).unwrap_err(),
        w.fillet(&block, &[1, 1], 1.0).unwrap_err(),
        w.fillet(&block, &[], 1.0).unwrap_err(),
        w.tessellate(&block, 0.0).unwrap_err(),
    ] {
        assert!(err.suggestion().is_some(), "{err}");
    }
    let sent = w.requests_sent();
    for radius in [f64::NAN, -1.0, 0.0, 1e-12, 1e-7, 1e7] {
        let err = w.fillet(&block, &[1], radius).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{radius}: {err}");
        assert_eq!(
            err.suggestion(),
            Some("give the fillet a valid radius"),
            "{err}"
        );
    }
    assert_eq!(
        w.requests_sent(),
        sent,
        "a refused radius reached the worker"
    );
}
