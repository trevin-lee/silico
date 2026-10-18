//! A document's sketch solved (docs/design/m3.3-solving.md): translated into the sketch
//! worker's terms, every form to the PlaneGCS rows FreeCAD 1.1.3 makes for it, solved in
//! FreeCAD's order, and read back as data: the solved geometry, or what's wrong, naming the
//! sketch's constraints and geometry by ID.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::{PI, TAU};
use std::path::{Path, PathBuf};
use std::time::Duration;

use silico_protocol::{
    Algorithm, Generation, Geometry, Op, PointOf, PointValues, Reply, SketchGeometry, SketchSystem,
    SolveStatus, Solved, SolverConstraint, Tag, Value, arc_from_points, no_distance,
    squared_distance,
};

use crate::document::{
    Angle as DocumentAngle, Checked, Coordinate, Direction, ElementId, FeatureId, Form, Length,
    LineSide, Operation, Reference, Sketch, SketchConstraint, TangentSide,
};
use crate::worker::{Stamp, Worker, WorkerKind, identity_of};
use crate::{Error, ErrorCode};

/// The algorithms a sketch is solved with, in FreeCAD's order (`Sketch::internalSolve`).
pub const ALGORITHMS: [Algorithm; 4] = [
    Algorithm::DogLeg,
    Algorithm::LevenbergMarquardt,
    Algorithm::Bfgs,
    Algorithm::AnchoredSqp,
];

/// How far a redundant constraint may be from met, in millimetres, and still agree with the
/// others (decision 5): a hundredth of the file's nanometre, a hundred times the solver's
/// convergence (1e-10), and 86 times the spacing of doubles a kilometre out.
pub const LENGTH_TOLERANCE: f64 = 1e-8;

/// How far a redundant constraint may be from met, in radians, and still agree with the
/// others (decision 5): ten times the solver's convergence, 57 nanodegrees.
pub const ANGLE_TOLERANCE: f64 = 1e-9;

/// How many times, at most, a solution is solved again as a load solves it, to reach one a
/// load leaves where it is ([`Translation::solve`]).
pub const AGAIN: usize = 3;

/// How long a solve may take before the worker is stopped. A sketch the constraints can't
/// solve is tried by every algorithm in turn, and at the most unknowns the worker takes
/// (1,024, a chain of 512 points held too far apart), break-it measured DogLeg 11.5 s,
/// Levenberg–Marquardt 28.9 s, BFGS 0.2 s, and anchored SQP 33.2 s on a loaded machine:
/// 74 s, past the kernel's 60. The time grows with the equations the diagnosis keeps
/// too: the same chain with one line given `horizontal` 2,000 times took DogLeg 8.7× as
/// long, and passes 300 s with all four (`MAX_SKETCH_UNKNOWNS`'s doc). A sketch solved
/// again from its solution ([`Translation::solve`]) takes it 1 + [`AGAIN`] times at most.
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
    /// The executable it was started from, to start again; none for a worker handed in.
    exe: Option<Executable>,
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

/// A sketch worker's executable as it was when the worker started: started again only if
/// it's still that build, since another would answer differently under the same name.
struct Executable {
    path: PathBuf,
    stamp: Option<Stamp>,
    /// `sketch worker sha256:…` of its bytes.
    identity: String,
}

impl SketchWorker {
    /// Starts the sketch worker: [`SKETCH_WORKER_ENV`], or `silico-sketch-worker` next to
    /// the running executable.
    pub fn start() -> Result<Self, Error> {
        Self::start_worker(&crate::worker::worker_path(
            SKETCH_WORKER_ENV,
            "silico-sketch-worker",
            "sketch worker",
            "cargo build -p silico-sketch --bin silico-sketch-worker",
        )?)
    }

