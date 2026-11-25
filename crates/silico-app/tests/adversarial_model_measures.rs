//! Hostile input for the document's measures (`silico_app::model_measures`, the prototype's
//! `docProps` Volume, Area, and Faces): a model asked of again before its reads answer, a
//! session closed under its reads, a session that never was, a part with no body, a kernel
//! that refuses to describe the solid or measures it past what a measure holds, and a key
//! of a model the session never published, and every published read of the session running
//! when the measures ask, a refusal that passes. Each read on the fake kernel, polled as the app
//! polls it, never waited on. Expected values by hand.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use silico_app::model_measures::{ModelKey, ModelMeasures};
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
    of: ModelKey,
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
    settled_at(measures, Instant::now());
}

/// [`settled`], each poll at `now`.
fn settled_at(measures: &mut ModelMeasures, now: Instant) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while measures.reading() {
        measures.poll(now);
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
        measures.ask(
            &opened.sessions,
            &opened.id,
            Some((count, opened.of.1)),
            Instant::now(),
        );
    }
    assert_eq!(measures.reads(), 20);
    // The reads let go may still be running, holding the session's published reads: the
    // newest model's may be refused for the moment, and are asked again once their delay is
    // up, till they're answered.
    let newest = (10, opened.of.1);
    let mut now = Instant::now();
    settled_at(&mut measures, now);
    let deadline = Instant::now() + Duration::from_secs(60);
    while let Some(refused) = measures.answers(newest).unwrap().refused() {
        // Only that refusal: any other fails here, saying it.
        assert!(
            refused.errors()[0]
                .message()
                .contains("published reads are running"),
            "{refused:?}"
        );
        assert!(Instant::now() < deadline, "still refused: {refused:?}");
        std::thread::sleep(Duration::from_millis(50));
        now += silico_app::viewport::RETRY_MOST;
        measures.ask(&opened.sessions, &opened.id, Some(newest), now);
        settled_at(&mut measures, now);
    }
    for count in 1..10 {
        assert!(measures.answers((count, opened.of.1)).is_none(), "{count}");
    }
    let answers = measures.answers((10, opened.of.1)).unwrap();
    // 4 × 2 × 1 = 8 mm³, 2·(8 + 4 + 2) = 28 mm², 6 faces.
    assert_eq!(answers.volume(), Some(Measure::from_millionths(8_000_000)));
    assert_eq!(answers.area(), Some(Measure::from_millionths(28_000_000)));
    assert_eq!(answers.face_count(), Some(6));
}

#[test]
fn a_session_closed_under_its_reads_is_answered_with_refusals_and_asks_again_cleanly() {
    let opened = opened(&[BLOCK], |_| {});
    let mut measures = ModelMeasures::default();
    measures.ask(
        &opened.sessions,
        &opened.id,
        Some(opened.of),
        Instant::now(),
    );
    opened.sessions.close(&opened.id).wait();
    settled(&mut measures);
    // Each read answered: measured before the close, or refused for it; never a panic, and
    // never left out.
    let answers = measures.answers(opened.of).unwrap();
    assert!(answers.bodies.is_some() && answers.faces.is_some());
    // A model asked of the closed session: both refused, the refusal the first.
    let later = (2, opened.of.1);
    measures.ask(&opened.sessions, &opened.id, Some(later), Instant::now());
    settled(&mut measures);
    let answers = measures.answers(later).unwrap();
    let refused = answers.refused().expect("refused");
    assert_eq!(refused.errors()[0].code(), ErrorCode::InvalidInput);
    assert_eq!(
        refused.errors()[0].message(),
        format!("no session {} is open", opened.id)
    );
    assert_eq!(answers.volume(), None);
    assert_eq!(answers.face_count(), None);
    // And let go: nothing asked, nothing held.
    measures.ask(&opened.sessions, &opened.id, None, Instant::now());
    assert!(!measures.reading());
    assert!(measures.answers(later).is_none());
}

#[test]
fn a_session_that_never_was_is_refused_by_name() {
    let opened = opened(&[BLOCK], |_| {});
    let mut measures = ModelMeasures::default();
    let stranger = SessionId::parse("s999").unwrap();
    measures.ask(&opened.sessions, &stranger, Some(opened.of), Instant::now());
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
    measures.ask(
        &opened.sessions,
        &opened.id,
        Some(opened.of),
        Instant::now(),
    );
    settled(&mut measures);
    let answers = measures.answers(opened.of).unwrap();
    assert!(answers.refused().is_none(), "{:?}", answers.refused());
    assert_eq!(answers.measured_features().unwrap(), Vec::<&_>::new());
    assert_eq!(answers.volume(), Some(Measure::from_millionths(0)));
    assert_eq!(answers.area(), Some(Measure::from_millionths(0)));
    assert_eq!(answers.face_count(), Some(0));
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
    measures.ask(
        &opened.sessions,
        &opened.id,
        Some(opened.of),
        Instant::now(),
    );
    settled(&mut measures);
    let answers = measures.answers(opened.of).unwrap();
    // Its volume and area are its fingerprint's, which asks no description.
    assert_eq!(answers.volume(), Some(Measure::from_millionths(8_000_000)));
    assert_eq!(answers.face_count(), None);
    let refused = answers.refused().unwrap();
    assert!(
        refused.errors()[0]
            .message()
            .contains("the worker couldn't describe it"),
        "{:?}",
        refused.errors()
    );
}

