//! M4.7: `read_logs` (docs/design/m4.1-command-api.md, section 7, and #50's M4.7 row): a
//! session's events, kept by the process's subscriber through the core's log layer, each
//! under the command that made it, read by level, feature, and a cursor; never another
//! session's; answered at once, beside a command running, adding nothing of its own. Alone
//! in its process (nextest runs each test in its own), each test sets the process's tracing
//! subscriber, as `silico_core::logging::init` does: the core's layer and nothing else.
//!
//! The fake kernel's refusals stand for a kernel worker's here, so the events of a failed
//! fillet are the core's: the worker's own lines are read on a real one (silico-kernel's
//! tests/session_logs.rs).

use std::sync::Arc;

use silico_core::ErrorCode;
use silico_core::command::logs::{LogEvent, LogLevel, Logs};
use silico_core::command::{CheckModel, Response, SessionId, Sessions};
use silico_core::fake::FakeKernel;
use silico_core::kernel::Op;
use silico_core::{Error, document::FeatureId};
use tracing_subscriber::layer::SubscriberExt;

use crate::logged::{
    checked, every, keep_logs, opened, query, read, refusing_fillets, rounded, sessions,
};
use crate::sessions::{answer, id, refusal};

fn messages(events: &[LogEvent]) -> Vec<&str> {
    events.iter().map(|e| e.message.as_str()).collect()
}

#[test]
fn a_failed_fillets_logs_read_by_its_feature_say_why_it_failed_under_the_check_that_rebuilt_it() {
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions(refusing_fillets(), &dir);
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    checked(&sessions, &s1, false);
    // The fillet's, at info and above: its step's failure, in the fake's words, under the
    // check, its rebuild, the recompute, and the step, which names the feature.
    let logs = read(
        &sessions,
        &s1,
        &query(LogLevel::Info, Some("f0000001"), None, 1000),
    );
    assert_eq!(
        messages(&logs.events),
        ["the step failed: Fillet failed code=kernel_failed"]
    );
    let event = &logs.events[0];
    assert_eq!(event.level, LogLevel::Info);
    assert_eq!(event.feature, Some(id("f0000001")));
    assert_eq!(event.spans[0], "check");
    assert_eq!(event.spans[1], "rebuild revision=1 fresh=false");
    assert_eq!(event.spans[2], "recompute generation=1 fresh=false");
    assert!(
        event.spans[3].starts_with("recompute_step feature=f0000001 key=")
            && event.spans[3].ends_with(" job=make"),
        "{:?}",
        event.spans
    );
    assert_eq!(event.spans.len(), 4, "{:?}", event.spans);
    assert_eq!(logs.dropped, 0);
    // Nothing at warn or above: a failed feature is the document's, not the process's.
    let warned = read(
        &sessions,
        &s1,
        &query(LogLevel::Warn, Some("f0000001"), None, 1000),
    );
    assert!(warned.events.is_empty(), "{:?}", warned.events);
    // The box built: none of its events is a failure.
    let block = read(
        &sessions,
        &s1,
        &query(LogLevel::Trace, Some("k7f2qx3m"), None, 1000),
    );
    assert!(
        block
            .events
            .iter()
            .all(|e| e.feature == Some(id("k7f2qx3m")) && !e.message.contains("failed")),
        "{:?}",
        block.events
    );
    // Every event, numbered from 1, one by one, its time not falling, its first span a
    // command: the open's first.
    let all = every(&sessions, &s1);
    let seqs: Vec<u64> = all.events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>());
    assert!(all.events.windows(2).all(|w| w[0].at_ms <= w[1].at_ms));
    assert_eq!(all.events[0].spans[0], "open");
    assert!(
        all.events
            .iter()
            .all(|e| matches!(e.spans[0].as_str(), "open" | "check")),
        "{:?}",
        all.events
    );
    assert_eq!(all.next, seqs.len() as u64);
    assert!(all.events.contains(event));
}

