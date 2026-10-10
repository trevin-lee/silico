//! Silico's headless core. So far it holds the structured [`Error`] every client sees, the
//! worker client ([`worker::Worker`]), typed kernel requests on one worker
//! ([`kernel::KernelWorker`]) or a pool of them ([`pool::KernelPool`]); the document model,
//! feature graph, recompute, topological naming, and the command API every client uses
//! arrive from M2 on.

mod error;
pub mod kernel;
pub mod logging;
pub mod pool;
pub mod worker;

pub use error::{Error, ErrorCode};