#[test]
fn a_solid_measured_past_what_a_measure_holds_is_refused_not_added() {
    // The kernel answers a volume no measure holds (finite, past i128's millionths).
    let opened = opened(&[BLOCK], unmeasurable);
    let mut measures = ModelMeasures::default();
    measures.ask(
        &opened.sessions,
        &opened.id,
        Some(opened.of),
        Instant::now(),
    );
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
    measures.ask(&opened.sessions, &opened.id, Some(never), Instant::now());
    settled(&mut measures);
    let answers = measures.answers(never).unwrap();
    assert!(answers.bodies.is_none() && answers.faces.is_none());
    assert!(answers.refused().is_none());
}

#[test]
fn a_refusal_for_the_moment_passes_once_the_reads_running_answer() {
    // The session's published reads all running (8, `MAX_PUBLISHED_READS`), each a distance
    // the kernel holds: the measures' reads are refused for it, a refusal for the moment.
    let opened = opened(&[BLOCK], |_| {});
    let listed = opened
        .sessions
        .elements(&opened.id, ModelChoice::Published, None, None, 2)
        .wait()
        .result
        .unwrap();
    let names = [
        listed.entries[0].name.to_string(),
        listed.entries[1].name.to_string(),
    ];
    let hold = opened.kernel.hold(|op| matches!(op, Op::Distance { .. }));
    let out: Vec<_> = (0..8)
        .map(|_| {
            opened
                .sessions
                .distance(&opened.id, ModelChoice::Published, names.clone(), None)
        })
        .collect();
    assert!(opened.kernel.wait_running(1, Duration::from_secs(60)));
    let start = Instant::now();
    let mut measures = ModelMeasures::default();
    measures.ask(&opened.sessions, &opened.id, Some(opened.of), start);
    settled_at(&mut measures, start);
    let answers = measures.answers(opened.of).unwrap();
    assert!(answers.bodies_refused() && answers.faces_refused());
    assert_eq!(measures.reads(), 2);
    // The reads running answer.
    drop(hold);
    for running in out {
        running.wait().result.unwrap();
    }
    // Asked again once its delay is up (the longest there is, so whatever it was): both
    // reads asked again, and answered.
    let later = start + silico_app::viewport::RETRY_MOST;
    measures.ask(&opened.sessions, &opened.id, Some(opened.of), later);
    assert_eq!(measures.reads(), 4);
    settled_at(&mut measures, later);
    let answers = measures.answers(opened.of).unwrap();
    assert!(answers.refused().is_none(), "{:?}", answers.refused());
    assert_eq!(answers.volume(), Some(Measure::from_millionths(8_000_000)));
    assert_eq!(answers.face_count(), Some(6));
    assert_eq!(measures.retry_at(), None);
    // Nothing more asked of it.
    measures.ask(
        &opened.sessions,
        &opened.id,
        Some(opened.of),
        later + silico_app::viewport::RETRY_MOST,
    );
    assert_eq!(measures.reads(), 4);
}

/// A kernel that measures the block past what a measure holds, so `mass_properties` is
/// refused and `query`, which reads the description, answers.
fn unmeasurable(kernel: &FakeKernel) {
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
}

#[test]
fn a_mass_properties_refusal_alone_is_asked_again_alone() {
    use silico_app::viewport::RETRY_FIRST;
    let opened = opened(&[BLOCK], unmeasurable);
    let start = Instant::now();
    let mut measures = ModelMeasures::default();
    measures.ask(&opened.sessions, &opened.id, Some(opened.of), start);
    settled_at(&mut measures, start);
    let answers = measures.answers(opened.of).unwrap();
    assert!(answers.bodies_refused() && !answers.faces_refused());
    assert_eq!(answers.face_count(), Some(6));
    assert_eq!(measures.retry_at(), Some(start + RETRY_FIRST));
    measures.ask(
        &opened.sessions,
        &opened.id,
        Some(opened.of),
        start + RETRY_FIRST,
    );
    assert_eq!(measures.reads(), 3, "the mass properties asked again alone");
    // Its refusal held, and the faces' answer kept, while it's out and once it answers.
    let answers = measures.answers(opened.of).unwrap();
    assert!(answers.bodies_refused());
    assert_eq!(answers.face_count(), Some(6));
    settled_at(&mut measures, start + RETRY_FIRST);
    assert_eq!(measures.retry_at(), Some(start + 3 * RETRY_FIRST));
    let answers = measures.answers(opened.of).unwrap();
    assert!(answers.bodies_refused() && !answers.faces_refused());
    assert_eq!(answers.face_count(), Some(6));
}

#[test]
fn a_new_models_first_refusal_waits_its_own_first_delay() {
    use silico_app::viewport::RETRY_FIRST;
    let opened = opened(&[BLOCK], unmeasurable);
    let start = Instant::now();
    let mut measures = ModelMeasures::default();
    measures.ask(&opened.sessions, &opened.id, Some(opened.of), start);
    settled_at(&mut measures, start);
    // Refused again, the delay doubled: due at start + 3 · RETRY_FIRST.
    let due = start + RETRY_FIRST;
    measures.ask(&opened.sessions, &opened.id, Some(opened.of), due);
    settled_at(&mut measures, due);
    assert_eq!(measures.retry_at(), Some(due + 2 * RETRY_FIRST));
    // Another model, a moment later: its first refusal waits the first delay from then,
    // never the old model's deadline nor its doubled delay.
    let next = (2, opened.of.1);
    let soon = due + Duration::from_millis(1);
    measures.ask(&opened.sessions, &opened.id, Some(next), soon);
    settled_at(&mut measures, soon);
    assert!(measures.answers(next).unwrap().bodies_refused());
    assert_eq!(measures.retry_at(), Some(soon + RETRY_FIRST));
}
