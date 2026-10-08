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

/// The file descriptor a worker inherits its end of the socket pair as.
pub const WORKER_FD: i32 = 3;

/// The environment variable that tells a worker which descriptor its socket is. A worker
/// started without it wasn't started by the core, and refuses to run.
pub const WORKER_FD_ENV: &str = "SILICO_WORKER_FD";

/// A request from the core to a worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Chosen by the core; the response carries it back.
    pub id: u64,
    /// The document generation the request belongs to. Results for a superseded
    /// generation are discarded by the core.
    pub generation: u64,
    /// Ties the worker's logs for this request to the core's span for it (ARCHITECTURE.md,
    /// Errors and logging): the worker records it on its request span.
    pub trace: u64,
    pub op: Op,
}

/// An operation for a worker. Lengths are in the document's unit (millimetres in v1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    /// A box from the origin to `(dx, dy, dz)`. Replies with [`Reply::Shape`].
    MakeBox { dx: f64, dy: f64, dz: f64 },
    /// A shape's volume, area, center, topology counts, and validity. Replies with
    /// [`Reply::Summary`].
    Summarize { shape: Blob },
    /// Rounds `edges` of `shape` with a constant `radius`. Edges are numbered from 1 in
    /// OCCT's `TopExp::MapShapes` order, as FreeCAD's `Edge1`, `Edge2`, and so on. Replies
    /// with [`Reply::Shape`].
    Fillet {
        shape: Blob,
        edges: Vec<u32>,
        radius: f64,
    },
    /// Sends `data` straight back as [`Reply::Echo`]: a health check, and the cheapest
    /// round trip, which the IPC benchmark measures.
    Echo { data: Blob },
    /// Test operations, answered only by the test worker: sleep, then reply [`Reply::Done`].
    DebugSleep { millis: u64 },
    /// Test operation: abort the process.
    DebugAbort,
    /// Test operation: crash with a segmentation fault.
    DebugSegfault,
    /// Test operation: write bytes that aren't a valid frame.
    DebugGarbage,
    /// Test operation: turn floating-point traps on (only possible on Linux), which the
    /// worker must notice after the request.
    DebugEnableFpTraps,
    /// Test operation: first send a reply out of turn, carrying the previous request's id
    /// (`wrong_id`) or this request's id with another generation.
    DebugReplyOutOfTurn { wrong_id: bool },
}

impl Op {
    /// The operation's name, for logs and errors.
    pub fn name(&self) -> &'static str {
        match self {
            Op::MakeBox { .. } => "MakeBox",
            Op::Summarize { .. } => "Summarize",
            Op::Fillet { .. } => "Fillet",
            Op::Echo { .. } => "Echo",
            Op::DebugSleep { .. } => "DebugSleep",
            Op::DebugAbort => "DebugAbort",
            Op::DebugSegfault => "DebugSegfault",
            Op::DebugGarbage => "DebugGarbage",
            Op::DebugEnableFpTraps => "DebugEnableFpTraps",
            Op::DebugReplyOutOfTurn { .. } => "DebugReplyOutOfTurn",
        }
    }
}

/// A worker's answer to the request with the same `id`, echoing its `generation`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub generation: u64,
    pub result: Result<Reply, Failure>,
}

/// A successful result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Reply {
    /// A shape, as OCCT's binary B-rep.
    Shape {
        brep: Blob,
    },
    Summary(ShapeSummary),
    Echo {
        data: Blob,
    },
    /// The operation succeeded and has nothing to return.
    Done,
}

/// What a solid is, from [`Op::Summarize`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShapeSummary {
    pub volume: f64,
    pub area: f64,
    pub center: [f64; 3],
    pub faces: u32,
    pub edges: u32,
    pub vertices: u32,
    /// Whether OCCT's `BRepCheck_Analyzer` finds the shape valid.
    pub valid: bool,
}

/// An operation the worker refused or that failed. The core turns this into a structured
/// error, adding the feature and element it knows about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub kind: FailureKind,
    /// What went wrong, in the kernel's words where the kernel reported it.
    pub message: String,
    /// The sub-element at fault, such as `Edge1`, when the worker knows it.
    pub element: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureKind {
    /// The request's values were invalid.
    InvalidInput,
    /// The kernel refused or failed the operation.
    KernelFailed,
    /// This worker doesn't perform that operation.
    UnknownOp,
    /// Something went wrong in the worker itself, such as floating-point traps found
    /// switched on after the request.
    Internal,
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
