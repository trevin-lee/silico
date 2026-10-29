//! A document's sketch solved (docs/design/m3.3-solving.md): translated into the sketch
//! worker's terms, every form to the PlaneGCS rows FreeCAD 1.1.3 makes for it, solved in
//! FreeCAD's order, and read back as data: the solved geometry, or what's wrong, naming the
//! sketch's constraints and geometry by ID.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::{PI, TAU};
use std::path::Path;
use std::time::Duration;

use silico_protocol::{
    Algorithm, DragBegin, Generation, Geometry, Op, Part, Parts, PointOf, PointValues, Reply,
    SketchGeometry, SketchSystem, SolveStatus, Solved, SolverConstraint, Tag, Value,
    arc_from_points, no_distance, squared_distance,
};

pub use silico_protocol::MovedOnce;

use crate::document::{
    Angle as DocumentAngle, Checked, Coordinate, Direction, ElementId, FeatureId, Form, Length,
    LineSide, MemberPaths, Operation, Reference, Sketch, SketchConstraint, TangentSide,
};
use crate::worker::{Executable, Worker, WorkerKind};
use crate::{Error, ErrorCode};

/// The algorithms a sketch is solved with, in FreeCAD's order (`Sketch::internalSolve`).
pub const ALGORITHMS: [Algorithm; 4] = [
    Algorithm::DogLeg,
    Algorithm::LevenbergMarquardt,
    Algorithm::Bfgs,
    Algorithm::AnchoredSqp,
];

/// The least a redundant constraint's gap may be and still agree with the others, in
/// millimetres (decision 5): a hundredth of the file's nanometre, a hundred times the
/// solver's convergence on a position (1e-10 mm), and 86 times the spacing of doubles a
/// kilometre out. A redundant constraint's tolerance is this, [`EXTENT_TOLERANCE`] times
/// the extent of the part of the sketch it's in, or [`TANGENCY_TOLERANCE`] over that part's
/// tightest circle tangency (at least [`LEAST_TANGENCY`]), whichever is most: a kept row's
/// slack reaches only the unknowns of its own part
/// ([`SketchSystem::parts`](silico_protocol::SketchSystem::parts)), which PlaneGCS solves
/// apart from the rest. Only for that: how close geometry may come is OCCT's tolerance,
/// `silico_protocol::LENGTH_TOLERANCE`.
pub const REDUNDANCY_GAP_MM: f64 = 1e-8;

/// How much a redundant constraint's gap may grow per millimetre of the extent of the part
/// of the sketch it's in, the diagonal of the box around the part's points (decision 5):
/// ten times the solver's convergence on a direction (1e-10 rad), or on a position carried
/// along a line's direction, either of which moves a point at most the extent away by
/// 1e-10 × the extent. So two dimensions a nanometre apart conflict on any part of a sketch
/// that fits in a metre, corner to corner.
pub const EXTENT_TOLERANCE: f64 = 1e-9;

/// How much a redundant constraint's gap may grow per inverse millimetre of the tightest
/// circle-to-circle tangency of the part of the sketch it's in, in square millimetres
/// (decision 5). PlaneGCS measures such a tangency as `d² − (r1 ± r2)²`
/// (`ConstraintTangentCircumf::error`), its one row in mm², and a solve holds it within
/// 1e-10 mm², which holds `|d − (r1 ± r2)|` only within 1e-10 / (d + r1 ± r2) mm. Ten times that, the margin [`EXTENT_TOLERANCE`]
/// has. Above [`REDUNDANCY_GAP_MM`] only when d + r1 ± r2 is under 0.1 mm: an inside
/// tangency nearly concentric, or an outside one between tiny circles. At most 1e-4 mm:
/// the tangency counts as [`LEAST_TANGENCY`] at the least.
pub const TANGENCY_TOLERANCE: f64 = 1e-9;

/// The least a part's tightest circle tangency counts as, in millimetres (decision 5): the
/// square root of the 1e-10 mm² a solve holds the tangency's row within. A kept tangency
/// leaves `|d − (r1 ± r2)|` within 1e-10 / T, and within T itself (two circles' gap is at
/// most their centres' distance plus their radii's sum or difference), so within
/// `min(T, 1e-10 / T)`, which is never more than 1e-5. Under 1e-5 the quotient only says
/// where the solver happened to stop on a double root (equal radii inside: the row is d²
/// alone, met anywhere under 1e-5), not how far the circles can be from tangent, which is
/// the same 1e-5 from every start.
pub const LEAST_TANGENCY: f64 = 1e-5;

/// The squared distance between two tangent circles' centres, in mm², under which PlaneGCS
/// measures their tangency as `r1 − r2`, in millimetres (`ConstraintTangentCircumf::error`,
/// FreeCAD #24227, in 1.1.3): a row with no mm² slack, so [`TANGENCY_TOLERANCE`] has no term
/// for it. That form is right only for an inside tangency, whatever side is given; an
/// outside tangency solved with its centres this close is refused
/// (`Translation::concentric_outside`).
const CONCENTRIC_TANGENCY: f64 = 1e-14;

/// How many loads, at most, a solve's answer is checked by: its solution written as the file
/// holds it and solved as a load solves it, until one leaves it where it is
/// ([`Translation::solve`]).
pub const MAX_LOAD_RESOLVES: usize = 3;

/// How long a solve may take before the worker is stopped. A sketch the constraints can't
/// solve is tried by every algorithm in turn, and at the most unknowns the worker takes
/// (1,024, a chain of 512 points held too far apart), break-it measured DogLeg 11.5 s,
/// Levenberg–Marquardt 28.9 s, BFGS 0.2 s, and anchored SQP 33.2 s on a loaded machine:
/// 74 s, past the kernel's 60. The time grows with the equations the diagnosis keeps
/// too: the same chain with one line given `horizontal` 2,000 times took DogLeg 8.7× as
/// long, and passes 300 s with all four (`MAX_SKETCH_UNKNOWNS`'s doc). Every solve that
/// reaches a solution is checked by at least one load ([`Translation::solve`]): at least
/// two solves, and 1 + [`MAX_LOAD_RESOLVES`] at most, each with this limit.
pub const SKETCH_TIMEOUT: Duration = Duration::from_secs(300);

/// The environment variable that names the sketch worker executable.
pub const SKETCH_WORKER_ENV: &str = "SILICO_SKETCH_WORKER";

/// Solves sketches: the sketch worker, or a stand-in in tests.
pub trait SketchSolver: Send {
    /// What the solver found for `system`.
    fn solve(&mut self, system: SketchSystem) -> Result<Solved, Error>;

    /// The last reply didn't answer what was asked, so the solver can't be trusted: a
    /// worker is stopped, and the next solve starts another (a stand-in needn't do
    /// anything).
    fn distrust(&mut self) {}

    /// Readies the solver for a solve, and gives a handle that stops that solve from
    /// another thread (a recompute superseded mid-solve: the solve then fails, and the
    /// next starts a worker again); none if a solve can't be stopped. Once called, every
    /// solve begins with it: a solve runs from one call to the next.
    fn stopper(&mut self) -> Result<Option<SolveStopper>, Error> {
        Ok(None)
    }

    /// Readies the solver for a request of a drag under way, and gives a handle that stops
    /// it, as [`stopper`](Self::stopper) does, but never starts a worker: one that ended
    /// took the drag's session with it, so the request fails as a crash. Every solver that
    /// starts again in `stopper` must override it.
    fn session_stopper(&mut self) -> Result<Option<SolveStopper>, Error> {
        self.stopper()
    }

    /// Starts a drag (M3.7's session in the sketch worker), each request begun as a solve
    /// is, with [`stopper`](Self::stopper): the sketch diagnosed once and its point pinned.
    /// A solver that can't drag refuses.
    fn drag_begin(&mut self, begin: DragBegin) -> Result<DragStarted, Error> {
        let _ = begin;
        Err(no_drags())
    }

    /// Moves the drag's cursor to `to`, solved from where the drag began, under `timeout`.
    fn drag_move(
        &mut self,
        to: [f64; 2],
        timeout: Duration,
    ) -> Result<silico_protocol::Moved, Error> {
        let _ = (to, timeout);
        Err(no_drags())
    }

    /// Ends the drag.
    fn drag_end(&mut self) -> Result<(), Error> {
        Err(no_drags())
    }

    /// One move of `begin`'s point to `to`, begun as a solve is, with
    /// [`stopper`](Self::stopper): a sketch edit's (an extend's), held to a solve's limits
    /// and its time, not a drag's (`Op::MoveOnce`). A solver that can't refuses.
    fn move_once(&mut self, begin: DragBegin, to: [f64; 2]) -> Result<MovedOnce, Error> {
        let _ = (begin, to);
        Err(no_drags())
    }
}

/// A drag's start, as the worker found it: the degrees of freedom left, and the
/// constraints in conflict, by tag (none, or no drag was started).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragStarted {
    pub dofs: i32,
    pub conflicting: Vec<Tag>,
}

/// A solver that can't drag refusing a drag's request.
fn no_drags() -> Error {
    Error::new(
        ErrorCode::Internal,
        "this sketch solver can't drag a sketch",
    )
    .with_suggestion(crate::worker::SILICO_BUG)
}

/// Stops a solve from another thread ([`SketchSolver::stopper`]).
#[derive(Clone)]
pub struct SolveStopper(std::sync::Arc<dyn Fn() + Send + Sync>);

impl SolveStopper {
    pub fn new(stop: impl Fn() + Send + Sync + 'static) -> Self {
        Self(std::sync::Arc::new(stop))
    }

    /// Stops the solve, if it's still running.
    pub fn stop(&self) {
        (self.0)();
    }
}

impl std::fmt::Debug for SolveStopper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SolveStopper")
    }
}

/// The sketch worker, one request at a time. One that crashed, hung past its time, or
/// sent a reply that can't be trusted is stopped, and started again for the next solve.
pub struct SketchWorker {
    worker: Worker,
    /// The executable it was started from, to start again (only while it's still that
    /// build); none for a worker handed in.
    exe: Option<std::sync::Arc<Executable>>,
    timeout: Duration,
    /// Set by the [`SolveStopper`] it gave out for the solve under way: the worker was
    /// killed, perhaps after it answered (no call failed to say so). The solve's later
    /// requests are refused, and the next solve's start starts a worker again.
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Whether solves are begun with [`SketchSolver::stopper`] (as the engine begins every
    /// one): a solve then runs from one `stopper` call to the next, its requests start no
    /// worker (the stopper couldn't reach one), and only a `stopper` call does. A caller that
    /// has called `stopper` once has to begin each solve with it from then on.
    begun: bool,
}

/// The sketch worker executable: [`SKETCH_WORKER_ENV`], or `silico-sketch-worker` beside
/// the running executable ([`worker::worker_path`](crate::worker::worker_path)).
pub fn worker_path() -> Result<std::path::PathBuf, Error> {
    crate::worker::worker_path(
        SKETCH_WORKER_ENV,
        "silico-sketch-worker",
        "sketch worker",
        "cargo build -p silico-sketch --bin silico-sketch-worker",
    )
}

impl SketchWorker {
    /// Starts the sketch worker: [`SKETCH_WORKER_ENV`], or `silico-sketch-worker` next to
    /// the running executable.
    pub fn start() -> Result<Self, Error> {
        Self::start_worker(&worker_path()?)
    }

    /// Starts the sketch worker at `exe`.
    pub fn start_worker(exe: &Path) -> Result<Self, Error> {
        let exe = Executable::of(exe, WorkerKind::Sketch)?;
        let worker = exe.spawn()?;
        Ok(Self {
            exe: Some(exe),
            ..Self::from_worker(worker)
        })
    }

    /// A sketch worker that's already running. For tests, which hand it one that
    /// misbehaves: stopped, it isn't started again (there's nothing to start it from).
    pub fn from_worker(worker: Worker) -> Self {
        Self {
            worker,
            exe: None,
            timeout: SKETCH_TIMEOUT,
            stopped: std::sync::Arc::default(),
            begun: false,
        }
    }

    /// Starts the worker again if it was stopped after a failure, or is gone while idle
    /// (killed from outside), as the kernel pool replaces one: from its executable, if
    /// that's the build it started from (another would answer under this one's identity).
    fn ready(&mut self) -> Result<(), Error> {
        if !(self.worker.is_stopped() || self.worker.has_exited()) {
            return Ok(());
        }
        let Some(exe) = &self.exe else {
            return Ok(());
        };
        self.worker = exe.spawn()?;
        Ok(())
    }

