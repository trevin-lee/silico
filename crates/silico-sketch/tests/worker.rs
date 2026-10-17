//! M3.1: the core's worker client against the sketch worker, as M0.7 tests the kernel
//! worker: a sketch solved through the worker comes back as the bridge solves it in this
//! process; a worker that crashes, hangs, or replies with garbage becomes a structured
//! error and is reaped; and the worker refuses what isn't its to answer.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/systems.rs"]
mod systems;

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::worker::{Worker, WorkerKind};
use silico_protocol::{
    Algorithm, Blob, DebugOp, Generation, Op, OutOfTurn, Primitive, Reply, SketchSystem,
    SolveStatus, Solved,
};
use systems::rectangle_40_by_20;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");
const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-test-worker");
const TIMEOUT: Duration = Duration::from_secs(30);

fn worker() -> Worker {
    Worker::spawn_as(Path::new(WORKER), WorkerKind::Sketch).unwrap()
}

fn test_worker() -> Worker {
    Worker::spawn_as(Path::new(TEST_WORKER), WorkerKind::Sketch).unwrap()
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

fn solve(w: &mut Worker, system: SketchSystem) -> Solved {
    match w.call(Generation(1), Op::Solve(system), TIMEOUT).unwrap() {
        Reply::Solved(solved) => solved,
        other => panic!("Solve replied {other:?}"),
    }
}

fn echo(w: &mut Worker) -> bool {
    matches!(
        w.call(Generation(1), Op::Echo { data: Blob::inline(vec![7]) }, TIMEOUT),
        Ok(Reply::Echo { data }) if data.as_bytes() == [7]
    )
}

#[test]
fn a_sketch_solved_through_the_worker_is_solved_as_in_this_process() {
    let system = rectangle_40_by_20().b.system(Algorithm::DogLeg);
    let here = silico_sketch::solve(&system).unwrap();
    assert_eq!(here.status, SolveStatus::Success);
    let mut w = worker();
    let there = solve(&mut w, system.clone());
    assert_eq!(there, here);
    // Bit for bit, the worker's values against this process's.
    let bits = |s: &Solved| s.values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&there), bits(&here));
    // And again, from the same worker: it keeps nothing between requests.
    assert_eq!(solve(&mut w, system), here);
}

#[test]
fn a_system_that_fails_its_check_is_invalid_input_and_the_worker_lives_on() {
    let mut system = rectangle_40_by_20().b.system(Algorithm::DogLeg);
    system.values[3] = f64::NAN;
    let mut w = worker();
    let err = w
        .call(Generation(1), Op::Solve(system), TIMEOUT)
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert_eq!(err.parameter(), Some("values"), "{err}");
    assert!(err.message().contains("value 3"), "{err}");
    assert!(echo(&mut w));
}

#[test]
fn a_kernel_operation_is_refused_as_another_workers() {
    let mut w = worker();
    let err = w
        .call(
            Generation(1),
            Op::MakePrimitive(Primitive::Box {
                dx: 1.0,
                dy: 1.0,
                dz: 1.0,
            }),
            TIMEOUT,
        )
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::Internal, "{err}");
    assert!(
        err.cause().unwrap().contains("another kind of worker"),
        "{err}"
    );
    assert!(echo(&mut w));
}

#[test]
fn production_worker_refuses_debug_ops() {
    let mut w = worker();
    for op in [
        Op::Debug(DebugOp::Sleep { millis: 1 }),
        Op::Debug(DebugOp::Abort),
        Op::Debug(DebugOp::Segfault),
        Op::Debug(DebugOp::Garbage),
        Op::Debug(DebugOp::ReplyOutOfTurn(OutOfTurn::WrongId)),
    ] {
        let name = op.name();
        let err = w.call(Generation(1), op, TIMEOUT).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Internal, "{name}: {err}");
        assert!(
            err.cause().unwrap().contains("test operation"),
            "{name}: {err}"
        );
    }
    assert!(echo(&mut w));
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
    let start = Instant::now();
    let err = w
        .call(
            Generation(1),
            Op::Debug(DebugOp::Sleep { millis: 60_000 }),
            Duration::from_secs(1),
        )
        .unwrap_err();
    assert!(
        start.elapsed() < Duration::from_secs(10),
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
    let err = w
        .call(Generation(1), Op::Debug(DebugOp::Garbage), TIMEOUT)
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
    assert!(!process_exists(pid), "worker {pid} still exists");
}

#[test]
fn a_reply_out_of_turn_is_a_protocol_error() {
    for out_of_turn in [OutOfTurn::WrongId, OutOfTurn::WrongGeneration] {
        let mut w = test_worker();
        let pid = w.pid();
        assert!(echo(&mut w));
        let err = w
            .call(
                Generation(1),
                Op::Debug(DebugOp::ReplyOutOfTurn(out_of_turn)),
                TIMEOUT,
            )
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
        assert!(!process_exists(pid), "worker {pid} still exists");
    }
}

#[test]
fn a_worker_not_started_by_the_core_refuses_to_run() {
    let out = Command::new(WORKER).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("SILICO_WORKER_FD"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The worker and the test worker import no inexact platform math: PlaneGCS's comes from
/// the libm shim (ARCHITECTURE.md, Deterministic kernel).
#[test]
fn workers_import_no_platform_math() {
    for exe in [WORKER, TEST_WORKER] {
        let script = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../third_party/occt/scripts/check-math-imports.sh"
        );
        let out = Command::new("bash").arg(script).arg(exe).output().unwrap();
        assert!(
            out.status.success(),
            "{exe}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
