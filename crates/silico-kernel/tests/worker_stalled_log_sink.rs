//! M2.4: a log sink that blocks outright (a stalled terminal, a pipe no one reads) never
//! holds up a worker. Its log is still read, so the line every request ends with doesn't
//! fill the worker's pipe until the worker blocks writing one, and stops answering, which
//! would look like a hang in the geometry. Alone in its binary: it installs the process's
//! global subscriber.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::kernel::{KernelRequest, KernelWorker};
use silico_core::worker::Worker;
use silico_protocol::{Blob, BooleanOp, DebugOp, Generation, Op, Primitive};
use tracing::Subscriber;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

/// Blocks at the first worker line until released, and counts the worker lines it's given.
#[derive(Clone, Default)]
struct Stalled {
    entered: Arc<AtomicBool>,
    release: Arc<(Mutex<bool>, Condvar)>,
    lines: Arc<AtomicUsize>,
}

impl<S: Subscriber> Layer<S> for Stalled {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() != "silico::worker" {
            return;
        }
        self.lines.fetch_add(1, Ordering::SeqCst);
        self.entered.store(true, Ordering::SeqCst);
        let (released, opened) = &*self.release;
        let mut released = released.lock().unwrap();
        while !*released {
            released = opened.wait(released).unwrap();
        }
    }
}

#[test]
fn a_log_sink_that_blocks_outright_never_holds_up_a_worker() {
    let sink = Stalled::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(sink.clone()))
        .unwrap();
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
    let deadline = Instant::now() + Duration::from_secs(30);
    while !sink.entered.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "the worker's warning never came");
        std::thread::sleep(Duration::from_millis(5));
    }
    // Twice as many requests as a pipe holds the lines that end them (about 2,300 in a
    // 64 KiB pipe: the core's unit test of the pipe measures it), each answered in time.
    let requests: u32 = 5_000;
    for i in 0..requests {
        let data = Blob::inline(i.to_le_bytes().to_vec());
        let echoed = worker
            .run(KernelRequest::echo(data.clone()).with_timeout(Duration::from_secs(10)))
            .unwrap_or_else(|e| panic!("request {i} of {requests}: {e}"));
        assert_eq!(echoed, data, "request {i}");
    }
    // The sink took the warning and nothing since: no request logged.
    assert_eq!(sink.lines.load(Ordering::SeqCst), 1);
    // A worker that crashes while the sink is stalled is still reported promptly, with its
    // last lines: they're kept as they're read, not as the sink takes them.
    let mut doomed =
        Worker::spawn(Path::new(env!("CARGO_BIN_EXE_silico-kernel-test-worker"))).unwrap();
    let asked = Instant::now();
    let err = doomed
        .call(
            Generation(1),
            Op::Debug(DebugOp::Abort),
            Duration::from_secs(10),
        )
        .unwrap_err();
    assert!(
        asked.elapsed() < Duration::from_secs(8),
        "the crash took {:?} to report",
        asked.elapsed()
    );
    assert_eq!(err.code(), ErrorCode::WorkerCrashed, "{err}");
    let cause = err.cause().unwrap_or_default();
    assert!(cause.contains("aborting, as asked: line 25"), "{cause}");
    // Released, the sink takes every line it was given: the warning, then the crashed
    // worker's 25, none dropped.
    let (released, opened) = &*sink.release;
    *released.lock().unwrap() = true;
    opened.notify_all();
    let deadline = Instant::now() + Duration::from_secs(30);
    while sink.lines.load(Ordering::SeqCst) < 26 {
        assert!(
            Instant::now() < deadline,
            "the sink was given {} lines",
            sink.lines.load(Ordering::SeqCst)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(worker);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(sink.lines.load(Ordering::SeqCst), 26);
}
