//! The core's render client (`silico_core::render`), on the render worker and its test
//! worker: a worker started on the first render, stopped after a large one, a failed one,
//! and when idle; renders in turn, each with its own time; and cancelling, queued or
//! running (docs/design/m4.6-render.md, In the core, the render; Tests, the worker).

use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::render::{RenderAdapter, RenderOutcome, RenderSettings};
use silico_render::bench::front_of_a_box;
use silico_render::test_scenes;

use crate::common::{
    LeakOnPanic, TEST_WORKER, UNREACHED, WAIT, WORKER, at, client, crashing, drawn, eventually,
    failed, hanging, process_exists, small, wait_within,
};
use crate::lateness::{Lateness, allowance};

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
    assert!(eventually(WAIT, || c.pid().is_none()));
    assert!(
        eventually(WAIT, || !process_exists(pid)),
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
fn a_scene_that_fails_its_check_is_answered_at_once_as_a_bug_and_never_sent() {
    // The command builds scenes from checked values: one that fails is Silico's bug, and
    // the worker isn't started for it, nor stopped if it runs.
    let c = client(WORKER, RenderSettings::default());
    let e = failed(c.render(front_of_a_box(0, 8)).wait());
    assert_eq!(e.code(), ErrorCode::Internal, "{e:?}");
    assert_eq!(e.parameter(), Some("width"), "{e:?}");
    assert!(e.suggestion().is_some(), "{e:?}");
    assert_eq!(c.pid(), None, "a worker started for a scene never sent");
    drawn(c.render(small()).wait());
    let pid = c.pid().unwrap();
    let e = failed(c.render(front_of_a_box(8, 4097)).wait());
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::Internal, Some("height"))
    );
    assert_eq!(c.pid(), Some(pid));
}

#[test]
fn a_worker_that_answers_other_than_asked_is_a_protocol_error_and_stopped() {
    // Each of the test worker's misbehaving answers to a checked scene: one ID short, no
    // image, the wrong reply, an ID buffer not asked for and none when asked, and a refusal
    // of a scene the core checked.
    let c = client(TEST_WORKER, RenderSettings::default());
    let mut unasked = at(test_scenes::IDS_FLIPPED);
    unasked.ids = false;
    for (what, scene) in [
        ("one ID short", at(test_scenes::IDS_SHORT)),
        ("no image", at(test_scenes::NO_IMAGE)),
        ("an echo", at(test_scenes::WRONG_REPLY)),
        ("no ID buffer, though asked", at(test_scenes::IDS_FLIPPED)),
        ("an ID buffer not asked for", unasked),
        ("a checked scene refused", at(test_scenes::REFUSES)),
    ] {
        drawn(c.render(small()).wait());
        let pid = c.pid().unwrap();
        let e = failed(c.render(scene).wait());
        assert_eq!(e.code(), ErrorCode::ProtocolError, "{what}: {e:?}");
        assert!(e.suggestion().is_some(), "{what}: {e:?}");
        assert_eq!(c.pid(), None, "{what}: the worker kept");
        assert!(
            eventually(WAIT, || !process_exists(pid)),
            "{what}: pid {pid} still runs"
        );
    }
    // Each cause, said.
    let cause = |scene| failed(c.render(scene).wait()).cause().map(str::to_owned);
    assert_eq!(
        cause(at(test_scenes::IDS_SHORT)).as_deref(),
        Some("10 IDs for 11 pixels")
    );
    assert_eq!(
        cause(at(test_scenes::NO_IMAGE)).as_deref(),
        Some("no image")
    );
    assert_eq!(
        cause(at(test_scenes::WRONG_REPLY)).as_deref(),
        Some("it answered Echo")
    );
    assert_eq!(
        cause(at(test_scenes::IDS_FLIPPED)).as_deref(),
        Some("no ID buffer, though one was asked for")
    );
    let mut unasked = at(test_scenes::IDS_FLIPPED);
    unasked.ids = false;
    assert_eq!(
        cause(unasked).as_deref(),
        Some("an ID buffer that wasn't asked for")
    );
    drawn(c.render(small()).wait());
}

