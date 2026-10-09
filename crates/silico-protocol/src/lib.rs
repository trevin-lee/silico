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

/// Identifies a request to a worker. Chosen by the core; the response carries it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RequestId(pub u64);

/// The document generation a request belongs to. Results for a superseded generation are
/// discarded by the core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Generation(pub u64);

/// Ties a worker's logs for a request to the core's span for it (ARCHITECTURE.md, Errors
/// and logging): the worker records it on its request span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TraceId(pub u64);

/// A request from the core to a worker. The three numbers are distinct types, so one
/// can't be passed for another; on the wire each is a plain number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: RequestId,
    pub generation: Generation,
    pub trace: TraceId,
    pub op: Op,
}

/// An operation for a worker. Lengths are in the document's unit (millimetres in v1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    /// A box from the origin to `(dx, dy, dz)`. Replies with [`Reply::Shape`].
    MakeBox { dx: f64, dy: f64, dz: f64 },
    /// A shape's volume, area, center, topology counts, and validity (as `shape` says).
    /// Replies with [`Reply::Summary`].
    Summarize { shape: ShapeData },
    /// Rounds `edges` of `shape` with a constant `radius`. Edges are numbered from 1 in
    /// OCCT's `TopExp::MapShapes` order, as FreeCAD's `Edge1`, `Edge2`, and so on. Replies
    /// with [`Reply::Shape`].
    Fillet {
        shape: ShapeData,
        edges: Vec<u32>,
        radius: f64,
    },
    /// A triangle mesh of a shape for display, no further than `deflection` from its
    /// surface. Replies with [`Reply::Mesh`].
    Tessellate { shape: ShapeData, deflection: f64 },
    /// Sends `data` straight back as [`Reply::Echo`]: a health check, and the cheapest
    /// round trip, which the IPC benchmark measures.
    Echo { data: Blob },
    /// Operations only the test worker answers, for tests of how the core treats a worker
    /// that crashes, hangs, or misbehaves. The production worker refuses them all.
    Debug(DebugOp),
}

impl Op {
    /// The operation's name, for logs and errors.
    pub fn name(&self) -> &'static str {
        match self {
            Op::MakeBox { .. } => "MakeBox",
            Op::Summarize { .. } => "Summarize",
            Op::Fillet { .. } => "Fillet",
            Op::Tessellate { .. } => "Tessellate",
            Op::Echo { .. } => "Echo",
            Op::Debug(op) => op.name(),
        }
    }
}

/// A test operation (see [`Op::Debug`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DebugOp {
    /// Sleep, then reply [`Reply::Done`].
    Sleep { millis: u64 },
    /// Abort the process.
    Abort,
    /// Crash with a segmentation fault.
    Segfault,
    /// Write bytes that aren't a valid frame.
    Garbage,
    /// Turn floating-point traps on (only possible on Linux), which the worker must notice
    /// after the request.
    EnableFpTraps,
    /// First send a reply out of turn, then the real one.
    ReplyOutOfTurn(OutOfTurn),
}

/// How [`DebugOp::ReplyOutOfTurn`]'s extra reply is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutOfTurn {
    /// It carries the previous request's id.
    WrongId,
    /// It carries this request's id, with another generation.
    WrongGeneration,
}

impl DebugOp {
    pub fn name(&self) -> &'static str {
        match self {
            DebugOp::Sleep { .. } => "DebugSleep",
            DebugOp::Abort => "DebugAbort",
            DebugOp::Segfault => "DebugSegfault",
            DebugOp::Garbage => "DebugGarbage",
            DebugOp::EnableFpTraps => "DebugEnableFpTraps",
            DebugOp::ReplyOutOfTurn(OutOfTurn::WrongId) => "DebugReplyWithWrongId",
            DebugOp::ReplyOutOfTurn(OutOfTurn::WrongGeneration) => "DebugReplyWithWrongGeneration",
        }
    }
}

/// A worker's answer to the request with the same `id`, echoing its `generation`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: RequestId,
    pub generation: Generation,
    pub result: Result<Reply, Failure>,
}

/// A successful result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Reply {
    /// A shape a worker made.
    Shape(ShapeData),
    Summary(ShapeSummary),
    Mesh(Mesh),
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

