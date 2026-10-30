//! `Sessions::render` (docs/design/m4.6-render.md, In the core, step 4): a session's model,
//! current or published, rendered on the read's own kernel client by the sessions' renderer,
//! through the meshes they keep; refused without a renderer. The fake kernel and the fake
//! renderer, whose record says what each render sent.

use std::sync::Arc;

use super::example::EXAMPLE;
use super::listing::{Fixture, answer, fixture};
use silico_core::ErrorCode;
use silico_core::command::render::Render;
use silico_core::command::{ModelChoice, SessionId};
use silico_core::fake::FakeRenderer;
use silico_core::kernel::Op;
use silico_core::views::NamedView;

fn front() -> Render {
    Render {
        view: Some(NamedView::Front),
        camera: None,
        width: 100,
        height: 100,
        highlight: vec![],
        to: None,
        replace: false,
    }
}

/// The fixture's sessions, drawing with a fake renderer kept to read its record, and the
/// example opened.
fn rendering() -> (Fixture, Arc<FakeRenderer>, SessionId) {
    let Fixture {
        dir,
        kernel,
        sessions,
    } = fixture();
    let renderer = Arc::new(FakeRenderer::new());
    let f = Fixture {
        dir,
        kernel,
        sessions: sessions.rendering(Arc::clone(&renderer)),
    };
    let s = f.open("example.silico", EXAMPLE);
    (f, renderer, s)
}

#[test]
fn a_sessions_current_model_is_rendered_by_its_renderer_meshed_on_the_kernel() {
    let (f, renderer, s) = rendering();
    let done = answer(f.sessions.render(&s, ModelChoice::Current, front()));
    let rendering = done.result.unwrap();
    assert_eq!(rendering.png, b"PNG 100x100");
    assert_eq!(rendering.result.png_bytes, 11);
    let sent = renderer.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].bodies.len(), 1);
    assert_eq!(sent[0].bodies[0].body, 1);
    // Its body meshed on the kernel once; a second render at one level, from the sessions'
    // meshes, not again.
    let meshed = || {
        f.kernel
            .calls()
            .iter()
            .filter(|c| matches!(c.op, Op::Tessellate { .. }))
            .count()
    };
    assert_eq!(meshed(), 1);
    answer(f.sessions.render(&s, ModelChoice::Current, front()))
        .result
        .unwrap();
    assert_eq!(meshed(), 1);
    assert_eq!(renderer.sent().len(), 2);
}

#[test]
fn a_sessions_published_model_is_rendered_alike() {
    let (f, renderer, s) = rendering();
    answer(f.sessions.render(&s, ModelChoice::Current, front()))
        .result
        .unwrap();
    let published = answer(f.sessions.render(&s, ModelChoice::Published, front()));
    assert_eq!(published.model.map(|m| m.of), Some(ModelChoice::Published));
    published.result.unwrap();
    assert_eq!(renderer.sent().len(), 2);
}

#[test]
fn sessions_given_no_renderer_refuse_a_render_as_unavailable() {
    let f = fixture();
    let s = f.open("example.silico", EXAMPLE);
    let done = answer(f.sessions.render(&s, ModelChoice::Current, front()));
    let problems = done.result.unwrap_err();
    assert_eq!(problems.errors()[0].code(), ErrorCode::WorkerUnavailable);
    assert!(problems.errors()[0].suggestion().is_some());
}

#[test]
fn a_render_refused_for_its_values_sends_nothing_through_the_session() {
    let (f, renderer, s) = rendering();
    let mut bad = front();
    bad.width = 0;
    let done = answer(f.sessions.render(&s, ModelChoice::Current, bad));
    let problems = done.result.unwrap_err();
    assert_eq!(problems.errors()[0].parameter(), Some("width"));
    assert!(renderer.sent().is_empty());
}

