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

#[path = "common/pool.rs"]
mod pool;
#[path = "common/process.rs"]
mod process;

use std::collections::BTreeSet;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use pool::{
    PATIENCE, TEST_WORKER, WORKER, assert_reaped, children, done, echo, kill9_and_wait, outcome,
    pool, signal, sleep, spin, state, wait_full, wait_no_children,
};
use process::{open_fds, scratch_dir, threads};
use silico_core::ErrorCode;
use silico_core::kernel::{Blob, KernelRequest, Primitive};
use silico_core::pool::{KernelPool, Outcome};
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
        (_, op) => silico_kernel::worker::handle(op),
    }
}

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
    // request (a 10 s sleep) would. Swept over the moment of the drop, so attempts land in
    // that window.
    for attempt in 0..120u32 {
        let pool = pool(TEST_WORKER, 1);
        let pid = pool.pids()[0];
        kill9_and_wait(pid);
        let pending = pool.submit(Generation(1), sleep(10_000));
        spin(Duration::from_micros(u64::from(attempt % 60) * 100));
        let mut held = pool.pids();
        held.push(pid);
        let start = Instant::now();
        drop(pool);
        let took = start.elapsed();
        assert_reaped(&held, &format!("attempt {attempt}"));
        // The drop may land while the slot is starting the replacement worker, before its
        // killer is listed; the slot checks for a stop once it has, so the new worker never
        // runs the 10 s request (it did, and the drop waited for it).
        assert!(
            took < Duration::from_secs(3),
            "attempt {attempt}: dropping the pool took {took:?}"
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
        assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
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
        assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
        assert!(err.message().contains(&*exe.to_string_lossy()), "{err}");
        assert!(err.cause().is_some(), "{err}");
    }
    assert!(pool.pids().is_empty(), "{:?}", pool.pids());
    std::os::unix::fs::symlink(TEST_WORKER, &exe).unwrap();
    assert!(done(pool.submit(Generation(1), sleep(1))).is_ok());
    let held = wait_full(&pool, 2, &before.iter().copied().collect());
    drop(pool);
    assert_reaped(&held, "after the drop");
    wait_no_children("after the drop");
}

