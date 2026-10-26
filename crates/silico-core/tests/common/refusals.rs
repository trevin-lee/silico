//! For the tests of kept measures: a kernel's refusals of a measure, as a worker gives
//! them where a worker would.

use silico_core::fake::FakeKernel;
use silico_core::{Error, ErrorCode};
use silico_protocol::{Failure, Operation, Refusal};

/// The failure a kernel worker gives for a measure refused with `code`, as data, if a
/// worker gives that code (a crash, a hang, a reply that can't be read are the worker
/// client's own).
pub fn worker_failure(code: ErrorCode) -> Option<Failure> {
    let refusal = match code {
        ErrorCode::InvalidInput => Refusal::BrepCorrupted,
        ErrorCode::InvalidResult => Refusal::ResultInvalid {
            operation: Operation::Other,
            edges: Vec::new(),
        },
        ErrorCode::KernelFailed => Refusal::NotFinite {
            quantity: "volume".to_owned(),
            value: f64::INFINITY,
        },
        ErrorCode::ResourceExhausted => Refusal::OutOfMemory,
        ErrorCode::Internal => Refusal::Internal {
            what: "the fake was told to".to_owned(),
        },
        _ => return None,
    };
    Some(Failure::new(refusal))
}

/// A kernel that refuses every measure with `code`: as a worker would, if one gives it.
pub fn refusing(kernel: &FakeKernel, code: ErrorCode) {
    refusing_to(kernel, code, "Summarize");
}

/// A kernel that refuses every request of the operation `op` (a measure's, `Summarize` or
/// `Describe`) with `code`: as a worker would, if one gives it.
pub fn refusing_to(kernel: &FakeKernel, code: ErrorCode, op: &'static str) {
    match worker_failure(code) {
        Some(failure) => {
            kernel.refuse_as_worker(move |asked| (asked.name() == op).then(|| failure.clone()))
        }
        None => kernel.refuse(move |asked| {
            (asked.name() == op).then(|| Error::new(code, format!("{op} failed")))
        }),
    }
}
