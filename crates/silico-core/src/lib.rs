//! Silico's headless core. So far it holds the structured [`Error`] every client sees, the
//! worker client ([`worker::Worker`]), typed kernel requests on one worker
//! ([`kernel::KernelWorker`]) or, through the [`kernel::Kernel`] trait, a pool of them
//! ([`pool::KernelPool`]) or an in-memory fake for tests (`fake::FakeKernel`, with the
//! `fake` feature), the document's types, read and written exactly and checked
//! ([`document`]), recompute ([`recompute::Recompute`]), and the rebuild cache
//! ([`cache::Cache`]), with each feature's fingerprint ([`document::Fingerprint`]);
//! topological naming and the command API every client uses arrive later in M2.

pub mod cache;
pub mod document;
mod error;
#[cfg(feature = "fake")]
pub mod fake;
pub mod kernel;
pub mod logging;
pub mod naming;
pub mod pool;
pub mod recompute;
pub mod worker;

pub use error::{Error, ErrorCode};

/// `bytes` in lowercase hex, two digits each.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut hex = String::with_capacity(2 * bytes.len());
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}
