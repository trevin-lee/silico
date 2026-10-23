//! M2.4: a log sink that stalls, panics, or is merely slower than the workers never holds
//! a worker up. Each worker's log is read on its own thread and handed to the one thread
//! that emits every worker's (`silico_core::worker`: `forward_logs`, `LogQueue`), so the
//! line every request ends with never fills the worker's pipe until the worker blocks
//! writing one and stops answering, which would look like a hang in the geometry. Driven by
//! the kernel worker, and by workers that are shell scripts, so every byte of their log is
//! the test's to choose; the cases with "break-it" in them it found, against an earlier
//! version of the hand-off.
//!
//! Each test installs the process's global subscriber, since the sink thread is the core's
//! own: run under nextest (a process per test), as `pixi run test` does.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::scripts;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use scripts::script;
use silico_core::ErrorCode;
use silico_core::kernel::{KernelRequest, KernelWorker};
use silico_core::worker::Worker;
use silico_protocol::{Blob, BooleanOp, DebugOp, Generation, Op, Primitive};
use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

const WARNING: &str = "of the worker's log lines were dropped: the log sink fell behind";

/// A file of `count` lines, `line 0` to `line {count-1}`, for a worker to write at once,
/// faster than the reader takes them.
fn burst_file(name: &str, count: usize) -> PathBuf {
    let body: String = (0..count).map(|i| format!("line {i}\n")).collect();
    let burst = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("worker-logs-{name}-{}.txt", std::process::id()));
    std::fs::write(&burst, body).unwrap();
    burst
}

/// A script writing `count` lines at once (see [`burst_file`]), then `then`.
fn burst_then(name: &str, count: usize, then: &str) -> PathBuf {
    let burst = burst_file(name, count);
    script(
        &format!("worker-logs-{name}"),
        &format!("/bin/cat '{}' >&2\n{then}", burst.display()),
    )
}

