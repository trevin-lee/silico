//! `timeline` (docs/design/m5.0-app.md, section 7, M5.1; its test plan's `timeline`
//! bullet), through the sessions' own command: each feature's health as the model named
//! gave it, excepting what a check of that model excepts, the model's revision carried,
//! and a document the gate or the reader refused answered, not refused. Every expected
//! value is by hand: each feature's health and counts written out, from what the fake
//! kernel builds and measures (boxes and their fuse by formula, as [`STORED`] says).

use super::*;
use crate::WarningCode;
use crate::timeline::{Entry, Health};

/// [`STORED`] with no fingerprint stored for the lug: a feature that builds with none, so
/// a check warns of it (`fingerprint_missing`) unless an open transaction's edits reach it.
fn unprinted() -> String {
    let text = STORED.replace(
        r#", "fingerprint": {"volume": 1000, "area": 600, "bbox": [100, 0, 0, 110, 10, 10], "faces": 6, "edges": 12, "vertices": 8}"#,
        "",
    );
    assert_ne!(text, STORED);
    text
}

/// `text` with the lug 20 long: an edit that reaches the lug and the fuse that uses it.
fn lengthened(text: &str) -> String {
    let longer = text.replace(
        r#""length": 10, "width": 10"#,
        r#""length": 20, "width": 10"#,
    );
    assert_ne!(longer, text);
    longer
}

/// [`STORED`]'s features, in timeline order, each with its health, problems, and warnings.
fn stored(marks: [(Health, u64, u64); 3]) -> Vec<Entry> {
    let [block, lug, both] = marks;
    vec![
        entry("k7f2qx3m", "box", "Block", &[], block),
        entry("b0000001", "box", "Lug", &[], lug),
        entry("f0000001", "fuse", "Both", &["k7f2qx3m", "b0000001"], both),
    ]
}

fn entry(
    feature: &str,
    kind: &'static str,
    name: &str,
    inputs: &[&str],
    (health, problems, warnings): (Health, u64, u64),
) -> Entry {
    Entry {
        id: id(feature),
        kind,
        name: name.to_owned(),
        suppressed: false,
        inputs: inputs.iter().map(|input| id(input)).collect(),
        health,
        problems,
        warnings,
    }
}

const HEALTHY: (Health, u64, u64) = (Health::Healthy, 0, 0);
const UNKNOWN: (Health, u64, u64) = (Health::Unknown, 0, 0);
/// The block's drift: its volume stored 1e-6 mm³ over the 8000 it measures.
const DRIFTED: (Health, u64, u64) = (Health::Error, 1, 0);

impl Fixture {
    fn timeline(&self, model: ModelChoice) -> Response<TimelineAnswer> {
        self.sessions.timeline(&self.id, model).wait()
    }

    fn check_current(&self) -> CheckAnswer {
        self.sessions
            .check(&self.id, CheckModel::Current { fresh: false })
            .wait()
            .result
            .unwrap()
    }
}

/// A timeline's answer of a model of `model`, at the session's `revision`, with
/// `features` and the bodies `bodies`.
fn answer(revision: u64, model: u64, features: Vec<Entry>, bodies: &[&str]) -> TimelineAnswer {
    TimelineAnswer {
        revision: Revision(revision),
        model: Some(Revision(model)),
        problems: 0,
        features: Some(features),
        bodies: bodies.iter().map(|body| id(body)).collect(),
    }
}

/// A check's problems, or its warnings, each by its code and the feature it's on.
type Said<C> = Vec<(C, String)>;

/// What a check's problems and warnings are, by code and feature.
fn checked_as(check: &CheckAnswer) -> (Said<ErrorCode>, Said<WarningCode>) {
    (
        check
            .problems
            .errors()
            .iter()
            .map(|e| (e.code(), e.feature().unwrap().to_owned()))
            .collect(),
        check
            .warnings
            .listed()
            .iter()
            .map(|w| (w.code(), w.feature().unwrap().to_owned()))
            .collect(),
    )
}

