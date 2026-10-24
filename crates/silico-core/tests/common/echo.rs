//! For tests of a kernel's queues and clients: echoes told apart by a byte, and the bytes
//! of those a kernel started, in order.

use silico_core::fake::Call;
use silico_core::kernel::{Blob, KernelRequest, Op};

/// An echo of the one byte `byte`.
pub fn echo(byte: u8) -> KernelRequest<Blob> {
    KernelRequest::echo(Blob::inline(vec![byte]))
}

/// The first byte of each echo in `calls`, in the order they started; failing on any call
/// that isn't an echo of at least one byte.
pub fn echoed(calls: &[Call]) -> Vec<u8> {
    calls
        .iter()
        .map(|call| match &call.op {
            Op::Echo { data } if !data.as_bytes().is_empty() => data.as_bytes()[0],
            other => panic!("not an echo of a byte: {other:?}"),
        })
        .collect()
}
