//! Break-it for M4.7's `read_logs`: hostile requests on the wire, hostile text in the events
//! a session keeps (an error's words, which a kernel's refusal or a document can make
//! anything), huge events, reads racing rebuilds, cursors of another session, and a read
//! answered after the sessions are gone. Alone in its process (nextest runs each test in
//! its own), each test sets the process's subscriber to the core's layer alone.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use silico_core::cache::Cache;
use silico_core::command::logs::{LOG_READ_BYTES, LogEvent, LogLevel, LogQuery, Logs};
use silico_core::command::{CheckModel, SessionId, Sessions};
use silico_core::fake::FakeKernel;
use silico_core::kernel::Op;
use silico_core::{Error, ErrorCode};
use tempfile::TempDir;
use tracing_subscriber::layer::SubscriberExt;

use crate::sessions::{answer, id, refusal};

/// A box and a fillet of one of its edges, `fillet` its ID.
fn rounded(fillet: &str) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}}, {{"id": "{fillet}", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge1"], "radius": 2}}]}}"#
    )
}

fn keep_logs() {
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(silico_core::command::logs::layer()),
    )
    .unwrap();
}

/// A fake kernel that refuses every fillet with `message` (and `cause`, if given).
fn refusing_fillets_with(message: String, cause: Option<String>) -> Arc<FakeKernel> {
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    kernel.refuse(move |op| {
        matches!(op, Op::Fillet { .. }).then(|| {
            let error = Error::new(ErrorCode::KernelFailed, message.clone());
            match &cause {
                Some(cause) => error.with_cause(cause.clone()),
                None => error,
            }
        })
    });
    kernel
}

fn sessions(kernel: Arc<FakeKernel>, dir: &TempDir) -> Sessions<Arc<FakeKernel>> {
    let secret = dir.path().join("secret");
    Sessions::with_cache_opener(kernel, move |cache| Cache::with_secret(cache, &secret))
}

fn opened(
    sessions: &Sessions<Arc<FakeKernel>>,
    dir: &TempDir,
    name: &str,
    text: &str,
) -> SessionId {
    let path = dir.path().join(name);
    std::fs::write(&path, text).unwrap();
    answer(sessions.open(&path, false)).session.unwrap().id
}

fn query(level: LogLevel, feature: Option<&str>, after: Option<u64>, limit: u32) -> LogQuery {
    LogQuery {
        level,
        feature: feature.map(id),
        after,
        limit,
    }
}

fn read(sessions: &Sessions<Arc<FakeKernel>>, session: &SessionId, query: &LogQuery) -> Logs {
    sessions.read_logs(session, query).result.unwrap()
}

fn checked(sessions: &Sessions<Arc<FakeKernel>>, session: &SessionId, fresh: bool) {
    let response = answer(sessions.check(session, CheckModel::Current { fresh }));
    assert!(response.result.is_ok(), "{response:?}");
}

/// The text of an event as a read counts it.
fn bytes(event: &LogEvent) -> usize {
    event.message.len()
        + event.feature.as_ref().map_or(0, |f| f.as_str().len())
        + event.spans.iter().map(String::len).sum::<usize>()
}

/// Every hostile character JSON has to escape, and some it needn't but a careless writer
/// mangles: quotes, backslashes, every C0 control (NUL among them), DEL, the line and
/// paragraph separators, a BOM, a lone combining mark, and characters past the BMP.
fn hostile_text() -> String {
    let mut text = String::from("q\"b\\s/ ");
    for c in 0u32..0x20 {
        text.push(char::from_u32(c).unwrap());
    }
    text.push_str("\u{7f}\u{2028}\u{2029}\u{feff}\u{301}😀𝄞 }]\"{");
    text
}

#[test]
fn hostile_text_in_an_event_comes_back_through_read_logs_json_exactly() {
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let hostile = hostile_text();
    let sessions = sessions(
        refusing_fillets_with(hostile.clone(), Some(hostile.clone())),
        &dir,
    );
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    checked(&sessions, &s1, false);
    let called = crate::calls::call(
        &sessions,
        "read_logs",
        r#"{"session": "s1", "level": "info", "feature": "f0000001", "after": null, "limit": 10}"#,
    );
    // The envelope is JSON, whatever the event holds.
    let value: serde_json::Value = serde_json::from_str(&called.text)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", called.text));
    assert_eq!(value["ok"], true, "{}", called.text);
    let events = value["result"]["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{}", called.text);
    // Exactly the error's words, then its code and cause, as the step logged them.
    assert_eq!(
        events[0]["message"].as_str().unwrap(),
        format!("the step failed: {hostile} code=kernel_failed cause={hostile}")
    );
    // And the same as the Rust API reads it.
    let direct = read(
        &sessions,
        &s1,
        &query(LogLevel::Info, Some("f0000001"), None, 10),
    );
    assert_eq!(
        direct.events[0].message,
        events[0]["message"].as_str().unwrap()
    );
}