/// More lines than the queue to the sink holds (4 MiB, each line its message, the item's
/// own size, and 64 bytes): about 24,000 of these.
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
    // lines, each its message and more than 64 bytes (`line N` is at least 6); a loose
    // bound, so it doesn't depend on the item's size.
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
    // was queued, the count should be said, not held until the worker someday ends. Found
    // by break-it, against an earlier version of the hand-off, which said the count with
    // the worker's next line.
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
        "worker-logs-child-holds-log",
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
        "worker-logs-child-holds-log-stalled",
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
    // reader must go on, a crash report must still wait for the log as it does above, and
    // the sink must go on emitting. Found by break-it, against an earlier version of the
    // hand-off, whose sink thread held the only "log done" signal, so its panic said the
    // log was all read when it wasn't.
    let sink = Sink::install(Mode::Panic);
    let exe = script(
        "worker-logs-child-holds-log-panic",
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
    // The sink thread is every worker's: it lost the line it panicked on, and goes on.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !sink.seen().contains(&Seen::Line("LAST WORDS".to_owned())) {
        assert!(
            Instant::now() < deadline,
            "the sink stopped: {:?}",
            sink.seen()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_log_sink_that_blocks_outright_never_holds_up_a_kernel_worker() {
    let sink = Sink::install(Mode::Stall);
    let mut worker =
        KernelWorker::start_worker(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker"))).unwrap();
    // A sphere fused with a cylinder of its radius: OCCT warns about the touching surfaces,
    // and the worker logs the warning, at which the sink stalls.
    let ball = worker.make(Primitive::Sphere { radius: 5.0 }).unwrap();
    let post = worker
        .make(Primitive::Cylinder {
            radius: 5.0,
            height: 5.0,
        })
        .unwrap();
    worker.boolean(BooleanOp::Fuse, &ball, &post).unwrap();
    sink.wait_entered();
    // Twice as many requests as a pipe holds the lines that end them (about 2,300 in a
    // 64 KiB pipe: the core's unit test of the pipe measures it), each answered in time;
    // before the hand-off, the 2,296th timed out.
    let requests: u32 = 5_000;
    for i in 0..requests {
        let data = Blob::inline(i.to_le_bytes().to_vec());
        let echoed = worker
            .run(KernelRequest::echo(data.clone()).with_timeout(Duration::from_secs(10)))
            .unwrap_or_else(|e| panic!("request {i} of {requests}: {e}"));
        assert_eq!(echoed, data, "request {i}");
    }
    // The sink took the warning and nothing since: no request logged.
    assert_eq!(sink.seen().len(), 1, "{:?}", sink.seen());
    // A worker that ends while the sink is stalled is reported at once, with its last lines
    // (kept as they're read): the report waits a moment for the sink, no more, so a stalled
    // sink doesn't hold a pool's slot up (it held it the whole 2 s exit grace). A script
    // that exits, not one that aborts, so no core dump is in the time.
    let lines: String = (1..=25)
        .map(|i| format!("printf 'last {i}\\n' >&2\n"))
        .collect();
    let mut doomed =
        Worker::spawn(&script("worker-logs-doomed", &format!("{lines}exit 3"))).unwrap();
    let asked = Instant::now();
    let err = doomed
        .call(Generation(1), echo(), Duration::from_secs(10))
        .unwrap_err();
    assert!(
        asked.elapsed() < Duration::from_millis(1500),
        "the crash took {:?} to report",
        asked.elapsed()
    );
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    let cause = err.cause().unwrap_or_default();
    assert!(cause.ends_with("last 25"), "{cause}");
    // Its lines wait behind the one stalled sink that every worker's go to.
    assert_eq!(sink.seen().len(), 1, "{:?}", sink.seen());
    // Released, the sink takes every line it was given: the warning, then the ended
    // worker's 25, none dropped.
    sink.release();
    assert_eq!(sink.wait_accounted(26, Duration::from_secs(30)), 26);
    drop(worker);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(sink.seen().len(), 26, "{:?}", sink.seen());
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
    // Found by break-it, against an earlier version of the hand-off: a queue of 1,024
    // lines filled in such a burst, and most of it was dropped, though the sink never
    // stalled.
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
    // The lines given, and the counts dropped, in the log so far.
    let counted = || {
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
        (given, dropped)
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    let (given, dropped) = loop {
        let (given, dropped) = counted();
        if given + dropped >= total || Instant::now() >= deadline {
            break (given, dropped);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(given + dropped, total, "{given} given, {dropped} counted");
    // Found by break-it, against an earlier version of the hand-off: a queue of 1,024
    // lines dropped most of the burst, though the sink never stalled.
    assert_eq!(given, total, "{} of {total} lines dropped", total - given);
}

/// Blocks closing a span named `asked` on the core's worker-log threads (a span exporter
/// whose collector stalls: tracing-opentelemetry exports a span as it closes, and a
/// synchronous processor waits on the collector), for a minute, so the test ends; and notes
/// where they closed: the sink thread's closing them is meant, the log thread's mustn't be.
#[derive(Clone, Default)]
struct StalledExporter {
    stall: bool,
    on_sink: Arc<AtomicBool>,
    on_reader: Arc<AtomicBool>,
}

impl<S> Layer<S> for StalledExporter
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_close(&self, id: tracing::span::Id, ctx: Context<'_, S>) {
        if !ctx.span(&id).is_some_and(|span| span.name() == "asked") {
            return;
        }
        let thread = std::thread::current();
        let name = thread.name().unwrap_or_default();
        let reader = name.starts_with("worker-") && name.ends_with("-logs");
        let sink = name == "silico-worker-log-sink";
        if reader {
            self.on_reader.store(true, Ordering::SeqCst);
        }
        if sink {
            self.on_sink.store(true, Ordering::SeqCst);
        }
        if self.stall && (reader || sink) {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
}

/// `printf` octal escapes for `bytes`.
fn octal(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\{b:03o}")).collect()
}

/// `printf` octal escapes for a reply of done to request `id`.
fn reply(id: u64) -> String {
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
}

/// The done line of request `trace`, as a worker writes it.
fn done(trace: u64) -> String {
    format!("printf '{{\"silico_request_done\":{trace}}}\\n' >&2")
}

#[test]
fn a_span_the_log_thread_lets_go_closes_on_the_sink_thread_and_the_next_call_is_answered() {
    // The control for the next test: the same worker, a sink that doesn't stall.
    span_close_case(false, &one_release(), 2);
}

#[test]
fn a_sink_stalled_closing_a_requests_span_never_hangs_the_next_request() {
    // Found by break-it, against main before the hand-off: the log thread let a request's
    // span go (`Callers::done`) holding the worker's callers lock, so the sink's `on_close`
    // ran there with the lock held; stalled, it stalled the log thread (the worker's pipe
    // went unread), and the next call blocked taking the lock before its timeout started,
    // and never returned.
    span_close_case(true, &one_release(), 2);
}

/// The worker answers the first request (trace 1: the first of this process) at once, and
/// says it's done half a second later, so the caller has let its span go by then and the
/// log thread's handle is the last: the span closes when the log thread lets it go. Then
/// it answers the second request.
fn one_release() -> String {
    format!(
        "/bin/dd bs=1 count=1 <&3 >/dev/null 2>&1\nprintf '{}' >&3\n/bin/sleep 0.5\n{}\n\
         /bin/sleep 1\nprintf '{}' >&3\nread -r _ <&3\nexit 0",
        reply(0),
        done(1),
        reply(1),
    )
}

#[test]
fn a_sink_stalled_closing_a_span_with_the_queue_full_never_hangs_the_next_request() {
    // Found in review, against an earlier version of the hand-off, which closed a request's
    // span on the log thread when the queue had no room for it. The sink stalls closing the
    // first request's span; more lines than the queue holds; the second request's done
    // line, its caller gone, with no room for its span; and more lines than a pipe holds,
    // before the third request's answer: the log thread must go on reading for it to come.
    let burst = burst_file("full-queue", PAST_THE_QUEUE);
    let body = format!(
        "/bin/dd bs=1 count=1 <&3 >/dev/null 2>&1\nprintf '{}' >&3\n/bin/sleep 0.5\n{}\n\
         /bin/sleep 1\nprintf '{}' >&3\n/bin/sleep 0.5\n/bin/cat '{burst}' >&2\n{}\n\
         /bin/cat '{burst}' >&2\nprintf '{}' >&3\nread -r _ <&3\nexit 0",
        reply(0),
        done(1),
        reply(1),
        done(2),
        reply(2),
        burst = burst.display(),
    );
    span_close_case(true, &body, 3);
}

/// Runs `calls` requests, a second apart, each in a span named `asked`, on a worker
/// running `body`, with a span exporter that stalls closing them if `stall`; each must be
/// answered.
fn span_close_case(stall: bool, body: &str, calls: u64) {
    let exporter = StalledExporter {
        stall,
        ..StalledExporter::default()
    };
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(exporter.clone()))
        .unwrap();
    let exe = script(&format!("worker-logs-span-close-{calls}"), body);
    let (answers, answered) = std::sync::mpsc::channel();
    let caller = std::thread::spawn(move || {
        let mut worker = Worker::spawn(&exe).unwrap();
        for i in 0..calls {
            let span = tracing::info_span!("asked", i);
            let answer = span.in_scope(|| {
                worker.call(
                    Generation(1),
                    Op::Debug(DebugOp::Sleep { millis: 0 }),
                    Duration::from_secs(5),
                )
            });
            drop(span);
            answers
                .send((i, answer.map_err(|e| e.to_string())))
                .unwrap();
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    for i in 0..calls {
        // Each call has 5 s; 20 s is a hang no deadline ends.
        let (asked, answer) = answered
            .recv_timeout(Duration::from_secs(20))
            .unwrap_or_else(|_| panic!("call {i} never returned"));
        assert_eq!(asked, i);
        assert_eq!(answer, Ok(silico_protocol::Reply::Done), "call {i}");
    }
    // The worker gone too (the last request's span, never said done, is let go then) and
    // its log read to the end: the first request's span did close as the log thread let it
    // go, on the sink thread, so the case was the one meant, and none on the log thread.
    caller.join().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(exporter.on_sink.load(Ordering::SeqCst));
    assert!(!exporter.on_reader.load(Ordering::SeqCst));
}

#[test]
fn a_timed_out_requests_span_never_closes_on_the_log_thread() {
    // Found in review, against an earlier version of the hand-off: a request that timed out
    // never has its done line, so its span stays with the log thread's; with a child of the
    // worker holding the log open, the log thread outlived the worker, and let the span go
    // last, closing it there (where a stalled exporter would have kept it forever).
    let exporter = StalledExporter::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(exporter.clone()))
        .unwrap();
    let exe = script(
        "worker-logs-timed-out-span",
        "( exec 3>&-; /bin/sleep 1 ) &\nexec /bin/sleep 30",
    );
    let mut worker = Worker::spawn(&exe).unwrap();
    let span = tracing::info_span!("asked");
    let err = span
        .in_scope(|| worker.call(Generation(1), echo(), Duration::from_millis(300)))
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerTimeout, "{err}");
    drop(span);
    drop(worker);
    // The child lets the log close a second on; then the span goes.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !exporter.on_sink.load(Ordering::SeqCst) && !exporter.on_reader.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "the request's span never closed");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!exporter.on_reader.load(Ordering::SeqCst));
    assert!(exporter.on_sink.load(Ordering::SeqCst));
}

#[test]
fn every_workers_log_goes_to_one_sink_thread() {
    // One thread, and one bound, however many workers: a second worker's line waits behind
    // the first's at the one stalled sink, rather than going to a sink thread of its own
    // (each of which, left stalled, would be a thread and a queue for every worker the pool
    // replaced).
    let sink = Sink::install(Mode::Stall);
    let _first = Worker::spawn(&script(
        "worker-logs-first",
        "printf 'first\\n' >&2\nread -r _ <&3\nexit 0",
    ))
    .unwrap();
    sink.wait_entered();
    let mut second = Worker::spawn(&script(
        "worker-logs-second",
        "printf 'second\\n' >&2\nexit 0",
    ))
    .unwrap();
    let err = second
        .call(Generation(1), echo(), Duration::from_secs(20))
        .unwrap_err();
    assert!(err.cause().unwrap_or_default().ends_with("second"), "{err}");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(sink.seen(), [Seen::Line("first".to_owned())]);
    sink.release();
    assert_eq!(sink.wait_accounted(2, Duration::from_secs(10)), 2);
}

#[test]
fn a_crash_report_comes_after_its_workers_last_lines_are_logged() {
    // A sink that keeps up, if slowly (2 ms a line, 40 ms in all, well within the report's
    // wait for it): every line is logged before the error that reports them comes back.
    let sink = Sink::install(Mode::Slow { micros: 2_000 });
    let body: String = (0..20)
        .map(|i| format!("printf 'line {i}\\n' >&2\n"))
        .collect();
    let mut worker = Worker::spawn(&script("worker-logs-flush", &format!("{body}exit 0"))).unwrap();
    let err = worker
        .call(Generation(1), echo(), Duration::from_secs(20))
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    assert_eq!(line_numbers(&sink.seen()), (0..20).collect::<Vec<u64>>());
}

/// Sleeps opening each `worker_call` span (a slow log sink on the calling thread).
#[derive(Clone, Default)]
struct SlowToOpenCalls;

impl<S: Subscriber> Layer<S> for SlowToOpenCalls {
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _: &tracing::span::Id,
        _: Context<'_, S>,
    ) {
        if attrs.metadata().name() == "worker_call" {
            std::thread::sleep(Duration::from_millis(1500));
        }
    }
}

#[test]
fn a_calls_time_starts_after_its_own_logging() {
    // A log sink slow on the calling thread spends the caller's time, not the worker's: a
    // worker that answers at once isn't timed out for it.
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(SlowToOpenCalls))
        .unwrap();
    let exe = script(
        "worker-logs-slow-call-span",
        &format!(
            "/bin/dd bs=1 count=1 <&3 >/dev/null 2>&1\nprintf '{}' >&3\nread -r _ <&3\nexit 0",
            reply(0)
        ),
    );
    let mut worker = Worker::spawn(&exe).unwrap();
    let answer = worker.call(
        Generation(1),
        Op::Debug(DebugOp::Sleep { millis: 0 }),
        Duration::from_secs(1),
    );
    assert_eq!(
        answer.map_err(|e| e.to_string()),
        Ok(silico_protocol::Reply::Done)
    );
}

/// Counts the core's warnings that a worker hasn't said requests were done.
#[derive(Clone, Default)]
struct PushedOut(Arc<std::sync::atomic::AtomicUsize>);

impl<S: Subscriber> Layer<S> for PushedOut {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        if fields
            .message
            .contains("hasn't said 4096 requests were done")
        {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[test]
fn a_call_warns_when_its_worker_hasnt_said_the_most_requests_were_done() {
    // A worker that answers 4,097 requests and never says one is done: the core holds the
    // spans of the most (4,096), and the request after them pushes the oldest out, warned
    // of once, with that call.
    let warned = PushedOut::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(warned.clone()))
        .unwrap();
    let calls: u64 = 4_097;
    let replies: String = (0..calls).map(reply).collect();
    let frames = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("worker-logs-replies-{}.txt", std::process::id()));
    std::fs::write(&frames, format!("printf '{replies}' >&3\n")).unwrap();
    let exe = script(
        "worker-logs-never-done",
        // It reads the requests (and drops them), so they never fill its socket: Linux
        // holds less of them than macOS.
        &format!(
            "/bin/cat <&3 >/dev/null &\n. '{}'\nexec /bin/sleep 60",
            frames.display()
        ),
    );
    let mut worker = Worker::spawn(&exe).unwrap();
    for i in 0..calls {
        let answer = worker.call(
            Generation(1),
            Op::Debug(DebugOp::Sleep { millis: 0 }),
            Duration::from_secs(10),
        );
        assert_eq!(
            answer.map_err(|e| e.to_string()),
            Ok(silico_protocol::Reply::Done)
        );
        let want = usize::from(i == calls - 1);
        assert_eq!(warned.0.load(Ordering::SeqCst), want, "call {i}");
    }
}