    /// How long each solve may take ([`SKETCH_TIMEOUT`] unless set).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets how long each solve may take from now on.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// How long each solve may take.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// The worker's process.
    pub fn pid(&self) -> u32 {
        self.worker.pid()
    }
}

impl SketchSolver for SketchWorker {
    fn distrust(&mut self) {
        self.worker.shut_down();
    }

    fn stopper(&mut self) -> Result<Option<SolveStopper>, Error> {
        // The last solve's stop may have landed after it answered, killing an idle worker
        // that may not be reaped yet (council, robustness): it's started again here, at the
        // start of a solve, and only here.
        if self.stopped.load(std::sync::atomic::Ordering::SeqCst) {
            self.worker.shut_down();
        }
        self.ready()?;
        let killer = self.worker.killer();
        // A flag of this solve's own, so a stopper given out for an earlier solve marks
        // nothing of this one. It does kill whatever process its worker still is, so the
        // engine fires a stopper only during or just after its own solve (it holds them
        // under its lock, and clears each when its solve ends).
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.stopped = std::sync::Arc::clone(&stopped);
        self.begun = true;
        Ok(Some(SolveStopper::new(move || {
            stopped.store(true, std::sync::atomic::Ordering::SeqCst);
            killer.kill();
        })))
    }

    fn solve(&mut self, system: SketchSystem) -> Result<Solved, Error> {
        self.usable()?;
        match self.call(Op::Solve(system), self.timeout)? {
            Reply::Solved(solved) => Ok(solved),
            other => Err(self.wrong_reply("Solve", &other)),
        }
    }

    fn session_stopper(&mut self) -> Result<Option<SolveStopper>, Error> {
        if self.stopped.load(std::sync::atomic::Ordering::SeqCst)
            || self.worker.is_stopped()
            || self.worker.has_exited()
        {
            return Err(Error::new(
                ErrorCode::WorkerCrashed,
                "the sketch worker ended during the drag, and its session with it",
            )
            .with_suggestion("begin the drag again: the next begin starts another worker"));
        }
        let killer = self.worker.killer();
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.stopped = std::sync::Arc::clone(&stopped);
        self.begun = true;
        Ok(Some(SolveStopper::new(move || {
            stopped.store(true, std::sync::atomic::Ordering::SeqCst);
            killer.kill();
        })))
    }

    fn drag_begin(&mut self, begin: DragBegin) -> Result<DragStarted, Error> {
        self.usable()?;
        match self.call(Op::DragBegin(begin), self.timeout)? {
            Reply::DragStarted { dofs, conflicting } => Ok(DragStarted { dofs, conflicting }),
            other => Err(self.wrong_reply("DragBegin", &other)),
        }
    }

    fn drag_move(
        &mut self,
        to: [f64; 2],
        timeout: Duration,
    ) -> Result<silico_protocol::Moved, Error> {
        self.usable()?;
        match self.call(Op::DragMove { to }, timeout)? {
            Reply::Moved(moved) => Ok(moved),
            other => Err(self.wrong_reply("DragMove", &other)),
        }
    }

    fn move_once(&mut self, begin: DragBegin, to: [f64; 2]) -> Result<MovedOnce, Error> {
        self.usable()?;
        // A solve's time, as it's a sketch edit's one move, not a drag's.
        match self.call(Op::MoveOnce { begin, to }, self.timeout)? {
            Reply::MovedOnce(once) => Ok(once),
            other => Err(self.wrong_reply("MoveOnce", &other)),
        }
    }

    fn drag_end(&mut self) -> Result<(), Error> {
        self.usable()?;
        // A drag's end does no solving: past a move's limit, the worker is hung.
        match self.call(Op::DragEnd, crate::drag::DRAG_MOVE_TIMEOUT)? {
            Reply::Done => Ok(()),
            other => Err(self.wrong_reply("DragEnd", &other)),
        }
    }
}

impl SketchWorker {
    /// Whether a request may run now: a solve's, or a drag's, each begun as a solve is.
    fn usable(&mut self) -> Result<(), Error> {
        // A solve that was stopped stays stopped: its later requests (a re-diagnosis from
        // its solution) aren't run on a new worker (council, robustness, round 2: one ran
        // 125 s after its recompute was superseded, and the newer waited for it).
        if self.stopped.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(Error::new(
                ErrorCode::WorkerCrashed,
                "the sketch solve was stopped",
            ));
        }
        if self.begun {
            // A solve begun with a stopper starts no worker: a new one would be out of the
            // stopper's reach, and run on after a stop (council, robustness, round 3). One
            // gone fails the solve as a crash; the next solve's start replaces it.
            if self.worker.is_stopped() {
                return Err(Error::new(
                    ErrorCode::WorkerCrashed,
                    "the sketch worker ended during the solve",
                )
                .with_suggestion("solve again: the next solve starts another worker"));
            }
            if self.worker.has_exited() {
                // Read what it sent before it ended first: whether it said it started decides
                // the code, as a call to it would (`Worker::crashed`). One that never did was
                // never a worker of this build, whenever the solve comes (break-it).
                self.worker
                    .drain_replies(std::time::Instant::now() + crate::worker::EXIT_GRACE);
                return Err(if self.worker.has_started() {
                    Error::new(
                        ErrorCode::WorkerCrashed,
                        "the sketch worker ended during the solve",
                    )
                    .with_suggestion("solve again: the next solve starts another worker")
                } else {
                    Error::new(
                        ErrorCode::WorkerUnavailable,
                        "the sketch worker ended before it started",
                    )
                    .with_suggestion(
                        "check Silico's install: the sketch worker ends as it starts, so no \
                         solve can run until it's fixed (see its log)",
                    )
                });
            }
        } else {
            self.ready()?;
        }
        Ok(())
    }

    /// `op` run on the worker, under `timeout`.
    fn call(&mut self, op: Op, timeout: Duration) -> Result<Reply, Error> {
        self.worker.call(Generation(0), op, timeout)
    }

    /// A reply of the wrong kind: the worker can't be trusted, and is stopped.
    fn wrong_reply(&mut self, name: &str, reply: &Reply) -> Error {
        self.worker.shut_down();
        Error::new(
            ErrorCode::ProtocolError,
            format!("the sketch worker replied to {name} with {}", reply.kind()),
        )
    }
}

/// Refuses `solved` unless it could be the solution of `sent`: as many values, its fixed
/// ones as sent, every value within reach (one that can't be built may not be), lists of
/// tags sent and unknowns, in order, a residual for each tag sent, and an algorithm exactly
/// where a solution is given ([`Translation::check_reply`]'s, and a drag's closing solve's).
/// A worker that answers otherwise can't be trusted.
pub(crate) fn check_solved(sent: &SketchSystem, solved: &Solved) -> Result<(), Error> {
    let bad = |what: String| {
        Err(Error::new(
            ErrorCode::ProtocolError,
            format!("the sketch worker's solution {what}"),
        ))
    };
    if solved.values.len() != sent.values.len() {
        return bad(format!(
            "has {} values for {} sent",
            solved.values.len(),
            sent.values.len()
        ));
    }
    for (i, (a, b)) in sent.values.iter().zip(&solved.values).enumerate() {
        let unknown = sent.unknowns.binary_search(&Value(i as u32)).is_ok();
        if !unknown && a.to_bits() != b.to_bits() {
            return bad(format!("moves value {i}, which is fixed"));
        }
        // A solution that can't be built may be out of reach: it's named, not used.
        if solved.status != SolveStatus::SolvedInvalid && !silico_protocol::within_reach(*b) {
            return bad(format!("has value {i} out of reach"));
        }
    }
    let mut tags: Vec<Tag> = sent
        .constraints
        .iter()
        .filter_map(SolverConstraint::tag)
        .collect();
    tags.sort_unstable();
    for (name, list) in [
        ("conflicting", &solved.conflicting),
        ("redundant", &solved.redundant),
        ("partially redundant", &solved.partially_redundant),
    ] {
        if list.windows(2).any(|w| w[0] >= w[1]) || list.iter().any(|t| !tags.contains(t)) {
            return bad(format!("lists {name} tags it wasn't sent, or out of order"));
        }
    }
    if solved.free.windows(2).any(|w| w[0] >= w[1])
        || solved
            .free
            .iter()
            .any(|v| sent.unknowns.binary_search(v).is_err())
    {
        return bad("lists free values that aren't unknowns, or out of order".to_owned());
    }
    if solved
        .residuals
        .iter()
        .map(|(t, _)| *t)
        .ne(tags.iter().copied())
    {
        return bad("has residuals for other tags than were sent".to_owned());
    }
    let solution = matches!(
        solved.status,
        SolveStatus::Success | SolveStatus::SolvedExceptRedundant | SolveStatus::SolvedInvalid
    );
    if solution != solved.algorithm.is_some_and(|a| ALGORITHMS.contains(&a)) {
        return bad(format!("is {:?} by {:?}", solved.status, solved.algorithm));
    }
    Ok(())
}

/// What a solved sketch is: where everything is, and what the solve found.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchSolution {
    /// Each point's coordinates, by ID, as the file holds them (M3.4 writes them back).
    pub points: BTreeMap<ElementId, [Coordinate; 2]>,
    /// Each circle's radius, by ID.
    pub radii: BTreeMap<ElementId, Coordinate>,
    /// Each reference dimension's value as the solved geometry measures it, by the
    /// constraint's ID.
    pub references: BTreeMap<ElementId, Measured>,
    /// Degrees of freedom left: 0 is fully constrained.
    pub dofs: i32,
    /// The points, circles, and arcs the constraints don't fix (SolidWorks' under-defined
    /// geometry): any with a value the solver finds free.
    pub free: BTreeSet<ElementId>,
    /// Constraints that repeat others and agree with them (decision 5): kept, and reported,
    /// each with its gap, in millimetres: the least distance a point would have to move for
    /// it to hold exactly. Only those all of whose equations repeat others' (PlaneGCS's
    /// redundant, FreeCAD's "redundant constraints"); removing one leaves the sketch as
    /// constrained as it is.
    pub redundant: BTreeMap<ElementId, f64>,
    /// Constraints some, but not all, of whose equations repeat others' and agree with them
    /// (PlaneGCS's partially redundant, FreeCAD's "partially redundant" constraints), each
    /// with its gap, as [`redundant`](Self::redundant)'s: kept apart from those, since
    /// removing one frees what only it fixes.
    pub partially_redundant: BTreeMap<ElementId, f64>,
    /// The algorithm that solved.
    pub algorithm: Algorithm,
}

impl SketchSolution {
    /// Where point `id` is, in millimetres, if the sketch has it.
    pub fn point(&self, id: &ElementId) -> Option<[f64; 2]> {
        self.points.get(id).map(|[x, y]| [x.mm(), y.mm()])
    }

    /// Circle `id`'s radius, in millimetres, if the sketch has it.
    pub fn radius(&self, id: &ElementId) -> Option<f64> {
        self.radii.get(id).map(|r| r.mm())
    }
}

impl SketchSolution {
    /// `sketch`, whose solution this is, as its file holds it once the solve is written
    /// back (M3.4): its points where the solve put them, its circles' radii, and its
    /// reference dimensions' values as the solve measured them, rounded as the file writes
    /// a value of their kind (a point–line distance's side too). A measure that can't be
    /// written (not finite) keeps the value stored.
    pub fn written(&self, sketch: &Sketch) -> Sketch {
        written(sketch, &self.points, &self.radii, &self.references)
    }
}

/// `sketch` with its points at `points`, its circles' radii at `radii`, and its reference
/// dimensions holding `references`, as its file holds them: what a save writes
/// ([`SketchSolution::written`]) and what a load starts from ([`Translation::solve`]). A
/// measure that can't be written (not finite) keeps the value stored.
fn written(
    sketch: &Sketch,
    points: &BTreeMap<ElementId, [Coordinate; 2]>,
    radii: &BTreeMap<ElementId, Coordinate>,
    references: &BTreeMap<ElementId, Measured>,
) -> Sketch {
    let mut out = sketch.clone();
    for p in &mut out.points {
        if let Some(&[x, y]) = points.get(&p.id) {
            (p.x, p.y) = (x, y);
        }
    }
    for c in &mut out.circles {
        if let Some(&r) = radii.get(&c.id) {
            c.radius = r;
        }
    }
    for c in &mut out.constraints {
        if let Some(&measured) = references.get(&c.id) {
            measure_into(&mut c.form, measured);
        }
    }
    out
}

