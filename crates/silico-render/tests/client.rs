//! The core's render client (`silico_core::render`), on the render worker and its test
//! worker: a worker started on the first render, stopped after a large one, a failed one,
//! and when idle; renders in turn, each with its own time; and cancelling, queued or
//! running (docs/design/m4.6-render.md, In the core, the render; Tests, the worker).

use std::path::Path;
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::render::{RenderClient, RenderOutcome, RenderSettings, Rendered};
use silico_protocol::RenderScene;
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

/// A scene the test worker hangs on, and one it aborts on.
fn hanging() -> RenderScene {
    front_of_a_box(1, 7)
}

fn crashing() -> RenderScene {
    front_of_a_box(1, 9)
}

fn process_exists(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string()])
        .output()
        .unwrap()
        .status
        .success()
}

/// Waits up to `limit` for `done`.
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

#[test]
fn the_worker_starts_on_the_first_render_and_draws_what_the_worker_draws() {
    let c = client(WORKER, RenderSettings::default());
    assert_eq!(c.pid(), None, "started before any render");
    let rendered = drawn(c.render(front_of_a_box(40, 30)).wait());
    assert_eq!(&rendered.png[1..4], b"PNG");
    let ids = rendered.ids.unwrap();
    assert_eq!(ids.len(), 40 * 30);
    // The front face at the image's centre's column, a little left of its middle.
    assert!(ids.contains(&[1, 3]), "{ids:?}");
    let pid = c.pid().unwrap();
    // A second render, on the same worker.
    drawn(c.render(front_of_a_box(40, 30)).wait());
    assert_eq!(c.pid(), Some(pid));
}

#[test]
fn an_idle_worker_is_stopped_and_the_next_render_starts_another() {
    let c = client(
        WORKER,
        RenderSettings {
            idle: Duration::from_millis(200),
            ..RenderSettings::default()
        },
    );
    drawn(c.render(front_of_a_box(16, 16)).wait());
    let pid = c.pid().unwrap();
    assert!(eventually(Duration::from_secs(5), || c.pid().is_none()));
    assert!(
        eventually(Duration::from_secs(5), || !process_exists(pid)),
        "pid {pid} still runs"
    );
    drawn(c.render(front_of_a_box(16, 16)).wait());
    assert!(c.pid().is_some_and(|p| p != pid));
}

#[test]
fn a_large_render_by_pixels_or_faces_stops_the_worker_once_it_answers() {
    // 100 pixels or 3 faces large here: a 10 x 10 render isn't, an 11 x 10 one is, and the
    // box's 6 faces are.
    let c = client(
        WORKER,
        RenderSettings {
            large_pixels: 100,
            large_faces: 6,
            ..RenderSettings::default()
        },
    );
    drawn(c.render(front_of_a_box(10, 10)).wait());
    assert!(c.pid().is_some(), "stopped after a render of 100 pixels");
    drawn(c.render(front_of_a_box(11, 10)).wait());
    assert_eq!(c.pid(), None, "kept after a render of 110 pixels");
    let c = client(
        WORKER,
        RenderSettings {
            large_faces: 5,
            ..RenderSettings::default()
        },
    );
    drawn(c.render(front_of_a_box(10, 10)).wait());
    assert_eq!(c.pid(), None, "kept after a render of 6 faces, past 5");
}

#[test]
fn a_scene_the_worker_refuses_leaves_it_running() {
    let c = client(WORKER, RenderSettings::default());
    drawn(c.render(front_of_a_box(8, 8)).wait());
    let pid = c.pid().unwrap();
    let e = failed(c.render(front_of_a_box(0, 8)).wait());
    assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
    assert_eq!(c.pid(), Some(pid));
}

#[test]
fn a_render_past_its_time_is_a_timeout_and_the_next_render_works() {
    let c = client(
        TEST_WORKER,
        RenderSettings {
            timeout: Duration::from_secs(2),
            ..RenderSettings::default()
        },
    );
    let start = Instant::now();
    let e = failed(c.render(hanging()).wait());
    assert_eq!(e.code(), ErrorCode::WorkerTimeout, "{e:?}");
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(c.pid(), None);
    drawn(c.render(front_of_a_box(8, 8)).wait());
}

#[test]
fn a_worker_that_crashes_is_a_crash_and_the_next_render_works() {
    let c = client(TEST_WORKER, RenderSettings::default());
    let e = failed(c.render(crashing()).wait());
    assert_eq!(e.code(), ErrorCode::WorkerCrashed, "{e:?}");
    assert_eq!(c.pid(), None);
    drawn(c.render(front_of_a_box(8, 8)).wait());
}

#[test]
fn renders_queued_each_have_their_own_time_from_when_theyre_sent() {
    // The first hangs for its 2 s; the second, queued behind it, waits those 2 s but still
    // has its own 2 s once it's sent, and draws.
    let c = client(
        TEST_WORKER,
        RenderSettings {
            timeout: Duration::from_secs(2),
            ..RenderSettings::default()
        },
    );
    let first = c.render(hanging());
    let second = c.render(front_of_a_box(8, 8));
    assert_eq!(failed(first.wait()).code(), ErrorCode::WorkerTimeout);
    drawn(second.wait());
}

#[test]
fn a_render_cancelled_while_queued_is_never_sent_and_one_running_is_stopped() {
    let c = client(TEST_WORKER, RenderSettings::default());
    let running = c.render(hanging());
    let queued = c.render(front_of_a_box(8, 8));
    // The queued one, cancelled first, is dropped when its turn comes.
    queued.cancel();
    // The running one, hung (its time is 30 s), stops at once when cancelled.
    assert!(eventually(Duration::from_secs(10), || c.pid().is_some()));
    std::thread::sleep(Duration::from_millis(200));
    let start = Instant::now();
    running.cancel();
    assert!(matches!(running.wait(), RenderOutcome::Cancelled));
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
    assert!(matches!(queued.wait(), RenderOutcome::Cancelled));
    // And the next render starts a worker and draws.
    drawn(c.render(front_of_a_box(8, 8)).wait());
}
