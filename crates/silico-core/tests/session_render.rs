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