/// `form`, a reference dimension, holding `measured`.
fn measure_into(form: &mut Form, measured: Measured) {
    let length = |v: f64, value: &mut Length| {
        if let Some(rounded) = Length::rounded(v) {
            *value = rounded;
        }
    };
    match (form, measured) {
        (
            Form::Length { value, .. }
            | Form::Distance { value, .. }
            | Form::DistanceX { value, .. }
            | Form::DistanceY { value, .. }
            | Form::Radius { value, .. }
            | Form::Diameter { value, .. },
            Measured::Length(v),
        ) => length(v, value),
        (
            Form::PointLineDistance { value, side, .. },
            Measured::FromLine {
                distance,
                side: measured,
            },
        ) => {
            length(distance, value);
            *side = measured;
        }
        (Form::Angle { value, .. }, Measured::Angle(degrees)) => {
            if let Some(rounded) = DocumentAngle::rounded(degrees) {
                *value = rounded;
            }
        }
        _ => {}
    }
}

/// One element of a stored sketch that a solve written back changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Moved {
    pub(crate) id: ElementId,
    /// The path of its first field changed, as an error's `parameter` names it
    /// (`points.p0000003.x`, [`MemberPaths`]).
    pub(crate) field: String,
    /// What changed (`p0000003 x 4 → 4.5`).
    pub(crate) said: String,
}

/// What a solve written back changes of `stored`: each point, circle, and constraint whose
/// written form differs, in the file's order.
pub(crate) fn moved(stored: &Sketch, written: &Sketch) -> Vec<Moved> {
    let same = |a: Coordinate, b: Coordinate| a.mm().to_bits() == b.mm().to_bits();
    let paths = MemberPaths::of(stored);
    let mut out = Vec::new();
    // Points, circles, then constraints: ElementKind::ALL's order, less the lists a
    // write-back never moves (lines and arcs hold no values of their own).
    for ((s, w), named) in stored.points.iter().zip(&written.points).zip(&paths.points) {
        let mut what = Vec::new();
        let mut first = None;
        if !same(s.x, w.x) {
            what.push(format!("x {} → {}", s.x.mm(), w.x.mm()));
            first.get_or_insert("x");
        }
        if !same(s.y, w.y) {
            what.push(format!("y {} → {}", s.y.mm(), w.y.mm()));
            first.get_or_insert("y");
        }
        if let Some(key) = first {
            out.push(Moved {
                id: s.id.clone(),
                field: format!("{}.{key}", named.path),
                said: format!("{} {}", s.id, what.join(", ")),
            });
        }
    }
    for ((s, w), named) in stored
        .circles
        .iter()
        .zip(&written.circles)
        .zip(&paths.circles)
    {
        if !same(s.radius, w.radius) {
            out.push(Moved {
                id: s.id.clone(),
                field: format!("{}.radius", named.path),
                said: format!("{} radius {} → {}", s.id, s.radius.mm(), w.radius.mm()),
            });
        }
    }
    for ((s, w), named) in stored
        .constraints
        .iter()
        .zip(&written.constraints)
        .zip(&paths.constraints)
    {
        if s.form != w.form {
            let said = match (s.form.value_text(), w.form.value_text()) {
                (Some(before), Some(after)) => format!("{} value {before} → {after}", s.id),
                _ => format!("{}", s.id),
            };
            // A point's distance from a line measured on its other side, at the same
            // distance, changes its side alone.
            let key = match (&s.form, &w.form) {
                (
                    Form::PointLineDistance {
                        value: a, side: x, ..
                    },
                    Form::PointLineDistance {
                        value: b, side: y, ..
                    },
                ) if a == b && x != y => Some("side"),
                _ if s.form.value_text().is_some() => Some("value"),
                _ => None,
            };
            let field = match key {
                Some(key) => format!("{}.{key}", named.path),
                None => named.path.clone(),
            };
            out.push(Moved {
                id: s.id.clone(),
                field,
                said,
            });
        }
    }
    out
}

/// A reference dimension's value as the solved geometry measures it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Measured {
    /// A length, distance, radius, or diameter, in millimetres; a horizontal or vertical
    /// distance signed.
    Length(f64),
    /// A point's distance from a line, in millimetres, and the side of the line it's on.
    FromLine { distance: f64, side: LineSide },
    /// An angle, in degrees: on the turn of the stored value, then reduced as 1.1.3 reduces
    /// it (`fmod(v, 2π)`, `ba78b194b3`).
    Angle(f64),
}

/// Solves `feature`, a sketch of `document`, with `solver`.
pub fn solve(
    document: &Checked,
    feature: &FeatureId,
    solver: &mut dyn SketchSolver,
) -> Result<SketchSolution, Error> {
    let found = document.feature(feature)?;
    let Operation::Sketch(sketch) = &found.operation else {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!("feature {} isn't a sketch", feature.as_str()),
        )
        .with_feature(feature.as_str()));
    };
    let _span = tracing::info_span!("solve sketch", feature = feature.as_str()).entered();
    let translation = Translation::new(sketch).map_err(|e| e.with_feature(feature.as_str()))?;
    let named = |e: Error| e.with_feature(feature.as_str());
    let answer = translation.solve(solver).map_err(named)?;
    answer.outcome.map_err(named)
}

/// Whether `file`, a sketch with a solve written back, is one the gate takes: a solve can be
/// met only to rounding, and where a value is at its limit the file's measure of it can
/// land past (break-it: an arc of radius 1 km solved to points 1 km and an ulp apart, which
/// the gate, measuring an arc's radius from its points, refuses), so a save of it would
/// write a file that can't be opened again. Refused as an invalid result, naming what the
/// gate names.
///
/// [`Translation::solve`]'s load path runs it on every solve that moves the drawing, on the
/// file a save would write (`SketchSolution::written` and the load's `stored` are one
/// `written()`, the same file but where a reference's measure can't be written, which then
/// keeps a value the gate already took); a solve that doesn't move it leaves the file the
/// gate already took. So a
/// solution [`solve`] returns is always one a save can write (the M3 review's mutants found
/// a second check of it, after the solve, that no input could reach, and it went).
fn holdable(file: &Sketch) -> Result<(), Error> {
    let problems = crate::document::sketch_problems(file);
    let Some(first) = problems.first() else {
        return Ok(());
    };
    let elements: BTreeSet<&str> = problems
        .iter()
        .flat_map(|e| e.elements())
        .map(String::as_str)
        .collect();
    // A curve the constraints draw too small takes the gate's own advice; a value at a
    // limit, rounded past it, keeping inside it.
    let suggestion = match first.suggestion() {
        Some(own) if crate::document::too_small(first) => {
            format!("change the constraints that draw it this small: {own}")
        }
        _ => "keep the sketch's dimensions inside their limits, not at them: a solve meets them only to rounding".to_owned(),
    };
    let mut error = Error::new(
        ErrorCode::InvalidResult,
        format!(
            "the sketch solves to geometry its file can't hold: {}",
            first.message()
        ),
    )
    .with_elements(elements)
    .with_suggestion(suggestion);
    if let Some(parameter) = first.parameter() {
        error = error.with_parameter(parameter);
    }
    Err(error)
}

/// What a solve's diagnosis says of a sketch's members, by ID, for every answer that stands,
/// solved or refused ([`Translation::diagnosis`]; docs/ports/sketcher.md, *Definition
/// states*): PlaneGCS's diagnosis, taken where the solve started, before any algorithm ran,
/// and the constraints decision 5 refuses. What each member's state is, from it, is
/// [`crate::sketch_states`]'s.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Diagnosis {
    /// Degrees of freedom left, as PlaneGCS counts them: negative when constraints
    /// conflict, and −1 for a sketch of no unknowns.
    pub dofs: i32,
    /// The points, circles, and arcs with a value of their own the diagnosis finds free (a
    /// point's x or y, a circle's radius, an arc's radius or either angle): every one, when
    /// no constraint touches an unknown (FreeCAD's reading). A line has no value of its own.
    pub free: BTreeSet<ElementId>,
    /// The constraints in conflict: PlaneGCS's, and the redundant ones that disagree with
    /// the others past decision 5's tolerance, which the read refuses as `over_constrained`
    /// (they're in [`redundant`](Self::redundant) or
    /// [`partially_redundant`](Self::partially_redundant) too, as PlaneGCS lists them).
    pub conflicting: BTreeSet<ElementId>,
    /// The constraints all of whose equations repeat others', as PlaneGCS lists them.
    pub redundant: BTreeSet<ElementId>,
    /// The constraints some, but not all, of whose equations repeat others', as PlaneGCS
    /// lists them.
    pub partially_redundant: BTreeSet<ElementId>,
    /// Each constraint listed above, with the points whose x or y its rows read
    /// ([`SketchSystem::reads`]): none, for one whose rows read only fixed values, radii,
    /// and angles.
    pub points_read: BTreeMap<ElementId, BTreeSet<ElementId>>,
}

/// What solving a [`Translation`] found: the worker's answer the outcome is read from, and
/// the outcome.
#[derive(Debug, Clone)]
pub struct Answer {
    pub solved: Solved,
    pub outcome: Result<SketchSolution, Error>,
}

/// A sketch in the solver's terms, and how to read the solution back.
#[derive(Debug, Clone)]
pub struct Translation {
    /// What's sent.
    pub system: SketchSystem,
    /// The sketch's constraints, sorted by ID: a constraint's tag is its place here, plus
    /// one.
    constraints: Vec<SketchConstraint>,
    points: BTreeMap<ElementId, PointValues>,
    /// Each point's own geometry, which constraints name it by.
    point_geometry: BTreeMap<ElementId, Geometry>,
    lines: BTreeMap<ElementId, Line>,
    circles: BTreeMap<ElementId, Round>,
    arcs: BTreeMap<ElementId, Arc>,
    /// The points the sketch's `on` constraints hold on curves, and those curves.
    on: Vec<(ElementId, Reference)>,
    /// The sketch this translates, as its file holds it: what a solution is written into
    /// for a load to start from ([`Translation::solve`]).
    sketch: Sketch,
}

/// A line, or an axis, as the solver holds it.
#[derive(Debug, Clone, Copy)]
struct Line {
    geometry: Geometry,
    from: PointValues,
    to: PointValues,
}

/// A circle, or an arc's circle.
#[derive(Debug, Clone, Copy)]
struct Round {
    geometry: Geometry,
    center: PointValues,
    radius: Value,
}

/// An arc: its circle, its ends' points, and its angles.
#[derive(Debug, Clone)]
struct Arc {
    round: Round,
    start: ElementId,
    end: ElementId,
    angles: [Value; 2],
}

/// Which end of a line or an arc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Start,
    End,
}

/// A curve a constraint names.
#[derive(Debug, Clone, Copy)]
enum Curve {
    Line(Line),
    Round(Round),
}

/// What two points drawn at one place would leave the solver no line between, for
/// [`Translation::apart`]'s words.
#[derive(Clone, Copy)]
enum Between {
    Symmetry,
    Distance,
    /// A tangency at a point of a circle or an arc.
    TangencyOnRound(RoundKind),
    /// A point held on a circle or an arc.
    OnRound(RoundKind),
}

/// Which of a sketch's round curves, for [`Translation::apart`]'s words.
#[derive(Clone, Copy)]
enum RoundKind {
    Circle,
    Arc,
}

impl RoundKind {
    fn word(self) -> &'static str {
        match self {
            RoundKind::Circle => "circle",
            RoundKind::Arc => "arc",
        }
    }

    /// The word with its article: "a circle", "an arc".
    fn a(self) -> &'static str {
        match self {
            RoundKind::Circle => "a circle",
            RoundKind::Arc => "an arc",
        }
    }
}

impl Curve {
    fn geometry(self) -> Geometry {
        match self {
            Curve::Line(l) => l.geometry,
            Curve::Round(r) => r.geometry,
        }
    }
}

/// The origin's values, fixed, and the axes' far ends.
const ORIGIN: PointValues = PointValues {
    x: Value(0),
    y: Value(1),
};
const X_AXIS_END: PointValues = PointValues {
    x: Value(2),
    y: Value(3),
};
const Y_AXIS_END: PointValues = PointValues {
    x: Value(4),
    y: Value(5),
};
const ORIGIN_GEOMETRY: Geometry = Geometry(0);
const X_AXIS: Line = Line {
    geometry: Geometry(1),
    from: ORIGIN,
    to: X_AXIS_END,
};
const Y_AXIS: Line = Line {
    geometry: Geometry(2),
    from: ORIGIN,
    to: Y_AXIS_END,
};