#[test]
fn a_cursor_read_again_returns_nothing_new_and_goes_on_from_where_it_left_off() {
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions(refusing_fillets(), &dir);
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    checked(&sessions, &s1, false);
    let first = every(&sessions, &s1);
    let next = first.next;
    assert!(next > 0);
    // Read again from its cursor, twice: nothing new, the cursor where it was.
    for _ in 0..2 {
        let again = read(
            &sessions,
            &s1,
            &query(LogLevel::Trace, None, Some(next), 1000),
        );
        assert!(again.events.is_empty(), "{:?}", again.events);
        assert_eq!(again.next, next);
    }
    // A fresh check: only its events, numbered on from the cursor, under it.
    checked(&sessions, &s1, true);
    let then = read(
        &sessions,
        &s1,
        &query(LogLevel::Trace, None, Some(next), 1000),
    );
    assert!(!then.events.is_empty());
    let seqs: Vec<u64> = then.events.iter().map(|e| e.seq).collect();
    assert_eq!(
        seqs,
        ((next + 1)..=(next + seqs.len() as u64)).collect::<Vec<_>>()
    );
    assert!(then.events.iter().all(|e| e.spans[0] == "check"));
    assert!(
        then.events
            .iter()
            .any(|e| e.spans.get(1).map(String::as_str) == Some("rebuild revision=1 fresh=true")),
        "{:?}",
        then.events
    );
    assert_eq!(then.next, next + seqs.len() as u64);
    // A cursor filtered past: a read of the errors finds none, and its cursor is the
    // newest, so the next read of them doesn't look at these again.
    let errors = read(
        &sessions,
        &s1,
        &query(LogLevel::Error, None, Some(next), 1000),
    );
    assert!(errors.events.is_empty());
    assert_eq!(errors.next, then.next);
}

#[test]
fn a_reads_limit_stops_it_its_next_says_where_and_limits_and_cursors_past_the_bounds_are_refused() {
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions(refusing_fillets(), &dir);
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    checked(&sessions, &s1, false);
    let all = every(&sessions, &s1);
    assert!(all.events.len() >= 3, "{:?}", all.events);
    // A page at a time, each starting where the last stopped: the same events in all.
    let mut paged = Vec::new();
    let mut after = None;
    loop {
        let page = read(&sessions, &s1, &query(LogLevel::Trace, None, after, 2));
        if page.events.is_empty() {
            assert_eq!(page.next, all.next);
            break;
        }
        assert!(page.events.len() <= 2);
        assert_eq!(page.next, page.events.last().unwrap().seq);
        after = Some(page.next);
        paged.extend(page.events);
    }
    assert_eq!(paged, all.events);
    // A limit of none, or past 1,000, is refused, naming it.
    for limit in [0, 1001, u32::MAX] {
        let response = sessions.read_logs(&s1, &query(LogLevel::Info, None, None, limit));
        assert_eq!(
            refusal(&response),
            (
                ErrorCode::InvalidInput,
                format!("limit must be from 1 to 1000, got {limit}"),
                Some("limit".to_owned()),
                Some(
                    "read in pages of at most 1000, with after set to the last read's next"
                        .to_owned()
                )
            )
        );
        // As it was: the session.
        assert_eq!(response.session.map(|s| s.id), Some(s1));
    }
    // The limit's bounds themselves are read.
    assert_eq!(
        read(&sessions, &s1, &query(LogLevel::Trace, None, None, 1))
            .events
            .len(),
        1
    );
    // A cursor past the newest event is one this session never gave: refused, naming it.
    let past = all.next + 1;
    assert_eq!(
        refusal(&sessions.read_logs(&s1, &query(LogLevel::Info, None, Some(past), 10))),
        (
            ErrorCode::InvalidInput,
            format!(
                "after must be at most {}, the newest event of s1, got {past}",
                all.next
            ),
            Some("after".to_owned()),
            Some(
                "give the last read's next, or null to read from the oldest event kept".to_owned()
            )
        )
    );
    // The newest itself is a cursor: nothing after it.
    let at = read(
        &sessions,
        &s1,
        &query(LogLevel::Info, None, Some(all.next), 10),
    );
    assert!(at.events.is_empty());
}

