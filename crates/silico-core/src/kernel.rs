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
    Axis, Blob, BooleanOp, BuiltData, ChamferSize, Curve, DESCRIBE_BATCH, Description, Distance,
    EdgeCurve, EdgeDescription, ElementCounts, ElementFate, ElementKind, ElementRef, Ends,
    ExportMesh, FaceDescription, FaceRegions, FreecadMeasures, Generation, History, InputHistory,
    Mesh, MeshAt, Op, Primitive, Profile, Region, Reported, ShapeData, ShapeSummary, SketchPlane,
    Surface, SweepKind, Topology,
};
use silico_protocol::{DebugOp, Failure, LENGTH_TOLERANCE, PointPlace, Reply};

use crate::pool::Pending;
use crate::references::EdgeNames;
use crate::worker::Worker;
use crate::{Error, ErrorCode};

/// What runs kernel requests for recompute: the worker pool
/// ([`KernelPool`](crate::pool::KernelPool)), or, in tests, the in-memory fake
/// (`fake::FakeKernel`, with the `fake` feature), or a client of either ([`Clients`]:
/// [`PoolClient`](crate::pool::PoolClient), `fake::FakeClient`). Every request is for a
/// generation of the client that submits it; starting a newer one supersedes that client's
/// older requests, which come to [`Outcome::Superseded`](crate::pool::Outcome::Superseded),
/// never a result, and never another client's. A kernel itself is its own first client,
/// [`ClientId::KERNEL`].
pub trait Kernel: Send + Sync {
    /// What the kernel is, for keying results kept on disk ([`crate::cache`]): two kernels
    /// with the same identity give the same result for the same request, and a kernel that
    /// might not (another build of the worker) has another identity.
    fn identity(&self) -> &str;

    /// The generation this client's requests are for: the newest [`advance`](Self::advance)
    /// started, or, for a client held to the one it began at (a read's,
    /// [`ReadClient`](crate::command::ReadClient)), that one, however far it's advanced.
    fn generation(&self) -> Generation;

    /// Starts `generation` for this client, superseding every older one of its own. A
    /// generation no newer than the client's current one changes nothing.
    fn advance(&self, generation: Generation);

    /// Queues `request` for this client's `generation`, to run when the kernel is free and
    /// it's the client's turn. A request for a generation older than the client's current
    /// one is superseded at once.
    fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T>;
}

/// A kernel shared as one client, by the recomputes of several documents run one after
/// another (the CLI's files): each starts a generation newer than the client's, so none
/// supersedes another's finished work. Run at once on one client, the newest supersedes the
/// others, as on one engine; engines that run at once each take a client of their own
/// ([`Clients::client`]), and then none supersedes another's.
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

/// One client of a kernel: whose requests a generation is of. Each client's generations
/// are its own, so one client's newer generation supersedes only its own older requests,
/// and a kernel serves its clients' waiting requests in turn. [`ClientId::KERNEL`] is the
/// kernel's own; every other is a [`Clients::client`] handle's, numbered from one in the
/// order the kernel gave them, and never given again by that kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClientId(u64);

impl ClientId {
    /// The kernel's own client: what a kernel's own [`Kernel`] implementation submits as.
    pub const KERNEL: ClientId = ClientId(0);

    /// The client numbered `number` by its kernel.
    pub(crate) const fn new(number: u64) -> Self {
        ClientId(number)
    }

    /// Its number: 0 for the kernel's own, then 1, 2, ... in the order the kernel gave them.
    pub const fn number(self) -> u64 {
        self.0
    }
}

/// A kernel that gives handles of its own, each a client with its own generations: a
/// handle's [`advance`](Kernel::advance) supersedes only the requests that handle
/// submitted, never another handle's or the kernel's own, and the kernel serves its
/// clients' waiting requests in turn, so one client's long queue doesn't starve another.
/// Every handle has the same identity, the kernel's, so the results kept on disk are shared
/// between clients. Dropping a handle retires its client: its requests still waiting or held
/// come to [`Outcome::Superseded`](crate::pool::Outcome::Superseded), and those running are
/// stopped as superseded requests are. Only the handles are clients to submit through: the
/// trait gives no [`Kernel`] of its own, so code generic over it can't submit as the kernel's
/// own client by mistake. How sessions use their clients is
/// docs/design/m4.1-command-api.md's (section 3, and decision 33's read lane).
pub trait Clients: Send + Sync {
    /// A handle on the kernel, as a client of its own.
    type Client: Kernel + 'static;

    /// A new client of the kernel, at generation 0. It may outlive the kernel: its requests
    /// then end as the kernel's do once it stops.
    fn client(&self) -> Self::Client;
}