impl Translation {
    /// `sketch`, which the gate has checked, in the solver's terms
    /// (docs/design/m3.3-solving.md, *The translation*).
    pub fn new(sketch: &Sketch) -> Result<Self, Error> {
        let mut values = vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
        let mut unknowns = Vec::new();
        let mut geometry = vec![
            SketchGeometry::Point(ORIGIN),
            SketchGeometry::Line {
                start: ORIGIN,
                end: X_AXIS_END,
            },
            SketchGeometry::Line {
                start: ORIGIN,
                end: Y_AXIS_END,
            },
        ];
        let mut unknown = |values: &mut Vec<f64>, v: f64| {
            values.push(v);
            let at = Value((values.len() - 1) as u32);
            unknowns.push(at);
            at
        };
        let add = |geometry: &mut Vec<SketchGeometry>, g: SketchGeometry| {
            geometry.push(g);
            Geometry((geometry.len() - 1) as u32)
        };
        let mut sorted_points: Vec<_> = sketch.points.iter().collect();
        sorted_points.sort_by(|a, b| a.id.cmp(&b.id));
        let mut points = BTreeMap::new();
        let mut point_geometry = BTreeMap::new();
        for p in sorted_points {
            let v = PointValues {
                x: unknown(&mut values, p.x.mm()),
                y: unknown(&mut values, p.y.mm()),
            };
            point_geometry.insert(p.id.clone(), add(&mut geometry, SketchGeometry::Point(v)));
            points.insert(p.id.clone(), v);
        }
        let point = |id: &ElementId| {
            points
                .get(id)
                .copied()
                .ok_or_else(|| unchecked(format!("names point {id}, which it doesn't have")))
        };
        let mut lines = BTreeMap::new();
        let mut sorted_lines: Vec<_> = sketch.lines.iter().collect();
        sorted_lines.sort_by(|a, b| a.id.cmp(&b.id));
        for l in sorted_lines {
            let (from, to) = (point(&l.from)?, point(&l.to)?);
            let g = add(
                &mut geometry,
                SketchGeometry::Line {
                    start: from,
                    end: to,
                },
            );
            lines.insert(
                l.id.clone(),
                Line {
                    geometry: g,
                    from,
                    to,
                },
            );
        }
        let mut circles = BTreeMap::new();
        let mut sorted_circles: Vec<_> = sketch.circles.iter().collect();
        sorted_circles.sort_by(|a, b| a.id.cmp(&b.id));
        for c in sorted_circles {
            let center = point(&c.center)?;
            let radius = unknown(&mut values, c.radius.mm());
            let g = add(&mut geometry, SketchGeometry::Circle { center, radius });
            circles.insert(
                c.id.clone(),
                Round {
                    geometry: g,
                    center,
                    radius,
                },
            );
        }
        let mut arcs = BTreeMap::new();
        let mut rules = Vec::new();
        let mut sorted_arcs: Vec<_> = sketch.arcs.iter().collect();
        sorted_arcs.sort_by(|a, b| a.id.cmp(&b.id));
        for a in sorted_arcs {
            let (center, start, end) = (point(&a.center)?, point(&a.start)?, point(&a.end)?);
            let at = |p: PointValues| {
                [
                    values.get(p.x.0 as usize).copied().unwrap_or(f64::NAN),
                    values.get(p.y.0 as usize).copied().unwrap_or(f64::NAN),
                ]
            };
            let Some([r, a0, a1]) = arc_from_points(at(center), at(start), at(end)) else {
                return Err(Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "arc {}'s start and end are at one angle from its centre, so it sweeps nothing, or a whole turn",
                        a.id
                    ),
                )
                .with_element(a.id.as_str())
                .with_suggestion(
                    "move the arc's end round its centre from its start, or draw a circle",
                ));
            };
            let radius = unknown(&mut values, r);
            let angles = [unknown(&mut values, a0), unknown(&mut values, a1)];
            let g = add(
                &mut geometry,
                SketchGeometry::Arc {
                    center,
                    radius,
                    start_angle: angles[0],
                    end_angle: angles[1],
                    start,
                    end,
                },
            );
            rules.push(SolverConstraint::ArcRules { arc: g });
            arcs.insert(
                a.id.clone(),
                Arc {
                    round: Round {
                        geometry: g,
                        center,
                        radius,
                    },
                    start: a.start.clone(),
                    end: a.end.clone(),
                    angles,
                },
            );
        }
        let mut constraints = sketch.constraints.clone();
        constraints.sort_by(|a, b| a.id.cmp(&b.id));
        let on = constraints
            .iter()
            .filter_map(|c| match &c.form {
                Form::On {
                    point: Reference::Element(p),
                    curve,
                } => Some((p.clone(), curve.clone())),
                _ => None,
            })
            .collect();
        let mut translation = Translation {
            system: SketchSystem {
                values,
                unknowns,
                geometry,
                constraints: rules,
                algorithms: ALGORITHMS.to_vec(),
            },
            constraints: Vec::new(),
            points,
            point_geometry,
            lines,
            circles,
            arcs,
            on,
            sketch: sketch.clone(),
        };
        for (i, c) in constraints.iter().enumerate() {
            let tag = Tag(i as u32 + 1);
            let sent = translation
                .constraint(c, tag)
                .map_err(|e| e.with_element(c.id.as_str()))?;
            translation.system.constraints.extend(sent);
        }
        translation.constraints = constraints;
        Ok(translation)
    }

    /// The point a constraint names: points are geometry of their own, so it's `Point(g)`;
    /// the origin is the fixed point every sketch has.
    fn point_ref(&self, r: &Reference) -> Result<PointOf, Error> {
        match r {
            Reference::Origin => Ok(PointOf::Point(ORIGIN_GEOMETRY)),
            Reference::Element(id) => self
                .point_geometry
                .get(id)
                .map(|&g| PointOf::Point(g))
                .ok_or_else(|| unchecked(format!("names {id} as a point"))),
            Reference::XAxis | Reference::YAxis => Err(unchecked(format!("names {r} as a point"))),
        }
    }

    /// A value the solver keeps.
    fn fixed(&mut self, v: f64) -> Value {
        self.system.values.push(v);
        Value((self.system.values.len() - 1) as u32)
    }

    fn line(&self, r: &Reference) -> Result<Line, Error> {
        match r {
            Reference::XAxis => Ok(X_AXIS),
            Reference::YAxis => Ok(Y_AXIS),
            Reference::Element(id) => self
                .lines
                .get(id)
                .copied()
                .ok_or_else(|| unchecked(format!("names {id} as a line"))),
            Reference::Origin => Err(unchecked("names the origin as a line".to_owned())),
        }
    }

    fn round(&self, r: &Reference) -> Result<Round, Error> {
        let id = r
            .element()
            .ok_or_else(|| unchecked(format!("names {r} as a circle or arc")))?;
        self.circles
            .get(id)
            .copied()
            .or_else(|| self.arcs.get(id).map(|a| a.round))
            .ok_or_else(|| unchecked(format!("names {id} as a circle or arc")))
    }

    fn curve(&self, r: &Reference) -> Result<Curve, Error> {
        self.line(r)
            .map(Curve::Line)
            .or_else(|_| self.round(r).map(Curve::Round))
    }

    /// Which end of curve `a` (a line or an arc) the point `at` is.
    fn end_of(&self, a: &Reference, at: &Reference) -> Result<End, Error> {
        let (Some(a), Some(at)) = (a.element(), at.element()) else {
            return Err(unchecked(format!("names {at} as an end of {a}")));
        };
        let (from, to) = match (self.lines.get(a), self.arcs.get(a)) {
            (Some(l), _) => (
                self.points.get(at) == Some(&l.from),
                self.points.get(at) == Some(&l.to),
            ),
            (None, Some(arc)) => (&arc.start == at, &arc.end == at),
            (None, None) => (false, false),
        };
        match (from, to) {
            (true, false) => Ok(End::Start),
            (false, true) => Ok(End::End),
            _ => Err(unchecked(format!("names {at} as an end of {a}"))),
        }
    }

    /// The rows constraint `c`, tagged `tag`, sends: none for a reference dimension.
    fn constraint(
        &mut self,
        c: &SketchConstraint,
        tag: Tag,
    ) -> Result<Option<SolverConstraint>, Error> {
        use SolverConstraint as S;
        let point_of = |r: &Reference| self.point_ref(r);
        let sent = match &c.form {
            Form::Horizontal { line } => S::Horizontal {
                tag,
                line: self.line(line)?.geometry,
            },
            Form::Vertical { line } => S::Vertical {
                tag,
                line: self.line(line)?.geometry,
            },
            Form::HorizontalPoints { a, b } => S::HorizontalPoints {
                tag,
                a: point_of(a)?,
                b: point_of(b)?,
            },
            Form::VerticalPoints { a, b } => S::VerticalPoints {
                tag,
                a: point_of(a)?,
                b: point_of(b)?,
            },
            Form::Length { driving: false, .. }
            | Form::Distance { driving: false, .. }
            | Form::DistanceX { driving: false, .. }
            | Form::DistanceY { driving: false, .. }
            | Form::PointLineDistance { driving: false, .. }
            | Form::Angle { driving: false, .. }
            | Form::Radius { driving: false, .. }
            | Form::Diameter { driving: false, .. } => return Ok(None),
            Form::Length { line, value, .. } => {
                let l = self.line(line)?;
                S::Distance {
                    tag,
                    a: PointOf::Start(l.geometry),
                    b: PointOf::End(l.geometry),
                    value: self.fixed(value.mm()),
                }
            }
            Form::Distance { a, b, value, .. } => S::Distance {
                tag,
                a: point_of(a)?,
                b: point_of(b)?,
                value: self.fixed(value.mm()),
            },
            Form::DistanceX { a, b, value, .. } => S::DistanceX {
                tag,
                a: point_of(a)?,
                b: point_of(b)?,
                value: self.fixed(value.mm()),
            },
            Form::DistanceY { a, b, value, .. } => S::DistanceY {
                tag,
                a: point_of(a)?,
                b: point_of(b)?,
                value: self.fixed(value.mm()),
            },
            Form::PointLineDistance {
                point, line, value, ..
            } => S::PointLineDistance {
                tag,
                point: point_of(point)?,
                line: self.line(line)?.geometry,
                value: self.fixed(value.mm()),
            },
            Form::On { point, curve } => match self.curve(curve)? {
                Curve::Line(l) => S::PointOnLine {
                    tag,
                    point: point_of(point)?,
                    line: l.geometry,
                },
                Curve::Round(r) => S::PointOnCircle {
                    tag,
                    point: point_of(point)?,
                    circle: r.geometry,
                },
            },
            Form::Parallel { a, b } => S::Parallel {
                tag,
                a: self.line(a)?.geometry,
                b: self.line(b)?.geometry,
            },
            Form::Perpendicular { a, b } => S::Perpendicular {
                tag,
                a: self.line(a)?.geometry,
                b: self.line(b)?.geometry,
            },
            Form::Angle { a, b, value, .. } => S::Angle {
                tag,
                a: self.line(a)?.geometry,
                b: self.line(b)?.geometry,
                value: self.fixed(value.degrees().to_radians()),
            },
            Form::Radius { curve, value, .. } => S::Radius {
                tag,
                circle: self.round(curve)?.geometry,
                value: self.fixed(value.mm()),
            },
            // As a radius of half the diameter, exact in binary: FreeCAD's `Proportional`
            // with 0.5 has the same residual, bit for bit.
            Form::Diameter { curve, value, .. } => S::Radius {
                tag,
                circle: self.round(curve)?.geometry,
                value: self.fixed(value.mm() / 2.0),
            },
            Form::Equal { a, b } => match (self.curve(a)?, self.curve(b)?) {
                (Curve::Line(a), Curve::Line(b)) => S::EqualLength {
                    tag,
                    a: a.geometry,
                    b: b.geometry,
                },
                (Curve::Round(a), Curve::Round(b)) => S::EqualRadius {
                    tag,
                    a: a.geometry,
                    b: b.geometry,
                },
                _ => return Err(unchecked("makes a line equal to a circle".to_owned())),
            },
            Form::Tangent { a, b, side } => match (self.curve(a)?, self.curve(b)?, side) {
                (Curve::Line(l), Curve::Round(r), TangentSide::Left | TangentSide::Right)
                | (Curve::Round(r), Curve::Line(l), TangentSide::Left | TangentSide::Right) => {
                    S::TangentLine {
                        tag,
                        line: l.geometry,
                        circle: r.geometry,
                    }
                }
                (Curve::Round(a), Curve::Round(b), TangentSide::Outside | TangentSide::Inside) => {
                    S::TangentCircles {
                        tag,
                        a: a.geometry,
                        b: b.geometry,
                        internal: *side == TangentSide::Inside,
                    }
                }
                _ => {
                    return Err(unchecked(format!(
                        "is a tangency with side {}",
                        side.as_str()
                    )));
                }
            },
            Form::TangentOn {
                a,
                at,
                b,
                direction,
            } => {
                // `a`'s direction leaving `at` against `b`'s own: leaving `a`'s start is its
                // own direction, leaving its end the reverse (docs/design/m3.3-solving.md).
                let end = self.end_of(a, at)?;
                let ga = self.curve(a)?.geometry();
                let angle = match (end, direction) {
                    (End::Start, Direction::Same) | (End::End, Direction::Opposite) => 0.0,
                    (End::Start, Direction::Opposite) | (End::End, Direction::Same) => PI,
                };
                S::TangentOn {
                    tag,
                    a: ga,
                    point: match end {
                        End::Start => PointOf::Start(ga),
                        End::End => PointOf::End(ga),
                    },
                    b: self.curve(b)?.geometry(),
                    angle: self.fixed(angle),
                }
            }
            Form::TangentAt {
                a,
                b,
                at,
                direction,
            } => {
                // A path along `a` into `at` and on along `b`: smooth (`same`) with the two
                // curves' own directions the same there when `at` is one's end and the other's
                // start, opposite when it's both starts or both ends.
                let own_directions_same = match (self.end_of(a, at)?, self.end_of(b, at)?) {
                    (End::End, End::Start) | (End::Start, End::End) => true,
                    (End::Start, End::Start) | (End::End, End::End) => false,
                };
                let angle = match (own_directions_same, direction) {
                    (true, Direction::Same) | (false, Direction::Opposite) => 0.0,
                    (true, Direction::Opposite) | (false, Direction::Same) => PI,
                };
                S::AngleViaPoint {
                    tag,
                    a: self.curve(a)?.geometry(),
                    b: self.curve(b)?.geometry(),
                    point: point_of(at)?,
                    angle: self.fixed(angle),
                }
            }
            Form::Symmetric { a, b, about } => {
                let (pa, pb) = (point_of(a)?, point_of(b)?);
                match about {
                    Reference::Origin => S::SymmetricPoint {
                        tag,
                        a: pa,
                        b: pb,
                        center: PointOf::Point(ORIGIN_GEOMETRY),
                    },
                    Reference::Element(id) if self.points.contains_key(id) => S::SymmetricPoint {
                        tag,
                        a: pa,
                        b: pb,
                        center: point_of(about)?,
                    },
                    _ => {
                        let line = self.line(about)?.geometry;
                        if self.symmetric_arc(a, b, about) {
                            S::MidpointOnLine {
                                tag,
                                a: pa,
                                b: pb,
                                line,
                            }
                        } else {
                            S::SymmetricLine {
                                tag,
                                a: pa,
                                b: pb,
                                line,
                            }
                        }
                    }
                }
            }
            Form::Midpoint { point, line } => {
                let l = self.line(line)?;
                S::SymmetricPoint {
                    tag,
                    a: PointOf::Start(l.geometry),
                    b: PointOf::End(l.geometry),
                    center: point_of(point)?,
                }
            }
        };
        match sent {
            S::SymmetricLine { a, b, .. } | S::SymmetricPoint { a, b, .. } => {
                self.apart(self.at(a), self.at(b), Between::Symmetry)?;
            }
            // Two different points: the gate refuses a distance of a point from itself, and
            // a line's length runs from its start to its end.
            S::Distance { a, b, .. } => {
                self.apart(self.at(a), self.at(b), Between::Distance)?;
            }
            S::TangentOn { point, b, .. } => {
                if let Some((center, round)) = self.center_of(b) {
                    let center = self.xy(&self.system.values, center);
                    self.apart(self.at(point), center, Between::TangencyOnRound(round))?;
                }
            }
            S::PointOnCircle { point, circle, .. } => {
                if let Some((center, round)) = self.center_of(circle) {
                    let center = self.xy(&self.system.values, center);
                    self.apart(self.at(point), center, Between::OnRound(round))?;
                }
            }
            _ => {}
        }
        Ok(Some(sent))
    }

    /// Whether `a` and `b` are one arc's own start and end, and the sketch holds its centre
    /// on `about` (as an end of that line, or by an `on` constraint): FreeCAD's `365af33fc6`,
    /// decided from the references alone (docs/design/m3.3-solving.md).
    fn symmetric_arc(&self, a: &Reference, b: &Reference, about: &Reference) -> bool {
        let (Some(a), Some(b)) = (a.element(), b.element()) else {
            return false;
        };
        let Some(arc) = self
            .arcs
            .values()
            .find(|arc| (&arc.start == a && &arc.end == b) || (&arc.start == b && &arc.end == a))
        else {
            return false;
        };
        let center = arc.round.center;
        let an_end = match about {
            Reference::Element(id) => self
                .lines
                .get(id)
                .is_some_and(|l| l.from == center || l.to == center),
            _ => false,
        };
        an_end
            || self
                .constraints_on(about)
                .any(|point| self.points.get(point) == Some(&center))
    }

    /// The points the sketch's `on` constraints hold on `curve`.
    fn constraints_on<'a>(&'a self, curve: &'a Reference) -> impl Iterator<Item = &'a ElementId> {
        self.on
            .iter()
            .filter_map(move |(p, c)| (c == curve).then_some(p))
    }

    /// That two points aren't drawn at one place where PlaneGCS measures a constraint by the
    /// line between them (as the worker's own check refuses, `SketchSystem::check`): there's
    /// no such line, the solve starts from 0/0, and the constraint is taken to conflict,
    /// though it can hold once the points are apart.
    fn apart(&self, pa: [f64; 2], pb: [f64; 2], between: Between) -> Result<(), Error> {
        if !no_distance(pa, pb) {
            return Ok(());
        }
        let (what, suggestion) = match between {
            Between::Symmetry => (
                "a symmetry of two points drawn at one place".to_owned(),
                "draw the two points apart, where they'd mirror each other".to_owned(),
            ),
            Between::Distance => (
                "a distance between two points drawn at one place".to_owned(),
                "draw the two points apart, about as far apart as the distance says".to_owned(),
            ),
            Between::TangencyOnRound(round) => (
                format!(
                    "a tangency at a point drawn on the {}'s centre",
                    round.word()
                ),
                format!(
                    "draw the point on the {}, away from its centre",
                    round.word()
                ),
            ),
            Between::OnRound(round) => (
                format!("a point on {} drawn at its centre", round.a()),
                format!(
                    "draw the point on the {}, away from its centre",
                    round.word()
                ),
            ),
        };
        Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "{what}, ({}, {}): the solver measures it by the line between them",
                pa[0], pa[1]
            ),
        )
        .with_suggestion(suggestion))
    }

    /// Where `p` is drawn.
    fn at(&self, p: PointOf) -> [f64; 2] {
        self.point_of(&self.system.values, p)
    }

    /// The centre of geometry `g`, if it's a circle or an arc of the sketch, and which.
    fn center_of(&self, g: Geometry) -> Option<(PointValues, RoundKind)> {
        let circle = self.circles.values().find(|c| c.geometry == g);
        let arc = self
            .arcs
            .values()
            .map(|a| &a.round)
            .find(|a| a.geometry == g);
        circle
            .map(|c| (c.center, RoundKind::Circle))
            .or(arc.map(|a| (a.center, RoundKind::Arc)))
    }

    fn xy(&self, values: &[f64], p: PointValues) -> [f64; 2] {
        [
            values.get(p.x.0 as usize).copied().unwrap_or(f64::NAN),
            values.get(p.y.0 as usize).copied().unwrap_or(f64::NAN),
        ]
    }
}

