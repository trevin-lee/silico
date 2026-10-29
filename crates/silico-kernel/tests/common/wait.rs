//! The one wait the kernel tests allow for something they expect to happen (a worker's
//! answer, a worker to start or exit, a pool to refill, a log line to arrive): long enough
//! for a loaded machine, so only a real hang fails, and it costs nothing when the event
//! comes. Half nextest's 120 s, so a hang is said here.
#![allow(dead_code)]

use std::time::Duration;

pub const WAIT: Duration = Duration::from_secs(60);

/// A request's time no test reaches (a sleep, or a timeout): set where a test checks that
/// something comes promptly (a drop, a kill, a stop), so that, wrong, it would wait this out
/// and fail at [`WAIT`] rather than pass, however loaded the machine. Ten times `WAIT`.
pub const UNREACHED: Duration = Duration::from_secs(600);

/// `f`'s result, if it returns within [`WAIT`]: for something that should be prompt (a
/// drop) and would, wrong, block for [`UNREACHED`], which a test must fail at, not hang on.
/// Run on a thread of its own, left blocked if it never returns; a panic in it is its own.
pub fn promptly<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(WAIT) {
        Ok(out) => out,
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => match thread.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            Ok(()) => panic!("{what} ended without a result"),
        },
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!("{what} didn't return in {WAIT:?}")
        }
    }
}
