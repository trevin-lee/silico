//! The one wait the oracle's tests allow for something they expect to happen (a harness run
//! to end, its stand-in FreeCAD hung or not): long enough for a loaded machine, so only a
//! run that truly hangs fails, and below the harness's own case deadline (120 s), so a
//! harness that ignored a shorter one still fails.
#![allow(dead_code)]

use std::time::Duration;

pub const WAIT: Duration = Duration::from_secs(60);