#[test]
fn one_sessions_logs_never_show_anothers() {
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions(refusing_fillets(), &dir);
    let s1 = opened(&sessions, &dir, "one.silico", &rounded("f0000001"));
    let s2 = opened(&sessions, &dir, "two.silico", &rounded("f0000002"));
    // Both rebuilt at once, on one kernel.
    let one = sessions.check(&s1, CheckModel::Current { fresh: false });
    let two = sessions.check(&s2, CheckModel::Current { fresh: false });
    assert!(answer(one).result.is_ok());
    assert!(answer(two).result.is_ok());
    for (session, own, other) in [(s1, "f0000001", "f0000002"), (s2, "f0000002", "f0000001")] {
        let all = every(&sessions, &session);
        // Its own fillet's failure, once; the other's never.
        let failed: Vec<&LogEvent> = all
            .events
            .iter()
            .filter(|e| e.message.starts_with("the step failed"))
            .collect();
        assert_eq!(failed.len(), 1, "{session}: {:?}", all.events);
        assert_eq!(failed[0].feature, Some(id(own)));
        assert!(
            all.events.iter().all(|e| e.feature != Some(id(other))
                && e.spans.iter().all(|span| !span.contains(other))),
            "{session}: {:?}",
            all.events
        );
        // Each its own open and check, numbered from 1.
        assert_eq!(all.events[0].seq, 1);
        assert_eq!(all.events[0].spans[0], "open");
        let by_other: Vec<FeatureId> = read(
            &sessions,
            &session,
            &query(LogLevel::Trace, Some(other), None, 1000),
        )
        .events
        .into_iter()
        .filter_map(|e| e.feature)
        .collect();
        assert!(by_other.is_empty(), "{by_other:?}");
    }
}

#[test]
fn read_logs_answers_while_a_command_runs_and_adds_no_event_of_its_own() {
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let kernel = refusing_fillets();
    let held = kernel.hold(|op| matches!(op, Op::Fillet { .. }));
    let sessions = sessions(Arc::clone(&kernel), &dir);
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    let check = sessions.check(&s1, CheckModel::Current { fresh: false });
    // The fillet's request is held: the check waits on it, and read_logs answers anyway,
    // on this thread.
    assert!(kernel.wait_running(1, crate::sessions::HANG));
    let during = every(&sessions, &s1);
    assert!(
        during.events.iter().any(|e| e.spans[0] == "check"),
        "{:?}",
        during.events
    );
    assert!(
        during
            .events
            .iter()
            .all(|e| !e.message.starts_with("the step failed")),
        "{:?}",
        during.events
    );
    // Read again: the same events, none added by the read.
    assert_eq!(every(&sessions, &s1), during);
    held.release();
    assert!(answer(check).result.is_ok());
    let after = every(&sessions, &s1);
    assert!(after.next > during.next);
    assert_eq!(after.events[..during.events.len()], during.events[..]);
}

#[test]
fn read_logs_through_call_is_its_envelope_and_a_closed_session_has_none_to_read() {
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions(refusing_fillets(), &dir);
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    checked(&sessions, &s1, false);
    let called = crate::calls::call(
        &sessions,
        "read_logs",
        r#"{"session": "s1", "level": "info", "feature": "f0000001", "after": null, "limit": 10}"#,
    );
    let value = crate::calls::envelope(&called);
    let called = &called.text;
    assert_eq!(value["ok"], true, "{called}");
    assert_eq!(value["session"]["id"], "s1");
    let events = value["result"]["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{called}");
    assert_eq!(events[0]["feature"], "f0000001");
    assert_eq!(
        events[0]["message"],
        "the step failed: Fillet failed code=kernel_failed"
    );
    // A level that isn't one is the reader's refusal, with the levels as candidates.
    let refused = crate::calls::call(
        &sessions,
        "read_logs",
        r#"{"session": "s1", "level": "verbose", "feature": null, "after": null, "limit": 10}"#,
    );
    let value = crate::calls::envelope(&refused);
    assert_eq!(value["ok"], false);
    assert_eq!(
        value["errors"][0]["candidates"],
        serde_json::json!(["error", "warn", "info", "debug", "trace"])
    );
    // Closed: none to read.
    assert!(answer(sessions.close(&s1)).result.is_ok());
    let closed = sessions.read_logs(&s1, &query(LogLevel::Info, None, None, 10));
    assert_eq!(refusal(&closed).2.as_deref(), Some("session"));
}

