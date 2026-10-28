//! break-it: the kernel worker pool (silico_core::pool) and what it builds on
//! (KernelRequest::run_on, WorkerKiller, Worker::has_exited). Workers killed, stopped, and
//! deleted from outside; workers that answer more than they're asked; the pool dropped
//! while a slot is replacing its worker; generations advanced while results are on their
//! way; sizes no machine has; and the pool's own resources (threads, descriptors, child
//! processes) across many replacements.
//!
//! These tests watch the test process's own children, threads, and descriptors, so like
//! `pool.rs` they assume one test per process (nextest, which `pixi run test` uses).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../silico-core/tests/common/lateness.rs"]
mod lateness;
#[path = "common/launches.rs"]
mod launches;
#[path = "common/pool.rs"]
mod pool;
#[path = "common/process.rs"]
mod process;
#[path = "../../silico-core/tests/common/share.rs"]
mod share;
#[path = "common/wait.rs"]
mod wait;

use crate::wait::{UNREACHED, WAIT, promptly};
use lateness::{Lateness, allowance};
use share::Share;
use std::collections::BTreeSet;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use launches::{
    Decision, Decisions, HANG, assert_gaps_exceed, gaps, launch_logger, launch_times, launches,
    log_launch, wait_launches, wait_launches_after,
};
use pool::{
    PATIENCE, TEST_WORKER, WORKER, assert_reaped, children, done, echo, kill9_and_wait, outcome,
    pool, signal, sleep, spin, state, wait_full, wait_full_by, wait_no_children,
};
use process::{open_fds, scratch_dir, threads};
use silico_core::ErrorCode;
use silico_core::kernel::{Blob, Kernel, KernelRequest, Primitive};
use silico_core::pool::{IDLE_CHECK, KernelPool, Outcome};
use silico_core::worker::Worker;
use silico_protocol::{DebugOp, Failure, Generation, Op, Refusal, Reply, Response, write_message};