#[test]
fn a_render_without_the_id_buffer_draws_and_has_none() {
    let c = client(WORKER, RenderSettings::default());
    let mut scene = front_of_a_box(40, 30);
    scene.ids = false;
    let rendered = drawn(c.render(scene).wait());
    assert_eq!(&rendered.png[1..4], b"PNG");
    assert_eq!(rendered.ids, None);
}

#[test]
fn the_adapter_policy_reaches_the_worker_as_given() {
    for (adapter, said) in [
        (RenderAdapter::Software, "policy Software"),
        (RenderAdapter::Any, "policy Any"),
    ] {
        let c = client(
            TEST_WORKER,
            RenderSettings {
                adapter,
                ..RenderSettings::default()
            },
        );
        let e = failed(c.render(at(test_scenes::SAYS_POLICY)).wait());
        assert_eq!(e.code(), ErrorCode::WorkerUnavailable, "{e:?}");
        assert!(format!("{e:?}").contains(said), "{adapter:?}: {e:?}");
    }
}

#[test]
fn a_worker_given_an_argument_it_doesnt_know_exits_saying_so() {
    for exe in [WORKER, TEST_WORKER] {
        for args in [&["--any"][..], &["--any-adapter", "--any-adapter"]] {
            let out = std::process::Command::new(exe).args(args).output().unwrap();
            assert_eq!(out.status.code(), Some(2), "{exe} {args:?}");
            let said = String::from_utf8_lossy(&out.stderr);
            assert!(
                said.contains("unexpected argument"),
                "{exe} {args:?}: {said}"
            );
        }
    }
}

#[test]
fn renders_drawn_keep_the_worker_from_idling() {
    // Idle 2 s; a render every 300 ms for 5 s, each counting the idle time afresh: the
    // worker is never idle that long (a render may take 1.7 s, loaded, and still keep it).
    let c = client(
        WORKER,
        RenderSettings {
            idle: Duration::from_millis(2000),
            ..RenderSettings::default()
        },
    );
    drawn(c.render(small()).wait());
    let pid = c.pid().unwrap();
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(5000) {
        std::thread::sleep(Duration::from_millis(300));
        drawn(c.render(small()).wait());
        assert_eq!(
            c.pid(),
            Some(pid),
            "worker replaced after {:?}",
            start.elapsed()
        );
    }
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
    // Its 2 s, then a kill and a reap: 10 s on a quiet machine with room to spare, and the
    // wakes on the way as late as this machine runs them.
    let lateness = Lateness::start();
    let start = Instant::now();
    let e = failed(c.render(hanging()).wait());
    let took = start.elapsed();
    assert_eq!(e.code(), ErrorCode::WorkerTimeout, "{e:?}");
    let bound = allowance(Duration::from_secs(10), lateness.most());
    assert!(took < bound, "{took:?}, the bound {bound:?}");
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
    // The first hangs for its 5 s; the second, queued behind it, waits those 5 s but still
    // has its own 5 s once it's sent, and draws: timed from when it was queued, it would have
    // none left. 5 s, not less: a small render on a fresh worker takes that, loaded.
    let c = client(
        TEST_WORKER,
        RenderSettings {
            timeout: Duration::from_secs(5),
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
    let c = LeakOnPanic::new(client(
        TEST_WORKER,
        RenderSettings {
            timeout: UNREACHED,
            ..RenderSettings::default()
        },
    ));
    let running = c.render(hanging());
    let queued = c.render(front_of_a_box(8, 8));
    // The queued one, cancelled first, is dropped when its turn comes.
    queued.cancel();
    // The running one, hung (its time one no test reaches), stops when cancelled: nothing
    // else answers it within `WAIT`.
    assert!(eventually(WAIT, || c.pid().is_some()));
    std::thread::sleep(Duration::from_millis(200));
    running.cancel();
    let outcome = wait_within(running, WAIT).expect("the cancelled render never answered");
    assert!(matches!(outcome, RenderOutcome::Cancelled), "{outcome:?}");
    assert!(matches!(queued.wait(), RenderOutcome::Cancelled));
    // And the next render starts a worker and draws.
    drawn(c.render(front_of_a_box(8, 8)).wait());
}