#[test]
fn a_refusal_of_two_mebibytes_is_cut_to_fit_one_read_at_a_characters_boundary() {
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    // Two-byte and four-byte characters, so a cut that ignores boundaries lands inside one.
    let huge = "é😀".repeat((2 << 20) / 6 + 1);
    let sessions = sessions(refusing_fillets_with(huge.clone(), None), &dir);
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    checked(&sessions, &s1, false);
    let logs = read(
        &sessions,
        &s1,
        &query(LogLevel::Info, Some("f0000001"), None, 1000),
    );
    assert_eq!(logs.events.len(), 1, "{:?}", logs.next);
    let event = &logs.events[0];
    assert!(event.message.starts_with("the step failed: é😀é😀"));
    assert!(event.message.ends_with('…'), "not marked as cut");
    assert!(bytes(event) <= LOG_READ_BYTES, "{}", bytes(event));
    // As close to the bound as a character allows: within a 4-byte character and the mark.
    assert!(bytes(event) > LOG_READ_BYTES - 4, "{}", bytes(event));
    // Through the wire too: the envelope parses, and holds it whole.
    let called = crate::calls::call(
        &sessions,
        "read_logs",
        r#"{"session": "s1", "level": "info", "feature": "f0000001", "after": null, "limit": 1000}"#,
    );
    let value = crate::calls::envelope(&called);
    assert_eq!(
        value["result"]["events"][0]["message"].as_str().unwrap(),
        event.message
    );
    // The session goes on: a fresh check logs the refusal again, numbered on.
    checked(&sessions, &s1, true);
    let again = read(
        &sessions,
        &s1,
        &query(LogLevel::Info, Some("f0000001"), Some(logs.next), 1000),
    );
    assert_eq!(again.events.len(), 1);
    assert!(again.events[0].seq > event.seq);
}

#[test]
fn a_mebibyte_of_nuls_each_escaped_six_fold_still_answers_as_json_and_pages_by_its_bytes() {
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    // NULs: each written `\u0000`, six bytes for one, so a read of 1 MiB of text is some
    // 6 MiB of JSON, well under the response's 128 MiB.
    let nuls = "\0".repeat(LOG_READ_BYTES / 3);
    let sessions = sessions(refusing_fillets_with(nuls, None), &dir);
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    for fresh in [false, true, true] {
        checked(&sessions, &s1, fresh);
    }
    let called = crate::calls::call(
        &sessions,
        "read_logs",
        r#"{"session": "s1", "level": "info", "feature": "f0000001", "after": null, "limit": 1000}"#,
    );
    let value = crate::calls::envelope(&called);
    let events = value["result"]["events"].as_array().unwrap();
    // Two fit in a mebibyte: the read stopped at its bytes, its cursor the second.
    assert_eq!(events.len(), 2, "{}", &called.text[..200]);
    assert_eq!(value["result"]["next"], events[1]["seq"]);
    assert!(called.text.len() < 128 << 20);
}

/// Each refusal a bad `read_logs` request gets on the wire: its code and the parameter it
/// names.
fn wire_refusal(sessions: &Sessions<Arc<FakeKernel>>, arguments: &str) -> (String, String) {
    let called = crate::calls::call(sessions, "read_logs", arguments);
    let value = crate::calls::envelope(&called);
    assert_eq!(value["ok"], false, "{arguments}: {}", called.text);
    let error = &value["errors"][0];
    (
        error["code"].as_str().unwrap().to_owned(),
        error["parameter"].as_str().unwrap_or("").to_owned(),
    )
}