impl<K: Clients> Clients for Arc<K> {
    type Client = K::Client;

    fn client(&self) -> K::Client {
        (**self).client()
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

/// A batch of a shape's elements described ([`KernelRequest::describe`]): how many of its kind
/// the shape has, and those asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct DescribedBatch {
    pub total: u32,
    pub description: Description,
}

impl KernelRequest<DescribedBatch> {
    /// The geometry and measures of the shape's elements of `kind` numbered from `first`
    /// (from 1, in OCCT's order), `count` of them (at most [`DESCRIBE_BATCH`]) or as many as
    /// there are (docs/design/m4.4-describe.md). The batch is checked before it's returned
    /// ([`Description::validate`], and that it holds what was asked for), so a value can be
    /// trusted as its type says.
    ///
    /// Refused if `kind` is solids, which the kernel doesn't describe, or `first` is 0, which
    /// numbers nothing.
    pub fn describe(
        shape: &ShapeData,
        kind: ElementKind,
        first: u32,
        count: u32,
    ) -> Result<Self, Error> {
        if kind == ElementKind::Solid {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                "a description is of faces, edges, or vertices, not solids",
            )
            .with_parameter("kind"));
        }
        if first == 0 {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                "elements are numbered from 1, so a description can't start at 0",
            )
            .with_parameter("first"));
        }
        let count = count.min(DESCRIBE_BATCH);
        let op = Op::Describe {
            shape: shape.clone(),
            kind,
            first,
            count,
        };
        Ok(Self::new(op, move |name, reply| match reply {
            Reply::Described { total, description } => {
                match batch_problem(kind, first, count, total, &description) {
                    None => Ok(DescribedBatch { total, description }),
                    Some(problem) => Err(description_error(problem)),
                }
            }
            other => Err(unexpected(name, &other)),
        }))
    }
}

impl KernelRequest<Distance> {
    /// The least distance between `first`, an element of `shape`, and `second`, one of
    /// `other` (or of `shape` too, if there's no other), as bounded, with a pair of points
    /// where it's met (M4.5, docs/ports/measure.md; [`Op::Distance`]). The reply is checked
    /// before it's returned: its distance and points finite, the distance not negative, and
    /// at least one pair found.
    ///
    /// Refused, naming `first` or `second`, for a solid, which a distance isn't between in
    /// M4.5, or an element numbered 0, which numbers nothing.
    pub fn distance(
        shape: &ShapeData,
        first: ElementRef,
        other: Option<&ShapeData>,
        second: ElementRef,
    ) -> Result<Self, Error> {
        if let Err(refusal) = silico_protocol::check_distance(first, second) {
            return Err(crate::worker::failure_error(
                "Distance",
                Failure::new(refusal),
            ));
        }
        let op = Op::Distance {
            shape: shape.clone(),
            first,
            other: other.cloned(),
            second,
        };
        Ok(Self::new(op, distance_reply))
    }

    /// The least distance from `point` to `element` of `shape`, as bounded, with a pair of
    /// points where it's met, `from` the point ([`Op::PointDistance`]: `distance`'s
    /// `center_to_element`, as FreeCAD's QuickMeasure measures a circle's centre against a
    /// face or an edge). The reply is checked as [`KernelRequest::distance`]'s is, and is the
    /// worker's fault ([`point_distance_fault`]) if it isn't from `point` or reaches past
    /// what any point within the bound could.
    ///
    /// Refused, naming `element` or `point`, for a solid or an element numbered 0, or a point
    /// that isn't finite or is past [`silico_protocol::MAX_POINT_COORDINATE`] of the origin
    /// ([`silico_protocol::check_point_distance`]).
    pub fn point_distance(
        shape: &ShapeData,
        element: ElementRef,
        point: [f64; 3],
    ) -> Result<Self, Error> {
        if let Err(refusal) = silico_protocol::check_point_distance(element, point) {
            return Err(crate::worker::failure_error(
                "PointDistance",
                Failure::new(refusal),
            ));
        }
        let op = Op::PointDistance {
            shape: shape.clone(),
            element,
            point,
        };
        Ok(Self::new(op, move |name, reply| {
            let distance = distance_reply(name, reply)?;
            match point_distance_fault(point, &distance) {
                None => Ok(distance),
                Some(fault) => Err(Error::new(
                    ErrorCode::ProtocolError,
                    format!("the kernel worker answered {name} with a distance the point hasn't"),
                )
                .with_cause(fault)
                .with_suggestion("start a new kernel worker")),
            }
        }))
    }
}

