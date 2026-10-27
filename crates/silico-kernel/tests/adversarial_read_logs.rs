//! Break-it for M4.7's worker lines on a real kernel worker: a worker's own refusal line,
//! logged at info since M4.7 so a session's logs hold it, must come back to the core as a
//! line it reads (its level, its message, and the request it's of), whatever the refusal
//! lists. Alone in its process (nextest runs each test in its own), it sets the process's
//! subscriber to a layer that records every event the core emits.

use std::io::Write as _;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use silico_core::kernel::KernelWorker;
use silico_protocol::{BooleanOp, Primitive, ShapeData};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

/// One event the core emitted: its level, target, message, and the names of the spans it
/// is in, from the innermost.
#[derive(Debug, Clone)]
struct Seen {
    level: tracing::Level,
    target: String,
    message: String,
    spans: Vec<String>,
}

static SEEN: Mutex<Vec<Seen>> = Mutex::new(Vec::new());

struct Recording;

#[derive(Default)]
struct Message(String);

impl Visit for Message {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0.push_str(&format!("{value:?}"));
        }
    }
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Recording {
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut message = Message::default();
        event.record(&mut message);
        let spans = ctx
            .event_scope(event)
            .map(|scope| scope.map(|span| span.name().to_owned()).collect())
            .unwrap_or_default();
        SEEN.lock().unwrap().push(Seen {
            level: *event.metadata().level(),
            target: event.metadata().target().to_owned(),
            message: message.0,
            spans,
        });
    }
}

fn shape(built: silico_protocol::BuiltData) -> ShapeData {
    built.shape
}

/// A 130 × 130 × 10 plate with a 32 × 32 grid of through holes, 1 mm in radius, 4 mm apart:
/// 1,024 cylindrical holes, each with a seam, which OCCT can't fillet (it's smooth there).
fn plate_with_holes(w: &mut KernelWorker) -> ShapeData {
    let cylinder = w
        .make(Primitive::Cylinder {
            radius: 1.0,
            height: 20.0,
        })
        .unwrap();
    let mut grid = shape(w.translate(&cylinder, [3.0, 3.0, -5.0]).unwrap());
    // A row of 32 by doubling, then 32 rows.
    for axis in [0, 1] {
        for k in 0..5 {
            let mut offset = [0.0; 3];
            offset[axis] = 4.0 * f64::from(1u32 << k);
            let moved = shape(w.translate(&grid, offset).unwrap());
            grid = shape(w.boolean(BooleanOp::Fuse, &grid, &moved).unwrap());
        }
    }
    let plate = w
        .make(Primitive::Box {
            dx: 130.0,
            dy: 130.0,
            dz: 10.0,
        })
        .unwrap();
    shape(w.boolean(BooleanOp::Cut, &plate, &grid).unwrap())
}

// Found by break-it: the worker logs its refusal at info (silico-worker's serve, M4.7) as
// one JSON line naming every refused edge; past the core's 8 KiB cut of a worker's line
// (silico_protocol::MAX_LOG_LINE) the line was no JSON, so the core re-emitted the fragment
// as a warning of the worker's, under no span: on the app's stderr at the default level, and
// not in the session's logs. The worker now cuts the message to fit, as JSON escapes it.
#[test]
fn a_fillet_refused_on_a_thousand_seams_comes_back_as_the_workers_info_line_under_its_call() {
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(Recording))
        .unwrap();
    let mut w = KernelWorker::start_worker(Path::new(WORKER)).unwrap();
    let plate = plate_with_holes(&mut w);
    let summary = w.summarize(&plate).unwrap();
    // 12 of the plate's own, and each hole's two rims and seam.
    assert_eq!(summary.edges, 12 + 3 * 1024, "{summary:?}");
    let edges: Vec<u32> = (1..=summary.edges).collect();
    let refused = {
        let _asking = tracing::info_span!("asking").entered();
        w.fillet(&plate, &edges, 0.1).unwrap_err()
    };
    // The worker's refusal: of the 1,024 seams.
    assert_eq!(refused.elements_total(), 1024, "{}", refused.message());
    // Its own line for it comes through the core's log thread, maybe after the call
    // answers: wait for it, or for a line of the worker's that isn't one.
    let deadline = Instant::now() + Duration::from_secs(30);
    let worker_lines = loop {
        let lines: Vec<Seen> = SEEN
            .lock()
            .unwrap()
            .iter()
            .filter(|seen| seen.target == "silico::worker")
            .cloned()
            .collect();
        let refusal_seen = lines.iter().any(|seen| {
            seen.message.starts_with("Fillet refused:") || seen.message.contains("Fillet refused")
        });
        if refusal_seen || Instant::now() > deadline {
            break lines;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    // No line the core couldn't read: the worker wrote nothing but its tracing lines.
    let unread: Vec<&Seen> = worker_lines
        .iter()
        .filter(|seen| seen.message.starts_with('{'))
        .collect();
    assert!(
        unread.is_empty(),
        "the core re-emitted {} worker line(s) it couldn't read, at {:?}, under {:?}: {}…",
        unread.len(),
        unread[0].level,
        unread[0].spans,
        &unread[0].message[..200.min(unread[0].message.len())]
    );
    // The worker's own words, at info, under the call that asked.
    let line = worker_lines
        .iter()
        .find(|seen| seen.message.starts_with("Fillet refused:"))
        .unwrap_or_else(|| panic!("no refusal line of the worker's in {worker_lines:#?}"));
    assert_eq!(line.level, tracing::Level::INFO);
    assert!(line.spans.iter().any(|s| s == "asking"), "{:?}", line.spans);
}

/// Set in the child that
/// [`an_explicit_silico_log_of_warn_still_lets_the_workers_refusal_into_the_sessions_logs`]
/// runs: the child reads the logs.
const LOG_CHILD: &str = "SILICO_BREAK_IT_LOG_CHILD";

/// A 10 mm cube and a fillet of one of its edges at 20 mm, which OCCT can't build.
const TOO_ROUND: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}, {"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": ["k7f2qx3m.box.Edge1"], "radius": 20}]}"#;