    /// Starts the sketch worker at `exe`.
    pub fn start_worker(exe: &Path) -> Result<Self, Error> {
        let stamp = Stamp::of(exe);
        let identity = identity_of(exe, WorkerKind::Sketch)?;
        Ok(Self {
            exe: Some(Executable {
                path: exe.to_owned(),
                stamp,
                identity,
            }),
            ..Self::from_worker(Worker::spawn_as(exe, WorkerKind::Sketch)?)
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
        let same = (exe.stamp.is_some() && Stamp::of(&exe.path) == exe.stamp)
            || identity_of(&exe.path, WorkerKind::Sketch)? == exe.identity;
        if !same {
            return Err(Error::new(
                ErrorCode::WorkerCrashed,
                format!(
                    "the sketch worker {} changed since it started: another build of it \
                     isn't started in its place",
                    exe.path.display()
                ),
            )
            .with_suggestion("restart Silico, so the new worker's solutions are kept as its own"));
        }
        self.worker = Worker::spawn_as(&exe.path, WorkerKind::Sketch)?;
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
            if self.worker.is_stopped() || self.worker.has_exited() {
                return Err(Error::new(
                    ErrorCode::WorkerCrashed,
                    "the sketch worker ended during the solve",
                )
                .with_suggestion("solve again: the next solve starts another worker"));
            }
        } else {
            self.ready()?;
        }
        let op = Op::Solve(system);
        let name = op.name();
        match self.worker.call(Generation(0), op, self.timeout)? {
            Reply::Solved(solved) => Ok(solved),
            other => {
                self.worker.shut_down();
                Err(Error::new(
                    ErrorCode::ProtocolError,
                    format!("the sketch worker replied to {name} with {}", other.kind()),
                ))
            }
        }
    }
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
    /// Constraints that repeat others and agree with them (decision 5): kept, and reported.
    pub redundant: BTreeSet<ElementId>,
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
        let mut out = sketch.clone();
        for p in &mut out.points {
            if let Some(&[x, y]) = self.points.get(&p.id) {
                (p.x, p.y) = (x, y);
            }
        }
        for c in &mut out.circles {
            if let Some(&r) = self.radii.get(&c.id) {
                c.radius = r;
            }
        }
        for c in &mut out.constraints {
            if let Some(&measured) = self.references.get(&c.id) {
                measure_into(&mut c.form, measured);
            }
        }
        out
    }
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

/// What a solve written back changes of `stored`: each point, circle, and constraint whose
/// written form differs, in the file's order, with what changed (`p0000003 x 4 → 4.5`).
pub(crate) fn moved(stored: &Sketch, written: &Sketch) -> Vec<(ElementId, String)> {
    let same = |a: Coordinate, b: Coordinate| a.mm().to_bits() == b.mm().to_bits();
    let mut out = Vec::new();
    for (s, w) in stored.points.iter().zip(&written.points) {
        let mut what = Vec::new();
        if !same(s.x, w.x) {
            what.push(format!("x {} → {}", s.x.mm(), w.x.mm()));
        }
        if !same(s.y, w.y) {
            what.push(format!("y {} → {}", s.y.mm(), w.y.mm()));
        }
        if !what.is_empty() {
            out.push((s.id.clone(), format!("{} {}", s.id, what.join(", "))));
        }
    }
    for (s, w) in stored.circles.iter().zip(&written.circles) {
        if !same(s.radius, w.radius) {
            out.push((
                s.id.clone(),
                format!("{} radius {} → {}", s.id, s.radius.mm(), w.radius.mm()),
            ));
        }
    }
    for (s, w) in stored.constraints.iter().zip(&written.constraints) {
        if s.form != w.form {
            let said = match (s.form.value_text(), w.form.value_text()) {
                (Some(before), Some(after)) => format!("{} value {before} → {after}", s.id),
                _ => format!("{}", s.id),
            };
            out.push((s.id.clone(), said));
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
    let found = document
        .document()
        .features
        .iter()
        .find(|f| &f.id == feature)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidInput,
                format!("the document has no feature {}", feature.as_str()),
            )
        })?;
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
    let solution = answer.outcome.map_err(named)?;
    savable(sketch, &solution).map_err(named)?;
    Ok(solution)
}

