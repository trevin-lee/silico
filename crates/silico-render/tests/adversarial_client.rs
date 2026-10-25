//! Hostile use of the core's render client (`silico_core::render`): a client dropped with
//! renders queued and running, its worker killed or stopped from outside while idle,
//! cancels racing the render they cancel, many threads rendering and cancelling at once,
//! its executable removed or swapped after it started, settings at their ends, renders
//! cancelled only to keep the worker from going idle, and a scene of few faces and many
//! triangles that holds the worker's memory.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::render::{RenderClient, RenderOutcome, RenderSettings, RenderTicket, Rendered};
use silico_protocol::{Mesh, RenderScene};
use silico_render::bench::front_of_a_box;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-render-worker");
const TEST_WORKER: &str = env!("CARGO_BIN_EXE_silico-render-test-worker");

fn client(exe: &str, settings: RenderSettings) -> RenderClient {
    RenderClient::start(Path::new(exe), settings).unwrap()
}

fn drawn(outcome: RenderOutcome) -> Rendered {
    match outcome {
        RenderOutcome::Done(Ok(rendered)) => rendered,
        other => panic!("not drawn: {other:?}"),
    }
}

fn failed(outcome: RenderOutcome) -> silico_core::Error {
    match outcome {
        RenderOutcome::Done(Err(e)) => e,
        other => panic!("not failed: {other:?}"),
    }
}

/// A scene the test worker hangs on (for 600 s), and one it aborts on.
fn hanging() -> RenderScene {
    front_of_a_box(1, 7)
}

fn crashing() -> RenderScene {
    front_of_a_box(1, 9)
}

fn small() -> RenderScene {
    front_of_a_box(8, 8)
}

fn process_exists(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string()])
        .output()
        .unwrap()
        .status
        .success()
}

fn signal(pid: u32, sig: &str) {
    let ok = std::process::Command::new("kill")
        .args([sig, &pid.to_string()])
        .status()
        .unwrap()
        .success();
    assert!(ok, "kill {sig} {pid}");
}

fn eventually(limit: Duration, done: impl Fn() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

/// The ticket's outcome, or None if it doesn't come within `limit` (the wait left running on
/// a thread of its own).
fn wait_within(ticket: RenderTicket, limit: Duration) -> Option<RenderOutcome> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(ticket.wait());
    });
    rx.recv_timeout(limit).ok()
}

/// A scratch directory of this test's own under the system's temporary directory.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "silico-adversarial-client-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ---- Dropping the client ----

#[test]
fn dropping_the_client_with_renders_queued_behind_a_hung_one_returns_promptly() {
    // BREAKS: dropping the client closes its queue, but the thread drains it, sending every
    // queued render to the worker in turn, each its full timeout: three renders queued behind
    // a hung one hold the drop (and whoever drops it: a `Sessions` closing, the app quitting)
    // for four timeouts, 8 s here, 2 minutes at the default 30 s. The design's session
    // closing drops queued renders and kills the running one's worker; a client dropped
    // should do no less.
    let c = client(
        TEST_WORKER,
        RenderSettings {
            timeout: Duration::from_secs(2),
            ..RenderSettings::default()
        },
    );
    let tickets: Vec<RenderTicket> = (0..4).map(|_| c.render(hanging())).collect();
    assert!(eventually(Duration::from_secs(10), || c.pid().is_some()));
    let pid = c.pid().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let start = Instant::now();
    drop(c);
    let took = start.elapsed();
    // Every ticket has its outcome by now, whatever it is.
    for t in tickets {
        assert!(
            wait_within(t, Duration::from_millis(100)).is_some(),
            "a ticket still waits after the client was dropped"
        );
    }
    assert!(
        eventually(Duration::from_secs(5), || !process_exists(pid)),
        "pid {pid} still runs after the client was dropped"
    );
    assert!(
        took < Duration::from_secs(1),
        "dropping the client took {took:?}"
    );
}

#[test]
fn dropping_the_client_with_only_a_warm_worker_stops_it_at_once() {
    let c = client(WORKER, RenderSettings::default());
    drawn(c.render(small()).wait());
    let pid = c.pid().unwrap();
    let start = Instant::now();
    drop(c);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    assert!(
        eventually(Duration::from_secs(5), || !process_exists(pid)),
        "pid {pid} still runs"
    );
}

