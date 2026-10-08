//! M0.7: the core's worker client against the real workers. A box built and measured
//! through the worker comes back right; a worker that crashes, hangs, or replies with
//! garbage becomes a structured error and is reaped, and the test process carries on.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/imports.rs"]
mod imports;

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use silico_core::worker::Worker;
use silico_core::{Error, ErrorCode};
use silico_protocol::{Blob, Op, Reply, ShapeSummary};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");
const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-test-worker");
const TIMEOUT: Duration = Duration::from_secs(30);

fn worker() -> Worker {
    Worker::spawn(Path::new(WORKER)).unwrap()
}

fn test_worker() -> Worker {
    Worker::spawn(Path::new(TEST_WORKER)).unwrap()
}

fn relative_error(actual: f64, expected: f64) -> f64 {
    ((actual - expected) / expected).abs()
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

fn make_box(w: &mut Worker, dx: f64, dy: f64, dz: f64) -> Result<Blob, Error> {
    match w.call(1, Op::MakeBox { dx, dy, dz }, TIMEOUT)? {
        Reply::Shape { brep } => Ok(brep),
        other => panic!("MakeBox replied {other:?}"),
    }
}

#[test]
fn box_volume_through_worker() {
    let mut w = worker();
    let brep = make_box(&mut w, 10.0, 20.0, 30.0).unwrap();
    match w.call(1, Op::Summarize { shape: brep }, TIMEOUT).unwrap() {
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
        .call(1, Op::Summarize { shape: brep }, TIMEOUT)
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
    assert!(err.message().contains("box size dx"), "{err}");

    let err = make_box(&mut w, 0.0, 1.0, 1.0).unwrap_err();
    assert_eq!(err.code(), ErrorCode::KernelFailed, "{err}");
    assert_eq!(err.message(), "MakeBox failed");
    assert!(
        err.cause().unwrap().contains("Standard_DomainError"),
        "{err}"
    );

    let err = w
        .call(
            1,
            Op::Summarize {
                shape: Blob::inline(b"not a shape".to_vec()),
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
fn production_worker_refuses_debug_ops() {
    let mut w = worker();
    for op in [
        Op::DebugSleep { millis: 1 },
        Op::DebugAbort,
        Op::DebugSegfault,
        Op::DebugGarbage,
        Op::DebugEnableFpTraps,
        Op::DebugReplyOutOfTurn { wrong_id: true },
    ] {
        let name = op.name();
        let err = w.call(1, op, TIMEOUT).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Internal, "{name}: {err}");
        assert!(
            err.cause().unwrap().contains("test operation"),
            "{name}: {err}"
        );
    }
    // Still alive and answering.
    assert!(matches!(
        w.call(1, Op::Echo { data: Blob::inline(vec![1]) }, TIMEOUT).unwrap(),
        Reply::Echo { data } if data.as_bytes() == [1]
    ));
}

fn assert_crashed(op: Op, signal: &str) {
    let mut w = test_worker();
    let pid = w.pid();
    assert!(process_exists(pid), "worker {pid} isn't running");
    let err = w.call(1, op, TIMEOUT).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    assert!(err.cause().unwrap().contains(signal), "{err}");
    assert!(w.is_stopped());
    assert!(!process_exists(pid), "worker {pid} wasn't reaped");
    // A stopped worker refuses further calls at once, saying to start a new one.
    let err = w
        .call(
            1,
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
    assert_crashed(Op::DebugSegfault, "SIGSEGV");
}

#[test]
fn abort_becomes_error() {
    assert_crashed(Op::DebugAbort, "SIGABRT");
}

#[test]
fn hang_times_out() {
    let mut w = test_worker();
    let pid = w.pid();
    assert!(process_exists(pid), "worker {pid} isn't running");
    let start = Instant::now();
    let err = w
        .call(1, Op::DebugSleep { millis: 60_000 }, Duration::from_secs(1))
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
    let err = w.call(1, Op::DebugGarbage, TIMEOUT).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
    assert!(!process_exists(pid), "worker {pid} still exists");
}

#[test]
fn a_crash_report_carries_the_workers_last_output() {
    // The test worker logs 25 lines before aborting; the report keeps the last 20.
    let mut w = test_worker();
    let err = w.call(1, Op::DebugAbort, TIMEOUT).unwrap_err();
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
    for (wrong_id, cause) in [
        (
            true,
            "reply for request 0 (generation 1), expected 1 (generation 1)",
        ),
        (
            false,
            "reply for request 1 (generation 2), expected 1 (generation 1)",
        ),
    ] {
        let mut w = test_worker();
        let pid = w.pid();
        assert!(process_exists(pid), "worker {pid} isn't running");
        assert!(echo(&mut w, vec![], TIMEOUT).is_ok());
        let err = w
            .call(1, Op::DebugReplyOutOfTurn { wrong_id }, TIMEOUT)
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
    let err = w.call(1, Op::DebugEnableFpTraps, TIMEOUT).unwrap_err();
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
fn fp_traps_cant_be_switched_on_on_apple_silicon() {
    // The hardware ignores the trap-enable bits, so the request succeeds and traps stay off.
    let mut w = test_worker();
    assert!(matches!(
        w.call(1, Op::DebugEnableFpTraps, TIMEOUT).unwrap(),
        Reply::Done
    ));
}

#[test]
fn fp_trap_check_fails_when_traps_are_on() {
    // In-process: the check the worker runs at startup and after every request.
    assert!(silico_kernel::worker::check_fp_traps().is_ok());
    silico_kernel::test_ops::enable_fp_traps().unwrap();
    let on = silico_kernel::worker::check_fp_traps();
    silico_kernel::disable_fp_traps().unwrap();
    if cfg!(target_os = "linux") {
        assert!(on.unwrap_err().contains("floating-point traps are on"));
    } else {
        assert!(on.is_ok(), "Apple silicon can't enable traps");
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
            0 => Op::DebugSegfault,
            1 => Op::DebugAbort,
            _ => Op::DebugSleep { millis: 10_000 },
        };
        assert!(w.call(1, op, Duration::from_millis(200)).is_err());
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
fn script(name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}.sh", std::process::id()));
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn echo(w: &mut Worker, data: Vec<u8>, timeout: Duration) -> Result<Vec<u8>, Error> {
    match w.call(
        1,
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
        assert!(err.is_invalid_input(), "byte {at}: {err}");
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
    let start = Instant::now();
    let err = echo(
        &mut w,
        vec![0; silico_protocol::MAX_FRAME_LEN as usize + 1],
        TIMEOUT,
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert!(!w.is_stopped());
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
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
                generation,
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
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
}

#[test]
fn a_worker_killed_while_idle_is_reported_on_the_next_call() {
    let mut w = worker();
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

#[test]
fn the_timeout_covers_sending() {
    // A worker that never reads: a request bigger than the socket buffers can't be sent,
    // and the call must still give up when its timeout runs out.
    let exe = script("never-reads", "exec sleep 600");
    let (done, result) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut w = Worker::spawn(&exe).unwrap();
        let start = Instant::now();
        let err = echo(&mut w, vec![0; 32 * 1024 * 1024], Duration::from_secs(1)).unwrap_err();
        let _ = done.send((err, start.elapsed()));
    });
    let (err, elapsed) = result
        .recv_timeout(Duration::from_secs(10))
        .expect("the call was still blocked after 10 s");
    assert_eq!(err.code(), ErrorCode::WorkerTimeout, "{err}");
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
}

#[test]
fn the_trap_check_rejects_any_trap_bit() {
    // The decision itself, on every platform (Apple silicon can't switch traps on).
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
    assert!(w.call(1, Op::DebugAbort, TIMEOUT).is_err());
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    // The core's span for the call names its trace.
    let trace = text
        .split("worker_call{trace=")
        .nth(1)
        .and_then(|rest| rest.split([' ', '}']).next())
        .unwrap_or_else(|| panic!("no worker_call span in:\n{text}"));
    // The worker's warning comes back at WARN, from the worker, with the same trace.
    let forwarded = text
        .lines()
        .find(|l| l.contains("aborting, as asked: line 25"))
        .unwrap_or_else(|| panic!("worker warning not forwarded:\n{text}"));
    assert!(forwarded.contains("WARN"), "{forwarded}");
    assert!(forwarded.contains("silico::worker"), "{forwarded}");
    assert!(forwarded.contains(&format!("trace={trace}")), "{forwarded}");
}

#[test]
fn mass_properties_that_arent_finite_are_refused() {
    use silico_kernel::worker::check_finite;
    assert!(check_finite(6000.0, [5.0, 10.0, 15.0]).is_ok());
    for (volume, center) in [
        (f64::NAN, [0.0; 3]),
        (f64::INFINITY, [0.0; 3]),
        (1.0, [f64::NAN, 0.0, 0.0]),
        (1.0, [0.0, f64::NEG_INFINITY, 0.0]),
        (1.0, [0.0, 0.0, f64::INFINITY]),
    ] {
        let failure = check_finite(volume, center).unwrap_err();
        assert_eq!(failure.kind, silico_protocol::FailureKind::KernelFailed);
        assert!(
            failure.message.contains("aren't finite"),
            "{}",
            failure.message
        );
    }
}
