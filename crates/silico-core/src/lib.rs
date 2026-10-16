//! Silico's headless core. So far it holds the structured [`Error`] every client sees, the
//! worker client ([`worker::Worker`]), typed kernel requests on one worker
//! ([`kernel::KernelWorker`]) or, through the [`kernel::Kernel`] trait, a pool of them
//! ([`pool::KernelPool`]) or an in-memory fake for tests (`fake::FakeKernel`, with the `fake` feature), and the document's
//! types, read and written exactly ([`document`]); the feature graph, recompute,
//! topological naming, and the command API every client uses arrive later in M2.

pub mod cache;
pub mod document;
mod error;
#[cfg(feature = "fake")]
pub mod fake;
pub mod kernel;
pub mod logging;
pub mod pool;
pub mod recompute;
pub mod worker;

pub use error::{Error, ErrorCode};
