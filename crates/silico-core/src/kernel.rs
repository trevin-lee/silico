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
    Axis, Blob, BooleanOp, BuiltData, ChamferSize, Curve, Description, EdgeCurve, EdgeDescription,
    ElementCounts, ElementFate, ElementKind, ElementRef, Ends, FaceDescription, FaceRegions,
    FreecadMeasures, Generation, History, InputHistory, Mesh, Op, Primitive, Profile, Region,
    Reported, ShapeData, ShapeSummary, SketchPlane, Surface, SweepKind, Topology,
};
use silico_protocol::{DebugOp, Failure, LENGTH_TOLERANCE, Reply};

use crate::pool::Pending;
use crate::references::EdgeNames;
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
    /// The document's names for the edges a fillet or chamfer lists, for its errors.
    names: Option<EdgeNames>,
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
            names: None,
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

    /// The same request, its errors naming the edges it lists as `names` says (a
    /// document's fillet or chamfer, whose edges are names) rather than by their numbers.
    /// A fake kernel's errors are its own, and name none.
    #[must_use]
    pub(crate) fn with_edge_names(mut self, names: EdgeNames) -> Self {
        self.names = Some(names);
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
        let reply = worker.call_with_edge_names(
            generation,
            self.op,
            self.timeout.unwrap_or(DEFAULT_TIMEOUT),
            self.names.as_ref(),
        )?;
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

    /// A face for each of `regions`' regions, on its sketch's plane: each curve an edge and
    /// each point a vertex the curves meeting there share; one face, or a compound of them
    /// in the regions' order (docs/design/m3.5-profile.md). The history has the regions as
    /// its one input: each curve kept as one edge, and each point as one vertex, in the
    /// order sent ([`FaceRegions`]). Regions that can't be sent are refused, saying why.
    pub fn make_faces(regions: &FaceRegions) -> Result<Self, Error> {
        if let Err(invalid) = regions.check() {
            return Err(faces_refused(invalid));
        }
        let shape = FacesShape::of(regions);
        let read = read_built(1);
        Ok(Self::new(
            Op::MakeFaces(regions.clone()),
            move |name, reply| {
                let built = read(name, reply)?;
                match shape.fault(&built.history) {
                    None => Ok(built),
                    Some(problem) => Err(Error::new(
                        ErrorCode::ProtocolError,
                        format!(
                            "the kernel worker sent faces that don't fit the regions for {name}"
                        ),
                    )
                    .with_cause(problem)
                    .with_suggestion("start a new kernel worker")),
                }
            },
        ))
    }

    /// `faces` (a profile's, as [`make_faces`](Self::make_faces) makes them) moved by `start`,
    /// then swept along `vector` into a solid for each face (docs/design/m3.6-sketch-features.md,
    /// S3): the core works out both from the document, and the worker builds exactly them.
    /// The history has the faces as its one input: each face's, edge's, and vertex's start
    /// counterpart its one `modified` element (never kept), its end `generated` of its own
    /// kind, and its sides `generated` ([`History::sweep_fault`]); a reply that breaks that is
    /// the worker's fault. Values a prism can't take are refused, naming the parameter.
    pub fn prism(faces: &ShapeData, start: [f64; 3], vector: [f64; 3]) -> Result<Self, Error> {
        if let Err(invalid) = silico_protocol::check_prism(start, vector) {
            return Err(refused("prism", None, invalid));
        }
        let op = Op::Prism {
            faces: faces.clone(),
            start,
            vector,
        };
        Ok(Self::new(op, read_sweep(SweepKind::Prism)))
    }

    /// `faces` swept from where they are along `direction` (a unit vector) through all of
    /// `base`, which only sizes the prism: [`silico_protocol::THROUGH_ALL_FACTOR`] times the
    /// diagonal of the tight box around both, refused past a kilometre. The history is the
    /// faces' alone, as [`prism`](Self::prism)'s.
    pub fn prism_through_all(
        faces: &ShapeData,
        base: &ShapeData,
        direction: [f64; 3],
    ) -> Result<Self, Error> {
        if let Err(invalid) = silico_protocol::check_through_all(direction) {
            return Err(refused("through-all prism", None, invalid));
        }
        let op = Op::PrismThroughAll {
            faces: faces.clone(),
            base: base.clone(),
            direction,
        };
        Ok(Self::new(op, read_sweep(SweepKind::Prism)))
    }

    /// `faces` turned `start` radians about `axis` (its direction a unit vector), then swept
    /// `angle` radians about it, right-handed: a solid for each face (design S3). The history
    /// is as [`prism`](Self::prism)'s, but for an element on the axis, whose end is its start,
    /// and a whole turn ([`silico_protocol::is_whole_turn`]), which has no ends. Values a
    /// revolution can't take are refused, naming the parameter.
    pub fn revolution(
        faces: &ShapeData,
        axis: Axis,
        start: f64,
        angle: f64,
    ) -> Result<Self, Error> {
        if let Err(invalid) =
            silico_protocol::check_revolution(axis.origin, axis.direction, start, angle)
        {
            return Err(refused("revolution", None, invalid));
        }
        let op = Op::Revolution {
            faces: faces.clone(),
            axis,
            start,
            angle,
        };
        Ok(Self::new(op, read_sweep(SweepKind::of_turn(angle))))
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
pub(crate) fn summary_fault(summary: &ShapeSummary) -> Option<String> {
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

impl KernelRequest<Description> {
    /// Each of the shape's faces, edges, and vertices: its geometry and measures, in OCCT's
    /// order (docs/design/m4.4-describe.md). The description is checked before it's
    /// returned ([`Description::check`]), so a value can be trusted as its type says.
    pub fn describe(shape: &ShapeData) -> Self {
        let op = Op::Describe {
            shape: shape.clone(),
        };
        Self::new(op, |name, reply| match reply {
            Reply::Described(description) => match description.check() {
                Ok(()) => Ok(description),
                Err(problem) => Err(Error::new(
                    ErrorCode::ProtocolError,
                    "the kernel worker sent a description it can't have",
                )
                .with_cause(problem)
                .with_suggestion("start a new kernel worker")),
            },
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

/// Reads a sweep's result: its history checked as any one-input operation's
/// ([`read_built`]), and by the sweep's own rule ([`History::sweep_fault`]).
fn read_sweep(kind: SweepKind) -> impl FnOnce(&'static str, Reply) -> Result<BuiltData, Error> {
    let read = read_built(1);
    move |name, reply| {
        let built = read(name, reply)?;
        match built.history.sweep_fault(kind) {
            None => Ok(built),
            Some(problem) => Err(Error::new(
                ErrorCode::ProtocolError,
                format!("the kernel worker sent a history that breaks the sweep's rule for {name}"),
            )
            .with_cause(problem)
            .with_suggestion("start a new kernel worker")),
        }
    }
}

/// What faces made from regions must be, to be read by curve and point: each region's
/// loops, by their curves' numbers in the order sent (from 1), and how many points and
/// circles there are.
struct FacesShape {
    /// For each region, each loop's curves' numbers, its outer loop first.
    regions: Vec<Vec<std::ops::Range<u32>>>,
    curves: u32,
    points: u32,
    circles: u32,
}

impl FacesShape {
    fn of(regions: &FaceRegions) -> Self {
        let number = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        let mut next = 1u32;
        let shape = regions
            .regions
            .iter()
            .map(|region| {
                region
                    .loops()
                    .map(|curves| {
                        let first = next;
                        next = next.saturating_add(number(curves.len()));
                        first..next
                    })
                    .collect()
            })
            .collect();
        let circles = regions
            .curves()
            .filter(|c| matches!(c, Curve::Circle { .. }))
            .count();
        Self {
            regions: shape,
            curves: next - 1,
            points: number(regions.points().count()),
            circles: number(circles),
        }
    }

    /// What's wrong with `history` (already checked by `History::validate`) for these
    /// regions, if anything: it must keep each curve as an edge of its own and each point
    /// as a vertex of its own, have no other element than those and one vertex for each
    /// circle, no faces and no ends of its input, and its face `n` must be region `n`, its
    /// outer edges the outer loop's curves' and its edges all its loops'.
    fn fault(&self, history: &History) -> Option<String> {
        let input = history.inputs.first()?;
        let counts = history.result;
        if !input.faces.is_empty() {
            return Some(format!(
                "the regions' history has {} faces, but regions have none",
                input.faces.len()
            ));
        }
        let want = (
            self.regions.len() as u64,
            u64::from(self.curves),
            u64::from(self.points) + u64::from(self.circles),
            0,
        );
        let got = (
            u64::from(counts.faces),
            u64::from(counts.edges),
            u64::from(counts.vertices),
            u64::from(counts.solids),
        );
        if got != want {
            return Some(format!(
                "the faces have {got:?} faces, edges, vertices, and solids, but the regions make {want:?}"
            ));
        }
        if !history.ends.start.is_empty() || !history.ends.end.is_empty() {
            return Some("the faces have ends, but they aren't a sweep".to_owned());
        }
        // Each curve and point kept as an element of its own, and nothing else said of it.
        let kept = |fates: &[ElementFate], count: u32, what: &str| -> Result<Vec<u32>, String> {
            if fates.len() != count as usize {
                return Err(format!(
                    "the regions' history has {} {what}s, but {count} were sent",
                    fates.len()
                ));
            }
            let mut seen = std::collections::BTreeSet::new();
            fates
                .iter()
                .enumerate()
                .map(|(n, fate)| match fate.kept {
                    Some(kept)
                        if fate.modified.is_empty()
                            && fate.generated.is_empty()
                            && seen.insert(kept) =>
                    {
                        Ok(kept)
                    }
                    _ => Err(format!(
                        "the regions' {what} {} isn't kept as an element of its own: {fate:?}",
                        n + 1
                    )),
                })
                .collect()
        };
        let edges = match kept(&input.edges, self.curves, "curve")
            .and_then(|edges| kept(&input.vertices, self.points, "point").map(|_| edges))
        {
            Ok(edges) => edges,
            Err(problem) => return Some(problem),
        };
        // Face n is region n: its outer edges its outer loop's, its edges all its loops'.
        let topology = &history.topology;
        for (n, loops) in self.regions.iter().enumerate() {
            let of = |range: &std::ops::Range<u32>| -> Vec<u32> {
                range
                    .clone()
                    .filter_map(|curve| edges.get(curve as usize - 1).copied())
                    .collect()
            };
            let sorted = |mut list: Vec<u32>| {
                list.sort_unstable();
                list
            };
            let outer = loops.first().map(of).map(sorted).unwrap_or_default();
            let all = sorted(loops.iter().flat_map(of).collect());
            let face_outer = topology.outer_edges.get(n).cloned().unwrap_or_default();
            let face_all = sorted(topology.face_edges.get(n).cloned().unwrap_or_default());
            if face_outer != outer || face_all != all {
                return Some(format!(
                    "Face{} isn't region {}: its edges are {face_all:?} (outer {face_outer:?}), the region's {all:?} (outer {outer:?})",
                    n + 1,
                    n + 1
                ));
            }
        }
        None
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

    /// Faces from a profile's regions ([`KernelRequest::make_faces`]).
    pub fn make_faces(&mut self, regions: &FaceRegions) -> Result<BuiltData, Error> {
        self.run(KernelRequest::make_faces(regions)?)
    }

    /// A prism of a profile's faces ([`KernelRequest::prism`]).
    pub fn prism(
        &mut self,
        faces: &ShapeData,
        start: [f64; 3],
        vector: [f64; 3],
    ) -> Result<BuiltData, Error> {
        self.run(KernelRequest::prism(faces, start, vector)?)
    }

    /// A prism of a profile's faces through all of a base
    /// ([`KernelRequest::prism_through_all`]).
    pub fn prism_through_all(
        &mut self,
        faces: &ShapeData,
        base: &ShapeData,
        direction: [f64; 3],
    ) -> Result<BuiltData, Error> {
        self.run(KernelRequest::prism_through_all(faces, base, direction)?)
    }

    /// A revolution of a profile's faces ([`KernelRequest::revolution`]).
    pub fn revolution(
        &mut self,
        faces: &ShapeData,
        axis: Axis,
        start: f64,
        angle: f64,
    ) -> Result<BuiltData, Error> {
        self.run(KernelRequest::revolution(faces, axis, start, angle)?)
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

    /// A shape's elements described ([`KernelRequest::describe`]).
    pub fn describe(&mut self, shape: &ShapeData) -> Result<Description, Error> {
        self.run(KernelRequest::describe(shape))
    }

    /// A shape's mesh ([`KernelRequest::tessellate`]).
    pub fn tessellate(&mut self, shape: &ShapeData, deflection: f64) -> Result<Mesh, Error> {
        self.run(KernelRequest::tessellate(shape, deflection))
    }
}

/// An operation's values refused before sending, naming the parameter at fault as a dotted
/// name (`InvalidParameter::name`, as the worker names it): `under`, the field the checked
/// value is a part of, if any.
pub(crate) fn refused(
    what: &str,
    under: Option<&str>,
    invalid: silico_protocol::InvalidParameter,
) -> Error {
    let words = match invalid.parameter {
        "points" => "profile",
        "other" => "other distance",
        parameter => parameter,
    };
    Error::new(ErrorCode::InvalidInput, invalid.message.clone())
        .with_parameter(invalid.name(under))
        .with_suggestion(format!("give the {what} a valid {words}"))
}

/// Regions the faces request refuses, worded as [`refused`] words any request's values,
/// with the advice that fits: the regions come from a profile the core made, which
/// refuses by name all the request does, so a refusal here is Silico's bug.
pub(crate) fn faces_refused(invalid: silico_protocol::InvalidParameter) -> Error {
    refused("faces", None, invalid)
        .with_suggestion("send the faces of a profile the core made: this is a bug in the profile")
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

    /// A 4 × 3 rectangle on XY with a circle hole, and a circle beside it: two regions,
    /// six curves, four points.
    fn two_regions() -> FaceRegions {
        let p = [[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0]];
        FaceRegions {
            plane: SketchPlane::Xy,
            regions: vec![
                Region {
                    outer: (0..4)
                        .map(|i| Curve::Line {
                            start: p[i],
                            end: p[(i + 1) % 4],
                        })
                        .collect(),
                    holes: vec![vec![Curve::Circle {
                        center: [2.0, 1.5],
                        radius: 0.5,
                    }]],
                },
                Region {
                    outer: vec![Curve::Circle {
                        center: [10.0, 0.0],
                        radius: 1.0,
                    }],
                    holes: vec![],
                },
            ],
        }
    }

    /// The faces the kernel makes of [`two_regions`]: each curve kept as the edge of its
    /// number, each point as its vertex, the circles' seams Vertex5 and Vertex6.
    fn two_faces() -> BuiltData {
        let kept = |n: u32| ElementFate {
            kept: Some(n),
            ..ElementFate::default()
        };
        BuiltData {
            shape: ShapeData::checked(Blob::inline(vec![]), true),
            history: History {
                inputs: vec![InputHistory {
                    faces: vec![],
                    edges: (1..=6).map(kept).collect(),
                    vertices: (1..=4).map(kept).collect(),
                }],
                result: ElementCounts {
                    faces: 2,
                    edges: 6,
                    vertices: 6,
                    solids: 0,
                },
                ends: Ends::default(),
                topology: Topology {
                    face_edges: vec![vec![1, 2, 3, 4, 5], vec![6]],
                    outer_edges: vec![vec![1, 2, 3, 4], vec![6]],
                    edge_vertices: vec![
                        vec![1, 2],
                        vec![2, 3],
                        vec![3, 4],
                        vec![4, 1],
                        vec![5],
                        vec![6],
                    ],
                },
            },
        }
    }

    #[test]
    fn faces_that_dont_fit_their_regions_are_refused_as_a_protocol_error_saying_why() {
        let read = |built: BuiltData| {
            let request = KernelRequest::make_faces(&two_regions()).unwrap();
            (request.read)("MakeFaces", Reply::Built(built))
        };
        assert_eq!(read(two_faces()), Ok(two_faces()));
        // The edges in another order, each still the curve's own, are fine: naming reads
        // the history, not the numbers.
        let mut swapped = two_faces();
        swapped.history.inputs[0].edges.swap(0, 1);
        swapped.history.topology.edge_vertices.swap(0, 1);
        assert!(read(swapped).is_ok());
        type Spoil = fn(&mut History);
        let cases: [(Spoil, &str); 11] = [
            (
                |h| h.inputs[0].edges[1].kept = Some(1),
                "the regions' curve 2 isn't kept as an element of its own",
            ),
            (
                |h| h.inputs[0].vertices[3].kept = None,
                "the regions' point 4 isn't kept as an element of its own",
            ),
            (
                |h| {
                    h.inputs[0].edges[0].generated.push(Reported {
                        element: ElementRef::face(1),
                        position: 1,
                    });
                },
                "the regions' curve 1 isn't kept as an element of its own",
            ),
            (
                |h| {
                    h.inputs[0].edges.pop();
                },
                "the regions' history has 5 curves, but 6 were sent",
            ),
            (
                |h| {
                    h.inputs[0].vertices.pop();
                },
                "the regions' history has 3 points, but 4 were sent",
            ),
            (
                |h| h.inputs[0].faces.push(ElementFate::default()),
                "the regions' history has 1 faces, but regions have none",
            ),
            (
                |h| {
                    h.result.vertices = 5;
                    h.topology.edge_vertices[5] = vec![5];
                },
                "the faces have (2, 6, 5, 0) faces, edges, vertices, and solids, but the regions make (2, 6, 6, 0)",
            ),
            (
                |h| h.ends.start = vec![1],
                "the faces have ends, but they aren't a sweep",
            ),
            // The faces the other way round: Face1 is the circle's.
            (
                |h| {
                    h.topology.face_edges.swap(0, 1);
                    h.topology.outer_edges.swap(0, 1);
                },
                "Face1 isn't region 1: its edges are [6] (outer [6]), the region's [1, 2, 3, 4, 5] (outer [1, 2, 3, 4])",
            ),
            // The hole's edge on the other face: each face's outer wire is right, its
            // edges not.
            (
                |h| {
                    h.topology.face_edges[0] = vec![1, 2, 3, 4];
                    h.topology.face_edges[1] = vec![5, 6];
                },
                "Face1 isn't region 1: its edges are [1, 2, 3, 4] (outer [1, 2, 3, 4]), the region's [1, 2, 3, 4, 5] (outer [1, 2, 3, 4])",
            ),
            // A face whose outer wire isn't its outer loop's.
            (
                |h| h.topology.outer_edges[0] = vec![1, 2],
                "Face1 isn't region 1",
            ),
        ];
        for (spoil, why) in cases {
            let mut bad = two_faces();
            spoil(&mut bad.history);
            let error = read(bad).unwrap_err();
            assert_eq!(error.code(), ErrorCode::ProtocolError, "{why}");
            assert!(
                error.cause().unwrap_or_default().starts_with(why),
                "{error:?}"
            );
        }
    }

    /// A prism's reply for a triangle's faces (1 face, 3 edges, 3 vertices), by the rule:
    /// side faces 1–3, start face 4, end face 5; start edges 1–3, end edges 4–6, side edges
    /// 7–9; start vertices 1–3, end 4–6.
    fn triangle_prism() -> BuiltData {
        let r = |kind, index| Reported {
            element: ElementRef { kind, index },
            position: 1,
        };
        let fate = |start, generated| ElementFate {
            kept: None,
            modified: vec![start],
            generated,
        };
        use ElementKind::{Edge, Face, Solid, Vertex};
        let mut face_edges = vec![vec![1, 4, 7, 8], vec![2, 5, 8, 9], vec![3, 6, 9, 7]];
        face_edges.extend([vec![1, 2, 3], vec![4, 5, 6]]);
        let mut edge_vertices: Vec<Vec<u32>> =
            (0..3).map(|i| vec![i + 1, (i + 1) % 3 + 1]).collect();
        edge_vertices.extend((0..3).map(|i| vec![i + 4, (i + 1) % 3 + 4]));
        edge_vertices.extend((1..=3).map(|j| vec![j, j + 3]));
        BuiltData {
            shape: ShapeData::checked(Blob::inline(vec![]), true),
            history: History {
                inputs: vec![InputHistory {
                    faces: vec![fate(r(Face, 4), vec![r(Face, 5), r(Solid, 1)])],
                    edges: (1..=3)
                        .map(|i| fate(r(Edge, i), vec![r(Edge, i + 3), r(Face, i)]))
                        .collect(),
                    vertices: (1..=3)
                        .map(|j| fate(r(Vertex, j), vec![r(Vertex, j + 3), r(Edge, j + 6)]))
                        .collect(),
                }],
                result: ElementCounts {
                    faces: 5,
                    edges: 9,
                    vertices: 6,
                    solids: 1,
                },
                ends: Ends::default(),
                topology: Topology {
                    outer_edges: face_edges
                        .iter()
                        .map(|l| {
                            let mut l = l.clone();
                            l.sort_unstable();
                            l
                        })
                        .collect(),
                    face_edges,
                    edge_vertices,
                },
            },
        }
    }

    #[test]
    fn a_sweep_whose_history_breaks_the_rule_is_refused_as_a_protocol_error_saying_why() {
        let faces = ShapeData::checked(Blob::inline(vec![]), true);
        let axis = Axis {
            origin: [0.0; 3],
            direction: [0.0, 1.0, 0.0],
        };
        let requests = || {
            [
                KernelRequest::prism(&faces, [0.0; 3], [0.0, 0.0, 1.0]).unwrap(),
                KernelRequest::prism_through_all(&faces, &faces, [0.0, 0.0, 1.0]).unwrap(),
                KernelRequest::revolution(&faces, axis, 0.0, 1.0).unwrap(),
            ]
        };
        for request in requests() {
            let name = request.name();
            assert_eq!(
                (request.read)(name, Reply::Built(triangle_prism())),
                Ok(triangle_prism())
            );
        }
        type Spoil = fn(&mut History);
        let cases: [(Spoil, &str); 4] = [
            (
                |h| {
                    let start = h.inputs[0].faces[0].modified.remove(0);
                    h.inputs[0].faces[0].kept = Some(start.element.index);
                },
                "Face1 is kept as Face4: a sweep's start is modified, never kept",
            ),
            (
                |h| {
                    h.inputs[0].edges[0].generated.remove(0);
                },
                "Edge1 has 0 end",
            ),
            (
                |h| h.ends.end = vec![5],
                "the sweep reports whole-shape ends",
            ),
            // Inconsistent for any history: read as one first.
            (
                |h| h.inputs[0].vertices[0].modified[0].element.index = 99,
                "input 1 Vertex1 names Vertex99, which the result doesn't have",
            ),
        ];
        for (spoil, why) in cases {
            for request in requests() {
                let mut bad = triangle_prism();
                spoil(&mut bad.history);
                let name = request.name();
                let error = (request.read)(name, Reply::Built(bad)).unwrap_err();
                assert_eq!(error.code(), ErrorCode::ProtocolError, "{why}");
                assert!(
                    error.cause().unwrap_or_default().starts_with(why),
                    "{error:?}"
                );
                assert_eq!(error.suggestion(), Some("start a new kernel worker"));
            }
        }
        // A whole turn's rule is the whole turn's: the triangle's faces end, which a whole
        // turn's never do.
        let whole = KernelRequest::revolution(&faces, axis, 0.0, std::f64::consts::TAU).unwrap();
        let error = (whole.read)("Revolution", Reply::Built(triangle_prism())).unwrap_err();
        assert_eq!(
            error.cause(),
            Some("Face1 has 1 start counterparts in a WholeTurn sweep")
        );
        // The wrong kind of reply, as any request.
        let prism = KernelRequest::prism(&faces, [0.0; 3], [0.0, 0.0, 1.0]).unwrap();
        assert_eq!(
            (prism.read)("Prism", Reply::Done).unwrap_err().code(),
            ErrorCode::ProtocolError
        );
    }

    #[test]
    fn a_documents_plane_is_the_kernels_with_the_normal_d6_gives_it() {
        use crate::document::Plane;
        let cases = [
            (Plane::Xy, [0.0, 0.0, 1.0]),
            (Plane::Xz, [0.0, -1.0, 0.0]),
            (Plane::Yz, [1.0, 0.0, 0.0]),
        ];
        for (plane, normal) in cases {
            let kernel = SketchPlane::from(plane);
            assert_eq!(kernel.normal(), normal, "{plane:?}");
            assert_eq!(format!("{kernel:?}").to_lowercase(), plane.as_str());
        }
    }

    #[test]
    fn regions_that_cant_be_sent_are_refused_naming_the_regions() {
        let mut open = two_regions();
        open.regions[0].outer.pop();
        let error = KernelRequest::make_faces(&open).err().unwrap();
        assert_eq!(error.code(), ErrorCode::InvalidInput);
        assert_eq!(error.parameter(), Some("regions"));
        assert!(error.message().contains("doesn't close"), "{error:?}");
        assert!(error.suggestion().is_some());
    }

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