#[test]
fn a_process_whose_subscriber_keeps_no_session_logs_says_so() {
    // No subscriber set: the session's events are kept by none.
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions(refusing_fillets(), &dir);
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    checked(&sessions, &s1, false);
    let response: Response<Logs> = sessions.read_logs(&s1, &query(LogLevel::Trace, None, None, 10));
    let (code, message, parameter, suggestion) = refusal(&response);
    assert_eq!(code, ErrorCode::Internal);
    assert_eq!(
        message,
        "this process keeps no events of s1: its tracing subscriber has no session log layer, \
         or filters out the session's spans"
    );
    assert_eq!(parameter, None);
    assert!(
        suggestion.unwrap().contains("silico_core::logging::init"),
        "the advice names what keeps them"
    );
}

#[test]
fn the_apps_logging_keeps_each_sessions_events_at_every_level_whatever_its_log_shows() {
    // The CLI's and the server's subscriber: its log to stderr filtered by SILICO_LOG
    // (unset here: warn), the sessions' events kept beside it at every level.
    assert!(std::env::var_os("SILICO_LOG").is_none());
    silico_core::logging::init();
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions(refusing_fillets(), &dir);
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    checked(&sessions, &s1, false);
    let all = every(&sessions, &s1);
    // The fake client's start (debug) and the step's failure (info), below the log's warn.
    assert!(
        all.events
            .iter()
            .any(|e| e.level == LogLevel::Debug && e.spans == ["open"]),
        "{:?}",
        all.events
    );
    assert!(
        all.events.iter().any(|e| e.level == LogLevel::Info
            && e.message.starts_with("the step failed")
            && e.feature == Some(id("f0000001"))),
        "{:?}",
        all.events
    );
}

/// The step-failure events among `session`'s, of `feature`.
fn failures(
    sessions: &Sessions<Arc<FakeKernel>>,
    session: &SessionId,
    feature: &str,
) -> Vec<LogEvent> {
    read(
        sessions,
        session,
        &query(LogLevel::Trace, Some(feature), None, 1000),
    )
    .events
    .into_iter()
    .filter(|e| e.message.starts_with("the step failed: "))
    .collect()
}

#[test]
fn a_fillet_whose_edges_are_broken_logs_its_steps_failure_naming_the_first_broken_one() {
    // M2.8's failure: no kernel request is made, the step fails on its references. Two
    // broken, the first said (as the step's error says it), the rest in the check's result.
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions(Arc::new(FakeKernel::new(2).unwrap()), &dir);
    let text = rounded("f0000001").replace(
        r#"["k7f2qx3m.box.Edge1"]"#,
        r#"["k7f2qx3m.box.Edge98", "k7f2qx3m.box.Edge99"]"#,
    );
    let s1 = opened(&sessions, &dir, "part.silico", &text);
    checked(&sessions, &s1, false);
    let failed = failures(&sessions, &s1, "f0000001");
    assert_eq!(failed.len(), 1, "{failed:?}");
    let event = &failed[0];
    assert_eq!(event.level, LogLevel::Info);
    assert!(event.message.contains("k7f2qx3m.box.Edge98"), "{event:?}");
    assert!(!event.message.contains("Edge99"), "{event:?}");
    assert!(
        // Its fields after its message: the first's field and its name, one of one.
        event.message.ends_with(
            " code=reference_broken parameter=edges.0 elements=[\"k7f2qx3m.box.Edge98\"] \
             elements_total=1"
        ),
        "{event:?}"
    );
    assert!(
        event.spans[3].starts_with("recompute_step feature=f0000001 ")
            && event.spans[3].ends_with(" job=make"),
        "{event:?}"
    );
}

