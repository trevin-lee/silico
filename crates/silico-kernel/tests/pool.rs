//! M1.6: the kernel worker pool (silico_core::pool). Results through the pool are the same
//! as one worker's, request for request; workers killed mid-burst fail only their own
//! requests and are replaced; results for superseded generations are never delivered; and
//! no worker process outlives the pool or is left a zombie.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/imports.rs"]
mod imports;
#[path = "common/kernel.rs"]
mod kernel;
#[path = "../../silico-core/tests/common/lateness.rs"]
mod lateness;
#[path = "common/pool.rs"]
mod pool;
#[path = "common/scripts.rs"]
mod scripts;

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use kernel::kernel;
use lateness::{Lateness, allowance};
use pool::{
    PATIENCE, TEST_WORKER, WORKER, assert_reaped, children, done, echo, kill9_and_wait, outcome,
    pool, signal, sleep, spin, wait_full, wait_full_by, wait_no_children, wait_no_children_besides,
};
use silico_core::ErrorCode;
use silico_core::kernel::{
    Blob, BooleanOp, BuiltData, ChamferSize, Kernel, KernelRequest, Mesh, Primitive, Profile,
    ShapeData, ShapeSummary,
};
use silico_core::pool::{KernelPool, Outcome, Pending, PoolClient, STOP_AFTER};
use silico_protocol::{
    DebugOp, Failure, Generation, Misbehavior, Op, Refusal, Reply, RequestId, Response,
};

/// One of the mixed requests the concurrency test sends, for both the pool and the
/// reference worker.
enum Request {
    Shape(KernelRequest<ShapeData>),
    Built(KernelRequest<BuiltData>),
    Summary(KernelRequest<ShapeSummary>),
    Mesh(KernelRequest<Mesh>),
    Echo(KernelRequest<Blob>),
}

enum Submitted {
    Shape(Pending<ShapeData>),
    Built(Pending<BuiltData>),
    Summary(Pending<ShapeSummary>),
    Mesh(Pending<Mesh>),
    Echo(Pending<Blob>),
}

struct Inputs {
    block: ShapeData,
    cylinder: ShapeData,
    profile: Profile,
}

/// The `i`th request: eight kinds, each with values that vary with `i`.
fn request(i: u32, inputs: &Inputs) -> Request {
    let f = f64::from(i % 50);
    match i % 8 {
        0 => Request::Shape(
            KernelRequest::make(Primitive::Box {
                dx: 1.0 + f,
                dy: 2.0,
                dz: 3.0 + f / 10.0,
            })
            .unwrap(),
        ),
        1 => Request::Built(
            KernelRequest::fillet(&inputs.block, &[1 + i % 12], 0.5 + f / 25.0).unwrap(),
        ),
        2 => Request::Built(
            KernelRequest::chamfer(
                &inputs.block,
                &[1 + i % 12, 1 + (i + 5) % 12],
                ChamferSize::Equal {
                    distance: 0.25 + f / 50.0,
                },
                i.is_multiple_of(3),
            )
            .unwrap(),
        ),
        3 => Request::Built(KernelRequest::boolean(
            [BooleanOp::Fuse, BooleanOp::Cut, BooleanOp::Common][(i % 3) as usize],
            &inputs.block,
            &inputs.cylinder,
        )),
        4 => Request::Summary(KernelRequest::summarize(&inputs.block)),
        5 => Request::Mesh(KernelRequest::tessellate(
            &inputs.cylinder,
            0.05 + f / 100.0,
        )),
        6 => Request::Built(KernelRequest::extrude(&inputs.profile, 1.0 + f, None).unwrap()),
        _ => Request::Echo(KernelRequest::echo(Blob::inline(
            (0..i).map(|b| b as u8).collect(),
        ))),
    }
}

fn inputs(k: &mut silico_core::kernel::KernelWorker) -> Inputs {
    let block = k
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    let cylinder = k
        .make(Primitive::Cylinder {
            radius: 4.0,
            height: 40.0,
        })
        .unwrap();
    let cylinder = k.translate(&cylinder, [5.0, 10.0, -5.0]).unwrap().shape;
    let profile = Profile {
        points: vec![
            [0.0, 0.0, 0.0],
            [4.0, 0.0, 0.0],
            [4.0, 3.0, 0.0],
            [1.0, 5.0, 0.0],
        ],
    };
    Inputs {
        block,
        cylinder,
        profile,
    }
}

#[test]
fn concurrent_requests_correct() {
    // 1,000 requests of eight kinds submitted at once to a pool of four: each result is the
    // one a single worker gives for the same request, byte for byte (shapes, histories,
    // summaries, meshes, and echoes), whichever worker ran it and in whatever order.
    let mut reference = kernel();
    let inputs = inputs(&mut reference);
    let pool = pool(WORKER, 4);
    let submitted: Vec<Submitted> = (0..1000)
        .map(|i| match request(i, &inputs) {
            Request::Shape(r) => Submitted::Shape(pool.submit(Generation(1), r)),
            Request::Built(r) => Submitted::Built(pool.submit(Generation(1), r)),
            Request::Summary(r) => Submitted::Summary(pool.submit(Generation(1), r)),
            Request::Mesh(r) => Submitted::Mesh(pool.submit(Generation(1), r)),
            Request::Echo(r) => Submitted::Echo(pool.submit(Generation(1), r)),
        })
        .collect();
    for (i, pending) in (0..1000).zip(submitted) {
        match (pending, request(i, &inputs)) {
            (Submitted::Shape(p), Request::Shape(r)) => {
                assert_eq!(done(p).unwrap(), reference.run(r).unwrap(), "request {i}");
            }
            (Submitted::Built(p), Request::Built(r)) => {
                assert_eq!(done(p).unwrap(), reference.run(r).unwrap(), "request {i}");
            }
            (Submitted::Summary(p), Request::Summary(r)) => {
                assert_eq!(done(p).unwrap(), reference.run(r).unwrap(), "request {i}");
            }
            (Submitted::Mesh(p), Request::Mesh(r)) => {
                assert_eq!(done(p).unwrap(), reference.run(r).unwrap(), "request {i}");
            }
            (Submitted::Echo(p), Request::Echo(r)) => {
                assert_eq!(done(p).unwrap(), reference.run(r).unwrap(), "request {i}");
            }
            _ => panic!("request {i} changed kind"),
        }
    }
    // And one is right by hand: the 10 × 20 × 30 box's volume.
    let summary = done(pool.submit(Generation(1), KernelRequest::summarize(&inputs.block)));
    let volume = summary.unwrap().volume;
    assert!((volume - 6000.0).abs() <= 6000.0 * 1e-12, "{volume}");
}

#[test]
fn requests_run_in_parallel() {
    // Four sleeps on four workers run at once: all four have started before any is let go.
    // Two at a time, the third would never start.
    let marks = marks_dir("parallel");
    let pool = KernelPool::start_workers(&sleep_marking_worker(&marks), 4).unwrap();
    let sleeps: Vec<_> = (0..4)
        .map(|_| pool.submit(Generation(1), long_sleep()))
        .collect();
    wait_started(&marks, 4);
    release(&marks);
    for pending in sleeps {
        done(pending).unwrap();
    }
    let _ = std::fs::remove_dir_all(&marks);
}

