//! M3.1: the core's worker client against the sketch worker, as M0.7 tests the kernel
//! worker: a sketch solved through the worker comes back as the bridge solves it in this
//! process; a worker that crashes, hangs, or replies with garbage becomes a structured
//! error and is reaped; and the worker refuses what isn't its to answer.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::systems;
use super::wait::WAIT;

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::worker::{Worker, WorkerKind};
use silico_protocol::{
    Algorithm, Blob, DebugOp, Generation, Op, OutOfTurn, Primitive, Reply, SketchSystem,
    SolveStatus, Solved,
};
use systems::{process_exists, rectangle_40_by_20};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");
const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-test-worker");

fn worker() -> Worker {
    Worker::spawn_as(Path::new(WORKER), WorkerKind::Sketch).unwrap()
}

fn test_worker() -> Worker {
    Worker::spawn_as(Path::new(TEST_WORKER), WorkerKind::Sketch).unwrap()
}

fn solve(w: &mut Worker, system: SketchSystem) -> Solved {
    match w.call(Generation(1), Op::Solve(system), WAIT).unwrap() {
        Reply::Solved(solved) => solved,
        other => panic!("Solve replied {other:?}"),
    }
}

fn echo(w: &mut Worker) -> bool {
    matches!(
        w.call(Generation(1), Op::Echo { data: Blob::inline(vec![7]) }, WAIT),
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
fn a_row_of_fixed_values_alone_is_a_dependency_of_its_own_through_the_worker() {
    // M5.0 open item 32's addendum 4: a group may be one tag. A line whose ends are fixed
    // values, (0, 0) to (10, 0), made horizontal (tag 1), beside a free point with nothing
    // on it. By hand: 2 unknowns (the free point's x and y); the horizontal's row reads no
    // unknown, so its gradient is zero: 1 row of rank 0, the row past the rank with no pivot
    // in it, a dependency of its one row, [[1]]. PlaneGCS drops it, and it holds (0 = 0) on
    // the fixed ends, so it's redundant; 2 left, both unknowns free.
    let build = |end_y: f64| {
        let mut b = systems::Builder::default();
        let start = silico_protocol::PointValues {
            x: b.fixed(0.0),
            y: b.fixed(0.0),
        };
        let end = silico_protocol::PointValues {
            x: b.fixed(10.0),
            y: b.fixed(end_y),
        };
        let line = b.add(silico_protocol::SketchGeometry::Line { start, end });
        let free = b.point_values(3.0, 4.0);
        b.add(silico_protocol::SketchGeometry::Point(free));
        b.constrain(|tag| silico_protocol::SolverConstraint::Horizontal { tag, line });
        b.system(Algorithm::DogLeg)
    };
    let mut w = worker();
    let held = solve(&mut w, build(0.0));
    assert_eq!(held.dependencies, [vec![silico_protocol::Tag(1)]]);
    assert_eq!(held.redundant, [silico_protocol::Tag(1)]);
    assert!(held.conflicting.is_empty(), "{held:?}");
    assert_eq!(held.dofs, 2);
    assert_eq!(
        held.free,
        [silico_protocol::Value(4), silico_protocol::Value(5)]
    );
    // The far end at y 1: the same zero row, the same dependency of one, but it doesn't hold
    // (1 ≠ 0), so PlaneGCS finds it in conflict; the conflicting group is listed too.
    let broken = solve(&mut w, build(1.0));
    assert_eq!(broken.dependencies, [vec![silico_protocol::Tag(1)]]);
    assert_eq!(broken.conflicting, [silico_protocol::Tag(1)]);
    assert!(broken.redundant.is_empty(), "{broken:?}");
    // No dependency at all: the rectangle's rows are independent.
    let rectangle = solve(&mut w, rectangle_40_by_20().b.system(Algorithm::DogLeg));
    assert!(rectangle.dependencies.is_empty(), "{rectangle:?}");
}

#[test]
fn an_arc_whose_rules_alone_depend_lists_no_dependency_and_still_solves() {
    // Fable's Addendum 5, point 4: a dependency among an arc's rules alone (tag 0) names no
    // constraint, so the worker drops the empty group patch 0002 keeps. An arc whose centre
    // (0, 0), start (5, 0), and end (0, 5) are fixed values, only its radius and angles
    // unknown, and no constraint on it. By hand, over (r, a1, a2) at r = 5, a1 = 0, a2 =
    // π/2: the start's x row is (−cos a1, r sin a1, 0) = (−1, 0, 0), its y row (−sin a1,
    // −r cos a1, 0) = (0, −5, 0), the end's x row (−cos a2, 0, r sin a2) = (0, 0, 5), and its
    // y row (−sin a2, 0, −r cos a2) = (−1, 0, 0) (to cos(π/2)'s 6e-17): 4 rows of rank 3,
    // the start's x and the end's y one dependency, all tag 0. So no dependency is listed,
    // nothing is flagged, 0 left, nothing free, and the drawing, which meets every rule,
    // solves.
    let mut b = systems::Builder::default();
    let center = silico_protocol::PointValues {
        x: b.fixed(0.0),
        y: b.fixed(0.0),
    };
    let radius = b.unknown(5.0);
    let start_angle = b.unknown(0.0);
    let end_angle = b.unknown(std::f64::consts::FRAC_PI_2);
    let start = silico_protocol::PointValues {
        x: b.fixed(5.0),
        y: b.fixed(0.0),
    };
    let end = silico_protocol::PointValues {
        x: b.fixed(0.0),
        y: b.fixed(5.0),
    };
    let arc = b.add(silico_protocol::SketchGeometry::Arc {
        center,
        radius,
        start_angle,
        end_angle,
        start,
        end,
    });
    b.constraints
        .push(silico_protocol::SolverConstraint::ArcRules { arc });
    let solved = solve(&mut worker(), b.system(Algorithm::DogLeg));
    assert!(solved.dependencies.is_empty(), "{solved:?}");
    assert!(solved.conflicting.is_empty(), "{solved:?}");
    assert!(solved.redundant.is_empty(), "{solved:?}");
    assert!(solved.partially_redundant.is_empty(), "{solved:?}");
    assert_eq!(solved.dofs, 0, "{solved:?}");
    assert!(solved.free.is_empty(), "{solved:?}");
    assert_eq!(solved.status, SolveStatus::Success, "{solved:?}");
}

#[test]
fn a_system_that_fails_its_check_is_invalid_input_and_the_worker_lives_on() {
    let mut system = rectangle_40_by_20().b.system(Algorithm::DogLeg);
    system.values[3] = f64::NAN;
    let mut w = worker();
    let err = w.call(Generation(1), Op::Solve(system), WAIT).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert_eq!(err.parameter(), Some("values"), "{err}");
    assert!(err.message().contains("value 3"), "{err}");
    assert!(echo(&mut w));
}

#[test]
fn a_kernel_operation_or_a_render_is_refused_as_another_workers() {
    let mut w = worker();
    for (op, name) in [
        (
            Op::MakePrimitive(Primitive::Box {
                dx: 1.0,
                dy: 1.0,
                dz: 1.0,
            }),
            "MakeBox",
        ),
        (
            Op::Render(Box::new(silico_protocol::RenderScene {
                bodies: vec![],
                camera: silico_protocol::Camera {
                    projection: silico_protocol::Projection::Orthographic { view_height: 1.0 },
                    eye: [0.0, 0.0, 1.0],
                    target: [0.0; 3],
                    up: [0.0, 1.0, 0.0],
                },
                width: 1,
                height: 1,
                ids: false,
            })),
            "Render",
        ),
    ] {
        let err = w.call(Generation(1), op, WAIT).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Internal, "{err}");
        assert_eq!(
            err.message(),
            format!("the sketch worker couldn't do {name}")
        );
        assert_eq!(
            err.cause(),
            Some(format!("{name} is answered by another kind of worker, not this one").as_str())
        );
    }
    assert!(echo(&mut w));
}

/// Whether this machine keeps floating-point trap-enable bits once they're set: Linux
/// does; some Apple silicon ignores them (they read back as 0), and some keeps them.
fn traps_stay_on_here() -> bool {
    silico_sketch::test_ops::enable_fp_traps().unwrap();
    let on = silico_sketch::fp_trap_mask() != 0;
    silico_sketch::disable_fp_traps().unwrap();
    on
}

#[test]
fn fp_traps_switched_on_by_a_request_fail_it() {
    let mut w = test_worker();
    let result = w.call(Generation(1), Op::Debug(DebugOp::EnableFpTraps), WAIT);
    if traps_stay_on_here() {
        let err = result.unwrap_err();
        assert_eq!(err.code(), ErrorCode::Internal, "{err}");
        assert!(
            err.cause().unwrap().contains("floating-point traps are on"),
            "{err}"
        );
    } else {
        // The hardware ignores the trap-enable bits: the request succeeds, traps off.
        assert!(matches!(result.unwrap(), Reply::Done));
    }
    // Either way the worker has them off again and carries on.
    let system = rectangle_40_by_20().b.system(Algorithm::DogLeg);
    assert_eq!(solve(&mut w, system).status, SolveStatus::Success);
}

#[test]
fn fp_trap_check_fails_when_traps_are_on() {
    // In-process: the check the sketch worker runs at startup and after every request, which
    // a worker whose output is a result must run (ARCHITECTURE.md, Deterministic kernel, 7).
    let platform = &silico_sketch::worker::PLATFORM;
    assert!(platform.traps.check().is_some());
    assert!(platform.traps.check().unwrap().is_ok());
    let stays_on = traps_stay_on_here();
    silico_sketch::test_ops::enable_fp_traps().unwrap();
    let on = platform.traps.check().unwrap();
    silico_sketch::disable_fp_traps().unwrap();
    if cfg!(target_os = "linux") {
        assert!(stays_on, "Linux keeps trap-enable bits");
    }
    if stays_on {
        assert!(on.unwrap_err().contains("floating-point traps are on"));
    } else {
        assert!(on.is_ok());
    }
    assert!(platform.traps.check().unwrap().is_ok());
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
        Op::Debug(DebugOp::ThrowBadAlloc),
    ] {
        let name = op.name();
        let err = w.call(Generation(1), op, WAIT).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Internal, "{name}: {err}");
        assert!(
            err.cause().unwrap().contains("test operation"),
            "{name}: {err}"
        );
    }
    assert!(echo(&mut w));
}

