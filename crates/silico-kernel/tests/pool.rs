//! M1.6: the kernel worker pool (silico_core::pool). Results through the pool are the same
//! as one worker's, request for request; workers killed mid-burst fail only their own
//! requests and are replaced; results for superseded generations are never delivered; and
//! no worker process outlives the pool or is left a zombie.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/imports.rs"]
mod imports;
#[path = "common/kernel.rs"]
mod kernel;
#[path = "common/pool.rs"]
mod pool;
#[path = "common/scripts.rs"]
mod scripts;

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

use kernel::kernel;
use pool::{
    TEST_WORKER, WORKER, assert_reaped, children, done, echo, kill9_and_wait, outcome, pool,
    signal, sleep, spin, wait_full, wait_no_children, wait_no_children_besides,
};
use silico_core::ErrorCode;
use silico_core::kernel::{
    Blob, BooleanOp, BuiltData, ChamferSize, KernelRequest, Mesh, Primitive, Profile, ShapeData,
    ShapeSummary,
};
use silico_core::pool::{KernelPool, Outcome, Pending, STOP_AFTER};
use silico_protocol::{DebugOp, Generation, Misbehavior};

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
    // Four sleeps of a second on four workers end in about a second: two at a time would
    // take two.
    let pool = pool(TEST_WORKER, 4);
    let start = Instant::now();
    let sleeps: Vec<_> = (0..4)
        .map(|_| pool.submit(Generation(1), sleep(1000)))
        .collect();
    for pending in sleeps {
        done(pending).unwrap();
    }
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(1900), "{elapsed:?}");
}

#[test]
fn the_pool_finds_its_worker_and_has_one_per_core() {
    // As KernelWorker::start does: next to the test binary's directory.
    let pool = KernelPool::start().unwrap();
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    assert_eq!(pool.size(), cores.min(silico_core::pool::MAX_WORKERS));
    assert_eq!(pool.pids().len(), pool.size());
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
    let pool = pool(TEST_WORKER, 1);
    let start = Instant::now();
    let pending = pool
        .submit(Generation(1), sleep(500))
        .wait_timeout(Duration::from_millis(50))
        .expect_err("done in 50 ms");
    assert!(
        start.elapsed() < Duration::from_millis(400),
        "{:?}",
        start.elapsed()
    );
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
    // Four workers busy on 3 s sleeps for generation 1. Generation 2 starts: the sleeps are
    // stopped (their workers killed and replaced, about 30 ms each), so generation 2's first
    // request runs at once, not after nearly 3 s.
    let pool = pool(TEST_WORKER, 4);
    let before: BTreeSet<u32> = pool.pids().into_iter().collect();
    let stale: Vec<_> = (0..4)
        .map(|_| pool.submit(Generation(1), sleep(3_000)))
        .collect();
    std::thread::sleep(Duration::from_millis(200));
    let start = Instant::now();
    pool.advance(Generation(2));
    let current = pool.submit(Generation(2), echo(&[1])).wait();
    let waited = start.elapsed();
    // Every one of them stopped, not only the first: none runs on to its 3 s.
    for pending in stale {
        assert!(matches!(outcome(pending), Outcome::Superseded));
    }
    let all_stopped = start.elapsed();
    assert!(matches!(current, Outcome::Done(Ok(_))), "{current:?}");
    assert!(
        waited < Duration::from_millis(500),
        "waited {waited:?} behind superseded work"
    );
    assert!(
        all_stopped < Duration::from_secs(1),
        "stale work ran on for {all_stopped:?}"
    );
    // And every worker was replaced.
    wait_full(&pool, 4, &before);
}

