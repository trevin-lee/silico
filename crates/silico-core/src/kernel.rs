//! Typed geometry operations: the core's layer over the protocol. Each operation is a
//! [`KernelRequest`], which refuses values that can't work before anything is sent, matches
//! the worker's reply to the operation, and checks what comes back, so nothing above it
//! handles a worker's raw reply. A [`KernelWorker`] runs requests on one worker, and a
//! [`KernelPool`](crate::pool::KernelPool) on many; recompute builds on the pool, and debug
//! tools use either directly. Clients reach geometry through the command API
//! (ARCHITECTURE.md, invariant 3) once there is one.

use std::path::{Path, PathBuf};
use std::time::Duration;

pub use silico_protocol::{
    Axis, Blob, BooleanOp, BuiltData, ChamferSize, ElementCounts, ElementFate, ElementKind,
    ElementRef, Ends, FreecadMeasures, History, InputHistory, Mesh, Primitive, Profile, Reported,
    ShapeData, ShapeSummary,
};
use silico_protocol::{DebugOp, Generation, Op, Reply};

use crate::worker::Worker;
use crate::{Error, ErrorCode};

/// The environment variable naming the kernel worker executable, for when it isn't next
/// to the running one.
pub const KERNEL_WORKER_ENV: &str = "SILICO_KERNEL_WORKER";

/// How long an operation may take before its worker is stopped, unless set otherwise.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// One kernel operation, its values checked, and how its reply is read: what a
/// [`KernelWorker`] runs at once, or a [`KernelPool`](crate::pool::KernelPool) runs when one
/// of its workers is free. Values that can't work are refused when the request is made, so
/// they never reach a worker. The reply is matched to the operation and checked (a
/// history's elements, a mesh's indices) before it's returned; a reply that fails that
/// comes from a worker that can't be trusted, which is stopped.
pub struct KernelRequest<T> {
    op: Op,
    read: Box<dyn FnOnce(&'static str, Reply) -> Result<T, Error> + Send>,
    timeout: Option<Duration>,
}

impl<T> KernelRequest<T> {
    /// A request for `op`, whose reply `read` turns into the result. `read` may return only
    /// errors that mean the worker can't be trusted: the runner stops the worker on any.
    fn new(
        op: Op,
        read: impl FnOnce(&'static str, Reply) -> Result<T, Error> + Send + 'static,
    ) -> Self {
        Self {
            op,
            read: Box::new(read),
            timeout: None,
        }
    }

    /// The same request, allowed `timeout` before its worker is stopped, rather than the
    /// runner's default ([`DEFAULT_TIMEOUT`]).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// The operation's name, for logs and errors.
    pub fn name(&self) -> &'static str {
        self.op.name()
    }

    /// Sends the request to `worker` for `generation` and reads the reply, stopping the
    /// worker if the reader finds the reply untrustworthy (every error a reader returns
    /// means that). The worker itself stops on a crash, a hang, or a reply out of turn.
    pub(crate) fn run_on(self, worker: &mut Worker, generation: Generation) -> Result<T, Error> {
        let name = self.op.name();
        let reply = worker.call(generation, self.op, self.timeout.unwrap_or(DEFAULT_TIMEOUT))?;
        (self.read)(name, reply).inspect_err(|_| worker.shut_down())
    }
}

impl KernelRequest<ShapeData> {
    /// A solid primitive (see [`Primitive`] for where each sits). Values that can't make one
    /// are refused, naming the parameter.
    pub fn make(primitive: Primitive) -> Result<Self, Error> {
        if let Err(invalid) = primitive.check() {
            return Err(
                Error::new(ErrorCode::InvalidInput, invalid.message).with_suggestion(format!(
                    "give the {} a valid {}",
                    primitive.name(),
                    invalid.parameter
                )),
            );
        }
        Ok(Self::new(Op::MakePrimitive(primitive), read_shape))
    }
}

impl KernelRequest<BuiltData> {
    /// Rounds `edges` of `shape` (numbered from 1, as FreeCAD's `Edge1`, ...) with a
    /// constant `radius`, and says what became of the shape's faces, edges, and vertices. A
    /// radius that can't round an edge is refused, naming it.
    pub fn fillet(shape: &ShapeData, edges: &[u32], radius: f64) -> Result<Self, Error> {
        if let Err(invalid) = silico_protocol::check_fillet_radius(radius) {
            return Err(refused("fillet", invalid));
        }
        let op = Op::Fillet {
            shape: shape.clone(),
            edges: edges.to_vec(),
            radius,
        };
        Ok(Self::new(op, read_built(1)))
    }

