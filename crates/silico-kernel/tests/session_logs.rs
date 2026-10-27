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
use silico_core::command::{CheckModel, Running, Sessions};
use silico_core::document::FeatureId;
use silico_core::pool::KernelPool;
use tracing_subscriber::layer::SubscriberExt;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

/// A 10 mm cube and a fillet of one of its edges at 20 mm, which OCCT can't build.
const TOO_ROUND: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}, {"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge1"], "radius": 20}]}"#;

fn answer<T>(running: Running<T>) -> silico_core::command::Response<T> {
    running
        .wait_timeout(Duration::from_secs(120))
        .unwrap_or_else(|_| panic!("the command didn't answer"))
}

#[test]
fn a_failed_fillets_logs_hold_the_request_that_failed_and_the_workers_own_line() {
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(silico_core::command::logs::layer()),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let path = dir.path().join("part.silico");
    std::fs::write(&path, TOO_ROUND).unwrap();
    let pool = KernelPool::start_on_demand(Path::new(WORKER), 2).unwrap();
    let sessions =
        Sessions::with_cache_opener(pool, move |cache| Cache::with_secret(cache, &secret));
    let s1 = answer(sessions.open(&path, false)).session.unwrap().id;
    let checked = answer(sessions.check(&s1, CheckModel::Current { fresh: false }));
    let problems = checked.result.unwrap().problems;
    assert_eq!(problems.errors()[0].feature(), Some("f0000001"));
    let query = LogQuery {
        level: LogLevel::Info,
        feature: FeatureId::new("f0000001"),
        after: None,
        limit: 1000,
    };
    // The worker's line comes through the core's log thread, which may emit it after the
    // check answers: read until it's there.
    let worker_line = "Fillet refused: OCCT couldn't fillet Edge1 (Standard_Failure: the \
                       fillet couldn't be built)";
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
    let request = by(
        "the kernel worker refused Fillet: Fillet failed code=kernel_failed \
         cause=Standard_Failure: the fillet couldn't be built",
    );
    under_the_step(request);
    let call = request.spans.last().unwrap();
    assert!(
        call.starts_with("worker_call trace=") && call.ends_with(" op=Fillet"),
        "{request:?}"
    );
    // And the step's failure, as the check reports it.
    let step = by(
        "the step failed: Fillet failed code=kernel_failed cause=Standard_Failure: the fillet \
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