/// A gate the sketch should have passed, and didn't: a bug in Silico.
fn unchecked(what: String) -> Error {
    Error::new(
        ErrorCode::Internal,
        format!("the gate let through a sketch that {what}"),
    )
    .with_suggestion("this is a bug in Silico's sketch gate: report it")
}

impl Translation {
    /// Solves this with `solver`, and reads the answer. A reply that can't be trusted, or a
    /// solver that couldn't answer (a crash, a timeout), is the error; what the answer says
    /// about the sketch is the outcome.
    ///
    /// M3.4: a sketch is solved on every load from the solution its file stores, so the
    /// answer kept must be one its own file gives back. PlaneGCS diagnoses where the sketch
    /// is drawn, as FreeCAD does, so a load, starting from the solution, can drop other
    /// redundant constraints, count other degrees of freedom, call the sketch conflicting,
    /// or move it (an arc's radius and angles, worked out from its stored points, aren't the
    /// solver's own; a kept constraint can fix a value only loosely, through a square root
    /// or an angle along a long side). So every solve that reaches a solution is checked by
    /// loads: its solution written as the file holds it, translated again, and solved, until
    /// a load leaves the file where it was, bit for bit, at most [`MAX_LOAD_RESOLVES`]
    /// times. That load's answer, solved, is a fixed point: its file gives it back (a load
    /// runs this same check, from the same file, and finds it at once). A refusal writes no
    /// file, so its load solves the drawing again and refuses alike; so a refusal on the
    /// drawing stands unless a load reaches a solved fixed point (SolidWorks solves such a
    /// sketch), and a solved answer that loads don't settle on is refused: as a load
    /// refuses it, or, if they keep moving, as a solution that doesn't settle.
    pub fn solve(&self, solver: &mut dyn SketchSolver) -> Result<Answer, Error> {
        let first = self.answer(solver)?;
        // A refusal writes no file, so its load solves the drawing again and refuses alike.
        // Only an over-constrained one is checked by loads (a diagnosis on the drawing): a load
        // from an invalid result would measure sides from where that result put them.
        let checked_by_load = match &first.outcome {
            Ok(_) => true,
            Err(e) => e.code() == ErrorCode::OverConstrained,
        };
        if !reached(&first.solved) || !checked_by_load {
            return Ok(first);
        }
        // A solve that leaves its own file as it was drawn is its own load: the load would
        // send the same system and get the same answer. Every load of a saved sketch is one.
        if self
            .stored(&first.solved.values)
            .is_ok_and(|file| file == self.sketch)
        {
            return Ok(first);
        }
        let mut latest = first.solved.clone();
        let mut solved_by = self.clone();
        for _ in 0..MAX_LOAD_RESOLVES {
            let file = match solved_by.stored(&latest.values) {
                Ok(file) => file,
                Err(e) => return Ok(refusal(first, Err(e))),
            };
            // A file the gate refuses can't be saved or loaded (`holdable`).
            if let Err(e) = holdable(&file) {
                return Ok(refusal(first, Err(e)));
            }
            let load = match Translation::new(&file) {
                Ok(load) => load,
                Err(e) => return Ok(refusal(first, Err(e))),
            };
            // A worker that failed (crashed, timed out, ran short) or a reply that can't be
            // trusted is the error, as for the first solve; the worker refusing the start is
            // the load's error too, so a drawing that solved is refused with it.
            let here = match load.answer(solver) {
                Ok(here) => here,
                Err(e) if e.code() == ErrorCode::InvalidInput && first.outcome.is_err() => {
                    return Ok(first);
                }
                Err(e) => return Err(e),
            };
            if !reached(&here.solved) {
                return Ok(refusal(first, Ok(here)));
            }
            // Coordinates compare bit for bit (a file keeps -0 and 0 apart).
            let settled = match load.stored(&here.solved.values) {
                Ok(next) => next == file,
                Err(e) => return Ok(refusal(first, Err(e))),
            };
            if settled {
                return Ok(if here.outcome.is_ok() {
                    here
                } else {
                    refusal(first, Ok(here))
                });
            }
            latest = here.solved;
            solved_by = load;
        }
        Ok(refusal(
            first,
            Err(Error::new(
                ErrorCode::SolveFailed,
                format!(
                    "the sketch's solution doesn't settle on load: each of {MAX_LOAD_RESOLVES} loads of its solution moved it"
                ),
            )
            .with_suggestion("draw the geometry nearer its solution, or report the sketch: Silico can't keep a solution its own file doesn't give back")),
        ))
    }

    /// Solves this with `solver`, checks the reply, and reads it.
    fn answer(&self, solver: &mut dyn SketchSolver) -> Result<Answer, Error> {
        let solved = solver.solve(self.system.clone())?;
        if let Err(e) = self.check_reply(&solved) {
            solver.distrust();
            return Err(e);
        }
        Ok(Answer {
            outcome: self.read(&solved),
            solved,
        })
    }

    /// The points, circles' radii, and reference dimensions' measures at `values`.
    fn placed(&self, values: &[f64]) -> Result<Placed, Error> {
        // Finite, as check_reply and the buildable checks have made sure of a solution.
        let coordinate = |v: f64| {
            Coordinate::new(v).ok_or_else(|| {
                Error::new(
                    ErrorCode::Internal,
                    format!("a solution the core checked has a value that isn't a number: {v}"),
                )
            })
        };
        let points = self
            .points
            .iter()
            .map(|(id, &p)| {
                let [x, y] = self.xy(values, p);
                Ok((id.clone(), [coordinate(x)?, coordinate(y)?]))
            })
            .collect::<Result<_, Error>>()?;
        let radii = self
            .circles
            .iter()
            .map(|(id, c)| Ok((id.clone(), coordinate(at(values, c.radius))?)))
            .collect::<Result<_, Error>>()?;
        Ok((points, radii, self.references(values)?))
    }