#[test]
fn a_timelines_marks_are_its_checks_outside_and_during_an_open_transaction() {
    let f = opened(&unprinted());
    // Outside a transaction: the block drifts (an error, as check's problem), the lug has
    // no fingerprint (a warning, as check's), and the fuse matches its 9000, area 3400.
    let check = f.check_current();
    assert_eq!(
        checked_as(&check),
        (
            vec![(ErrorCode::Drift, "k7f2qx3m".to_owned())],
            vec![(WarningCode::FingerprintMissing, "b0000001".to_owned())],
        )
    );
    let response = f.timeline(ModelChoice::Current);
    assert_eq!(
        response.result.unwrap(),
        answer(
            1,
            1,
            stored([DRIFTED, (Health::Warning, 0, 1), HEALTHY]),
            &["f0000001"]
        )
    );
    // During one whose edit lengthens the lug, reaching it and the fuse: the lug's missing
    // fingerprint and the fuse's drift (9000 stored, 8000 + 2000 = 10000 rebuilt) are what
    // its commit writes, so check says neither, and the timeline marks both healthy. The
    // block, unreached, still drifts.
    f.begin();
    assert_eq!(f.edit(&lengthened(&unprinted())), Revision(2));
    let check = f.check_current();
    assert_eq!(
        checked_as(&check),
        (vec![(ErrorCode::Drift, "k7f2qx3m".to_owned())], vec![])
    );
    let current = answer(2, 2, stored([DRIFTED, HEALTHY, HEALTHY]), &["f0000001"]);
    let response = f.timeline(ModelChoice::Current);
    assert_eq!(
        response.model,
        Some(ModelRead {
            revision: Revision(2),
            of: ModelChoice::Current,
            fault: None,
        })
    );
    assert_eq!(response.result.unwrap(), current);
    // The published model is revision 2's now, read beside the queue: the same marks.
    let response = f.timeline(ModelChoice::Published);
    assert_eq!(
        response.model,
        Some(ModelRead {
            revision: Revision(2),
            of: ModelChoice::Published,
            fault: None,
        })
    );
    assert_eq!(response.result.unwrap(), current);
}

#[test]
fn the_published_model_older_than_the_session_marks_what_changed_since_unknown_with_its_dependents()
{
    let f = opened(STORED);
    assert_eq!(
        f.timeline(ModelChoice::Current).result.unwrap().model,
        Some(Revision(1))
    );
    f.begin();
    // The lug 20 long is held, so revision 2's rebuild, which the published read starts,
    // doesn't end while it's read.
    let hold = f.kernel.hold(
        |op| matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx > 15.0 && *dx < 25.0),
    );
    assert_eq!(f.edit(&longer()), Revision(2));
    // Revision 1's model under revision 2's document: the lug changed since, so it and the
    // fuse that uses it are unknown; the block, unchanged, drifts as revision 1's model has
    // it. The fuse's solid stays the body while its rebuild runs.
    let response = f
        .sessions
        .timeline(&f.id, ModelChoice::Published)
        .wait_timeout(WAIT)
        .ok()
        .unwrap();
    assert_eq!(
        response.model,
        Some(ModelRead {
            revision: Revision(1),
            of: ModelChoice::Published,
            fault: None,
        })
    );
    assert_eq!(response.session.unwrap().revision, Revision(2));
    assert_eq!(
        response.result.unwrap(),
        answer(2, 1, stored([DRIFTED, UNKNOWN, UNKNOWN]), &["f0000001"])
    );
    // The current model waits for revision 2's rebuild: built, and the drift of what the
    // edit reached excepted, as check excepts it.
    hold.release();
    assert_eq!(
        f.timeline(ModelChoice::Current).result.unwrap(),
        answer(2, 2, stored([DRIFTED, HEALTHY, HEALTHY]), &["f0000001"])
    );
}

#[test]
fn a_failed_feature_is_an_error_and_so_is_what_it_blocks() {
    // The lug, 10 long as stored, can't be made: its failure is a problem on it, and the
    // fuse, blocked on it, didn't compute, an error with no problem of its own (check gives
    // the lug's and the block's drift, nothing of the fuse). The block is the body, its
    // consumer not built.
    let f = opened(STORED);
    f.kernel.refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx > 5.0 && *dx < 15.0)
            .then(|| Error::new(ErrorCode::KernelFailed, "the lug can't be made"))
    });
    assert_eq!(
        checked_as(&f.check_current()).0,
        [
            (ErrorCode::KernelFailed, "b0000001".to_owned()),
            (ErrorCode::Drift, "k7f2qx3m".to_owned())
        ]
    );
    assert_eq!(
        f.timeline(ModelChoice::Current).result.unwrap(),
        answer(
            1,
            1,
            stored([DRIFTED, (Health::Error, 1, 0), (Health::Error, 0, 0)]),
            &["k7f2qx3m"]
        )
    );
}

#[test]
fn a_kernel_fault_is_unknown_and_so_is_what_it_alone_blocks() {
    // The lug crashes the worker (decision 32): nothing of the document, so the lug and the
    // fuse it stops are unknown, and the fault is given beside the result.
    let f = opened(STORED);
    f.kernel.refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx > 5.0 && *dx < 15.0)
            .then(|| Error::new(ErrorCode::WorkerCrashed, "the lug crashed the worker"))
    });
    let response = f.timeline(ModelChoice::Current);
    assert_eq!(
        the_fault(&response),
        (ErrorCode::WorkerCrashed, Some("b0000001".to_owned()))
    );
    assert_eq!(
        response.result.unwrap(),
        answer(1, 1, stored([DRIFTED, UNKNOWN, UNKNOWN]), &["k7f2qx3m"])
    );
}