/// The child: a session's failed fillet, its logs read for the worker's own line, as
/// silico-kernel's session_logs test reads them with SILICO_LOG unset. Exits 0 if the line
/// is there, 3 if it isn't.
#[test]
fn read_logs_child_entry() {
    use silico_core::cache::Cache;
    use silico_core::command::logs::{LogLevel, LogQuery};
    use silico_core::command::{CheckModel, Sessions};
    use silico_core::document::FeatureId;
    use silico_core::pool::KernelPool;
    if std::env::var_os(LOG_CHILD).is_none() {
        return;
    }
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(silico_core::command::logs::layer()),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let path = dir.path().join("part.silico");
    std::fs::write(&path, TOO_ROUND).unwrap();
    let pool = KernelPool::start_on_demand(Path::new(WORKER), 1).unwrap();
    let sessions =
        Sessions::with_cache_opener(pool, move |cache| Cache::with_secret(cache, &secret));
    let wait = Duration::from_secs(120);
    let s1 = sessions
        .open(&path, false)
        .wait_timeout(wait)
        .unwrap_or_else(|_| panic!("open hung"))
        .session
        .unwrap()
        .id;
    let checked = sessions
        .check(&s1, CheckModel::Current { fresh: false })
        .wait_timeout(wait)
        .unwrap_or_else(|_| panic!("check hung"));
    assert!(checked.result.is_ok());
    let query = LogQuery {
        level: LogLevel::Trace,
        feature: FeatureId::new("f0000001"),
        after: None,
        limit: 1000,
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let events = sessions.read_logs(&s1, &query).result.unwrap().events;
        if events
            .iter()
            .any(|e| e.message.starts_with("Fillet refused:"))
        {
            std::process::exit(0);
        }
        if Instant::now() > deadline {
            let _ = writeln!(std::io::stderr(), "no worker line in {events:#?}");
            std::process::exit(3);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// Found by break-it: workers took SILICO_LOG as their own filter (silico-worker's run), so a
// SILICO_LOG setting the app's stderr log to warn, its default, set each worker to warn too,
// and the worker's info line for a refusal never left it, though `logging::init` and the
// design say a session keeps every level whatever SILICO_LOG lets through to stderr. A
// worker now raises a level for every target to info. Unset, the same check is the control.
#[test]
fn an_explicit_silico_log_of_warn_still_lets_the_workers_refusal_into_the_sessions_logs() {
    let me = std::env::current_exe().unwrap();
    let run = |log: Option<&str>| {
        let mut child = std::process::Command::new(&me);
        child
            .args([
                "--exact",
                "adversarial_read_logs::read_logs_child_entry",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(LOG_CHILD, "1")
            .env_remove("SILICO_LOG");
        if let Some(log) = log {
            child.env("SILICO_LOG", log);
        }
        let out = child.output().unwrap();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let (unset, why) = run(None);
    assert_eq!(unset, Some(0), "unset: {why}");
    let (warn, why) = run(Some("warn"));
    assert_eq!(
        warn,
        Some(0),
        "SILICO_LOG=warn: {}",
        &why[why.len().saturating_sub(600)..]
    );
}