#[test]
fn std_bad_alloc_in_the_bridge_is_the_cores_resource_exhausted_and_the_worker_lives_on() {
    // libc++'s std::bad_alloc thrown in the worker's C++, read as a solve reads it,
    // sent as a refusal, and worded by the core: the whole way a sketch that runs the
    // solver out of memory takes.
    let mut w = test_worker();
    let err = w
        .call(Generation(1), Op::Debug(DebugOp::ThrowBadAlloc), WAIT)
        .unwrap_err();
    assert_eq!(
        (err.code(), err.message(), err.cause(), err.suggestion()),
        (
            ErrorCode::ResourceExhausted,
            "DebugThrowBadAlloc ran out of memory",
            Some("std::bad_alloc"),
            Some("free some memory, or make the part or sketch simpler, and try again")
        ),
        "{err}"
    );
    assert!(echo(&mut w));
}

fn assert_crashed(op: Op, signal: &str) {
    let mut w = test_worker();
    let pid = w.pid();
    assert!(process_exists(pid), "worker {pid} isn't running");
    let err = w.call(Generation(1), op, WAIT).unwrap_err();
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
    // Its 1 s, then a kill: 10 s on a quiet machine with room to spare, and the wakes on the
    // way as late as this machine runs them.
    let lateness = crate::lateness::Lateness::start();
    let start = Instant::now();
    let err = w
        .call(
            Generation(1),
            Op::Debug(DebugOp::Sleep { millis: 60_000 }),
            Duration::from_secs(1),
        )
        .unwrap_err();
    assert!(
        start.elapsed() < crate::lateness::allowance(Duration::from_secs(10), lateness.most()),
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
        .call(Generation(1), Op::Debug(DebugOp::Garbage), WAIT)
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
                WAIT,
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

/// The worker imports no inexact platform math: PlaneGCS's comes from the libm shim
/// (ARCHITECTURE.md, Deterministic kernel). (The test worker, which can switch traps on,
/// imports `feenableexcept` on Linux, which only OCCT's TKernel may.)
#[test]
fn the_worker_imports_no_platform_math() {
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../third_party/occt/scripts/check-math-imports.sh"
    );
    let out = Command::new("bash")
        .arg(script)
        .arg(WORKER)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_begun_solve_whose_worker_then_died_says_it_ended_and_to_solve_again() {
    // The milestone's mutants: a solve begun with a stopper starts no worker, and one whose
    // worker has died fails at once, saying the worker ended during the solve and that the
    // next solve starts another, rather than sending the request to the dead worker and
    // reporting a crash during it (`is_stopped() || has_exited()` read as `&&` passed every
    // test). It answered once first, so it had started: its death is a crash.
    use silico_core::sketch::{SketchSolver, SketchWorker};
    let mut w = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    w.solve(rectangle_40_by_20().b.system(Algorithm::DogLeg))
        .unwrap();
    let _stopper = w.stopper().unwrap();
    let pid = w.pid().to_string();
    assert!(
        Command::new("kill")
            .args(["-9", &pid])
            .status()
            .unwrap()
            .success()
    );
    // Until it has exited (a zombie, or gone): the solve finds it so.
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
    let e = w
        .solve(rectangle_40_by_20().b.system(Algorithm::DogLeg))
        .unwrap_err();
    assert_eq!(e.code(), ErrorCode::WorkerCrashed, "{e}");
    assert_eq!(
        e.message(),
        "the sketch worker ended during the solve",
        "{e}"
    );
    assert_eq!(
        e.suggestion(),
        Some("solve again: the next solve starts another worker"),
        "{e}"
    );
}
