//! Hostile input for the document's measures (`silico_app::model_measures`, the prototype's
//! `docProps` Volume, Area, and Faces): a model asked of again before its reads answer, a
//! session closed under its reads, a session that never was, a part with no body, a kernel
//! that refuses to describe the solid or measures it past what a measure holds, and a key
//! of a model the session never published. Each read on the fake kernel, polled as the app
//! polls it, never waited on. Expected values by hand.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use silico_app::model_measures::{ModelMeasures, Of};
use silico_core::ErrorCode;
use silico_core::command::{ModelChoice, Revision, SessionId, Sessions};
use silico_core::document::Measure;
use silico_core::fake::FakeKernel;
use silico_core::kernel::Op;

type Fake = Arc<FakeKernel>;

struct Opened {
    _dir: tempfile::TempDir,
    kernel: Fake,
    sessions: Arc<Sessions<Fake>>,
    id: SessionId,
    of: Of,
}

/// A 4 × 2 × 1 block, as a document writes it.
const BLOCK: &str = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 4, "width": 2, "height": 1}"#;

/// A document of `features` opened on a fake kernel `setup` prepares, and rebuilt.
fn opened(features: &[&str], setup: impl FnOnce(&FakeKernel)) -> Opened {
    let dir = tempfile::tempdir().unwrap();
    let document = dir.path().join("part.silico");
    std::fs::write(
        &document,
        format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 6, "features": [{}]}}"#,
            features.join(", ")
        ),
    )
    .unwrap();
    let fake = FakeKernel::new(1).unwrap();
    setup(&fake);
    let kernel = Arc::new(fake);
    let sessions = Arc::new(Sessions::with_cache_opener(Arc::clone(&kernel), |dir| {
        silico_core::cache::Cache::unsealed(dir)
    }));
    let id = sessions.open(&document, false).wait().session.unwrap().id;
    // Rebuilt, whatever its features say (one may fail).
    let _ = sessions.timeline(&id, ModelChoice::Current).wait();
    let revision = sessions.published(&id).result.unwrap().revision;
    Opened {
        _dir: dir,
        kernel,
        sessions,
        id,
        of: (1, revision),
    }
}