/// [`Reply::Distance`], checked ([`distance_fault`]), as the answer to the request `name`.
fn distance_reply(name: &str, reply: Reply) -> Result<Distance, Error> {
    match reply {
        Reply::Distance(distance) => match distance_fault(&distance) {
            None => Ok(distance),
            Some(fault) => Err(Error::new(
                ErrorCode::ProtocolError,
                format!("the kernel worker answered {name} with a distance no pair has"),
            )
            .with_cause(fault)
            .with_suggestion("start a new kernel worker")),
        },
        other => Err(unexpected(name, &other)),
    }
}

/// What's wrong with `distance`, if it's not one two elements could have: a value or point
/// that isn't finite, a negative distance, or no pair found. (The worker refuses the first
/// itself; the core checks what it's given, as it checks any reply.)
pub(crate) fn distance_fault(distance: &Distance) -> Option<String> {
    let values = std::iter::once(("distance", distance.distance))
        .chain(distance.from.iter().map(|&c| ("first point", c)))
        .chain(distance.to.iter().map(|&c| ("second point", c)));
    for (what, value) in values {
        if !value.is_finite() {
            return Some(format!("its {what} has {value}"));
        }
    }
    if distance.distance < 0.0 {
        return Some(format!("its distance is {}, below 0", distance.distance));
    }
    (distance.solutions == 0).then(|| "it found no pair of points".to_owned())
}

/// What's wrong with `distance`, a reply to a distance from `point` that passes
/// [`distance_fault`], if it's not one the point could have: a pair not from `point` (the
/// worker measures from a vertex made at it), a point on the element past
/// [`silico_protocol::MAX_POINT_COORDINATE`] of the origin along an axis (past any the core
/// sends, and any solid the gate admits), or a distance farther than any two points within
/// that bound are apart.
pub(crate) fn point_distance_fault(point: [f64; 3], distance: &Distance) -> Option<String> {
    let most = silico_protocol::MAX_POINT_COORDINATE;
    if distance.from != point {
        return Some(format!(
            "its pair is from {:?}, not the point asked, {point:?}",
            distance.from
        ));
    }
    if distance.to.iter().any(|c| c.abs() > most) {
        return Some(format!(
            "its point on the element, {:?}, is past {most} mm of the origin",
            distance.to
        ));
    }
    // The diagonal of the cube the bound draws: no two points within it are farther apart.
    let farthest = 2.0 * 3f64.sqrt() * most;
    (distance.distance > farthest).then(|| {
        format!(
            "its distance is {}, farther than any two points within {most} mm of the origin \
             are apart",
            distance.distance
        )
    })
}

/// The error for a description no shape has, `cause` saying why: the worker's fault.
pub(crate) fn description_error(cause: String) -> Error {
    Error::new(
        ErrorCode::ProtocolError,
        "the kernel worker sent a description it can't have",
    )
    .with_cause(cause)
    .with_suggestion("start a new kernel worker")
}

/// The error for a batch saying the shape has `total` elements of `kind` when its summary
/// counts otherwise, if it does: checked before the batch's elements are kept, so a worker
/// can't make the core ask for batch after batch of elements the shape doesn't have.
pub(crate) fn total_fault(summary: &ShapeSummary, kind: ElementKind, total: u32) -> Option<Error> {
    let counted = match kind {
        ElementKind::Face => summary.faces,
        ElementKind::Edge => summary.edges,
        ElementKind::Vertex => summary.vertices,
        ElementKind::Solid => return None,
    };
    (counted != total).then(|| {
        description_error(format!(
            "it says the shape has {total} {}, and its summary counts {counted}",
            kind.plural()
        ))
    })
}

/// What's wrong with `description` as the whole of a shape whose summary is `summary`, if
/// anything: a kind it describes more or fewer of than the summary counts (both are counted
/// in `TopExp::MapShapes` order, of the same shape, so a shape's agree).
pub(crate) fn description_fault(
    summary: &ShapeSummary,
    description: &Description,
) -> Option<String> {
    [
        (ElementKind::Face, summary.faces, description.faces.len()),
        (ElementKind::Edge, summary.edges, description.edges.len()),
        (
            ElementKind::Vertex,
            summary.vertices,
            description.vertices.len(),
        ),
    ]
    .into_iter()
    .find(|(_, counted, described)| u64::from(*counted) != *described as u64)
    .map(|(kind, counted, described)| {
        format!(
            "it describes {described} {}, and its summary counts {counted}",
            kind.plural()
        )
    })
}