#[test]
fn a_published_render_whose_session_closes_mid_mesh_answers_closed_not_cancelled() {
    // Found by round 2: the render answered `cancelled`, though nobody cancelled it. A
    // published read answers as of a session that closed (decision 34), as `distance` does.
    use std::time::Duration;
    let (f, renderer, s) = rendering();
    answer(f.sessions.render(&s, ModelChoice::Current, front()))
        .result
        .unwrap();
    // Meshed afresh (another size's level), held in the kernel.
    let hold = f.kernel.hold(|op| op.name() == "Tessellate");
    let mut wider = front();
    wider.width = 1000;
    wider.height = 1000;
    let held = f.sessions.render(&s, ModelChoice::Published, wider);
    let held = match held.wait_timeout(Duration::from_millis(300)) {
        Ok(r) => panic!("answered while held: {:?}", r.result),
        Err(running) => running,
    };
    assert!(answer(f.sessions.close(&s)).result.is_ok());
    let refused = answer(held).result.unwrap_err();
    hold.release();
    let e = &refused.errors()[0];
    assert_eq!(
        (e.code(), e.parameter(), e.feature()),
        (ErrorCode::InvalidInput, Some("session"), None),
        "{e:?}"
    );
    assert_eq!(renderer.sent().len(), 1);
}

#[test]
fn a_render_cancelled_through_its_session_answers_cancelled() {
    // A cancel's answer is the session's: `cancelled`, the render stopped as a close stops
    // it.
    use std::time::Duration;
    let (f, renderer, s) = rendering();
    let hold = f.kernel.hold(|op| op.name() == "Tessellate");
    let held = f.sessions.render(&s, ModelChoice::Current, front());
    let held = match held.wait_timeout(Duration::from_millis(300)) {
        Ok(r) => panic!("answered while held: {:?}", r.result),
        Err(running) => running,
    };
    assert!(held.cancel());
    let refused = answer(held).result.unwrap_err();
    hold.release();
    assert_eq!(
        refused.errors()[0].code(),
        ErrorCode::Cancelled,
        "{refused:?}"
    );
    assert!(renderer.sent().is_empty());
}