#[test]
fn a_pool_whose_binary_comes_back_after_many_failed_starts_fills_at_once() {
    // While the binary is missing, each failed start doubles its slot's wait, toward a
    // minute. Once a worker answers, the binary is back: every slot starts one at its next
    // check, not after its wait.
    let dir = scratch_dir("restored-late");
    let exe = dir.join("silico-kernel-test-worker");
    std::os::unix::fs::symlink(TEST_WORKER, &exe).unwrap();
    let pool = KernelPool::start_workers(&exe, 2).unwrap();
    std::fs::remove_file(&exe).unwrap();
    let before = pool.pids();
    for &pid in &before {
        kill9_and_wait(pid);
    }
    // In pairs, so both slots fail to start workers.
    for _ in 0..8 {
        let pair = [
            pool.submit(Generation(1), sleep(1)),
            pool.submit(Generation(1), sleep(1)),
        ];
        for pending in pair {
            assert!(done(pending).is_err());
        }
    }
    std::os::unix::fs::symlink(TEST_WORKER, &exe).unwrap();
    assert!(done(pool.submit(Generation(1), sleep(1))).is_ok());
    let held = wait_full(&pool, 2, &before.iter().copied().collect());
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
    let start = Instant::now();
    let err = done(pool.submit(
        Generation(1),
        echo(&[1]).with_timeout(Duration::from_millis(300)),
    ))
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerTimeout, "{err}");
    assert!(
        start.elapsed() < Duration::from_secs(5),
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
        .map(|_| pool.submit(Generation(1), echo(&[1])))
        .collect();
    std::thread::sleep(Duration::from_millis(100));
    let start = Instant::now();
    drop(pool);
    assert_reaped(&pids, "after the drop");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
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
    let deadline = Instant::now() + Duration::from_secs(5);
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
/// descriptor 3, which it may copy).
fn not_allowed(
    round: usize,
    pids: &[u32],
    allowed: impl Fn(&Descriptor, Option<&String>) -> bool,
) -> Vec<Descriptor> {
    let open = sockets_and_pipes(pids);
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

/// Starts a pool on another thread, failing the test if it hasn't started within ten
/// seconds (a pool of a few workers starts in well under one).
fn start_within(exe: &Path, size: usize) -> KernelPool {
    let (tx, rx) = std::sync::mpsc::channel();
    let exe = exe.to_owned();
    std::thread::spawn(move || {
        let _ = tx.send(KernelPool::start_workers(&exe, size));
    });
    match rx.recv_timeout(Duration::from_secs(10)) {
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
    // each worker is given up to five seconds to start running.
    let size = std::thread::available_parallelism()
        .map_or(8, usize::from)
        .max(8);
    for round in 0..40 {
        let pool = start_within(Path::new(WORKER), size);
        let pids = pool.pids();
        let deadline = Instant::now() + Duration::from_secs(5);
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
    write_executable(&exe, "#!/bin/sh\nexec sleep 30\n");
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
    let deadline = Instant::now() + Duration::from_secs(5);
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
    let start = Instant::now();
    let err = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(150));
            killer.kill();
        });
        worker
            .call(
                Generation(0),
                Op::Debug(DebugOp::Sleep { millis: 20_000 }),
                Duration::from_secs(60),
            )
            .unwrap_err()
    });
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    assert!(err.cause().unwrap().contains("SIGKILL"), "{err}");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
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
    assert!(
        start.elapsed() < Duration::from_secs(30),
        "{:?}",
        start.elapsed()
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
    // 40 requests submitted and their `Pending`s dropped at once: they still run (there's
    // no cancelling but `advance`), and a request after them is answered once they've run.
    let pool = pool(TEST_WORKER, 2);
    for _ in 0..40 {
        drop(pool.submit(Generation(1), sleep(10)));
    }
    let start = Instant::now();
    assert_eq!(
        done(pool.submit(Generation(1), echo(&[4]))).unwrap(),
        Blob::inline(vec![4])
    );
    // 40 × 10 ms on two workers: a jammed pool would never answer, and one that stalls on
    // each abandoned request would take seconds. (Bounds are against what they rule out, not
    // how fast a loaded machine is.)
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
    // And `advance` drops what's still queued: a request after it waits at most for the two
    // sleeps already running, where the queue would take 40 × 500 ms on two workers, 10 s.
    for _ in 0..40 {
        drop(pool.submit(Generation(1), sleep(500)));
    }
    pool.advance(Generation(2));
    let start = Instant::now();
    done(pool.submit(Generation(2), echo(&[5]))).unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn every_idle_worker_killed_at_once_is_replaced_every_time() {
    // Every worker of a production-sized pool killed while idle, 5 times. The slots' idle
    // checks replace them all within about a second, each on its own clock (which drift
    // apart enough that these starts rarely overlap the way the pool's first ones do).
    let size = std::thread::available_parallelism()
        .map_or(8, usize::from)
        .max(8);
    let pool = start_within(Path::new(TEST_WORKER), size);
    let mut gone = BTreeSet::new();
    for round in 0..5 {
        let before = wait_full(&pool, size, &gone);
        let status = Command::new("kill")
            .arg("-9")
            .args(before.iter().map(u32::to_string))
            .status()
            .unwrap();
        assert!(status.success());
        gone.extend(before);
        let after = wait_full(&pool, size, &gone);
        assert_eq!(after.len(), size, "round {round}");
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
    // request ends Superseded at once, not when its 5 s sleep would have, and the worker is
    // replaced.
    let pool = pool(TEST_WORKER, 1);
    let before = pool.pids();
    let slow = pool.submit(Generation(1), sleep(5_000));
    std::thread::sleep(Duration::from_millis(100));
    let start = Instant::now();
    pool.advance(Generation(2));
    assert!(matches!(outcome(slow), Outcome::Superseded));
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
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
    let dir = std::env::temp_dir().join(format!("silico-pool-dies-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("worker.sh");
    let log = dir.join("launches");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho launched >> '{}'\necho 'no OCCT here' >&2\nexit 3\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let launches = || std::fs::read_to_string(&log).map_or(0, |l| l.lines().count());
    let pool = KernelPool::start_workers(&script, 4).unwrap();
    // Each slot waits twice as long each time (1 s, 2 s, 4 s): between 4 s and 8 s idle,
    // about one launch a slot, where starting one every second would be four.
    std::thread::sleep(Duration::from_secs(4));
    let early = launches();
    std::thread::sleep(Duration::from_secs(4));
    let late = launches() - early;
    assert!(late <= 8, "{late} launches in 4 s idle, {early} before");
    let err = done(pool.submit(Generation(1), echo(&[1]))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    assert!(
        err.cause()
            .is_some_and(|c| c.contains("exited with code 3") && c.contains("no OCCT here")),
        "{err}"
    );
    drop(pool);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A worker script that logs the time of each launch to a file, then runs `then`.
fn launch_logger(dir: &Path, then: &str) -> (PathBuf, PathBuf) {
    let script = dir.join("worker.sh");
    let log = dir.join("launches");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nperl -MTime::HiRes=time -e 'print time, \"\\n\"' >> '{}'\n{then}\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, log)
}

/// The launch times a [`launch_logger`] script logged, and the gaps between them.
fn launches(log: &Path) -> (Vec<f64>, Vec<f64>) {
    let times: Vec<f64> = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|l| l.trim().parse().unwrap())
        .collect();
    let gaps = times.windows(2).map(|w| w[1] - w[0]).collect();
    (times, gaps)
}

#[test]
fn an_idle_slot_whose_worker_dies_at_start_waits_longer_each_time() {
    // One slot, its worker ending at once each time: it's started again after waits that
    // double from 1 s (each after a tick to notice the end), so launches at about 0, 2, 5,
    // 10, and 19 s. Exactly four in 16 s, each gap longer than the one before: not never,
    // not every tick, not a fixed wait, and not a wait that grows by a tick each time
    // (launches at 0, 2, 5, 9, and 14 s).
    let dir = scratch_dir("backoff");
    let (script, log) = launch_logger(&dir, "exit 3");
    let pool = KernelPool::start_workers(&script, 1).unwrap();
    std::thread::sleep(Duration::from_secs(16));
    drop(pool);
    let (times, gaps) = launches(&log);
    assert_eq!(times.len(), 4, "launches at {times:?}");
    for pair in gaps.windows(2) {
        assert!(pair[1] > pair[0], "gaps {gaps:?} don't grow");
    }
}

#[test]
fn an_idle_slot_whose_worker_is_killed_at_launch_backs_off_after_the_second_kill() {
    // SIGKILL before the worker starts, every time, as macOS kills a binary whose code
    // signature is broken. The first may be a kill from outside, so it's replaced at once;
    // after the second in a row the slot waits as for a binary that can't start. Launches
    // at about 0, 1, 3, 6, and 11 s (20 next): exactly five in 16 s, the gaps growing.
    // Not every tick (16), and not waiting from the first kill (0, 2, 5, 10).
    let dir = scratch_dir("killed-at-launch");
    let (script, log) = launch_logger(&dir, "kill -KILL $$");
    let pool = KernelPool::start_workers(&script, 1).unwrap();
    std::thread::sleep(Duration::from_secs(16));
    drop(pool);
    let (times, gaps) = launches(&log);
    assert_eq!(times.len(), 5, "launches at {times:?}");
    for pair in gaps.windows(2) {
        assert!(pair[1] > pair[0], "gaps {gaps:?} don't grow");
    }
}

#[test]
fn an_idle_slot_whose_worker_starts_then_exits_a_tick_later_waits_longer_each_time() {
    // The worker says it has started, runs past an idle check (seen running), then exits on
    // its own at 1.5 s. Its having run doesn't end the slot's wait: only an answer does.
    // Launches at about 0, 3, and 7 s (13 next): three in 10.5 s, not one every 3 s.
    let dir = scratch_dir("starts-runs-exits");
    let (script, log) = launch_logger(&dir, "printf '\\0\\0\\0\\0' >&3\nsleep 1.5\nexit 3");
    let pool = KernelPool::start_workers(&script, 1).unwrap();
    std::thread::sleep(Duration::from_millis(10_500));
    drop(pool);
    let (times, _) = launches(&log);
    assert_eq!(times.len(), 3, "launches at {times:?}");
}

#[test]
fn a_kill_at_launch_after_an_exit_of_its_own_is_taken_as_from_outside() {
    // The first launch exits on its own (code 3) before it starts; every later one is
    // killed (SIGKILL) before it starts. The first kill follows an exit, not a kill, so it's
    // taken as from outside and replaced at once; the second in a row is waited on. Launches
    // at about 0, 2, 3, and 6 s: the second gap about one check, the third longer.
    let dir = scratch_dir("exit-then-kills");
    let (script, log) = launch_logger(
        &dir,
        &format!(
            "if mkdir '{}' 2>/dev/null; then exit 3; fi\nkill -KILL $$",
            dir.join("first").display()
        ),
    );
    let pool = KernelPool::start_workers(&script, 1).unwrap();
    std::thread::sleep(Duration::from_millis(7_000));
    drop(pool);
    let (times, gaps) = launches(&log);
    assert!(times.len() >= 4, "launches at {times:?}");
    assert!(gaps[1] < 1.8, "the first kill was waited on: gaps {gaps:?}");
    assert!(
        gaps[2] > gaps[1] + 0.5,
        "the second kill wasn't: gaps {gaps:?}"
    );
}

#[test]
fn an_answer_ends_a_slots_wait() {
    // While `fail` exists, a launch exits on its own before it starts; otherwise it's the
    // test worker. The slot's wait grows (launches at about 0, 2, and 5 s, the next wait 4
    // s), then a worker starts and answers a request, refusing it. Killed after `fail` is
    // back, it's replaced at once and that one fails: the next launch comes after a fresh
    // wait (about 2 s later), not after the one grown before the answer (4 s, about 5 s
    // later).
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
    std::thread::sleep(Duration::from_millis(5_500));
    std::fs::remove_file(&fail).unwrap();
    // A refusal is an answer too: the worker reads the bytes, finds no shape in them, and
    // says so.
    let garbage = silico_protocol::ShapeData::unchecked(Blob::inline(b"not a shape".to_vec()));
    let refused = done(pool.submit(Generation(1), KernelRequest::summarize(&garbage))).unwrap_err();
    assert_eq!(refused.code(), ErrorCode::InvalidInput, "{refused}");
    std::fs::write(&fail, "").unwrap();
    let killed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    kill9_and_wait(pool.pids()[0]);
    std::thread::sleep(Duration::from_secs(5));
    drop(pool);
    let (times, _) = launches(&log);
    let after: Vec<f64> = times.iter().copied().filter(|t| *t > killed_at).collect();
    assert!(
        after.len() >= 2,
        "launches at {times:?}, killed at {killed_at}"
    );
    assert!(
        after[1] - after[0] < 3.0,
        "the wait from before the answer: launches at {times:?}, killed at {killed_at}"
    );
}

#[test]
fn a_wait_ended_early_by_a_newer_worker_still_doubles_on_the_next_failure() {
    // Two slots, A and B, of a script that runs the test worker as a child (so killing the
    // child makes the script exit 3, on its own) when `mode` says work, and exits 3 at once
    // when it says fail. A fails until its wait is 8 s; then B's worker is killed from
    // outside and replaced by one that works, which ends A's wait early. A's worker then
    // fails again: its wait doubles to 16 s, not back to 1 s, so no launch follows within
    // 5 s.
    let dir = scratch_dir("moot-keeps");
    let log = dir.join("launches");
    let mode = dir.join("mode");
    let children = dir.join("children");
    let script = dir.join("worker.sh");
    std::fs::write(&mode, "work").unwrap();
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nm=$(cat '{}')\nperl -MTime::HiRes=time -e 'print time, \"\\n\"' >> '{}'\n\
             if [ \"$m\" = work ]; then '{TEST_WORKER}' & echo \"$$ $!\" >> '{}'; wait; fi\n\
             exit 3\n",
            mode.display(),
            log.display(),
            children.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let count = || {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .count()
    };
    let pool = KernelPool::start_workers(&script, 2).unwrap();
    let pids = pool.pids();
    assert_eq!(pids.len(), 2);
    let (a, b) = (pids[0], pids[1]);
    // Both scripts have read `mode` (work) and started their workers, however slowly the
    // machine runs them.
    let child_of = |script: u32| {
        std::fs::read_to_string(&children)
            .unwrap_or_default()
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{script} ")).map(str::to_owned))
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let a_child = loop {
        if let (Some(child), Some(_)) = (child_of(a), child_of(b)) {
            break child;
        }
        assert!(Instant::now() < deadline, "the workers never started");
        std::thread::sleep(Duration::from_millis(5));
    };
    std::fs::write(&mode, "fail").unwrap();
    signal("KILL", a_child.trim().parse().unwrap());
    // A: its exit seen at about 1 s, launches at about 2, 5, and 10 s, its wait 8 s from
    // about 11 s (19 next).
    std::thread::sleep(Duration::from_millis(11_500));
    assert_eq!(count(), 5, "launches: {}", count());
    std::fs::write(&mode, "work").unwrap();
    signal("KILL", b);
    // B's replacement (launch 6) works; A's next launch (7), early, must fail.
    let deadline = Instant::now() + Duration::from_secs(6);
    while count() < 6 {
        assert!(Instant::now() < deadline, "B wasn't replaced");
        std::thread::sleep(Duration::from_millis(5));
    }
    std::fs::write(&mode, "fail").unwrap();
    // Well before A's own wait would end (about 7 s from here).
    let deadline = Instant::now() + Duration::from_secs(5);
    while count() < 7 {
        assert!(Instant::now() < deadline, "A's wait didn't end early");
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_secs(5));
    drop(pool);
    let (times, gaps) = launches(&log);
    assert_eq!(times.len(), 7, "gaps {gaps:?}");
    // A's early launch (7) came well before its own 8 s wait from its last failure (launch
    // 5) would have run out, about 9 s after it, however slowly the machine ran.
    assert!(
        times[6] - times[4] < 6.0,
        "A's wait didn't end early: gaps {gaps:?}"
    );
}

#[test]
fn a_request_refused_before_its_sent_doesnt_end_a_slots_wait() {
    // Launches at about 0, 2, and 5 s (the wait 2 s). A request over the frame limit just
    // after the third starts a worker (which fails: launch 4) and is refused before it's
    // sent: no answer, so the next wait is 4 s (a launch about 5 s after the refusal), not
    // 1 s (about 2 s after). Still four launches 3.5 s after the refusal.
    let dir = scratch_dir("refusal-keeps-wait");
    let (script, log) = launch_logger(&dir, "exit 3");
    let count = || {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .count()
    };
    let pool = KernelPool::start_workers(&script, 1).unwrap();
    // The third launch, however slowly its script logs it.
    std::thread::sleep(Duration::from_millis(5_000));
    let deadline = Instant::now() + Duration::from_secs(10);
    while count() < 3 {
        assert!(Instant::now() < deadline, "only {} launches", count());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(count(), 3, "launches before the request");
    // Its script logs, then exits: time to exit, well short of the idle check (1 s) that
    // sees it, so the request starts a worker of its own.
    std::thread::sleep(Duration::from_millis(200));
    let too_large = vec![0; silico_protocol::MAX_FRAME_LEN as usize + 1];
    let err =
        done(pool.submit(Generation(1), KernelRequest::echo(Blob::inline(too_large)))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    // The request's own launch: soon, not the slot's next, seconds later.
    let deadline = Instant::now() + Duration::from_secs(2);
    while count() < 4 {
        assert!(Instant::now() < deadline, "the request started no worker");
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(3_500));
    drop(pool);
    let (times, gaps) = launches(&log);
    assert_eq!(times.len(), 4, "a refusal ended the wait: gaps {gaps:?}");
}

#[test]
fn a_worker_started_before_a_slots_failure_doesnt_end_its_wait() {
    // Two slots: the first launch is a worker that starts 3 s late; every other ends at
    // once. The failing slot launches at about 0, 2, and 5 s (10 next). The slow worker
    // starting at 3 s says nothing about the binary since the failure (it was launched
    // before it), so it doesn't end the wait: four launches in 8.5 s, not five or more.
    let dir = scratch_dir("older-start");
    let (script, log) = launch_logger(
        &dir,
        &format!(
            "if mkdir '{}' 2>/dev/null; then sleep 3; exec '{TEST_WORKER}'; fi\nexit 3",
            dir.join("slow").display()
        ),
    );
    let pool = KernelPool::start_workers(&script, 2).unwrap();
    std::thread::sleep(Duration::from_millis(8_500));
    drop(pool);
    let (times, _) = launches(&log);
    assert_eq!(times.len(), 4, "launches at {times:?}");
}

#[test]
fn a_slot_waiting_out_a_long_back_off_starts_a_worker_once_a_newer_one_has_started() {
    // A slot whose worker can't be started (its binary missing) waits longer each time:
    // failures at about 1, 2, 4, and 8 s, the next at 16. With the binary back at 8.5 s
    // and the other slot's worker killed then, that slot starts a worker at once (a worker
    // killed after it started is replaced now); once it has started, the first slot stops
    // waiting, so the pool is full within 5 s, before its wait would end.
    let dir = scratch_dir("waits-for-newer");
    let exe = dir.join("silico-kernel-test-worker");
    std::os::unix::fs::symlink(TEST_WORKER, &exe).unwrap();
    let pool = KernelPool::start_workers(&exe, 2).unwrap();
    // Time for both workers to start (about 25 ms).
    std::thread::sleep(Duration::from_secs(1));
    let before = pool.pids();
    std::fs::remove_file(&exe).unwrap();
    kill9_and_wait(before[0]);
    std::thread::sleep(Duration::from_millis(8_500));
    std::os::unix::fs::symlink(TEST_WORKER, &exe).unwrap();
    kill9_and_wait(before[1]);
    let held = wait_full(&pool, 2, &before.iter().copied().collect());
    drop(pool);
    assert_reaped(&held, "after the drop");
    wait_no_children("after the drop");
}
