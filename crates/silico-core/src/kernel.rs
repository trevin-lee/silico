//! Typed geometry operations on one kernel worker: the core's layer over the protocol.
//! Each operation refuses values that can't work before anything is sent, matches the
//! worker's reply to the operation, and checks what comes back, so nothing above it handles
//! a worker's raw reply. Recompute and the worker pool (M1.6) build on it, and debug tools
//! use it directly; clients reach geometry through the command API (ARCHITECTURE.md,
//! invariant 3) once there is one.

use std::path::{Path, PathBuf};
use std::time::Duration;

pub use silico_protocol::{
    Axis, BooleanOp, BuiltData, ElementCounts, ElementFate, ElementKind, ElementRef, Ends, History,
    InputHistory, Mesh, Primitive, Profile, Reported, ShapeData, ShapeSummary,
};
use silico_protocol::{Generation, Op, Reply};

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
    /// constant `radius`, and says what became of the shape's faces, edges, and vertices.
    pub fn fillet(
        &mut self,
        shape: &ShapeData,
        edges: &[u32],
        radius: f64,
    ) -> Result<BuiltData, Error> {
        let op = Op::Fillet {
            shape: shape.clone(),
            edges: edges.to_vec(),
            radius,
        };
        self.built(op, 1)
    }

    /// `shape` moved by `offset`; each face, edge, and vertex is modified into its moved
    /// self. An offset that isn't finite, or reaches past a kilometre, is refused here and
    /// never reaches the worker.
    pub fn translate(&mut self, shape: &ShapeData, offset: [f64; 3]) -> Result<BuiltData, Error> {
        if let Err(invalid) = silico_protocol::check_offset(offset) {
            return Err(
                Error::new(ErrorCode::InvalidInput, invalid.message).with_suggestion(
                    "move the shape by a finite offset, no more than a kilometre each way",
                ),
            );
        }
        let op = Op::Translate {
            shape: shape.clone(),
            offset,
        };
        self.built(op, 1)
    }

    /// `profile` swept into a solid along its normal by `length`, or along `direction` far
    /// enough to rise `length` along the normal; its history is the profile's, with the
    /// bottom and top faces as its ends. Values that can't make one are refused here,
    /// naming the parameter, and never reach the worker.
    pub fn extrude(
        &mut self,
        profile: &Profile,
        length: f64,
        direction: Option<[f64; 3]>,
    ) -> Result<BuiltData, Error> {
        let checked = profile
            .check()
            .and_then(|plane| silico_protocol::extrusion_vector(&plane, length, direction));
        if let Err(invalid) = checked {
            return Err(refused("extrusion", invalid));
        }
        let op = Op::Extrude {
            profile: profile.clone(),
            length,
            direction,
        };
        self.built(op, 1)
    }

    /// `profile` swept around `axis` (in its plane, with the profile on one side) by
    /// `angle` degrees, more than 0 and at most 360; its history is the profile's. Values
    /// that can't make one are refused here, naming the parameter.
    pub fn revolve(
        &mut self,
        profile: &Profile,
        axis: Axis,
        angle: f64,
    ) -> Result<BuiltData, Error> {
        let checked = profile.check().and_then(|plane| {
            silico_protocol::check_revolve(&plane, &profile.points, &axis, angle)
        });
        if let Err(invalid) = checked {
            return Err(refused("revolve", invalid));
        }
        let op = Op::Revolve {
            profile: profile.clone(),
            axis,
            angle,
        };
        self.built(op, 1)
    }

    /// Fuses `tool` with `base`, cuts it from `base`, or keeps what they share, and says
    /// what became of both: `base` is the history's input 0, `tool` its input 1.
    pub fn boolean(
        &mut self,
        op: BooleanOp,
        base: &ShapeData,
        tool: &ShapeData,
    ) -> Result<BuiltData, Error> {
        let op = Op::Boolean {
            op,
            base: base.clone(),
            tool: tool.clone(),
        };
        self.built(op, 2)
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

    /// An operation's result, its history checked before it's returned: for `inputs`
    /// input shapes, naming only elements the result has.
    fn built(&mut self, op: Op, inputs: usize) -> Result<BuiltData, Error> {
        let name = op.name();
        match self.call(op)? {
            Reply::Built(built) => match built.history.validate(inputs) {
                Ok(()) => Ok(built),
                Err(problem) => {
                    // As for a mesh that doesn't hold together: the worker can't be trusted.
                    self.worker.shut_down();
                    Err(Error::new(
                        ErrorCode::ProtocolError,
                        format!("the kernel worker sent an inconsistent history for {name}"),
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

/// A sweep's values refused before sending, naming the parameter.
fn refused(what: &str, invalid: silico_protocol::InvalidParameter) -> Error {
    let parameter = match invalid.parameter {
        "points" => "profile",
        other => other,
    };
    Error::new(ErrorCode::InvalidInput, invalid.message)
        .with_suggestion(format!("give the {what} a valid {parameter}"))
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