#[test]
fn hostile_read_logs_requests_are_refused_naming_their_field_and_leave_the_session_readable() {
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions(refusing_fillets_with("no".to_owned(), None), &dir);
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    checked(&sessions, &s1, false);
    let ok = |level: &str, feature: &str, after: &str, limit: &str| {
        format!(
            r#"{{"session": "s1", "level": {level}, "feature": {feature}, "after": {after}, "limit": {limit}}}"#
        )
    };
    let cases: Vec<(String, &str)> = vec![
        (ok("\"info\"", "null", "null", "0"), "limit"),
        (ok("\"info\"", "null", "null", "1001"), "limit"),
        (ok("\"info\"", "null", "null", "4294967295"), "limit"),
        (ok("\"info\"", "null", "null", "4294967296"), "limit"),
        (ok("\"info\"", "null", "null", "-1"), "limit"),
        (ok("\"info\"", "null", "null", "1.5"), "limit"),
        (ok("\"info\"", "null", "null", "1e2"), "limit"),
        (ok("\"info\"", "null", "null", "null"), "limit"),
        (ok("\"info\"", "null", "null", "\"10\""), "limit"),
        (ok("\"info\"", "null", "-1", "10"), "after"),
        (ok("\"info\"", "null", "1.0", "10"), "after"),
        (ok("\"info\"", "null", "\"5\"", "10"), "after"),
        (
            ok("\"info\"", "null", "18446744073709551615", "10"),
            "after",
        ),
        (
            ok("\"info\"", "null", "18446744073709551616", "10"),
            "after",
        ),
        (
            ok("\"info\"", "null", "99999999999999999999999999999", "10"),
            "after",
        ),
        (ok("\"info\"", "null", "true", "10"), "after"),
        (ok("\"INFO\"", "null", "null", "10"), "level"),
        (ok("\"warning\"", "null", "null", "10"), "level"),
        (ok("\"\"", "null", "null", "10"), "level"),
        (ok("null", "null", "null", "10"), "level"),
        (ok("3", "null", "null", "10"), "level"),
        (ok("\"info\\u0000\"", "null", "null", "10"), "level"),
        (ok("\"info\"", "\"F0000001\"", "null", "10"), "feature"),
        (ok("\"info\"", "\"\"", "null", "10"), "feature"),
        (ok("\"info\"", "\"f000000\"", "null", "10"), "feature"),
        (ok("\"info\"", "\"f0000001 \"", "null", "10"), "feature"),
        (ok("\"info\"", "[\"f0000001\"]", "null", "10"), "feature"),
        (ok("\"info\"", "7", "null", "10"), "feature"),
    ];
    for (arguments, field) in &cases {
        let (code, parameter) = wire_refusal(&sessions, arguments);
        assert_eq!(code, "invalid_input", "{arguments}");
        assert_eq!(parameter, *field, "{arguments}");
    }
    // A missing field, an extra one, and a session that isn't one.
    for (arguments, field) in [
        (
            r#"{"session": "s1", "level": "info", "feature": null, "after": null}"#,
            "limit",
        ),
        (
            r#"{"session": "s1", "level": "info", "feature": null, "after": null, "limit": 1, "since": 3}"#,
            "since",
        ),
        (
            r#"{"session": "s99", "level": "info", "feature": null, "after": null, "limit": 1}"#,
            "session",
        ),
        (
            r#"{"session": "S1", "level": "info", "feature": null, "after": null, "limit": 1}"#,
            "session",
        ),
    ] {
        let (_, parameter) = wire_refusal(&sessions, arguments);
        assert_eq!(parameter, field, "{arguments}");
    }
    // Not JSON at all, and JSON that isn't an object: refused, not a panic.
    for arguments in [
        "",
        "{",
        "[]",
        "null",
        "\"s1\"",
        "{\"session\": \"s1\"} trailing",
        r#"{"session": "s1", "level": "info", "feature": null, "after": null, "limit": 010}"#,
        r#"{"session": "s1", "level": "info", "feature": null, "after": 01, "limit": 1}"#,
        r#"{"session": "s1", "session": "s1", "level": "info", "feature": null, "after": null, "limit": 1}"#,
    ] {
        let called = crate::calls::call(&sessions, "read_logs", arguments);
        let value = crate::calls::envelope(&called);
        assert_eq!(value["ok"], false, "{arguments:?}: {}", called.text);
    }
    // Afterwards, the session reads as before, the refusals having added nothing.
    let after = read(&sessions, &s1, &query(LogLevel::Trace, None, None, 1000));
    let checked_events = after.events.len();
    let again = read(&sessions, &s1, &query(LogLevel::Trace, None, None, 1000));
    assert_eq!(again.events.len(), checked_events);
    assert_eq!(again.next, after.next);
}