    /// The sketch as its file holds it once `values` are written back, as a save writes a
    /// solution ([`SketchSolution::written`]): what a load starts from.
    pub(crate) fn stored(&self, values: &[f64]) -> Result<Sketch, Error> {
        let (points, radii, references) = self.placed(values)?;
        Ok(written(&self.sketch, &points, &radii, &references))
    }

    /// That `solved` answers what was sent: as many values, the fixed ones as sent and all
    /// within reach, lists of tags sent and unknowns, a residual for each tag sent, and an
    /// algorithm exactly where a solution is given. A worker that answers otherwise can't
    /// be trusted.
    pub fn check_reply(&self, solved: &Solved) -> Result<(), Error> {
        check_solved(&self.system, solved)
    }

    /// Point `id`'s own geometry, which a drag of it pins, if the sketch has the point.
    pub(crate) fn point_geometry(&self, id: &ElementId) -> Option<Geometry> {
        self.point_geometry.get(id).copied()
    }

    /// Each point's values, by ID.
    pub(crate) fn point_values(&self) -> impl Iterator<Item = (&ElementId, PointValues)> {
        self.points.iter().map(|(id, &v)| (id, v))
    }

    /// Each circle's and arc's radius, by ID.
    pub(crate) fn radius_values(&self) -> Vec<(&ElementId, Value)> {
        let mut radii: Vec<(&ElementId, Value)> = self
            .circles
            .iter()
            .map(|(id, c)| (id, c.radius))
            .chain(self.arcs.iter().map(|(id, a)| (id, a.round.radius)))
            .collect();
        radii.sort_by(|a, b| a.0.cmp(b.0));
        radii
    }

    /// The ID of the constraint the solver reports as `tag`, if one is.
    pub fn constraint_id(&self, tag: Tag) -> Option<&ElementId> {
        self.constraint_tagged(tag).map(|c| &c.id)
    }

    /// The document's constraint tagged `tag`, if one is.
    fn constraint_tagged(&self, tag: Tag) -> Option<&SketchConstraint> {
        (tag.0 as usize)
            .checked_sub(1)
            .and_then(|i| self.constraints.get(i))
    }

    /// The form of the document's constraint tagged `tag`, if one is.
    fn form(&self, tag: Tag) -> Option<&Form> {
        self.constraint_tagged(tag).map(|c| &c.form)
    }

    /// The angle sent for the `i`th constraint (sorted by ID), if it's a tangency at an end
    /// or where two curves meet: 0 or π.
    pub fn sent_angle(&self, i: usize) -> Option<f64> {
        let tag = Tag(u32::try_from(i).ok()?.checked_add(1)?);
        self.system.constraints.iter().find_map(|c| match *c {
            SolverConstraint::TangentOn { tag: t, angle, .. }
            | SolverConstraint::AngleViaPoint { tag: t, angle, .. }
                if t == tag =>
            {
                self.system.values.get(angle.0 as usize).copied()
            }
            _ => None,
        })
    }

    /// The constraint tagged `tag`'s ID.
    fn id(&self, tag: Tag) -> Result<&ElementId, Error> {
        self.constraint_id(tag).ok_or_else(|| {
            Error::new(
                ErrorCode::ProtocolError,
                format!("the sketch worker names tag {}, which wasn't sent", tag.0),
            )
        })
    }

    fn ids(&self, tags: &[Tag]) -> Result<Vec<String>, Error> {
        tags.iter()
            .map(|&t| self.id(t).map(|id| id.as_str().to_owned()))
            .collect()
    }

    /// What `solved`, which [`check_reply`](Self::check_reply) passed, says about the sketch
    /// (docs/design/m3.3-solving.md, *Outcomes, as data*).
    pub fn read(&self, solved: &Solved) -> Result<SketchSolution, Error> {
        self.read_noting(solved, &mut Vec::new())
    }

    /// What `solved`'s diagnosis says of each of the sketch's members, whatever the read
    /// of it ([`read`](Self::read)) comes to: solved, or refused
    /// (docs/ports/sketcher.md, *Definition states*). `solved` is one
    /// [`check_reply`](Self::check_reply) passed, for this translation or any other of the
    /// same sketch, as a load's (an [`Answer`]'s `solved`): a translation decides its rows
    /// from the sketch's references alone, never from where it's drawn, so every translation
    /// of one sketch reads a reply alike.
    pub fn diagnosis(&self, solved: &Solved) -> Result<Diagnosis, Error> {
        let _span = tracing::info_span!("diagnose sketch").entered();
        let mut disagreeing = Vec::new();
        // Only the constraints decision 5 refuses are wanted of the read: its outcome is
        // the answer's, read already.
        let _ = self.read_noting(solved, &mut disagreeing);
        let named = |tags: &[Tag]| -> Result<BTreeSet<ElementId>, Error> {
            tags.iter().map(|&t| self.id(t).cloned()).collect()
        };
        let mut conflicting = named(&solved.conflicting)?;
        conflicting.extend(named(&disagreeing)?);
        let redundant = named(&solved.redundant)?;
        let partially_redundant = named(&solved.partially_redundant)?;
        // Each listed constraint's rows, in one pass over the rows, and the points whose x or
        // y they read.
        let listed: BTreeSet<Tag> = solved
            .conflicting
            .iter()
            .chain(&disagreeing)
            .chain(&solved.redundant)
            .chain(&solved.partially_redundant)
            .copied()
            .collect();
        let owner: BTreeMap<Value, &ElementId> = self
            .points
            .iter()
            .flat_map(|(id, p)| [(p.x, id), (p.y, id)])
            .collect();
        let mut points_read: BTreeMap<ElementId, BTreeSet<ElementId>> = BTreeMap::new();
        for &tag in &listed {
            points_read.insert(self.id(tag)?.clone(), BTreeSet::new());
        }
        for row in &self.system.constraints {
            let Some(tag) = row.tag().filter(|t| listed.contains(t)) else {
                continue;
            };
            let read = self
                .system
                .reads(row)
                .into_iter()
                .filter_map(|v| owner.get(&v).map(|&id| id.clone()));
            points_read
                .entry(self.id(tag)?.clone())
                .or_default()
                .extend(read);
        }
        Ok(Diagnosis {
            dofs: solved.dofs,
            free: self.free(solved),
            conflicting,
            redundant,
            partially_redundant,
            points_read,
        })
    }

    /// [`read`](Self::read), noting in `disagreeing` the redundant constraints, by tag, it
    /// refuses for disagreeing with the others past decision 5's tolerance: none, unless it
    /// refuses them.
    fn read_noting(
        &self,
        solved: &Solved,
        disagreeing_out: &mut Vec<Tag>,
    ) -> Result<SketchSolution, Error> {
        if !solved.conflicting.is_empty() {
            let n = solved.conflicting.len();
            return Err(Error::new(
                ErrorCode::OverConstrained,
                format!("the sketch's constraints conflict: these {n} can't all hold"),
            )
            .with_elements(self.ids(&solved.conflicting)?)
            .with_suggestion("remove or change one of the constraints named"));
        }
        let algorithm = match (solved.status, solved.algorithm) {
            (SolveStatus::Success | SolveStatus::SolvedExceptRedundant, Some(a)) => a,
            (SolveStatus::SolvedInvalid, _) => {
                let unbuildable = self.unbuildable(&solved.values);
                if unbuildable.is_empty() {
                    return Err(Error::new(
                        ErrorCode::ProtocolError,
                        "the sketch worker calls its solution one that can't be built, and every piece of it can be",
                    ));
                }
                return Err(unbuildable_error(unbuildable));
            }
            (status, _) => {
                return Err(Error::new(
                    ErrorCode::SolveFailed,
                    "no solver could meet the sketch's constraints",
                )
                .with_cause(format!(
                    "{status:?} after DogLeg, Levenberg–Marquardt, BFGS, and anchored SQP"
                ))
                .with_suggestion(
                    "look for constraints that can't hold together, or draw the geometry nearer its solution",
                ));
            }
        };
        let values = &solved.values;
        // A solution stands only on geometry that can be built, whatever the worker says.
        let unbuildable = self.unbuildable(values);
        if !unbuildable.is_empty() {
            return Err(unbuildable_error(unbuildable));
        }
        // A symmetry whose two points the solution puts at one place: the solver measures a
        // symmetry by the line between them, so no later solve could start from it
        // (robustness review: a point held on the line it's mirrored about).
        let collapsed: Vec<Tag> = self
            .system
            .constraints
            .iter()
            .filter_map(|c| match *c {
                SolverConstraint::SymmetricLine { tag, a, b, .. }
                | SolverConstraint::SymmetricPoint { tag, a, b, .. } => {
                    no_distance(self.point_of(values, a), self.point_of(values, b)).then_some(tag)
                }
                _ => None,
            })
            .collect();
        if !collapsed.is_empty() {
            return Err(Error::new(
                ErrorCode::InvalidResult,
                "the solution puts a symmetry's two points at one place, where the next solve can't measure it",
            )
            .with_elements(self.ids(&collapsed)?)
            .with_suggestion(
                "share one point instead, or change the constraints that bring the two together",
            ));
        }
        // Decision 5: a redundant constraint is kept only if it's met on the solution, its
        // gap within the tolerance of the part of the sketch it's in.
        let mut dropped: Vec<Tag> = solved
            .redundant
            .iter()
            .chain(&solved.partially_redundant)
            .copied()
            .collect();
        dropped.sort_unstable();
        dropped.dedup();
        let parts = self.system.parts();
        let mut windows: BTreeMap<Option<Part>, Window> = BTreeMap::new();
        let mut gaps = Vec::with_capacity(dropped.len());
        let mut disagreeing = Vec::new();
        // The disagreeing row with the largest gap (one that isn't a number the largest),
        // the lowest tag on a tie, and its part's window.
        let mut worst: Option<(Tag, f64, Window)> = None;
        for &tag in &dropped {
            let c = self.sent(tag)?;
            let part = parts.of_constraint(c);
            let window = *windows
                .entry(part)
                .or_insert_with(|| self.window(values, &parts, part));
            let gap = self.gap(c, values);
            if !met(gap, window.tolerance) {
                let larger = match worst {
                    None => true,
                    Some((_, w, _)) => !w.is_nan() && (gap.is_nan() || gap > w),
                };
                if larger {
                    worst = Some((tag, gap, window));
                }
                disagreeing.push(tag);
            }
            gaps.push((tag, gap));
        }
        if let Some((tag, worst, window)) = worst {
            let Window {
                extent,
                tightest,
                tolerance,
            } = window;
            let tangency = tightest.map_or_else(String::new, |t| {
                format!(
                    "{TANGENCY_TOLERANCE:e} mm² over the {t:e} mm of its tightest circle tangency ({LEAST_TANGENCY:e} mm at the least), "
                )
            });
            let id = self.id(tag)?;
            disagreeing_out.extend_from_slice(&disagreeing);
            return Err(Error::new(
                ErrorCode::OverConstrained,
                format!(
                    "constraints repeat others but disagree with them, by up to {worst:e} mm ({id}), past the {tolerance:e} mm of the part of the sketch it's in ({EXTENT_TOLERANCE:e} of the part's {extent:e} mm extent, {tangency}or {REDUNDANCY_GAP_MM:e} mm at least)"
                ),
            )
            .with_elements(self.ids(&disagreeing)?)
            .with_suggestion(
                "remove the constraints named, make them agree with the rest, or make a dimension among them a reference (`driving: false`)",
            ));
        }
        // An arc's own ends mirrored about a line its centre is held on are sent as their
        // middle on the line alone (FreeCAD's 365af33fc6); on a half circle that's the
        // centre, which the line already holds, so the solve can leave the ends unmirrored
        // (break-it; FreeCAD 1.1.3 does). The symmetry is checked whole.
        let unmirrored: Vec<Tag> = self
            .system
            .constraints
            .iter()
            .filter_map(|c| match *c {
                SolverConstraint::MidpointOnLine { tag, a, b, line } => {
                    let whole = SolverConstraint::SymmetricLine { tag, a, b, line };
                    (!met(self.gap(&whole, values), REDUNDANCY_GAP_MM)).then_some(tag)
                }
                _ => None,
            })
            .collect();
        if !unmirrored.is_empty() {
            return Err(Error::new(
                ErrorCode::InvalidResult,
                "the solution leaves an arc's ends unmirrored about the line through its centre: a half circle, its chord through the centre",
            )
            .with_elements(self.ids(&unmirrored)?)
            .with_suggestion("draw the arc's ends nearer mirror images, or turn the arc less than half a circle"));
        }
        let wrong_side = self.wrong_sides(values)?;
        if !wrong_side.is_empty() {
            return Err(Error::new(
                ErrorCode::InvalidResult,
                "the solution puts a point or a circle on the other side of its line from the side its constraint gives",
            )
            .with_elements(self.ids(&wrong_side)?)
            .with_suggestion("draw it on the side the constraint gives, or change the side"));
        }
        let concentric = self.concentric_outside(values)?;
        if !concentric.is_empty() {
            return Err(Error::new(
                ErrorCode::InvalidResult,
                "the solution puts two circles held tangent outside on one centre: concentric, they can't touch outside",
            )
            .with_elements(concentric)
            .with_suggestion("dimension their centres apart, or make the tangency inside"));
        }
        // Each kept constraint as the worker listed it (each list checked sorted, above): one
        // it lists as both is redundant.
        let mut redundant = BTreeMap::new();
        let mut partially_redundant = BTreeMap::new();
        for &(t, gap) in &gaps {
            let id = self.id(t)?.clone();
            if solved.redundant.binary_search(&t).is_ok() {
                tracing::info!("{id} repeats others, met within {gap:.1e} mm");
                redundant.insert(id, gap);
            } else {
                tracing::info!("{id} partly repeats others, met within {gap:.1e} mm");
                partially_redundant.insert(id, gap);
            }
        }
        let (points, radii, references) = self.placed(values)?;
        Ok(SketchSolution {
            points,
            radii,
            references,
            dofs: solved.dofs,
            free: self.free(solved),
            redundant,
            partially_redundant,
            algorithm,
        })
    }