#[test]
fn superseded_work_too_young_to_stop_at_advance_is_stopped_once_it_has_run_stop_after() {
    // Generation 2 starts 10 ms into 3 s sleeps of generation 1, too soon to stop them. They
    // aren't left to run: each is stopped once it has run STOP_AFTER, so generation 2's first
    // request waits that and a worker start, not nearly 3 s. On a pool of one, and of four.
    for size in [1, 4] {
        let pool = pool(TEST_WORKER, size);
        let before: BTreeSet<u32> = pool.pids().into_iter().collect();
        let stale: Vec<_> = (0..size)
            .map(|_| pool.submit(Generation(1), sleep(3_000)))
            .collect();
        std::thread::sleep(Duration::from_millis(10));
        let start = Instant::now();
        pool.advance(Generation(2));
        let current = pool.submit(Generation(2), echo(&[1])).wait();
        let waited = start.elapsed();
        for pending in stale {
            assert!(matches!(outcome(pending), Outcome::Superseded));
        }
        let all_stopped = start.elapsed();
        assert!(matches!(current, Outcome::Done(Ok(_))), "{current:?}");
        assert!(
            waited < Duration::from_millis(500),
            "pool of {size}: waited {waited:?} behind superseded work"
        );
        assert!(
            all_stopped < Duration::from_secs(1),
            "pool of {size}: stale work ran on for {all_stopped:?}"
        );
        wait_full(&pool, size, &before);
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
    // its killed worker in place, found by the review.)
    let pool = pool(TEST_WORKER, 1);
    pool.set_stop_after(Duration::ZERO);
    let mut failures = Vec::new();
    let mut workers = BTreeSet::new();
    for attempt in 0..300u64 {
        let g = pool.generation().0;
        drop(pool.submit(Generation(g), sleep(200)));
        spin(Duration::from_micros(attempt % 100));
        pool.advance(Generation(g + 1));
        match outcome(pool.submit(Generation(g + 1), echo(&[1]))) {
            Outcome::Done(Ok(_)) => {}
            other => failures.push((attempt, format!("{other:?}"))),
        }
        workers.extend(pool.pids());
    }
    assert!(failures.is_empty(), "{failures:?}");
    assert!(workers.len() > 1, "no worker was ever stopped");
}

#[test]
fn a_worker_killed_as_its_request_ends_is_never_given_another() {
    // `advance` swept across the end of a quick request (every superseded request stopped at
    // once): one that lands after the reply but before the slot is idle kills a worker with
    // nothing left to do, and the slot must replace it, not give it the next request.
    let pool = pool(TEST_WORKER, 1);
    pool.set_stop_after(Duration::ZERO);
    let mut failures = Vec::new();
    let mut workers = BTreeSet::new();
    for attempt in 0..600u64 {
        let g = pool.generation().0;
        let first = pool.submit(Generation(g), echo(&[0]));
        spin(Duration::from_micros(attempt % 300));
        pool.advance(Generation(g + 1));
        let _ = outcome(first);
        match outcome(pool.submit(Generation(g + 1), echo(&[1]))) {
            Outcome::Done(Ok(_)) => {}
            other => failures.push((attempt, format!("{other:?}"))),
        }
        workers.extend(pool.pids());
    }
    assert!(failures.is_empty(), "{failures:?}");
    assert!(workers.len() > 1, "no worker was ever stopped");
}

#[test]
fn a_running_superseded_request_that_ends_within_stop_after_finishes_on_its_worker() {
    // Stopping a request costs a new worker (about 30 ms), more than letting a short one
    // finish: a superseded request, running, that ends within STOP_AFTER is left to, its
    // result discarded, and its worker kept. A short request is superseded soon after it
    // starts, ATTEMPTS times. One not yet running by then is dropped instead, and one a
    // loaded machine makes take past STOP_AFTER is rightly stopped; but none is stopped
    // sooner, and at least one must have run to its end on its worker.
    const REQUEST_MILLIS: u64 = 25;
    const SUPERSEDED_AFTER: Duration = Duration::from_millis(8);
    const ATTEMPTS: u64 = 10;
    let pool = pool(TEST_WORKER, 1);
    done(pool.submit(Generation(0), echo(b"warm"))).unwrap();
    let mut finished_on_its_worker = 0;
    for g in 1..=ATTEMPTS {
        let before = pool.pids();
        let start = Instant::now();
        let young = pool.submit(Generation(g), sleep(REQUEST_MILLIS));
        std::thread::sleep(SUPERSEDED_AFTER);
        pool.advance(Generation(g + 1));
        assert!(matches!(outcome(young), Outcome::Superseded));
        let elapsed = start.elapsed();
        // Once another request has been through the slot, a stopped worker has been
        // replaced.
        assert_eq!(
            done(pool.submit(Generation(g + 1), echo(&[1]))).unwrap(),
            Blob::inline(vec![1])
        );
        if pool.pids() == before {
            if elapsed >= Duration::from_millis(REQUEST_MILLIS) {
                finished_on_its_worker += 1;
            }
        } else {
            assert!(
                elapsed >= STOP_AFTER,
                "attempt {g}: its worker was stopped {elapsed:?} in, before STOP_AFTER"
            );
        }
    }
    assert!(
        finished_on_its_worker > 0,
        "no superseded request ran to its end on its worker"
    );
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
    // With a stop-after of a second, a request superseded 500 ms in is stopped at 1 s, not
    // 1.5 s.
    let pool = pool(TEST_WORKER, 1);
    pool.set_stop_after(Duration::from_secs(1));
    let before: BTreeSet<u32> = pool.pids().into_iter().collect();
    let start = Instant::now();
    let stale = pool.submit(Generation(1), sleep(5_000));
    std::thread::sleep(Duration::from_millis(500));
    pool.advance(Generation(2));
    assert!(matches!(outcome(stale), Outcome::Superseded));
    let ended = start.elapsed();
    assert!(
        ended >= Duration::from_millis(950) && ended < Duration::from_millis(1_300),
        "{ended:?}"
    );
    wait_full(&pool, 1, &before);
}

#[test]
fn each_superseded_request_is_stopped_when_it_is_due() {
    // Four superseded requests, started 400 ms apart, with a stop-after of 1.5 s: each is
    // stopped at its own 1.5 s, none kept waiting for another's, whichever slots they ran
    // on. (The advance comes after the last starts and before the first is due.)
    const COUNT: u32 = 4;
    let stop_after = Duration::from_millis(1_500);
    let stagger = Duration::from_millis(400);
    let advance_at = stagger * (COUNT - 1) + Duration::from_millis(100);
    let pool = pool(TEST_WORKER, COUNT as usize);
    pool.set_stop_after(stop_after);
    let start = Instant::now();
    let mut stale = Vec::new();
    for i in 0..COUNT {
        std::thread::sleep((stagger * i).saturating_sub(start.elapsed()));
        stale.push(pool.submit(Generation(1), sleep(10_000)));
    }
    assert!(advance_at < stop_after);
    std::thread::sleep(advance_at.saturating_sub(start.elapsed()));
    pool.advance(Generation(2));
    for (i, pending) in (0..COUNT).zip(stale) {
        assert!(matches!(outcome(pending), Outcome::Superseded));
        let ended = start.elapsed();
        // Never sooner: each started no earlier than its stagger.
        let due = stagger * i + stop_after;
        assert!(
            ended >= due && ended < due + Duration::from_millis(300),
            "request {i}: ended at {ended:?}, due at {due:?}"
        );
    }
}

#[test]
fn lowering_the_stop_after_stops_superseded_work_already_running() {
    // A superseded request left running under a stop-after of ten seconds is stopped once
    // the stop-after is lowered to zero, not left to its 3 s.
    let pool = pool(TEST_WORKER, 1);
    pool.set_stop_after(Duration::from_secs(10));
    let start = Instant::now();
    let stale = pool.submit(Generation(1), sleep(3_000));
    std::thread::sleep(Duration::from_millis(100));
    pool.advance(Generation(2));
    std::thread::sleep(Duration::from_millis(100));
    pool.set_stop_after(Duration::ZERO);
    assert!(matches!(outcome(stale), Outcome::Superseded));
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
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