#[test]
fn the_pool_finds_its_worker_and_has_one_per_core_started_as_requests_need_them() {
    // As KernelWorker::start does: next to the test binary's directory. A slot for each
    // core, none started until a request needs it; requests one at a time need one.
    let pool = KernelPool::start().unwrap();
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    assert_eq!(pool.size(), cores.min(silico_core::pool::MAX_WORKERS));
    assert_eq!(pool.pids(), Vec::<u32>::new());
    let block = done(
        pool.submit(
            Generation(1),
            KernelRequest::make(Primitive::Box {
                dx: 1.0,
                dy: 2.0,
                dz: 3.0,
            })
            .unwrap(),
        ),
    )
    .unwrap();
    let summary = done(pool.submit(Generation(1), KernelRequest::summarize(&block))).unwrap();
    assert!(
        (summary.volume - 6.0).abs() <= 6.0 * 1e-12,
        "{}",
        summary.volume
    );
    assert_eq!(pool.pids().len(), 1, "{:?}", pool.pids());
}

#[test]
fn a_pool_on_demand_starts_as_many_workers_as_requests_run_at_once_and_no_more() {
    let marks = marks_dir("on-demand");
    let pool = KernelPool::start_on_demand(&sleep_marking_worker(&marks), 8).unwrap();
    assert_eq!(pool.size(), 8);
    assert_eq!(pool.pids(), Vec::<u32>::new());
    // `n` sleeps at once, let go only once as many as the pool runs at once (`n`, up to
    // its eight) have started: three workers for three, each request on its own.
    let at_once = |n: usize| {
        let _ = std::fs::remove_file(marks.join("running"));
        let _ = std::fs::remove_file(marks.join("release"));
        let pending: Vec<_> = (0..n)
            .map(|_| pool.submit(Generation(1), long_sleep()))
            .collect();
        wait_started(&marks, n.min(8));
        release(&marks);
        for p in pending {
            done(p).unwrap();
        }
    };
    at_once(3);
    assert_eq!(pool.pids().len(), 3, "{:?}", pool.pids());
    // Idle, the pool starts no more; three at once again run on the three it has, at once.
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(pool.pids().len(), 3, "{:?}", pool.pids());
    let before: BTreeSet<u32> = pool.pids().into_iter().collect();
    at_once(3);
    assert_eq!(pool.pids().into_iter().collect::<BTreeSet<u32>>(), before);
    // Five at once: two more. Twelve: the pool's eight, no more.
    at_once(5);
    assert_eq!(pool.pids().len(), 5, "{:?}", pool.pids());
    at_once(12);
    assert_eq!(pool.pids().len(), 8, "{:?}", pool.pids());
    drop(pool);
    assert_eq!(children(), Vec::new());
    let _ = std::fs::remove_dir_all(&marks);
}

