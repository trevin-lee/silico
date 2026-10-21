//! Break-it: the worker log's hand-off from the thread that reads it to the thread that
//! emits it (`worker.rs`: `forward_logs`, `LogQueue`, the log and log-sink threads), driven
//! by workers that are shell scripts, so every byte of their log is the test's to choose,
//! and by log sinks that stall, panic, or are merely slower than the reader.
//!
//! Each test installs the process's global subscriber, since the sink thread is the core's
//! own: run under nextest (a process per test), as `pixi run test` does.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::worker::Worker;
use silico_protocol::{Blob, Generation, Op};
use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

const WARNING: &str = "of the worker's log lines were dropped: the log sink fell behind";

/// Writes `body` as a `/bin/sh` script and gives its path: a worker that writes what the
/// test says to its log (fd 2), and never answers on its socket (fd 3).
fn script(name: &str, body: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "adversarial-worker-logs-{}-{name}.sh",
        std::process::id()
    ));
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// A script writing `count` lines at once, `line 0` to `line {count-1}` (from a file, so
/// faster than the reader takes them), then `then`.
fn burst_then(name: &str, count: usize, then: &str) -> PathBuf {
    let body: String = (0..count).map(|i| format!("line {i}\n")).collect();
    let burst = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "adversarial-worker-logs-{}-{name}.txt",
        std::process::id()
    ));
    std::fs::write(&burst, body).unwrap();
    script(name, &format!("/bin/cat '{}' >&2\n{then}", burst.display()))
}

/// More lines than the queue to the sink holds (4 MiB, each line's message and 64 bytes):
/// about 57,000 of these.
const PAST_THE_QUEUE: usize = 100_000;

/// What the sink was given of the worker's: a line's message, or a count dropped.
#[derive(Debug, Clone, PartialEq)]
enum Seen {
    Line(String),
    Dropped(u64),
}

#[derive(Default)]
struct Fields {
    message: String,
    dropped: Option<u64>,
}

impl Visit for Fields {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "dropped" {
            self.dropped = Some(value);
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        }
    }
}

/// How the sink behaves on the worker's first line.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Takes every line at once.
    Fast,
    /// Blocks on the first line until released.
    Stall,
    /// Panics on the first line (a layer with a bug).
    Panic,
    /// Takes each line in about `micros` microseconds (a slow terminal, say), never stalling.
    Slow { micros: u64 },
}

#[derive(Clone)]
struct Sink {
    mode: Mode,
    seen: Arc<Mutex<Vec<Seen>>>,
    entered: Arc<AtomicBool>,
    release: Arc<(Mutex<bool>, Condvar)>,
}

