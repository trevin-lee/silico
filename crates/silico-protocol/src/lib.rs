//! Message types shared by the core and the worker processes, and how they're framed on
//! the wire (ARCHITECTURE.md, Process model).
//!
//! The core sends a [`Request`]; a worker answers with a [`Response`] carrying the same
//! `id`. Each message travels as one frame: a 4-byte little-endian length, then the message
//! encoded with `postcard` ([`frame`]). Both ends are Rust and always ship together, so
//! there's no version negotiation.

pub mod frame;

use serde::{Deserialize, Serialize};

pub use frame::{FrameError, FrameReader, MAX_FRAME_LEN, write_message};

/// A request from the core to a worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Chosen by the core; the response carries it back.
    pub id: u64,
    /// The document generation the request belongs to. Results for a superseded
    /// generation are discarded by the core.
    pub generation: u64,
    pub op: Op,
}

/// An operation for a worker. Lengths are in the document's unit (millimetres in v1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    /// A box from the origin to `(dx, dy, dz)`. Replies with [`Reply::Shape`].
    MakeBox { dx: f64, dy: f64, dz: f64 },
    /// The volume and center of mass of a shape. Replies with [`Reply::MassProperties`].
    MassProperties { shape: Blob },
    /// Test operations, answered only by the test worker: sleep, then reply [`Reply::Done`].
    DebugSleep { millis: u64 },
    /// Test operation: abort the process.
    DebugAbort,
    /// Test operation: crash with a segmentation fault.
    DebugSegfault,
    /// Test operation: write bytes that aren't a valid frame.
    DebugGarbage,
}

/// A worker's answer to the request with the same `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub result: Result<Reply, Failure>,
}

/// A successful result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Reply {
    /// A shape, as OCCT's binary B-rep.
    Shape {
        brep: Blob,
    },
    MassProperties {
        volume: f64,
        center: [f64; 3],
    },
    /// The operation succeeded and has nothing to return.
    Done,
}

/// An operation the worker refused or that failed. The core turns this into a structured
/// error, adding the feature and element it knows about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub kind: FailureKind,
    /// What went wrong, in the kernel's words where the kernel reported it.
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureKind {
    /// The request's values were invalid.
    InvalidInput,
    /// The kernel refused or failed the operation.
    KernelFailed,
    /// This worker doesn't perform that operation.
    UnknownOp,
}

/// Bytes carried by a message, such as a serialized shape. Sent inline in the frame for
/// now; a shared-memory variant can be added for large payloads without changing any
/// message that carries a `Blob`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Blob {
    Inline(#[serde(with = "serde_bytes")] Vec<u8>),
}

impl Blob {
    pub fn inline(bytes: Vec<u8>) -> Self {
        Blob::Inline(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Blob::Inline(bytes) => bytes,
        }
    }
}

impl std::fmt::Debug for Blob {
    /// The length, not the bytes: a shape can be megabytes.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Blob::Inline(bytes) => write!(f, "Blob::Inline({} bytes)", bytes.len()),
        }
    }
}