// ---- The worker ended or stopped from outside ----

#[test]
fn a_worker_killed_while_idle_is_replaced_and_the_next_render_draws() {
    let c = client(WORKER, RenderSettings::default());
    drawn(c.render(small()).wait());
    let pid = c.pid().unwrap();
    signal(pid, "-KILL");
    assert!(eventually(Duration::from_secs(5), || !process_exists(pid)
        || zombie(pid)));
    // Not a crash of this render's: the worker was gone before it was sent.
    drawn(c.render(small()).wait());
    assert!(c.pid().is_some_and(|p| p != pid));
}

/// Whether `pid` is a zombie (exited, not yet reaped by the client).
fn zombie(pid: u32) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().starts_with('Z')
}

#[test]
fn a_worker_stopped_with_sigstop_while_idle_times_out_and_is_killed() {
    let c = client(
        WORKER,
        RenderSettings {
            timeout: Duration::from_secs(2),
            ..RenderSettings::default()
        },
    );
    drawn(c.render(small()).wait());
    let pid = c.pid().unwrap();
    signal(pid, "-STOP");
    let start = Instant::now();
    let outcome = wait_within(c.render(small()), Duration::from_secs(10))
        .expect("a render on a stopped worker never answered");
    let e = failed(outcome);
    assert_eq!(e.code(), ErrorCode::WorkerTimeout, "{e:?}");
    assert!(
        start.elapsed() < Duration::from_secs(6),
        "{:?}",
        start.elapsed()
    );
    assert!(
        eventually(Duration::from_secs(5), || !process_exists(pid)),
        "the stopped worker {pid} was left"
    );
    drawn(c.render(small()).wait());
}

#[test]
fn a_crash_with_renders_queued_behind_it_fails_only_itself() {
    let c = client(TEST_WORKER, RenderSettings::default());
    let crash = c.render(crashing());
    let after: Vec<RenderTicket> = (0..3).map(|_| c.render(small())).collect();
    assert_eq!(failed(crash.wait()).code(), ErrorCode::WorkerCrashed);
    for t in after {
        drawn(t.wait());
    }
}

// ---- Cancels racing their render ----

#[test]
fn a_render_cancelled_at_once_is_cancelled_or_drawn_never_failed() {
    // Cancelled straight after it's asked, after it's sent, or as it answers: whichever the
    // cancel lands in, the render is cancelled or drawn, never a failure, and the next one
    // draws.
    let c = client(WORKER, RenderSettings::default());
    for i in 0..40u64 {
        let t = c.render(small());
        if i % 4 != 0 {
            std::thread::sleep(Duration::from_micros(i * 500));
        }
        t.cancel();
        match wait_within(t, Duration::from_secs(30)).expect("no outcome") {
            RenderOutcome::Cancelled | RenderOutcome::Done(Ok(_)) => {}
            RenderOutcome::Done(Err(e)) => panic!("cancel {i} failed the render: {e:?}"),
        }
    }
    drawn(c.render(small()).wait());
}