/// What's wrong with a batch of a description for a request of `count` elements of `kind`
/// from `first`, of `total`, if anything: a value it can't hold, elements of another kind,
/// or more or fewer than there are from `first` up to `count`.
fn batch_problem(
    kind: ElementKind,
    first: u32,
    count: u32,
    total: u32,
    description: &Description,
) -> Option<String> {
    if let Err(problem) = description.validate(first) {
        return Some(problem);
    }
    let (faces, edges, vertices) = (
        description.faces.len(),
        description.edges.len(),
        description.vertices.len(),
    );
    let (of_kind, others) = match kind {
        ElementKind::Face => (faces, edges + vertices),
        ElementKind::Edge => (edges, faces + vertices),
        _ => (vertices, faces + edges),
    };
    let left = u64::from(total).saturating_sub(u64::from(first) - 1);
    let want = left.min(u64::from(count));
    if others != 0 {
        Some(format!(
            "it describes elements of a kind not asked for ({others} of them)"
        ))
    } else if of_kind as u64 != want {
        Some(format!(
            "it describes {of_kind}, not {want} (from {first}, of {total})"
        ))
    } else {
        None
    }
}

impl KernelRequest<PointPlace> {
    /// Where `point` is against `shape`: inside, on its boundary, or outside, by OCCT's
    /// solid classifier (FreeCAD's `Shape.isInside(point, tolerance, True)`). What counts as
    /// inside is the caller's: M4.6's render counts a camera's eye on a face as inside it.
    pub fn classify(shape: &ShapeData, point: [f64; 3]) -> Self {
        let op = Op::Classify {
            shape: shape.clone(),
            point,
        };
        Self::new(op, |name, reply| match reply {
            Reply::Classified { place } => Ok(place),
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
            at_least_the_floor: false,
        };
        Self::new(op, |name, reply| match reply {
            Reply::Mesh(mesh) => checked_mesh(mesh),
            other => Err(unexpected(name, &other)),
        })
        .deciding(|name, mesh: Mesh| decided_mesh(name, mesh))
    }
}

impl KernelRequest<MeshAt> {
    /// [`tessellate`](KernelRequest::tessellate), at the larger of `deflection` and the
    /// shape's floor (the finest the kernel meshes it), never refused for being too fine;
    /// and the deflection the kernel used, which must be the one asked or a coarser one.
    /// M4.6's renders ask for half a pixel, which a close view takes under the floor.
    pub fn tessellate_at_least_the_floor(shape: &ShapeData, deflection: f64) -> Self {
        let op = Op::Tessellate {
            shape: shape.clone(),
            deflection,
            at_least_the_floor: true,
        };
        Self::new(op, move |name, reply| match reply {
            Reply::MeshAt(made) => checked_mesh_at(name, deflection, made),
            other => Err(unexpected(name, &other)),
        })
        .deciding(|name, made: MeshAt| {
            Ok(MeshAt {
                mesh: decided_mesh(name, made.mesh)?,
                deflection: made.deflection,
            })
        })
    }
}

/// A mesh at the floor from a worker, if it was made no finer than `asked`: coarser only
/// for the floor, so never finer. Its floor's most (`mesh_floor` of the shape's diagonal)
/// isn't checked, as the core hasn't the shape's box.
fn checked_mesh_at(name: &str, asked: f64, made: MeshAt) -> Result<MeshAt, Error> {
    let used = made.deflection;
    if !(used.is_finite() && used >= asked) {
        return Err(Error::new(
            ErrorCode::ProtocolError,
            format!("the kernel worker answered {name} with a deflection finer than asked"),
        )
        .with_cause(format!("asked {asked}, used {used}"))
        .with_suggestion("start a new kernel worker"));
    }
    Ok(MeshAt {
        mesh: checked_mesh(made.mesh)?,
        deflection: used,
    })
}

/// A mesh from a worker, if it holds together: a worker that sends one that doesn't can't be
/// trusted, as one that replies with the wrong kind can't (`run_on` stops it).
fn checked_mesh(mesh: Mesh) -> Result<Mesh, Error> {
    match mesh.validate() {
        Ok(()) => Ok(mesh),
        Err(problem) => Err(Error::new(
            ErrorCode::ProtocolError,
            "the kernel worker sent an inconsistent mesh",
        )
        .with_cause(problem)
        .with_suggestion("start a new kernel worker")),
    }
}

/// Faces OCCT couldn't mesh: left out of a valid shape's mesh (slivers too narrow to show),
/// and the mesh refused for one that isn't valid, naming them (there they say something is
/// broken).
fn decided_mesh(name: &str, mesh: Mesh) -> Result<Mesh, Error> {
    match mesh.refusal() {
        Some(refusal) => Err(crate::worker::failure_error(name, Failure::new(refusal))),
        None => Ok(mesh),
    }
}

impl KernelRequest<ExportMesh> {
    /// `shape`'s solids meshed for export at `tolerance` mm, and its angular rule
    /// (docs/ports/mesh-export.md): each face's triangles and where its nodes lie on the
    /// shape's edges and vertices, for [`crate::export::printable`] to join and check. A
    /// tolerance outside the export's range is refused, naming it.
    pub fn export_mesh(shape: &ShapeData, tolerance: f64) -> Result<Self, Error> {
        Self::export_mesh_forced(shape, tolerance, false)
    }