#[test]
fn a_cursor_of_one_session_read_on_a_quieter_one_is_refused_naming_after() {
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions(refusing_fillets_with("no".to_owned(), None), &dir);
    let s1 = opened(&sessions, &dir, "one.silico", &rounded("f0000001"));
    let s2 = opened(&sessions, &dir, "two.silico", &rounded("f0000002"));
    for fresh in [false, true, true] {
        checked(&sessions, &s1, fresh);
    }
    let busy = read(&sessions, &s1, &query(LogLevel::Trace, None, None, 1000)).next;
    let quiet = read(&sessions, &s2, &query(LogLevel::Trace, None, None, 1000)).next;
    assert!(busy > quiet, "{busy} {quiet}");
    let response = sessions.read_logs(&s2, &query(LogLevel::Trace, None, Some(busy), 10));
    let (code, message, parameter, _) = refusal(&response);
    assert_eq!(code, ErrorCode::InvalidInput);
    assert_eq!(parameter.as_deref(), Some("after"));
    assert!(message.contains(&format!("at most {quiet}")), "{message}");
    // The largest cursor of all: refused, never an overflow.
    let response = sessions.read_logs(&s2, &query(LogLevel::Trace, None, Some(u64::MAX), 10));
    assert_eq!(refusal(&response).2.as_deref(), Some("after"));
}

#[test]
fn read_logs_called_and_answered_after_the_sessions_are_dropped_is_refused_not_a_hang() {
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions(refusing_fillets_with("no".to_owned(), None), &dir);
    let _s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    let calling = crate::calls::calling(
        &sessions,
        "read_logs",
        r#"{"session": "s1", "level": "info", "feature": null, "after": null, "limit": 10}"#,
    );
    drop(sessions);
    let called = crate::calls::answered(calling);
    let value = crate::calls::envelope(&called);
    assert_eq!(value["ok"], false, "{}", called.text);
    assert_eq!(
        value["errors"][0]["parameter"], "session",
        "{}",
        called.text
    );
}

#[test]
fn pages_read_while_rebuilds_log_never_repeat_skip_or_reorder_an_event() {
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let sessions = Arc::new(sessions(refusing_fillets_with("no".to_owned(), None), &dir));
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (sessions, stop) = (Arc::clone(&sessions), Arc::clone(&stop));
        std::thread::spawn(move || {
            let mut n = 0;
            while !stop.load(Ordering::Relaxed) && n < 200 {
                checked(&sessions, &s1, true);
                n += 1;
            }
        })
    };
    // Pages of 7, from the oldest, each from the last's cursor, until the writer stops and
    // the last page is empty.
    let mut seen: Vec<LogEvent> = Vec::new();
    let mut after = None;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let page = read(&sessions, &s1, &query(LogLevel::Trace, None, after, 7));
        assert_eq!(
            page.dropped, 0,
            "200 checks of a fillet stay within the bound"
        );
        assert!(page.events.len() <= 7);
        if let Some(last) = page.events.last() {
            assert_eq!(page.next, last.seq);
        }
        after = Some(page.next);
        let empty = page.events.is_empty();
        seen.extend(page.events);
        if empty && writer.is_finished() {
            break;
        }
        if Instant::now() > deadline {
            stop.store(true, Ordering::Relaxed);
        }
    }
    writer.join().unwrap();
    let seqs: Vec<u64> = seen.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>());
    assert!(seen.windows(2).all(|w| w[0].at_ms <= w[1].at_ms));
    // And a read of them all at once agrees, page for page.
    let mut whole = Vec::new();
    let mut after = None;
    loop {
        let page = read(&sessions, &s1, &query(LogLevel::Trace, None, after, 1000));
        if page.events.is_empty() {
            break;
        }
        after = Some(page.next);
        whole.extend(page.events);
    }
    assert_eq!(whole, seen);
}

// Found by break-it: a process that keeps no session's logs answered a read with a cursor
// as one past the newest event ("after must be at most 0"), checked first, so an agent
// resending the last read's `next` was told to fix its cursor, never that this process keeps
// no logs at all, which no cursor fixes. The buffer's keeping is now checked first.
#[test]
fn a_cursor_read_in_a_process_that_keeps_no_logs_says_it_keeps_none() {
    // No subscriber set.
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions(refusing_fillets_with("no".to_owned(), None), &dir);
    let s1 = opened(&sessions, &dir, "part.silico", &rounded("f0000001"));
    checked(&sessions, &s1, false);
    let response = sessions.read_logs(&s1, &query(LogLevel::Trace, None, Some(5), 10));
    let (code, message, parameter, _) = refusal(&response);
    assert_eq!(
        (code, parameter.as_deref()),
        (ErrorCode::Internal, None),
        "{message}"
    );
}