#[test]
fn a_cancel_twice_and_a_cancel_after_the_answer_do_nothing_more() {
    let c = client(WORKER, RenderSettings::default());
    let t = c.render(small());
    t.cancel();
    t.cancel();
    assert!(matches!(
        t.wait(),
        RenderOutcome::Cancelled | RenderOutcome::Done(Ok(_))
    ));
    // A render whose outcome has come, then cancelled (the ticket kept): the worker stays.
    let t = c.render(small());
    let pid = loop {
        if let Some(pid) = c.pid() {
            break pid;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let next = c.render(small());
    drawn(next.wait());
    // `t` answered before `next` did: cancelling it now kills nothing.
    t.cancel();
    drawn(t.wait());
    assert_eq!(c.pid(), Some(pid));
    drawn(c.render(small()).wait());
    assert_eq!(c.pid(), Some(pid), "a late cancel stopped the worker");
}

#[test]
fn many_threads_render_crash_hang_and_cancel_at_once_and_every_ticket_answers() {
    let c = Arc::new(client(
        TEST_WORKER,
        RenderSettings {
            timeout: Duration::from_millis(800),
            ..RenderSettings::default()
        },
    ));
    let threads: Vec<_> = (0..6u64)
        .map(|t| {
            let c = Arc::clone(&c);
            std::thread::spawn(move || {
                for i in 0..5u64 {
                    let which = (t * 7 + i * 3) % 5;
                    let scene = match which {
                        0 => hanging(),
                        1 => crashing(),
                        _ => small(),
                    };
                    let ticket = c.render(scene);
                    if (t + i) % 3 == 0 {
                        std::thread::sleep(Duration::from_millis(10 * i));
                        ticket.cancel();
                    }
                    let outcome = wait_within(ticket, Duration::from_secs(60))
                        .unwrap_or_else(|| panic!("thread {t} render {i} never answered"));
                    if let RenderOutcome::Done(Err(e)) = &outcome {
                        assert!(
                            matches!(
                                e.code(),
                                ErrorCode::WorkerTimeout | ErrorCode::WorkerCrashed
                            ),
                            "thread {t} render {i}: {e:?}"
                        );
                        // Only the scenes that hang or crash fail.
                        assert!(which <= 1, "thread {t} render {i} ({which}): {e:?}");
                    }
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    drawn(c.render(small()).wait());
}

// ---- The executable after the start ----

#[test]
fn a_missing_or_unreadable_executable_is_refused_at_the_start() {
    let dir = scratch("missing");
    for exe in [dir.join("no-such-worker"), dir.clone()] {
        match RenderClient::start(&exe, RenderSettings::default()) {
            Ok(_) => panic!("{} taken as a render worker", exe.display()),
            Err(e) => {
                assert_eq!(e.code(), ErrorCode::WorkerUnavailable, "{e:?}");
                assert!(e.suggestion().is_some(), "{e:?}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_executable_removed_after_the_start_fails_each_render_until_its_back() {
    let dir = scratch("removed");
    let link = dir.join("silico-render-worker");
    std::os::unix::fs::symlink(WORKER, &link).unwrap();
    let c = RenderClient::start(
        &link,
        RenderSettings {
            idle: Duration::from_millis(100),
            ..RenderSettings::default()
        },
    )
    .unwrap();
    drawn(c.render(small()).wait());
    assert!(eventually(Duration::from_secs(5), || c.pid().is_none()));
    std::fs::remove_file(&link).unwrap();
    let e = failed(wait_within(c.render(small()), Duration::from_secs(10)).expect("no outcome"));
    assert_eq!(e.code(), ErrorCode::WorkerUnavailable, "{e:?}");
    assert!(e.suggestion().is_some(), "{e:?}");
    std::os::unix::fs::symlink(WORKER, &link).unwrap();
    drawn(c.render(small()).wait());
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_executable_swapped_for_another_build_after_the_start_is_refused_with_advice() {
    let dir = scratch("swapped");
    let link = dir.join("silico-render-worker");
    std::os::unix::fs::symlink(WORKER, &link).unwrap();
    let c = RenderClient::start(
        &link,
        RenderSettings {
            idle: Duration::from_millis(100),
            ..RenderSettings::default()
        },
    )
    .unwrap();
    drawn(c.render(small()).wait());
    assert!(eventually(Duration::from_secs(5), || c.pid().is_none()));
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(TEST_WORKER, &link).unwrap();
    match wait_within(c.render(small()), Duration::from_secs(30)).expect("no outcome") {
        // Refused as another build, with what to do.
        RenderOutcome::Done(Err(e)) => {
            assert!(e.suggestion().is_some(), "{e:?}");
        }
        other => panic!("a swapped executable drew: {other:?}"),
    }
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- Settings at their ends ----

#[test]
fn settings_of_no_time_and_all_the_time_neither_panic_nor_hang() {
    // No idle time: the worker stopped after each render, and each render still draws.
    let c = client(
        WORKER,
        RenderSettings {
            idle: Duration::ZERO,
            ..RenderSettings::default()
        },
    );
    drawn(c.render(small()).wait());
    drawn(c.render(small()).wait());
    drop(c);
    // The longest times: no deadline to overflow.
    let c = client(
        WORKER,
        RenderSettings {
            idle: Duration::MAX,
            timeout: Duration::MAX,
            ..RenderSettings::default()
        },
    );
    drawn(c.render(small()).wait());
    drawn(c.render(small()).wait());
    drop(c);
    // No time to render: a timeout, said as one, and the client still answers.
    let c = client(
        WORKER,
        RenderSettings {
            timeout: Duration::ZERO,
            ..RenderSettings::default()
        },
    );
    let e = failed(wait_within(c.render(small()), Duration::from_secs(10)).expect("no outcome"));
    assert_eq!(e.code(), ErrorCode::WorkerTimeout, "{e:?}");
}

#[test]
fn large_by_pixels_or_faces_exactly_at_the_bound_isnt_large() {
    // 64 pixels and 6 faces exactly at their bounds: kept; one past either: stopped.
    let at = |large_pixels, large_faces| {
        client(
            WORKER,
            RenderSettings {
                large_pixels,
                large_faces,
                ..RenderSettings::default()
            },
        )
    };
    let c = at(64, 6);
    drawn(c.render(small()).wait());
    assert!(c.pid().is_some(), "stopped at exactly the bounds");
    let c = at(63, 6);
    drawn(c.render(small()).wait());
    assert_eq!(c.pid(), None, "kept one pixel past the bound");
    let c = at(64, 5);
    drawn(c.render(small()).wait());
    assert_eq!(c.pid(), None, "kept one face past the bound");
}

// ---- The idle stop ----

#[test]
fn renders_cancelled_before_theyre_sent_dont_keep_the_worker_from_idling() {
    // BREAKS (low): the idle wait restarts on every render taken from the queue, one
    // cancelled before it's sent included, so a client that keeps asking and cancelling keeps
    // the worker (and a large render's memory, were it kept) past its idle time, though it
    // draws nothing.
    let c = Arc::new(client(
        WORKER,
        RenderSettings {
            idle: Duration::from_millis(400),
            ..RenderSettings::default()
        },
    ));
    drawn(c.render(small()).wait());
    let pid = c.pid().unwrap();
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1500) {
        let t = c.render(small());
        t.cancel();
        assert!(matches!(t.wait(), RenderOutcome::Cancelled));
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !process_exists(pid) || zombie(pid),
        "the worker idle for 1.5 s, past its 0.4 s, still runs"
    );
}

// ---- Memory the large test doesn't count ----

/// A body of one face meshed `n` × `n` quads (2n² triangles) in the box's front plane, its
/// four edges around it.
fn fine_front(n: u32) -> RenderScene {
    let mut scene = front_of_a_box(64, 64);
    let mut mesh = Mesh {
        positions: vec![],
        normals: vec![],
        triangles: vec![],
        face_triangles: vec![],
        edge_points: vec![],
        edge_point_counts: vec![],
        valid: None,
    };
    let step = |i: u32, size: f64| f64::from(i) * size / f64::from(n);
    for j in 0..=n {
        for i in 0..=n {
            mesh.positions.push([step(i, 40.0), 0.0, step(j, 10.0)]);
            mesh.normals.push([0.0, -1.0, 0.0]);
        }
    }
    let at = |i: u32, j: u32| j * (n + 1) + i;
    for j in 0..n {
        for i in 0..n {
            mesh.triangles
                .push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
            mesh.triangles
                .push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
        }
    }
    mesh.face_triangles.push(2 * n * n);
    for (a, b) in [
        ([0.0, 0.0, 0.0], [40.0, 0.0, 0.0]),
        ([40.0, 0.0, 0.0], [40.0, 0.0, 10.0]),
        ([40.0, 0.0, 10.0], [0.0, 0.0, 10.0]),
        ([0.0, 0.0, 10.0], [0.0, 0.0, 0.0]),
    ] {
        mesh.edge_points.extend([a, b]);
        mesh.edge_point_counts.push(2);
    }
    scene.bodies[0].mesh = mesh;
    scene
}

/// The process's resident memory, in KiB.
fn rss_kib(pid: u32) -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

#[test]
fn a_render_of_one_face_and_millions_of_triangles_doesnt_keep_its_memory() {
    // BREAKS: the large test counts pixels and faces, not triangles, so a 64 x 64 render of
    // one face of 2 million triangles is "small", and the worker is kept warm for its idle
    // time (2 minutes) holding 1,069 MiB more than before it (146 → 1,215 MiB resident),
    // about 0.5 KiB a triangle: past the 142 MiB a small render's peak was measured at, the
    // memory the large stop exists to give back. (A part of 2,000 faces meshed at 1,000
    // triangles each, well under the 10,000-face bound, is the same.)
    let c = client(WORKER, RenderSettings::default());
    drawn(c.render(small()).wait());
    let pid = c.pid().unwrap();
    let before = rss_kib(pid);
    drawn(c.render(fine_front(1000)).wait());
    match c.pid() {
        // Stopped: its memory is given back.
        None => {}
        Some(p) => {
            assert_eq!(p, pid);
            let after = rss_kib(pid);
            // A small render's peak is 142 MiB (the design's measurement): kept, it must
            // hold no more than that much over what it held before.
            assert!(
                after.saturating_sub(before) < 142 * 1024,
                "kept after a 2-million-triangle render, holding {} MiB more ({} → {} MiB)",
                (after.saturating_sub(before)) / 1024,
                before / 1024,
                after / 1024
            );
        }
    }
}

// ---- Executables that aren't render workers ----

/// An executable shell script at `dir/name` running `body`.
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn a_worker_that_exits_at_once_fails_each_render_promptly_and_says_what_to_do() {
    let dir = scratch("exits");
    let exe = script(&dir, "exits", "exit 3");
    let c = RenderClient::start(
        &exe,
        RenderSettings {
            timeout: Duration::from_secs(5),
            ..RenderSettings::default()
        },
    )
    .unwrap();
    for _ in 0..3 {
        let start = Instant::now();
        let e =
            failed(wait_within(c.render(small()), Duration::from_secs(20)).expect("no outcome"));
        assert!(
            matches!(
                e.code(),
                ErrorCode::WorkerCrashed | ErrorCode::WorkerUnavailable
            ),
            "{e:?}"
        );
        assert!(e.suggestion().is_some(), "{e:?}");
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "{:?}",
            start.elapsed()
        );
    }
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_worker_that_never_says_it_started_times_out_and_is_killed() {
    let dir = scratch("silent");
    let marker = dir.join("pid");
    let exe = script(
        &dir,
        "silent",
        &format!("echo $$ > '{}'\nexec sleep 600", marker.display()),
    );
    let c = RenderClient::start(
        &exe,
        RenderSettings {
            timeout: Duration::from_secs(2),
            ..RenderSettings::default()
        },
    )
    .unwrap();
    let start = Instant::now();
    let e = failed(wait_within(c.render(small()), Duration::from_secs(20)).expect("no outcome"));
    assert!(
        matches!(
            e.code(),
            ErrorCode::WorkerTimeout | ErrorCode::WorkerUnavailable
        ),
        "{e:?}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(6),
        "{:?}",
        start.elapsed()
    );
    let pid: u32 = std::fs::read_to_string(&marker)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        eventually(Duration::from_secs(5), || !process_exists(pid)),
        "the silent worker {pid} was left running"
    );
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_queued_render_cancelled_answers_cancelled_at_once_not_when_its_turn_comes() {
    // BREAKS: a queued render's cancel only sets its flag; it answers `Cancelled` when the
    // client's thread reaches it, after every render ahead of it, so one session's cancel
    // waits out another's render (a hung one's whole timeout, 30 s by default; 5 s here).
    let c = client(
        TEST_WORKER,
        RenderSettings {
            timeout: Duration::from_secs(5),
            ..RenderSettings::default()
        },
    );
    let running = c.render(hanging());
    let queued = c.render(small());
    assert!(eventually(Duration::from_secs(10), || c.pid().is_some()));
    std::thread::sleep(Duration::from_millis(200));
    queued.cancel();
    let start = Instant::now();
    let outcome = wait_within(queued, Duration::from_secs(10)).expect("no outcome");
    assert!(matches!(outcome, RenderOutcome::Cancelled), "{outcome:?}");
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "the cancelled render answered after {:?}",
        start.elapsed()
    );
    drop(running);
}
