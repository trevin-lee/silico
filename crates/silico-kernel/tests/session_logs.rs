//! M4.7: a failed fillet's logs, read by its feature's ID (#50's M4.7 row), on kernel
//! workers: the kernel request that failed, the worker's own line for it, re-emitted under
//! the call that asked, and the rebuild step's failure, each under the check that rebuilt
//! it. Alone in its process (nextest runs each test in its own), it sets the process's
//! tracing subscriber, as `silico_core::logging::init` does, with the core's log layer; its
//! workers log at their default level, info.

use std::path::Path;
use std::time::{Duration, Instant};

use silico_core::cache::Cache;
use silico_core::command::logs::{LogEvent, LogLevel, LogQuery};
use silico_core::command::{CheckModel, ModelChoice, Running, SessionId, Sessions};
use silico_core::document::FeatureId;
use silico_core::pool::KernelPool;
use tracing_subscriber::layer::SubscriberExt;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

/// A 10 mm cube and a fillet of one of its edges at 20 mm, which OCCT can't build.
pub const TOO_ROUND: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}, {"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge1"], "radius": 20}]}"#;

fn answer<T>(running: Running<T>) -> silico_core::command::Response<T> {
    running
        .wait_timeout(Duration::from_secs(120))
        .unwrap_or_else(|_| panic!("the command didn't answer"))
}

/// Sets the process's subscriber to the core's log layer alone.
fn keep_logs() {
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(silico_core::command::logs::layer()),
    )
    .unwrap();
}

/// Sessions on a pool of kernel workers, their cache sealed with a secret in `dir`, and
/// `text` written there and opened: the session.
fn opened(dir: &tempfile::TempDir, text: &str) -> (Sessions<KernelPool>, SessionId) {
    let secret = dir.path().join("secret");
    let path = dir.path().join("part.silico");
    std::fs::write(&path, text).unwrap();
    let pool = KernelPool::start_on_demand(Path::new(WORKER), 2).unwrap();
    let sessions =
        Sessions::with_cache_opener(pool, move |cache| Cache::with_secret(cache, &secret));
    let s1 = answer(sessions.open(&path, false)).session.unwrap().id;
    (sessions, s1)
}

/// A read of `level` and above, of `feature` if given (which must be a feature's ID), after
/// `after`, up to 1,000 events.
fn query(level: LogLevel, feature: Option<&str>, after: Option<u64>) -> LogQuery {
    LogQuery {
        level,
        feature: feature.map(|id| FeatureId::new(id).unwrap()),
        after,
        limit: 1000,
    }
}

