//! Typed geometry operations: the core's layer over the protocol. Each operation is a
//! [`KernelRequest`], which refuses values that can't work before anything is sent, matches
//! the worker's reply to the operation, and checks what comes back, so nothing above it
//! handles a worker's raw reply. A [`KernelWorker`] runs requests on one worker, and a
//! [`KernelPool`](crate::pool::KernelPool) on many; recompute runs on a [`Kernel`] (the pool,
//! or in tests the in-memory fake), and debug tools use a worker or the pool directly. Clients reach geometry through the command API
//! (ARCHITECTURE.md, invariant 3) once there is one.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub use silico_protocol::{
    Axis, Blob, BooleanOp, BuiltData, ChamferSize, ElementCounts, ElementFate, ElementKind,
    ElementRef, Ends, FreecadMeasures, Generation, History, InputHistory, Mesh, Op, Primitive,
    Profile, Reported, ShapeData, ShapeSummary, Topology,
};
use silico_protocol::{DebugOp, Failure, LENGTH_TOLERANCE, Reply};

use crate::pool::Pending;
use crate::worker::Worker;
use crate::{Error, ErrorCode};

/// What runs kernel requests for recompute: the worker pool
/// ([`KernelPool`](crate::pool::KernelPool)), or, in tests, the in-memory fake
/// (`fake::FakeKernel`, with the `fake` feature). Every request is for a document generation;
/// starting a newer one supersedes the older ones' requests, which come to
/// [`Outcome::Superseded`](crate::pool::Outcome::Superseded), never a result.
pub trait Kernel: Send + Sync {
    /// What the kernel is, for keying results kept on disk ([`crate::cache`]): two kernels
    /// with the same identity give the same result for the same request, and a kernel that
    /// might not (another build of the worker) has another identity.
    fn identity(&self) -> &str;

    /// The generation requests are for: the newest [`advance`](Self::advance) started.
    fn generation(&self) -> Generation;

    /// Starts `generation`, superseding every older one. A generation no newer than the
    /// current one changes nothing.
    fn advance(&self, generation: Generation);

    /// Queues `request` for `generation`, to run when the kernel is free. A request for a
    /// generation older than the current one is superseded at once.
    fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T>;
}

/// A kernel shared, by the recomputes of several documents run one after another (the CLI's
/// files): each starts a generation newer than the kernel's, so none supersedes another's
/// finished work. Run at once, the newest supersedes the others, as on one engine.
impl<K: Kernel> Kernel for Arc<K> {
    fn identity(&self) -> &str {
        (**self).identity()
    }

    fn generation(&self) -> Generation {
        (**self).generation()
    }