#[test]
fn a_render_cancelled_while_its_scene_waits_to_be_drawn_answers_cancelled() {
    // M5.0's section 3 (M4.6's open item): the stop button's `cancel` of a render whose
    // scene is with the renderer answers `cancelled`, never `internal`, for the current
    // model and the published one alike, and through either way of stopping it.
    use silico_core::command::SessionCommand;
    use std::time::{Duration, Instant};
    for (choice, by_name) in [
        (ModelChoice::Current, true),
        (ModelChoice::Published, true),
        (ModelChoice::Current, false),
    ] {
        let (f, renderer, s) = rendering();
        // Published once, so a published read has a model to draw.
        answer(f.sessions.render(&s, ModelChoice::Current, front()))
            .result
            .unwrap();
        renderer.hold_all();
        let held = f.sessions.render(&s, choice, front());
        let deadline = Instant::now() + Duration::from_secs(60);
        while renderer.sent().len() < 2 {
            assert!(
                Instant::now() < deadline,
                "the scene never reached the renderer"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        if by_name {
            let cancelled = f
                .sessions
                .cancel(&s, SessionCommand::Render)
                .result
                .unwrap();
            assert!(cancelled.stopped, "{choice:?}");
        } else {
            assert!(held.cancel());
        }
        let refused = answer(held).result.unwrap_err();
        assert_eq!(refused.total(), 1, "{refused:?}");
        assert_eq!(
            refused.errors()[0].code(),
            ErrorCode::Cancelled,
            "{choice:?}: {refused:?}"
        );
        assert_eq!(
            refused.errors()[0].message(),
            "render was cancelled before it finished"
        );
    }
}

/// A renderer that, asked to draw, cancels the read that asked (its stopper, once the test
/// has handed it over) and then answers as the fake does: the read cancelled after its PNG
/// is drawn, before it's written.
struct CancelsOnceDrawn {
    inner: FakeRenderer,
    stopper: Arc<(
        std::sync::Mutex<Option<silico_core::command::Stopper>>,
        std::sync::Condvar,
    )>,
}

impl silico_core::command::render::Renderer for CancelsOnceDrawn {
    fn render(&self, scene: silico_protocol::RenderScene) -> silico_core::render::RenderTicket {
        let (slot, handed) = &*self.stopper;
        let slot = handed
            .wait_timeout_while(slot.lock().unwrap(), super::listing::HANG, |s| s.is_none())
            .unwrap()
            .0;
        assert!(slot.as_ref().unwrap().cancel());
        drop(slot);
        self.inner.render(scene)
    }
}

#[test]
fn a_render_cancelled_once_drawn_answers_cancelled_and_writes_no_file() {
    // Decision 34: a cancelled command leaves no trace. The cancel comes after the PNG is
    // drawn: the render settles before writing it, the settle refuses, and nothing is
    // written, neither the file nor one beside it.
    let Fixture {
        dir,
        kernel,
        sessions,
    } = fixture();
    let stopper = Arc::new((std::sync::Mutex::new(None), std::sync::Condvar::new()));
    let f = Fixture {
        dir,
        kernel,
        sessions: sessions.rendering(CancelsOnceDrawn {
            inner: FakeRenderer::new(),
            stopper: Arc::clone(&stopper),
        }),
    };
    let s = f.open("example.silico", EXAMPLE);
    let out = f.dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let mut request = front();
    request.to = Some(out.join("view.png"));
    let running = f.sessions.render(&s, ModelChoice::Current, request);
    *stopper.0.lock().unwrap() = Some(running.stopper());
    stopper.1.notify_all();
    let refused = answer(running).result.unwrap_err();
    assert_eq!(
        refused.errors()[0].code(),
        ErrorCode::Cancelled,
        "{refused:?}"
    );
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
}

#[test]
fn a_render_to_a_document_a_session_holds_is_refused_and_the_document_kept() {
    // Decision 35: a file a command writes is never a document the sessions hold, whatever
    // it's named. One named `.png`, open in a session, isn't replaced by its own render.
    let (f, renderer, _) = rendering();
    let held = f.dir.path().join("held.png");
    let s = f.open("held.png", EXAMPLE);
    let mut request = front();
    request.to = Some(held.clone());
    request.replace = true;
    let done = answer(f.sessions.render(&s, ModelChoice::Current, request));
    let problems = done.result.unwrap_err();
    let e = &problems.errors()[0];
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::InvalidInput, Some("to")),
        "{e:?}"
    );
    assert!(
        e.message().contains("is a document a session holds"),
        "{e:?}"
    );
    assert_eq!(std::fs::read_to_string(&held).unwrap(), EXAMPLE);
    assert!(renderer.sent().is_empty());
}

#[test]
fn a_render_to_another_name_of_a_document_a_session_holds_is_refused_and_the_document_kept() {
    // The held document is a file, not a spelling of its path: a hard link to it by another
    // name (as a case-insensitive file system's `HELD.png` is its `Held.png`, on Linux too,
    // where the path's spelling is kept as given) is the document. Refused, its bytes kept,
    // nothing drawn.
    let (f, renderer, _) = rendering();
    let s = f.open("Held.png", EXAMPLE);
    let held = f.dir.path().join("Held.png");
    let alias = f.dir.path().join("Alias.png");
    std::fs::hard_link(&held, &alias).unwrap();
    let mut request = front();
    request.to = Some(alias.clone());
    request.replace = true;
    let done = answer(f.sessions.render(&s, ModelChoice::Current, request));
    let problems = done.result.unwrap_err();
    let e = &problems.errors()[0];
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::InvalidInput, Some("to")),
        "{e:?}"
    );
    assert!(
        e.message().contains("is a document a session holds"),
        "{e:?}"
    );
    assert_eq!(std::fs::read_to_string(&held).unwrap(), EXAMPLE);
    assert_eq!(std::fs::read_to_string(&alias).unwrap(), EXAMPLE);
    assert!(renderer.sent().is_empty());
}