/// A triangle mesh of a shape's faces, and polylines along its edges, for display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mesh {
    pub positions: Vec<[f64; 3]>,
    /// Vertex indices, three per triangle, wound counterclockwise seen from outside.
    pub triangles: Vec<[u32; 3]>,
    /// How many triangles belong to each face, in OCCT's face order.
    pub face_triangles: Vec<u32>,
    /// The points of every edge's polyline, one edge after another, split by
    /// `edge_point_counts`. Flat, so every value on the wire costs no more than a few times
    /// its size to read (a list of lists would let a few bytes cost a whole empty list).
    pub edge_points: Vec<[f64; 3]>,
    /// How many of `edge_points` belong to each edge: the first count is `Edge1`'s, as
    /// [`Op::Fillet`] numbers them.
    pub edge_point_counts: Vec<u32>,
}

impl Mesh {
    /// Each edge's polyline, in order: the first is `Edge1`. An edge whose points aren't
    /// there (in a mesh that fails [`Mesh::validate`]) is empty.
    pub fn edges(&self) -> impl Iterator<Item = &[[f64; 3]]> {
        let mut start = 0usize;
        self.edge_point_counts.iter().map(move |&count| {
            let end = start.saturating_add(count as usize);
            let points = self.edge_points.get(start..end).unwrap_or(&[]);
            start = end;
            points
        })
    }

    /// Checks that the mesh is consistent: every index names a vertex, the per-face counts
    /// and per-edge counts add up, and every coordinate is finite. Run it before indexing a
    /// mesh that came from another process.
    pub fn validate(&self) -> Result<(), String> {
        let vertices = self.positions.len();
        if let Some(t) = self
            .triangles
            .iter()
            .find(|t| t.iter().any(|&i| i as usize >= vertices))
        {
            return Err(format!(
                "triangle {t:?} names a vertex past the {vertices} there are"
            ));
        }
        let counted: u64 = self.face_triangles.iter().map(|&n| u64::from(n)).sum();
        if counted != self.triangles.len() as u64 {
            return Err(format!(
                "faces account for {counted} triangles, but there are {}",
                self.triangles.len()
            ));
        }
        let edge_points: u64 = self.edge_point_counts.iter().map(|&n| u64::from(n)).sum();
        if edge_points != self.edge_points.len() as u64 {
            return Err(format!(
                "edges account for {edge_points} points, but there are {}",
                self.edge_points.len()
            ));
        }
        let finite = |p: &[f64; 3]| p.iter().all(|c| c.is_finite());
        if !self.positions.iter().chain(&self.edge_points).all(finite) {
            return Err("a coordinate isn't finite".to_owned());
        }
        Ok(())
    }
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
    /// What to try instead, when the worker has a suggestion.
    pub suggestion: Option<String>,
}

impl Failure {
    /// A failure with no element or suggestion; add them with the fields.
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            element: None,
            suggestion: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureKind {
    /// The request's values were invalid.
    InvalidInput,
    /// The kernel refused or failed the operation.
    KernelFailed,
    /// The kernel performed the operation, but the result isn't valid, so the worker
    /// refused it.
    InvalidResult,
    /// This worker doesn't perform that operation.
    UnknownOp,
    /// Something went wrong in the worker itself, such as floating-point traps found
    /// switched on after the request.
    Internal,
}

/// A shape as it crosses the worker boundary: OCCT's binary B-rep (with its checksum), and
/// whether OCCT's `BRepCheck_Analyzer` finds the whole shape valid, if that's known. A
/// worker checks each shape it makes and says; the core sends the shape back as it got it,
/// and the worker takes its word, so a shape a worker made is never checked as a whole
/// again (the check costs as much as the shape is large). A shape no worker has vouched
/// for (a file, an import) is [`ShapeData::unchecked`], and each worker given it checks
/// it.
///
/// The fields are private so validity can't be claimed by accident: a
/// [`ShapeData::checked`] is only ever made from a worker's word, given live in a reply
/// or replayed from where the core stored that reply (the cache, M2, stores the validity
/// with the B-rep, under the same checksum).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapeData {
    brep: Blob,
    valid: Option<bool>,
}

impl ShapeData {
    /// A shape whose validity isn't known.
    pub fn unchecked(brep: Blob) -> Self {
        Self { brep, valid: None }
    }

    /// A shape whose validity a worker has checked: in its reply, or rebuilt from a stored
    /// reply. Never from a guess.
    pub fn checked(brep: Blob, valid: bool) -> Self {
        Self {
            brep,
            valid: Some(valid),
        }
    }

    pub fn brep(&self) -> &Blob {
        &self.brep
    }

    /// Whether the whole shape is valid, if a worker has checked it.
    pub fn valid(&self) -> Option<bool> {
        self.valid
    }
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