fn write_executable(path: &Path, text: &str) {
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(text.as_bytes()).unwrap();
    drop(file);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

// ---------------------------------------------------------------------------------------
// A fake worker: this test binary, re-run as a worker by a wrapper script (a worker gets
// no environment of its own, so the script sets the mode). It answers as the production
// worker does, except as its mode says.

const FAKE_ENV: &str = "SILICO_ADVERSARIAL_POOL_FAKE_WORKER";

/// Not a test of its own: when run by [`fake_worker`]'s script, this test binary serves as
/// a kernel worker. Run normally, it does nothing.
#[test]
fn fake_worker_entry() {
    let Ok(mode) = std::env::var(FAKE_ENV) else {
        return;
    };
    let _ = silico_kernel::worker::run(move |id, generation, op, socket| {
        fake_handle(&mode, id, generation, op, socket)
    });
    std::process::exit(0);
}

/// The fake worker's answer to `op`, in `mode`.
fn fake_handle(
    mode: &str,
    id: silico_protocol::RequestId,
    generation: Generation,
    op: Op,
    socket: &mut dyn std::io::Write,
) -> Result<Reply, Failure> {
    match (mode, op) {
        // Every echo is answered twice: the second answer is unasked for, and waits on the
        // socket until the core next reads from this worker.
        ("echo-twice", Op::Echo { data }) => {
            let extra = Response {
                id,
                generation,
                result: Ok(Reply::Echo { data: data.clone() }),
            };
            write_message(socket, &extra).map_err(|e| {
                Failure::new(Refusal::Internal {
                    what: e.to_string(),
                })
            })?;
            Ok(Reply::Echo { data })
        }
        // An echo of `hold:DIR` is held: the worker writes its process ID to DIR/started,
        // then answers only once DIR/release exists. Any other echo is answered at once.
        ("hold", Op::Echo { data }) if data.as_bytes().starts_with(HOLD) => {
            let dir = Path::new(std::str::from_utf8(&data.as_bytes()[HOLD.len()..]).unwrap());
            let started = dir.join("started");
            let partial = dir.join("started.partial");
            std::fs::write(&partial, std::process::id().to_string()).unwrap();
            std::fs::rename(&partial, &started).unwrap();
            while !dir.join("release").exists() {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(Reply::Echo { data })
        }
        (_, op) => silico_kernel::worker::handle(op),
    }
}

/// What an echo held by the fake worker's `hold` mode begins with.
const HOLD: &[u8] = b"hold:";

/// A worker executable that is this test binary in `mode` (see [`fake_worker_entry`]).
fn fake_worker(mode: &str) -> PathBuf {
    let dir = scratch_dir(mode);
    let script = dir.join("fake-worker");
    let me = std::env::current_exe().unwrap();
    let run = "--exact fake_worker_entry --nocapture --test-threads=1";
    write_executable(
        &script,
        &format!(
            "#!/bin/sh\n{FAKE_ENV}={mode} exec '{}' {run}\n",
            me.display()
        ),
    );
    script
}

// ---------------------------------------------------------------------------------------
// Shutdown races

#[test]
fn dropping_the_pool_while_a_slot_replaces_its_dead_worker_stops_at_once() {
    // A pool of one whose worker was killed while idle. A request comes; its slot takes it,
    // finds the worker dead, and starts a new one. A drop that lands while the new worker
    // is being started finds no worker in the slot to kill; the slot checks for a stop once
    // the new worker is listed, so the drop still ends at once rather than when the
    // request (a 30 s sleep) would. Swept over the moment of the drop, so attempts land in
    // that window.
    const SLEEP: Duration = Duration::from_secs(30);
    for attempt in 0..120u32 {
        let pool = pool(TEST_WORKER, 1);
        let pid = pool.pids()[0];
        kill9_and_wait(pid);
        let pending = pool.submit(Generation(1), sleep(SLEEP.as_millis() as u64));
        spin(Duration::from_micros(u64::from(attempt % 60) * 100));
        let mut held = pool.pids();
        held.push(pid);
        let start = Instant::now();
        drop(pool);
        let took = start.elapsed();
        assert_reaped(&held, &format!("attempt {attempt}"));
        // The drop may land while the slot is starting the replacement worker, before its
        // killer is listed; the slot checks for a stop once it has, so the new worker never
        // runs the request (it did, and the drop waited for it). A drop that waits for it
        // can't end before the sleep does, which began after the drop did: so the bound is
        // the sleep, which a drop that doesn't wait ends far within (milliseconds), however
        // loaded the machine.
        assert!(
            took < SLEEP,
            "attempt {attempt}: dropping the pool took {took:?}, as long as the request"
        );
        let err = done(pending).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Internal, "{err}");
        wait_no_children(&format!("attempt {attempt}"));
    }
}

// ---------------------------------------------------------------------------------------
// Generations

#[test]
fn a_result_still_pending_when_advance_returns_is_never_delivered() {
    // `advance` promises that results of requests still running are discarded (a quick
    // echo is younger than STOP_AFTER, so it's left to finish rather than stopped): once it
    // returns, a request whose outcome hasn't come must end superseded. Here a quick echo
    // races `advance` many times, the advance swept across the echo's round trip. It
    // catches a missing generation check; the narrower window of a result sent after the
    // check (the pool decides and sends under the lock `advance` takes) is too short for a
    // race to land in, and is tested directly in the pool's own unit tests.
    let pool = pool(WORKER, 1);
    // Warm up, and measure a round trip.
    let start = Instant::now();
    for _ in 0..200 {
        done(pool.submit(Generation(0), echo(&[0]))).unwrap();
    }
    let trip = start.elapsed() / 200;
    let mut generation = 0u64;
    let mut pending_after_advance = 0u32;
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut i = 0u64;
    while Instant::now() < deadline {
        let p = pool.submit(Generation(generation), echo(&[1, 2, 3]));
        // From 0 to twice the round trip, in 1/64 steps.
        spin(trip * u32::try_from(i % 128).unwrap() / 64);
        generation += 1;
        pool.advance(Generation(generation));
        i += 1;
        if let Err(p) = p.try_take() {
            pending_after_advance += 1;
            match outcome(p) {
                Outcome::Superseded => {}
                Outcome::Done(result) => panic!(
                    "attempt {i}: a request of generation {} still pending after \
                     advance({generation}) returned was delivered: {result:?}",
                    generation - 1
                ),
            }
        }
    }
    assert!(pending_after_advance > 0, "no attempt raced the advance");
}

#[test]
fn generations_at_the_ends_of_the_range() {
    let pool = pool(WORKER, 1);
    // A request for a generation the pool hasn't started runs.
    assert_eq!(
        done(pool.submit(Generation(u64::MAX), echo(&[9]))).unwrap(),
        Blob::inline(vec![9])
    );
    pool.advance(Generation(u64::MAX));
    assert_eq!(pool.generation(), Generation(u64::MAX));
    // Nothing is newer than the last generation; everything else is superseded at once.
    for g in [0, 1, u64::MAX - 1] {
        let p = pool.submit(Generation(g), echo(&[g as u8]));
        assert!(
            matches!(p.try_take().ok(), Some(Outcome::Superseded)),
            "{g}"
        );
    }
    assert_eq!(
        done(pool.submit(Generation(u64::MAX), echo(&[7]))).unwrap(),
        Blob::inline(vec![7])
    );
    // Going back changes nothing.
    pool.advance(Generation(0));
    assert_eq!(pool.generation(), Generation(u64::MAX));
}

#[test]
fn many_submitters_and_advancers_never_cross_results() {
    // Eight threads submit echoes of their own bytes, at generations they read from the
    // pool, while two threads advance it. Every request gets its own bytes back, or is
    // superseded; one whose generation was never passed is never superseded; and every
    // request ends.
    let pool = pool(WORKER, 3);
    let advances = 400u64;
    std::thread::scope(|scope| {
        let mut submitters = Vec::new();
        for t in 0..8u8 {
            let pool = &pool;
            submitters.push(scope.spawn(move || {
                let mut sent = Vec::new();
                for n in 0..400u32 {
                    let bytes: Vec<u8> = [t].into_iter().chain(n.to_le_bytes()).collect();
                    // Some for the current generation, some for one ahead of it.
                    let g = pool.generation().0 + u64::from(n % 3 == 0);
                    sent.push((g, bytes.clone(), pool.submit(Generation(g), echo(&bytes))));
                }
                sent
            }));
        }
        let advancers: Vec<_> = (0..2)
            .map(|_| {
                let pool = &pool;
                scope.spawn(move || {
                    for _ in 0..advances / 2 {
                        let next = pool.generation().0 + 1;
                        pool.advance(Generation(next));
                        std::thread::sleep(Duration::from_micros(200));
                    }
                })
            })
            .collect();
        let sent: Vec<_> = submitters
            .into_iter()
            .flat_map(|s| s.join().unwrap())
            .collect();
        for a in advancers {
            a.join().unwrap();
        }
        // Every advancer has finished: no generation newer than this was ever started.
        let last = pool.generation();
        let mut superseded = 0;
        for (g, bytes, p) in sent {
            match outcome(p) {
                Outcome::Done(result) => assert_eq!(result.unwrap(), Blob::inline(bytes)),
                Outcome::Superseded => {
                    assert!(
                        g < last.0,
                        "generation {g} superseded, but {last:?} is the newest"
                    );
                    superseded += 1;
                }
            }
        }
        assert!(superseded > 0);
    });
    // After all of it, a request of the final generation runs.
    let last = pool.generation();
    assert!(last.0 >= 1 && last.0 <= advances, "{last:?}");
    assert_eq!(
        done(pool.submit(last, echo(&[42]))).unwrap(),
        Blob::inline(vec![42])
    );
}

// ---------------------------------------------------------------------------------------
// Sizes and executables

#[test]
fn a_pool_of_usize_max_workers_is_refused() {
    // A size from a config file or a CLI flag must be refused as InvalidInput, not crash
    // the app (`vec![None; size]` for the slots' killers panicked with "capacity overflow"
    // before anything checked the size).
    let result =
        std::panic::catch_unwind(|| KernelPool::start_workers(Path::new(WORKER), usize::MAX).err());
    let err = result
        .unwrap_or_else(|panic| {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_default();
            panic!("start_workers(usize::MAX) panicked: {message}")
        })
        .expect("a pool of usize::MAX workers started");
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert_eq!(children(), Vec::new());
}

#[test]
fn a_pool_of_a_program_that_isnt_a_worker() {
    // /usr/bin/true starts and exits at once. Requests fail as crashes naming how it
    // ended; the pool doesn't spin replacing it; dropping it leaves nothing behind.
    let pool = KernelPool::start_workers(Path::new("/usr/bin/true"), 2);
    let pool = match pool {
        // Refusing it at the start would be fine too.
        Err(e) => {
            assert_eq!(e.code(), ErrorCode::WorkerCrashed, "{e}");
            return;
        }
        Ok(pool) => pool,
    };
    for _ in 0..4 {
        let err = done(pool.submit(Generation(1), echo(&[1]))).unwrap_err();
        assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
        assert!(
            err.cause()
                .is_some_and(|c| c.contains("exited with code 0")),
            "{err}"
        );
    }
    let held = pool.pids();
    drop(pool);
    assert_reaped(&held, "after the drop");
    wait_no_children("after the drop");
}

#[test]
fn a_worker_executable_deleted_and_restored_under_a_running_pool() {
    // The worker binary is rebuilt (deleted, then written again) while a pool runs, and
    // its workers die meanwhile. Requests fail with a structured error naming the
    // executable while it's missing, never hang, and the pool comes back to full size once
    // the binary is back.
    // A link to the worker, not a copy: macOS holds a new executable's first launches while
    // it assesses it, which here would only add delay.
    let dir = scratch_dir("deleted");
    let exe = dir.join("silico-kernel-test-worker");
    std::os::unix::fs::symlink(TEST_WORKER, &exe).unwrap();
    let pool = KernelPool::start_workers(&exe, 2).unwrap();
    std::fs::remove_file(&exe).unwrap();
    let before = pool.pids();
    for &pid in &before {
        kill9_and_wait(pid);
    }
    for _ in 0..6 {
        let err = done(pool.submit(Generation(1), sleep(1))).unwrap_err();
        assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
        assert!(err.message().contains(&*exe.to_string_lossy()), "{err}");
        assert!(err.cause().is_some(), "{err}");
    }
    // Every slot ends up with no worker, but not necessarily by now: a slot drops its
    // dead worker when it takes a request or at its idle check (IDLE_CHECK), and the six
    // requests may all have gone to one slot (on Linux they did, now and then, failing a
    // gate under load). So wait for it: within an idle check or so (IDLE_CHECK); HANG only
    // guards against a hang. Meanwhile only the killed workers may be held: none can
    // start while the executable is gone.
    let deadline = Instant::now() + HANG;
    loop {
        let held = pool.pids();
        assert!(
            held.iter().all(|pid| before.contains(pid)),
            "a worker started while its executable was gone: {held:?}, before {before:?}"
        );
        if held.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the pool still holds its killed workers {held:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    std::os::unix::fs::symlink(TEST_WORKER, &exe).unwrap();
    assert!(done(pool.submit(Generation(1), sleep(1))).is_ok());
    // Full again, whenever the slots' waits let them start: that they start at once is
    // `a_pool_whose_binary_comes_back_after_many_failed_starts_fills_at_once`'s to show.
    let held = wait_full_by(
        &pool,
        2,
        &before.iter().copied().collect(),
        Instant::now() + HANG,
    );
    drop(pool);
    assert_reaped(&held, "after the drop");
    wait_no_children("after the drop");
}

/// A worker executable, build `n` of it: a script that runs the test worker, its bytes
/// (and so its identity) its build's own.
fn build(n: u32) -> String {
    format!("#!/bin/sh\n# build {n}\nexec '{TEST_WORKER}'\n")
}

/// Puts `text` in place at `exe` as a build tool does: written beside it, renamed over it.
fn put_in_place(exe: &Path, text: &str) {
    let beside = exe.with_extension("new");
    write_executable(&beside, text);
    std::fs::rename(&beside, exe).unwrap();
}

/// Whether `err` says the pool's worker executable changed, naming it, and to restart.
fn says_changed(err: &silico_core::Error, exe: &Path) -> bool {
    err.code() == ErrorCode::WorkerUnavailable
        && err.message().contains("changed since it started")
        && err.message().contains(&*exe.to_string_lossy())
        && err.suggestion().is_some_and(|s| s.contains("restart"))
}

#[test]
fn a_worker_executable_replaced_by_another_build_isnt_started_under_the_pools_identity() {
    // Rebuilt to other bytes while a pool runs (a build, an update): the workers running go
    // on under the pool's identity, but none is started from the other build, which would
    // answer under it. Each start fails saying the executable changed, never hangs, and the
    // pool's identity stays its own build's. With that build back, workers start again.
    // One worker, which answers first (so it has started, and been checked, as build 1)
    // before the other build is put in place.
    let dir = scratch_dir("replaced");
    let exe = dir.join("worker");
    write_executable(&exe, &build(1));
    let pool = KernelPool::start_workers(&exe, 1).unwrap();
    assert!(done(pool.submit(Generation(1), sleep(1))).is_ok());
    put_in_place(&exe, &build(2));
    assert!(done(pool.submit(Generation(1), sleep(1))).is_ok());
    let before = pool.pids();
    for &pid in &before {
        kill9_and_wait(pid);
    }
    for _ in 0..4 {
        let err = done(pool.submit(Generation(1), sleep(1))).unwrap_err();
        assert!(says_changed(&err, &exe), "{err}");
    }
    assert!(pool.pids().iter().all(|pid| before.contains(pid)));
    // The pool's own build back, as a new file of the same bytes: it starts again.
    put_in_place(&exe, &build(1));
    assert!(done(pool.submit(Generation(1), sleep(1))).is_ok());
}

#[test]
fn a_worker_executable_rewritten_with_its_own_bytes_goes_on() {
    // A relink that writes the same bytes again (a new file, a new time): the same build,
    // so workers start as before.
    let dir = scratch_dir("rewritten");
    let exe = dir.join("worker");
    write_executable(&exe, &build(1));
    let pool = KernelPool::start_workers(&exe, 1).unwrap();
    put_in_place(&exe, &build(1));
    for pid in pool.pids() {
        kill9_and_wait(pid);
    }
    assert!(done(pool.submit(Generation(1), sleep(1))).is_ok());
}

#[test]
fn a_worker_whose_executable_changes_as_it_starts_isnt_used() {
    // The executable is replaced between the pool's check and the worker's start: here by
    // the worker itself, which puts another build in its place as it starts, so it's always
    // after the check. The worker running may be either build, so it isn't used: it's
    // stopped before its answer is taken, and the request fails as a change.
    let dir = scratch_dir("changed-as-started");
    let exe = dir.join("worker");
    let other = dir.join("other-build");
    write_executable(&other, &build(2));
    write_executable(
        &exe,
        &changing_as_it_starts(
            &exe,
            &format!(
                "cp '{}' \"$exe.$$\" && mv \"$exe.$$\" \"$exe\"",
                other.display()
            ),
        ),
    );
    let pool = KernelPool::start_on_demand(&exe, 1).unwrap();
    let err = done(pool.submit(Generation(1), sleep(1))).unwrap_err();
    assert!(says_changed(&err, &exe), "{err}");
    let deadline = Instant::now() + HANG;
    while !pool.pids().is_empty() {
        assert!(Instant::now() < deadline, "the worker is still held");
        std::thread::sleep(Duration::from_millis(10));
    }
    wait_no_children("after the change");
}

/// A copy of the test worker that can't be run (not executable), at `dir/unrunnable`: an
/// executable linked to it can't start a worker, but is still the pool's build (its bytes
/// the same), so a worker whose start's check runs after the link changes isn't refused.
fn unrunnable_copy(dir: &Path) -> PathBuf {
    let copy = dir.join("unrunnable");
    std::fs::copy(TEST_WORKER, &copy).unwrap();
    std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o644)).unwrap();
    copy
}

/// Links `exe` to `target`, in place at once (a link made beside it, renamed over it).
fn link(exe: &Path, target: &Path) {
    let beside = exe.with_extension("new");
    std::os::unix::fs::symlink(target, &beside).unwrap();
    std::fs::rename(&beside, exe).unwrap();
}

/// A request for a slot that can't start a worker (its binary missing, or not runnable): fails, and doubles
/// the slot's wait.
fn fail_to_start(pool: &KernelPool) {
    let err = done(pool.submit(Generation(1), echo(&[1]))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
}

/// Crashes the one idle slot's worker while its binary is missing (or not runnable), then fails `more` starts
/// on it (the other slot busy, so every request goes to this one): each failure doubles its
/// wait from [`IDLE_CHECK`], so after `more` it's `2^more` checks. Returns the moment before
/// the last request was submitted: the slot's wait runs at least that long from then.
fn grow_idle_slots_wait(pool: &KernelPool, more: u32) -> Instant {
    let err =
        done(pool.submit(Generation(1), KernelRequest::debug(DebugOp::Segfault))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    let mut last = Instant::now();
    for _ in 0..more {
        last = Instant::now();
        fail_to_start(pool);
    }
    last
}

#[test]
fn a_pool_whose_binary_comes_back_after_many_failed_starts_fills_at_once() {
    // While the binary can't be run, each failed start doubles its slot's wait, toward a
    // minute. (Not runnable, rather than missing: its bytes still the pool's, so a worker
    // whose start's check runs after the binary goes isn't refused for it, and the slot that
    // started it is still busy when the test kills it.) Once a worker answers, the binary is back: every slot starts one at its next
    // check, not after its wait.
    //
    // Each slot's wait is grown in turn while the other is busy, so the requests that fail
    // all go to it (requests on two idle slots go to either). X runs a long sleep on its
    // first worker while Y fails six times: Y's wait is 32 s. With the binary back a moment,
    // a sleep starts a worker on Y (a request starts one whatever the wait); with it gone
    // again, X's worker is killed under its sleep and X fails six times: X's wait is 32 s.
    // Y's worker is killed: Y fails again, its wait a minute. Then, the binary back, a
    // request starts a worker on one slot and it answers. The other must be full well within
    // 32 s of X's last failure (in a second or two), which is the soonest either wait can
    // end, however loaded the machine.
    let dir = scratch_dir("restored-late");
    let exe = dir.join("silico-kernel-test-worker");
    std::os::unix::fs::symlink(TEST_WORKER, &exe).unwrap();
    let unrunnable = unrunnable_copy(&dir);
    let pool = KernelPool::start_workers(&exe, 2).unwrap();
    let before = pool.pids();
    let mut gone: BTreeSet<u32> = before.iter().copied().collect();
    link(&exe, &unrunnable);
    // Queued first, so taken first: by X.
    let x_busy = pool.submit(Generation(1), sleep(50_000));
    grow_idle_slots_wait(&pool, 5);
    link(&exe, Path::new(TEST_WORKER));
    let y_busy = pool.submit(Generation(1), sleep(50_000));
    let deadline = Instant::now() + HANG;
    let y_worker = loop {
        let pids = pool.pids();
        if let Some(&new) = pids.iter().find(|p| !before.contains(p)) {
            break new;
        }
        assert!(
            Instant::now() < deadline,
            "Y never started a worker: {pids:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    link(&exe, &unrunnable);
    let x_worker = pool
        .pids()
        .into_iter()
        .find(|p| before.contains(p))
        .unwrap();
    kill9_and_wait(x_worker);
    assert_eq!(done(x_busy).unwrap_err().code(), ErrorCode::WorkerCrashed);
    // X's replacement failed to start (its first failure): five more.
    let mut x_last = Instant::now();
    for _ in 0..5 {
        x_last = Instant::now();
        fail_to_start(&pool);
    }
    kill9_and_wait(y_worker);
    assert_eq!(done(y_busy).unwrap_err().code(), ErrorCode::WorkerCrashed);
    gone.insert(y_worker);
    // Y lets its worker go, then fails to start another, in the same moment. (Were the
    // binary back before that, Y would start a worker instead: the pool would still fill at
    // once, but the test would no longer tell a slot that waits out its wait from one that
    // doesn't.)
    let deadline = Instant::now() + HANG;
    while !pool.pids().is_empty() {
        assert!(Instant::now() < deadline, "Y kept {:?}", pool.pids());
        std::thread::sleep(Duration::from_millis(5));
    }
    link(&exe, Path::new(TEST_WORKER));
    assert!(done(pool.submit(Generation(1), sleep(1))).is_ok());
    let held = wait_full_by(&pool, 2, &gone, x_last + IDLE_CHECK * 31);
    drop(pool);
    assert_reaped(&held, "after the drop");
    wait_no_children("after the drop");
}

// ---------------------------------------------------------------------------------------
// Workers stopped, slowed, and talking out of turn

#[test]
fn a_stopped_worker_times_out_and_is_replaced_and_reaped() {
    // SIGSTOP: the worker is alive but never answers. Its request times out, it's killed
    // (SIGKILL ends a stopped process) and reaped, and a new worker takes its place.
    let pool = pool(TEST_WORKER, 1);
    let pid = pool.pids()[0];
    signal("STOP", pid);
    // Its 300 ms, then a kill and a reap: 5 s on a quiet machine, and the wakes on the way
    // as late as this machine runs them.
    let lateness = Lateness::start();
    let start = Instant::now();
    let err = done(pool.submit(
        Generation(1),
        echo(&[1]).with_timeout(Duration::from_millis(300)),
    ))
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerTimeout, "{err}");
    assert!(
        start.elapsed() < allowance(Duration::from_secs(5), lateness.most()),
        "{:?}",
        start.elapsed()
    );
    let after = wait_full(&pool, 1, &BTreeSet::from([pid]));
    assert_eq!(state(pid), None, "the stopped worker wasn't reaped");
    assert!(done(pool.submit(Generation(1), echo(&[2]))).is_ok());
    assert_ne!(after, [pid]);
}

#[test]
fn dropping_a_pool_whose_worker_is_stopped_is_quick() {
    let pool = pool(TEST_WORKER, 2);
    let pids = pool.pids();
    for &pid in &pids {
        signal("STOP", pid);
    }
    let pending: Vec<_> = (0..4)
        // Each its time no test reaches: a drop that waited for them to time out never returns.
        .map(|_| pool.submit(Generation(1), echo(&[1]).with_timeout(UNREACHED)))
        .collect();
    std::thread::sleep(Duration::from_millis(100));
    promptly("dropping the pool", move || drop(pool));
    assert_reaped(&pids, "after the drop");
    for p in pending {
        assert_eq!(done(p).unwrap_err().code(), ErrorCode::Internal);
    }
    wait_no_children("after the drop");
}

#[test]
fn a_request_allowed_no_time_fails_as_a_timeout_and_the_pool_goes_on() {
    let pool = pool(TEST_WORKER, 1);
    let err =
        done(pool.submit(Generation(1), echo(&[1]).with_timeout(Duration::ZERO))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerTimeout, "{err}");
    assert!(err.suggestion().is_some(), "{err}");
    assert_eq!(
        done(pool.submit(Generation(1), echo(&[2]))).unwrap(),
        Blob::inline(vec![2])
    );
}

#[test]
fn a_timeout_counts_from_sending_not_from_queueing() {
    // A request allowed 300 ms, queued behind a 600 ms one on a pool of one, still runs.
    let pool = pool(TEST_WORKER, 1);
    let first = pool.submit(Generation(1), sleep(600));
    let second = pool.submit(
        Generation(1),
        echo(&[5]).with_timeout(Duration::from_millis(300)),
    );
    done(first).unwrap();
    assert_eq!(done(second).unwrap(), Blob::inline(vec![5]));
}

#[test]
fn a_reply_left_over_from_one_request_doesnt_fail_the_next() {
    // A worker that answers a request twice: the second answer arrives unasked, and waits
    // on the socket. The pool checks, before giving a worker a request, that it has said
    // nothing out of turn, and replaces one that has, so the next request, someone else's,
    // isn't failed by the left-over answer.
    let exe = fake_worker("echo-twice");
    let pool = KernelPool::start_workers(&exe, 1).unwrap();
    let before = pool.pids();
    assert_eq!(
        done(pool.submit(Generation(1), echo(&[1]))).unwrap(),
        Blob::inline(vec![1])
    );
    // Let the extra answer arrive.
    std::thread::sleep(Duration::from_millis(100));
    let next = done(pool.submit(Generation(1), echo(&[2])));
    assert_eq!(next.unwrap(), Blob::inline(vec![2]));
    // And the worker that answered out of turn was replaced, not kept.
    assert_ne!(
        pool.pids(),
        before,
        "the worker that answered out of turn was kept"
    );
}

// ---------------------------------------------------------------------------------------
// Resources across many replacements

#[test]
fn dropping_pools_leaks_no_threads() {
    // Each pool has a thread per worker and its stopper; dropping it ends them all.
    drop(pool(TEST_WORKER, 1));
    std::thread::sleep(Duration::from_millis(200));
    let before = threads();
    for _ in 0..20 {
        drop(pool(TEST_WORKER, 1));
    }
    let deadline = Instant::now() + WAIT;
    while threads() > before && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(threads() <= before, "threads: {before} → {}", threads());
}

#[test]
fn replacing_workers_leaks_no_threads_descriptors_or_processes() {
    // 40 crashes, each followed by a replacement: the core's threads and descriptors
    // return to where they were, and the only child is the pool's one live worker.
    let pool = pool(TEST_WORKER, 1);
    let crash = |pool: &KernelPool| {
        let err =
            done(pool.submit(Generation(1), KernelRequest::debug(DebugOp::Segfault))).unwrap_err();
        assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    };
    for _ in 0..3 {
        crash(&pool);
    }
    wait_full(&pool, 1, &BTreeSet::new());
    std::thread::sleep(Duration::from_millis(200));
    let (fds, threads_before) = (open_fds(), threads());
    for _ in 0..40 {
        crash(&pool);
    }
    let pids = wait_full(&pool, 1, &BTreeSet::new());
    // The old workers' reader threads end when their sockets close; give them a moment.
    let deadline = Instant::now() + WAIT;
    while (open_fds() > fds || threads() > threads_before) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(open_fds() <= fds, "descriptors: {fds} → {}", open_fds());
    assert!(
        threads() <= threads_before,
        "threads: {threads_before} → {}",
        threads()
    );
    let kids: Vec<u32> = children().iter().map(|(p, _)| *p).collect();
    assert_eq!(kids, pids);
}

/// A socket or pipe a process holds open, as `lsof` lists it.
#[derive(Clone, Debug)]
struct Descriptor {
    pid: u32,
    fd: i32,
    kind: Kind,
    /// The kernel object's address.
    address: String,
}

/// What a [`Descriptor`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A Unix socket (`unix` to `lsof`).
    Socket,
    /// A pipe or FIFO (`PIPE` or `FIFO`).
    Pipe,
}

/// The descriptor a worker's socket to the core is at.
const SOCKET_FD: i32 = silico_protocol::WORKER_FD;
/// The descriptor a worker's stderr pipe is at.
const STDERR_FD: i32 = 2;

/// Each worker's sockets and pipes that `allowed` doesn't allow it, in round `round` of a
/// test. `allowed` is given each one and the worker's socket's address (the socket at
/// descriptor 3, which it may copy). One listing can miss a worker's socket for a moment
/// (on Linux, `lsof` reads `/proc/net/unix` in chunks, and an entry is skipped when a
/// socket before it closes in between; the socket is then listed as `sock`, not `unix`),
/// so the listing is taken again, for up to [`WAIT`], until it shows every worker's
/// socket.
fn not_allowed(
    round: usize,
    pids: &[u32],
    allowed: impl Fn(&Descriptor, Option<&String>) -> bool,
) -> Vec<Descriptor> {
    let deadline = Instant::now() + WAIT;
    let open = loop {
        let open = sockets_and_pipes(pids);
        let listed = pids
            .iter()
            .all(|&pid| open.iter().any(|o| o.pid == pid && o.fd == SOCKET_FD));
        if listed || Instant::now() >= deadline {
            break open;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut extra = Vec::new();
    for &pid in pids {
        let mine: Vec<_> = open.iter().filter(|o| o.pid == pid).collect();
        let socket = mine.iter().find(|o| o.fd == SOCKET_FD).map(|o| &o.address);
        assert!(
            socket.is_some(),
            "round {round}: lsof didn't list {pid}'s socket"
        );
        extra.extend(
            mine.iter()
                .filter(|o| !allowed(o, socket))
                .map(|o| (*o).clone()),
        );
    }
    extra
}

/// Each process's open descriptors that are sockets or pipes, as `lsof` lists them.
fn sockets_and_pipes(pids: &[u32]) -> Vec<Descriptor> {
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let out = Command::new("lsof")
        .args(["-n", "-P", "-a", "-p", &list, "-d", "0-1023", "-F", "pftdn"])
        .output()
        .unwrap();
    assert!(out.status.success(), "lsof failed: {out:?}");
    let mut found = Vec::new();
    let mut pid = 0;
    let (mut fd, mut kind, mut device) = (None, String::new(), String::new());
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let (tag, value) = line.split_at(1);
        match tag {
            "p" => pid = value.parse().unwrap(),
            "f" => {
                // Numbers, since only descriptors 0 to 1023 are asked for.
                fd = value.parse().ok();
                kind.clear();
                device.clear();
            }
            "t" => kind = value.to_owned(),
            "d" => device = value.to_owned(),
            // The name comes last for each descriptor.
            "n" => {
                let kind = match kind.as_str() {
                    "unix" => Kind::Socket,
                    "PIPE" | "FIFO" => Kind::Pipe,
                    _ => continue,
                };
                found.push(Descriptor {
                    pid,
                    fd: fd.unwrap(),
                    kind,
                    address: format!("{device} {value}"),
                });
            }
            _ => {}
        }
    }
    found
}

/// Starts a pool on another thread, failing the test if it hasn't started within [`WAIT`]
/// (a pool of a few workers starts in well under a second).
fn start_within(exe: &Path, size: usize) -> KernelPool {
    let (tx, rx) = std::sync::mpsc::channel();
    let exe = exe.to_owned();
    std::thread::spawn(move || {
        let _ = tx.send(KernelPool::start_workers(&exe, size));
    });
    match rx.recv_timeout(WAIT) {
        Ok(pool) => pool.unwrap(),
        Err(_) => panic!("starting a pool of {size} workers hung"),
    }
}

#[test]
fn running_workers_hold_only_their_own_descriptors() {
    // A worker, once running, holds only its own socket (3, and the copy it makes of it) and
    // its own stderr pipe (2): nothing a sibling started at the same time left open to it
    // (on macOS, std marks a new socket or pipe close-on-exec only after making it, so a
    // sibling forked in between inherited it, and some starts never finished: a slot's
    // `Command::spawn` waited for good on its exec-status pipe, whose write end a sibling
    // held), and nothing the app holds without close-on-exec (a test runner's jobserver
    // pipe, say). Workers now start one at a time, and close what they inherit when they
    // run. Pools as large as `KernelPool::start` makes on this machine, started 40 times;
    // each worker is given up to `WAIT` to start running.
    let size = std::thread::available_parallelism()
        .map_or(8, usize::from)
        .max(8);
    for round in 0..40 {
        let pool = start_within(Path::new(WORKER), size);
        let pids = pool.pids();
        let deadline = Instant::now() + WAIT;
        loop {
            let extra = not_allowed(round, &pids, |o, socket| {
                o.fd == STDERR_FD
                    || o.fd == SOCKET_FD
                    || (o.kind == Kind::Socket && Some(&o.address) == socket)
            });
            if extra.is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "round {round}: workers hold descriptors that aren't theirs: {extra:#?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(pool);
    }
}

#[test]
fn workers_started_together_inherit_no_siblings_socket() {
    // Starting workers one at a time is what keeps a worker from inheriting a sibling's
    // socket or pipes in the first place; closing what it inherited when it runs only ends
    // that once it runs. A worker that never closes them (here a script that sleeps), checked
    // at once after the pool starts, holds only its own: and no start hangs.
    let dir = scratch_dir("noclose");
    let exe = dir.join("worker");
    // It sleeps past `WAIT`, so a start hung on a sibling's pipe isn't released by that
    // sibling ending within it.
    write_executable(&exe, "#!/bin/sh\nexec sleep 600\n");
    let size = std::thread::available_parallelism()
        .map_or(8, usize::from)
        .max(8);
    for round in 0..40 {
        let pool = start_within(&exe, size);
        // Only another worker's socket is checked: a worker that never runs keeps the
        // descriptors it inherits from the app.
        let extra = not_allowed(round, &pool.pids(), |o, socket| {
            o.kind != Kind::Socket || o.fd == SOCKET_FD || Some(&o.address) == socket
        });
        assert!(extra.is_empty(), "round {round}: {extra:#?}");
        drop(pool);
    }
}

#[test]
fn running_workers_close_what_the_app_left_open() {
    // A pipe the app holds without close-on-exec (a library's, or a test runner's jobserver
    // pipe) is inherited by every worker it starts; a worker closes it when it runs.
    let (reader, writer) = std::io::pipe().unwrap();
    for end in [
        rustix::fd::AsFd::as_fd(&reader),
        rustix::fd::AsFd::as_fd(&writer),
    ] {
        rustix::io::fcntl_setfd(end, rustix::io::FdFlags::empty()).unwrap();
    }
    let pool = start_within(Path::new(WORKER), 2);
    let pids = pool.pids();
    let deadline = Instant::now() + WAIT;
    loop {
        let kept: Vec<_> = sockets_and_pipes(&pids)
            .into_iter()
            .filter(|o| o.kind != Kind::Socket && o.fd != STDERR_FD)
            .collect();
        if kept.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "workers kept the app's pipe: {kept:#?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    drop((reader, writer));
}

// ---------------------------------------------------------------------------------------
// WorkerKiller and has_exited, directly

#[test]
fn a_killer_ends_a_call_at_once_and_is_harmless_after_the_worker_is_gone() {
    let mut worker = Worker::spawn(Path::new(TEST_WORKER)).unwrap();
    let killer = worker.killer();
    assert_eq!(killer.pid(), worker.pid());
    assert!(!worker.has_exited());
    // Answered once, so it has started: a kill mid-call is then a crash. A worker killed
    // before it said it started is `worker_unavailable` (its install, not its request), and
    // one slow to start (a loaded machine, a binary just built and checked by macOS) took
    // longer than the 150 ms below to say it.
    worker
        .call(
            Generation(0),
            Op::Debug(DebugOp::Sleep { millis: 0 }),
            Duration::from_secs(60),
        )
        .unwrap();
    // A sleep no test reaches, and a call allowed `WAIT`: a kill that didn't end the call
    // would end it as a timeout, not the crash asserted below.
    let err = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(150));
            killer.kill();
        });
        worker
            .call(
                Generation(0),
                Op::Debug(DebugOp::Sleep {
                    millis: UNREACHED.as_millis() as u64,
                }),
                WAIT,
            )
            .unwrap_err()
    });
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    assert!(err.cause().unwrap().contains("SIGKILL"), "{err}");
    assert!(worker.is_stopped());
    // Killed and reaped: the killer, used again, signals nothing, and the worker has exited.
    killer.kill();
    assert!(worker.has_exited());
    let pid = worker.pid();
    drop(worker);
    killer.kill();
    assert_eq!(killer.pid(), pid);
    assert_eq!(state(pid), None);
}

#[test]
fn has_exited_reaps_a_worker_killed_from_outside_and_its_next_call_fails_cleanly() {
    let mut worker = Worker::spawn(Path::new(TEST_WORKER)).unwrap();
    // Answered once, so it had started: its death is a crash (killed before its first
    // frame, it would be worker_unavailable).
    worker
        .call(
            Generation(0),
            Op::Echo {
                data: Blob::inline(vec![0]),
            },
            PATIENCE,
        )
        .unwrap();
    let pid = worker.pid();
    kill9_and_wait(pid);
    assert!(worker.has_exited());
    assert_eq!(state(pid), None, "has_exited didn't reap it");
    // Asking again is the same answer.
    assert!(worker.has_exited());
    let err = worker
        .call(
            Generation(0),
            Op::Echo {
                data: Blob::inline(vec![1]),
            },
            PATIENCE,
        )
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    assert!(err.cause().unwrap().contains("SIGKILL"), "{err}");
}

#[test]
fn a_box_through_the_pool_after_every_kind_of_failure() {
    // Corruption check: after a crash, a hang, a garbage reply, a reply out of turn, and an
    // abort, each on the one worker of a pool, the next real operation is right (a
    // 10 x 20 x 30 box has volume 6000).
    let pool = pool(TEST_WORKER, 1);
    let failures = [
        KernelRequest::debug(DebugOp::Segfault),
        KernelRequest::debug(DebugOp::Abort),
        KernelRequest::debug(DebugOp::Garbage),
        KernelRequest::debug(DebugOp::ReplyOutOfTurn(silico_protocol::OutOfTurn::WrongId)),
        KernelRequest::debug(DebugOp::ReplyOutOfTurn(
            silico_protocol::OutOfTurn::WrongGeneration,
        )),
        sleep(10_000).with_timeout(Duration::from_millis(200)),
    ];
    for request in failures {
        let name = request.name();
        let err = done(pool.submit(Generation(1), request)).unwrap_err();
        assert!(
            matches!(
                err.code(),
                ErrorCode::WorkerCrashed | ErrorCode::WorkerTimeout | ErrorCode::ProtocolError
            ),
            "{name}: {err}"
        );
        let shape = done(
            pool.submit(
                Generation(1),
                KernelRequest::make(Primitive::Box {
                    dx: 10.0,
                    dy: 20.0,
                    dz: 30.0,
                })
                .unwrap(),
            ),
        )
        .unwrap();
        let summary = done(pool.submit(Generation(1), KernelRequest::summarize(&shape))).unwrap();
        assert!(
            (summary.volume - 6000.0).abs() <= 6000.0 * 1e-12,
            "{name}: {summary:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// Bursts

#[test]
fn every_worker_crashing_at_once_leaves_a_full_pool_every_time() {
    // As many workers as `KernelPool::start` makes here, all crashed at once, 8 times:
    // every slot replaces its worker at about the same moment. Each time the pool must be
    // back to full size with new workers within seconds, and run a request on every slot.
    let size = std::thread::available_parallelism()
        .map_or(8, usize::from)
        .max(8);
    let pool = start_within(Path::new(TEST_WORKER), size);
    for round in 0..8 {
        let before = wait_full(&pool, size, &BTreeSet::new());
        let crashes: Vec<_> = (0..size)
            .map(|_| pool.submit(Generation(1), KernelRequest::debug(DebugOp::Segfault)))
            .collect();
        for c in crashes {
            let err = done(c).unwrap_err();
            assert_eq!(err.code(), ErrorCode::WorkerCrashed, "round {round}: {err}");
        }
        // The pool may hand a crash to a slot it has already replaced, so a worker can be
        // missed: that one is killed by hand (SIGKILL: Rust's runtime survives a SIGSEGV
        // sent from outside). A worker that crashed and wasn't replaced isn't running, so
        // it's left for `wait_full` to find.
        let running: BTreeSet<u32> = children()
            .into_iter()
            .filter(|(_, stat)| !stat.starts_with('Z'))
            .map(|(pid, _)| pid)
            .collect();
        for &pid in before.iter().filter(|p| running.contains(p)) {
            // It may have ended since it was listed: `wait_full` checks what came of it.
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
        let after = wait_full(&pool, size, &before.iter().copied().collect());
        assert_eq!(after.len(), size, "round {round}");
        let sleeps: Vec<_> = (0..size)
            .map(|_| pool.submit(Generation(1), sleep(20)))
            .collect();
        for s in sleeps {
            done(s).unwrap();
        }
    }
}

#[test]
fn twenty_thousand_small_requests_all_come_back_right() {
    let pool = pool(WORKER, 4);
    // Their wall time over how much slower than a quiet machine this one ran them (`Share`),
    // which load doesn't move.
    let share = Share::start();
    let start = Instant::now();
    let pending: Vec<_> = (0..20_000u32)
        .map(|i| (i, pool.submit(Generation(1), echo(&i.to_le_bytes()))))
        .collect();
    for (i, p) in pending {
        assert_eq!(
            done(p).unwrap(),
            Blob::inline(i.to_le_bytes().to_vec()),
            "{i}"
        );
    }
    let wall = start.elapsed();
    let slowdown = share.finish().slowdown;
    assert!(
        wall.div_f64(slowdown) < Duration::from_secs(30),
        "{wall:?}, the machine {slowdown:.2} times slower than a quiet one"
    );
}

#[test]
fn a_request_over_the_frame_limit_is_refused_and_its_worker_kept() {
    let pool = pool(TEST_WORKER, 1);
    let before = pool.pids();
    let too_large = vec![0; silico_protocol::MAX_FRAME_LEN as usize + 1];
    let err =
        done(pool.submit(Generation(1), KernelRequest::echo(Blob::inline(too_large)))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert_eq!(
        done(pool.submit(Generation(1), echo(&[3]))).unwrap(),
        Blob::inline(vec![3])
    );
    assert_eq!(
        pool.pids(),
        before,
        "a refused request cost the pool its worker"
    );
}

#[test]
fn requests_whose_callers_stopped_waiting_dont_jam_the_pool() {
    // 40 requests submitted whose callers never wait on them (their `Pending`s kept, so
    // they aren't given up): they run, and a request after them is answered once they've
    // run. (A `Pending` dropped gives its request up, `pool::Pending`.)
    let pool = pool(TEST_WORKER, 2);
    let unwaited: Vec<_> = (0..40)
        .map(|_| pool.submit(Generation(1), sleep(10)))
        .collect();
    let lateness = Lateness::start();
    let start = Instant::now();
    assert_eq!(
        done(pool.submit(Generation(1), echo(&[4]))).unwrap(),
        Blob::inline(vec![4])
    );
    // 40 × 10 ms on two workers: a jammed pool would never answer, and one that stalls on
    // each abandoned request would take seconds. (Bounds are against what they rule out, not
    // how fast a loaded machine is, and allow for how late it wakes threads.)
    assert!(
        start.elapsed() < allowance(Duration::from_secs(10), lateness.most()),
        "{:?}",
        start.elapsed()
    );
    drop(unwaited);
    // And `advance` drops what's still queued, their `Pending`s kept so only it can: a
    // request after it waits at most for the two sleeps already running (stopped once they've
    // run the stop-after), where the queue would take 40 × 4 s on two workers, 80 s, past
    // the wait `done` allows.
    let unwaited: Vec<_> = (0..40)
        .map(|_| pool.submit(Generation(1), sleep(4_000)))
        .collect();
    pool.advance(Generation(2));
    // Dropped by the advance itself, before any slot looks at them: all but the two running,
    // superseded already.
    let (superseded, waiting): (Vec<_>, Vec<_>) = unwaited
        .into_iter()
        .map(|p| p.try_take())
        .partition(|taken| matches!(taken, Ok(Outcome::Superseded)));
    assert!(
        superseded.len() >= 38,
        "{} of 40 superseded by the advance, {} not",
        superseded.len(),
        waiting.len()
    );
    done(pool.submit(Generation(2), echo(&[5]))).unwrap();
    drop(waiting);
}

#[test]
fn every_idle_worker_killed_at_once_is_replaced_at_once_every_time() {
    // Every worker of a production-sized pool killed while idle and running (each slot has
    // noted it running, so the kill is from outside), 5 times: each time every slot decides
    // to start another at once, never to wait, as the pool says it in its span's events, and
    // the pool is full again, of none of the killed. A slot that waited, as for a binary that
    // can't start, or left its worker dead, fails this. The decisions are what's checked, not
    // how soon the pool refills, so a machine short of CPU, which stretches how long the
    // workers take to start, can't fail it (gate 39813098861 took more than the 5 s this once
    // allowed); the waits are bounded by HANG only.
    let decisions = Decisions::record();
    let size = std::thread::available_parallelism()
        .map_or(8, usize::from)
        .max(8);
    let pool = KernelPool::start_workers(Path::new(TEST_WORKER), size).unwrap();
    let mut gone = BTreeSet::new();
    for round in 0..5 {
        let before = wait_full_by(&pool, size, &gone, Instant::now() + HANG);
        let deadline = Instant::now() + HANG;
        while !(0..size as u64).all(|slot| decisions.running(slot) > round) {
            assert!(
                Instant::now() < deadline,
                "round {round}: not every slot's worker running after {HANG:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let status = Command::new("kill")
            .arg("-9")
            .args(before.iter().map(u32::to_string))
            .status()
            .unwrap();
        assert!(status.success());
        gone.extend(before);
    }
    wait_full_by(&pool, size, &gone, Instant::now() + HANG);
    for slot in 0..size as u64 {
        assert_eq!(
            decisions.of_slot(slot, 6),
            [Decision::AtOnce; 5],
            "slot {slot}"
        );
    }
}

#[test]
fn a_test_operation_sent_to_production_workers_is_refused_and_the_worker_kept() {
    let pool = pool(WORKER, 1);
    let before = pool.pids();
    let err = done(pool.submit(Generation(1), sleep(1))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::Internal, "{err}");
    assert!(err.message().contains("DebugSleep"), "{err}");
    assert_eq!(
        done(pool.submit(Generation(1), echo(&[6]))).unwrap(),
        Blob::inline(vec![6])
    );
    assert_eq!(pool.pids(), before);
}

#[test]
fn a_superseded_request_whose_worker_is_killed_ends_superseded_and_the_worker_is_replaced() {
    // `advance` stops a superseded request that's running by killing its worker: the
    // request ends Superseded, not when its worker would answer (never: it's held until
    // let go, and allowed an hour before the pool would stop it as hung), and the worker is
    // replaced. The advance comes once the worker says it has the request: one still
    // waiting for the slot would be dropped unrun, and its worker kept.
    let pool = KernelPool::start_workers(&fake_worker("hold"), 1).unwrap();
    let before = pool.pids();
    let dir = scratch_dir("killed-held");
    let mut hold = HOLD.to_vec();
    hold.extend_from_slice(dir.to_str().unwrap().as_bytes());
    let slow = pool.submit(
        Generation(1),
        echo(&hold).with_timeout(Duration::from_secs(3600)),
    );
    let started = dir.join("started");
    let deadline = Instant::now() + PATIENCE;
    while !started.exists() {
        assert!(
            Instant::now() < deadline,
            "the held request didn't reach its worker within {PATIENCE:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    pool.advance(Generation(2));
    assert!(matches!(outcome(slow), Outcome::Superseded));
    wait_full(&pool, 1, &before.iter().copied().collect());
    assert_eq!(
        done(pool.submit(Generation(2), echo(&[7]))).unwrap(),
        Blob::inline(vec![7])
    );
}

#[test]
fn a_pool_started_from_a_relative_path_keeps_its_workers_after_the_directory_changes() {
    // Given a relative path, the pool finds its worker once, at start, so a replacement
    // after the working directory changes comes from the same place.
    let dir = scratch_dir("relative");
    std::os::unix::fs::symlink(TEST_WORKER, dir.join("worker")).unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(&dir).unwrap();
    let pool = KernelPool::start_workers(Path::new("./worker"), 1).unwrap();
    std::env::set_current_dir(&original).unwrap();
    let before = pool.pids();
    kill9_and_wait(before[0]);
    // The replacement comes from where the pool found its worker at start (it was looked
    // for at ./worker in the new working directory, and every request after failed).
    let result = done(pool.submit(Generation(1), echo(&[8])));
    assert_eq!(result.unwrap(), Blob::inline(vec![8]));
}

#[test]
fn a_worker_that_dies_as_it_starts_isnt_started_over_while_the_pool_is_idle() {
    // A worker binary that starts and exits at once (a missing library, a refusal at start):
    // it isn't launched again every second for as long as the pool is idle, but less and
    // less often; a request starts one at once, and fails saying how it ended.
    //
    // A slot sees its worker's end at an idle check, at least a second after it launched
    // it, and then waits 1 s, 2 s, 4 s, ... before launching another: so its launches come
    // no sooner than 0, 2, 5, and 10 s after the pool started, however loaded the machine
    // (load only makes them later). In the first 8 s, at most three a slot, twelve in all,
    // where launching at every check would be about 32, and a fixed 1 s wait about 20.
    let dir = scratch_dir("dies");
    let (script, log) = launch_logger(&dir, "echo 'no OCCT here' >&2\nexit 3");
    let started = epoch_now();
    let pool = KernelPool::start_workers(&script, 4).unwrap();
    // Each slot launching a fourth time (at about 10 s): the pool doesn't give up on them.
    wait_launches(&log, 16);
    let times = launch_times(&log);
    let early: Vec<f64> = times
        .iter()
        .map(|t| t - started)
        .filter(|&t| t < 8.0)
        .collect();
    assert!(
        early.len() <= 12,
        "{} launches in the first 8 s idle, at {early:?}",
        early.len()
    );
    let err = done(pool.submit(Generation(1), echo(&[1]))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
    assert!(
        err.cause()
            .is_some_and(|c| c.contains("exited with code 3") && c.contains("no OCCT here")),
        "{err}"
    );
    drop(pool);
}

/// Now, in seconds since the epoch: the clock [`log_launch`] logs by.
fn epoch_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

/// The process ID of launch `n` (from 0, earliest first) logged to `log`.
fn launch_pid(log: &Path, n: usize) -> u32 {
    launches(log)[n].1
}

/// Whether process `pid` has ended: exited (a zombie), or reaped (`ps` may still list one
/// reaped an instant before, with no state: `?`).
fn has_ended(pid: u32) -> bool {
    state(pid).is_none_or(|s| s.starts_with('Z') || s.starts_with('?'))
}

/// Waits until worker `pid` has exited.
fn wait_exited(pid: u32) {
    let deadline = Instant::now() + HANG;
    while !has_ended(pid) {
        assert!(Instant::now() < deadline, "{pid} never exited");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Waits until the pool has let worker `pid` go after its end: reaped it (which only the
/// slot holding it does, so it had been listed) and no longer lists it. Its slot decides
/// how to go on in the same moment, microseconds later, with nothing to show it has.
fn wait_let_go(pool: &KernelPool, pid: u32) {
    let deadline = Instant::now() + HANG;
    while state(pid).is_some_and(|s| !s.starts_with('?')) || pool.pids().contains(&pid) {
        assert!(Instant::now() < deadline, "the pool kept {pid}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn an_idle_slot_whose_worker_dies_at_start_waits_longer_each_time() {
    // One slot, its worker ending at once each time: it's started again after waits that
    // double from 1 s (each after a tick to notice the end), so launches at about 0, 2, 5,
    // 10, and 19 s, each gap longer than its wait: 1, 2, 4, and 8 s. Not never (no fifth
    // launch), not every tick (gaps of about 1 s), not a fixed wait (about 2 s), and not a
    // wait that grows by a tick each time (launches at 0, 2, 5, 9, and 14 s: the last gap
    // about 5 s).
    let dir = scratch_dir("backoff");
    let (script, log) = launch_logger(&dir, "exit 3");
    let pool = KernelPool::start_workers(&script, 1).unwrap();
    let times = wait_launches(&log, 5);
    drop(pool);
    assert_gaps_exceed(&times[..5], &[1.0, 2.0, 4.0, 8.0]);
}

#[test]
fn an_idle_slot_whose_worker_starts_then_exits_a_tick_later_waits_longer_each_time() {
    // The worker says it has started, runs past an idle check (seen running), then exits on
    // its own after 1.5 s. Its having run doesn't end the slot's wait: only an answer does.
    // Launches at about 0, 3, 7, and 13 s: each gap longer than the worker's 1.5 s and the
    // wait after it, 1, 2, and 4 s. Not one every 3 s, as when having run ended the wait (the
    // third gap about 3 s, where over 5.5 s is due).
    let dir = scratch_dir("starts-runs-exits");
    let (script, log) = launch_logger(&dir, "printf '\\0\\0\\0\\0' >&3\nsleep 1.5\nexit 3");
    let pool = KernelPool::start_workers(&script, 1).unwrap();
    let times = wait_launches(&log, 4);
    drop(pool);
    assert_gaps_exceed(&times[..4], &[2.5, 3.5, 5.5]);
}

#[test]
fn an_answer_ends_a_slots_wait() {
    // While `fail` exists, a launch exits on its own before it starts; otherwise it's the
    // test worker. The slot's wait grows (launches at about 0, 2, 5, and 10 s, 4 s waited
    // before the last), then a worker starts and answers a request, refusing it. Killed after
    // `fail` is back, it's replaced at once and that one fails: the next launch comes after a
    // fresh wait, of 1 s, not after the one grown before the answer, doubled: at least 8 s,
    // however the machine runs. 6 s for a loaded machine to run an idle check late.
    let dir = scratch_dir("answer-resets");
    let fail = dir.join("fail");
    std::fs::write(&fail, "").unwrap();
    let (script, log) = launch_logger(
        &dir,
        &format!(
            "if [ -e '{}' ]; then exit 3; fi\nexec '{TEST_WORKER}'",
            fail.display()
        ),
    );
    let pool = KernelPool::start_workers(&script, 1).unwrap();
    wait_launches(&log, 4);
    // That launch has failed (were `fail` gone before it looked, the request might go to a
    // worker on its way out, and fail as a crash), so the request starts a worker of its own.
    wait_exited(launch_pid(&log, 3));
    std::fs::remove_file(&fail).unwrap();
    // A refusal is an answer too: the worker reads the bytes, finds no shape in them, and
    // says so.
    let garbage = silico_protocol::ShapeData::unchecked(Blob::inline(b"not a shape".to_vec()));
    let refused = done(pool.submit(Generation(1), KernelRequest::summarize(&garbage))).unwrap_err();
    assert_eq!(refused.code(), ErrorCode::InvalidInput, "{refused}");
    std::fs::write(&fail, "").unwrap();
    // Every launch before the kill logged before it: the one that answered had run.
    let killed_at = epoch_now();
    kill9_and_wait(pool.pids()[0]);
    let after = wait_launches_after(&log, killed_at, 2);
    drop(pool);
    let gap = after[1] - after[0];
    assert!(
        gap > 1.0,
        "no wait after the failure: launches at {after:?}"
    );
    assert!(
        gap < 8.0,
        "the wait from before the answer: launches {after:?} after the kill"
    );
}

#[test]
fn a_wait_ended_early_by_a_newer_worker_still_doubles_on_the_next_failure() {
    // Two slots, A and B, of a script that, for the launches that work, runs the test
    // worker as a child (so killing the child makes the script exit 3, on its own), and
    // otherwise exits 3 at once. The first two launches work, one a slot. A's child is
    // killed, and A fails (launches 3 to 6) until its wait is 16 s. Then B's worker is
    // killed from outside: B's replacement (launch 7) works, which ends A's wait early. A's
    // next launch (8) fails: its wait doubles to 32 s, not back to 1 s, so no launch follows
    // (it would within about 2 s). Each launch takes its number in turn, so which launch
    // works is the same however the machine runs. What each slot decides is what's checked,
    // as the pool says it (A: waits of 1, 2, 4, 8, and 16 s, the last ended early, then
    // 32 s; B: at once), so a stalled machine can't fail it by the times between launches.
    let decisions = Decisions::record();
    let dir = scratch_dir("moot-keeps");
    let children = dir.join("children");
    let (script, log) = launch_logger(
        &dir,
        &format!(
            "n=1; while ! mkdir '{}'/launch$n 2>/dev/null; do n=$((n + 1)); done\n\
             case $n in 1|2|7) '{TEST_WORKER}' & echo \"$$ $!\" >> '{}'; wait;; esac\n\
             exit 3",
            dir.display(),
            children.display()
        ),
    );
    let started = |script: u32| {
        std::fs::read_to_string(&children)
            .unwrap_or_default()
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{script} ")).map(str::to_owned))
    };
    let pool = KernelPool::start_workers(&script, 2).unwrap();
    let pids = pool.pids();
    assert_eq!(pids.len(), 2);
    let (a, b) = (pids[0], pids[1]);
    // Both first launches have started their workers.
    let deadline = Instant::now() + HANG;
    let a_child = loop {
        if let (Some(child), Some(_)) = (started(a), started(b)) {
            break child;
        }
        assert!(Instant::now() < deadline, "the workers never started");
        std::thread::sleep(Duration::from_millis(5));
    };
    signal("KILL", a_child.trim().parse().unwrap());
    // A: launches at about 2, 5, 10, and 19 s fail; once it has seen the last end (its
    // worker let go), its wait is 16 s.
    wait_launches(&log, 6);
    wait_let_go(&pool, launch_pid(&log, 5));
    signal("KILL", b);
    // B's replacement (launch 7) works; A's next (8), early, fails.
    let early = wait_launches(&log, 8)[7];
    // A says its wait after launch 8 once it has seen launch 8 end: waited for, so a slow
    // machine can't drop the pool first. Then long enough for a launch after a 1 s wait
    // (about 2 s), for the times to show.
    let deadline = Instant::now() + HANG;
    while decisions.of_slot(0, 7).len() < 7 {
        assert!(Instant::now() < deadline, "A never decided after launch 8");
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_secs(6));
    drop(pool);
    let times = launch_times(&log);
    let gaps = gaps(&times);
    assert_eq!(
        times.len(),
        8,
        "A's wait went back after its early launch: gaps {gaps:?}"
    );
    assert_eq!(times[7], early);
    // A's last wait was ended early by B's new worker, not waited out, and the next was
    // doubled from it; B's worker, killed from outside, was replaced at once.
    use Decision::{AtOnce, Wait, WaitEnded};
    assert_eq!(
        decisions.of_slot(0, 7),
        [
            Wait(1000),
            Wait(2000),
            Wait(4000),
            Wait(8000),
            Wait(16000),
            WaitEnded,
            Wait(32000)
        ],
        "gaps {gaps:?}"
    );
    assert_eq!(decisions.of_slot(1, 1), [AtOnce], "gaps {gaps:?}");
    // Three launches worked: the first two, and B's replacement.
    let worked = std::fs::read_to_string(&children).unwrap().lines().count();
    assert_eq!(worked, 3, "gaps {gaps:?}");
}

#[test]
fn a_request_refused_before_its_sent_doesnt_end_a_slots_wait() {
    // Launches at about 0, 2, and 5 s, each worker ending at once (exit 3): the slot waits
    // 1, 2, then 4 s, from the idle check that sees each end. A request over the frame
    // limit, sent while the slot waits, starts a worker (which ends too) and is refused
    // before it's sent: no answer, so the slot's wait isn't ended, and its next is the last
    // doubled, at least 4 s after the request's worker (not 1 s, a wait begun anew).
    // The request is sent once the slot has let its last worker go, so while it waits; and
    // the next launch is timed from the request's return, after its worker started, since
    // a loaded machine can log a launch seconds late (a worker's own `perl` writes it).
    // That was a race: sent once the worker had exited, by a test thread that then polled
    // `ps` and could fall 4 s behind on a loaded machine, it met the slot's own next launch
    // and started none (proven by a 5 s stall there). So each pass waits, after the
    // request, until the slot has let go of every worker it holds: each logs before it
    // ends, so every launch of the pass is then in the log. The pass counts only if it
    // holds one launch, a worker the pool didn't hold before the request; otherwise (the
    // slot started its own, the request meeting it or ending it and starting another) the
    // request is sent again, in the slot's next, longer wait.
    let dir = scratch_dir("refusal-keeps-wait");
    let (script, log) = launch_logger(&dir, "exit 3");
    let pool = KernelPool::start_workers(&script, 1).unwrap();
    wait_launches(&log, 3);
    let too_large = vec![0; silico_protocol::MAX_FRAME_LEN as usize + 1];
    let (request, returned) = loop {
        let before = launches(&log).len();
        wait_let_go(&pool, launch_pid(&log, before - 1));
        let held = pool.pids();
        let err = done(pool.submit(
            Generation(1),
            KernelRequest::echo(Blob::inline(too_large.clone())),
        ))
        .unwrap_err();
        let returned = epoch_now();
        assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
        for pid in pool.pids() {
            wait_let_go(&pool, pid);
        }
        if let [(time, pid)] = launches(&log)[before..]
            && !held.contains(&pid)
        {
            break (time, returned);
        }
    };
    let next = wait_launches_after(&log, request, 1)[0];
    drop(pool);
    assert!(
        next - returned > 4.0,
        "a refusal ended the wait: the slot started its next worker {:.3} s after the \
         request returned (launches at {:?})",
        next - returned,
        launch_times(&log)
    );
}

#[test]
fn a_worker_started_before_a_slots_failure_doesnt_end_its_wait() {
    // Two slots: the first launch is a worker that starts when told to; every other ends at
    // once. The failing slot launches at about 0, 2, 5, and 10 s, each gap longer than its
    // wait (1, 2, 4 s). The slow worker is told to start once the failing slot has failed
    // (its second launch is logged): it was launched before the failure, so its starting
    // says nothing about the binary since, and doesn't end the wait. (It did, and the
    // failing slot launched about every 2 s once the slow worker had started.)
    let dir = scratch_dir("older-start");
    let log = dir.join("launches");
    let go = dir.join("go");
    let script = dir.join("worker.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nif mkdir '{}' 2>/dev/null; then\n\
             while [ ! -e '{}' ]; do sleep 0.05; done; exec '{TEST_WORKER}'; fi\n{}\nexit 3\n",
            dir.join("slow").display(),
            go.display(),
            log_launch(&log)
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let pool = KernelPool::start_workers(&script, 2).unwrap();
    wait_launches(&log, 2);
    std::fs::write(&go, "").unwrap();
    let times = wait_launches(&log, 4);
    drop(pool);
    assert_gaps_exceed(&times[..4], &[1.0, 2.0, 4.0]);
}

#[test]
fn a_slot_waiting_out_a_long_back_off_starts_a_worker_once_a_newer_one_has_started() {
    // A slot whose worker can't be started (its binary not runnable: see
    // `a_pool_whose_binary_comes_back_after_many_failed_starts_fills_at_once`) waits longer each time it
    // tries: here the other slot is busy, so every request goes to the first, which fails
    // six times in a row (its worker crashed, then five requests): its wait is 32 s. With
    // the binary back, the busy slot's worker is killed under its request: that slot starts
    // another at once, and once it has started (seen at its next idle check), the first
    // stops waiting and starts one at its own next check. So the pool is full within a
    // second or two, well before 32 s from the first slot's last failure, the soonest its
    // wait could end, however loaded the machine.
    let dir = scratch_dir("waits-for-newer");
    let exe = dir.join("silico-kernel-test-worker");
    std::os::unix::fs::symlink(TEST_WORKER, &exe).unwrap();
    let unrunnable = unrunnable_copy(&dir);
    let pool = KernelPool::start_workers(&exe, 2).unwrap();
    let before = pool.pids();
    link(&exe, &unrunnable);
    // Queued first, so taken first, by the slot kept busy.
    let busy = pool.submit(Generation(1), sleep(50_000));
    let last = grow_idle_slots_wait(&pool, 5);
    link(&exe, Path::new(TEST_WORKER));
    let working = pool.pids();
    assert_eq!(working.len(), 1, "{working:?}");
    kill9_and_wait(working[0]);
    assert_eq!(done(busy).unwrap_err().code(), ErrorCode::WorkerCrashed);
    let held = wait_full_by(
        &pool,
        2,
        &before.iter().copied().collect(),
        last + IDLE_CHECK * 31,
    );
    drop(pool);
    assert_reaped(&held, "after the drop");
    wait_no_children("after the drop");
}

/// The race silico-core's `no_result_of_an_older_generation_arrives_after_advance_returns`
/// runs on the fake kernel, run on the pool, which decides and sends under the lock
/// `advance` takes: once `advance` has returned with no outcome in the channel, the outcome
/// is `Superseded`.
#[test]
fn on_the_pool_no_result_of_an_older_generation_arrives_after_advance_returns() {
    let real = KernelPool::start_workers(Path::new(WORKER), 1).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut generation = 0u64;
    let mut spin = 0u32;
    let mut late = None;
    while Instant::now() < deadline && late.is_none() {
        let pending = real.submit(
            Generation(generation),
            KernelRequest::echo(Blob::inline(vec![])),
        );
        spin = (spin + 37) % 40_000;
        for _ in 0..spin {
            std::hint::spin_loop();
        }
        generation += 1;
        real.advance(Generation(generation));
        if let Err(pending) = pending.try_take()
            && let Outcome::Done(_) = pool::outcome(pending)
        {
            late = Some(generation - 1);
        }
    }
    assert!(
        generation > 100,
        "too few rounds to mean anything: {generation}"
    );
    assert_eq!(late, None);
}

// ---------------------------------------------------------------------------------------
// break-it: slots started on demand (KernelPool::start_on_demand, grow, State::started and
// busy, the Busy guard). Requests one after another, bursts from many threads, drops and
// advances while slots start, and executables that change before any worker has started.

/// A pool whose slots start on demand, as `KernelPool::start`'s do.
fn on_demand(exe: &str, size: usize) -> KernelPool {
    KernelPool::start_on_demand(Path::new(exe), size).unwrap()
}

#[test]
fn requests_one_after_another_on_demand_start_one_worker_and_one_slot() {
    // Requests one after another run on one worker. Each request is
    // submitted only once the one before it has come back, so no two are ever waiting or
    // running at once: a second slot (a second thread, and with its first request a second
    // worker) is never needed.
    let pool = on_demand(TEST_WORKER, 8);
    done(pool.submit(Generation(1), sleep(0))).unwrap();
    let first = pool.pids();
    assert_eq!(first.len(), 1, "{first:?}");
    let before = threads();
    let mut grew_at = None;
    // Many: the race (a caller submitting on its result as the slot lets go of it) is
    // narrow, and a single run of a few thousand missed it one time in five.
    for i in 0..20_000u32 {
        done(pool.submit(Generation(1), echo(&[i as u8]))).unwrap();
        if grew_at.is_none() && pool.pids().len() > 1 {
            grew_at = Some(i);
        }
    }
    // Found by break-it, against an earlier version: a slot let go of its request only
    // after the outcome was delivered and its loop iteration ended, so a caller woken by
    // the outcome that submitted the next at once could find every started slot busy, and
    // `grow` started another: two slots, and two workers, for requests that never
    // overlapped (9 runs of 9). Now the slot is freed under the lock that delivers.
    assert_eq!(
        pool.pids(),
        first,
        "requests one after another started more workers (the second at request {grew_at:?})"
    );
    assert_eq!(
        threads(),
        before,
        "requests one after another started more slots"
    );
}

#[test]
fn a_warm_start_starts_the_workers_it_says_and_a_request_after_takes_one_of_them() {
    // Warmed, two slots of four start their workers ahead of any request, up once the warm
    // start returns; a request then runs on one of them, starting no slot or worker more.
    let pool = on_demand(TEST_WORKER, 4);
    assert_eq!(pool.warm(2), 2);
    let warmed = pool.pids();
    assert_eq!(warmed.len(), 2, "{warmed:?}");
    let before = threads();
    done(pool.submit(Generation(1), sleep(0))).unwrap();
    assert_eq!(pool.pids(), warmed);
    assert_eq!(threads(), before);
}

#[test]
fn a_request_while_a_warmed_worker_starts_waits_for_it_rather_than_starting_another() {
    // From the tests review: the order fixed. A worker that takes half a second to start
    // (a script that waits, then becomes the test worker). A warm start of one returns once
    // its process is started; a request then comes while the worker is still starting: the
    // slot, started and free, takes it once its worker is up, and the pool doesn't grow.
    // (Were a warming slot not counted free, the request would start a second.)
    let dir = scratch_dir("warm-slow");
    let exe = dir.join("worker");
    write_executable(
        &exe,
        &format!("#!/bin/sh\nsleep 0.5\nexec '{TEST_WORKER}' \"$@\"\n"),
    );
    for _ in 0..3 {
        let pool = on_demand(exe.to_str().unwrap(), 4);
        assert_eq!(pool.warm(1), 1);
        let warmed = pool.pids();
        assert_eq!(warmed.len(), 1, "{warmed:?}");
        done(pool.submit(Generation(1), sleep(0))).unwrap();
        assert_eq!(pool.pids(), warmed);
    }
}

#[test]
fn sessions_warm_their_pool_and_their_sketch_solvers_and_a_second_warm_starts_none() {
    // The app's call: both halves at once, each up when it returns; called again, none
    // more.
    use silico_core::sketch::SketchSolver;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let pool = std::sync::Arc::new(on_demand(TEST_WORKER, 4));
    let starts = std::sync::Arc::new(AtomicUsize::new(0));
    let counted = std::sync::Arc::clone(&starts);
    let sessions = silico_core::command::Sessions::with_stand_ins(
        std::sync::Arc::clone(&pool),
        |dir: &std::path::Path| silico_core::cache::Cache::unsealed(dir),
        move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(silico_core::fake::FakeSketches::new()) as Box<dyn SketchSolver>)
        },
    );
    // Counts that differ, so neither half can take the other's.
    assert_eq!(sessions.warm(2, 1), (2, 1));
    assert_eq!(pool.pids().len(), 2);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert_eq!(sessions.warm(2, 1), (0, 0));
    assert_eq!(pool.pids().len(), 2);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
}

#[test]
fn a_warm_start_starts_only_the_slots_not_yet_started() {
    let pool = on_demand(TEST_WORKER, 2);
    assert_eq!(pool.warm(5), 2);
    assert_eq!(pool.warm(1), 0);
    wait_full(&pool, 2, &BTreeSet::new());
    // A request first starts one slot on demand; a warm start then starts the other.
    let pool = on_demand(TEST_WORKER, 2);
    done(pool.submit(Generation(1), sleep(0))).unwrap();
    assert_eq!(pool.warm(2), 1);
    wait_full(&pool, 2, &BTreeSet::new());
    // A pool that refuses every request has no slot to start.
    let refusing = KernelPool::refusing(silico_core::Error::new(
        ErrorCode::WorkerCrashed,
        "no kernel worker",
    ));
    assert_eq!(refusing.warm(1), 0);
}

#[test]
fn a_request_after_one_that_crashed_its_worker_waits_for_the_slot_not_a_new_one() {
    // Requests one after another, every other one crashing its worker: the slot replaces it
    // (a new process) after the crash is reported, free, so the next request, submitted the
    // moment the crash is reported, waits for it rather than starting another slot.
    let pool = on_demand(TEST_WORKER, 4);
    for _ in 0..10 {
        let err =
            done(pool.submit(Generation(1), KernelRequest::debug(DebugOp::Abort))).unwrap_err();
        assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
        done(pool.submit(Generation(1), sleep(0))).unwrap();
    }
    // Found by break-it, against an earlier version, which replaced the worker with the
    // slot still counted busy: the request after the first crash grew the pool by a slot
    // and a worker (4 runs of 5), two held for work that never ran two at once.
    let pids = wait_full(&pool, pool.pids().len(), &BTreeSet::new());
    assert_eq!(pids.len(), 1, "{pids:?}");
}

#[test]
fn a_request_while_the_only_worker_is_busy_starts_a_second_worker_on_demand() {
    // A request held in the pool's one worker, and another submitted (by another client)
    // while it's held: the pool, opened on demand with room for two, starts a second slot
    // and worker for it, rather than leaving it queued behind the held one. The second
    // comes back while the first is still held, so the two ran at once, on two workers.
    let exe = fake_worker("hold");
    let pool = KernelPool::start_on_demand(&exe, 2).unwrap();
    let dir = scratch_dir("held");
    let mut hold = HOLD.to_vec();
    hold.extend_from_slice(dir.to_str().unwrap().as_bytes());
    // Allowed far longer than the test waits, so it's never stopped and replaced as a
    // timeout, which would free the slot for the second request.
    let held = pool.submit(Generation(1), echo(&hold).with_timeout(4 * PATIENCE));
    let started = dir.join("started");
    let deadline = Instant::now() + PATIENCE;
    while !started.exists() {
        assert!(
            Instant::now() < deadline,
            "the held request didn't reach its worker within {PATIENCE:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let held_pid: u32 = std::fs::read_to_string(&started).unwrap().parse().unwrap();
    assert_eq!(pool.pids(), vec![held_pid]);
    // Within PATIENCE, or `done` fails: a pool that didn't grow would leave it queued
    // behind the held request, which is never let go until it's back.
    let client = pool.client();
    assert_eq!(
        done(client.submit(Generation(1), echo(b"second"))).unwrap(),
        Blob::inline(b"second".to_vec())
    );
    let Err(held) = held.try_take() else {
        panic!("the held request ended before it was let go");
    };
    let pids: BTreeSet<u32> = pool.pids().into_iter().collect();
    assert_eq!(pids.len(), 2, "{pids:?}");
    assert!(pids.contains(&held_pid), "{pids:?} without {held_pid}");
    std::fs::write(dir.join("release"), "").unwrap();
    assert_eq!(done(held).unwrap(), Blob::inline(hold));
}

#[test]
fn bursts_from_many_threads_on_demand_never_pass_the_size_and_every_request_ends() {
    // Sixteen threads submit at once, again and again, to a pool of three opened on demand:
    // every request comes back with its own bytes, the pool never holds more than three
    // workers, and it holds three (the bursts need them all).
    let pool = on_demand(TEST_WORKER, 3);
    let before = threads();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..16u8)
            .map(|t| {
                let pool = &pool;
                scope.spawn(move || {
                    for round in 0..50u32 {
                        let pending: Vec<_> = (0..4u32)
                            .map(|k| {
                                let bytes: Vec<u8> = [t]
                                    .into_iter()
                                    .chain((round * 4 + k).to_le_bytes())
                                    .collect();
                                (bytes.clone(), pool.submit(Generation(1), echo(&bytes)))
                            })
                            .collect();
                        for (bytes, p) in pending {
                            assert_eq!(done(p).unwrap(), Blob::inline(bytes));
                        }
                        assert!(pool.pids().len() <= 3, "{:?}", pool.pids());
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    });
    assert_eq!(pool.pids().len(), 3, "{:?}", pool.pids());
    // Each slot is a thread and each worker at most a few (its log reader): the pool's
    // threads are bounded by its size, however many submitted.
    let grown = threads().saturating_sub(before);
    assert!(grown <= 3 * 4, "the pool grew {grown} threads for 3 slots");
    drop(pool);
    wait_no_children("after the drop");
}

#[test]
fn many_submitters_and_advancers_on_demand_never_cross_results() {
    // The race of `many_submitters_and_advancers_never_cross_results` on a pool whose
    // slots start as the race needs them, with superseded requests stopped at once (their
    // workers condemned, killed, and replaced while slots are still starting).
    let pool = on_demand(TEST_WORKER, 3);
    pool.set_stop_after(Duration::ZERO);
    std::thread::scope(|scope| {
        let submitters: Vec<_> = (0..8u8)
            .map(|t| {
                let pool = &pool;
                scope.spawn(move || {
                    let mut sent = Vec::new();
                    for n in 0..150u32 {
                        let bytes: Vec<u8> = [t].into_iter().chain(n.to_le_bytes()).collect();
                        let g = pool.generation().0 + u64::from(n % 3 == 0);
                        // Some sleep, so slots are busy and condemned while others start.
                        if n % 5 == 0 {
                            sent.push((
                                g,
                                None,
                                AnyPending::Unit(pool.submit(Generation(g), sleep(3))),
                            ));
                        } else {
                            sent.push((
                                g,
                                Some(bytes.clone()),
                                AnyPending::Blob(pool.submit(Generation(g), echo(&bytes))),
                            ));
                        }
                    }
                    sent
                })
            })
            .collect();
        let advancer = {
            let pool = &pool;
            scope.spawn(move || {
                for _ in 0..200 {
                    pool.advance(Generation(pool.generation().0 + 1));
                    std::thread::sleep(Duration::from_micros(300));
                }
            })
        };
        let sent: Vec<_> = submitters
            .into_iter()
            .flat_map(|s| s.join().unwrap())
            .collect();
        advancer.join().unwrap();
        let last = pool.generation();
        for (g, bytes, p) in sent {
            match p.wait() {
                Ended::Done(Ok(got)) => {
                    if let Some(bytes) = bytes {
                        assert_eq!(got, Some(Blob::inline(bytes)));
                    }
                }
                // A superseded sleep ends superseded, even if its worker was killed: never
                // an error.
                Ended::Done(Err(e)) => panic!("generation {g} (last {last:?}): {e}"),
                Ended::Superseded => assert!(g < last.0, "{g} superseded, newest {last:?}"),
            }
        }
    });
    assert!(pool.pids().len() <= 3, "{:?}", pool.pids());
    let last = pool.generation();
    assert_eq!(
        done(pool.submit(last, echo(&[42]))).unwrap(),
        Blob::inline(vec![42])
    );
    drop(pool);
    wait_no_children("after the drop");
}

/// A pending request of either kind, for the race above.
enum AnyPending {
    Unit(silico_core::pool::Pending<()>),
    Blob(silico_core::pool::Pending<Blob>),
}

enum Ended {
    Done(Result<Option<Blob>, silico_core::Error>),
    Superseded,
}

impl AnyPending {
    fn wait(self) -> Ended {
        match self {
            AnyPending::Unit(p) => match outcome(p) {
                Outcome::Done(r) => Ended::Done(r.map(|()| None)),
                Outcome::Superseded => Ended::Superseded,
            },
            AnyPending::Blob(p) => match outcome(p) {
                Outcome::Done(r) => Ended::Done(r.map(Some)),
                Outcome::Superseded => Ended::Superseded,
            },
        }
    }
}

#[test]
fn dropping_an_on_demand_pool_while_its_slots_start_stops_at_once() {
    // Eight long requests to a pool of eight with no slot started: each submit starts a
    // slot, whose first request starts its worker. A drop swept over those starts must
    // still kill every worker at once (a worker started after the drop listed the killers
    // is never given its request), end every request without a result, and leave no
    // process behind.
    // A sleep no test reaches: a drop that waited for one never returns within `WAIT`.
    const SLEEP: Duration = UNREACHED;
    for attempt in 0..40u32 {
        let pool = on_demand(TEST_WORKER, 8);
        let pending: Vec<_> = (0..8)
            .map(|_| {
                pool.submit(
                    Generation(1),
                    sleep(SLEEP.as_millis() as u64).with_timeout(UNREACHED),
                )
            })
            .collect();
        spin(Duration::from_micros(u64::from(attempt) * 250));
        promptly(&format!("attempt {attempt}: the drop"), move || drop(pool));
        for p in pending {
            let err = done(p).unwrap_err();
            assert_eq!(err.code(), ErrorCode::Internal, "attempt {attempt}: {err}");
        }
        wait_no_children(&format!("attempt {attempt}"));
    }
}

#[test]
fn an_on_demand_pool_advanced_before_any_slot_started() {
    // Nothing started: an advance has nothing to stop, a stale request is superseded
    // without starting a slot, and a current one starts the first.
    let pool = on_demand(TEST_WORKER, 4);
    pool.set_stop_after(Duration::ZERO);
    let before = threads();
    pool.advance(Generation(5));
    assert!(matches!(
        pool.submit(Generation(3), echo(&[3])).try_take().ok(),
        Some(Outcome::Superseded)
    ));
    assert_eq!(pool.pids(), Vec::<u32>::new());
    assert_eq!(threads(), before, "a superseded request started a slot");
    assert_eq!(
        done(pool.submit(Generation(5), echo(&[5]))).unwrap(),
        Blob::inline(vec![5])
    );
    assert_eq!(pool.pids().len(), 1);
}

#[test]
fn a_request_superseded_while_its_slot_starts_its_first_worker() {
    // A slot's first request is superseded around the moment its worker starts (swept over
    // that moment), with the stop-after zero: it ends superseded, never with a result or a
    // crash, and the pool goes on with one worker.
    let pool = on_demand(TEST_WORKER, 2);
    pool.set_stop_after(Duration::ZERO);
    let mut g = 1u64;
    for attempt in 0..100u32 {
        let p = pool.submit(Generation(g), sleep(2_000));
        spin(Duration::from_micros(u64::from(attempt % 50) * 200));
        g += 1;
        pool.advance(Generation(g));
        match outcome(p) {
            Outcome::Superseded => {}
            Outcome::Done(r) => panic!("attempt {attempt}: {r:?}"),
        }
        assert_eq!(
            done(pool.submit(Generation(g), echo(&[attempt as u8]))).unwrap(),
            Blob::inline(vec![attempt as u8])
        );
    }
    assert!(pool.pids().len() <= 2, "{:?}", pool.pids());
}

#[test]
fn an_on_demand_pool_whose_executable_changes_before_its_first_request() {
    // The pool's identity is the executable's as it opened. Changed before any worker
    // started, the first request must fail naming the change, not run another build
    // under the old identity; put back as it was, requests run again.
    let dir = scratch_dir("changed-before-first");
    let exe = dir.join("silico-kernel-test-worker");
    let original = format!("#!/bin/sh\nexec '{TEST_WORKER}'\n");
    write_executable(&exe, &original);
    let pool = KernelPool::start_on_demand(&exe, 2).unwrap();
    write_executable(
        &exe,
        &format!("#!/bin/sh\n# another build\nexec '{TEST_WORKER}'\n"),
    );
    let err = done(pool.submit(Generation(1), echo(&[1]))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
    assert!(err.message().contains("changed since it started"), "{err}");
    assert_eq!(pool.pids(), Vec::<u32>::new());
    write_executable(&exe, &original);
    assert_eq!(
        done(pool.submit(Generation(1), echo(&[2]))).unwrap(),
        Blob::inline(vec![2])
    );
    // Deleted: the request fails as an install that can't run, and a restore is used by
    // the next request at once (a request never waits out a slot's retry).
    let held = pool.pids();
    kill9_and_wait(held[0]);
    std::fs::remove_file(&exe).unwrap();
    let err = done(pool.submit(Generation(1), echo(&[3]))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
    write_executable(&exe, &original);
    let at = Instant::now();
    assert_eq!(
        done(pool.submit(Generation(1), echo(&[4]))).unwrap(),
        Blob::inline(vec![4])
    );
    let restored = at.elapsed();
    // Against a worker's start, now, on this machine (loaded or not): within it and less
    // than half the retry wait a failed start begins (IDLE_CHECK), not the wait as well.
    let cold = on_demand(TEST_WORKER, 1);
    let at = Instant::now();
    done(cold.submit(Generation(1), echo(&[5]))).unwrap();
    let start = at.elapsed();
    drop(cold);
    assert!(
        restored < start + IDLE_CHECK / 2,
        "the restore waited: {restored:?}, a start {start:?}"
    );
    drop(pool);
    wait_no_children("after the drop");
}

#[test]
fn failed_starts_one_after_another_start_one_slot() {
    // A worker that can't start fails each request, and frees its slot before the failure
    // is delivered: a caller that submits on it finds the slot free, and no slot more is
    // started (found by review: freed after, a few thousand failures one after another
    // grew the pool to its size).
    let dir = scratch_dir("failed-starts");
    let exe = dir.join("silico-kernel-test-worker");
    write_executable(&exe, &format!("#!/bin/sh\nexec '{TEST_WORKER}'\n"));
    let pool = on_demand(exe.to_str().unwrap(), 8);
    std::fs::remove_file(&exe).unwrap();
    assert!(done(pool.submit(Generation(1), echo(&[0]))).is_err());
    let before = threads();
    for i in 0..3000u32 {
        assert!(done(pool.submit(Generation(1), echo(&[i as u8]))).is_err());
    }
    assert_eq!(
        threads(),
        before,
        "failed starts one after another started more slots"
    );
}

#[test]
fn an_on_demand_pool_of_one_crashing_every_request_never_wedges() {
    // One slot, every other request crashing its worker: the slot's busy count must come
    // back exactly once each time; released twice, it would underflow and end the only
    // slot's thread, and a later request would wait with no slot to take it.
    let pool = on_demand(TEST_WORKER, 1);
    for i in 0..30u32 {
        let op = [DebugOp::Abort, DebugOp::Segfault, DebugOp::Garbage][(i % 3) as usize].clone();
        assert!(done(pool.submit(Generation(1), KernelRequest::debug(op))).is_err());
        assert_eq!(
            done(pool.submit(Generation(1), echo(&[i as u8]))).unwrap(),
            Blob::inline(vec![i as u8])
        );
    }
    assert_eq!(pool.pids().len(), 1);
}

// ---------------------------------------------------------------------------------------
// break-it: a worker checked against the pool's executable as it starts
// (`Worker::spawn_checked`, `Executable::spawn`, a worker's `refused`). Executables that
// change as their worker starts (another build, deleted, a directory, a FIFO), rewritten in
// place so their size, time, and inode stay as they were, or replaced by a FIFO under a
// running pool; refused workers given long requests; and many slots starting at once from
// an executable that changes under them.

/// A worker executable at `exe` that, as it starts (after the pool's check, before its
/// first frame), runs `then` (shell, with `$exe` its own path), then runs the test worker.
fn changing_as_it_starts(exe: &Path, then: &str) -> String {
    format!(
        "#!/bin/sh\n# build 1\nexe='{}'\n{then}\nexec '{TEST_WORKER}'\n",
        exe.display()
    )
}

fn mkfifo(path: &Path) {
    let status = Command::new("mkfifo").arg(path).status().unwrap();
    assert!(status.success(), "mkfifo {}", path.display());
}

/// Until `stop` is set, opens the FIFO at `path` for writing whenever a reader waits on it,
/// and closes it, so every reader blocked opening it gets an end of file: so that a test
/// that finds a hang can still let the pool go and end.
fn feed_fifo(
    path: PathBuf,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            // ENXIO while no reader has it open; opened (and closed at once) when one has.
            let _ = std::os::unix::fs::OpenOptionsExt::custom_flags(
                std::fs::OpenOptions::new().write(true),
                libc::O_NONBLOCK,
            )
            .open(&path);
            std::thread::sleep(Duration::from_millis(5));
        }
    })
}

/// Drops `pool` on another thread, saying whether the drop ended within `within`; if it
/// didn't, feeds the FIFO at `fifo` until it does, so the test can end.
fn drop_within(pool: KernelPool, within: Duration, fifo: &Path) -> bool {
    let (dropped_tx, dropped) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(pool);
        let _ = dropped_tx.send(());
    });
    let in_time = dropped.recv_timeout(within).is_ok();
    if !in_time {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let feeder = feed_fifo(fifo.to_owned(), std::sync::Arc::clone(&stop));
        let _ = dropped.recv_timeout(WAIT);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        feeder.join().unwrap();
    }
    in_time
}

/// How long a step here may take before it counts as a hang: [`HANG`], far longer than any
/// of them takes (milliseconds), however loaded the machine, and far shorter than the
/// requests' sleeps and timeouts (minutes) that a hang would wait out.
const BOUND: Duration = HANG;

// ---------------------------------------------------------------------------------------
// A refused worker given a request

#[test]
fn a_refused_worker_given_a_long_request_fails_it_at_once_not_when_it_answers() {
    // The worker's executable changes as it starts, so its start's check refuses it within
    // milliseconds of its launch. Its request is a 5 min sleep: the refusal is known long
    // before the worker could answer, so the request fails then, with it.
    // BREAKS: the refusal is read only once the worker's reply has come (or the call's time
    // run out): the request is sent to the refused worker anyway and the call waits all 5 min
    // for an answer it then throws away (a build that never answers holds the slot for the
    // request's whole timeout, 60 s by default).
    const SLEEP: Duration = Duration::from_secs(300);
    let dir = scratch_dir("refused-long-request");
    let exe = dir.join("worker");
    let other = dir.join("other-build");
    write_executable(&other, &build(2));
    write_executable(
        &exe,
        &changing_as_it_starts(
            &exe,
            &format!(
                "cp '{}' \"$exe.tmp\" && mv \"$exe.tmp\" \"$exe\"",
                other.display()
            ),
        ),
    );
    let pool = KernelPool::start_on_demand(&exe, 1).unwrap();
    let start = Instant::now();
    let err = done(pool.submit(Generation(1), sleep(SLEEP.as_millis() as u64))).unwrap_err();
    let took = start.elapsed();
    assert!(says_changed(&err, &exe), "{err}");
    assert!(
        took < BOUND,
        "the refused worker's request failed after {took:?}, once its {SLEEP:?} answer came"
    );
}

// ---------------------------------------------------------------------------------------
// Executables changed so their stamp stays

#[test]
fn an_executable_rewritten_in_place_to_another_build_of_its_size_and_time_isnt_started() {
    // Another build of the same size written into the same file (its inode kept, as `cp`
    // onto an existing file does), its modification time put back (as `cp -p` and
    // `rsync --inplace -t` do): the file's size, time, and inode are as they were, its bytes
    // aren't. A worker started from it is another build's, so it mustn't be started under
    // the pool's identity.
    // BREAKS: the start's check takes an unchanged size, time, and inode for an unchanged
    // file and never hashes it, so the other build is started and its answer taken under
    // the pool's identity (and kept so in the rebuild cache). The file's change time
    // (`st_ctime`), which no write can put back, would show it.
    let dir = scratch_dir("rewritten-in-place");
    let exe = dir.join("worker");
    write_executable(&exe, &build(1));
    let pool = KernelPool::start_workers(&exe, 1).unwrap();
    let identity = pool.identity().to_owned();
    assert!(done(pool.submit(Generation(1), sleep(1))).is_ok());
    let before = std::fs::metadata(&exe).unwrap();
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&exe)
            .unwrap();
        file.write_all(build(2).as_bytes()).unwrap();
        file.set_modified(before.modified().unwrap()).unwrap();
    }
    let after = std::fs::metadata(&exe).unwrap();
    // The premise: size, time, and inode as they were; the bytes another build's.
    assert_eq!(after.len(), before.len());
    assert_eq!(
        std::os::unix::fs::MetadataExt::ino(&after),
        std::os::unix::fs::MetadataExt::ino(&before)
    );
    assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    assert_eq!(std::fs::read_to_string(&exe).unwrap(), build(2));
    for pid in pool.pids() {
        kill9_and_wait(pid);
    }
    let got = done(pool.submit(Generation(1), sleep(1)));
    match got {
        Err(err) => assert!(says_changed(&err, &exe), "{err}"),
        Ok(()) => panic!(
            "a worker of another build ({}) answered under the pool's identity {identity}",
            exe.display()
        ),
    }
}

// ---------------------------------------------------------------------------------------
// Executables that aren't files

#[test]
fn an_executable_replaced_by_a_fifo_under_a_running_pool_fails_its_request_and_lets_the_pool_go() {
    // A FIFO renamed over the worker's executable while the pool runs (a broken install, a
    // hostile one). The next worker can't be started from it; the request that needs one
    // fails, naming the executable, and the pool still drops at once.
    // BREAKS: checking the executable before a start reads whatever the path names to hash
    // it, and opening a FIFO with no writer blocks: the slot's thread blocks for ever, the
    // request never gets an outcome, and dropping the pool (which joins its threads) hangs.
    // (The same read of a symlink to /dev/zero would read without end, until the app runs
    // out of memory: not run here.)
    let dir = scratch_dir("fifo-under-pool");
    let exe = dir.join("worker");
    write_executable(&exe, &build(1));
    let pool = KernelPool::start_workers(&exe, 1).unwrap();
    assert!(done(pool.submit(Generation(1), sleep(1))).is_ok());
    let fifo = dir.join("fifo");
    mkfifo(&fifo);
    std::fs::rename(&fifo, &exe).unwrap();
    for pid in pool.pids() {
        kill9_and_wait(pid);
    }
    let pending = pool.submit(Generation(1), echo(&[1]));
    let got = pending.wait_timeout(BOUND);
    let answered = got.is_ok();
    let dropped = drop_within(pool, BOUND, &exe);
    assert!(
        answered,
        "a request needing a worker from a FIFO had no outcome within {BOUND:?}"
    );
    assert!(
        dropped,
        "dropping the pool hung, its slot blocked reading a FIFO"
    );
    let err = got.ok().unwrap().done().unwrap().unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
    assert!(err.message().contains(&*exe.to_string_lossy()), "{err}");
    assert!(err.suggestion().is_some(), "{err}");
}

#[test]
fn a_worker_whose_executable_becomes_a_fifo_as_it_starts_is_refused_at_once() {
    // The executable is replaced by a FIFO after the pool's check, as its worker starts. The
    // start's check, at the worker's first frame, finds it no longer the pool's: the request
    // fails saying so, within moments, and the pool drops at once.
    // BREAKS: the check, on the worker's reply thread, blocks opening the FIFO, so the
    // worker's frames are never passed on: the request waits out its whole timeout and fails
    // as a `worker_timeout` saying the worker "hadn't started" (it had), with advice to try
    // again; the reply thread stays blocked for good; and the slot, replacing the worker,
    // blocks in the same read, so dropping the pool hangs.
    const TIMEOUT: Duration = Duration::from_secs(300);
    let dir = scratch_dir("fifo-as-started");
    let exe = dir.join("worker");
    write_executable(
        &exe,
        &changing_as_it_starts(&exe, "mkfifo \"$exe.fifo\" && mv \"$exe.fifo\" \"$exe\""),
    );
    let pool = KernelPool::start_on_demand(&exe, 1).unwrap();
    let start = Instant::now();
    let pending = pool.submit(Generation(1), sleep(1).with_timeout(TIMEOUT));
    let got = pending.wait_timeout(TIMEOUT + BOUND);
    let took = start.elapsed();
    let dropped = drop_within(pool, BOUND, &exe);
    let err = got
        .ok()
        .expect("no outcome even after the request's timeout")
        .done()
        .unwrap()
        .unwrap_err();
    assert!(
        took < BOUND,
        "the request failed after {took:?} ({err}), not at its worker's refusal"
    );
    assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
    assert!(err.message().contains(&*exe.to_string_lossy()), "{err}");
    assert!(
        dropped,
        "dropping the pool hung, its slot blocked reading a FIFO"
    );
}

#[test]
fn a_worker_whose_executable_becomes_a_directory_as_it_starts_is_refused_naming_it() {
    // The executable is removed and a directory put in its place as its worker starts. The
    // worker isn't used: the request fails with a structured error naming the executable,
    // with advice; no process is left; and with the build back the pool works again.
    let dir = scratch_dir("dir-as-started");
    let exe = dir.join("worker");
    let marker = dir.join("change-once");
    let script = format!(
        "#!/bin/sh\n# build 1\nif [ -e '{m}' ]; then rm '{m}'; rm '{e}'; mkdir '{e}'; fi\nexec '{TEST_WORKER}'\n",
        m = marker.display(),
        e = exe.display()
    );
    write_executable(&exe, &script);
    std::fs::write(&marker, b"").unwrap();
    let pool = KernelPool::start_on_demand(&exe, 1).unwrap();
    let start = Instant::now();
    let err = done(pool.submit(Generation(1), sleep(1))).unwrap_err();
    assert!(start.elapsed() < BOUND, "took {:?}", start.elapsed());
    assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
    assert!(err.message().contains(&*exe.to_string_lossy()), "{err}");
    assert!(err.suggestion().is_some(), "{err}");
    let deadline = Instant::now() + BOUND;
    while !pool.pids().is_empty() {
        assert!(
            Instant::now() < deadline,
            "the refused worker is still held"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // The build back, the same bytes as a new file: the pool starts workers again (after
    // the slot's wait, which a request doesn't wait for).
    std::fs::remove_dir(&exe).unwrap();
    write_executable(&exe, &script);
    assert!(done(pool.submit(Generation(1), sleep(1))).is_ok());
    drop(pool);
    wait_no_children("after the drop");
}

#[test]
fn a_worker_whose_executable_is_deleted_as_it_starts_is_refused_then_the_pool_recovers() {
    // The executable is deleted as its worker starts (a clean before a rebuild): the worker
    // isn't used, the request fails naming the executable, and once it's back (the same
    // bytes) the next request is answered.
    let dir = scratch_dir("deleted-as-started");
    let exe = dir.join("worker");
    let marker = dir.join("change-once");
    let script = format!(
        "#!/bin/sh\n# build 1\nif [ -e '{m}' ]; then rm '{m}'; rm '{e}'; fi\nexec '{TEST_WORKER}'\n",
        m = marker.display(),
        e = exe.display()
    );
    write_executable(&exe, &script);
    std::fs::write(&marker, b"").unwrap();
    let pool = KernelPool::start_on_demand(&exe, 1).unwrap();
    let err = done(pool.submit(Generation(1), sleep(1))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
    assert!(err.message().contains(&*exe.to_string_lossy()), "{err}");
    assert!(err.suggestion().is_some(), "{err}");
    write_executable(&exe, &script);
    assert!(done(pool.submit(Generation(1), sleep(1))).is_ok());
    drop(pool);
    wait_no_children("after the drop");
}

#[test]
fn an_executable_replaced_by_a_fifo_written_without_end_is_refused_without_reading_it() {
    // Found by review: a FIFO with no writer reads as empty, so no test needed the check that
    // the file is a regular one. Without it, a FIFO (opened without waiting) reads as
    // whatever is in it so far, or as nothing, and is hashed as if a file; a link to
    // /dev/zero would be read without end. Refused unread, it says why: not a regular file.
    let dir = scratch_dir("fifo-written");
    let exe = dir.join("worker");
    write_executable(&exe, &build(1));
    let pool = KernelPool::start_workers(&exe, 1).unwrap();
    assert!(done(pool.submit(Generation(1), sleep(1))).is_ok());
    std::fs::remove_file(&exe).unwrap();
    mkfifo(&exe);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = {
        let (stop, fifo) = (std::sync::Arc::clone(&stop), exe.clone());
        std::thread::spawn(move || {
            let block = vec![0; 1 << 16];
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                // Opened whenever a reader has it open, and written until that reader goes.
                let opened = std::os::unix::fs::OpenOptionsExt::custom_flags(
                    std::fs::OpenOptions::new().write(true),
                    libc::O_NONBLOCK,
                )
                .open(&fifo);
                match opened {
                    Ok(mut file) => {
                        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                            match file.write(&block) {
                                Ok(_) => {}
                                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                    std::thread::sleep(Duration::from_millis(1));
                                }
                                Err(_) => break,
                            }
                        }
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        })
    };
    for pid in pool.pids() {
        kill9_and_wait(pid);
    }
    let answer = pool.submit(Generation(1), sleep(1)).wait_timeout(BOUND);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let Ok(outcome) = answer else {
        writer.join().unwrap();
        panic!("the request waited on hashing a FIFO written without end");
    };
    let err = outcome.done().expect("superseded").unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerUnavailable, "{err}");
    assert!(err.message().contains(&*exe.to_string_lossy()), "{err}");
    assert!(
        err.cause().is_some_and(|c| c.contains("regular file")),
        "{err:?}"
    );
    drop(pool);
    writer.join().unwrap();
}

// ---------------------------------------------------------------------------------------
// Many slots at once

#[test]
fn many_slots_starting_from_an_executable_that_changes_under_them_take_no_answer() {
    // Eight slots start at once, on demand, from an executable that replaces itself with
    // another build as its first worker starts. Some workers run the first build, some the
    // other; each start races the replacement. None may be taken: every request fails as a
    // change, and none is answered under the pool's identity.
    const SLOTS: usize = 8;
    let dir = scratch_dir("many-changing");
    let exe = dir.join("worker");
    let other = dir.join("other-build");
    write_executable(&other, &build(2));
    for round in 0..5 {
        // The first build back, for the round.
        write_executable(
            &exe,
            &changing_as_it_starts(
                &exe,
                &format!(
                    "cp '{}' \"$exe.$$\" && mv \"$exe.$$\" \"$exe\"",
                    other.display()
                ),
            ),
        );
        let pool = KernelPool::start_on_demand(&exe, SLOTS).unwrap();
        let pending: Vec<_> = (0..SLOTS * 2)
            .map(|_| pool.submit(Generation(1), sleep(50)))
            .collect();
        for p in pending {
            match done(p) {
                Err(err) => assert!(says_changed(&err, &exe), "round {round}: {err}"),
                Ok(()) => panic!(
                    "round {round}: a worker's answer was taken after its executable changed"
                ),
            }
        }
        drop(pool);
        wait_no_children(&format!("round {round}"));
    }
}

// ---------------------------------------------------------------------------------------
// The pool's clients (M4.2), counting the workers the pool launched: a worker stopped and
// replaced shows as a launch more.

/// A pool of `size` test workers, all started now, each logging its launch: the pool, and
/// the log.
fn logged_pool(what: &str, size: usize) -> (KernelPool, PathBuf) {
    let (script, log) = launch_logger(&scratch_dir(what), &format!("exec '{TEST_WORKER}'"));
    let pool = KernelPool::start_workers(&script, size).unwrap();
    // A worker counts as started once its process is; its log line may come just after.
    wait_launches(&log, size);
    assert_eq!(launches(&log).len(), size, "the pool's first workers");
    (pool, log)
}

#[test]
fn another_clients_advances_never_stop_a_running_request() {
    // `a`'s 300 ms sleep, running at its generation 1, outlasts `b`'s advances to its
    // generations 2 to 10, on a pool of one that stops every superseded request at once.
    // `b`'s generations aren't `a`'s, so the sleep runs to its end on the worker it started
    // on: still the pool's one launch. (With one generation for the whole pool, `b`'s first
    // advance stopped it: `Superseded`, and a second launch for the worker put in its
    // place.)
    let (pool, log) = logged_pool("another-clients-advance", 1);
    pool.set_stop_after(Duration::ZERO);
    let (a, b) = (pool.client(), pool.client());
    a.advance(Generation(1));
    let running = a.submit(Generation(1), sleep(300));
    std::thread::sleep(Duration::from_millis(100));
    for g in 2..=10 {
        b.advance(Generation(g));
    }
    let ran = outcome(running);
    assert!(matches!(ran, Outcome::Done(Ok(()))), "{ran:?}");
    // A request after it runs on the pool's worker: a replacement would have logged its
    // launch before it could take it.
    let after = outcome(a.submit(Generation(1), echo(&[1])));
    assert!(matches!(after, Outcome::Done(Ok(_))), "{after:?}");
    assert_eq!(launches(&log).len(), 1, "{:?}", launches(&log));
    // Each client's generation is its own, and the pool's own is untouched.
    assert_eq!(a.generation(), Generation(1));
    assert_eq!(b.generation(), Generation(10));
    assert_eq!(pool.generation(), Generation(0));
}

#[test]
fn a_clients_own_advance_stops_its_running_request_and_no_others() {
    // The same sleep, but `a` advances: its sleep is superseded and stopped (its worker
    // killed, and a second launched in its place), as the pool's own requests are. `b`'s
    // request, waiting behind it at `b`'s generation 0, is left alone and runs.
    let (pool, log) = logged_pool("own-advance", 1);
    pool.set_stop_after(Duration::ZERO);
    let (a, b) = (pool.client(), pool.client());
    a.advance(Generation(1));
    let running = a.submit(Generation(1), sleep(300));
    std::thread::sleep(Duration::from_millis(100));
    let waiting = b.submit(Generation(0), echo(&[2]));
    for g in 2..=10 {
        a.advance(Generation(g));
    }
    let stopped = outcome(running);
    assert!(matches!(stopped, Outcome::Superseded), "{stopped:?}");
    assert_eq!(done(waiting).unwrap(), Blob::inline(vec![2]));
    assert_eq!(
        done(a.submit(Generation(10), echo(&[3]))).unwrap(),
        Blob::inline(vec![3])
    );
    wait_launches(&log, 2);
    assert_eq!(launches(&log).len(), 2, "{:?}", launches(&log));
}

#[test]
fn dropping_a_client_mid_request_stops_its_work_and_no_others() {
    // On a pool of two, `a`'s sleep (one no test reaches) and `b`'s 1 s sleep run at once,
    // and a second sleep of `a`'s waits. `a` is dropped: its waiting sleep is superseded at
    // once, and its running one stopped once it has run the stop-after (a sleep left to run
    // has no outcome within the wait `outcome` allows), its worker replaced (a third
    // launch). `b`'s sleep runs to its end on its worker, which the pool
    // still holds.
    let (pool, log) = logged_pool("drop-client", 2);
    let first = pool.pids();
    let (a, b) = (pool.client(), pool.client());
    let long = a.submit(
        Generation(0),
        sleep(UNREACHED.as_millis() as u64).with_timeout(UNREACHED),
    );
    let other = b.submit(Generation(0), sleep(1_000));
    let queued = a.submit(Generation(0), echo(&[1]));
    std::thread::sleep(Duration::from_millis(200));
    drop(a);
    assert!(
        matches!(queued.try_take(), Ok(Outcome::Superseded)),
        "a's waiting request wasn't superseded as a was dropped"
    );
    let stopped = outcome(long);
    assert!(matches!(stopped, Outcome::Superseded), "{stopped:?}");
    let ran = outcome(other);
    assert!(matches!(ran, Outcome::Done(Ok(()))), "{ran:?}");
    wait_launches(&log, 3);
    assert_eq!(launches(&log).len(), 3, "{:?}", launches(&log));
    // One of the first two workers, b's, is still the pool's; a's is gone.
    let now: BTreeSet<u32> = wait_full(&pool, 2, &BTreeSet::new()).into_iter().collect();
    assert_eq!(
        first.iter().filter(|p| now.contains(p)).count(),
        1,
        "{first:?} → {now:?}"
    );
    // And b goes on.
    assert_eq!(
        done(b.submit(Generation(0), echo(&[2]))).unwrap(),
        Blob::inline(vec![2])
    );
}

#[test]
fn a_clients_submits_racing_its_pools_drop_leave_no_worker_or_thread_behind() {
    // A client starts submitting as fast as its requests end, and its pool (starting its
    // workers on demand) is dropped 0 to 50 µs after, from another thread, so the drop lands
    // across the client's first submits and the slot they start. Each request runs or ends
    // saying the pool stopped; and once the client's thread has ended, the process has the
    // threads it had before (the pool's own joined by its drop, a slot a submit started as
    // the pool stopped joined by that submit), and no worker is left.
    drop(pool(TEST_WORKER, 1));
    let before = threads();
    for attempt in 0..200u64 {
        let pool = KernelPool::start_on_demand(Path::new(TEST_WORKER), 4).unwrap();
        let client = pool.client();
        let started = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let submitter = scope.spawn(|| {
                started.wait();
                loop {
                    match outcome(client.submit(Generation(0), echo(&[1]))) {
                        Outcome::Done(Ok(_)) => {}
                        Outcome::Done(Err(e)) => {
                            assert_eq!(
                                e.message(),
                                "the kernel pool stopped before Echo was done",
                                "attempt {attempt}: {e}"
                            );
                            return;
                        }
                        Outcome::Superseded => panic!("attempt {attempt}: superseded"),
                    }
                }
            });
            started.wait();
            spin(Duration::from_micros(attempt % 51));
            drop(pool);
            submitter.join().unwrap();
        });
        drop(client);
        // `ps` can list a thread an instant after it's joined.
        let deadline = Instant::now() + WAIT;
        while threads() > before && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            threads() <= before,
            "attempt {attempt}: threads {before} → {}",
            threads()
        );
    }
    wait_no_children("after the pools and their clients");
}

// break-it (warm start): warm starts racing each other and requests, of workers that can't
// start or whose binary changed after the pool opened.

#[test]
fn warm_starts_from_many_threads_at_once_start_each_slot_once_and_no_more() {
    let pool = std::sync::Arc::new(on_demand(TEST_WORKER, 4));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let pool = std::sync::Arc::clone(&pool);
            std::thread::spawn(move || pool.warm(usize::MAX))
        })
        .collect();
    let started: usize = threads.into_iter().map(|t| t.join().unwrap()).sum();
    assert_eq!(started, 4, "eight warm starts at once of a pool of four");
    let warmed = wait_full(&pool, 4, &BTreeSet::new());
    // Eight requests at once run on the four: no slot or worker more.
    let pending: Vec<_> = (0..8u8)
        .map(|i| pool.submit(Generation(1), echo(&[i])))
        .collect();
    for (i, p) in pending.into_iter().enumerate() {
        assert_eq!(
            done(p).unwrap(),
            Blob::inline(vec![u8::try_from(i).unwrap()])
        );
    }
    assert_eq!(pool.pids(), warmed);
    assert_eq!(pool.warm(usize::MAX), 0);
}

#[test]
fn a_warm_start_of_a_worker_that_exits_at_once_leaves_a_request_failed_not_waiting() {
    let dir = scratch_dir("warm-exits");
    let exe = dir.join("worker");
    write_executable(&exe, "#!/bin/sh\nexit 3\n");
    let pool = on_demand(exe.to_str().unwrap(), 1);
    // Its process starts and exits: counted started or not, as the exit is seen before or
    // after the start; the requests fail promptly either way, which is what's checked.
    pool.warm(1);
    for byte in 0..3u8 {
        let err = pool
            .submit(Generation(1), echo(&[byte]))
            .wait_timeout(WAIT)
            .unwrap_or_else(|_| panic!("request {byte} waited on a warm start that can't start"))
            .done()
            .expect("superseded")
            .unwrap_err();
        assert!(
            matches!(
                err.code(),
                ErrorCode::WorkerCrashed | ErrorCode::WorkerUnavailable
            ),
            "{err}"
        );
    }
}

#[test]
fn a_warm_start_after_the_workers_binary_changed_leaves_each_request_failed_promptly() {
    let dir = scratch_dir("warm-changed");
    let exe = dir.join("worker");
    std::fs::copy(TEST_WORKER, &exe).unwrap();
    let pool = on_demand(exe.to_str().unwrap(), 1);
    // Another build put in its place after the pool opened: the warm start's worker isn't
    // taken, and each request fails at once rather than waiting on it.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&exe)
        .unwrap()
        .write_all(b"\0")
        .unwrap();
    // The changed binary's worker isn't taken: none warmed.
    assert_eq!(pool.warm(1), 0);
    let err = pool
        .submit(Generation(1), echo(&[1]))
        .wait_timeout(WAIT)
        .unwrap_or_else(|_| panic!("the request waited on a warm start that can't start"))
        .done()
        .expect("superseded")
        .unwrap_err();
    assert!(!err.to_string().is_empty());
    // And the next, as promptly: no worker of the changed binary is ever taken.
    pool.submit(Generation(1), echo(&[2]))
        .wait_timeout(WAIT)
        .unwrap_or_else(|_| panic!("the second request waited"))
        .done()
        .expect("superseded")
        .unwrap_err();
    assert!(pool.pids().is_empty(), "{:?}", pool.pids());
}
