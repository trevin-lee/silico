//! A test's time limit in CPU time, which a loaded machine doesn't change.

use std::time::Duration;

/// `f`'s result, if it took at most `limit` of its thread's CPU time: what the work costs,
/// which a loaded machine doesn't change (other processes take wall time, not the thread's
/// CPU). A hang fails at ten times `limit` of wall time. A test using this is named
/// `..._promptly`, so `.config/nextest.toml` runs it apart from the heavy tests.
pub fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (out, spent) = spending(limit * 10, f);
    assert!(spent <= limit, "took {spent:?} of CPU, more than {limit:?}");
    out
}

/// `f`'s result and the CPU time its thread took, failing if it hasn't finished within
/// `hang` of wall time.
pub fn spending<T: Send + 'static>(
    hang: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> (T, Duration) {
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let start = cpu_time::ThreadTime::now();
        let out = f();
        let _ = tx.send((out, start.elapsed()));
    });
    match rx.recv_timeout(hang) {
        Ok(done) => done,
        // `f` panicked: its panic, not a timeout.
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => match thread.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            Ok(()) => panic!("the thread ended without a result"),
        },
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!("didn't finish within {hang:?} of wall time")
        }
    }
}