/// `measures` polled until neither read is out, within a minute.
fn settled(measures: &mut ModelMeasures) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while measures.reading() {
        measures.poll();
        assert!(Instant::now() < deadline, "never answered");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn asked_of_a_newer_model_before_the_first_answers_only_the_newest_is_held() {
    let opened = opened(&[BLOCK], |_| {});
    let mut measures = ModelMeasures::default();
    // Ten models in a row, each asked before the last's reads were polled: each read let go
    // as the next is asked, and only the last's answers held.
    for count in 1..=10 {
        measures.ask(&opened.sessions, &opened.id, Some((count, opened.of.1)));
    }
    assert_eq!(measures.reads(), 20);
    settled(&mut measures);
    for count in 1..10 {
        assert!(measures.answers((count, opened.of.1)).is_none(), "{count}");
    }
    let answers = measures.answers((10, opened.of.1)).unwrap();
    // 4 × 2 × 1 = 8 mm³, 2·(8 + 4 + 2) = 28 mm², 6 faces.
    assert_eq!(answers.volume(), Some(Measure::from_millionths(8_000_000)));
    assert_eq!(answers.area(), Some(Measure::from_millionths(28_000_000)));
    assert_eq!(answers.faces(), Some(6));
}

#[test]
fn a_session_closed_under_its_reads_is_answered_with_refusals_and_asks_again_cleanly() {
    let opened = opened(&[BLOCK], |_| {});
    let mut measures = ModelMeasures::default();
    measures.ask(&opened.sessions, &opened.id, Some(opened.of));
    opened.sessions.close(&opened.id).wait();
    settled(&mut measures);
    // Each read answered: measured before the close, or refused for it; never a panic, and
    // never left out.
    let answers = measures.answers(opened.of).unwrap();
    assert!(answers.bodies.is_some() && answers.faces.is_some());
    // A model asked of the closed session: both refused, the refusal the first.
    let later = (2, opened.of.1);
    measures.ask(&opened.sessions, &opened.id, Some(later));
    settled(&mut measures);
    let answers = measures.answers(later).unwrap();
    let refused = answers.refused().expect("refused");
    assert_eq!(refused.errors()[0].code(), ErrorCode::InvalidInput);
    assert_eq!(
        refused.errors()[0].message(),
        format!("no session {} is open", opened.id)
    );
    assert_eq!(answers.volume(), None);
    assert_eq!(answers.faces(), None);
    // And let go: nothing asked, nothing held.
    measures.ask(&opened.sessions, &opened.id, None);
    assert!(!measures.reading());
    assert!(measures.answers(later).is_none());
}

#[test]
fn a_session_that_never_was_is_refused_by_name() {
    let opened = opened(&[BLOCK], |_| {});
    let mut measures = ModelMeasures::default();
    let stranger = SessionId::parse("s999").unwrap();
    measures.ask(&opened.sessions, &stranger, Some(opened.of));
    settled(&mut measures);
    let answers = measures.answers(opened.of).unwrap();
    let refused = answers.refused().unwrap();
    assert_eq!(refused.errors()[0].code(), ErrorCode::InvalidInput);
    assert_eq!(refused.errors()[0].message(), "no session s999 is open");
}

#[test]
fn a_part_with_no_body_measures_nothing_at_zero() {
    // Every feature suppressed: no body, so nothing added and no face.
    let suppressed = BLOCK.replace(r#""height": 1"#, r#""height": 1, "suppressed": true"#);
    let opened = opened(&[&suppressed], |_| {});
    let mut measures = ModelMeasures::default();
    measures.ask(&opened.sessions, &opened.id, Some(opened.of));
    settled(&mut measures);
    let answers = measures.answers(opened.of).unwrap();
    assert!(answers.refused().is_none(), "{:?}", answers.refused());
    assert_eq!(answers.bodies().unwrap(), Vec::<&_>::new());
    assert_eq!(answers.volume(), Some(Measure::from_millionths(0)));
    assert_eq!(answers.area(), Some(Measure::from_millionths(0)));
    assert_eq!(answers.faces(), Some(0));
}

#[test]
fn a_kernel_that_wont_describe_the_solid_refuses_the_faces_alone_in_its_words() {
    let opened = opened(&[BLOCK], |kernel| {
        kernel.refuse(|op| {
            matches!(op, Op::DescribeAll { .. }).then(|| {
                silico_core::Error::new(ErrorCode::KernelFailed, "the worker couldn't describe it")
            })
        });
    });
    let mut measures = ModelMeasures::default();
    measures.ask(&opened.sessions, &opened.id, Some(opened.of));
    settled(&mut measures);
    let answers = measures.answers(opened.of).unwrap();
    // Its volume and area are its fingerprint's, which asks no description.
    assert_eq!(answers.volume(), Some(Measure::from_millionths(8_000_000)));
    assert_eq!(answers.faces(), None);
    let refused = answers.refused().unwrap();
    assert!(
        refused.errors()[0]
            .message()
            .contains("the worker couldn't describe it"),
        "{:?}",
        refused.errors()
    );
    assert!(!opened.kernel.calls().is_empty());
}

#[test]
fn a_solid_measured_past_what_a_measure_holds_is_refused_not_added() {
    // The kernel answers a volume no measure holds (finite, past i128's millionths).
    let opened = opened(&[BLOCK], |kernel| {
        kernel.answer_instead(|op| {
            matches!(op, Op::Summarize { .. }).then(|| {
                silico_protocol::Reply::Summary(silico_protocol::ShapeSummary {
                    volume: 1e300,
                    area: 28.0,
                    center: [2.0, 1.0, 0.5],
                    faces: 6,
                    edges: 12,
                    vertices: 8,
                    bounds: Some([[0.0; 3], [4.0, 2.0, 1.0]]),
                    valid: true,
                })
            })
        });
    });
    let mut measures = ModelMeasures::default();
    measures.ask(&opened.sessions, &opened.id, Some(opened.of));
    settled(&mut measures);
    let answers = measures.answers(opened.of).unwrap();
    assert_eq!(answers.volume(), None);
    assert_eq!(answers.area(), None);
    assert!(answers.refused().is_some());
}

#[test]
fn a_key_of_a_revision_never_published_holds_no_answer() {
    let opened = opened(&[BLOCK], |_| {});
    let mut measures = ModelMeasures::default();
    let never = (1, Revision::from_number(u64::MAX).unwrap());
    measures.ask(&opened.sessions, &opened.id, Some(never));
    settled(&mut measures);
    let answers = measures.answers(never).unwrap();
    assert!(answers.bodies.is_none() && answers.faces.is_none());
    assert!(answers.refused().is_none());
}