    /// Cuts `edges` of `shape` (numbered from 1, as FreeCAD's `Edge1`, ...) back by `size`,
    /// measured from each edge's lower-numbered face, or its higher-numbered one when `flip`
    /// is set, and says what became of the shape's faces, edges, and vertices. A size that
    /// can't make a chamfer is refused, naming the parameter.
    pub fn chamfer(
        shape: &ShapeData,
        edges: &[u32],
        size: ChamferSize,
        flip: bool,
    ) -> Result<Self, Error> {
        if let Err(invalid) = size.check() {
            return Err(refused("chamfer", invalid));
        }
        let op = Op::Chamfer {
            shape: shape.clone(),
            edges: edges.to_vec(),
            size,
            flip,
        };
        Ok(Self::new(op, read_built(1)))
    }

    /// `shape` moved by `offset`; each face, edge, and vertex is modified into its moved
    /// self. An offset that isn't finite, or reaches past a kilometre, is refused.
    pub fn translate(shape: &ShapeData, offset: [f64; 3]) -> Result<Self, Error> {
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
        Ok(Self::new(op, read_built(1)))
    }

    /// `profile` swept into a solid along its normal by `length`, or along `direction` far
    /// enough to rise `length` along the normal; its history is the profile's, with the
    /// bottom and top faces as its ends. Values that can't make one are refused, naming the
    /// parameter.
    pub fn extrude(
        profile: &Profile,
        length: f64,
        direction: Option<[f64; 3]>,
    ) -> Result<Self, Error> {
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
        Ok(Self::new(op, read_built(1)))
    }

    /// `profile` swept around `axis` (in its plane, with the profile on one side) by
    /// `angle` degrees, more than 0 and at most 360; its history is the profile's. Values
    /// that can't make one are refused, naming the parameter.
    pub fn revolve(profile: &Profile, axis: Axis, angle: f64) -> Result<Self, Error> {
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
        Ok(Self::new(op, read_built(1)))
    }