    fn advance(&self, generation: Generation) {
        (**self).advance(generation);
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        (**self).submit(generation, request)
    }
}

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
    /// What the core decides from a reply it trusts (a mesh's faces OCCT couldn't mesh):
    /// its refusals leave the worker running.
    decide: Box<dyn FnOnce(&'static str, T) -> Result<T, Error> + Send>,
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
            decide: Box::new(|_, value| Ok(value)),
            timeout: None,
        }
    }

    /// The same request, deciding from the reply `read` trusted with `decide`, whose errors
    /// are the core's refusals, not the worker's misbehavior: the worker goes on.
    fn deciding(
        mut self,
        decide: impl FnOnce(&'static str, T) -> Result<T, Error> + Send + 'static,
    ) -> Self {
        self.decide = Box::new(decide);
        self
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

    /// The operation the request sends.
    #[cfg(feature = "fake")]
    pub(crate) fn op(&self) -> &Op {
        &self.op
    }

    /// Reads and decides from `reply`, as from a worker's, for a kernel that isn't a worker
    /// (the fake): the same checks, so a reply it gets wrong is refused as a worker's is.
    #[cfg(feature = "fake")]
    pub(crate) fn answer(self, reply: Result<Reply, Error>) -> Result<T, Error> {
        let name = self.op.name();
        let value = (self.read)(name, reply?)?;
        (self.decide)(name, value)
    }

    /// Sends the request to `worker` for `generation` and reads the reply, stopping the
    /// worker if the reader finds the reply untrustworthy (every error a reader returns
    /// means that), then decides from it. The worker itself stops on a crash, a hang, or a
    /// reply out of turn.
    pub(crate) fn run_on(self, worker: &mut Worker, generation: Generation) -> Result<T, Error> {
        let name = self.op.name();
        let reply = worker.call(generation, self.op, self.timeout.unwrap_or(DEFAULT_TIMEOUT))?;
        let value = (self.read)(name, reply).inspect_err(|_| worker.shut_down())?;
        (self.decide)(name, value)
    }
}

impl KernelRequest<ShapeData> {
    /// A solid primitive (see [`Primitive`] for where each sits). Values that can't make one
    /// are refused, naming the parameter.
    pub fn make(primitive: Primitive) -> Result<Self, Error> {
        if let Err(invalid) = primitive.check() {
            return Err(refused(primitive.name(), None, invalid));
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
            return Err(refused("fillet", None, invalid));
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
            return Err(refused(
                "chamfer",
                Some(silico_protocol::SIZE_FIELD),
                invalid,
            ));
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
            // Its own suggestion, in place of `refused`'s: how far is the question.
            return Err(
                refused("translation", Some(silico_protocol::OFFSET_FIELD), invalid)
                    .with_suggestion(
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
            return Err(refused("extrusion", None, invalid));
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
            return Err(refused("revolve", None, invalid));
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
            Reply::Summary(summary) => match summary_fault(&summary) {
                None => Ok(summary),
                Some(fault) => Err(Error::new(
                    ErrorCode::ProtocolError,
                    format!("the kernel worker answered {name} with a summary no shape has"),
                )
                .with_cause(fault)
                .with_suggestion("start a new kernel worker")),
            },
            other => Err(unexpected(name, &other)),
        })
    }
}

/// What's wrong with `summary`, if it's not one a shape could have: a measure that isn't
/// finite, or a box whose lowest corner is past its highest. (The worker refuses the first
/// itself; the core checks what it's given, as it checks any reply.)
fn summary_fault(summary: &ShapeSummary) -> Option<String> {
    let [x, y, z] = summary.center;
    let mut values = vec![
        ("volume", summary.volume),
        ("area", summary.area),
        ("center x", x),
        ("center y", y),
        ("center z", z),
    ];
    if let Some([low, high]) = summary.bounds {
        for (axis, (l, h)) in ["x", "y", "z"].into_iter().zip(low.into_iter().zip(high)) {
            values.push(("box", l));
            values.push(("box", h));
            if l > h {
                return Some(format!(
                    "its box's lowest {axis}, {l}, is past its highest, {h}"
                ));
            }
        }
    }
    values
        .into_iter()
        .find(|(_, value)| !value.is_finite())
        .map(|(what, value)| format!("its {what} has {value}"))
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
    /// surface. The mesh is checked before it's returned, so it can be indexed safely. A face
    /// OCCT can't mesh on a valid shape has no triangles (`Mesh::face_triangles`).
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
        // Faces OCCT couldn't mesh: left out of a valid shape's mesh (slivers too narrow to
        // show), and the mesh refused for one that isn't valid, naming them (there they say
        // something is broken).
        .deciding(|name, mesh: Mesh| match mesh.refusal() {
            Some(refusal) => Err(crate::worker::failure_error(name, Failure::new(refusal))),
            None => Ok(mesh),
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

/// An operation's values refused before sending, naming the parameter at fault as a dotted
/// name (`InvalidParameter::name`, as the worker names it): `under`, the field the checked
/// value is a part of, if any.
fn refused(what: &str, under: Option<&str>, invalid: silico_protocol::InvalidParameter) -> Error {
    let words = match invalid.parameter {
        "points" => "profile",
        "other" => "other distance",
        parameter => parameter,
    };
    Error::new(ErrorCode::InvalidInput, invalid.message.clone())
        .with_parameter(invalid.name(under))
        .with_suggestion(format!("give the {what} a valid {words}"))
}

/// The kernel worker executable: [`KERNEL_WORKER_ENV`], or `silico-kernel-worker` next to
/// the running executable ([`worker::worker_path`](crate::worker::worker_path)).
pub(crate) fn worker_path() -> Result<PathBuf, Error> {
    crate::worker::worker_path(
        KERNEL_WORKER_ENV,
        "silico-kernel-worker",
        "kernel worker",
        "cargo build -p silico-kernel --bin silico-kernel-worker",
    )
}

/// The faces, edges, vertices, and solids OCCT gives a primitive: a primitive's element map
/// names them, and the fake makes them (checked against OCCT's by silico-kernel's
/// `fake_kernel` and `naming` tests).
pub fn primitive_counts(primitive: &Primitive) -> ElementCounts {
    let (faces, edges, vertices) = match *primitive {
        Primitive::Box { .. } => (6, 12, 8),
        Primitive::Cylinder { .. } => (3, 3, 2),
        Primitive::Sphere { .. } => (1, 3, 2),
        Primitive::Cone {
            radius1,
            radius2,
            height,
        } => match cone_caps(radius1, radius2, height) {
            (true, true) => (3, 3, 2),
            _ => (2, 3, 2),
        },
        Primitive::Torus { .. } => (1, 2, 1),
    };
    ElementCounts {
        faces,
        edges,
        vertices,
        solids: 1,
    }
}

/// The numbers a primitive's elements are named by, of `kind`, by OCCT's number (the
/// first is `Face1`'s): each element numbered as in the primitive's fullest form, whatever
/// its values remove (Trevin's, 2026-10-17), so one number always names one element and an
/// edit that removes an element never gives its number to another. Only a cone's elements
/// depend on its values: always Face1 its side, Face2 its top cap, Face3 its base cap;
/// Edge1 the top circle, Edge2 the seam, Edge3 the base circle; Vertex1 the top circle's,
/// Vertex2 the base circle's. A pointed cone's apex, which its fullest form hasn't, is
/// numbered after them: Edge4 and Vertex3 at the top, Edge5 and Vertex4 at the base.
/// Every other primitive's are OCCT's own numbers.
pub fn primitive_numbers(primitive: &Primitive, kind: ElementKind) -> Vec<u32> {
    let counts = primitive_counts(primitive);
    if let Primitive::Cone {
        radius1,
        radius2,
        height,
    } = *primitive
    {
        let (faces, edges, vertices): (&[u32], &[u32], &[u32]) =
            match cone_caps(radius1, radius2, height) {
                (true, true) => (&[1, 2, 3], &[1, 2, 3], &[1, 2]),
                // To a point at the top: the side, the base cap; the apex's edge, the seam,
                // the base circle; the apex, the base circle's vertex.
                (true, false) => (&[1, 3], &[4, 2, 3], &[3, 2]),
                // To a point at the base: the side, the top cap; the top circle, the seam,
                // the apex's edge; the top circle's vertex, the apex.
                (false, _) => (&[1, 2], &[1, 2, 5], &[1, 4]),
            };
        return match kind {
            ElementKind::Face => faces,
            ElementKind::Edge => edges,
            ElementKind::Vertex => vertices,
            ElementKind::Solid => &[1],
        }
        .to_vec();
    }
    (1..=counts.of(kind)).collect()
}

/// Whether OCCT gives a cone its base cap and its top cap. OCCT leaves out a cap where the
/// cone's meridian meets the axis, within its tolerance (BRepPrim_OneAxis::MeridianOnAxis):
/// at the base, radius1 from it; at the top, the meridian's end, computed as BRepPrim_Cone
/// and ElCLib::LineValue compute it (its direction normalized by gp_Dir2d, no fused
/// multiply-add), with the math OCCT uses, so the edge cases fall as they do in OCCT.
pub(crate) fn cone_caps(radius1: f64, radius2: f64, height: f64) -> (bool, bool) {
    let rise = radius2 - radius1;
    let half_angle = libm::atan(rise / height);
    let (sin, cos) = (libm::sin(half_angle), libm::cos(half_angle));
    let along = sin / (sin * sin + cos * cos).sqrt();
    let length = (height * height + rise * rise).sqrt();
    let top = length * along + radius1;
    (
        radius1.abs() >= LENGTH_TOLERANCE,
        top.abs() >= LENGTH_TOLERANCE,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> ShapeSummary {
        ShapeSummary {
            volume: 6000.0,
            area: 2200.0,
            center: [5.0, 10.0, 15.0],
            faces: 6,
            edges: 12,
            vertices: 8,
            bounds: Some([[0.0; 3], [10.0, 20.0, 30.0]]),
            valid: true,
        }
    }

    /// The summary request's reading of `reply`, as a worker's would be read.
    fn read(reply: ShapeSummary) -> Result<ShapeSummary, Error> {
        let request = KernelRequest::summarize(&ShapeData::checked(Blob::inline(vec![]), true));
        (request.read)("Summarize", Reply::Summary(reply))
    }

    #[test]
    fn a_summary_no_shape_has_is_refused_as_a_protocol_error_saying_why() {
        assert_eq!(read(summary()), Ok(summary()));
        // A flat box, and none, are fine.
        let flat = ShapeSummary {
            bounds: Some([[0.0; 3], [10.0, 0.0, 30.0]]),
            ..summary()
        };
        assert!(read(flat).is_ok());
        assert!(
            read(ShapeSummary {
                bounds: None,
                ..summary()
            })
            .is_ok()
        );
        type Spoil = fn(&mut ShapeSummary);
        let cases: [(Spoil, &str); 10] = [
            (
                |s| s.bounds = Some([[0.0; 3], [10.0, 20.0, f64::INFINITY]]),
                "its box has inf",
            ),
            (|s| s.volume = f64::NAN, "its volume has NaN"),
            (|s| s.area = f64::INFINITY, "its area has inf"),
            (|s| s.center[0] = f64::NAN, "its center x has NaN"),
            (|s| s.center[1] = f64::NEG_INFINITY, "its center y has -inf"),
            (|s| s.center[2] = f64::INFINITY, "its center z has inf"),
            (
                |s| s.bounds = Some([[0.0, 0.0, f64::NAN], [10.0, 20.0, 30.0]]),
                "its box has NaN",
            ),
            (
                |s| s.bounds = Some([[11.0, 0.0, 0.0], [10.0, 20.0, 30.0]]),
                "its box's lowest x, 11, is past its highest, 10",
            ),
            (
                |s| s.bounds = Some([[0.0, 21.0, 0.0], [10.0, 20.0, 30.0]]),
                "its box's lowest y, 21, is past its highest, 20",
            ),
            (
                |s| s.bounds = Some([[0.0, 0.0, 31.0], [10.0, 20.0, 30.0]]),
                "its box's lowest z, 31, is past its highest, 30",
            ),
        ];
        for (spoil, why) in cases {
            let mut bad = summary();
            spoil(&mut bad);
            let error = read(bad).unwrap_err();
            assert_eq!(error.code(), ErrorCode::ProtocolError, "{why}");
            assert_eq!(error.cause(), Some(why));
            assert_eq!(
                error.message(),
                "the kernel worker answered Summarize with a summary no shape has"
            );
        }
    }
}
