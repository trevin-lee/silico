//! Typed geometry operations on one kernel worker: the core's layer over the protocol.
//! Each operation refuses values that can't work before anything is sent, matches the
//! worker's reply to the operation, and checks what comes back, so nothing above it handles
//! a worker's raw reply. Recompute and the worker pool (M1.6) build on it, and debug tools
//! use it directly; clients reach geometry through the command API (ARCHITECTURE.md,
//! invariant 3) once there is one.

use std::path::{Path, PathBuf};
use std::time::Duration;

use silico_protocol::{Generation, Op, Reply};
pub use silico_protocol::{Mesh, Primitive, ShapeData, ShapeSummary};

use crate::worker::Worker;
use crate::{Error, ErrorCode};

/// The environment variable naming the kernel worker executable, for when it isn't next
/// to the running one.
pub const KERNEL_WORKER_ENV: &str = "SILICO_KERNEL_WORKER";

/// How long an operation may take before its worker is stopped, unless set otherwise.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// A kernel worker, and the operations it performs.
pub struct KernelWorker {
    worker: Worker,
    timeout: Duration,
}

impl KernelWorker {
    /// Starts the kernel worker: the one [`KERNEL_WORKER_ENV`] names, or
    /// `silico-kernel-worker` next to the running executable (or one directory up, where a
    /// test binary's worker is).
    pub fn start() -> Result<Self, Error> {
        Self::start_worker(&worker_path()?)
    }

    /// Starts the kernel worker at `exe`.
    pub fn start_worker(exe: &Path) -> Result<Self, Error> {
        Ok(Self {
            worker: Worker::spawn(exe)?,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// Operations on a worker that's already running. For tests, which hand it a worker
    /// that misbehaves.
    pub fn from_worker(worker: Worker) -> Self {
        Self {
            worker,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// How many requests have been sent to the worker. For tests.
    pub fn requests_sent(&self) -> u64 {
        self.worker.requests_sent()
    }

    /// A solid primitive (see [`Primitive`] for where each sits). Values that can't make one
    /// are refused here, naming the parameter, and never reach the worker.
    pub fn make(&mut self, primitive: Primitive) -> Result<ShapeData, Error> {
        if let Err(invalid) = primitive.check() {
            return Err(
                Error::new(ErrorCode::InvalidInput, invalid.message).with_suggestion(format!(
                    "give the {} a valid {}",
                    primitive.name(),
                    invalid.parameter
                )),
            );
        }
        self.shape(Op::MakePrimitive(primitive))
    }

    /// Rounds `edges` of `shape` (numbered from 1, as FreeCAD's `Edge1`, ...) with a
    /// constant `radius`.
    pub fn fillet(
        &mut self,
        shape: &ShapeData,
        edges: &[u32],
        radius: f64,
    ) -> Result<ShapeData, Error> {
        self.shape(Op::Fillet {
            shape: shape.clone(),
            edges: edges.to_vec(),
            radius,
        })
    }

    /// The shape's volume, area, center, topology counts, and validity.
    pub fn summarize(&mut self, shape: &ShapeData) -> Result<ShapeSummary, Error> {
        let op = Op::Summarize {
            shape: shape.clone(),
        };
        let name = op.name();
        match self.call(op)? {
            Reply::Summary(summary) => Ok(summary),
            other => Err(self.unexpected(name, &other)),
        }
    }

    /// A triangle mesh of `shape` for display, no further than `deflection` from its
    /// surface. The mesh is checked before it's returned, so it can be indexed safely.
    pub fn tessellate(&mut self, shape: &ShapeData, deflection: f64) -> Result<Mesh, Error> {
        let op = Op::Tessellate {
            shape: shape.clone(),
            deflection,
        };
        let name = op.name();
        match self.call(op)? {
            Reply::Mesh(mesh) => match mesh.validate() {
                Ok(()) => Ok(mesh),
                // A worker that sends a mesh that doesn't hold together can't be trusted:
                // stopped, as for a reply of the wrong kind.
                Err(problem) => {
                    self.worker.shut_down();
                    Err(Error::new(
                        ErrorCode::ProtocolError,
                        "the kernel worker sent an inconsistent mesh",
                    )
                    .with_cause(problem)
                    .with_suggestion("start a new kernel worker"))
                }
            },
            other => Err(self.unexpected(name, &other)),
        }
    }

    fn shape(&mut self, op: Op) -> Result<ShapeData, Error> {
        let name = op.name();
        match self.call(op)? {
            Reply::Shape(shape) => Ok(shape),
            other => Err(self.unexpected(name, &other)),
        }
    }

    /// A reply of the wrong kind: the worker is broken, so it's stopped, as a worker that
    /// replies out of turn is.
    fn unexpected(&mut self, op: &str, reply: &Reply) -> Error {
        self.worker.shut_down();
        Error::new(
            ErrorCode::ProtocolError,
            format!("the kernel worker answered {op} with the wrong kind of reply"),
        )
        .with_cause(format!("a {} reply", reply.kind()))
        .with_suggestion("start a new kernel worker")
    }

    fn call(&mut self, op: Op) -> Result<Reply, Error> {
        // One generation until there's a document to recompute (M2).
        self.worker.call(Generation(0), op, self.timeout)
    }
}

/// The kernel worker executable: [`KERNEL_WORKER_ENV`], or `silico-kernel-worker` next to
/// the running executable. Cargo puts test binaries in a `deps` directory below the
/// worker's, so from there it looks one directory up.
fn worker_path() -> Result<PathBuf, Error> {
    if let Some(path) = std::env::var_os(KERNEL_WORKER_ENV) {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe().map_err(|e| {
        Error::new(ErrorCode::Internal, "can't find the running executable")
            .with_cause(e.to_string())
    })?;
    let name = format!("silico-kernel-worker{}", std::env::consts::EXE_SUFFIX);
    let dir = exe.parent().unwrap_or(Path::new("."));
    let dir = if dir.file_name().is_some_and(|n| n == "deps") {
        dir.parent().unwrap_or(dir)
    } else {
        dir
    };
    let worker = dir.join(&name);
    if worker.is_file() {
        Ok(worker)
    } else {
        Err(Error::new(
            ErrorCode::Internal,
            format!("no kernel worker at {}", worker.display()),
        )
        .with_suggestion(format!(
            "build it (cargo build -p silico-kernel --bin silico-kernel-worker), or name it in {KERNEL_WORKER_ENV}"
        )))
    }
}
