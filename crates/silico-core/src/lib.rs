//! Silico's headless core. So far it holds the structured [`Error`] every client sees and
//! the worker client ([`worker::Worker`]); the document model, feature graph, recompute,
//! topological naming, and the command API every client uses arrive from M1 on.

mod error;
pub mod kernel;
pub mod logging;
pub mod worker;

pub use error::{Error, ErrorCode};