    /// The constraint sent tagged `tag`.
    fn sent(&self, tag: Tag) -> Result<&SolverConstraint, Error> {
        self.system
            .constraints
            .iter()
            .find(|c| c.tag() == Some(tag))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ProtocolError,
                    format!("the sketch worker names tag {}, which wasn't sent", tag.0),
                )
            })
    }

    /// The points, lines, circles, and arcs `values` can't build, by ID, in order: the
    /// protocol's rule ([`silico_protocol::unbuildable`]), which the worker passes a
    /// solution over by.
    fn unbuildable(&self, values: &[f64]) -> Vec<String> {
        let mut names: BTreeMap<u32, &ElementId> = BTreeMap::new();
        names.extend(self.point_geometry.iter().map(|(id, g)| (g.0, id)));
        names.extend(self.lines.iter().map(|(id, l)| (l.geometry.0, id)));
        names.extend(self.circles.iter().map(|(id, c)| (c.geometry.0, id)));
        names.extend(self.arcs.iter().map(|(id, a)| (a.round.geometry.0, id)));
        let named: BTreeSet<&str> = self
            .system
            .unbuildable(values)
            .iter()
            .filter_map(|g| names.get(&g.0).map(|id| id.as_str()))
            .collect();
        named.into_iter().map(str::to_owned).collect()
    }

    /// The driving constraints that hold a point, or a circle's centre, on one side of a
    /// line (`point_line_distance`, a line's `tangent`), whose solution has it on the other:
    /// 1.1.3's PlaneGCS measures them unsigned (D4).
    fn wrong_sides(&self, values: &[f64]) -> Result<Vec<Tag>, Error> {
        let mut out = Vec::new();
        for (i, c) in self.constraints.iter().enumerate() {
            let tag = Tag(i as u32 + 1);
            let (left, point, line) = match &c.form {
                Form::PointLineDistance {
                    point,
                    line,
                    side,
                    driving: true,
                    ..
                } => (*side == LineSide::Left, self.point_at(values, point)?, line),
                Form::Tangent { a, b, side, .. }
                    if matches!(side, TangentSide::Left | TangentSide::Right) =>
                {
                    let (line, round) = match (self.line(a), self.line(b)) {
                        (Ok(_), _) => (a, b),
                        (_, Ok(_)) => (b, a),
                        _ => {
                            return Err(unchecked(
                                "is a tangency with a side, of no line".to_owned(),
                            ));
                        }
                    };
                    let r = self.round(round)?;
                    (*side == TangentSide::Left, self.xy(values, r.center), line)
                }
                _ => continue,
            };
            let l = self.line(line)?;
            let d = signed_distance(point, self.xy(values, l.from), self.xy(values, l.to));
            if !(if left { d > 0.0 } else { d < 0.0 }) {
                out.push(tag);
            }
        }
        Ok(out)
    }

    /// The tangencies that hold two circles or arcs outside each other whose solution has
    /// their centres under [`CONCENTRIC_TANGENCY`] apart, squared, each followed by its two
    /// curves (each named once): there PlaneGCS measures the tangency as `r1 − r2`, an inside
    /// tangency's form, whatever side it's given, so a solve can meet it with the circles on
    /// one centre. FreeCAD reads the side from the drawing each solve
    /// (`System::addConstraintTangent`), so it sends `outside` whenever each centre starts
    /// outside the other circle (d ≥ r1 and d ≥ r2); if other constraints then drive them together, 1.1.3 lands on equal circles on
    /// one centre and accepts them (#24227). Silico refuses that on purpose: circles on one
    /// centre touch outside only at radius 0, which no circle has.
    fn concentric_outside(&self, values: &[f64]) -> Result<Vec<String>, Error> {
        let mut out: Vec<String> = Vec::new();
        for (i, c) in self.constraints.iter().enumerate() {
            let Form::Tangent {
                a,
                b,
                side: TangentSide::Outside,
            } = &c.form
            else {
                continue;
            };
            let (ra, rb) = (self.round(a)?, self.round(b)?);
            let squared = squared_distance(self.xy(values, ra.center), self.xy(values, rb.center));
            if squared < CONCENTRIC_TANGENCY {
                let tangency = self.id(Tag(i as u32 + 1))?.as_str().to_owned();
                for name in [tangency, a.to_string(), b.to_string()] {
                    if !out.contains(&name) {
                        out.push(name);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Where a point the constraint names is.
    fn point_at(&self, values: &[f64], r: &Reference) -> Result<[f64; 2], Error> {
        match r {
            Reference::Origin => Ok(self.xy(values, ORIGIN)),
            Reference::Element(id) => self
                .points
                .get(id)
                .map(|&p| self.xy(values, p))
                .ok_or_else(|| unchecked(format!("names {id} as a point"))),
            _ => Err(unchecked(format!("names {r} as a point"))),
        }
    }

    /// Each reference dimension's value, measured on the solution.
    fn references(&self, values: &[f64]) -> Result<BTreeMap<ElementId, Measured>, Error> {
        let mut out = BTreeMap::new();
        for c in &self.constraints {
            let length = |a: [f64; 2], b: [f64; 2]| squared_distance(a, b).sqrt();
            let measured = match &c.form {
                Form::Length {
                    line,
                    driving: false,
                    ..
                } => {
                    let l = self.line(line)?;
                    Measured::Length(length(self.xy(values, l.from), self.xy(values, l.to)))
                }
                Form::Distance {
                    a,
                    b,
                    driving: false,
                    ..
                } => Measured::Length(length(self.point_at(values, a)?, self.point_at(values, b)?)),
                Form::DistanceX {
                    a,
                    b,
                    driving: false,
                    ..
                } => Measured::Length(self.point_at(values, b)?[0] - self.point_at(values, a)?[0]),
                Form::DistanceY {
                    a,
                    b,
                    driving: false,
                    ..
                } => Measured::Length(self.point_at(values, b)?[1] - self.point_at(values, a)?[1]),
                Form::PointLineDistance {
                    point,
                    line,
                    side,
                    driving: false,
                    ..
                } => {
                    let l = self.line(line)?;
                    let d = signed_distance(
                        self.point_at(values, point)?,
                        self.xy(values, l.from),
                        self.xy(values, l.to),
                    );
                    Measured::FromLine {
                        distance: d.abs(),
                        side: if d > 0.0 {
                            LineSide::Left
                        } else if d < 0.0 {
                            LineSide::Right
                        } else {
                            *side
                        },
                    }
                }
                Form::Angle {
                    a,
                    b,
                    value,
                    driving: false,
                } => {
                    let (la, lb) = (self.line(a)?, self.line(b)?);
                    let measured = angle_between(
                        direction(self.xy(values, la.from), self.xy(values, la.to)),
                        direction(self.xy(values, lb.from), self.xy(values, lb.to)),
                    );
                    // On the stored value's turn, as 1.1.3's solve leaves it, then reduced
                    // as it writes it back: the measure plus the whole turns that takes.
                    // Only the turns come from the stored value, so the result doesn't
                    // carry its low bits, and a save of it measures it again bit for bit
                    // (break-it: `fmod(stored + wrap(measured − stored), 2π)` rounded with
                    // the stored value, and its last nanodegree flipped on every save).
                    let stored = value.degrees().to_radians();
                    let reduced = libm::fmod(stored + wrap(measured - stored), TAU);
                    let turns = libm::round((reduced - measured) / TAU);
                    Measured::Angle((measured + turns * TAU).to_degrees())
                }
                Form::Radius {
                    curve,
                    driving: false,
                    ..
                } => Measured::Length(at(values, self.round(curve)?.radius)),
                Form::Diameter {
                    curve,
                    driving: false,
                    ..
                } => Measured::Length(2.0 * at(values, self.round(curve)?.radius)),
                _ => continue,
            };
            out.insert(c.id.clone(), measured);
        }
        Ok(out)
    }

    /// The points, circles, and arcs with a value the solver finds free: every one, when
    /// no constraint touches an unknown (FreeCAD's reading).
    fn free(&self, solved: &Solved) -> BTreeSet<ElementId> {
        let free = |v: Value| solved.no_equations || solved.free.binary_search(&v).is_ok();
        let mut out = BTreeSet::new();
        for (id, p) in &self.points {
            if free(p.x) || free(p.y) {
                out.insert(id.clone());
            }
        }
        for (id, c) in &self.circles {
            if free(c.radius) {
                out.insert(id.clone());
            }
        }
        for (id, a) in &self.arcs {
            if free(a.round.radius) || a.angles.iter().any(|&v| free(v)) {
                out.insert(id.clone());
            }
        }
        out
    }

    /// Where `p`, a point the system's geometry has, is: a point, or a line's or an arc's end
    /// (what the translation sends).
    fn point_of(&self, values: &[f64], p: PointOf) -> [f64; 2] {
        let (PointOf::Point(g) | PointOf::Start(g) | PointOf::End(g) | PointOf::Center(g)) = p;
        let v = match (self.system.geometry.get(g.0 as usize), p) {
            (Some(SketchGeometry::Point(v)), _) => *v,
            (
                Some(SketchGeometry::Line { start, .. } | SketchGeometry::Arc { start, .. }),
                PointOf::Start(_),
            ) => *start,
            (
                Some(SketchGeometry::Line { end, .. } | SketchGeometry::Arc { end, .. }),
                PointOf::End(_),
            ) => *end,
            // A centre is a point of the sketch's own (shared, D2): the translation sends
            // it as that `Point`, never as a curve's `Center`.
            _ => return [f64::NAN; 2],
        };
        self.xy(values, v)
    }

    /// A line's ends, by its geometry.
    fn ends(&self, values: &[f64], g: Geometry) -> ([f64; 2], [f64; 2]) {
        (
            self.point_of(values, PointOf::Start(g)),
            self.point_of(values, PointOf::End(g)),
        )
    }

    /// A circle's or arc's centre and radius, by its geometry.
    fn circle(&self, values: &[f64], g: Geometry) -> ([f64; 2], f64) {
        match self.system.geometry.get(g.0 as usize) {
            Some(
                SketchGeometry::Circle { center, radius }
                | SketchGeometry::Arc { center, radius, .. },
            ) => (self.xy(values, *center), at(values, *radius)),
            _ => ([f64::NAN; 2], f64::NAN),
        }
    }

    /// A curve's normal at `p`, as PlaneGCS's angle at a point takes it: a line's direction
    /// turned a quarter counter-clockwise, a circle's or arc's from `p` to its centre.
    fn normal(&self, values: &[f64], g: Geometry, p: [f64; 2]) -> [f64; 2] {
        match self.system.geometry.get(g.0 as usize) {
            Some(SketchGeometry::Line { .. }) => {
                let (a, b) = self.ends(values, g);
                let d = direction(a, b);
                [-d[1], d[0]]
            }
            _ => {
                let (c, _) = self.circle(values, g);
                [c[0] - p[0], c[1] - p[1]]
            }
        }
    }

    /// A curve's lever on `values`: a line's length, a circle's or arc's radius; none for
    /// an axis, which can't move.
    fn lever_of(&self, values: &[f64], g: Geometry) -> Option<f64> {
        if g == X_AXIS.geometry || g == Y_AXIS.geometry {
            return None;
        }
        match self.system.geometry.get(g.0 as usize) {
            Some(SketchGeometry::Line { .. }) => {
                let (a, b) = self.ends(values, g);
                Some(squared_distance(a, b).sqrt())
            }
            Some(SketchGeometry::Circle { .. } | SketchGeometry::Arc { .. }) => {
                Some(self.circle(values, g).1)
            }
            _ => None,
        }
    }

    /// Decision 5's window for the redundant constraints in `part` on `values`: its extent,
    /// its tightest circle tangency, and the tolerance they give.
    fn window(&self, values: &[f64], parts: &Parts, part: Option<Part>) -> Window {
        let extent = self.extent(values, parts, part);
        let tightest = self.tightest_tangency(values, parts, part);
        let tolerance = REDUNDANCY_GAP_MM
            .max(EXTENT_TOLERANCE * extent)
            .max(tightest.map_or(0.0, |t| TANGENCY_TOLERANCE / t.max(LEAST_TANGENCY)));
        Window {
            extent,
            tightest,
            tolerance,
        }
    }

    /// The extent of `part` of the sketch on `values` (decision 5): the diagonal of the
    /// axis-aligned box around every point of the sketch whose x is in it (points, lines'
    /// ends, centres, and arcs' ends, construction geometry included; the origin and axes
    /// never), measured as the core measures lengths. 0 for a
    /// part of one point or none (a constraint that reads no unknown is in none). A box's
    /// corners are a min and a max, which no order changes, so the extent is the same bits
    /// on every platform.
    fn extent(&self, values: &[f64], parts: &Parts, part: Option<Part>) -> f64 {
        let mut points = self
            .points
            .values()
            .filter(|p| parts.of(p.x) == part)
            .map(|&p| self.xy(values, p));
        let Some(first) = points.next() else {
            return 0.0;
        };
        let (lo, hi) = points.fold((first, first), |(lo, hi), [x, y]| {
            ([lo[0].min(x), lo[1].min(y)], [hi[0].max(x), hi[1].max(y)])
        });
        squared_distance(lo, hi).sqrt()
    }

    /// The tightest circle-to-circle tangency of `part` of the sketch on `values` (decision
    /// 5): the least, over every one sent in it (circles and arcs, kept or dropped), of
    /// d + r1 + r2 outside or d + |r1 − r2| inside, d the centres' distance; none if it has
    /// no such tangency. A tangency whose centres are under [`CONCENTRIC_TANGENCY`] apart,
    /// squared, isn't counted: PlaneGCS measures it in millimetres there. A minimum is the
    /// same whatever the order, so it's the same bits on every platform.
    fn tightest_tangency(&self, values: &[f64], parts: &Parts, part: Option<Part>) -> Option<f64> {
        self.system
            .constraints
            .iter()
            .filter(|c| parts.of_constraint(c) == part)
            .filter_map(|c| match *c {
                SolverConstraint::TangentCircles { a, b, internal, .. } => {
                    let ((ca, ra), (cb, rb)) = (self.circle(values, a), self.circle(values, b));
                    let squared = squared_distance(ca, cb);
                    (squared >= CONCENTRIC_TANGENCY).then(|| {
                        let s = if internal { (ra - rb).abs() } else { ra + rb };
                        squared.sqrt() + s
                    })
                }
                _ => None,
            })
            .reduce(f64::min)
    }

    /// How far a sent constraint is from met on `values` (decision 5), in millimetres: the
    /// least distance a point would have to move for it to hold exactly. A direction's gap
    /// is its angle carried by the smaller lever of its own curves (an axis has none);
    /// horizontal and vertical are a line's rise and run.
    fn gap(&self, c: &SolverConstraint, values: &[f64]) -> f64 {
        use SolverConstraint as S;
        let v = |value: Value| at(values, value);
        let p = |q: PointOf| self.point_of(values, q);
        let length = |a: [f64; 2], b: [f64; 2]| squared_distance(a, b).sqrt();
        let from_line = |q: [f64; 2], line: Geometry| {
            let (a, b) = self.ends(values, line);
            signed_distance(q, a, b)
        };
        let off_curve = |q: [f64; 2], g: Geometry| match self.system.geometry.get(g.0 as usize) {
            Some(SketchGeometry::Line { .. }) => from_line(q, g).abs(),
            _ => {
                let (c, r) = self.circle(values, g);
                (length(q, c) - r).abs()
            }
        };
        let dir = |g: Geometry| {
            let (a, b) = self.ends(values, g);
            direction(a, b)
        };
        // An angle carried by the smaller lever of `a` and `b`; not a number if neither
        // can move.
        let carried = |angle: f64, a: Geometry, b: Geometry| {
            let lever = [a, b]
                .into_iter()
                .filter_map(|g| self.lever_of(values, g))
                .fold(f64::NAN, f64::min);
            angle * lever
        };
        let at_point = |a: Geometry, b: Geometry, q: [f64; 2], angle: Value| {
            let turn = angle_between(self.normal(values, a, q), self.normal(values, b, q));
            carried(wrap(turn - v(angle)).abs(), a, b)
        };
        match *c {
            S::ArcRules { .. } => 0.0,
            S::Coincident { a, b, .. } => length(p(a), p(b)),
            S::Horizontal { line, .. } => {
                let (a, b) = self.ends(values, line);
                (b[1] - a[1]).abs()
            }
            S::Vertical { line, .. } => {
                let (a, b) = self.ends(values, line);
                (b[0] - a[0]).abs()
            }
            S::HorizontalPoints { a, b, .. } => (p(a)[1] - p(b)[1]).abs(),
            S::VerticalPoints { a, b, .. } => (p(a)[0] - p(b)[0]).abs(),
            S::DistanceX { a, b, value, .. } => (p(b)[0] - p(a)[0] - v(value)).abs(),
            S::DistanceY { a, b, value, .. } => (p(b)[1] - p(a)[1] - v(value)).abs(),
            S::Distance { a, b, value, .. } => (length(p(a), p(b)) - v(value)).abs(),
            S::PointOnLine { point, line, .. } => from_line(p(point), line).abs(),
            S::PointLineDistance {
                point, line, value, ..
            } => (from_line(p(point), line).abs() - v(value)).abs(),
            S::PointOnCircle { point, circle, .. } => off_curve(p(point), circle),
            S::Parallel { a, b, .. } => {
                let t = angle_between(dir(a), dir(b)).abs();
                carried(t.min(PI - t), a, b)
            }
            S::Perpendicular { a, b, .. } => {
                carried((angle_between(dir(a), dir(b)).abs() - PI / 2.0).abs(), a, b)
            }
            S::Angle { a, b, value, .. } => {
                carried(wrap(angle_between(dir(a), dir(b)) - v(value)).abs(), a, b)
            }
            S::EqualLength { a, b, .. } => {
                let (a0, a1) = self.ends(values, a);
                let (b0, b1) = self.ends(values, b);
                (length(a0, a1) - length(b0, b1)).abs()
            }
            S::EqualRadius { a, b, .. } => {
                (self.circle(values, a).1 - self.circle(values, b).1).abs()
            }
            S::Radius { circle, value, .. } => (self.circle(values, circle).1 - v(value)).abs(),
            S::TangentLine { line, circle, .. } => {
                let (c, r) = self.circle(values, circle);
                (from_line(c, line).abs() - r).abs()
            }
            S::TangentCircles { a, b, internal, .. } => {
                let ((ca, ra), (cb, rb)) = (self.circle(values, a), self.circle(values, b));
                let want = if internal { (ra - rb).abs() } else { ra + rb };
                (length(ca, cb) - want).abs()
            }
            S::TangentOn {
                a, point, b, angle, ..
            } => {
                let q = p(point);
                // The larger, and not a number if either isn't.
                let (off, turned) = (off_curve(q, b), at_point(a, b, q, angle));
                if off.is_nan() || turned.is_nan() {
                    f64::NAN
                } else {
                    off.max(turned)
                }
            }
            S::AngleViaPoint {
                a, b, point, angle, ..
            } => at_point(a, b, p(point), angle),
            S::SymmetricLine { a, b, line, .. } => {
                let (f, t) = self.ends(values, line);
                length(p(a), mirror(p(b), f, t))
            }
            S::MidpointOnLine { a, b, line, .. } => {
                let (pa, pb) = (p(a), p(b));
                let mid = [(pa[0] + pb[0]) / 2.0, (pa[1] + pb[1]) / 2.0];
                from_line(mid, line).abs()
            }
            S::SymmetricPoint { tag, a, b, center } => {
                let (pa, pb, m) = (p(a), p(b), p(center));
                let midpoint = matches!(self.form(tag), Some(Form::Midpoint { .. }));
                if midpoint {
                    // A midpoint's gap is the point's distance from the line's middle.
                    let middle = [(pa[0] + pb[0]) / 2.0, (pa[1] + pb[1]) / 2.0];
                    length(m, middle)
                } else {
                    // A symmetry's, one point's distance from the other's mirror image.
                    length(pa, [2.0 * m[0] - pb[0], 2.0 * m[1] - pb[1]])
                }
            }
        }
    }
}

/// Geometry a solution can't build, named.
fn unbuildable_error(elements: Vec<String>) -> Error {
    Error::new(
        ErrorCode::InvalidResult,
        "the sketch solves only to geometry that can't be built: past a kilometre, a line of no length, a circle or arc of no radius, or an arc whose ends meet",
    )
    .with_elements(elements)
    .with_suggestion(
        "draw the geometry nearer its solution, or check the dimensions that move it past a kilometre or shrink it to nothing",
    )
}

/// A solution's points, circles' radii, and reference dimensions' measures, by ID.
type Placed = (
    BTreeMap<ElementId, [Coordinate; 2]>,
    BTreeMap<ElementId, Coordinate>,
    BTreeMap<ElementId, Measured>,
);

/// The answer that stands when no load reaches a solved fixed point. A drawing that was
/// refused keeps its own refusal: a refusal writes no file, so its load solves the drawing
/// again and refuses alike. A drawing that solved can't keep a solution its file doesn't
/// give back, so it stands as `load` ended: the last load's answer, or, where a load
/// couldn't be made, checked, or settled, the first solve's values with that error.
fn refusal(first: Answer, load: Result<Answer, Error>) -> Answer {
    if first.outcome.is_err() {
        return first;
    }
    match load {
        Ok(load) => load,
        Err(e) => Answer {
            solved: first.solved,
            outcome: Err(e),
        },
    }
}

/// Decision 5's window for the redundant constraints of one part of a sketch.
#[derive(Debug, Clone, Copy)]
struct Window {
    /// The part's extent, in millimetres.
    extent: f64,
    /// The part's tightest circle tangency, in millimetres, as measured.
    tightest: Option<f64>,
    /// The most a redundant constraint's gap may be there and still agree.
    tolerance: f64,
}

/// Whether a gap is within `tolerance` (decision 5); a gap that isn't a number isn't.
fn met(gap: f64, tolerance: f64) -> bool {
    gap <= tolerance
}

/// Whether `solved` reached a solution, one that can be built or not.
fn reached(solved: &Solved) -> bool {
    matches!(
        solved.status,
        SolveStatus::Success | SolveStatus::SolvedExceptRedundant
    )
}

fn at(values: &[f64], v: Value) -> f64 {
    values.get(v.0 as usize).copied().unwrap_or(f64::NAN)
}

fn direction(from: [f64; 2], to: [f64; 2]) -> [f64; 2] {
    [to[0] - from[0], to[1] - from[1]]
}

/// The angle from direction `a` to `b`, counter-clockwise, in (−π, π].
fn angle_between(a: [f64; 2], b: [f64; 2]) -> f64 {
    libm::atan2(a[0] * b[1] - a[1] * b[0], a[0] * b[0] + a[1] * b[1])
}

/// `x` less whole turns, into [−π, π].
fn wrap(x: f64) -> f64 {
    libm::remainder(x, TAU)
}

/// `q`'s distance from the line through `a` and `b`, left of its direction positive.
fn signed_distance(q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = direction(a, b);
    (d[0] * (q[1] - a[1]) - d[1] * (q[0] - a[0])) / squared_distance(a, b).sqrt()
}

/// `q` mirrored in the line through `a` and `b`.
fn mirror(q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let d = direction(a, b);
    let t = (d[0] * (q[0] - a[0]) + d[1] * (q[1] - a[1])) / squared_distance(a, b);
    let foot = [a[0] + t * d[0], a[1] + t * d[1]];
    [2.0 * foot[0] - q[0], 2.0 * foot[1] - q[1]]
}
