//! Silico's headless core. So far it holds the structured [`Error`] every client sees, the
//! worker client ([`worker::Worker`]), typed kernel requests on one worker
//! ([`kernel::KernelWorker`]) or, through the [`kernel::Kernel`] trait, a pool of them
//! ([`pool::KernelPool`]) or an in-memory fake for tests (`fake::FakeKernel`, with the
//! `fake` feature), the document's types, read and written exactly and checked
//! ([`document`]), recompute ([`recompute::Recompute`]), and the rebuild cache
//! ([`cache::Cache`]), with each feature's fingerprint ([`document::Fingerprint`]),
//! topological naming ([`naming`]: each element's name, from the kernel's history), and the
//! first of the commands every client runs ([`command`]: gate, rebuild, check, format, outline,
//! and sessions, each a document held open between commands on a thread of its own), and a
//! document's file read and saved safely ([`file`]); and a solved sketch's profile
//! ([`profile`]: its loops, nested into regions and oriented, or what keeps it from being
//! one, named), and what a sketch feature's rebuild refuses of its profile, its solve, and
//! its result ([`sweep`]); the rest of the command API arrives later.

pub mod cache;
pub mod command;
pub mod document;
pub mod elements;
mod error;
pub mod export;
#[cfg(feature = "fake")]
pub mod fake;
pub mod file;
pub mod kernel;
pub mod logging;
pub mod meshes;
pub mod naming;
pub mod pool;
pub mod profile;
pub mod recompute;
mod references;
pub mod render;
pub mod sketch;
pub mod sweep;
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

/// `N` bytes of the system's randomness, from `/dev/urandom`: the cache's secret, and the
/// IDs of new features.
pub(crate) fn system_random<const N: usize>() -> std::io::Result<[u8; N]> {
    use std::io::Read as _;
    let mut bytes = [0; N];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes)
}