impl Sink {
    fn install(mode: Mode) -> Self {
        let sink = Self {
            mode,
            seen: Arc::default(),
            entered: Arc::default(),
            release: Arc::default(),
        };
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(sink.clone()))
            .expect("one subscriber per process: run under nextest");
        sink
    }

    fn wait_entered(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !self.entered.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "the sink was never given a line");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn release(&self) {
        let (released, opened) = &*self.release;
        *released.lock().unwrap() = true;
        opened.notify_all();
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// The lines given and the counts dropped, summed: every line the worker wrote, if
    /// none is lost.
    fn accounted(&self) -> u64 {
        self.seen()
            .iter()
            .map(|s| match s {
                Seen::Line(_) => 1,
                Seen::Dropped(n) => *n,
            })
            .sum()
    }

    /// Waits until `accounted` reaches `want`, or `limit` passes.
    fn wait_accounted(&self, want: u64, limit: Duration) -> u64 {
        let deadline = Instant::now() + limit;
        loop {
            let got = self.accounted();
            if got >= want || Instant::now() >= deadline {
                return got;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl<S: Subscriber> Layer<S> for Sink {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() != "silico::worker" {
            return;
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        let seen = match fields.dropped {
            Some(n) if fields.message.contains(WARNING) => Seen::Dropped(n),
            _ => Seen::Line(fields.message),
        };
        self.seen.lock().unwrap().push(seen);
        let first = !self.entered.swap(true, Ordering::SeqCst);
        match self.mode {
            Mode::Fast => {}
            Mode::Stall if first => {
                let (released, opened) = &*self.release;
                let mut released = released.lock().unwrap();
                while !*released {
                    released = opened.wait(released).unwrap();
                }
            }
            Mode::Stall => {}
            Mode::Panic if first => panic!("a log layer with a bug"),
            Mode::Panic => {}
            Mode::Slow { micros } => {
                let until = Instant::now() + Duration::from_micros(micros);
                while Instant::now() < until {
                    std::hint::spin_loop();
                }
            }
        }
    }
}

fn echo() -> Op {
    Op::Echo {
        data: Blob::inline(vec![1, 2, 3]),
    }
}

/// The numbers of the `line N` lines the sink was given, in the order given.
fn line_numbers(seen: &[Seen]) -> Vec<u64> {
    seen.iter()
        .filter_map(|s| match s {
            Seen::Line(text) => text.strip_prefix("line ").and_then(|n| n.parse().ok()),
            Seen::Dropped(_) => None,
        })
        .collect()
}

#[test]
fn a_stalled_sink_has_every_line_given_or_counted_and_in_order() {
    // More lines than the queue holds, dozens of pipes' worth, while the sink is stalled on
    // the first: the worker must still be read to its end (its crash reported, with its
    // last words), and once released the sink must have each line, or its count, exactly
    // once.
    let sink = Sink::install(Mode::Stall);
    let total = PAST_THE_QUEUE as u64 + 1;
    let exe = burst_then(
        "stalled-all",
        PAST_THE_QUEUE,
        "printf 'LAST WORDS\\n' >&2\nexit 0",
    );
    let mut worker = Worker::spawn(&exe).unwrap();
    sink.wait_entered();
    let asked = Instant::now();
    let err = worker
        .call(Generation(1), echo(), Duration::from_secs(20))
        .unwrap_err();
    assert!(
        asked.elapsed() < Duration::from_secs(8),
        "the crash took {:?} to report",
        asked.elapsed()
    );
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    let cause = err.cause().unwrap_or_default();
    assert!(
        cause.contains(&format!("line {}", PAST_THE_QUEUE - 1)) && cause.ends_with("LAST WORDS"),
        "{cause}"
    );
    sink.release();
    let accounted = sink.wait_accounted(total, Duration::from_secs(30));
    drop(worker);
    std::thread::sleep(Duration::from_millis(200));
    let seen = sink.seen();
    assert_eq!(sink.accounted(), total, "accounted {accounted} of {total}");
    // The queue held what it holds, no more: the stalled first, then at most 4 MiB of
    // lines, each its message and 64 bytes (`line N` is at least 6).
    let given = seen.iter().filter(|s| matches!(s, Seen::Line(_))).count();
    assert!(given > 1 && given < PAST_THE_QUEUE, "{given} lines given");
    assert!(
        given <= 1 + 4 * 1024 * 1024 / (6 + 64),
        "{given} lines given"
    );
    // In the order written, none twice.
    let numbers = line_numbers(&seen);
    assert!(numbers.windows(2).all(|w| w[0] < w[1]), "{numbers:?}");
    // The last line read (the crash's last words) is given or counted after every other.
    assert!(
        matches!(seen.last(), Some(Seen::Dropped(_)))
            || seen.last() == Some(&Seen::Line("LAST WORDS".to_owned())),
        "{:?}",
        seen.last()
    );
    // No count of zero, and never two counts in a row (each is said where its first line
    // was dropped, and the next only after a line the queue had room for).
    assert!(!seen.contains(&Seen::Dropped(0)));
    assert!(
        !seen
            .windows(2)
            .any(|w| matches!(w, [Seen::Dropped(_), Seen::Dropped(_)])),
        "{seen:?}"
    );
}

#[test]
fn lines_dropped_while_the_sink_stalled_are_counted_once_it_recovers_while_the_worker_is_quiet() {
    // A pool's worker lives as long as the app, and is quiet between requests. Lines it
    // wrote while the sink stalled are dropped; once the sink recovers and has taken what
    // was queued, the count should be said, not held until the worker someday ends.
    let sink = Sink::install(Mode::Stall);
    // Writes more lines than the queue holds and then waits on its socket, which the core
    // never writes to.
    let total = PAST_THE_QUEUE as u64;
    let exe = burst_then("quiet-after", PAST_THE_QUEUE, "read -r _ <&3\nexit 0");
    let worker = Worker::spawn(&exe).unwrap();
    sink.wait_entered();
    // Long enough for the shell to write its lines and the reader to read them all.
    std::thread::sleep(Duration::from_secs(1));
    sink.release();
    // Every line is given or counted while the worker lives: the count is said once the
    // sink gets to where lines were first dropped, not held for the worker's next line or
    // its end (a pool's worker is quiet between requests, and lives as long as the app).
    let while_alive = sink.wait_accounted(total, Duration::from_secs(20));
    let counted = sink.seen().iter().any(|s| matches!(s, Seen::Dropped(_)));
    assert!(
        counted && while_alive == total,
        "the sink recovered and accounted for {while_alive} of {total} lines while the \
         worker lived: {:?}",
        sink.seen().last()
    );
    drop(worker);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(sink.accounted(), total, "{:?}", sink.seen().last());
}

#[test]
fn a_crash_report_waits_for_a_log_held_open_by_the_workers_child() {
    // The control for the next test: a worker that started a process holding its log open
    // has that process's last words in its crash report, as `crashed` intends ("a process
    // it started may hold them open").
    let sink = Sink::install(Mode::Fast);
    let exe = script(
        "child-holds-log",
        "printf 'first\\n' >&2\n( exec 3>&-; /bin/sleep 1; printf 'LAST WORDS\\n' >&2 ) &\nexit 0",
    );
    let mut worker = Worker::spawn(&exe).unwrap();
    sink.wait_entered();
    let err = worker
        .call(Generation(1), echo(), Duration::from_secs(20))
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    let cause = err.cause().unwrap_or_default();
    assert!(cause.ends_with("first\nLAST WORDS"), "{cause}");
}

#[test]
fn a_stalled_sink_still_has_the_crash_report_wait_for_a_log_held_open() {
    let sink = Sink::install(Mode::Stall);
    let exe = script(
        "child-holds-log-stalled",
        "printf 'first\\n' >&2\n( exec 3>&-; /bin/sleep 1; printf 'LAST WORDS\\n' >&2 ) &\nexit 0",
    );
    let mut worker = Worker::spawn(&exe).unwrap();
    sink.wait_entered();
    let asked = Instant::now();
    let err = worker
        .call(Generation(1), echo(), Duration::from_secs(20))
        .unwrap_err();
    assert!(
        asked.elapsed() < Duration::from_secs(8),
        "{:?}",
        asked.elapsed()
    );
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    let cause = err.cause().unwrap_or_default();
    assert!(cause.ends_with("first\nLAST WORDS"), "{cause}");
    sink.release();
}

#[test]
fn a_sink_that_panics_leaves_the_crash_report_whole() {
    // A log layer with a bug panics on the worker's first line, on the sink thread. The
    // reader must go on, and a crash report must still wait for the log as it does above:
    // found by break-it, the sink thread then held the only "log done" signal, so its panic
    // said the log was all read when it wasn't.
    let sink = Sink::install(Mode::Panic);
    let exe = script(
        "child-holds-log-panic",
        "printf 'first\\n' >&2\n( exec 3>&-; /bin/sleep 1; printf 'LAST WORDS\\n' >&2 ) &\nexit 0",
    );
    let mut worker = Worker::spawn(&exe).unwrap();
    sink.wait_entered();
    // Let the sink thread's panic unwind.
    std::thread::sleep(Duration::from_millis(100));
    let err = worker
        .call(Generation(1), echo(), Duration::from_secs(20))
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    let cause = err.cause().unwrap_or_default();
    assert!(cause.ends_with("first\nLAST WORDS"), "{cause}");
}

#[test]
fn a_sink_slower_than_the_reader_but_never_stalled_loses_no_lines() {
    // A sink that takes every line, just more slowly than the reader reads them (20 us a
    // line: a terminal, a file on a busy disk, a JSON formatter): a burst of 5,000 lines,
    // as a worker at SILICO_LOG=trace writes in an OCCT operation, is 100 ms of sink work.
    // Nothing is stalled, and before the hand-off the worker waited on the pipe: no line
    // was lost.
    let sink = Sink::install(Mode::Slow { micros: 20 });
    let exe = burst_then("burst", 5_000, "read -r _ <&3\nexit 0");
    let worker = Worker::spawn(&exe).unwrap();
    // Every line that will be given is given within 5,000 x 20 us, with room to spare.
    sink.wait_accounted(5_000, Duration::from_secs(3));
    let given = line_numbers(&sink.seen()).len();
    // Once the worker ends, every line is given or counted.
    drop(worker);
    let accounted = sink.wait_accounted(5_000, Duration::from_secs(20));
    assert_eq!(accounted, 5_000, "the sink saw {accounted}");
    // Found by break-it: a queue of 1,024 lines filled in such a burst, and most of it was
    // dropped, though the sink never stalled.
    assert_eq!(given, 5_000, "{} of 5000 lines dropped", 5_000 - given);
}

#[test]
fn a_text_log_written_to_a_file_has_every_line_of_a_burst() {
    // The sink as the CLI sets it up (tracing_subscriber's text format), writing to a file
    // (`silico check model.silico 2>log.txt` with SILICO_LOG=debug): nothing stalls, but
    // formatting each line costs more than reading it. Every line of a 20,000-line burst
    // should be in the file; failing that, the file should say how many are missing.
    let log = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "adversarial-worker-logs-{}-fmt.log",
        std::process::id()
    ));
    let file = std::fs::File::create(&log).unwrap();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_writer(Mutex::new(file))
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .finish(),
    )
    .unwrap();
    let total = 20_000;
    let exe = burst_then("fmt-burst", total as usize, "exit 0");
    let mut worker = Worker::spawn(&exe).unwrap();
    // The worker ends once its burst is read; its crash report waits for the log.
    let err = worker
        .call(Generation(1), echo(), Duration::from_secs(20))
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    drop(worker);
    std::thread::sleep(Duration::from_secs(2));
    let text = std::fs::read_to_string(&log).unwrap();
    let given = text
        .lines()
        .filter(|l| l.contains("silico::worker") && l.contains(": line "))
        .count() as u64;
    let dropped: u64 = text
        .lines()
        .filter(|l| l.contains(WARNING))
        .filter_map(|l| {
            l.split("dropped=")
                .nth(1)?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })
        .sum();
    assert_eq!(given + dropped, total, "{given} given, {dropped} counted");
    // Found by break-it: a queue of 1,024 lines dropped most of the burst, though the sink
    // never stalled.
    assert_eq!(given, total, "{} of {total} lines dropped", total - given);
}

/// Blocks closing a span named `asked` when it closes on a worker's log sink thread (a
/// span exporter whose collector stalls: tracing-opentelemetry exports a span as it closes,
/// and a synchronous processor waits on the collector), for a minute, so the test ends;
/// and notes one closed on the log thread, which mustn't be.
#[derive(Clone, Default)]
struct StalledExporter {
    stall: bool,
    blocked: Arc<AtomicBool>,
    on_reader: Arc<AtomicBool>,
}

impl<S> Layer<S> for StalledExporter
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_close(&self, id: tracing::span::Id, ctx: Context<'_, S>) {
        let asked = ctx.span(&id).is_some_and(|span| span.name() == "asked");
        let thread = std::thread::current();
        let worker_thread = |end: &str| {
            thread
                .name()
                .is_some_and(|name| name.starts_with("worker-") && name.ends_with(end))
        };
        if asked && worker_thread("-logs") {
            self.on_reader.store(true, Ordering::SeqCst);
        }
        if asked && worker_thread("-log-sink") {
            self.blocked.store(true, Ordering::SeqCst);
            if self.stall {
                std::thread::sleep(Duration::from_secs(60));
            }
        }
    }
}