/// Whether `solution`, written into `sketch`, is a sketch the gate takes: a solve can be
/// met only to rounding, and where a value is at its limit the file's measure of it can
/// land past (break-it: an arc of radius 1 km solved to points 1 km and an ulp apart, which
/// the gate, measuring an arc's radius from its points, refuses), so a save of it would
/// write a file that can't be opened again. Refused as an invalid result, naming what the
/// gate names.
fn savable(sketch: &Sketch, solution: &SketchSolution) -> Result<(), Error> {
    let problems = crate::document::sketch_problems(&solution.written(sketch));
    let Some(first) = problems.first() else {
        return Ok(());
    };
    let elements: BTreeSet<&str> = problems
        .iter()
        .flat_map(|e| e.elements())
        .map(String::as_str)
        .collect();
    let mut error = Error::new(
        ErrorCode::InvalidResult,
        format!(
            "the sketch solves to geometry its file can't hold: {}",
            first.message()
        ),
    )
    .with_elements(elements)
    .with_suggestion(
        "keep the sketch's dimensions inside their limits, not at them: a solve meets them only to rounding",
    );
    if let Some(parameter) = first.parameter() {
        error = error.with_parameter(parameter);
    }
    Err(error)
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
        if let S::SymmetricLine { a, b, .. } | S::SymmetricPoint { a, b, .. } = sent {
            self.apart(a, b)?;
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

    /// That a symmetry's two points aren't drawn at one place: PlaneGCS measures a symmetry
    /// by the line between them.
    fn apart(&self, a: PointOf, b: PointOf) -> Result<(), Error> {
        let (pa, pb) = (
            self.point_of(&self.system.values, a),
            self.point_of(&self.system.values, b),
        );
        if no_distance(pa, pb) {
            return Err(Error::new(
                ErrorCode::InvalidInput,
                format!(
                    "a symmetry of two points drawn at one place, ({}, {}): the solver measures it by the line between them",
                    pa[0], pa[1]
                ),
            )
            .with_suggestion("draw the two points apart, where they'd mirror each other"));
        }
        Ok(())
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
    /// PlaneGCS diagnoses where the sketch is drawn, as FreeCAD does, and drops the
    /// redundant constraints it picks there; so a sketch drawn far from its solution (a
    /// template redimensioned a thousandfold) can be called conflicting, and one whose kept
    /// constraints fix a value only loosely (through a square root of their convergence, or
    /// an angle along a long side) can leave a dropped constraint measurably off (M3.3's
    /// break-it). A load (M3.4) solves the sketch from the solution its file stores, and
    /// diagnoses there: it can drop other constraints, call them conflicting, or move the
    /// solution. So when the solver reached a solution and either called the sketch
    /// over-constrained or dropped redundant constraints, the solution is written into the
    /// sketch as its file would hold it and solved as a load solves it (translated again,
    /// each arc's radius and angles worked out from its points), until a load leaves the
    /// sketch where it started, bit for bit, at most [`AGAIN`] times. That load's answer
    /// stands, a refusal or not, with the constraints it names: it is what every later
    /// load gives. So does a load that reaches no solution from a solution a save would
    /// write: it leaves the file as it is, so every load refuses it the same way. If no load
    /// settles, or one fails from a refusal, the first answer stands. SolidWorks solves such
    /// a sketch.
    pub fn solve(&self, solver: &mut dyn SketchSolver) -> Result<Answer, Error> {
        let first = self.answer(solver)?;
        if !reached(&first.solved) {
            return Ok(first);
        }
        let dropped =
            !first.solved.redundant.is_empty() || !first.solved.partially_redundant.is_empty();
        match &first.outcome {
            Ok(_) if dropped => {}
            Err(e) if e.code() == ErrorCode::OverConstrained => {}
            _ => return Ok(first),
        }
        // A solution a load can't start from (a translation the gate's rules refuse, which a
        // save refuses too), the worker refusing it as a start, or a load that reaches no
        // solution: the first answer stands. A worker that failed (crashed, timed out, ran
        // short) or a reply that can't be trusted is the error, as it is for the first solve.
        let Some(mut stored) = self.stored(&first.solved.values) else {
            return Ok(first);
        };
        // Whether the answer `stored` was written from solved: one a save would write.
        let mut saved = first.outcome.is_ok();
        for _ in 0..AGAIN {
            let Ok(load) = Translation::new(&stored) else {
                return Ok(first);
            };
            let here = match load.answer(solver) {
                Ok(here) => here,
                Err(e) if e.code() == ErrorCode::InvalidInput => return Ok(first),
                Err(e) => return Err(e),
            };
            if !reached(&here.solved) {
                // A load that reaches no solution leaves the file as it is, so every load
                // gives this refusal: a solution a save would write stands refused with it.
                // From a refusal, nothing is saved, and the first answer stands.
                return Ok(if saved { here } else { first });
            }
            let Some(next) = load.stored(&here.solved.values) else {
                return Ok(first);
            };
            if same_place(&next, &stored) {
                return Ok(here);
            }
            saved = here.outcome.is_ok();
            stored = next;
        }
        Ok(first)
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

    /// The sketch as its file holds it with its points and circles' radii at `values`;
    /// none if one isn't a finite number.
    fn stored(&self, values: &[f64]) -> Option<Sketch> {
        let mut out = self.sketch.clone();
        for p in &mut out.points {
            let [x, y] = self.xy(values, *self.points.get(&p.id)?);
            (p.x, p.y) = (Coordinate::new(x)?, Coordinate::new(y)?);
        }
        for c in &mut out.circles {
            c.radius = Coordinate::new(at(values, self.circles.get(&c.id)?.radius))?;
        }
        Some(out)
    }

    /// The tags sent, in increasing order.
    fn tags(&self) -> Vec<Tag> {
        let mut tags: Vec<Tag> = self
            .system
            .constraints
            .iter()
            .filter_map(SolverConstraint::tag)
            .collect();
        tags.sort_unstable();
        tags
    }

    /// That `solved` answers what was sent: as many values, the fixed ones as sent and all
    /// within reach, lists of tags sent and unknowns, a residual for each tag sent, and an
    /// algorithm exactly where a solution is given. A worker that answers otherwise can't
    /// be trusted.
    pub fn check_reply(&self, solved: &Solved) -> Result<(), Error> {
        let bad = |what: String| {
            Err(Error::new(
                ErrorCode::ProtocolError,
                format!("the sketch worker's solution {what}"),
            ))
        };
        let sent = &self.system;
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
        let tags = self.tags();
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
        // Decision 5: a redundant constraint is kept only if it's met on the solution.
        let mut dropped: Vec<Tag> = solved
            .redundant
            .iter()
            .chain(&solved.partially_redundant)
            .copied()
            .collect();
        dropped.sort_unstable();
        dropped.dedup();
        let mut disagreeing = Vec::new();
        let mut worst = Gap::default();
        for &tag in &dropped {
            let gap = self.gap(self.sent(tag)?, values);
            if !gap.met() {
                worst = worst.max(gap);
                disagreeing.push(tag);
            }
        }
        if !disagreeing.is_empty() {
            return Err(Error::new(
                ErrorCode::OverConstrained,
                format!(
                    "constraints repeat others but disagree with them, by up to {:e} mm and {:e} rad",
                    worst.mm, worst.rad
                ),
            )
            .with_elements(self.ids(&disagreeing)?)
            .with_suggestion("remove the constraints named, or make them agree with the rest"));
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
                    (!self.gap(&whole, values).met()).then_some(tag)
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
        let redundant = dropped
            .iter()
            .map(|&t| self.id(t).cloned())
            .collect::<Result<_, _>>()?;
        // Finite, as check_reply and the buildable check above have made sure.
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
        Ok(SketchSolution {
            points,
            radii,
            references: self.references(values)?,
            dofs: solved.dofs,
            free: self.free(solved),
            redundant,
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

    /// How far a sent constraint is from met on `values`, geometrically (decision 5).
    fn gap(&self, c: &SolverConstraint, values: &[f64]) -> Gap {
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
        let at_point = |a: Geometry, b: Geometry, q: [f64; 2], angle: Value| {
            let turn = angle_between(self.normal(values, a, q), self.normal(values, b, q));
            wrap(turn - v(angle)).abs()
        };
        match *c {
            S::ArcRules { .. } => Gap::default(),
            S::Coincident { a, b, .. } => Gap::mm(length(p(a), p(b))),
            S::Horizontal { line, .. } => {
                let d = dir(line);
                Gap::rad(libm::atan2(d[1].abs(), d[0].abs()))
            }
            S::Vertical { line, .. } => {
                let d = dir(line);
                Gap::rad(libm::atan2(d[0].abs(), d[1].abs()))
            }
            S::HorizontalPoints { a, b, .. } => Gap::mm((p(a)[1] - p(b)[1]).abs()),
            S::VerticalPoints { a, b, .. } => Gap::mm((p(a)[0] - p(b)[0]).abs()),
            S::DistanceX { a, b, value, .. } => Gap::mm((p(b)[0] - p(a)[0] - v(value)).abs()),
            S::DistanceY { a, b, value, .. } => Gap::mm((p(b)[1] - p(a)[1] - v(value)).abs()),
            S::Distance { a, b, value, .. } => Gap::mm((length(p(a), p(b)) - v(value)).abs()),
            S::PointOnLine { point, line, .. } => Gap::mm(from_line(p(point), line).abs()),
            S::PointLineDistance {
                point, line, value, ..
            } => Gap::mm((from_line(p(point), line).abs() - v(value)).abs()),
            S::PointOnCircle { point, circle, .. } => Gap::mm(off_curve(p(point), circle)),
            S::Parallel { a, b, .. } => {
                let t = angle_between(dir(a), dir(b)).abs();
                Gap::rad(t.min(PI - t))
            }
            S::Perpendicular { a, b, .. } => {
                Gap::rad((angle_between(dir(a), dir(b)).abs() - PI / 2.0).abs())
            }
            S::Angle { a, b, value, .. } => {
                Gap::rad(wrap(angle_between(dir(a), dir(b)) - v(value)).abs())
            }
            S::EqualLength { a, b, .. } => {
                let (a0, a1) = self.ends(values, a);
                let (b0, b1) = self.ends(values, b);
                Gap::mm((length(a0, a1) - length(b0, b1)).abs())
            }
            S::EqualRadius { a, b, .. } => {
                Gap::mm((self.circle(values, a).1 - self.circle(values, b).1).abs())
            }
            S::Radius { circle, value, .. } => {
                Gap::mm((self.circle(values, circle).1 - v(value)).abs())
            }
            S::TangentLine { line, circle, .. } => {
                let (c, r) = self.circle(values, circle);
                Gap::mm((from_line(c, line).abs() - r).abs())
            }
            S::TangentCircles { a, b, internal, .. } => {
                let ((ca, ra), (cb, rb)) = (self.circle(values, a), self.circle(values, b));
                let want = if internal { (ra - rb).abs() } else { ra + rb };
                Gap::mm((length(ca, cb) - want).abs())
            }
            S::TangentOn {
                a, point, b, angle, ..
            } => {
                let q = p(point);
                Gap {
                    mm: off_curve(q, b),
                    rad: at_point(a, b, q, angle),
                }
            }
            S::AngleViaPoint {
                a, b, point, angle, ..
            } => Gap::rad(at_point(a, b, p(point), angle)),
            S::SymmetricLine { a, b, line, .. } => {
                let (f, t) = self.ends(values, line);
                Gap::mm(length(p(a), mirror(p(b), f, t)))
            }
            S::MidpointOnLine { a, b, line, .. } => {
                let (pa, pb) = (p(a), p(b));
                let mid = [(pa[0] + pb[0]) / 2.0, (pa[1] + pb[1]) / 2.0];
                Gap::mm(from_line(mid, line).abs())
            }
            S::SymmetricPoint { tag, a, b, center } => {
                let (pa, pb, m) = (p(a), p(b), p(center));
                let midpoint = matches!(self.form(tag), Some(Form::Midpoint { .. }));
                if midpoint {
                    // A midpoint's gap is the point's distance from the line's middle.
                    let middle = [(pa[0] + pb[0]) / 2.0, (pa[1] + pb[1]) / 2.0];
                    Gap::mm(length(m, middle))
                } else {
                    // A symmetry's, one point's distance from the other's mirror image.
                    Gap::mm(length(pa, [2.0 * m[0] - pb[0], 2.0 * m[1] - pb[1]]))
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

/// How far a constraint is from met: millimetres and radians, each its own way of missing.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Gap {
    mm: f64,
    rad: f64,
}

impl Gap {
    fn mm(mm: f64) -> Self {
        Gap { mm, rad: 0.0 }
    }

    fn rad(rad: f64) -> Self {
        Gap { mm: 0.0, rad }
    }

    /// Within decision 5's tolerances (a gap that isn't a number isn't).
    fn met(self) -> bool {
        self.mm <= LENGTH_TOLERANCE && self.rad <= ANGLE_TOLERANCE
    }

    fn max(self, other: Gap) -> Gap {
        Gap {
            mm: self.mm.max(other.mm),
            rad: self.rad.max(other.rad),
        }
    }
}

/// Whether `solved` reached a solution, one that can be built or not.
fn reached(solved: &Solved) -> bool {
    matches!(
        solved.status,
        SolveStatus::Success | SolveStatus::SolvedExceptRedundant
    )
}

/// Whether `a` and `b` have their points and circles' radii at the same places, bit for
/// bit (a file keeps `-0` and `0` apart).
fn same_place(a: &Sketch, b: &Sketch) -> bool {
    let bits = |c: Coordinate| c.mm().to_bits();
    a.points.len() == b.points.len()
        && a.circles.len() == b.circles.len()
        && a.points
            .iter()
            .zip(&b.points)
            .all(|(p, q)| p.id == q.id && bits(p.x) == bits(q.x) && bits(p.y) == bits(q.y))
        && a.circles
            .iter()
            .zip(&b.circles)
            .all(|(c, d)| c.id == d.id && bits(c.radius) == bits(d.radius))
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