#[test]
fn a_document_the_gate_refuses_is_every_feature_unknown_with_the_gates_count() {
    // The block 0 long: the gate refuses it (one problem, `length`), so nothing is built,
    // and both models answer the document as read, none of it marked.
    let f = opened(&STORED.replace(r#""length": 40"#, r#""length": 0"#));
    let refused = TimelineAnswer {
        revision: Revision(1),
        model: None,
        problems: 1,
        features: Some(stored([UNKNOWN; 3])),
        bodies: vec![],
    };
    for choice in ModelChoice::ALL {
        let response = f.timeline(choice);
        assert_eq!(response.model, None, "{choice:?}");
        // The session the envelope says, beside the queue too: its revision, the answer's.
        assert_eq!(
            response.session.as_ref().unwrap().revision,
            Revision(1),
            "{choice:?}"
        );
        assert_eq!(response.result.unwrap(), refused, "{choice:?}");
    }
    assert!(f.kernel.calls().is_empty());
}

#[test]
fn a_refused_documents_count_is_every_problem_the_gate_found() {
    // The block and the lug both 0 long: two problems, one each, `length`.
    let f = opened(
        &STORED.replace(r#""length": 40"#, r#""length": 0"#).replace(
            r#""length": 10, "width": 10"#,
            r#""length": 0, "width": 10"#,
        ),
    );
    for choice in ModelChoice::ALL {
        assert_eq!(
            f.timeline(choice).result.unwrap(),
            TimelineAnswer {
                revision: Revision(1),
                model: None,
                problems: 2,
                features: Some(stored([UNKNOWN; 3])),
                bodies: vec![],
            },
            "{choice:?}"
        );
    }
}

#[test]
fn a_document_refused_since_the_published_model_is_every_feature_unknown() {
    // Revision 1's model is published; the file is then changed outside the session to
    // leave the block 0 long, which the gate refuses, and the current timeline's command
    // reloads it, at revision 2: no model of it, so none named and no bodies. The published
    // model, revision 1's, isn't of this document: every feature is unknown, none of
    // revision 1's marks (the block's drift) shown; but the timeline names it, as the
    // response does, and gives its body, the fuse, which a client still draws.
    let f = opened(STORED);
    assert_eq!(
        f.timeline(ModelChoice::Current).result.unwrap().model,
        Some(Revision(1))
    );
    std::fs::write(&f.path, STORED.replace(r#""length": 40"#, r#""length": 0"#)).unwrap();
    let refused = TimelineAnswer {
        revision: Revision(2),
        model: None,
        problems: 1,
        features: Some(stored([UNKNOWN; 3])),
        bodies: vec![],
    };
    let current = f.timeline(ModelChoice::Current);
    assert!(current.session.unwrap().reloaded);
    assert_eq!(current.result.unwrap(), refused);
    // Read beside the queue, of the published model, which is revision 1's still.
    assert_eq!(
        f.sessions.published(&f.id).result.unwrap().revision,
        Revision(1)
    );
    let published = f.timeline(ModelChoice::Published);
    assert_eq!(
        published.model,
        Some(ModelRead {
            revision: Revision(1),
            of: ModelChoice::Published,
            fault: None,
        })
    );
    assert_eq!(
        published.result.unwrap(),
        TimelineAnswer {
            model: Some(Revision(1)),
            bodies: vec![id("f0000001")],
            ..refused
        }
    );
}

#[test]
fn text_that_doesnt_read_as_a_document_has_no_features() {
    let f = opened(r#"{"silico": 1, "units": "#);
    for choice in ModelChoice::ALL {
        let response = f.timeline(choice);
        assert_eq!(response.model, None, "{choice:?}");
        assert_eq!(
            response.result.unwrap(),
            TimelineAnswer {
                revision: Revision(1),
                model: None,
                problems: 1,
                features: None,
                bodies: vec![],
            },
            "{choice:?}"
        );
    }
}

#[test]
fn a_timeline_is_named_by_status_and_stopped_by_cancel_queued_or_read_beside_the_queue() {
    let f = opened(BOX);
    let (started, was_started) = mpsc::channel();
    let (through, wait) = mpsc::channel();
    f.session().gate.shut(started, wait);
    for (choice, running, reads) in [
        (
            ModelChoice::Current,
            Some(RunningCommand::Command(SessionCommand::Timeline)),
            vec![],
        ),
        (ModelChoice::Published, None, vec![ReadCommand::Timeline]),
    ] {
        let timeline = f.sessions.timeline(&f.id, choice);
        was_started.recv_timeout(WAIT).unwrap();
        let status = f.sessions.status(&f.id).result.unwrap();
        assert_eq!(
            (status.running, status.reads),
            (running, reads),
            "{choice:?}"
        );
        assert_eq!(
            f.sessions
                .cancel(&f.id, SessionCommand::Timeline)
                .result
                .unwrap(),
            Cancelled {
                command: SessionCommand::Timeline.into(),
                stopped: true
            }
        );
        through.send(()).unwrap();
        assert_eq!(
            timeline.wait().result.unwrap_err().errors(),
            [cancelled("timeline")],
            "{choice:?}"
        );
    }
    // Every transport names it (M5.1's wire): a command of the wire's, by its own name,
    // and a read of the published model `status` lists.
    assert_eq!(
        SessionCommand::Timeline.command(),
        crate::command::Command::Timeline
    );
    assert_eq!(SessionCommand::Timeline.as_str(), "timeline");
    assert!(SessionCommand::ALL.contains(&SessionCommand::Timeline));
    assert!(ReadCommand::ALL.contains(&ReadCommand::Timeline));
}

/// The lug 20 long (revision 2), its model published by a current timeline, then the block
/// 50 long (revision 3), its rebuild held, all in one transaction: so the published model is
/// revision 2's, older than the session's, and what revision 2's edit reached (the lug and
/// the fuse) is excepted as a check of it excepts it.
fn older_with_the_lug_reached(f: &Fixture) -> crate::fake::Hold {
    f.begin();
    assert_eq!(f.edit(&longer()), Revision(2));
    assert_eq!(
        f.timeline(ModelChoice::Current).result.unwrap().model,
        Some(Revision(2))
    );
    let hold = f.kernel.hold(
        |op| matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx > 45.0 && *dx < 55.0),
    );
    assert_eq!(
        f.edit(&longer().replace(r#""length": 40"#, r#""length": 50"#)),
        Revision(3)
    );
    hold
}

#[test]
fn an_older_published_model_excepts_what_its_revisions_edits_reached() {
    // Revision 2's model under revision 3's document. Revision 2's check: the block drifts
    // (8000.000001 stored, 8000 built); the lug's (1000 stored, 2000 built) and the fuse's
    // (9000 stored, 10000 built) drift is excepted, revision 2's edit having reached them.
    // Revision 3 changed the block, so it and the fuse that uses it are unknown; the lug,
    // unchanged since, is as revision 2's check has it, healthy. The fuse, which revision 2
    // built and nothing consumes, is the body.
    let f = opened(STORED);
    let hold = older_with_the_lug_reached(&f);
    let check = f.sessions.check(&f.id, CheckModel::Published).wait();
    assert_eq!(check.model.as_ref().unwrap().revision, Revision(2));
    assert_eq!(
        checked_as(&check.result.unwrap()),
        (vec![(ErrorCode::Drift, "k7f2qx3m".to_owned())], vec![])
    );
    let response = f.timeline(ModelChoice::Published);
    assert_eq!(response.model.as_ref().unwrap().revision, Revision(2));
    assert_eq!(
        response.result.unwrap(),
        answer(3, 2, stored([UNKNOWN, HEALTHY, UNKNOWN]), &["f0000001"])
    );
    hold.release();
}

#[test]
fn an_older_published_model_spares_a_fingerprint_the_document_stores_now() {
    // Revision 1's model, built from a document with no fingerprint for the lug, under
    // revision 2's, whose edit stores the lug's (1000, area 600, as the fake measures it:
    // a fingerprint, which changes nothing the model builds) and makes the block 50 long
    // (held, so revision 2's rebuild never publishes). Its check doesn't warn of the lug's
    // missing fingerprint, which the session's document stores now: the lug is healthy, as
    // revision 1's model marks it. The block changed since, so it and the fuse are unknown.
    let f = opened(&unprinted());
    assert_eq!(
        f.timeline(ModelChoice::Current).result.unwrap(),
        answer(
            1,
            1,
            stored([DRIFTED, (Health::Warning, 0, 1), HEALTHY]),
            &["f0000001"]
        )
    );
    f.begin();
    let hold = f.kernel.hold(
        |op| matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx > 45.0 && *dx < 55.0),
    );
    assert_eq!(
        f.edit(&STORED.replace(r#""length": 40"#, r#""length": 50"#)),
        Revision(2)
    );
    let check = f.sessions.check(&f.id, CheckModel::Published).wait();
    assert_eq!(check.model.as_ref().unwrap().revision, Revision(1));
    assert_eq!(
        checked_as(&check.result.unwrap()),
        (vec![(ErrorCode::Drift, "k7f2qx3m".to_owned())], vec![])
    );
    let response = f.timeline(ModelChoice::Published);
    assert_eq!(response.model.as_ref().unwrap().revision, Revision(1));
    assert_eq!(
        response.result.unwrap(),
        answer(2, 1, stored([UNKNOWN, HEALTHY, UNKNOWN]), &["f0000001"])
    );
    hold.release();
}