/// `printf` octal escapes for `bytes`.
fn octal(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:03o}")).collect()
}

#[test]
fn a_requests_span_let_go_by_the_log_thread_closes_on_the_sink_thread_and_the_next_request_is_answered() {
    // The control for the next test: the same worker, a sink that doesn't stall.
    span_close_case(false);
}

#[test]
fn a_sink_stalled_closing_a_requests_span_never_hangs_the_next_request() {
    span_close_case(true);
}

fn span_close_case(stall: bool) {
    // The worker answers the first request (trace 1: the first of this process) at once,
    // and says it's done half a second later, so the caller has let its span go by then
    // and the log thread's handle is the last: the span closes when the log thread lets it
    // go, on the sink thread. Then it answers the second request.
    let exporter = StalledExporter {
        stall,
        ..StalledExporter::default()
    };
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(exporter.clone()))
        .unwrap();
    let reply = |id: u64| {
        let mut frame = Vec::new();
        silico_protocol::write_message(
            &mut frame,
            &silico_protocol::Response {
                id: silico_protocol::RequestId(id),
                generation: Generation(1),
                result: Ok(silico_protocol::Reply::Done),
            },
        )
        .unwrap();
        octal(&frame)
    };
    let exe = script(
        "span-close",
        &format!(
            "/bin/dd bs=1 count=1 <&3 >/dev/null 2>&1\nprintf '{}' >&3\n/bin/sleep 0.5\n\
             printf '{{\"silico_request_done\":1}}\\n' >&2\n/bin/sleep 1\nprintf '{}' >&3\n\
             read -r _ <&3\nexit 0",
            reply(0),
            reply(1),
        ),
    );
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut worker = Worker::spawn(&exe).unwrap();
        for i in 0..2 {
            let span = tracing::info_span!("asked", i);
            let answered = span.in_scope(|| {
                worker.call(
                    Generation(1),
                    Op::Debug(silico_protocol::DebugOp::Sleep { millis: 0 }),
                    Duration::from_secs(5),
                )
            });
            drop(span);
            done.send((i, answered.map_err(|e| e.to_string()))).unwrap();
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    for i in 0..2 {
        // Each call has 5 s; 20 s is a hang no deadline ends.
        let got = finished.recv_timeout(Duration::from_secs(20));
        // Found by break-it: the log thread let a request's span go (`Callers::done`)
        // holding the worker's callers lock, so the sink's `on_close` ran there with the
        // lock held; stalled, it stalled the log thread (the worker's pipe went unread), and
        // the next call blocked taking the lock before its timeout started, and never
        // returned.
        let (asked, answered) = got.unwrap_or_else(|_| {
            panic!(
                "call {i} never returned (the sink stalled closing a span: {})",
                exporter.blocked.load(Ordering::SeqCst)
            )
        });
        assert_eq!(asked, i);
        assert_eq!(answered, Ok(silico_protocol::Reply::Done), "call {i}");
    }
    // The first request's span did close as the log thread let it go, so the case was the
    // one meant, and never on the log thread itself.
    assert!(exporter.blocked.load(Ordering::SeqCst));
    assert!(!exporter.on_reader.load(Ordering::SeqCst));
}