#[test]
fn a_pool_on_demand_whose_worker_cant_start_fails_the_request_and_starts_on() {
    // A worker that can't be run: the pool opens (it starts none), and each request fails
    // as a start that crashed does, rather than waiting; made runnable, the next request
    // starts it and runs.
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("pool-on-demand-unrunnable-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("silico-kernel-test-worker");
    std::fs::write(&exe, format!("#!/bin/sh\nexec '{TEST_WORKER}'\n")).unwrap();
    let pool = KernelPool::start_on_demand(&exe, 2).unwrap();
    for _ in 0..2 {
        let err = done(pool.submit(Generation(1), sleep(0))).unwrap_err();
        assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    }
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    done(pool.submit(Generation(1), sleep(0))).unwrap();
    drop(pool);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sizes_past_the_most_workers_are_refused() {
    for size in [silico_core::pool::MAX_WORKERS + 1, 1 << 40, usize::MAX] {
        let err = KernelPool::start_workers(Path::new(TEST_WORKER), size)
            .err()
            .unwrap();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{size}: {err}");
    }
    assert_eq!(children(), Vec::new());
}

#[test]
fn wait_gives_each_outcome() {
    let pool = pool(TEST_WORKER, 1);
    assert!(matches!(
        pool.submit(Generation(1), echo(&[1])).wait(),
        Outcome::Done(Ok(_))
    ));
    pool.advance(Generation(2));
    assert!(matches!(
        pool.submit(Generation(1), echo(&[2])).wait(),
        Outcome::Superseded
    ));
    let running = pool.submit(Generation(2), sleep(30_000));
    std::thread::sleep(Duration::from_millis(100));
    drop(pool);
    match running.wait() {
        Outcome::Done(Err(e)) => {
            assert_eq!(e.code(), ErrorCode::Internal, "{e}");
            assert!(e.message().contains("DebugSleep"), "{e}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn wait_timeout_gives_the_request_back_when_it_isnt_done() {
    // A wait of 50 ms for a 3 s sleep gives the request back undone: one that waited for
    // the request would give its outcome.
    let pool = pool(TEST_WORKER, 1);
    let pending = pool
        .submit(Generation(1), sleep(3_000))
        .wait_timeout(Duration::from_millis(50))
        .expect_err("done in 50 ms");
    let pending = pending.try_take().expect_err("done at once");
    assert!(matches!(outcome(pending), Outcome::Done(Ok(()))));
}

#[test]
fn advancing_to_a_generation_keeps_its_queued_requests() {
    // A request for the generation about to start, queued behind a stale one, runs.
    let pool = pool(TEST_WORKER, 1);
    let slow = pool.submit(Generation(0), sleep(300));
    std::thread::sleep(Duration::from_millis(100));
    let queued = pool.submit(Generation(2), echo(&[1]));
    pool.advance(Generation(2));
    assert!(matches!(outcome(slow), Outcome::Superseded));
    assert_eq!(done(queued).unwrap(), Blob::inline(vec![1]));
}

#[test]
fn advance_stops_superseded_work_so_the_new_generation_doesnt_wait() {
    // Four workers busy on sleeps for generation 1, each longer than an outcome is waited
    // for. Generation 2 starts: the sleeps are stopped (their workers killed and replaced),
    // so generation 2's first request runs, and every stale one ends: not one runs on to
    // its end, which would outlast the wait for it.
    let pool = pool(TEST_WORKER, 4);
    let before: BTreeSet<u32> = pool.pids().into_iter().collect();
    let stale: Vec<_> = (0..4)
        .map(|_| pool.submit(Generation(1), long_sleep()))
        .collect();
    std::thread::sleep(Duration::from_millis(200));
    pool.advance(Generation(2));
    let current = outcome(pool.submit(Generation(2), echo(&[1])));
    for pending in stale {
        assert!(matches!(outcome(pending), Outcome::Superseded));
    }
    assert!(matches!(current, Outcome::Done(Ok(_))), "{current:?}");
    // And every worker was replaced.
    wait_full(&pool, 4, &before);
}

/// How long a test here waits for a full pool before failing: long enough that only a hang
/// reaches it, however loaded the machine.
const HANG: Duration = Duration::from_secs(60);

/// What stopping a superseded request takes on a quiet machine, with room to spare: from
/// when it's due to when its caller has the outcome (the stopper waking, the kill, the
/// slot seeing its worker end, the caller waking). A loaded machine gets ten times its
/// lateness on top ([`allowance`]); a stopper that looks again only once a second is up to
/// a second late, however quiet the machine.
const STOP_TAKES: Duration = Duration::from_millis(250);

#[test]
fn superseded_work_too_young_to_stop_at_advance_is_stopped_once_it_has_run_stop_after() {
    // Generation 2 starts 10 ms into 30 s sleeps of generation 1, too soon to stop them. They
    // aren't left to run: each is stopped once it has run STOP_AFTER, so generation 2's first
    // request waits that and a worker start, not the sleeps. On a pool of one, and of four.
    // Each is stopped when it's due (STOP_AFTER from its start, or the advance if that came
    // later), within what a stop takes and the machine's lateness; and generation 2's request
    // is answered before the sleeps would end.
    const SLEEP: Duration = Duration::from_secs(30);
    for size in [1, 4] {
        let pool = pool(TEST_WORKER, size);
        let before: BTreeSet<u32> = pool.pids().into_iter().collect();
        let lateness = Lateness::start();
        let start = Instant::now();
        let stale: Vec<_> = (0..size)
            .map(|_| pool.submit(Generation(1), sleep(SLEEP.as_millis() as u64)))
            .collect();
        std::thread::sleep(Duration::from_millis(10));
        let advanced = Instant::now();
        pool.advance(Generation(2));
        let current = pool.submit(Generation(2), echo(&[1]));
        // Each stale outcome as it's delivered, before generation 2's request is answered:
        // that waits for a worker to be started in the stopped one's place, work that takes
        // CPU, which the lateness probe doesn't measure (it measures how late threads wake).
        let mut ended = Vec::new();
        for pending in stale {
            assert!(matches!(outcome(pending), Outcome::Superseded));
            ended.push(Instant::now());
        }
        let current = current.wait();
        let waited = start.elapsed();
        let late = allowance(STOP_TAKES, lateness.most());
        let due = (start + STOP_AFTER).max(advanced);
        assert!(matches!(current, Outcome::Done(Ok(_))), "{current:?}");
        for (i, ended) in ended.into_iter().enumerate() {
            assert!(
                ended < due + late,
                "pool of {size}: request {i} stopped {:?} after it was due, past {late:?}",
                ended - due
            );
        }
        assert!(
            waited < SLEEP,
            "pool of {size}: waited {waited:?} behind superseded work, as long as it runs"
        );
        wait_full_by(&pool, size, &before, Instant::now() + HANG);
    }
}

#[test]
fn stopping_superseded_work_never_fails_a_current_request() {
    // With every superseded request stopped at once (stop-after zero), generations advance
    // every 100 or 300 µs on a pool of one while requests are submitted for whichever is
    // current. A request either succeeds or is superseded. One that fails as a crash was
    // given a worker `advance` had killed: as it was when `advance` landed between a slot
    // marking its request running and checking its generation, which superseded the
    // request but left the killed worker in place (found by the review; a 7 ms cadence
    // never reached it).
    let mut failures = Vec::new();
    let mut superseded = 0;
    for gap in [Duration::from_micros(100), Duration::from_micros(300)] {
        let pool = pool(TEST_WORKER, 1);
        pool.set_stop_after(Duration::ZERO);
        let mut workers = BTreeSet::new();
        let stop = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut generation = 1;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    generation += 1;
                    pool.advance(Generation(generation));
                    spin(gap);
                }
            });
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(4) {
                match outcome(pool.submit(pool.generation(), sleep(0))) {
                    Outcome::Done(Ok(())) => {}
                    Outcome::Superseded => superseded += 1,
                    Outcome::Done(Err(e)) => failures.push(e),
                }
                workers.extend(pool.pids());
            }
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        // Workers were stopped, so the race was run.
        assert!(workers.len() > 1, "{gap:?}: no worker was ever stopped");
    }
    assert!(superseded > 0, "no generation ever superseded a request");
    assert!(
        failures.is_empty(),
        "{} current requests failed; the first: {}",
        failures.len(),
        failures[0]
    );
}

#[test]
fn many_submitters_never_get_a_failure_from_stopped_work() {
    // The same, eight workers and six threads submitting, and advances every 0 to 300 µs.
    let pool = pool(TEST_WORKER, 8);
    pool.set_stop_after(Duration::ZERO);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let deadline = Instant::now() + Duration::from_secs(5);
    let (failures, workers): (Vec<String>, BTreeSet<u32>) = std::thread::scope(|scope| {
        let advancer = scope.spawn(|| {
            let mut workers = BTreeSet::new();
            let mut generation = 1u64;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                generation += 1;
                pool.advance(Generation(generation));
                workers.extend(pool.pids());
                spin(Duration::from_micros((generation * 37) % 300));
            }
            workers
        });
        let submitters: Vec<_> = (0..6u8)
            .map(|t| {
                let pool = &pool;
                scope.spawn(move || {
                    let mut failures = Vec::new();
                    while Instant::now() < deadline {
                        let batch: Vec<_> = (0..16)
                            .map(|_| pool.submit(pool.generation(), echo(&[t])))
                            .collect();
                        for pending in batch {
                            if let Outcome::Done(Err(e)) = outcome(pending) {
                                failures.push(e.to_string());
                            }
                        }
                    }
                    failures
                })
            })
            .collect();
        let all = submitters
            .into_iter()
            .flat_map(|s| s.join().unwrap())
            .collect();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        (all, advancer.join().unwrap())
    });
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(workers.len() > 8, "no worker was ever stopped");
    wait_full(&pool, 8, &BTreeSet::new());
}

#[test]
fn advancing_to_a_generation_leaves_its_running_requests_alone() {
    // A request for the generation `advance` starts, already running, isn't superseded.
    let pool = pool(TEST_WORKER, 1);
    pool.set_stop_after(Duration::ZERO);
    let running = pool.submit(Generation(2), sleep(300));
    std::thread::sleep(Duration::from_millis(100));
    pool.advance(Generation(2));
    assert!(matches!(outcome(running), Outcome::Done(Ok(()))));
}

#[test]
fn a_request_after_advance_never_lands_on_a_killed_worker() {
    // `advance` swept across the moment a superseded request is taken and marked running
    // (every superseded request stopped at once): the next request, for the current
    // generation, never gets the killed worker. (A request superseded before it ran left
    // its killed worker in place, found by the review.) An `advance` before that moment
    // supersedes the request unrun, keeping its worker; one after it stops the worker.
    let pool = pool(TEST_WORKER, 1);
    pool.set_stop_after(Duration::ZERO);
    let mut failures = Vec::new();
    sweep_across(300, Duration::from_micros(100), |delay| {
        let g = pool.generation().0;
        let before = pool.pids();
        drop(pool.submit(Generation(g), sleep(200)));
        spin(delay);
        pool.advance(Generation(g + 1));
        match outcome(pool.submit(Generation(g + 1), echo(&[1]))) {
            Outcome::Done(Ok(_)) => {}
            other => failures.push((delay, format!("{other:?}"))),
        }
        if pool.pids() == before {
            Landed::Before
        } else {
            Landed::After
        }
    });
    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
fn a_worker_killed_as_its_request_ends_is_never_given_another() {
    // `advance` swept across the end of a quick request (every superseded request stopped at
    // once): one that lands after the reply but before the slot is idle kills a worker with
    // nothing left to do, and the slot must replace it, not give it the next request. An
    // `advance` while the request runs, up to the slot's being idle, stops its worker; one
    // after it, the request's result is delivered. (One before the request was taken
    // supersedes it unrun, which says nothing of that moment.)
    let pool = pool(TEST_WORKER, 1);
    pool.set_stop_after(Duration::ZERO);
    let mut failures = Vec::new();
    sweep_across(600, Duration::from_micros(300), |delay| {
        let g = pool.generation().0;
        let before = pool.pids();
        let first = pool.submit(Generation(g), echo(&[0]));
        spin(delay);
        pool.advance(Generation(g + 1));
        let delivered = matches!(outcome(first), Outcome::Done(_));
        match outcome(pool.submit(Generation(g + 1), echo(&[1]))) {
            Outcome::Done(Ok(_)) => {}
            other => failures.push((delay, format!("{other:?}"))),
        }
        if delivered {
            Landed::After
        } else if pool.pids() != before {
            Landed::Before
        } else {
            Landed::Neither
        }
    });
    assert!(failures.is_empty(), "{failures:?}");
}

/// Which side of the moment a sweep aims at an attempt's `advance` landed on, as its
/// outcome shows, if it does.
enum Landed {
    Before,
    After,
    Neither,
}

/// Runs `attempt` `attempts` times, with delays from zero across `window`, and again across
/// a window twice as wide each time until attempts have landed on both sides of the moment
/// it aims at, so the sweep crossed it.
///
/// The moments these tests aim an `advance` at are inside a slot's own thread (between its
/// marking a request running and checking the generation, or between a reply and the slot
/// being idle), before the worker has the request or after it has answered, so no signal
/// from the worker reaches them and the test can't wait on one: only a sweep lands there.
/// A loaded machine moves the moment later, and the window widens to follow it rather than
/// ending short of it (a fixed 100 µs fell short of it on a loaded machine). Each wider
/// pass is sparser; the first keeps the density the moment was found with.
fn sweep_across(attempts: u32, window: Duration, mut attempt: impl FnMut(Duration) -> Landed) {
    let start = Instant::now();
    let mut window = window;
    let (mut before, mut after) = (0, 0);
    loop {
        for i in 0..attempts {
            match attempt(window * i / attempts) {
                Landed::Before => before += 1,
                Landed::After => after += 1,
                Landed::Neither => {}
            }
            assert!(
                (before > 0 && after > 0) || start.elapsed() < HANG,
                "the sweep never crossed the moment, with advances up to {window:?} after a \
                 request: {before} landed before it, {after} after"
            );
        }
        if before > 0 && after > 0 {
            return;
        }
        window *= 2;
    }
}

// ---------------------------------------------------------------------------------------
// A worker that says when it's running a request: this test binary, run again as a worker
// by a script (a worker gets no environment of its own, so the script names the directory),
// which adds a line to `running` in that directory as it takes a sleep, then sleeps, until
// `release` is there or the sleep is over.

const MARKS_ENV: &str = "SILICO_POOL_TEST_MARKS";

/// Not a test of its own: run by [`sleep_marking_worker`]'s script, this binary serves as a
/// kernel worker that marks each sleep it takes. Run normally, it does nothing.
#[test]
fn marking_worker_entry() {
    let Ok(dir) = std::env::var(MARKS_ENV) else {
        return;
    };
    let (running, release) = (
        Path::new(&dir).join("running"),
        Path::new(&dir).join("release"),
    );
    let _ = silico_kernel::worker::run(move |_, _, op, _| match op {
        Op::Debug(DebugOp::Sleep { millis }) => {
            let internal = |e: std::io::Error| {
                Failure::new(Refusal::Internal {
                    what: e.to_string(),
                })
            };
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&running)
                .and_then(|mut f| f.write_all(b"running\n"))
                .map_err(internal)?;
            let until = Instant::now() + Duration::from_millis(millis);
            while Instant::now() < until && !release.exists() {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(Reply::Done)
        }
        op => silico_kernel::worker::handle(op),
    });
    std::process::exit(0);
}

/// A worker that, as it takes a sleep, adds a line to `running` in `dir`, and ends the sleep
/// early once `release` is there (see [`marking_worker_entry`]).
fn sleep_marking_worker(dir: &Path) -> std::path::PathBuf {
    let me = std::env::current_exe().unwrap();
    scripts::script(
        "marking-worker",
        &format!(
            "{MARKS_ENV}='{}' exec '{}' --exact marking_worker_entry --nocapture --test-threads=1",
            dir.display(),
            me.display()
        ),
    )
}

/// A sleep longer than [`PATIENCE`], allowed an hour before the pool stops its worker as
/// hung: one that should have been stopped, or let go, and wasn't fails the test by its
/// outcome's wait, not by a bound on the time it took.
fn long_sleep() -> KernelRequest<()> {
    sleep(2 * PATIENCE.as_millis() as u64).with_timeout(Duration::from_secs(3600))
}

/// A fresh directory for a marking worker's marks.
fn marks_dir(what: &str) -> std::path::PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("silico-pool-{what}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Waits until `n` sleeps have started on the marking workers of `dir`, failing after
/// [`HANG`]: had fewer run at once, the rest would never start before they're let go.
fn wait_started(dir: &Path, n: usize) {
    let deadline = Instant::now() + HANG;
    loop {
        let started = std::fs::read_to_string(dir.join("running")).map_or(0, |s| s.lines().count());
        if started >= n {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{started} sleeps started at once, not {n}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Lets the marking workers' sleeps of `dir` go, and those after.
fn release(dir: &Path) {
    std::fs::write(dir.join("release"), b"").unwrap();
}

/// Lets the marking workers of its directory go when it's dropped, so a test that fails
/// leaves none waiting.
struct Released<'a>(&'a Path);

impl Drop for Released<'_> {
    fn drop(&mut self) {
        let _ = std::fs::write(self.0.join("release"), b"");
    }
}

#[test]
fn a_running_superseded_request_that_ends_within_stop_after_finishes_on_its_worker() {
    // Stopping a request costs a new worker (about 30 ms), more than letting a short one
    // finish: a superseded request, running, that ends within STOP_AFTER is left to, its
    // result discarded, and its worker kept. A short request is superseded once its worker
    // says it's running it (an event, not a guess at how soon it starts), ATTEMPTS times.
    // One a loaded machine makes answer before the test sees it running isn't superseded,
    // and one it makes take past STOP_AFTER is rightly stopped; but none is stopped sooner,
    // and at least one must have run to its end on its worker.
    const REQUEST_MILLIS: u64 = 25;
    const ATTEMPTS: u64 = 10;
    let marks = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("silico-pool-marks-{}", std::process::id()));
    std::fs::create_dir_all(&marks).unwrap();
    let running = marks.join("running");
    let pool = KernelPool::start_workers(&sleep_marking_worker(&marks), 1).unwrap();
    done(pool.submit(Generation(0), echo(b"warm"))).unwrap();
    let mut finished_on_its_worker = 0;
    for g in 1..=ATTEMPTS {
        let _ = std::fs::remove_file(&running);
        let before = pool.pids();
        let start = Instant::now();
        let young = pool.submit(Generation(g), sleep(REQUEST_MILLIS));
        while !running.exists() {
            assert!(start.elapsed() < HANG, "attempt {g}: the request never ran");
            std::thread::sleep(Duration::from_millis(1));
        }
        pool.advance(Generation(g + 1));
        let superseded = match outcome(young) {
            Outcome::Superseded => true,
            Outcome::Done(Ok(_)) => false,
            Outcome::Done(Err(e)) => panic!("attempt {g}: {e}"),
        };
        let elapsed = start.elapsed();
        // Once another request has been through the slot, a stopped worker has been
        // replaced.
        assert_eq!(
            done(pool.submit(Generation(g + 1), echo(&[1]))).unwrap(),
            Blob::inline(vec![1])
        );
        if pool.pids() == before {
            if superseded && elapsed >= Duration::from_millis(REQUEST_MILLIS) {
                finished_on_its_worker += 1;
            }
        } else {
            assert!(
                superseded && elapsed >= STOP_AFTER,
                "attempt {g}: its worker was stopped {elapsed:?} in, before STOP_AFTER"
            );
        }
    }
    assert!(
        finished_on_its_worker > 0,
        "no superseded request ran to its end on its worker"
    );
    let _ = std::fs::remove_dir_all(&marks);
}

#[test]
fn an_advance_after_a_result_never_replaces_the_worker_that_gave_it() {
    // The usual sequence of edits: a request's result comes, then the next edit starts a
    // generation. The slot that gave the result has nothing left to run, so its worker is
    // kept, however soon the advance comes (every superseded request stopped at once). A
    // slot still marked running until after its result was sent had its worker killed when
    // the advance came in between (found by the review).
    let pool = pool(TEST_WORKER, 1);
    pool.set_stop_after(Duration::ZERO);
    let mut replaced = Vec::new();
    for attempt in 0..1000u64 {
        let before = pool.pids();
        let g = pool.generation().0;
        assert!(matches!(
            outcome(pool.submit(Generation(g), sleep(1))),
            Outcome::Done(Ok(()))
        ));
        pool.advance(Generation(g + 1));
        done(pool.submit(Generation(g + 1), echo(&[1]))).unwrap();
        if pool.pids() != before {
            replaced.push(attempt);
        }
    }
    assert!(replaced.is_empty(), "workers replaced at {replaced:?}");
}

#[test]
fn superseded_work_is_stopped_its_stop_after_from_its_start_not_from_the_advance() {
    // With a stop-after of 3 s, a request superseded 2.9 s in is stopped 3 s from its start,
    // not 3 s from the advance (5.9 s): no sooner than 3 s after it was submitted (it can't
    // have started before), and within what a stop takes and the machine's lateness of
    // that. Due 0.1 s after the advance, so a stopper that looks again only once a second
    // stops it 0.9 s late.
    let stop_after = Duration::from_secs(3);
    let pool = pool(TEST_WORKER, 1);
    pool.set_stop_after(stop_after);
    let before: BTreeSet<u32> = pool.pids().into_iter().collect();
    let lateness = Lateness::start();
    let submitted = Instant::now();
    let stale = pool.submit(Generation(1), sleep(30_000));
    std::thread::sleep(Duration::from_millis(2_900));
    let advanced = Instant::now();
    pool.advance(Generation(2));
    assert!(matches!(outcome(stale), Outcome::Superseded));
    let ended = Instant::now();
    let late = allowance(STOP_TAKES, lateness.most());
    assert!(
        ended >= submitted + stop_after,
        "stopped {:?} after it was submitted, before its stop-after",
        ended - submitted
    );
    // Due at its stop-after from its start, or at the advance if that came later.
    let due = (submitted + stop_after).max(advanced);
    assert!(
        ended < due + late,
        "stopped {:?} after it was due, past {late:?}: counted from the advance, or late",
        ended - due
    );
    // And, whatever the lateness probe saw, before a stop counted from the advance would
    // come: load can't make a correct pool fail that.
    assert!(
        ended < advanced + stop_after,
        "stopped {:?} after the advance: counted from it",
        ended - advanced
    );
    wait_full_by(&pool, 1, &before, Instant::now() + HANG);
}

#[test]
fn each_superseded_request_is_stopped_when_it_is_due() {
    // Three superseded requests, started 2 s apart, with a stop-after of 7.1 s: each is
    // stopped at its own 7.1 s, none kept waiting for another's, whichever slots they ran
    // on. The advance comes 2 s after the last starts, before the first is due. Each ends
    // no sooner than 7.1 s after it was submitted (it can't have started before), and
    // within what a stop takes and the machine's lateness of that: stopping each only when
    // the last is due, or only the first, fails that, and so does a stopper that looks
    // again only once a second (they're due 1.1, 3.1, and 5.1 s after the advance: it
    // stops each 0.9 s late).
    const COUNT: usize = 3;
    const SLEEP: Duration = Duration::from_secs(30);
    let stop_after = Duration::from_millis(7_100);
    let stagger = Duration::from_secs(2);
    let pool = pool(TEST_WORKER, COUNT);
    pool.set_stop_after(stop_after);
    let lateness = Lateness::start();
    let mut submitted = Vec::new();
    let mut stale = Vec::new();
    for i in 0..COUNT {
        if i > 0 {
            std::thread::sleep(stagger);
        }
        submitted.push(Instant::now());
        stale.push(pool.submit(Generation(1), sleep(SLEEP.as_millis() as u64)));
    }
    std::thread::sleep(stagger);
    let advanced = Instant::now();
    pool.advance(Generation(2));
    let mut ended = Vec::new();
    for pending in stale {
        assert!(matches!(outcome(pending), Outcome::Superseded));
        ended.push(Instant::now());
    }
    let late = allowance(STOP_TAKES, lateness.most());
    for (i, ended) in ended.into_iter().enumerate() {
        assert!(
            ended >= submitted[i] + stop_after,
            "request {i}: stopped {:?} after it was submitted, before it was due",
            ended - submitted[i]
        );
        let due = (submitted[i] + stop_after).max(advanced);
        assert!(
            ended < due + late,
            "request {i}: stopped {:?} after it was due, past {late:?}",
            ended - due
        );
        // And, whatever the lateness probe saw, before the next is due: a stopper that waits
        // for the last fails that however loaded the machine.
        if let Some(&next) = submitted.get(i + 1) {
            assert!(
                ended < (next + stop_after).max(advanced),
                "request {i}: stopped {:?} after it was submitted, once the next was due",
                ended - submitted[i]
            );
        }
    }
}

#[test]
fn lowering_the_stop_after_stops_superseded_work_already_running() {
    // A superseded request left running under a stop-after longer than the test waits is
    // stopped once the stop-after is lowered to zero, not left to its end (longer than its
    // outcome is waited for).
    let pool = pool(TEST_WORKER, 1);
    pool.set_stop_after(Duration::from_secs(3600));
    let stale = pool.submit(Generation(1), long_sleep());
    std::thread::sleep(Duration::from_millis(100));
    pool.advance(Generation(2));
    std::thread::sleep(Duration::from_millis(100));
    pool.set_stop_after(Duration::ZERO);
    assert!(matches!(outcome(stale), Outcome::Superseded));
}

#[test]
fn a_stop_after_too_long_for_the_clock_never_stops_superseded_work() {
    // Any duration is a stop-after; one too long to add to an instant means never.
    let pool = pool(TEST_WORKER, 1);
    pool.set_stop_after(Duration::MAX);
    let before = pool.pids();
    let start = Instant::now();
    let stale = pool.submit(Generation(1), sleep(300));
    std::thread::sleep(Duration::from_millis(100));
    pool.advance(Generation(2));
    assert!(matches!(outcome(stale), Outcome::Superseded));
    assert!(start.elapsed() >= Duration::from_millis(300));
    assert_eq!(pool.pids(), before);
    assert_eq!(
        done(pool.submit(Generation(2), echo(&[1]))).unwrap(),
        Blob::inline(vec![1])
    );
}

#[test]
fn a_longer_stop_after_lets_superseded_work_run_on() {
    // The stop-after set on a pool is the one it keeps to (the tests above set it to zero,
    // to stop every superseded request at once): with ten seconds, a 300 ms request
    // superseded 100 ms in runs to its end on its worker, where STOP_AFTER would stop it.
    let pool = pool(TEST_WORKER, 1);
    pool.set_stop_after(Duration::from_secs(10));
    let before = pool.pids();
    let start = Instant::now();
    let stale = pool.submit(Generation(1), sleep(300));
    std::thread::sleep(Duration::from_millis(100));
    pool.advance(Generation(2));
    assert!(matches!(outcome(stale), Outcome::Superseded));
    assert!(start.elapsed() >= Duration::from_millis(300));
    assert_eq!(pool.pids(), before, "the superseded request was stopped");
}

#[test]
fn a_crashed_worker_is_replaced_at_once_not_at_the_idle_check() {
    let pool = pool(TEST_WORKER, 1);
    let before = pool.pids();
    let err =
        done(pool.submit(Generation(1), KernelRequest::debug(DebugOp::Segfault))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    // The idle check comes after a second: the replacement must be there well before.
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        let pids = pool.pids();
        if pids.len() == 1 && pids != before {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "not replaced within 500 ms: {pids:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn kill9_mid_burst() {
    // A burst of 240 requests (sleeps and fillets) on four workers, while four of the
    // workers are killed with SIGKILL, one every 60 ms. Each request killed under its worker
    // fails as a crash, and no other: there are no more failures than kills (a worker killed
    // while idle is replaced before it's given a request), every other request gives the
    // right result, and the pool returns to four live workers, none of them killed.
    let mut reference = kernel();
    let block = reference
        .make(Primitive::Box {
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
        })
        .unwrap();
    let fillet = |i: u32| KernelRequest::fillet(&block, &[1 + i % 12], 1.0).unwrap();
    let pool = pool(TEST_WORKER, 4);
    let size = pool.size();
    let mut killed = BTreeSet::new();
    let (sleeps, fillets) = std::thread::scope(|scope| {
        let submitter = scope.spawn(|| {
            let mut sleeps = Vec::new();
            let mut fillets = Vec::new();
            for i in 0..240u32 {
                if i.is_multiple_of(2) {
                    sleeps.push(pool.submit(Generation(1), sleep(15)));
                } else {
                    fillets.push((i, pool.submit(Generation(1), fillet(i))));
                }
            }
            (sleeps, fillets)
        });
        for n in 0..4 {
            std::thread::sleep(Duration::from_millis(60));
            let pids = pool.pids();
            if let Some(&pid) = pids.get(n % pids.len().max(1)) {
                signal("9", pid);
                killed.insert(pid);
            }
        }
        submitter.join().unwrap()
    });
    assert!(!killed.is_empty());
    let mut failures = 0;
    for pending in sleeps {
        match outcome(pending) {
            Outcome::Done(Ok(())) => {}
            Outcome::Done(Err(e)) => {
                assert_eq!(e.code(), ErrorCode::WorkerCrashed, "{e}");
                failures += 1;
            }
            Outcome::Superseded => panic!("superseded"),
        }
    }
    for (i, pending) in fillets {
        match outcome(pending) {
            Outcome::Done(Ok(built)) => {
                assert_eq!(built, reference.run(fillet(i)).unwrap(), "fillet {i}");
            }
            Outcome::Done(Err(e)) => {
                assert_eq!(e.code(), ErrorCode::WorkerCrashed, "{e}");
                failures += 1;
            }
            Outcome::Superseded => panic!("superseded"),
        }
    }
    // The kills land while the workers are busy, so at least one request is killed under
    // its worker: the path this test is for.
    assert!(
        (1..=killed.len()).contains(&failures),
        "{failures} requests failed for {} kills",
        killed.len()
    );
    let pids = wait_full(&pool, size, &killed);

    // pool_no_zombies: the only children are the pool's live workers (and the reference
    // worker); none is a zombie, so every killed worker was reaped.
    let mut expected: BTreeSet<u32> = pids.into_iter().collect();
    expected.insert(reference.pid());
    let kids: BTreeSet<u32> = children().iter().map(|(p, _)| *p).collect();
    assert_eq!(kids, expected, "children: {:?}", children());
    assert!(children().iter().all(|(_, stat)| !stat.starts_with('Z')));

    // And dropping the pool leaves no worker of its own behind.
    let held = pool.pids();
    drop(pool);
    assert_reaped(&held, "after the drop");
    wait_no_children_besides(&BTreeSet::from([reference.pid()]), "after the drop");
    assert_eq!(
        children()
            .iter()
            .map(|(p, _)| *p)
            .collect::<BTreeSet<u32>>(),
        BTreeSet::from([reference.pid()])
    );
}

#[test]
fn a_worker_killed_while_idle_is_replaced_without_failing_a_request() {
    let pool = pool(TEST_WORKER, 2);
    let before = pool.pids();
    kill9_and_wait(before[0]);
    // Every request succeeds: the dead worker is noticed before it's given one.
    for _ in 0..20 {
        done(pool.submit(Generation(1), sleep(1))).unwrap();
    }
    let gone = BTreeSet::from([before[0]]);
    let after = wait_full(&pool, 2, &gone);
    assert!(after.contains(&before[1]), "{before:?} → {after:?}");
}

#[test]
fn an_idle_pool_replaces_a_dead_worker_on_its_own() {
    let pool = pool(TEST_WORKER, 2);
    let before = pool.pids();
    signal("9", before[1]);
    // No request: the idle check finds it within a second or so.
    wait_full(&pool, 2, &BTreeSet::from([before[1]]));
    assert!(children().iter().all(|(_, stat)| !stat.starts_with('Z')));
}

#[test]
fn stale_generation_dropped() {
    // One worker. A slow request from generation 1 has been running 100 ms, and a quick one
    // from generation 1 waits behind it, when generation 2 starts: the waiting one is
    // dropped at once without running, and the slow one is stopped (it has run longer than
    // STOP_AFTER), never delivered. A request for generation 2 runs as usual; one still for
    // generation 1 is superseded at once.
    let pool = pool(TEST_WORKER, 1);
    let slow = pool.submit(Generation(1), sleep(500));
    std::thread::sleep(Duration::from_millis(100));
    let waiting = pool.submit(Generation(1), echo(&[1]));
    pool.advance(Generation(2));
    assert_eq!(pool.generation(), Generation(2));
    let waiting = waiting.try_take().map_err(|_| "still waiting").unwrap();
    assert!(matches!(waiting, Outcome::Superseded), "{waiting:?}");
    let late = pool.submit(Generation(1), echo(&[2]));
    let late = late.try_take().map_err(|_| "still waiting").unwrap();
    assert!(matches!(late, Outcome::Superseded), "{late:?}");
    let current = pool.submit(Generation(2), echo(&[3]));
    // The slow one, stopped after generation 2 started: never delivered.
    assert!(matches!(outcome(slow), Outcome::Superseded));
    assert_eq!(done(current).unwrap(), Blob::inline(vec![3]));
    // Starting an older or the same generation changes nothing.
    pool.advance(Generation(1));
    pool.advance(Generation(2));
    assert_eq!(pool.generation(), Generation(2));
    assert!(done(pool.submit(Generation(2), sleep(1))).is_ok());
}

#[test]
fn a_crash_fails_its_request_once_and_the_pool_goes_on() {
    let pool = pool(TEST_WORKER, 1);
    let before = pool.pids();
    let err =
        done(pool.submit(Generation(1), KernelRequest::debug(DebugOp::Segfault))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    assert!(err.cause().unwrap().contains("SIGSEGV"), "{err}");
    let after = wait_full(&pool, 1, &before.iter().copied().collect());
    assert_ne!(after, before);
    assert!(done(pool.submit(Generation(1), sleep(1))).is_ok());
}

#[test]
fn a_hung_request_times_out_and_the_pool_goes_on() {
    let pool = pool(TEST_WORKER, 1);
    let start = Instant::now();
    let err = done(pool.submit(
        Generation(1),
        sleep(30_000).with_timeout(Duration::from_millis(300)),
    ))
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerTimeout, "{err}");
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
    assert!(done(pool.submit(Generation(1), sleep(1))).is_ok());
}

#[test]
fn a_worker_whose_replies_cant_be_trusted_is_replaced() {
    // The test worker, told to misbehave, answers every request with the wrong kind of
    // reply: that request fails as a protocol error, the worker is replaced, and the next
    // request gets the right answer from the new one.
    let pool = pool(TEST_WORKER, 1);
    let before = pool.pids();
    done(pool.submit(
        Generation(1),
        KernelRequest::debug(DebugOp::Misbehave(Misbehavior::WrongReply)),
    ))
    .unwrap();
    let err = done(pool.submit(Generation(1), echo(&[7]))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ProtocolError, "{err}");
    wait_full(&pool, 1, &before.iter().copied().collect());
    let echoed = done(pool.submit(Generation(1), echo(&[8])));
    assert_eq!(echoed.unwrap(), Blob::inline(vec![8]));
}

#[test]
fn dropping_the_pool_stops_at_once_and_ends_every_request() {
    // Requests running (30 s sleeps) and waiting end without a result as soon as the pool
    // is dropped, and its workers are gone.
    let pool = pool(TEST_WORKER, 2);
    let pending: Vec<_> = (0..6)
        .map(|_| pool.submit(Generation(1), sleep(30_000)))
        .collect();
    std::thread::sleep(Duration::from_millis(200));
    let held = pool.pids();
    let start = Instant::now();
    drop(pool);
    assert_reaped(&held, "after the drop");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
    for p in pending {
        let err = done(p).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Internal, "{err}");
        assert_eq!(
            err.message(),
            "the kernel pool stopped before DebugSleep was done"
        );
    }
    wait_no_children("after the drop");
}

#[test]
fn a_pool_that_cant_start_starts_nothing() {
    let err = KernelPool::start_workers(Path::new("/nonexistent/silico-kernel-worker"), 3)
        .err()
        .unwrap();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    let err = KernelPool::start_workers(Path::new(WORKER), 0)
        .err()
        .unwrap();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    wait_no_children("after the failed starts");
}

#[test]
fn a_pool_whose_later_workers_cant_start_stops_its_first_ones() {
    // The first worker to start makes the executable unexecutable, then runs as the test
    // worker: later ones can't start, so the pool fails, and the workers that did start are
    // stopped and reaped. Enough slots that some start after the first has run. (A pool
    // whose slots all started before it ran starts; it's tried again, for up to a minute: on
    // a loaded machine the first worker can be slow to run, over and over.)
    const SLOTS: usize = 32;
    let start = std::time::Instant::now();
    for attempt in 0.. {
        if start.elapsed() > Duration::from_secs(60) {
            break;
        }
        let exe = scripts::script(
            &format!("unexecutable-after-first-{attempt}"),
            &format!("/bin/chmod -x \"$0\"\nexec '{TEST_WORKER}'"),
        );
        match KernelPool::start_workers(&exe, SLOTS) {
            Ok(pool) => drop(pool),
            Err(e) => {
                assert_eq!(e.code(), ErrorCode::WorkerCrashed, "{e}");
                wait_no_children("after a failed start");
                return;
            }
        }
    }
    panic!("every start succeeded; the scenario never ran");
}

#[test]
fn test_binary_imports_no_platform_math() {
    imports::assert_this_binary_imports_no_platform_math();
}

// The pool's clients (M4.2): each a handle with generations of its own, its requests served
// in turn with the others'. (Those that count the workers launched are adversarial_pool.rs's,
// with the launch log there.)

#[test]
fn a_pools_clients_are_numbered_from_one_and_share_its_identity() {
    let pool = pool(TEST_WORKER, 1);
    let clients: Vec<PoolClient> = (0..3).map(|_| pool.client()).collect();
    let numbers: Vec<u64> = clients.iter().map(|c| c.id().number()).collect();
    assert_eq!(numbers, [1, 2, 3]);
    drop(clients);
    // Never given again, even once retired.
    assert_eq!(pool.client().id().number(), 4);
    let client = pool.client();
    assert_eq!(client.identity(), Kernel::identity(&pool));
    // Each starts at generation 0, whatever the pool's own is.
    pool.advance(Generation(7));
    assert_eq!(pool.client().generation(), Generation(0));
    assert_eq!(client.generation(), Generation(0));
}

#[test]
fn a_clients_long_queue_never_starves_another() {
    // On a pool of one, `a` queues ten 200 ms sleeps, then `b` one echo. The pool takes the
    // clients in turn, so `b`'s echo runs after at most the sleep already running: second,
    // or third if this thread wakes a whole sleep late to look; never eleventh, after all of
    // `a`'s (as one queue for the pool, first come first served, ran it).
    let pool = pool(TEST_WORKER, 1);
    let (a, b) = (pool.client(), pool.client());
    let sleeps: Vec<_> = (0..10)
        .map(|_| a.submit(Generation(0), sleep(200)))
        .collect();
    let echoed = b.submit(Generation(0), echo(&[1]));
    assert_eq!(done(echoed).unwrap(), Blob::inline(vec![1]));
    // Outcomes are delivered in the order the requests ran, so the sleeps done now are at
    // least those before the echo.
    let mut before = 0;
    let mut rest = Vec::new();
    for pending in sleeps {
        match pending.try_take() {
            Ok(Outcome::Done(Ok(()))) => before += 1,
            Ok(other) => panic!("a sleep came to {other:?}"),
            Err(pending) => rest.push(pending),
        }
    }
    let place = before + 1;
    assert!(
        (2..=3).contains(&place),
        "b's echo ran {place}th of 11, after {before} of a's sleeps"
    );
    for pending in rest {
        done(pending).unwrap();
    }
}

#[test]
fn many_clients_submitting_and_advancing_never_get_a_failure_or_anothers_supersession() {
    // `many_submitters_never_get_a_failure_from_stopped_work`, with four clients: three
    // advancing on their own, each every 0 to 300 µs at its own cadence, and one that never
    // advances; two threads submitting for each at its current generation, on a pool of
    // eight that stops every superseded request at once. No request fails; one is superseded
    // only once its own client has advanced past it; and the client that never advances has
    // none superseded, though the others' advances stop workers all along.
    let pool = pool(TEST_WORKER, 8);
    pool.set_stop_after(Duration::ZERO);
    let clients: Vec<PoolClient> = (0..4).map(|_| pool.client()).collect();
    let stop = std::sync::atomic::AtomicBool::new(false);
    let deadline = Instant::now() + Duration::from_secs(5);
    // Per submitter: its client, its failures, its supersessions its client hadn't earned,
    // and how many it had superseded.
    type Tally = (usize, Vec<String>, Vec<String>, u32);
    let (tallies, workers): (Vec<Tally>, BTreeSet<u32>) = std::thread::scope(|scope| {
        let advancers: Vec<_> = clients[..3]
            .iter()
            .zip([37u64, 53, 71])
            .map(|(client, step)| {
                let (pool, stop) = (&pool, &stop);
                scope.spawn(move || {
                    let mut workers = BTreeSet::new();
                    let mut generation = 0u64;
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        generation += 1;
                        client.advance(Generation(generation));
                        workers.extend(pool.pids());
                        spin(Duration::from_micros((generation * step) % 300));
                    }
                    workers
                })
            })
            .collect();
        let submitters: Vec<_> = (0..8usize)
            .map(|t| {
                let c = t % 4;
                let client = &clients[c];
                scope.spawn(move || {
                    let (mut failures, mut unearned, mut superseded) = (Vec::new(), Vec::new(), 0);
                    while Instant::now() < deadline {
                        let batch: Vec<_> = (0..16)
                            .map(|_| {
                                let g = client.generation();
                                (g, client.submit(g, echo(&[t as u8])))
                            })
                            .collect();
                        for (g, pending) in batch {
                            match outcome(pending) {
                                Outcome::Done(Ok(_)) => {}
                                Outcome::Done(Err(e)) => failures.push(e.to_string()),
                                Outcome::Superseded => {
                                    superseded += 1;
                                    // A client's generation only grows: newer now if it was
                                    // when the request was superseded.
                                    let now = client.generation();
                                    if now <= g {
                                        unearned.push(format!(
                                            "client {c}: superseded at {g:?}, its generation {now:?}"
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    (c, failures, unearned, superseded)
                })
            })
            .collect();
        let tallies = submitters.into_iter().map(|s| s.join().unwrap()).collect();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let workers = advancers
            .into_iter()
            .flat_map(|a| a.join().unwrap())
            .collect();
        (tallies, workers)
    });
    let failures: Vec<&String> = tallies.iter().flat_map(|t| &t.1).collect();
    let unearned: Vec<&String> = tallies.iter().flat_map(|t| &t.2).collect();
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(unearned.is_empty(), "{unearned:#?}");
    let superseded = |c: usize| -> u32 { tallies.iter().filter(|t| t.0 == c).map(|t| t.3).sum() };
    assert_eq!(superseded(3), 0, "the client that never advanced");
    assert!(
        (0..3).map(superseded).sum::<u32>() > 0,
        "no advance ever superseded a request"
    );
    assert!(workers.len() > 8, "no worker was ever stopped");
    wait_full(&pool, 8, &BTreeSet::new());
}

#[test]
fn a_client_that_outlives_its_pool_ends_its_requests_without_a_result() {
    // A client's request running as its pool is dropped, and one it submits after, end
    // without a result, saying the pool stopped and to take a client of a running one; no
    // worker or thread is left behind.
    let pool = pool(TEST_WORKER, 1);
    let client = pool.client();
    let running = client.submit(Generation(0), sleep(30_000));
    std::thread::sleep(Duration::from_millis(100));
    drop(pool);
    let late = client.submit(Generation(0), echo(&[1]));
    let stopped = |name: &str, outcome: Result<(), silico_core::Error>| {
        let e = outcome.unwrap_err();
        assert_eq!(e.code(), ErrorCode::Internal, "{e}");
        assert_eq!(
            e.message(),
            format!("the kernel pool stopped before {name} was done")
        );
        assert_eq!(
            e.suggestion(),
            Some("take a client of a running kernel pool")
        );
    };
    stopped("DebugSleep", done(running));
    stopped("Echo", done(late).map(drop));
    client.advance(Generation(1));
    assert_eq!(client.generation(), Generation(1));
    drop(client);
    wait_no_children("after the pool and its client");
}

#[test]
fn an_idle_pool_still_tends_its_worker_after_a_clients_waiting_requests_are_superseded() {
    // A client's advance takes its waiting request out of the count of those waiting, so
    // the idle slot, finding none, tends its worker: one killed from outside is replaced.
    // (A count left at one keeps the slot waiting for work that isn't there.)
    let pool = pool(TEST_WORKER, 1);
    let a = pool.client();
    let running = a.submit(Generation(0), sleep(300));
    std::thread::sleep(Duration::from_millis(100));
    let waiting = a.submit(Generation(0), echo(&[1]));
    a.advance(Generation(1));
    assert!(matches!(waiting.try_take(), Ok(Outcome::Superseded)));
    assert!(matches!(outcome(running), Outcome::Superseded));
    let before = wait_full(&pool, 1, &BTreeSet::new());
    signal("9", before[0]);
    wait_full(&pool, 1, &BTreeSet::from([before[0]]));
}

#[test]
fn a_clients_stale_submit_is_superseded_at_once() {
    // `a` at generation 2 submits for its generation 1, while `b`'s sleep holds the pool's
    // one worker: superseded at once, by `a`'s own generation (the pool's own is still 0),
    // and `b`'s sleep runs on.
    let pool = pool(TEST_WORKER, 1);
    let (a, b) = (pool.client(), pool.client());
    let busy = b.submit(Generation(0), sleep(300));
    std::thread::sleep(Duration::from_millis(100));
    a.advance(Generation(2));
    let late = a.submit(Generation(1), echo(&[1]));
    let late = late.try_take().map_err(|_| "still waiting").unwrap();
    assert!(matches!(late, Outcome::Superseded), "{late:?}");
    assert!(matches!(outcome(busy), Outcome::Done(Ok(()))));
}

#[test]
fn a_worker_that_exited_is_replaced_before_its_next_request_though_its_socket_is_open() {
    // The script's first run says it started, waits for its request, and exits; a process it
    // started holds its socket open, so the pool never sees it hang up, and sends the
    // request's answer only once the worker has exited. The second request, queued behind
    // the first, finds the slot's worker exited but not hung up: it goes to a new worker
    // (the script's later runs are the test worker), not the dead one, where it would wait
    // out its timeout unanswered.
    let dir = marks_dir("exited-open");
    let (ran, reply) = (dir.join("ran"), dir.join("reply"));
    // The first request's answer, as the worker would send it: request 0, generation 0, done.
    let mut answer = Vec::new();
    silico_protocol::write_message(
        &mut answer,
        &Response {
            id: RequestId(0),
            generation: Generation(0),
            result: Ok(Reply::Done),
        },
    )
    .unwrap();
    std::fs::write(&reply, answer).unwrap();
    let exe = scripts::script(
        "exited-open",
        &format!(
            "if [ -e '{ran}' ]; then exec '{TEST_WORKER}'; fi
: > '{ran}'
printf '\\0\\0\\0\\0' >&3
dd bs=1 count=1 <&3 >/dev/null 2>&1
me=$$
(
  j=0
  while ps -o stat= -p \"$me\" | grep -q '^ *[^Z ]' && [ \"$j\" -lt 6000 ]; do sleep 0.01; j=$((j + 1)); done
  cat '{reply}' >&3
  i=0
  while [ ! -e '{release}' ] && [ \"$i\" -lt 2400 ]; do sleep 0.05; i=$((i + 1)); done
) </dev/null >/dev/null 2>&1 &
exit 0",
            ran = ran.display(),
            reply = reply.display(),
            release = dir.join("release").display(),
        ),
    );
    // The process the script leaves is let go however the test ends.
    let _released = Released(&dir);
    let pool = KernelPool::start_workers(&exe, 1).unwrap();
    let first_pids = pool.pids();
    let first = pool.submit(Generation(0), sleep(0));
    // Under PATIENCE, so a request sent to the dead worker fails by its own timeout.
    let second = pool.submit(
        Generation(0),
        sleep(0).with_timeout(Duration::from_secs(30)),
    );
    done(first).unwrap();
    done(second).unwrap();
    assert_ne!(
        pool.pids(),
        first_pids,
        "the second request went to the exited worker"
    );
}