    /// Fuses `tool` with `base`, cuts it from `base`, or keeps what they share, and says
    /// what became of both: `base` is the history's input 0, `tool` its input 1.
    pub fn boolean(op: BooleanOp, base: &ShapeData, tool: &ShapeData) -> Self {
        let op = Op::Boolean {
            op,
            base: base.clone(),
            tool: tool.clone(),
        };
        Self::new(op, read_built(2))
    }
}

impl KernelRequest<ShapeSummary> {
    /// The shape's volume, area, center, topology counts, and validity.
    pub fn summarize(shape: &ShapeData) -> Self {
        let op = Op::Summarize {
            shape: shape.clone(),
        };
        Self::new(op, |name, reply| match reply {
            Reply::Summary(summary) => Ok(summary),
            other => Err(unexpected(name, &other)),
        })
    }
}

impl KernelRequest<FreecadMeasures> {
    /// The shape measured as FreeCAD measures it, for comparing with FreeCAD like for like
    /// ([`FreecadMeasures`]).
    pub fn measure_as_freecad(shape: &ShapeData) -> Self {
        let op = Op::MeasureAsFreecad {
            shape: shape.clone(),
        };
        Self::new(op, |name, reply| match reply {
            Reply::FreecadMeasures(measures) => Ok(measures),
            other => Err(unexpected(name, &other)),
        })
    }
}

impl KernelRequest<Mesh> {
    /// A triangle mesh of `shape` for display, no further than `deflection` from its
    /// surface. The mesh is checked before it's returned, so it can be indexed safely.
    pub fn tessellate(shape: &ShapeData, deflection: f64) -> Self {
        let op = Op::Tessellate {
            shape: shape.clone(),
            deflection,
        };
        Self::new(op, |name, reply| match reply {
            Reply::Mesh(mesh) => match mesh.validate() {
                Ok(()) => Ok(mesh),
                // A worker that sends a mesh that doesn't hold together can't be trusted, as
                // one that replies with the wrong kind can't (`run_on` stops it).
                Err(problem) => Err(Error::new(
                    ErrorCode::ProtocolError,
                    "the kernel worker sent an inconsistent mesh",
                )
                .with_cause(problem)
                .with_suggestion("start a new kernel worker")),
            },
            other => Err(unexpected(name, &other)),
        })
    }
}

impl KernelRequest<Blob> {
    /// `data`, sent to the worker and back: a health check, and the cheapest round trip.
    pub fn echo(data: Blob) -> Self {
        Self::new(Op::Echo { data }, |name, reply| match reply {
            Reply::Echo { data } => Ok(data),
            other => Err(unexpected(name, &other)),
        })
    }
}

impl KernelRequest<()> {
    /// A test operation ([`DebugOp`]), which only the test worker answers (with
    /// [`Reply::Done`], if it answers at all). For tests of how workers fail.
    pub fn debug(op: DebugOp) -> Self {
        Self::new(Op::Debug(op), |name, reply| match reply {
            Reply::Done => Ok(()),
            other => Err(unexpected(name, &other)),
        })
    }
}

fn read_shape(name: &'static str, reply: Reply) -> Result<ShapeData, Error> {
    match reply {
        Reply::Shape(shape) => Ok(shape),
        other => Err(unexpected(name, &other)),
    }
}

/// Reads an operation's result, its history checked: for `inputs` input shapes, naming
/// only elements the result has.
fn read_built(inputs: usize) -> impl FnOnce(&'static str, Reply) -> Result<BuiltData, Error> {
    move |name, reply| match reply {
        Reply::Built(built) => match built.history.validate(inputs) {
            Ok(()) => Ok(built),
            // As for a mesh that doesn't hold together: the worker can't be trusted (and
            // `run_on` stops it).
            Err(problem) => Err(Error::new(
                ErrorCode::ProtocolError,
                format!("the kernel worker sent an inconsistent history for {name}"),
            )
            .with_cause(problem)
            .with_suggestion("start a new kernel worker")),
        },
        other => Err(unexpected(name, &other)),
    }
}

/// A reply of the wrong kind: the worker is broken (and `run_on` stops it, as a worker that
/// replies out of turn is stopped).
fn unexpected(op: &str, reply: &Reply) -> Error {
    Error::new(
        ErrorCode::ProtocolError,
        format!("the kernel worker answered {op} with the wrong kind of reply"),
    )
    .with_cause(format!("a {} reply", reply.kind()))
    .with_suggestion("start a new kernel worker")
}

/// A kernel worker, and the operations it performs. Its typed methods are the one-worker
/// shorthand for [`run`](Self::run) with the matching [`KernelRequest`].
pub struct KernelWorker {
    worker: Worker,
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
        })
    }

    /// Operations on a worker that's already running. For tests, which hand it a worker
    /// that misbehaves.
    pub fn from_worker(worker: Worker) -> Self {
        Self { worker }
    }

    /// How many requests have been sent to the worker. For tests.
    pub fn requests_sent(&self) -> u64 {
        self.worker.requests_sent()
    }

    /// The worker's process ID. For tests and diagnostics.
    pub fn pid(&self) -> u32 {
        self.worker.pid()
    }

    /// Runs `request` on this worker and returns its result.
    pub fn run<T>(&mut self, request: KernelRequest<T>) -> Result<T, Error> {
        // One generation until there's a document to recompute (M2).
        request.run_on(&mut self.worker, Generation(0))
    }

    /// A solid primitive ([`KernelRequest::make`]).
    pub fn make(&mut self, primitive: Primitive) -> Result<ShapeData, Error> {
        self.run(KernelRequest::make(primitive)?)
    }

    /// A fillet ([`KernelRequest::fillet`]).
    pub fn fillet(
        &mut self,
        shape: &ShapeData,
        edges: &[u32],
        radius: f64,
    ) -> Result<BuiltData, Error> {
        self.run(KernelRequest::fillet(shape, edges, radius)?)
    }

    /// A chamfer ([`KernelRequest::chamfer`]).
    pub fn chamfer(
        &mut self,
        shape: &ShapeData,
        edges: &[u32],
        size: ChamferSize,
        flip: bool,
    ) -> Result<BuiltData, Error> {
        self.run(KernelRequest::chamfer(shape, edges, size, flip)?)
    }

    /// A move ([`KernelRequest::translate`]).
    pub fn translate(&mut self, shape: &ShapeData, offset: [f64; 3]) -> Result<BuiltData, Error> {
        self.run(KernelRequest::translate(shape, offset)?)
    }

    /// An extrusion ([`KernelRequest::extrude`]).
    pub fn extrude(
        &mut self,
        profile: &Profile,
        length: f64,
        direction: Option<[f64; 3]>,
    ) -> Result<BuiltData, Error> {
        self.run(KernelRequest::extrude(profile, length, direction)?)
    }

    /// A revolve ([`KernelRequest::revolve`]).
    pub fn revolve(
        &mut self,
        profile: &Profile,
        axis: Axis,
        angle: f64,
    ) -> Result<BuiltData, Error> {
        self.run(KernelRequest::revolve(profile, axis, angle)?)
    }

    /// A boolean ([`KernelRequest::boolean`]).
    pub fn boolean(
        &mut self,
        op: BooleanOp,
        base: &ShapeData,
        tool: &ShapeData,
    ) -> Result<BuiltData, Error> {
        self.run(KernelRequest::boolean(op, base, tool))
    }

    /// A shape's summary ([`KernelRequest::summarize`]).
    pub fn summarize(&mut self, shape: &ShapeData) -> Result<ShapeSummary, Error> {
        self.run(KernelRequest::summarize(shape))
    }

    /// A shape measured as FreeCAD measures it ([`KernelRequest::measure_as_freecad`]).
    pub fn measure_as_freecad(&mut self, shape: &ShapeData) -> Result<FreecadMeasures, Error> {
        self.run(KernelRequest::measure_as_freecad(shape))
    }

    /// A shape's mesh ([`KernelRequest::tessellate`]).
    pub fn tessellate(&mut self, shape: &ShapeData, deflection: f64) -> Result<Mesh, Error> {
        self.run(KernelRequest::tessellate(shape, deflection))
    }
}

/// An operation's values refused before sending, naming the parameter.
fn refused(what: &str, invalid: silico_protocol::InvalidParameter) -> Error {
    let parameter = match invalid.parameter {
        "points" => "profile",
        "other" => "other distance",
        other => other,
    };
    Error::new(ErrorCode::InvalidInput, invalid.message)
        .with_suggestion(format!("give the {what} a valid {parameter}"))
}

/// The kernel worker executable: [`KERNEL_WORKER_ENV`], or `silico-kernel-worker` next to
/// the running executable. Cargo puts test binaries in a `deps` directory below the
/// worker's, so from there it looks one directory up.
pub(crate) fn worker_path() -> Result<PathBuf, Error> {
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