    /// [`export_mesh`](Self::export_mesh) with no face's deflection raised for its own
    /// tolerance (OCCT's `ForceFaceDeflection`): the mesh [`crate::export::printable`]
    /// compares a raised face's with, asked for only when [`crate::export::needs_unraised`].
    pub fn unraised_export_mesh(shape: &ShapeData, tolerance: f64) -> Result<Self, Error> {
        Self::export_mesh_forced(shape, tolerance, true)
    }

    fn export_mesh_forced(
        shape: &ShapeData,
        tolerance: f64,
        force_face_deflection: bool,
    ) -> Result<Self, Error> {
        if let Err(invalid) = silico_protocol::check_export_tolerance(tolerance) {
            return Err(refused("export", None, invalid));
        }
        let op = Op::ExportMesh {
            shape: shape.clone(),
            tolerance,
            force_face_deflection,
        };
        Ok(Self::new(op, move |name, reply| match reply {
            // A report for another tolerance would hold the export's checks to it: the
            // worker's, not the request's.
            Reply::ExportMesh(mesh) if mesh.tolerance.to_bits() != tolerance.to_bits() => {
                Err(Error::new(
                    ErrorCode::ProtocolError,
                    format!(
                        "the kernel worker sent an export mesh for a tolerance of {} mm, not the \
                         {} mm asked for",
                        silico_protocol::format_number(mesh.tolerance),
                        silico_protocol::format_number(tolerance)
                    ),
                )
                .with_suggestion("start a new kernel worker"))
            }
            Reply::ExportMesh(mesh) if mesh.force_face_deflection != force_face_deflection => {
                Err(Error::new(
                    ErrorCode::ProtocolError,
                    format!(
                        "the kernel worker sent an export mesh {} each face's deflection \
                         raised for its tolerance, not the one asked for",
                        if mesh.force_face_deflection {
                            "without"
                        } else {
                            "with"
                        }
                    ),
                )
                .with_suggestion("start a new kernel worker"))
            }
            Reply::ExportMesh(mesh) => match mesh.validate() {
                Ok(()) => Ok(*mesh),
                // As a display mesh that doesn't hold together: the worker can't be trusted.
                Err(problem) => Err(Error::new(
                    ErrorCode::ProtocolError,
                    "the kernel worker sent an inconsistent export mesh",
                )
                .with_cause(problem)
                .with_suggestion("start a new kernel worker")),
            },
            other => Err(unexpected(name, &other)),
        }))
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

    /// Every one of a shape's faces, edges, and vertices described, a batch
    /// ([`KernelRequest::describe`]) of at most [`DESCRIBE_BATCH`] after another, planned from
    /// the shape's summary, asked first ([`silico_protocol::batches`]), and joined, each
    /// batch's total checked against the summary's count, and the whole against the summary.
    pub fn describe(&mut self, shape: &ShapeData) -> Result<Description, Error> {
        let summary = self.summarize(shape)?;
        let mut description = Description {
            faces: Vec::new(),
            edges: Vec::new(),
            vertices: Vec::new(),
        };
        for (kind, first, count) in
            silico_protocol::batches(summary.faces, summary.edges, summary.vertices)
        {
            let batch = self.run(KernelRequest::describe(shape, kind, first, count)?)?;
            if let Some(fault) = total_fault(&summary, kind, batch.total) {
                return Err(fault);
            }
            description.faces.extend(batch.description.faces);
            description.edges.extend(batch.description.edges);
            description.vertices.extend(batch.description.vertices);
        }
        match description_fault(&summary, &description) {
            Some(fault) => Err(description_error(fault)),
            None => Ok(description),
        }
    }

    /// A shape's mesh ([`KernelRequest::tessellate`]).
    pub fn tessellate(&mut self, shape: &ShapeData, deflection: f64) -> Result<Mesh, Error> {
        self.run(KernelRequest::tessellate(shape, deflection))
    }

    /// [`KernelRequest::export_mesh`], run on this worker.
    pub fn export_mesh(&mut self, shape: &ShapeData, tolerance: f64) -> Result<ExportMesh, Error> {
        self.run(KernelRequest::export_mesh(shape, tolerance)?)
    }

    /// [`KernelRequest::unraised_export_mesh`], run on this worker.
    pub fn unraised_export_mesh(
        &mut self,
        shape: &ShapeData,
        tolerance: f64,
    ) -> Result<ExportMesh, Error> {
        self.run(KernelRequest::unraised_export_mesh(shape, tolerance)?)
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

    /// A mesh that holds together: one triangle, one face.
    fn triangle() -> Mesh {
        Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            triangles: vec![[0, 1, 2]],
            face_triangles: vec![1],
            edge_points: vec![],
            edge_point_counts: vec![],
            // Validity is sent only with a face left unmeshed.
            valid: None,
        }
    }

    #[test]
    fn a_mesh_at_the_floor_finer_than_asked_or_the_wrong_reply_is_a_protocol_error() {
        let shape = ShapeData::unchecked(Blob::inline(vec![1]));
        let answer = |reply: Reply| {
            KernelRequest::tessellate_at_least_the_floor(&shape, 0.25).answer(Ok(reply))
        };
        let at = |deflection| {
            Reply::MeshAt(MeshAt {
                mesh: triangle(),
                deflection,
            })
        };
        // As asked, or coarser (the floor): taken as said.
        for used in [0.25, 0.25f64.next_up(), 3.0] {
            let made = answer(at(used)).unwrap();
            assert_eq!((made.mesh, made.deflection), (triangle(), used));
        }
        for (reply, cause) in [
            (
                at(0.25f64.next_down()),
                Some("asked 0.25, used 0.24999999999999997"),
            ),
            (at(f64::NAN), Some("asked 0.25, used NaN")),
            (at(f64::INFINITY), Some("asked 0.25, used inf")),
            (at(-1.0), Some("asked 0.25, used -1")),
            (Reply::Mesh(triangle()), None),
        ] {
            let e = answer(reply).unwrap_err();
            assert_eq!(e.code(), ErrorCode::ProtocolError, "{e}");
            if let Some(cause) = cause {
                assert_eq!(e.cause(), Some(cause), "{e}");
            }
        }
        // A mesh that doesn't hold together, at a good deflection.
        let mut broken = triangle();
        broken.triangles[0] = [0, 1, 3];
        let e = answer(Reply::MeshAt(MeshAt {
            mesh: broken,
            deflection: 0.25,
        }))
        .unwrap_err();
        assert_eq!(e.code(), ErrorCode::ProtocolError, "{e}");
    }

    #[test]
    fn a_classification_is_the_kernels_place_and_the_wrong_reply_is_a_protocol_error() {
        let shape = ShapeData::unchecked(Blob::inline(vec![1]));
        let answer = |reply| KernelRequest::classify(&shape, [0.0; 3]).answer(Ok(reply));
        for place in [
            PointPlace::Inside,
            PointPlace::OnBoundary,
            PointPlace::Outside,
        ] {
            assert_eq!(answer(Reply::Classified { place }).unwrap(), place);
        }
        let e = answer(Reply::Mesh(triangle())).unwrap_err();
        assert_eq!(e.code(), ErrorCode::ProtocolError, "{e}");
    }

    #[test]
    fn a_description_batch_that_isnt_what_was_asked_for_is_a_protocol_error() {
        use silico_protocol::{EdgeCurve, EdgeDescription};
        let line = EdgeDescription {
            curve: EdgeCurve::Line,
            length: 1.0,
            start: [0.0; 3],
            end: [1.0, 0.0, 0.0],
            middle: [0.5, 0.0, 0.0],
        };
        let edges = |n: usize| Description {
            faces: Vec::new(),
            edges: vec![line.clone(); n],
            vertices: Vec::new(),
        };
        let shape = ShapeData::unchecked(Blob::inline(vec![1]));
        let answer = |total: u32, description: Description| {
            KernelRequest::describe(&shape, ElementKind::Edge, 3, 2)
                .unwrap()
                .answer(Ok(Reply::Described { total, description }))
        };
        // Edges 3 and 4 of 12; or the last of 3, edge 3 alone.
        assert_eq!(answer(12, edges(2)).unwrap().total, 12);
        assert_eq!(answer(3, edges(1)).unwrap().description, edges(1));
        let mut bent = edges(2);
        bent.edges[1].length = 0.0;
        let mut faced = edges(2);
        faced.vertices.push([0.0; 3]);
        for (total, description, cause) in [
            (12, edges(1), "it describes 1, not 2 (from 3, of 12)"),
            (3, edges(2), "it describes 2, not 1 (from 3, of 3)"),
            (
                12,
                faced,
                "it describes elements of a kind not asked for (1 of them)",
            ),
            // Edges 3 and 4 asked for, the second the kernel's Edge4.
            (12, bent, "the kernel's Edge4's length is 0"),
        ] {
            let e = answer(total, description).unwrap_err();
            assert_eq!(e.code(), ErrorCode::ProtocolError);
            assert_eq!(e.cause(), Some(cause));
        }
        // A request for solids, or from 0, isn't sent.
        for (kind, first, parameter) in [
            (ElementKind::Solid, 1, "kind"),
            (ElementKind::Edge, 0, "first"),
        ] {
            let e = KernelRequest::describe(&shape, kind, first, 2)
                .err()
                .unwrap();
            assert_eq!(
                (e.code(), e.parameter()),
                (ErrorCode::InvalidInput, Some(parameter))
            );
        }
    }

    #[test]
    fn a_batch_of_one_kind_counts_every_element_of_the_other_two() {
        // For each kind asked for, a batch holding what was asked and one of one other kind
        // and two of the third: three not asked for, counted by adding, whichever two they are.
        use silico_protocol::{EdgeCurve, EdgeDescription, FaceDescription, Surface};
        let face = FaceDescription {
            surface: Surface::Plane {
                normal: [0.0, 0.0, 1.0],
            },
            area: 1.0,
            centroid: [0.5, 0.5, 0.0],
        };
        let edge = EdgeDescription {
            curve: EdgeCurve::Line,
            length: 1.0,
            start: [0.0; 3],
            end: [1.0, 0.0, 0.0],
            middle: [0.5, 0.0, 0.0],
        };
        let vertex = [0.0; 3];
        for (kind, faces, edges, vertices) in [
            (ElementKind::Face, 1, 1, 2),
            (ElementKind::Edge, 2, 1, 1),
            (ElementKind::Vertex, 1, 2, 1),
        ] {
            let description = Description {
                faces: vec![face.clone(); faces],
                edges: vec![edge.clone(); edges],
                vertices: vec![vertex; vertices],
            };
            assert_eq!(
                batch_problem(kind, 1, 1, 1, &description).as_deref(),
                Some("it describes elements of a kind not asked for (3 of them)"),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn an_export_mesh_for_another_tolerance_than_asked_is_a_protocol_error() {
        // The council's robustness lens: the export's checks are held to the tolerance in
        // the report, so a report for 10 mm taken for a 0.1 mm export would hold them to
        // 10. One for another tolerance than the request's, by its bits, is the worker's
        // fault.
        let mesh = |tolerance: f64| silico_protocol::ExportMesh {
            tolerance,
            deflection: tolerance,
            nodes: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            face_nodes: vec![3],
            triangles: vec![[0, 1, 2]],
            face_triangles: vec![1],
            face_numbers: vec![1],
            face_deflections: vec![0.0],
            face_shells: vec![0],
            face_tolerances: vec![1e-7],
            shell_solids: vec![0],
            shell_outer: vec![true],
            solids: 1,
            ..Default::default()
        };
        let shape = ShapeData::checked(Blob::inline(vec![]), true);
        let asked = |tolerance: f64| {
            KernelRequest::export_mesh(&shape, 0.1)
                .unwrap()
                .answer(Ok(Reply::ExportMesh(Box::new(mesh(tolerance)))))
        };
        let error = asked(10.0).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ProtocolError, "{error}");
        assert!(error.message().contains("10 mm, not the 0.1 mm"), "{error}");
        // The next float up is another tolerance too.
        assert!(asked(0.1f64.next_up()).is_err());
        assert_eq!(asked(0.1).unwrap().tolerance, 0.1);
    }

    #[test]
    fn an_export_mesh_raised_or_not_other_than_asked_is_a_protocol_error() {
        // The unraised mesh is what a raised face is compared with: a raised one in its
        // place would always compare the same, and the other way round always differ.
        let mesh = |forced: bool| silico_protocol::ExportMesh {
            tolerance: 0.1,
            deflection: 0.1,
            force_face_deflection: forced,
            shell_solids: vec![0],
            shell_outer: vec![true],
            solids: 1,
            ..Default::default()
        };
        let shape = ShapeData::checked(Blob::inline(vec![]), true);
        for (asked_forced, sent_forced) in [(false, true), (true, false)] {
            let request = if asked_forced {
                KernelRequest::unraised_export_mesh(&shape, 0.1)
            } else {
                KernelRequest::export_mesh(&shape, 0.1)
            };
            let error = request
                .unwrap()
                .answer(Ok(Reply::ExportMesh(Box::new(mesh(sent_forced)))))
                .unwrap_err();
            assert_eq!(error.code(), ErrorCode::ProtocolError, "{error}");
            assert!(error.message().contains("not the one asked for"), "{error}");
        }
        for forced in [false, true] {
            let request = if forced {
                KernelRequest::unraised_export_mesh(&shape, 0.1)
            } else {
                KernelRequest::export_mesh(&shape, 0.1)
            };
            let got = request
                .unwrap()
                .answer(Ok(Reply::ExportMesh(Box::new(mesh(forced)))))
                .unwrap();
            assert_eq!(got.force_face_deflection, forced);
        }
    }

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

    #[test]
    fn a_batchs_total_that_isnt_its_summarys_count_is_a_workers_fault_naming_both() {
        // A box's summary counts 6 faces, 12 edges, and 8 vertices, by hand: a batch's total
        // of each kind one off either way, or far past it, is a protocol error naming both;
        // the summary's own count fits; and solids aren't described, so have no total.
        let summary = ShapeSummary {
            volume: 1.0,
            area: 6.0,
            center: [0.5; 3],
            faces: 6,
            edges: 12,
            vertices: 8,
            bounds: Some([[0.0; 3], [1.0; 3]]),
            valid: true,
        };
        for (kind, counted) in [
            (ElementKind::Face, 6),
            (ElementKind::Edge, 12),
            (ElementKind::Vertex, 8),
        ] {
            assert_eq!(total_fault(&summary, kind, counted), None, "{kind:?}");
        }
        for (kind, total, said) in [
            (
                ElementKind::Face,
                5,
                "it says the shape has 5 faces, and its summary counts 6",
            ),
            (
                ElementKind::Face,
                7,
                "it says the shape has 7 faces, and its summary counts 6",
            ),
            (
                ElementKind::Edge,
                11,
                "it says the shape has 11 edges, and its summary counts 12",
            ),
            (
                ElementKind::Edge,
                13,
                "it says the shape has 13 edges, and its summary counts 12",
            ),
            (
                ElementKind::Vertex,
                0,
                "it says the shape has 0 vertices, and its summary counts 8",
            ),
            (
                ElementKind::Vertex,
                u32::MAX,
                "it says the shape has 4294967295 vertices, and its summary counts 8",
            ),
        ] {
            let error = total_fault(&summary, kind, total).unwrap();
            assert_eq!(error.code(), ErrorCode::ProtocolError, "{said}");
            assert_eq!(error.cause(), Some(said));
            assert_eq!(
                error.message(),
                "the kernel worker sent a description it can't have"
            );
            assert_eq!(error.suggestion(), Some("start a new kernel worker"));
        }
        assert_eq!(total_fault(&summary, ElementKind::Solid, 99), None);
    }

    #[test]
    fn a_description_of_more_or_fewer_of_a_kind_than_its_summary_counts_is_at_fault() {
        // A box's summary counts 6 faces, 12 edges, and 8 vertices, by hand; a description
        // of each kind one short or one long is named for that kind, and one of 6, 12, and
        // 8 fits.
        let summary = ShapeSummary {
            volume: 1.0,
            area: 6.0,
            center: [0.5; 3],
            faces: 6,
            edges: 12,
            vertices: 8,
            bounds: Some([[0.0; 3], [1.0; 3]]),
            valid: true,
        };
        let of = |faces: usize, edges: usize, vertices: usize| Description {
            faces: vec![
                FaceDescription {
                    surface: silico_protocol::Surface::Other,
                    area: 1.0,
                    centroid: [0.0; 3],
                };
                faces
            ],
            edges: vec![
                EdgeDescription {
                    curve: EdgeCurve::Other,
                    length: 1.0,
                    start: [0.0; 3],
                    end: [1.0, 0.0, 0.0],
                    middle: [0.5, 0.0, 0.0],
                };
                edges
            ],
            vertices: vec![[0.0; 3]; vertices],
        };
        assert_eq!(description_fault(&summary, &of(6, 12, 8)), None);
        for (description, said) in [
            (
                of(5, 12, 8),
                "it describes 5 faces, and its summary counts 6",
            ),
            (
                of(7, 12, 8),
                "it describes 7 faces, and its summary counts 6",
            ),
            (
                of(6, 11, 8),
                "it describes 11 edges, and its summary counts 12",
            ),
            (
                of(6, 13, 8),
                "it describes 13 edges, and its summary counts 12",
            ),
            (
                of(6, 12, 7),
                "it describes 7 vertices, and its summary counts 8",
            ),
            (
                of(6, 12, 9),
                "it describes 9 vertices, and its summary counts 8",
            ),
            (
                of(0, 0, 0),
                "it describes 0 faces, and its summary counts 6",
            ),
        ] {
            assert_eq!(
                description_fault(&summary, &description).as_deref(),
                Some(said)
            );
        }
    }
}
