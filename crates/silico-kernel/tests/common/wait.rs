//! The one wait the kernel tests allow for something they expect to happen (a worker's
//! answer, a worker to start or exit, a pool to refill, a log line to arrive): long enough
//! for a loaded machine, so only a real hang fails, and it costs nothing when the event
//! comes. Half nextest's 120 s, so a hang is said here.
#![allow(dead_code)]

use std::time::Duration;

pub const WAIT: Duration = Duration::from_secs(60);