#[test]
fn a_failed_fillets_logs_hold_the_request_that_failed_and_the_workers_own_line() {
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let (sessions, s1) = opened(&dir, TOO_ROUND);
    let checked = answer(sessions.check(&s1, CheckModel::Current { fresh: false }));
    let problems = checked.result.unwrap().problems;
    assert_eq!(problems.errors()[0].feature(), Some("f0000001"));
    let query = query(LogLevel::Info, Some("f0000001"), None);
    // The worker's line comes through the core's log thread, which may emit it after the
    // check answers: read until it's there.
    // Its kind and OCCT's words, no element: the worker knows them only by their places.
    let worker_line = "Fillet refused, kernel_failed: Standard_Failure: the fillet couldn't be \
                       built";
    let deadline = Instant::now() + Duration::from_secs(30);
    let events: Vec<LogEvent> = loop {
        let events = sessions.read_logs(&s1, &query).result.unwrap().events;
        if events.iter().any(|e| e.message.starts_with(worker_line)) || Instant::now() > deadline {
            break events;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let by = |message: &str| {
        events
            .iter()
            .find(|e| e.message.starts_with(message))
            .unwrap_or_else(|| panic!("no {message:?} in {events:#?}"))
    };
    // Each the fillet's, under the check, its rebuild, and the step that made the fillet.
    let under_the_step = |event: &LogEvent| {
        assert_eq!(event.feature, FeatureId::new("f0000001"), "{event:?}");
        assert_eq!(event.level, LogLevel::Info, "{event:?}");
        assert_eq!(event.spans[0], "check", "{event:?}");
        assert_eq!(
            event.spans[1], "rebuild revision=1 fresh=false",
            "{event:?}"
        );
        assert!(
            event
                .spans
                .iter()
                .any(|s| s.starts_with("recompute_step feature=f0000001 ")
                    && s.ends_with(" job=make")),
            "{event:?}"
        );
    };
    // The worker's own words, under the span its request was asked in, then the worker's
    // process and the request's trace, as the core re-emits them.
    let worker = by(worker_line);
    under_the_step(worker);
    let fields = worker.message[worker_line.len()..].to_owned();
    assert!(
        fields.starts_with(" pid=") && fields.contains(" trace="),
        "{fields}"
    );
    assert!(
        worker
            .spans
            .last()
            .unwrap()
            .starts_with("worker_lines trace="),
        "{worker:?}"
    );
    // The request that failed: the worker's call of Fillet, which the core says refused.
    // Its element by its name, and the fillet's field it's in.
    let request = by(
        "the kernel worker refused Fillet: Fillet failed code=kernel_failed parameter=edges.0 \
         elements=[\"k7f2qx3m.box.Edge1\"] elements_total=1 cause=Standard_Failure: the fillet \
         couldn't be built",
    );
    under_the_step(request);
    let call = request.spans.last().unwrap();
    assert!(
        call.starts_with("worker_call trace=") && call.ends_with(" op=Fillet"),
        "{request:?}"
    );
    // And the step's failure, as the check reports it.
    let step = by(
        "the step failed: Fillet failed code=kernel_failed parameter=edges.0 \
         elements=[\"k7f2qx3m.box.Edge1\"] elements_total=1 cause=Standard_Failure: the fillet \
         couldn't be built",
    );
    under_the_step(step);
    assert!(
        step.spans.last().unwrap().starts_with("recompute_step "),
        "{step:?}"
    );
    // The box's events are its own: none is the fillet's.
    let block = sessions
        .read_logs(
            &s1,
            &LogQuery {
                feature: FeatureId::new("k7f2qx3m"),
                ..query
            },
        )
        .result
        .unwrap();
    assert!(
        block.events.iter().all(|e| !e.message.contains("Fillet")),
        "{:?}",
        block.events
    );
}

/// The rim of the example part's hole, by its name: an edge of a cut, whose place among the
/// cut's edges isn't its name.
const RIM: &str = "m3tq8v0z.cut.Edge(G k7f2qx3m.box.Face6, G a91cw0de.cylinder.Face1)";

#[test]
fn a_failed_fillet_of_a_cuts_edge_is_logged_by_the_edges_name_never_its_place() {
    // Architecture round 1: the worker knows an edge by its place in the cut's edges; the
    // logs give its name, as the check's error does.
    keep_logs();
    let hole = r#""note": "Clearance for an M5 bolt"}"#;
    assert!(super::example::EXAMPLE.contains(hole));
    let text = super::example::EXAMPLE.replace(
        hole,
        &format!(
            r#"{hole},
    {{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "m3tq8v0z", "edges": ["{RIM}"], "radius": 20}}"#
        ),
    );
    let dir = tempfile::tempdir().unwrap();
    let (sessions, s1) = opened(&dir, &text);
    let checked = answer(sessions.check(&s1, CheckModel::Current { fresh: false }));
    let problems = checked.result.unwrap().problems;
    let error = &problems.errors()[0];
    assert_eq!(error.feature(), Some("f0000001"));
    assert_eq!(error.elements(), [RIM]);
    let query = query(LogLevel::Info, Some("f0000001"), None);
    let deadline = Instant::now() + Duration::from_secs(30);
    let events: Vec<LogEvent> = loop {
        let events = sessions.read_logs(&s1, &query).result.unwrap().events;
        if events
            .iter()
            .any(|e| e.message.starts_with("Fillet refused, "))
            || Instant::now() > deadline
        {
            break events;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    // The core's lines, the request's and the step's, each name the rim, once in all.
    let named = format!(" elements=[\"{RIM}\"] elements_total=1 ");
    for start in ["the kernel worker refused Fillet: ", "the step failed: "] {
        let event = events
            .iter()
            .find(|e| e.message.starts_with(start))
            .unwrap_or_else(|| panic!("no {start:?} in {events:#?}"));
        assert!(event.message.contains(&named), "{event:?}");
        assert!(event.message.contains(" parameter=edges.0 "), "{event:?}");
    }
    // No line names an edge by a place: every `Edge` is the rim's name's.
    for event in &events {
        let without = event.message.replace(RIM, "");
        assert!(!without.contains("Edge"), "{event:?}");
    }
    let worker = events
        .iter()
        .find(|e| e.message.starts_with("Fillet refused, "))
        .unwrap_or_else(|| panic!("no worker line in {events:#?}"));
    assert!(
        worker.spans.last().unwrap().starts_with("worker_lines "),
        "{worker:?}"
    );
}

#[test]
fn a_read_of_the_published_model_is_kept_under_its_command_and_the_model_it_reads() {
    // Tests round 1: a published read runs on a thread of its own, beside the session's
    // queue; its events are the session's, labelled with the model it read.
    keep_logs();
    let dir = tempfile::tempdir().unwrap();
    let (sessions, s1) = opened(
        &dir,
        &TOO_ROUND.replace(r#""radius": 20"#, r#""radius": 1"#),
    );
    let checked = answer(sessions.check(&s1, CheckModel::Current { fresh: false }));
    assert!(checked.result.unwrap().problems.errors().is_empty());
    let before = sessions
        .read_logs(&s1, &query(LogLevel::Trace, None, None))
        .result
        .unwrap()
        .next;
    let faces = [
        "k7f2qx3m.box.Face1".to_owned(),
        "k7f2qx3m.box.Face2".to_owned(),
    ];
    let distance = answer(sessions.distance(&s1, ModelChoice::Published, faces, None));
    assert!(distance.result.is_ok(), "{distance:?}");
    let after = sessions
        .read_logs(&s1, &query(LogLevel::Trace, None, Some(before)))
        .result
        .unwrap();
    // The distance's own events (its kernel request's among them), each under it.
    assert!(!after.events.is_empty());
    assert!(
        after
            .events
            .iter()
            .all(|e| e.spans[0] == "distance model=published"),
        "{:#?}",
        after.events
    );
}