#[test]
fn a_solid_that_cant_be_measured_logs_its_measure_steps_failure() {
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    kernel.refuse(|op| {
        matches!(op, Op::Summarize { .. })
            .then(|| Error::new(ErrorCode::KernelFailed, "Summarize failed"))
    });
    let sessions = sessions(kernel, &dir);
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    checked(&sessions, &s1, false);
    // The box's measure fails (and the fillet's): each its own step's, at info.
    let failed = failures(&sessions, &s1, "k7f2qx3m");
    assert!(!failed.is_empty());
    for event in &failed {
        assert_eq!(event.level, LogLevel::Info);
        assert_eq!(
            event.message,
            "the step failed: Summarize failed code=kernel_failed"
        );
        let step = event.spans.last().unwrap();
        assert!(
            step.starts_with("recompute_step feature=k7f2qx3m ") && step.ends_with(" job=measure"),
            "{event:?}"
        );
    }
}

#[test]
fn a_cancel_is_kept_under_its_own_command_its_time_since_the_session_opened() {
    // Tests round 1's proof: the cancel's own event, filed under `cancel`, not the check's,
    // at least as late as the wait before it.
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let held = kernel.hold(|op| matches!(op, Op::Fillet { .. }));
    let sessions = sessions(Arc::clone(&kernel), &dir);
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    let check = sessions.check(&s1, CheckModel::Current { fresh: false });
    assert!(kernel.wait_running(1, crate::sessions::HANG));
    std::thread::sleep(std::time::Duration::from_millis(60));
    let cancel = sessions.cancel(&s1, silico_core::command::SessionCommand::Check);
    assert!(cancel.result.is_ok(), "{cancel:?}");
    held.release();
    let _ = answer(check);
    let all = every(&sessions, &s1);
    let cancelled: Vec<&LogEvent> = all
        .events
        .iter()
        .filter(|e| e.message.starts_with("cancelled"))
        .collect();
    assert_eq!(cancelled.len(), 1, "{:?}", all.events);
    assert_eq!(cancelled[0].spans, ["cancel"]);
    assert_eq!(cancelled[0].level, LogLevel::Info);
    assert!(cancelled[0].at_ms >= 60, "{:?}", cancelled[0]);
}

#[test]
fn a_subscriber_without_the_log_layer_keeps_no_session_logs_and_says_so() {
    // Tests round 1's proof: a registry with a log of its own, but not the layer.
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::sink)),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions(refusing_fillets(), &dir);
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    checked(&sessions, &s1, false);
    let (code, message, ..) =
        refusal(&sessions.read_logs(&s1, &query(LogLevel::Trace, None, None, 10)));
    assert_eq!(code, ErrorCode::Internal);
    assert!(
        message.starts_with("this process keeps no events of s1"),
        "{message}"
    );
}

#[test]
fn a_filter_on_the_subscriber_itself_keeps_no_session_logs_and_says_so() {
    // The layer, but under a filter of the whole subscriber at warn: the sessions' spans
    // (info) are off for every layer, so none is kept, and a read says so.
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(silico_core::command::logs::layer())
            .with(tracing_subscriber::EnvFilter::new("warn")),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions(refusing_fillets(), &dir);
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    checked(&sessions, &s1, false);
    let (code, ..) = refusal(&sessions.read_logs(&s1, &query(LogLevel::Trace, None, None, 10)));
    assert_eq!(code, ErrorCode::Internal);
}
