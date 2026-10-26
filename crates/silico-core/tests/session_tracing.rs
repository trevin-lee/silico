//! M4.2: a session's open reads its file and puts it through the gate on the session's own
//! thread (`session-s1`), never the caller's (ARCHITECTURE.md, invariant 4), and each
//! command's work traces back to the call that asked for it (invariant 9). The thread's name
//! is the one the thread runs under, read where each span is entered. Alone in its process
//! (nextest runs each test in its own), it sets the process's tracing subscriber.

use std::sync::{Arc, Mutex};

use silico_core::cache::Cache;
use silico_core::command::{ModelChoice, Sessions};
use silico_core::fake::FakeKernel;
use tracing::Subscriber;
use tracing::span::{Attributes, Id};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// Each `read`, `create`, `gate`, and `rebuild` span entered: its name, and the thread it was entered on.
type Seen = Arc<Mutex<Vec<(&'static str, String)>>>;

/// Each `session_command` span made: its ancestors' names, nearest first.
type Commands = Arc<Mutex<Vec<Vec<&'static str>>>>;

struct Entered(Seen, Commands);

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Entered {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if attrs.metadata().name() != "session_command" {
            return;
        }
        let ancestors = ctx
            .span(id)
            .map(|span| span.scope().skip(1).map(|s| s.name()).collect())
            .unwrap_or_default();
        self.1.lock().unwrap().push(ancestors);
    }

    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        if matches!(span.name(), "read" | "gate" | "create" | "rebuild") {
            let thread = std::thread::current().name().unwrap_or("").to_owned();
            self.0.lock().unwrap().push((span.name(), thread));
        }
    }
}

#[test]
fn an_opens_read_and_gate_run_on_its_sessions_thread_and_each_command_under_its_caller() {
    let seen: Seen = Arc::default();
    let commands: Commands = Arc::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(Entered(Arc::clone(&seen), Arc::clone(&commands))),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let path = dir.path().join("block.silico");
    std::fs::write(&path, r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": []}"#).unwrap();
    let sessions = Sessions::with_cache_opener(FakeKernel::new(1).unwrap(), move |cache| {
        Cache::with_secret(cache, &secret)
    });
    let response = tracing::info_span!("client_open")
        .in_scope(|| sessions.open(&path, false))
        .wait();
    assert!(response.result.is_ok(), "{response:?}");
    let s1 = response.session.unwrap().id;
    // Asked under no span: its command's span is a root, under nothing the session's thread
    // was in before.
    let text = sessions.text(&s1, None).wait();
    assert!(text.result.is_ok(), "{text:?}");
    // A check rebuilds the document, on the session's thread too.
    let check = sessions.check(&s1, ModelChoice::Current).wait();
    assert!(check.result.is_ok(), "{check:?}");
    // A document made, as `create` makes it, on its session's thread too.
    let made = sessions.open(dir.path().join("new.silico"), true).wait();
    assert!(made.result.unwrap().created);
    let caller = std::thread::current().name().map(str::to_owned);
    assert_ne!(caller.as_deref(), Some("session-s1"));
    assert_eq!(
        *seen.lock().unwrap(),
        [
            ("read", "session-s1".to_owned()),
            ("gate", "session-s1".to_owned()),
            ("rebuild", "session-s1".to_owned()),
            // The recompute it started runs under it, on its own thread, so its steps trace
            // back to the session's rebuild.
            ("rebuild", "recompute-1".to_owned()),
            ("create", "session-s2".to_owned()),
            ("gate", "session-s2".to_owned())
        ]
    );
    // Each command under the call that asked, the text's not under the open's: the open's
    // span ended with it.
    assert_eq!(
        *commands.lock().unwrap(),
        [vec!["client_open"], vec![], vec![], vec![]]
    );
}
