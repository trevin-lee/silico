//! A sketch as PlaneGCS sees it (docs/ports/sketcher.md): values, which of them the solver
//! may change, geometry made of those values, and constraints on the geometry, each with
//! the tag the solver reports it by. The core builds this from a document's sketch; the
//! sketch worker solves it and says what it found ([`Solved`]).
//!
//! Everything is in the solver's terms: lengths in the document's unit, angles in radians,
//! and the values in one list whose order is the core's, so the worker can hand PlaneGCS
//! the values in that order (its order of unknowns is their order in memory).

use serde::{Deserialize, Serialize};

use crate::primitive::{InvalidParameter, MAX_LENGTH, format_number};

/// At most this many values in a system.
pub const MAX_SKETCH_VALUES: usize = 10_000;

/// At most this many constraints in a system.
pub const MAX_SKETCH_CONSTRAINTS: usize = 10_000;

/// At most this many pieces of geometry in a system.
pub const MAX_SKETCH_GEOMETRY: usize = 10_000;

/// At most this many unknowns in a whole sketch (2 per point, 1 per circle, 3 per arc:
/// 512 points, or about 250 lines). With `MAX_SKETCH_CONSTRAINTS` it bounds the worker's
/// work, not its time: each algorithm runs at most 100 iterations, each a dense
/// factorization of the equations the diagnosis kept by the unknowns (about equations ×
/// unknowns² a step), and a sketch no algorithm solves is tried by all four (DogLeg,
/// Levenberg–Marquardt, BFGS, anchored SQP). The time that work takes is the machine's, and
/// `SKETCH_TIMEOUT` bounds it. Measured: a chain of 512 points held too far apart (514
/// equations) took 74 s on a loaded M4 Pro and 206 s at a load of 80, and at 2,048 unknowns
/// about 1,660 s; the same chain with one line given `horizontal` 2,000 times (2,514
/// equations, every one kept, since the diagnosis's trial solve fails and so calls none of
/// them redundant) took DogLeg alone 135 s at a load of 97 and all four past 300 s, which
/// ends as `worker_timeout` naming the sketch (M3.3's reviews; decided on the maintainer's
/// behalf, 2026-10-17). A count of the whole sketch; counting only its largest part
/// (`SketchSystem::part`'s union-find), which PlaneGCS solves apart, is the next step when
/// imports bring bigger sketches.
pub const MAX_SKETCH_UNKNOWNS: usize = 1024;

/// At most this many unknowns in the part of a sketch a drag moves (the unknowns its
/// constraints join to the points its pins hold: [`DragBegin::part`]): while the cursor
/// holds a point, PlaneGCS solves that part with SQP, on dense matrices, up to 100
/// iterations, so one move can cost
/// far more than a solve. A chain the cursor pulls far from where it began, linked end to
/// end: 0.76 s a move at 256 unknowns on a loaded M4 Pro, 6 s at 512, 64 s at 1024, past a
/// request's time. A larger part solves, but doesn't drag; parts the point isn't in don't
/// count (a grid of rectangles drags in 0.2 ms a move, measured at 1,936 unknowns before
/// a sketch was held to 1,024).
pub const MAX_DRAG_UNKNOWNS: usize = 256;

/// At most this many entries in the solver's Jacobian (rows × unknowns): PlaneGCS's
/// diagnosis builds it dense, so this bounds it to 256 MiB.
pub const MAX_JACOBIAN_ENTRIES: usize = 1 << 25;

/// The largest tag: PlaneGCS takes tags as C `int`s, and leaves a negative one out of its
/// diagnosis.
pub const MAX_TAG: u32 = i32::MAX as u32;

/// A value of the system, by its place in [`SketchSystem::values`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Value(pub u32);

/// A point, by its two coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PointValues {
    pub x: Value,
    pub y: Value,
}

/// A piece of geometry, by the values it's made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SketchGeometry {
    Point(PointValues),
    /// A line segment from `start` to `end`.
    Line {
        start: PointValues,
        end: PointValues,
    },
    Circle {
        center: PointValues,
        radius: Value,
    },
    /// An arc of a circle, counter-clockwise from `start_angle` to `end_angle` (radians),
    /// with its end points: [`SolverConstraint::ArcRules`] ties them to the angles. A
    /// document's arc is defined by its points: the core works its radius and angles out
    /// from them ([`arc_from_points`]) before it sends them.
    Arc {
        center: PointValues,
        radius: Value,
        start_angle: Value,
        end_angle: Value,
        start: PointValues,
        end: PointValues,
    },
}

impl SketchGeometry {
    fn kind(&self) -> GeometryKind {
        match self {
            SketchGeometry::Point(_) => GeometryKind::Point,
            SketchGeometry::Line { .. } => GeometryKind::Line,
            SketchGeometry::Circle { .. } => GeometryKind::Circle,
            SketchGeometry::Arc { .. } => GeometryKind::Arc,
        }
    }

    /// The values it's made of, in the order the sketch worker hands them to PlaneGCS.
    pub fn values(&self) -> Vec<Value> {
        match *self {
            SketchGeometry::Point(p) => vec![p.x, p.y],
            SketchGeometry::Line { start, end } => vec![start.x, start.y, end.x, end.y],
            SketchGeometry::Circle { center, radius } => vec![center.x, center.y, radius],
            SketchGeometry::Arc {
                center,
                radius,
                start_angle,
                end_angle,
                start,
                end,
            } => vec![
                center.x,
                center.y,
                radius,
                start_angle,
                end_angle,
                start.x,
                start.y,
                end.x,
                end.y,
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GeometryKind {
    Point,
    Line,
    Circle,
    Arc,
}

impl GeometryKind {
    fn word(self) -> &'static str {
        match self {
            GeometryKind::Point => "a point",
            GeometryKind::Line => "a line",
            GeometryKind::Circle => "a circle",
            GeometryKind::Arc => "an arc",
        }
    }
}

/// A piece of geometry, by its place in [`SketchSystem::geometry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Geometry(pub u32);

/// A point of a piece of geometry: a point itself, a line's or arc's ends, a circle's or
/// arc's centre (FreeCAD's `PointPos`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointOf {
    Point(Geometry),
    Start(Geometry),
    End(Geometry),
    Center(Geometry),
}

impl PointOf {
    /// The geometry the point is of.
    pub fn geometry(self) -> Geometry {
        match self {
            PointOf::Point(g) | PointOf::Start(g) | PointOf::End(g) | PointOf::Center(g) => g,
        }
    }
}

impl std::fmt::Display for PointOf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PointOf::Point(g) => write!(f, "point {}", g.0),
            PointOf::Start(g) => write!(f, "the start of geometry {}", g.0),
            PointOf::End(g) => write!(f, "the end of geometry {}", g.0),
            PointOf::Center(g) => write!(f, "the centre of geometry {}", g.0),
        }
    }
}

/// A constraint, in PlaneGCS's terms. Each named value is a fixed [`Value`] of the system:
/// reference dimensions, whose value is evaluated after the solve, never solved for, come
/// with the document's sketches (M3.3). Every one except
/// [`ArcRules`](SolverConstraint::ArcRules) carries the tag the solver reports it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SolverConstraint {
    /// An arc's end points on its circle at its angles: implicit in every arc whose values
    /// the solver may change, never reported (PlaneGCS's tag 0). Each of its four
    /// equations must involve an unknown: on fixed values they're rows of zeros that count
    /// against the degrees of freedom, as FreeCAD found (`c6de527391`: fixed arcs get no
    /// rules).
    ArcRules {
        arc: Geometry,
    },
    /// Two points in the same place.
    Coincident {
        tag: Tag,
        a: PointOf,
        b: PointOf,
    },
    /// A line parallel to the x axis.
    Horizontal {
        tag: Tag,
        line: Geometry,
    },
    /// A line parallel to the y axis.
    Vertical {
        tag: Tag,
        line: Geometry,
    },
    /// `value` = x(b) − x(a).
    DistanceX {
        tag: Tag,
        a: PointOf,
        b: PointOf,
        value: Value,
    },
    /// `value` = y(b) − y(a).
    DistanceY {
        tag: Tag,
        a: PointOf,
        b: PointOf,
        value: Value,
    },
    /// The distance between two points.
    Distance {
        tag: Tag,
        a: PointOf,
        b: PointOf,
        value: Value,
    },
    /// A point on a line (extended past its ends).
    PointOnLine {
        tag: Tag,
        point: PointOf,
        line: Geometry,
    },
    Parallel {
        tag: Tag,
        a: Geometry,
        b: Geometry,
    },
    Perpendicular {
        tag: Tag,
        a: Geometry,
        b: Geometry,
    },
    /// The angle from line `a`'s direction to line `b`'s, counter-clockwise (radians).
    Angle {
        tag: Tag,
        a: Geometry,
        b: Geometry,
        value: Value,
    },
    /// Two lines of the same length.
    EqualLength {
        tag: Tag,
        a: Geometry,
        b: Geometry,
    },
    /// A circle's or arc's radius.
    Radius {
        tag: Tag,
        circle: Geometry,
        value: Value,
    },
    /// Two points level: y(a) = y(b).
    HorizontalPoints {
        tag: Tag,
        a: PointOf,
        b: PointOf,
    },
    /// One point above the other: x(a) = x(b).
    VerticalPoints {
        tag: Tag,
        a: PointOf,
        b: PointOf,
    },
    /// A point's distance from a line (extended past its ends), on whichever side it is:
    /// 1.1.3's PlaneGCS has no signed distance, so the point stays on the side it starts.
    PointLineDistance {
        tag: Tag,
        point: PointOf,
        line: Geometry,
        value: Value,
    },
    /// A point on a circle, or on an arc's whole circle.
    PointOnCircle {
        tag: Tag,
        point: PointOf,
        circle: Geometry,
    },
    /// Two circles or arcs of the same radius.
    EqualRadius {
        tag: Tag,
        a: Geometry,
        b: Geometry,
    },
    /// A line tangent to a circle or arc along their edges: the centre as far from the line
    /// (extended) as the radius, on whichever side it is, as for [`PointLineDistance`].
    ///
    /// [`PointLineDistance`]: SolverConstraint::PointLineDistance
    TangentLine {
        tag: Tag,
        line: Geometry,
        circle: Geometry,
    },
    /// Two circles or arcs tangent along their edges: their centres the sum of their radii
    /// apart, or, `internal`, the difference (PlaneGCS's `TangentCircumf`).
    TangentCircles {
        tag: Tag,
        a: Geometry,
        b: Geometry,
        internal: bool,
    },
    /// `point`, an end of `a`, on `b` (a line extended, or a circle, or an arc's whole
    /// circle), and the angle from `a`'s normal to `b`'s there `angle`: FreeCAD's
    /// endpoint-to-edge tangency, two equations under one tag. A curve's normal is its own
    /// direction (a line's from start to end, a circle's or arc's counter-clockwise) turned a
    /// quarter counter-clockwise, so an `angle` of 0 holds the two directions the same there,
    /// and π opposite.
    TangentOn {
        tag: Tag,
        a: Geometry,
        point: PointOf,
        b: Geometry,
        angle: Value,
    },
    /// The angle from `a`'s normal to `b`'s at `point` (as for [`TangentOn`]): FreeCAD's
    /// endpoint-to-endpoint tangency, where the point is both curves' end, so they need no
    /// coincidence.
    ///
    /// [`TangentOn`]: SolverConstraint::TangentOn
    AngleViaPoint {
        tag: Tag,
        a: Geometry,
        b: Geometry,
        point: PointOf,
        angle: Value,
    },
    /// `a` and `b` mirror each other about a line: the segment between them perpendicular to
    /// the line, and its middle on the line (extended). Two equations.
    SymmetricLine {
        tag: Tag,
        a: PointOf,
        b: PointOf,
        line: Geometry,
    },
    /// The middle of `a` and `b` on a line (extended): what's left of [`SymmetricLine`] for
    /// an arc's own ends about a line its centre is held on (FreeCAD `365af33fc6`).
    ///
    /// [`SymmetricLine`]: SolverConstraint::SymmetricLine
    MidpointOnLine {
        tag: Tag,
        a: PointOf,
        b: PointOf,
        line: Geometry,
    },
    /// `a` and `b` mirror each other about `center`: it's on the line through them, and
    /// equally far from both. Two equations.
    SymmetricPoint {
        tag: Tag,
        a: PointOf,
        b: PointOf,
        center: PointOf,
    },
}

/// The number a constraint is reported by: from 1 (PlaneGCS never reports tag 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Tag(pub u32);

/// How PlaneGCS solves (its `Algorithm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Algorithm {
    DogLeg,
    LevenbergMarquardt,
    Bfgs,
    /// FreeCAD's last resort (`Sketch::internalSolve`): every unknown anchored at its
    /// starting value by a row of lower priority (tag −1), so each part with unknowns is
    /// solved by SQP, keeping the sketch's constraints and staying as near the start as they
    /// let it.
    AnchoredSqp,
}

/// What the sketch worker solves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SketchSystem {
    /// Every value, in the core's order: the solver starts from these.
    pub values: Vec<f64>,
    /// The values the solver may change, in increasing order. The rest are fixed.
    pub unknowns: Vec<Value>,
    pub geometry: Vec<SketchGeometry>,
    pub constraints: Vec<SolverConstraint>,
    /// The algorithms to try, in order, after one diagnosis, until one solves on geometry
    /// that can be built (FreeCAD's fallback: DogLeg, then Levenberg–Marquardt, then BFGS,
    /// then SQP anchored at the given values): one to four, each once. They're tried even
    /// when constraints conflict, which FreeCAD doesn't solve: the core reads the conflict.
    pub algorithms: Vec<Algorithm>,
}

/// One part of a sketch's system ([`SketchSystem::parts`]): unknowns no constraint joins to
/// another part's. Which part is which says nothing beyond that, and is never output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Part(usize);

/// The parts of a sketch's system ([`SketchSystem::parts`]), each unknown's resolved once.
#[derive(Debug, Clone)]
pub struct Parts<'a> {
    system: &'a SketchSystem,
    /// Each unknown's part, by its place in `system.unknowns`.
    roots: Vec<usize>,
}

impl Parts<'_> {
    /// The part `v` is in; none for a value that isn't an unknown.
    pub fn of(&self, v: Value) -> Option<Part> {
        let i = self.system.unknowns.binary_search(&v).ok()?;
        self.roots.get(i).copied().map(Part)
    }

    /// The part constraint `c` is in: every unknown it reads is in one, joined by `c`
    /// itself. None for a constraint that reads no unknown.
    pub fn of_constraint(&self, c: &SolverConstraint) -> Option<Part> {
        self.system.reads(c).into_iter().find_map(|v| self.of(v))
    }
}

/// How a solve ended: PlaneGCS's `SolveStatus`, and one reading of it. A status means
/// something only when the diagnosis found no conflict: FreeCAD doesn't solve a conflicting
/// sketch, and PlaneGCS can report `Success` for one with its conflicting constraints unmet.
/// And PlaneGCS solves only the parts of a sketch with unknowns: a constraint among fixed
/// values alone isn't solved, whatever the status, and its residual says whether it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SolveStatus {
    /// Every constraint the solver solved is met, and none was dropped as redundant.
    Success,
    /// Equations were dropped as redundant (`redundant`, `partially_redundant`), every
    /// constraint the solver kept is met, and the dropped ones weren't checked on the
    /// solution: their residuals say whether they agree. PlaneGCS checks them on the values
    /// it started from, before writing its solution back, and calls the solve `Converged`
    /// when they don't hold there, so its own status depends on where the sketch was drawn;
    /// this one doesn't. From DogLeg and Levenberg–Marquardt, which otherwise end only in
    /// success or failure, that's all a `Converged` can mean. Whether the dropped equations
    /// agree enough is the core's (docs/roadmap/m3-sketcher.md, decision 5); FreeCAD fails
    /// such a sketch unless it started where they hold.
    SolvedExceptRedundant,
    /// The solver stopped at a minimum where the constraints aren't all met (BFGS), or found
    /// the redundant equations unmet on the values it started from (BFGS too: its own
    /// `Converged` and that one can't be told apart, so, as FreeCAD does, it's unsolved).
    Converged,
    /// The solver failed.
    Failed,
    /// The solver met the constraints, but on geometry that can't be built: a line of no
    /// length (dx² + dy² is 0), a circle or arc whose radius isn't above 0, an arc whose
    /// start and end are at one place (dx² + dy² is 0), or (a solve's) any value out of reach
    /// ([`within_reach`]), which the next solve couldn't start from. Nothing
    /// is kept. FreeCAD undoes such a solution too and tries its next algorithm
    /// (`Sketch::internalSolve`, "Invalid solution", `fc128d357d`), as a solve does, but
    /// during a drag still reports it as a success and draws the drag's start for that
    /// frame; Silico reports it, and the client keeps its last good frame. A radius of
    /// exactly 0 FreeCAD keeps; Silico draws nothing of no size.
    SolvedInvalid,
}

/// What the sketch worker found: the diagnosis, then the solve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Solved {
    /// Degrees of freedom left: negative when over-constrained.
    pub dofs: i32,
    /// Constraints in conflict, by tag, in increasing order.
    pub conflicting: Vec<Tag>,
    /// Constraints that repeat others, by tag, in increasing order.
    pub redundant: Vec<Tag>,
    /// Constraints some of whose equations repeat others, by tag, in increasing order.
    pub partially_redundant: Vec<Tag>,
    /// Unknowns PlaneGCS finds the constraints don't fix, each once, in increasing order.
    /// It finds none when there are no equations to diagnose (`no_equations`), which
    /// FreeCAD reads as every unknown free.
    pub free: Vec<Value>,
    /// The diagnosis had no equations: no constraint touches an unknown.
    pub no_equations: bool,
    /// How the solve ended: the first algorithm to solve on geometry that can be built
    /// (`Success` or `SolvedExceptRedundant`); otherwise `SolvedInvalid` if any solved on
    /// geometry that can't, or the last tried's.
    pub status: SolveStatus,
    /// The algorithm whose solution `values` holds: the one that solved, or for
    /// `SolvedInvalid` the first whose solution can't be built.
    pub algorithm: Option<Algorithm>,
    /// Every value: the solution when the status is `Success` or `SolvedExceptRedundant`,
    /// the first solution that can't be built for
    /// `SolvedInvalid` (so the core can say what can't; its values may be out of reach, or
    /// not numbers), and otherwise as given.
    pub values: Vec<f64>,
    /// Each tagged constraint's residual on those values, by tag in increasing order, as
    /// PlaneGCS measures it: its error for a constraint of one equation, the root mean
    /// square of them for one of several, in the constraint's own terms: millimetres for
    /// distances and coincidences, radians for angles, and for parallel and perpendicular a
    /// sine or cosine scaled by the lines' starting lengths over their solved ones
    /// (PlaneGCS fixes the scale when it builds the constraint). `None` where it isn't a
    /// number (geometry of no size).
    pub residuals: Vec<(Tag, Option<f64>)>,
}

/// A drag's start (docs/ports/sketcher.md, Dragging): the sketch where the drag starts, and
/// the point the cursor holds. The sketch worker diagnoses the sketch once and, unless it
/// finds constraints in conflict, keeps a session: the sketch, and the point pinned to the
/// cursor by rows of lower priority than the sketch's own constraints (FreeCAD's
/// `Sketch::initMove`, for one point, moved to where the cursor is). Dragging an arc's
/// start or end pins its centre too, where it starts, a hundred times more weakly, so the
/// end moves before the centre does (FreeCAD `4d7a9435c1`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DragBegin {
    /// The sketch, its values where the drag starts: every move solves from them. Its
    /// algorithms aren't used (but are checked, as a solve's are): a drag solves with
    /// DogLeg alone, as FreeCAD's does.
    pub system: SketchSystem,
    /// The point dragged.
    pub point: PointOf,
}

impl DragBegin {
    /// Whether the worker can start this drag: the system passes
    /// [`SketchSystem::check`], the point is one its geometry has, as a constraint's points
    /// must be, and the part of the sketch its pins hold ([`DragBegin::pins`],
    /// [`SketchSystem::part`]) has at most [`MAX_DRAG_UNKNOWNS`] unknowns.
    pub fn check(&self) -> Result<(), InvalidParameter> {
        self.system.check()?;
        self.system.check_point(self.point, "point", "the drag")?;
        let n = self.part().unknowns.len();
        if n > MAX_DRAG_UNKNOWNS {
            return Err(invalid(
                "point",
                format!(
                    "the part of the sketch the drag moves has {n} unknowns, and a drag takes at most {MAX_DRAG_UNKNOWNS} there: each move of a larger part takes too long"
                ),
            ));
        }
        Ok(())
    }

    /// The points the drag pins, with their weights, as FreeCAD's `Sketch::initMove` pins
    /// one point: the point itself, held to the cursor; and for an arc's start or end, the
    /// arc's centre too, where it starts, a hundred times more weakly, so the end moves
    /// before the centre does (`4d7a9435c1`).
    pub fn pins(&self) -> Vec<(PointOf, f64)> {
        let mut pins = vec![(self.point, 1.0)];
        if let (Some(SketchGeometry::Arc { .. }), PointOf::Start(g) | PointOf::End(g)) = (
            self.system.geometry.get(self.point.geometry().0 as usize),
            self.point,
        ) {
            pins.push((PointOf::Center(g), 0.01));
        }
        pins
    }

    /// The part of the sketch the drag moves: the system reduced to the part its pins are
    /// in ([`SketchSystem::part`]). Each move solves only that, so no other part is solved
    /// again each frame; the rest stays where the drag began.
    pub fn part(&self) -> SketchSystem {
        let points: Vec<PointOf> = self.pins().into_iter().map(|(p, _)| p).collect();
        self.system.part(&points)
    }
}

/// Whether a drag's target, the cursor (`to` of a drag move), is a place a sketch may hold
/// a point: both coordinates finite and within reach ([`within_reach`]).
pub fn check_drag_target(to: [f64; 2]) -> Result<(), InvalidParameter> {
    for (axis, v) in ["x", "y"].into_iter().zip(to) {
        if !within_reach(v) {
            return Err(invalid(
                "to",
                format!(
                    "the drag's target's {axis} must be finite and within {} of 0, got {}",
                    format_number(MAX_LENGTH),
                    format_number(v)
                ),
            ));
        }
    }
    Ok(())
}

/// Where a drag's move left the sketch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Moved {
    /// PlaneGCS's status, as it gave it: `Success`, `Converged`, or `Failed`, never
    /// `SolvedExceptRedundant`; or `SolvedInvalid`, Silico's verdict on a `Success` whose
    /// geometry can't be built (the values withheld). A solve's reading of redundancy doesn't apply: the pin is
    /// of lower priority, so the part of the sketch it's in is solved by SQP, which can end
    /// `Converged` on its own (the cursor somewhere the constraints don't let the point
    /// reach), and FreeCAD's drag takes only `Success`.
    pub status: SolveStatus,
    /// Every value, in the system's order, when the status is `Success`; otherwise none, and
    /// the sketch stays where the last move that succeeded left it (FreeCAD's view keeps
    /// showing that).
    pub values: Option<Vec<f64>>,
}

impl SketchSystem {
    /// Whether the worker can solve this: sizes within the limits, every value finite and
    /// within reach, every reference in range and of the right kind, unknowns in
    /// increasing order, tags from 1, each used once, and an arc's rules only for an arc.
    pub fn check(&self) -> Result<(), InvalidParameter> {
        let n = self.values.len();
        for (parameter, count, limit, what) in [
            ("values", n, MAX_SKETCH_VALUES, "values"),
            (
                "unknowns",
                self.unknowns.len(),
                MAX_SKETCH_UNKNOWNS,
                "unknowns",
            ),
            (
                "geometry",
                self.geometry.len(),
                MAX_SKETCH_GEOMETRY,
                "pieces of geometry",
            ),
            (
                "constraints",
                self.constraints.len(),
                MAX_SKETCH_CONSTRAINTS,
                "constraints",
            ),
        ] {
            if count > limit {
                return Err(invalid(
                    parameter,
                    format!("a sketch may have at most {limit} {what}, got {count}"),
                ));
            }
        }
        let algorithms = &self.algorithms;
        if algorithms.is_empty()
            || algorithms.len() > 4
            || (1..algorithms.len()).any(|i| algorithms[..i].contains(&algorithms[i]))
        {
            return Err(invalid(
                "algorithms",
                format!("algorithms must be one to four different ones, got {algorithms:?}"),
            ));
        }
        // A kilometre, or as many radians: everything the kernel can make is within reach.
        if let Some((i, v)) = self
            .values
            .iter()
            .enumerate()
            .find(|(_, v)| !within_reach(**v))
        {
            return Err(invalid(
                "values",
                format!(
                    "value {i} must be finite and within {} of 0, got {}",
                    format_number(MAX_LENGTH),
                    format_number(*v)
                ),
            ));
        }
        let value = |v: Value, what: &str| -> Result<(), InvalidParameter> {
            if (v.0 as usize) < n {
                Ok(())
            } else {
                Err(invalid(
                    "values",
                    format!("{what} names value {}, but there are {n}", v.0),
                ))
            }
        };
        for pair in self.unknowns.windows(2) {
            if pair[0] >= pair[1] {
                return Err(invalid(
                    "unknowns",
                    format!(
                        "unknowns must be in increasing order, each once: {} comes before {}",
                        pair[0].0, pair[1].0
                    ),
                ));
            }
        }
        for &u in &self.unknowns {
            value(u, "an unknown")?;
        }
        for (i, g) in self.geometry.iter().enumerate() {
            for v in g.values() {
                value(v, &format!("geometry {i}"))?;
            }
            // A line drawn with no length gives the solver 0/0 for its direction: wrong
            // diagnoses, not errors. FreeCAD can't draw one. The solver works the length out
            // as the square root of dx² + dy², so ends close enough for that to be 0 (1e-200
            // apart, say) are no length to it either.
            if let SketchGeometry::Line { start, end } = *g {
                let at = |v: Value| self.values[v.0 as usize];
                if no_distance([at(start.x), at(start.y)], [at(end.x), at(end.y)]) {
                    return Err(invalid(
                        "geometry",
                        format!(
                            "geometry {i} is a line drawn with no length, its ends at ({}, {}) and ({}, {})",
                            format_number(at(start.x)),
                            format_number(at(start.y)),
                            format_number(at(end.x)),
                            format_number(at(end.y))
                        ),
                    ));
                }
            }
        }
        let mut tags = Vec::with_capacity(self.constraints.len());
        let mut ruled = Vec::new();
        let mut rows = 0usize;
        for (i, c) in self.constraints.iter().enumerate() {
            self.check_constraint(c).map_err(|e| InvalidParameter {
                parameter: e.parameter,
                message: match c.tag() {
                    Some(tag) => format!("constraint {i} (tag {}): {}", tag.0, e.message),
                    None => format!("constraint {i}: {}", e.message),
                },
            })?;
            match c.tag() {
                Some(tag) => tags.push(tag),
                None => {
                    if let SolverConstraint::ArcRules { arc } = *c {
                        ruled.push(arc);
                    }
                }
            }
            rows += c.rows();
        }
        ruled.sort_unstable();
        if let Some(pair) = ruled.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(invalid(
                "constraints",
                format!("arc {}'s rules are given twice", pair[0].0),
            ));
        }
        tags.sort_unstable();
        if let Some(pair) = tags.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(invalid(
                "constraints",
                format!(
                    "tag {} is used twice: each constraint has its own",
                    pair[0].0
                ),
            ));
        }
        if rows.saturating_mul(self.unknowns.len()) > MAX_JACOBIAN_ENTRIES {
            return Err(invalid(
                "constraints",
                format!(
                    "{rows} equations in {} unknowns is more than the solver can diagnose (at most {MAX_JACOBIAN_ENTRIES} entries)",
                    self.unknowns.len()
                ),
            ));
        }
        Ok(())
    }

    /// One constraint's references, values, and tag: the parts of [`check`](Self::check)
    /// a refusal names the constraint for.
    fn check_constraint(&self, c: &SolverConstraint) -> Result<(), InvalidParameter> {
        let n = self.values.len();
        let what = constraint_word(c);
        let kind = |g: Geometry| -> Result<GeometryKind, InvalidParameter> {
            self.geometry
                .get(g.0 as usize)
                .map(SketchGeometry::kind)
                .ok_or_else(|| {
                    invalid(
                        "constraints",
                        format!(
                            "{what} names geometry {}, but there are {}",
                            g.0,
                            self.geometry.len()
                        ),
                    )
                })
        };
        let want = |g: Geometry, kinds: &[GeometryKind]| {
            let k = kind(g)?;
            if kinds.contains(&k) {
                Ok(())
            } else {
                Err(invalid(
                    "constraints",
                    format!(
                        "{what} needs {}, but geometry {} is {}",
                        kinds_word(kinds),
                        g.0,
                        k.word()
                    ),
                ))
            }
        };
        let point = |p: PointOf| self.check_point(p, "constraints", what);
        // A dimension's value is fixed: reference dimensions, whose value is evaluated after
        // the solve, come with the document's sketches (M3.3).
        let dimension = |v: Value| -> Result<(), InvalidParameter> {
            if (v.0 as usize) >= n {
                return Err(invalid(
                    "values",
                    format!("{what} names value {}, but there are {n}", v.0),
                ));
            }
            if self.unknowns.binary_search(&v).is_ok() {
                return Err(invalid(
                    "constraints",
                    format!(
                        "{what}'s value {} is an unknown: a dimension's value must be fixed",
                        v.0
                    ),
                ));
            }
            Ok(())
        };
        use GeometryKind::{Arc, Circle, Line};
        match *c {
            SolverConstraint::ArcRules { arc } => {
                want(arc, &[Arc])?;
                self.check_arc_rules(arc)?;
            }
            SolverConstraint::Coincident { a, b, .. } => {
                point(a)?;
                point(b)?;
            }
            SolverConstraint::Horizontal { line, .. } | SolverConstraint::Vertical { line, .. } => {
                want(line, &[Line])?
            }
            SolverConstraint::DistanceX { a, b, value: v, .. }
            | SolverConstraint::DistanceY { a, b, value: v, .. } => {
                point(a)?;
                point(b)?;
                dimension(v)?;
            }
            SolverConstraint::Distance { a, b, value: v, .. } => {
                point(a)?;
                point(b)?;
                dimension(v)?;
                // Above 0, as the gate has it (FreeCAD's `setDatum` refuses 0 only): a
                // distance of 0 is a coincidence, whose solve would leave its points at one
                // place, a start the next solve refuses (below).
                // (Every value is finite by now: checked within reach above.)
                let value = self.values[v.0 as usize];
                if value <= 0.0 {
                    return Err(invalid(
                        "constraints",
                        format!(
                            "{what}'s value must be above 0 (points no distance apart are coincident), got {}",
                            format_number(value)
                        ),
                    ));
                }
                // A point's distance from itself can't hold whatever is drawn: the solver
                // finds it conflicting, as it is.
                if a != b {
                    self.apart(a, b, what)?;
                }
            }
            SolverConstraint::PointOnLine { point: p, line, .. } => {
                point(p)?;
                want(line, &[Line])?;
            }
            SolverConstraint::Parallel { a, b, .. }
            | SolverConstraint::Perpendicular { a, b, .. }
            | SolverConstraint::EqualLength { a, b, .. } => {
                want(a, &[Line])?;
                want(b, &[Line])?;
            }
            SolverConstraint::Angle { a, b, value: v, .. } => {
                want(a, &[Line])?;
                want(b, &[Line])?;
                dimension(v)?;
            }
            SolverConstraint::HorizontalPoints { a, b, .. }
            | SolverConstraint::VerticalPoints { a, b, .. } => {
                point(a)?;
                point(b)?;
            }
            SolverConstraint::PointLineDistance {
                point: p,
                line,
                value: v,
                ..
            } => {
                point(p)?;
                want(line, &[Line])?;
                dimension(v)?;
            }
            SolverConstraint::PointOnCircle {
                point: p, circle, ..
            } => {
                point(p)?;
                want(circle, &[Circle, Arc])?;
                // Held as its distance from the centre, whose gradient there is 0/0.
                self.off_centre(
                    p,
                    circle,
                    what,
                    "the solver measures it by the line from the centre",
                )?;
            }
            SolverConstraint::EqualRadius { a, b, .. }
            | SolverConstraint::TangentCircles { a, b, .. } => {
                want(a, &[Circle, Arc])?;
                want(b, &[Circle, Arc])?;
            }
            SolverConstraint::TangentLine { line, circle, .. } => {
                want(line, &[Line])?;
                want(circle, &[Circle, Arc])?;
            }
            SolverConstraint::TangentOn {
                a,
                point: p,
                b,
                angle,
                ..
            } => {
                want(a, &[Line, Arc])?;
                want(b, &[Line, Circle, Arc])?;
                if !matches!(p, PointOf::Start(g) | PointOf::End(g) if g == a) {
                    return Err(invalid(
                        "constraints",
                        format!("{what}'s point must be an end of geometry {}, got {p}", a.0),
                    ));
                }
                dimension(angle)?;
                // The solver takes a round curve's direction at the point from the line
                // from its centre: none at the centre itself.
                if matches!(kind(b)?, Circle | Arc) {
                    self.off_centre(
                        p,
                        b,
                        what,
                        "the solver takes the curve's direction there by the line from its centre",
                    )?;
                }
            }
            SolverConstraint::AngleViaPoint {
                a,
                b,
                point: p,
                angle,
                ..
            } => {
                want(a, &[Line, Circle, Arc])?;
                want(b, &[Line, Circle, Arc])?;
                point(p)?;
                dimension(angle)?;
            }
            SolverConstraint::SymmetricLine { a, b, line, .. } => {
                point(a)?;
                point(b)?;
                want(line, &[Line])?;
                self.apart(a, b, what)?;
            }
            SolverConstraint::MidpointOnLine { a, b, line, .. } => {
                point(a)?;
                point(b)?;
                want(line, &[Line])?;
            }
            SolverConstraint::SymmetricPoint { a, b, center, .. } => {
                point(a)?;
                point(b)?;
                point(center)?;
                self.apart(a, b, what)?;
            }
            SolverConstraint::Radius {
                circle, value: v, ..
            } => {
                want(circle, &[Circle, Arc])?;
                dimension(v)?;
            }
        }
        if let Some(tag) = c.tag()
            && !(1..=MAX_TAG).contains(&tag.0)
        {
            return Err(invalid(
                "constraints",
                format!(
                    "{what}'s tag must be from 1 to {MAX_TAG} (the solver never reports tag 0, and takes tags as C ints), got {}",
                    tag.0
                ),
            ));
        }
        Ok(())
    }
}

impl SketchSystem {
    /// The parts of the system: its unknowns, joined by the constraints that read them
    /// (PlaneGCS splits a system into parts that share no unknown and solves each apart,
    /// `System::initSolution`). Every constraint joins, whatever the solver later keeps or
    /// drops, so no renumbering changes them; fixed values (the origin's, the axes', every
    /// dimension's) are no part and join nothing. At least the parts PlaneGCS solves in: it
    /// drops redundant equations first, so its parts can be smaller.
    pub fn parts(&self) -> Parts<'_> {
        let index = |v: Value| self.unknowns.binary_search(&v).ok();
        let mut parent: Vec<usize> = (0..self.unknowns.len()).collect();
        fn root(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        for c in &self.constraints {
            let mut joined = self.reads(c).into_iter().filter_map(index);
            if let Some(first) = joined.next() {
                for other in joined {
                    let (a, b) = (root(&mut parent, first), root(&mut parent, other));
                    parent[a] = b;
                }
            }
        }
        let roots = (0..parent.len()).map(|i| root(&mut parent, i)).collect();
        Parts {
            system: self,
            roots,
        }
    }

    /// The part of the system `points` are in: the unknowns of their parts
    /// ([`parts`](Self::parts)), and the constraints that read them; every other value is
    /// held where it is, as a fixed value. Points that don't name a point their geometry
    /// has join nothing.
    pub fn part(&self, points: &[PointOf]) -> SketchSystem {
        let parts = self.parts();
        let mut wanted: Vec<Part> = points
            .iter()
            .filter_map(|&p| self.point_values(p))
            .flat_map(|v| [v.x, v.y])
            .filter_map(|v| parts.of(v))
            .collect();
        wanted.sort_unstable();
        wanted.dedup();
        let in_part = |v: Value| {
            parts
                .of(v)
                .is_some_and(|p| wanted.binary_search(&p).is_ok())
        };
        SketchSystem {
            values: self.values.clone(),
            unknowns: self
                .unknowns
                .iter()
                .copied()
                .filter(|&v| in_part(v))
                .collect(),
            geometry: self.geometry.clone(),
            constraints: self
                .constraints
                .iter()
                .copied()
                .filter(|c| self.reads(c).into_iter().any(in_part))
                .collect(),
            algorithms: self.algorithms.clone(),
        }
    }

    /// The values constraint `c` reads, as PlaneGCS's rows for it do.
    fn reads(&self, c: &SolverConstraint) -> Vec<Value> {
        let geometry = |g: Geometry| {
            self.geometry
                .get(g.0 as usize)
                .map(SketchGeometry::values)
                .unwrap_or_default()
        };
        let point = |p: PointOf| {
            self.point_values(p)
                .map(|v| vec![v.x, v.y])
                .unwrap_or_default()
        };
        // A circle's or arc's centre and radius.
        let round = |g: Geometry| match self.geometry.get(g.0 as usize) {
            Some(
                SketchGeometry::Circle { center, radius }
                | SketchGeometry::Arc { center, radius, .. },
            ) => vec![center.x, center.y, *radius],
            _ => Vec::new(),
        };
        match *c {
            SolverConstraint::ArcRules { arc } => geometry(arc),
            SolverConstraint::Coincident { a, b, .. } => [point(a), point(b)].concat(),
            SolverConstraint::DistanceX { a, b, value, .. }
            | SolverConstraint::DistanceY { a, b, value, .. }
            | SolverConstraint::Distance { a, b, value, .. } => {
                [point(a), point(b), vec![value]].concat()
            }
            SolverConstraint::PointOnLine { point: q, line, .. } => {
                [point(q), geometry(line)].concat()
            }
            SolverConstraint::Horizontal { line, .. } | SolverConstraint::Vertical { line, .. } => {
                geometry(line)
            }
            SolverConstraint::Parallel { a, b, .. }
            | SolverConstraint::Perpendicular { a, b, .. }
            | SolverConstraint::EqualLength { a, b, .. } => [geometry(a), geometry(b)].concat(),
            SolverConstraint::Angle { a, b, value, .. } => {
                [geometry(a), geometry(b), vec![value]].concat()
            }
            SolverConstraint::Radius { circle, value, .. } => {
                let radius = match self.geometry.get(circle.0 as usize) {
                    Some(
                        SketchGeometry::Circle { radius, .. } | SketchGeometry::Arc { radius, .. },
                    ) => vec![*radius],
                    _ => Vec::new(),
                };
                [radius, vec![value]].concat()
            }
            SolverConstraint::HorizontalPoints { a, b, .. }
            | SolverConstraint::VerticalPoints { a, b, .. } => [point(a), point(b)].concat(),
            SolverConstraint::PointLineDistance {
                point: q,
                line,
                value,
                ..
            } => [point(q), geometry(line), vec![value]].concat(),
            SolverConstraint::PointOnCircle {
                point: q, circle, ..
            } => [point(q), round(circle)].concat(),
            SolverConstraint::EqualRadius { a, b, .. }
            | SolverConstraint::TangentCircles { a, b, .. } => [round(a), round(b)].concat(),
            SolverConstraint::TangentLine { line, circle, .. } => {
                [geometry(line), round(circle)].concat()
            }
            SolverConstraint::TangentOn {
                a,
                point: q,
                b,
                angle,
                ..
            }
            | SolverConstraint::AngleViaPoint {
                a,
                b,
                point: q,
                angle,
                ..
            } => [point(q), geometry(a), geometry(b), vec![angle]].concat(),
            SolverConstraint::SymmetricLine { a, b, line, .. }
            | SolverConstraint::MidpointOnLine { a, b, line, .. } => {
                [point(a), point(b), geometry(line)].concat()
            }
            SolverConstraint::SymmetricPoint { a, b, center, .. } => {
                [point(a), point(b), point(center)].concat()
            }
        }
    }

    /// The values of the point `p` names, if its geometry has it.
    fn point_values(&self, p: PointOf) -> Option<PointValues> {
        match (self.geometry.get(p.geometry().0 as usize)?, p) {
            (SketchGeometry::Point(v), PointOf::Point(_)) => Some(*v),
            (
                SketchGeometry::Line { start, .. } | SketchGeometry::Arc { start, .. },
                PointOf::Start(_),
            ) => Some(*start),
            (
                SketchGeometry::Line { end, .. } | SketchGeometry::Arc { end, .. },
                PointOf::End(_),
            ) => Some(*end),
            (
                SketchGeometry::Circle { center, .. } | SketchGeometry::Arc { center, .. },
                PointOf::Center(_),
            ) => Some(*center),
            _ => None,
        }
    }

    /// That two points, which exist, are drawn at two places, where PlaneGCS measures a
    /// constraint by the line between them: both kinds of symmetry (its direction, scaled by
    /// its length where the system is built), a distance (whose gradient is that line's
    /// direction), and a tangency at an end lying on a round curve (whose direction there is
    /// the line from its centre). Two points at one place have no such line, so the solve
    /// would start from 0/0, and the constraint be taken to conflict.
    fn apart(&self, a: PointOf, b: PointOf, what: &str) -> Result<(), InvalidParameter> {
        match (self.place(a), self.place(b)) {
            (Some(pa), Some(pb)) if no_distance(pa, pb) => Err(invalid(
                "constraints",
                format!(
                    "{what} of {a} and {b}, drawn at one place, ({}, {}): the solver measures it by the line between them",
                    format_number(pa[0]),
                    format_number(pa[1])
                ),
            )),
            _ => Ok(()),
        }
    }

    /// That `p` isn't drawn on the centre of round geometry `round`, where `what` (worded by
    /// `why`) has no line from the centre to measure by.
    fn off_centre(
        &self,
        p: PointOf,
        round: Geometry,
        what: &str,
        why: &str,
    ) -> Result<(), InvalidParameter> {
        match (self.place(p), self.place(PointOf::Center(round))) {
            (Some(at), Some(center)) if no_distance(at, center) => Err(invalid(
                "constraints",
                format!(
                    "{what}, with {p} drawn on the centre of geometry {}, ({}, {}): {why}",
                    round.0,
                    format_number(at[0]),
                    format_number(at[1])
                ),
            )),
            _ => Ok(()),
        }
    }

    /// Where `p` is drawn, if it names a point there is.
    fn place(&self, p: PointOf) -> Option<[f64; 2]> {
        self.point_values(p).map(|v| {
            [
                self.values.get(v.x.0 as usize).copied().unwrap_or(f64::NAN),
                self.values.get(v.y.0 as usize).copied().unwrap_or(f64::NAN),
            ]
        })
    }

    /// That `p` names geometry there is, at a point that kind has: a point is only itself,
    /// a line has ends, a circle a centre, an arc all three. `what` names what names it.
    fn check_point(
        &self,
        p: PointOf,
        parameter: &'static str,
        what: &str,
    ) -> Result<(), InvalidParameter> {
        let g = p.geometry();
        let Some(k) = self.geometry.get(g.0 as usize).map(SketchGeometry::kind) else {
            return Err(invalid(
                parameter,
                format!(
                    "{what} names geometry {}, but there are {}",
                    g.0,
                    self.geometry.len()
                ),
            ));
        };
        let fits = self.point_values(p).is_some();
        if fits {
            Ok(())
        } else {
            Err(invalid(
                parameter,
                format!("{what} names {p}, but geometry {} is {}", g.0, k.word()),
            ))
        }
    }

    /// An arc's rules are four equations, the start's x and y and the end's from the
    /// centre, radius, and angle: each must involve an unknown (FreeCAD `c6de527391`).
    fn check_arc_rules(&self, arc: Geometry) -> Result<(), InvalidParameter> {
        let Some(SketchGeometry::Arc {
            center,
            radius,
            start_angle,
            end_angle,
            start,
            end,
        }) = self.geometry.get(arc.0 as usize).copied()
        else {
            return Ok(());
        };
        let unknown = |v: Value| self.unknowns.binary_search(&v).is_ok();
        for (what, row) in [
            ("start's x", [start.x, center.x, radius, start_angle]),
            ("start's y", [start.y, center.y, radius, start_angle]),
            ("end's x", [end.x, center.x, radius, end_angle]),
            ("end's y", [end.y, center.y, radius, end_angle]),
        ] {
            if !row.into_iter().any(unknown) {
                return Err(invalid(
                    "constraints",
                    format!(
                        "an arc's rules on arc {}, whose {what} equation has no unknown: rules go only on an arc the solver may change",
                        arc.0
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// Whether a value is one a sketch may hold: finite, and within a kilometre (or as many
/// radians) of 0. A solution must be too, so the next solve can start from it.
pub fn within_reach(v: f64) -> bool {
    v.is_finite() && v.abs() <= MAX_LENGTH
}

/// The square of the distance from `a` to `b`, as PlaneGCS works it out (dx² + dy², whose
/// square root is a line's length or an arc's radius to it). The rule lives here so the
/// core's gate and the sketch worker's check measure alike.
pub fn squared_distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    dx * dx + dy * dy
}

/// Whether `a` and `b` are no distance apart to the solver: at one place, or close enough
/// that their [`squared_distance`] is 0 (1e-200 apart, say), so a line between them has no
/// length and an arc about one through the other no radius.
pub fn no_distance(a: [f64; 2], b: [f64; 2]) -> bool {
    squared_distance(a, b) == 0.0
}

/// The pieces of `geometry` that values solved to `values` can't build, by their places
/// in it, in order: one with a value out of reach ([`within_reach`]), a line of no length
/// ([`no_distance`]: FreeCAD's `GeomLineSegment::setPoints`, "Both points are equal"), a
/// circle or an arc whose radius isn't above 0 (FreeCAD's `gp_Circ` refuses one below 0 and
/// keeps 0; Silico draws nothing of no size), or an arc whose start and end are at one
/// place. Which way an arc's angles went doesn't matter: FreeCAD trims the arc's circle from
/// its start angle to its end angle taken a whole number of turns past it (OCCT's periodic
/// trim), the counter-clockwise arc from its solved start to its solved end, which is how
/// Silico defines an arc. The rule the sketch worker passes a solution over by, and the
/// core names what can't be built by (docs/design/m3.3-solving.md).
pub fn unbuildable(geometry: &[SketchGeometry], values: &[f64]) -> Vec<Geometry> {
    let at = |v: Value| values.get(v.0 as usize).copied().unwrap_or(f64::NAN);
    let point = |p: PointValues| [at(p.x), at(p.y)];
    let builds = |g: &SketchGeometry| {
        g.values().into_iter().all(|v| within_reach(at(v)))
            && match *g {
                SketchGeometry::Point(_) => true,
                SketchGeometry::Line { start, end } => !no_distance(point(start), point(end)),
                SketchGeometry::Circle { radius, .. } => at(radius) > 0.0,
                SketchGeometry::Arc {
                    radius, start, end, ..
                } => at(radius) > 0.0 && !no_distance(point(start), point(end)),
            }
    };
    geometry
        .iter()
        .enumerate()
        .filter(|(_, g)| !builds(g))
        .map(|(i, _)| Geometry(i as u32))
        .collect()
}

impl SketchSystem {
    /// This system's geometry that `values` can't build ([`unbuildable`]).
    pub fn unbuildable(&self, values: &[f64]) -> Vec<Geometry> {
        unbuildable(&self.geometry, values)
    }
}

/// An arc's radius, start angle, and end angle (radians), worked out from its centre, start,
/// and end as the core starts a solve from them (docs/design/m3.3-solving.md): the
/// radius from the centre to the start (the square root of [`squared_distance`]), and the
/// angles of the start and the end about the centre (`atan2`, the `libm` crate's, so the
/// same on every platform), the end's taken a turn further when it isn't above the start's,
/// so the arc runs counter-clockwise from start to end. None when the two angles come out
/// the same: a sweep of nothing, or of a whole turn, which no arc has. That includes a start
/// and end on the circle's far side either side of −0 (`atan2` gives π and −π, and −π a turn
/// on is π again).
pub fn arc_from_points(center: [f64; 2], start: [f64; 2], end: [f64; 2]) -> Option<[f64; 3]> {
    let radius = squared_distance(center, start).sqrt();
    let angle = |p: [f64; 2]| libm::atan2(p[1] - center[1], p[0] - center[0]);
    let (start_angle, mut end_angle) = (angle(start), angle(end));
    if end_angle < start_angle {
        end_angle += std::f64::consts::TAU;
    }
    let sweep = end_angle - start_angle;
    (sweep > 0.0 && sweep < std::f64::consts::TAU).then_some([radius, start_angle, end_angle])
}

impl SolverConstraint {
    /// The tag it's reported by: every constraint but an arc's rules has one.
    pub fn tag(&self) -> Option<Tag> {
        match *self {
            SolverConstraint::ArcRules { .. } => None,
            SolverConstraint::Coincident { tag, .. }
            | SolverConstraint::Horizontal { tag, .. }
            | SolverConstraint::Vertical { tag, .. }
            | SolverConstraint::DistanceX { tag, .. }
            | SolverConstraint::DistanceY { tag, .. }
            | SolverConstraint::Distance { tag, .. }
            | SolverConstraint::PointOnLine { tag, .. }
            | SolverConstraint::Parallel { tag, .. }
            | SolverConstraint::Perpendicular { tag, .. }
            | SolverConstraint::Angle { tag, .. }
            | SolverConstraint::EqualLength { tag, .. }
            | SolverConstraint::Radius { tag, .. }
            | SolverConstraint::HorizontalPoints { tag, .. }
            | SolverConstraint::VerticalPoints { tag, .. }
            | SolverConstraint::PointLineDistance { tag, .. }
            | SolverConstraint::PointOnCircle { tag, .. }
            | SolverConstraint::EqualRadius { tag, .. }
            | SolverConstraint::TangentLine { tag, .. }
            | SolverConstraint::TangentCircles { tag, .. }
            | SolverConstraint::TangentOn { tag, .. }
            | SolverConstraint::AngleViaPoint { tag, .. }
            | SolverConstraint::SymmetricLine { tag, .. }
            | SolverConstraint::MidpointOnLine { tag, .. }
            | SolverConstraint::SymmetricPoint { tag, .. } => Some(tag),
        }
    }

    /// How many equations PlaneGCS makes of it.
    fn rows(&self) -> usize {
        match self {
            SolverConstraint::ArcRules { .. } => 4,
            SolverConstraint::Coincident { .. }
            | SolverConstraint::TangentOn { .. }
            | SolverConstraint::SymmetricLine { .. }
            | SolverConstraint::SymmetricPoint { .. } => 2,
            _ => 1,
        }
    }
}

fn constraint_word(c: &SolverConstraint) -> &'static str {
    match c {
        SolverConstraint::ArcRules { .. } => "an arc's rules",
        SolverConstraint::Coincident { .. } => "a coincidence",
        SolverConstraint::Horizontal { .. } => "a horizontal constraint",
        SolverConstraint::Vertical { .. } => "a vertical constraint",
        SolverConstraint::DistanceX { .. } => "a horizontal distance",
        SolverConstraint::DistanceY { .. } => "a vertical distance",
        SolverConstraint::Distance { .. } => "a distance",
        SolverConstraint::PointOnLine { .. } => "a point on a line",
        SolverConstraint::Parallel { .. } => "a parallel constraint",
        SolverConstraint::Perpendicular { .. } => "a perpendicular constraint",
        SolverConstraint::Angle { .. } => "an angle",
        SolverConstraint::EqualLength { .. } => "an equal length",
        SolverConstraint::Radius { .. } => "a radius",
        SolverConstraint::HorizontalPoints { .. } => "a horizontal constraint on two points",
        SolverConstraint::VerticalPoints { .. } => "a vertical constraint on two points",
        SolverConstraint::PointLineDistance { .. } => "a point's distance from a line",
        SolverConstraint::PointOnCircle { .. } => "a point on a circle",
        SolverConstraint::EqualRadius { .. } => "an equal radius",
        SolverConstraint::TangentLine { .. } => "a line's tangency",
        SolverConstraint::TangentCircles { .. } => "a tangency of circles",
        SolverConstraint::TangentOn { .. } => "a tangency at an end",
        SolverConstraint::AngleViaPoint { .. } => "an angle at a point",
        SolverConstraint::SymmetricLine { .. } => "a symmetry about a line",
        SolverConstraint::MidpointOnLine { .. } => "a midpoint on a line",
        SolverConstraint::SymmetricPoint { .. } => "a symmetry about a point",
    }
}

fn kinds_word(kinds: &[GeometryKind]) -> String {
    let words: Vec<&str> = kinds.iter().map(|k| k.word()).collect();
    words.join(" or ")
}

fn invalid(parameter: &'static str, message: String) -> InvalidParameter {
    InvalidParameter { parameter, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line from (0, 0) to (10, 0), a circle at (0, 5) of radius 2, and an arc; the
    /// line horizontal, its length 10, the circle's radius 2, and the arc's rules.
    fn system() -> SketchSystem {
        let p = |x: u32, y: u32| PointValues {
            x: Value(x),
            y: Value(y),
        };
        SketchSystem {
            // 0..4 the line, 4..7 the circle, 7..16 the arc, 16 and 17 the dimensions.
            values: vec![
                0.0, 0.0, 10.0, 0.0, 0.0, 5.0, 2.0, 1.0, 1.0, 1.0, 0.0, 1.5, 2.0, 1.0, 1.0, 2.0,
                10.0, 2.0,
            ],
            unknowns: (0..16).map(Value).collect(),
            geometry: vec![
                SketchGeometry::Line {
                    start: p(0, 1),
                    end: p(2, 3),
                },
                SketchGeometry::Circle {
                    center: p(4, 5),
                    radius: Value(6),
                },
                SketchGeometry::Arc {
                    center: p(7, 8),
                    radius: Value(9),
                    start_angle: Value(10),
                    end_angle: Value(11),
                    start: p(12, 13),
                    end: p(14, 15),
                },
            ],
            constraints: vec![
                SolverConstraint::ArcRules { arc: Geometry(2) },
                SolverConstraint::Horizontal {
                    tag: Tag(1),
                    line: Geometry(0),
                },
                SolverConstraint::Distance {
                    tag: Tag(2),
                    a: PointOf::Start(Geometry(0)),
                    b: PointOf::End(Geometry(0)),
                    value: Value(16),
                },
                SolverConstraint::Radius {
                    tag: Tag(3),
                    circle: Geometry(1),
                    value: Value(17),
                },
            ],
            algorithms: vec![Algorithm::DogLeg],
        }
    }

    /// The check's refusal of `spoil`ed [`system`], as (parameter, message).
    fn refused(spoil: impl FnOnce(&mut SketchSystem)) -> (&'static str, String) {
        let mut s = system();
        spoil(&mut s);
        let e = s.check().unwrap_err();
        (e.parameter, e.message)
    }

    #[test]
    fn each_kind_of_geometry_has_its_points_where_it_says() {
        // FreeCAD's testGetPointFromGeom* (tests/src/Mod/Sketcher/App/SketchObject.cpp): what
        // start, end, and centre are for each kind, as Silico has them. A line has its start
        // and end; an arc all three; a circle only its centre; a point only itself, as
        // `PointOf::Point`. FreeCAD differs there: a point's start, mid, and end are all the
        // point (Sketch.cpp's addPoint gives the three one id), and a circle's start and end
        // are (cx + r, cy). So an importer maps FreeCAD's (point, start) to `Point`, and has no
        // circle start or end to send. In `system()`: the line from values (0, 1) to (2, 3),
        // the circle about (4, 5), the arc about (7, 8) from (12, 13) to (14, 15); here a
        // point at (16, 17) too.
        let mut s = system();
        let pv = |x: u32, y: u32| PointValues {
            x: Value(x),
            y: Value(y),
        };
        s.geometry.push(SketchGeometry::Point(pv(16, 17)));
        let (line, circle, arc, point) = (Geometry(0), Geometry(1), Geometry(2), Geometry(3));
        let cases = [
            (PointOf::Start(line), Some(pv(0, 1))),
            (PointOf::End(line), Some(pv(2, 3))),
            (PointOf::Center(line), None),
            (PointOf::Point(line), None),
            (PointOf::Center(circle), Some(pv(4, 5))),
            (PointOf::Start(circle), None),
            (PointOf::End(circle), None),
            (PointOf::Start(arc), Some(pv(12, 13))),
            (PointOf::End(arc), Some(pv(14, 15))),
            (PointOf::Center(arc), Some(pv(7, 8))),
            (PointOf::Point(point), Some(pv(16, 17))),
            (PointOf::Start(point), None),
            (PointOf::End(point), None),
            (PointOf::Center(point), None),
            (PointOf::Point(circle), None),
            (PointOf::Point(arc), None),
            // Geometry the system doesn't have.
            (PointOf::Start(Geometry(9)), None),
        ];
        for (p, want) in cases {
            assert_eq!(s.point_values(p), want, "{p}");
        }
    }

    #[test]
    fn a_well_formed_system_passes() {
        system().check().unwrap();
    }

    #[test]
    fn values_that_arent_finite_or_are_out_of_reach_are_refused_naming_them() {
        for (bad, shown) in [
            (f64::NAN, "NaN"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (1.000_000_1e6, "1000000.1"),
        ] {
            let (parameter, message) = refused(|s| s.values[5] = bad);
            assert_eq!(parameter, "values");
            assert_eq!(
                message,
                format!("value 5 must be finite and within 1000000 of 0, got {shown}")
            );
        }
        // A kilometre is within reach.
        let mut s = system();
        s.values[5] = -1e6;
        s.check().unwrap();
    }

    #[test]
    fn unknowns_must_increase_and_name_values_there_are() {
        let (_, message) = refused(|s| s.unknowns.swap(2, 3));
        assert_eq!(
            message,
            "unknowns must be in increasing order, each once: 3 comes before 2"
        );
        let (_, message) = refused(|s| s.unknowns[4] = Value(3));
        assert!(message.contains("each once: 3 comes before 3"), "{message}");
        let (parameter, message) = refused(|s| s.unknowns.push(Value(18)));
        assert_eq!(parameter, "values");
        assert_eq!(message, "an unknown names value 18, but there are 18");
    }

    #[test]
    fn geometry_must_be_made_of_values_there_are() {
        let (_, message) = refused(|s| {
            s.geometry[1] = SketchGeometry::Circle {
                center: PointValues {
                    x: Value(4),
                    y: Value(5),
                },
                radius: Value(40),
            };
        });
        assert_eq!(message, "geometry 1 names value 40, but there are 18");
    }

    #[test]
    fn constraints_must_name_geometry_there_is_of_the_kinds_they_take() {
        let (parameter, message) = refused(|s| {
            s.constraints[1] = SolverConstraint::Horizontal {
                tag: Tag(1),
                line: Geometry(3),
            };
        });
        assert_eq!(parameter, "constraints");
        assert_eq!(
            message,
            "constraint 1 (tag 1): a horizontal constraint names geometry 3, but there are 3"
        );
        let (_, message) = refused(|s| {
            s.constraints[1] = SolverConstraint::Horizontal {
                tag: Tag(1),
                line: Geometry(1),
            };
        });
        assert_eq!(
            message,
            "constraint 1 (tag 1): a horizontal constraint needs a line, but geometry 1 is a circle"
        );
        let (_, message) = refused(|s| {
            s.constraints[3] = SolverConstraint::Radius {
                tag: Tag(3),
                circle: Geometry(0),
                value: Value(17),
            };
        });
        assert_eq!(
            message,
            "constraint 3 (tag 3): a radius needs a circle or an arc, but geometry 0 is a line"
        );
        let (_, message) =
            refused(|s| s.constraints[0] = SolverConstraint::ArcRules { arc: Geometry(1) });
        assert_eq!(
            message,
            "constraint 0: an arc's rules needs an arc, but geometry 1 is a circle"
        );
    }

    #[test]
    fn points_are_named_in_words() {
        assert_eq!(PointOf::Point(Geometry(3)).to_string(), "point 3");
        assert_eq!(
            PointOf::Start(Geometry(0)).to_string(),
            "the start of geometry 0"
        );
        assert_eq!(
            PointOf::End(Geometry(7)).to_string(),
            "the end of geometry 7"
        );
        assert_eq!(
            PointOf::Center(Geometry(2)).to_string(),
            "the centre of geometry 2"
        );
        let (_, message) = refused(|s| {
            s.constraints[2] = SolverConstraint::Distance {
                tag: Tag(2),
                a: PointOf::Start(Geometry(0)),
                b: PointOf::End(Geometry(1)),
                value: Value(16),
            };
        });
        assert_eq!(
            message,
            "constraint 2 (tag 2): a distance names the end of geometry 1, but geometry 1 is a circle"
        );
    }

    #[test]
    fn points_must_be_points_the_geometry_has() {
        // A circle has a centre but no ends; a line has ends but no centre; a point is
        // only itself.
        for (p, kind) in [
            (PointOf::Start(Geometry(1)), "a circle"),
            (PointOf::End(Geometry(1)), "a circle"),
            (PointOf::Center(Geometry(0)), "a line"),
            (PointOf::Point(Geometry(0)), "a line"),
        ] {
            let (_, message) = refused(|s| {
                s.constraints[2] = SolverConstraint::Distance {
                    tag: Tag(2),
                    a: PointOf::Start(Geometry(0)),
                    b: p,
                    value: Value(16),
                };
            });
            assert_eq!(
                message,
                format!(
                    "constraint 2 (tag 2): a distance names {p}, but geometry {} is {kind}",
                    p.geometry().0
                )
            );
        }
        // The arc has all three.
        for p in [
            PointOf::Start(Geometry(2)),
            PointOf::End(Geometry(2)),
            PointOf::Center(Geometry(2)),
        ] {
            let mut s = system();
            s.constraints[2] = SolverConstraint::Distance {
                tag: Tag(2),
                a: PointOf::Start(Geometry(0)),
                b: p,
                value: Value(16),
            };
            s.check().unwrap();
        }
    }

    #[test]
    fn a_dimensions_value_must_be_a_fixed_value_there_is() {
        let (_, message) = refused(|s| {
            s.constraints[2] = SolverConstraint::Distance {
                tag: Tag(2),
                a: PointOf::Start(Geometry(0)),
                b: PointOf::End(Geometry(0)),
                value: Value(3),
            };
        });
        assert_eq!(
            message,
            "constraint 2 (tag 2): a distance's value 3 is an unknown: a dimension's value must be fixed"
        );
        let (_, message) = refused(|s| {
            s.constraints[3] = SolverConstraint::Radius {
                tag: Tag(3),
                circle: Geometry(1),
                value: Value(18),
            };
        });
        assert_eq!(
            message,
            "constraint 3 (tag 3): a radius names value 18, but there are 18"
        );
    }

    #[test]
    fn tags_start_at_one_and_each_is_used_once() {
        let (_, message) = refused(|s| {
            s.constraints[1] = SolverConstraint::Horizontal {
                tag: Tag(0),
                line: Geometry(0),
            };
        });
        assert_eq!(
            message,
            "constraint 1 (tag 0): a horizontal constraint's tag must be from 1 to 2147483647 (the solver never reports tag 0, and takes tags as C ints), got 0"
        );
        let (_, message) = refused(|s| {
            s.constraints[3] = SolverConstraint::Radius {
                tag: Tag(1),
                circle: Geometry(1),
                value: Value(17),
            };
        });
        assert_eq!(message, "tag 1 is used twice: each constraint has its own");
    }

    #[test]
    fn an_arcs_rules_need_an_unknown_in_each_equation() {
        // FreeCAD c6de527391: rules on a fixed arc are rows of zeros that count against the
        // degrees of freedom (a fixed arc and a point on its start: dofs -4, not 0).
        let (_, message) = refused(|s| s.unknowns.retain(|v| !(7..16).contains(&v.0)));
        assert_eq!(
            message,
            "constraint 0: an arc's rules on arc 2, whose start's x equation has no unknown: rules go only on an arc the solver may change"
        );
        // Each equation on its own: the arc's values are the centre (7, 8), the radius (9),
        // the angles (10, 11), the start (12, 13), and the end (14, 15).
        for (fixed, row) in [
            ([7, 9, 10, 12], "start's x"),
            ([8, 9, 10, 13], "start's y"),
            ([7, 9, 11, 14], "end's x"),
            ([8, 9, 11, 15], "end's y"),
        ] {
            let (_, message) = refused(|s| s.unknowns.retain(|v| !fixed.contains(&v.0)));
            assert!(
                message.contains(&format!("whose {row} equation has no unknown")),
                "{row}: {message}"
            );
        }
        // An unknown in each is enough: only the start and end points free (12 to 15), the
        // centre, radius, and angles fixed.
        let mut s = system();
        s.unknowns.retain(|v| !(7..12).contains(&v.0));
        s.check().unwrap();
    }

    #[test]
    fn algorithms_are_one_to_four_different_ones() {
        use Algorithm::{AnchoredSqp, Bfgs, DogLeg, LevenbergMarquardt};
        for algorithms in [
            vec![DogLeg],
            vec![LevenbergMarquardt, DogLeg],
            vec![DogLeg, LevenbergMarquardt, Bfgs],
            vec![DogLeg, LevenbergMarquardt, Bfgs, AnchoredSqp],
            vec![AnchoredSqp],
        ] {
            let mut s = system();
            s.algorithms = algorithms;
            s.check().unwrap();
        }
        for algorithms in [
            vec![],
            vec![DogLeg, DogLeg],
            vec![Bfgs, DogLeg, Bfgs, LevenbergMarquardt],
            vec![AnchoredSqp, AnchoredSqp],
            vec![DogLeg, LevenbergMarquardt, Bfgs, AnchoredSqp, DogLeg],
        ] {
            let (parameter, message) = refused(|s| s.algorithms = algorithms.clone());
            assert_eq!(parameter, "algorithms");
            assert_eq!(
                message,
                format!("algorithms must be one to four different ones, got {algorithms:?}")
            );
        }
    }

    #[test]
    fn an_arcs_rules_are_given_once() {
        let (_, message) = refused(|s| {
            s.constraints
                .push(SolverConstraint::ArcRules { arc: Geometry(2) })
        });
        assert_eq!(message, "arc 2's rules are given twice");
    }

    #[test]
    fn sizes_past_the_limits_are_refused() {
        let (parameter, message) = refused(|s| s.values.resize(MAX_SKETCH_VALUES + 1, 0.0));
        assert_eq!(parameter, "values");
        assert_eq!(
            message,
            format!("a sketch may have at most {MAX_SKETCH_VALUES} values, got 10001")
        );
        let mut s = system();
        s.values.resize(MAX_SKETCH_VALUES, 0.0);
        s.check().unwrap();

        let horizontal = |t: u32| SolverConstraint::Horizontal {
            tag: Tag(t),
            line: Geometry(0),
        };
        let (parameter, message) = refused(|s| {
            s.constraints = (1..=MAX_SKETCH_CONSTRAINTS as u32 + 1)
                .map(horizontal)
                .collect();
        });
        assert_eq!(parameter, "constraints");
        assert_eq!(
            message,
            format!("a sketch may have at most {MAX_SKETCH_CONSTRAINTS} constraints, got 10001")
        );
    }

    #[test]
    fn each_two_equation_constraint_is_two_rows_of_the_jacobian() {
        // Found by the mutation run. A coincidence, a tangency at an end, and the two
        // symmetries are two equations each. 8,191 arcs' rules are 4 × 8,191 = 32,764 rows
        // on 1,024 unknowns; two more of a two-equation kind make 32,768, 2^25 entries
        // exactly, and three 32,770, past it (as one row each, three would be 32,767, within
        // it). Every value is an unknown but the last, the tangency's fixed angle.
        let (line, circle) = (Geometry(0), Geometry(1));
        let (start, end) = (PointOf::Start(line), PointOf::End(line));
        let two_rows = |kind: usize, t: u32| {
            let tag = Tag(t);
            match kind {
                0 => SolverConstraint::Coincident {
                    tag,
                    a: start,
                    b: PointOf::Center(circle),
                },
                1 => SolverConstraint::TangentOn {
                    tag,
                    a: line,
                    point: start,
                    b: circle,
                    angle: Value(MAX_SKETCH_UNKNOWNS as u32),
                },
                2 => SolverConstraint::SymmetricLine {
                    tag,
                    a: start,
                    b: end,
                    line,
                },
                _ => SolverConstraint::SymmetricPoint {
                    tag,
                    a: start,
                    b: end,
                    center: PointOf::Center(circle),
                },
            }
        };
        let with = |kind: usize, count: u32| {
            let mut s = system();
            s.values.resize(MAX_SKETCH_UNKNOWNS + 1, 1.0);
            s.unknowns = (0..MAX_SKETCH_UNKNOWNS as u32).map(Value).collect();
            let arc = s.geometry[2];
            s.geometry.extend((1..8191).map(|_| arc));
            s.constraints = (0..8191)
                .map(|i| SolverConstraint::ArcRules {
                    arc: Geometry(2 + i),
                })
                .collect();
            s.constraints.extend((1..=count).map(|t| two_rows(kind, t)));
            s
        };
        for kind in 0..4 {
            with(kind, 2).check().unwrap();
            let e = with(kind, 3).check().unwrap_err();
            assert_eq!(e.parameter, "constraints", "kind {kind}");
            assert_eq!(
                e.message,
                format!(
                    "32770 equations in 1024 unknowns is more than the solver can diagnose (at most {MAX_JACOBIAN_ENTRIES} entries)"
                ),
                "kind {kind}"
            );
        }
    }

    #[test]
    fn a_jacobian_past_the_limit_is_refused_and_one_at_it_passes() {
        // The limit is on rows × unknowns, PlaneGCS's dense Jacobian. An arc's rules are four
        // rows: 8,192 arcs (one geometry each, sharing the base arc's values), each with its
        // rules, on 1,024 unknowns is 2^25 exactly; one more is past it.
        let with = |arcs: u32| {
            let mut s = system();
            s.values.resize(MAX_SKETCH_UNKNOWNS, 1.0);
            s.unknowns = (0..MAX_SKETCH_UNKNOWNS as u32).map(Value).collect();
            let arc = s.geometry[2];
            s.geometry.extend((1..arcs).map(|_| arc));
            s.constraints = (0..arcs)
                .map(|i| SolverConstraint::ArcRules {
                    arc: Geometry(2 + i),
                })
                .collect();
            s
        };
        assert_eq!(4 * 8192 * MAX_SKETCH_UNKNOWNS, MAX_JACOBIAN_ENTRIES);
        with(8192).check().unwrap();
        let e = with(8193).check().unwrap_err();
        assert_eq!(e.parameter, "constraints");
        assert_eq!(
            e.message,
            format!(
                "32772 equations in 1024 unknowns is more than the solver can diagnose (at most {MAX_JACOBIAN_ENTRIES} entries)"
            )
        );
    }

    #[test]
    fn unknowns_and_geometry_past_their_limits_are_refused() {
        // Every value an unknown but the two dimensions' (16 and 17).
        let mut s = system();
        s.values.resize(MAX_SKETCH_UNKNOWNS + 3, 1.0);
        s.unknowns = (0..MAX_SKETCH_UNKNOWNS as u32 + 2)
            .filter(|v| !matches!(v, 16 | 17))
            .map(Value)
            .collect();
        s.check().unwrap();
        s.unknowns.push(Value(MAX_SKETCH_UNKNOWNS as u32 + 2));
        let e = s.check().unwrap_err();
        assert_eq!(e.parameter, "unknowns");
        assert_eq!(
            e.message,
            format!("a sketch may have at most {MAX_SKETCH_UNKNOWNS} unknowns, got 1025")
        );

        let mut s = system();
        let point = SketchGeometry::Point(PointValues {
            x: Value(0),
            y: Value(1),
        });
        s.geometry.resize(MAX_SKETCH_GEOMETRY, point);
        s.check().unwrap();
        s.geometry.push(point);
        let e = s.check().unwrap_err();
        assert_eq!(e.parameter, "geometry");
        assert_eq!(
            e.message,
            format!(
                "a sketch may have at most {MAX_SKETCH_GEOMETRY} pieces of geometry, got 10001"
            )
        );
    }

    #[test]
    fn a_tag_past_a_c_int_is_refused_and_the_largest_passes() {
        // PlaneGCS takes tags as C ints, and a negative one drops out of its diagnosis.
        let tagged = |t: u32| {
            let mut s = system();
            s.constraints[1] = SolverConstraint::Horizontal {
                tag: Tag(t),
                line: Geometry(0),
            };
            s
        };
        tagged(MAX_TAG).check().unwrap();
        for t in [MAX_TAG + 1, u32::MAX] {
            let e = tagged(t).check().unwrap_err();
            assert_eq!(e.parameter, "constraints");
            assert_eq!(
                e.message,
                format!(
                    "constraint 1 (tag {t}): a horizontal constraint's tag must be from 1 to 2147483647 (the solver never reports tag 0, and takes tags as C ints), got {t}"
                )
            );
        }
    }

    #[test]
    fn a_line_drawn_with_no_length_is_refused_and_one_of_any_length_passes() {
        let mut s = system();
        s.values[2] = 0.0;
        let e = s.check().unwrap_err();
        assert_eq!(e.parameter, "geometry");
        assert_eq!(
            e.message,
            "geometry 0 is a line drawn with no length, its ends at (0, 0) and (0, 0)"
        );
        // 1e-200 squared underflows to 0: no length to the solver.
        s.values[2] = 1e-200;
        let e = s.check().unwrap_err();
        assert!(
            e.message
                .starts_with("geometry 0 is a line drawn with no length")
        );
        // 1e-150 squared is 1e-300: a length.
        s.values[2] = 1e-150;
        s.check().unwrap();
        // 1e-160 squared is 1e-320, below the smallest normal float but not 0: a length.
        s.values[2] = 1e-160;
        s.check().unwrap();
    }

    fn drag(point: PointOf) -> DragBegin {
        DragBegin {
            system: system(),
            point,
        }
    }

    #[test]
    fn a_drag_of_a_point_the_geometry_has_passes() {
        // The line's ends, the circle's centre, and the arc's ends and centre.
        for point in [
            PointOf::Start(Geometry(0)),
            PointOf::End(Geometry(0)),
            PointOf::Center(Geometry(1)),
            PointOf::Start(Geometry(2)),
            PointOf::End(Geometry(2)),
            PointOf::Center(Geometry(2)),
        ] {
            drag(point).check().unwrap();
        }
        let mut with_point = drag(PointOf::Point(Geometry(3)));
        with_point
            .system
            .geometry
            .push(SketchGeometry::Point(PointValues {
                x: Value(0),
                y: Value(1),
            }));
        with_point.check().unwrap();
    }

    #[test]
    fn a_drag_of_geometry_there_isnt_or_a_point_it_lacks_is_refused_naming_the_point() {
        for (point, words) in [
            (
                PointOf::Start(Geometry(3)),
                "the drag names geometry 3, but there are 3",
            ),
            (
                PointOf::Point(Geometry(u32::MAX)),
                "the drag names geometry 4294967295, but there are 3",
            ),
            (
                PointOf::Center(Geometry(0)),
                "the drag names the centre of geometry 0, but geometry 0 is a line",
            ),
            (
                PointOf::Point(Geometry(0)),
                "the drag names point 0, but geometry 0 is a line",
            ),
            (
                PointOf::Start(Geometry(1)),
                "the drag names the start of geometry 1, but geometry 1 is a circle",
            ),
            (
                PointOf::End(Geometry(1)),
                "the drag names the end of geometry 1, but geometry 1 is a circle",
            ),
            (
                PointOf::Point(Geometry(2)),
                "the drag names point 2, but geometry 2 is an arc",
            ),
        ] {
            let e = drag(point).check().unwrap_err();
            assert_eq!((e.parameter, e.message.as_str()), ("point", words));
        }
    }

    #[test]
    fn a_drag_checks_its_system_first() {
        // A system that fails its check is refused as a solve's would be, whatever the
        // point.
        for point in [PointOf::Start(Geometry(0)), PointOf::Start(Geometry(9))] {
            let mut d = drag(point);
            d.system.values[5] = f64::NAN;
            let e = d.check().unwrap_err();
            assert_eq!(e.parameter, "values");
            assert_eq!(
                e.message,
                "value 5 must be finite and within 1000000 of 0, got NaN"
            );
        }
    }

    #[test]
    fn a_drag_target_must_be_finite_and_within_reach() {
        for to in [[0.0, 0.0], [-1e6, 1e6], [f64::MIN_POSITIVE, -0.0]] {
            check_drag_target(to).unwrap();
        }
        for (to, words) in [
            (
                [f64::NAN, 0.0],
                "the drag's target's x must be finite and within 1000000 of 0, got NaN",
            ),
            (
                [0.0, f64::INFINITY],
                "the drag's target's y must be finite and within 1000000 of 0, got inf",
            ),
            (
                [1.0, f64::NEG_INFINITY],
                "the drag's target's y must be finite and within 1000000 of 0, got -inf",
            ),
            (
                [-1.000_000_1e6, 0.0],
                "the drag's target's x must be finite and within 1000000 of 0, got -1000000.1",
            ),
            (
                [f64::NAN, f64::NAN],
                "the drag's target's x must be finite and within 1000000 of 0, got NaN",
            ),
        ] {
            let e = check_drag_target(to).unwrap_err();
            assert_eq!((e.parameter, e.message.as_str()), ("to", words), "{to:?}");
        }
    }

    #[test]
    fn a_system_crosses_the_wire_as_it_was() {
        let s = system();
        let bytes = postcard::to_stdvec(&s).unwrap();
        let back: SketchSystem = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, s);
    }

    /// Two free lines, l0 (values 0..4) and l1 (4..8), a free circle (8..11), and a free
    /// arc (11..20), with one constraint `c` (values from 20 fixed), dimensions at 20.
    fn two_lines_with(c: Option<SolverConstraint>) -> SketchSystem {
        let p = |x: u32, y: u32| PointValues {
            x: Value(x),
            y: Value(y),
        };
        let mut values: Vec<f64> = (0..20).map(|i| f64::from(i) + 1.0).collect();
        values.push(3.0);
        SketchSystem {
            values,
            unknowns: (0..20).map(Value).collect(),
            geometry: vec![
                SketchGeometry::Line {
                    start: p(0, 1),
                    end: p(2, 3),
                },
                SketchGeometry::Line {
                    start: p(4, 5),
                    end: p(6, 7),
                },
                SketchGeometry::Circle {
                    center: p(8, 9),
                    radius: Value(10),
                },
                SketchGeometry::Arc {
                    center: p(11, 12),
                    radius: Value(13),
                    start_angle: Value(14),
                    end_angle: Value(15),
                    start: p(16, 17),
                    end: p(18, 19),
                },
            ],
            constraints: c.into_iter().collect(),
            algorithms: vec![Algorithm::DogLeg],
        }
    }

    #[test]
    fn a_part_is_what_each_kind_of_constraint_joins() {
        let (l0, l1, c, a) = (Geometry(0), Geometry(1), Geometry(2), Geometry(3));
        let (t, v) = (Tag(1), Value(20));
        let end = PointOf::End(l0);
        // Counted by hand: l0's end alone is 2; with l0's start 4; with l1's start 4; with all
        // of l1 6; both lines 8.
        let cases = [
            (None, end, 2),
            (
                Some(SolverConstraint::Coincident {
                    tag: t,
                    a: end,
                    b: PointOf::Start(l1),
                }),
                end,
                4,
            ),
            (
                Some(SolverConstraint::DistanceX {
                    tag: t,
                    a: end,
                    b: PointOf::Start(l1),
                    value: v,
                }),
                end,
                4,
            ),
            (
                Some(SolverConstraint::DistanceY {
                    tag: t,
                    a: end,
                    b: PointOf::Start(l1),
                    value: v,
                }),
                end,
                4,
            ),
            (
                Some(SolverConstraint::Distance {
                    tag: t,
                    a: PointOf::Start(l0),
                    b: end,
                    value: v,
                }),
                end,
                4,
            ),
            (
                Some(SolverConstraint::PointOnLine {
                    tag: t,
                    point: end,
                    line: l1,
                }),
                end,
                6,
            ),
            (
                Some(SolverConstraint::Horizontal { tag: t, line: l0 }),
                end,
                4,
            ),
            (
                Some(SolverConstraint::Vertical { tag: t, line: l0 }),
                end,
                4,
            ),
            (
                Some(SolverConstraint::Parallel {
                    tag: t,
                    a: l0,
                    b: l1,
                }),
                end,
                8,
            ),
            (
                Some(SolverConstraint::Perpendicular {
                    tag: t,
                    a: l0,
                    b: l1,
                }),
                end,
                8,
            ),
            (
                Some(SolverConstraint::EqualLength {
                    tag: t,
                    a: l0,
                    b: l1,
                }),
                end,
                8,
            ),
            (
                Some(SolverConstraint::Angle {
                    tag: t,
                    a: l0,
                    b: l1,
                    value: v,
                }),
                end,
                8,
            ),
            // A radius reads the radius only: the circle's centre stays its own 2.
            (
                Some(SolverConstraint::Radius {
                    tag: t,
                    circle: c,
                    value: v,
                }),
                PointOf::Center(c),
                2,
            ),
            // An arc's rules read all nine of its values.
            (
                Some(SolverConstraint::ArcRules { arc: a }),
                PointOf::End(a),
                9,
            ),
            (None, PointOf::End(a), 2),
        ];
        for (c, point, want) in cases {
            let s = two_lines_with(c);
            s.check().unwrap();
            assert_eq!(s.part(&[point]).unknowns.len(), want, "{c:?}");
        }
        // Two points' parts together, and a point its geometry hasn't, which joins nothing.
        let s = two_lines_with(None);
        assert_eq!(s.part(&[end, PointOf::Start(l1)]).unknowns.len(), 4);
        assert_eq!(s.part(&[PointOf::Center(l0)]).unknowns.len(), 0);
        // The part keeps every value, held, and only the constraints that read it.
        let s = two_lines_with(Some(SolverConstraint::Horizontal { tag: t, line: l1 }));
        let part = s.part(&[end]);
        assert_eq!(part.values, s.values);
        assert_eq!(part.unknowns, [Value(2), Value(3)]);
        assert!(part.constraints.is_empty());
    }

    /// The origin (values 0 and 1, fixed), p1 (2, 3) and p2 (4, 5), the unknowns, each held
    /// from the origin by its x and y (tags 1 to 4); and, if `joined`, a distance between
    /// them (tag 5). Every dimension's value is 6, fixed.
    fn two_held_points(joined: bool) -> (SketchSystem, SolverConstraint) {
        let (origin, p1, p2) = (Geometry(0), Geometry(1), Geometry(2));
        let point = |x: u32| {
            SketchGeometry::Point(PointValues {
                x: Value(x),
                y: Value(x + 1),
            })
        };
        let v = Value(6);
        let held = |tag: u32, p: Geometry| {
            [
                SolverConstraint::DistanceX {
                    tag: Tag(tag),
                    a: PointOf::Point(origin),
                    b: PointOf::Point(p),
                    value: v,
                },
                SolverConstraint::DistanceY {
                    tag: Tag(tag + 1),
                    a: PointOf::Point(origin),
                    b: PointOf::Point(p),
                    value: v,
                },
            ]
        };
        let between = SolverConstraint::Distance {
            tag: Tag(5),
            a: PointOf::Point(p1),
            b: PointOf::Point(p2),
            value: v,
        };
        let mut constraints = [held(1, p1), held(3, p2)].concat();
        if joined {
            constraints.push(between);
        }
        let s = SketchSystem {
            values: vec![0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 6.0],
            unknowns: (2..6).map(Value).collect(),
            geometry: vec![point(0), point(2), point(4)],
            constraints,
            algorithms: vec![Algorithm::DogLeg],
        };
        (s, between)
    }

    #[test]
    fn parts_are_unknowns_joined_by_constraints_and_fixed_values_join_nothing() {
        // Each point held from the origin: two parts, each a point's x and y; the origin's
        // values and the dimensions' are fixed, so they're no part and don't join the two.
        let (s, between) = two_held_points(false);
        s.check().unwrap();
        let parts = s.parts();
        assert_eq!(parts.of(Value(0)), None, "the origin's x");
        assert_eq!(parts.of(Value(1)), None, "the origin's y");
        assert_eq!(parts.of(Value(6)), None, "a dimension's value");
        let (x1, y1, x2, y2) = (
            parts.of(Value(2)),
            parts.of(Value(3)),
            parts.of(Value(4)),
            parts.of(Value(5)),
        );
        assert!(x1.is_some() && x2.is_some());
        assert_eq!(x1, y1);
        assert_eq!(x2, y2);
        assert_ne!(x1, x2);
        // A row's part is its unknowns' part, wherever the fixed values it reads come.
        assert_eq!(parts.of_constraint(&s.constraints[0]), x1);
        assert_eq!(parts.of_constraint(&s.constraints[3]), x2);
        // Not sent, the distance would bridge the two; a row is read as it is, so it's the
        // part of the first unknown it reads, p1's here.
        assert_eq!(parts.of_constraint(&between), x1);
        // A row that reads no unknown: none.
        let fixed = SolverConstraint::DistanceX {
            tag: Tag(6),
            a: PointOf::Point(Geometry(0)),
            b: PointOf::Point(Geometry(0)),
            value: Value(6),
        };
        assert_eq!(parts.of_constraint(&fixed), None);
        // The drag's part of p1 is p1's two unknowns and its two rows.
        let part = s.part(&[PointOf::Point(Geometry(1))]);
        assert_eq!(part.unknowns, [Value(2), Value(3)]);
        assert_eq!(part.constraints, s.constraints[..2]);

        // With the distance sent: one part, the distance's too.
        let (s, between) = two_held_points(true);
        s.check().unwrap();
        let parts = s.parts();
        let one = parts.of(Value(2));
        assert!(one.is_some());
        for v in 3..6 {
            assert_eq!(parts.of(Value(v)), one, "{v}");
        }
        assert_eq!(parts.of_constraint(&between), one);
        assert_eq!(parts.of_constraint(&s.constraints[3]), one);
        assert_eq!(parts.of(Value(0)), None);
        let part = s.part(&[PointOf::Point(Geometry(1))]);
        assert_eq!(part.unknowns.len(), 4);
        assert_eq!(part.constraints, s.constraints);
    }

    #[test]
    fn an_arcs_radius_and_angles_come_from_its_points_counter_clockwise() {
        use std::f64::consts::{FRAC_PI_2, PI, TAU};
        // A quarter from (1, 0) to (0, 1) about the origin; the radius is the start's
        // distance, whatever the end's.
        assert_eq!(
            arc_from_points([0.0, 0.0], [1.0, 0.0], [0.0, 2.0]),
            Some([1.0, 0.0, FRAC_PI_2])
        );
        // From (−1, 0) at π to (0, −1) at −π/2: the end taken a turn on, so the arc runs
        // counter-clockwise, a quarter.
        assert_eq!(
            arc_from_points([0.0, 0.0], [-1.0, 0.0], [0.0, -1.0]),
            Some([1.0, PI, -FRAC_PI_2 + TAU])
        );
        // The other way round, three quarters.
        assert_eq!(
            arc_from_points([0.0, 0.0], [0.0, 1.0], [1.0, 0.0]),
            Some([1.0, FRAC_PI_2, TAU])
        );
        // About another centre: the start 5 off at (4, 3), the end at (−3, 4).
        let [r, a, b] = arc_from_points([1.0, 1.0], [5.0, 4.0], [-2.0, 5.0]).unwrap();
        assert_eq!(r, 5.0);
        assert!((a - 0.643_501_108_793_284_4).abs() < 1e-15, "{a}");
        assert!((b - 2.214_297_435_588_181).abs() < 1e-15, "{b}");
    }

    #[test]
    fn an_arc_whose_ends_are_at_one_angle_off_the_axes_has_none() {
        // Found by the mutation run. Both ends at 3π/4 (atan2(1, −1) and atan2(2, −2), each
        // libm's PI − (atan(1) − PI_LO) = 2.356194490192345): no sweep. Were one angle taken a
        // turn on, 3π/4 + TAU rounds to 8.63937979737193, and less 3π/4 that's
        // 6.283185307179585, below TAU: a whole turn that would pass as an arc.
        assert_eq!(arc_from_points([0.0, 0.0], [-1.0, 1.0], [-2.0, 2.0]), None);
        assert_eq!(arc_from_points([0.0, 0.0], [-1.0, 1.0], [-1.0, 1.0]), None);
    }

    #[test]
    fn an_arc_whose_ends_are_at_one_angle_has_none() {
        // The end further out along the start's direction: no sweep, or a whole turn.
        assert_eq!(arc_from_points([0.0, 0.0], [2.0, 0.0], [1.0, 0.0]), None);
        assert_eq!(arc_from_points([1.0, 1.0], [2.0, 2.0], [3.0, 3.0]), None);
        // On the far side, either side of −0: atan2 gives π and −π, a turn apart.
        assert_eq!(arc_from_points([0.0, 0.0], [-1.0, 0.0], [-2.0, -0.0]), None);
        assert_eq!(arc_from_points([0.0, 0.0], [-1.0, -0.0], [-2.0, 0.0]), None);
        // Just past it either way is an arc: nearly all of the circle, or nearly none.
        let tiny = arc_from_points([0.0, 0.0], [1.0, 0.0], [1.0, 1e-9]).unwrap();
        assert!(
            tiny[2] - tiny[1] > 0.0 && tiny[2] - tiny[1] < 1e-8,
            "{tiny:?}"
        );
        let most = arc_from_points([0.0, 0.0], [1.0, 1e-9], [1.0, 0.0]).unwrap();
        assert!(most[2] - most[1] > std::f64::consts::TAU - 1e-8, "{most:?}");
    }

    #[test]
    fn a_distance_or_a_tangency_on_a_round_curve_between_points_at_one_place_is_refused() {
        // A distance's gradient is the line between its points, and a round curve's
        // direction at a point the line from its centre: none for two at one place (the M3
        // review's RR1). The line runs (0, 0) to (10, 0); the circle's centre is moved onto
        // the line's start for the distance, onto its end for the tangency, and so is the
        // arc's for the tangency on it.
        let distance = |s: &mut SketchSystem| {
            s.values[4] = 0.0;
            s.values[5] = 0.0;
            s.constraints.push(SolverConstraint::Distance {
                tag: Tag(4),
                a: PointOf::Start(Geometry(0)),
                b: PointOf::Center(Geometry(1)),
                value: Value(16),
            });
        };
        let tangency = |on: u32| {
            move |s: &mut SketchSystem| {
                let at = if on == 1 { 4 } else { 7 };
                s.values[at] = 10.0;
                s.values[at + 1] = 0.0;
                s.constraints.push(SolverConstraint::TangentOn {
                    tag: Tag(4),
                    a: Geometry(0),
                    point: PointOf::End(Geometry(0)),
                    b: Geometry(on),
                    angle: Value(17),
                });
            }
        };
        let (parameter, message) = refused(distance);
        assert_eq!(parameter, "constraints");
        assert_eq!(
            message,
            "constraint 4 (tag 4): a distance of the start of geometry 0 and the centre of geometry 1, drawn at one place, (0, 0): the solver measures it by the line between them"
        );
        for on in [1, 2] {
            let (parameter, message) = refused(tangency(on));
            assert_eq!(parameter, "constraints");
            assert_eq!(
                message,
                format!(
                    "constraint 4 (tag 4): a tangency at an end, with the end of geometry 0 drawn on the centre of geometry {on}, (10, 0): the solver takes the curve's direction there by the line from its centre"
                )
            );
        }
        // A point held on a circle or an arc, drawn at its centre: the line's start (0, 0),
        // the circle's or the arc's centre moved onto it.
        for (on, at) in [(1u32, 4usize), (2, 7)] {
            let (parameter, message) = refused(|s| {
                s.values[at] = 0.0;
                s.values[at + 1] = 0.0;
                s.constraints.push(SolverConstraint::PointOnCircle {
                    tag: Tag(4),
                    point: PointOf::Start(Geometry(0)),
                    circle: Geometry(on),
                });
            });
            assert_eq!(parameter, "constraints");
            assert_eq!(
                message,
                format!(
                    "constraint 4 (tag 4): a point on a circle, with the start of geometry 0 drawn on the centre of geometry {on}, (0, 0): the solver measures it by the line from the centre"
                )
            );
        }
        // Apart, each is a system the worker takes: the distance's points (0, 0) and (0, 5),
        // the tangency's end (10, 0) and the circle's centre (0, 5), or the arc's (1, 1).
        for spoil in [
            Box::new(|s: &mut SketchSystem| {
                s.constraints.push(SolverConstraint::Distance {
                    tag: Tag(4),
                    a: PointOf::Start(Geometry(0)),
                    b: PointOf::Center(Geometry(1)),
                    value: Value(16),
                });
            }) as Box<dyn Fn(&mut SketchSystem)>,
            Box::new(|s: &mut SketchSystem| {
                for on in [1, 2] {
                    s.constraints.push(SolverConstraint::PointOnCircle {
                        tag: Tag(10 + on),
                        point: PointOf::Start(Geometry(0)),
                        circle: Geometry(on),
                    });
                }
            }),
            Box::new(|s: &mut SketchSystem| {
                for on in [1, 2] {
                    s.constraints.push(SolverConstraint::TangentOn {
                        tag: Tag(4 + on),
                        a: Geometry(0),
                        point: PointOf::End(Geometry(0)),
                        b: Geometry(on),
                        angle: Value(17),
                    });
                }
            }),
        ] {
            let mut s = system();
            spoil(&mut s);
            s.check().unwrap();
        }
        // A tangency on a line at an end of another measures by the line's own direction:
        // its end on the line's is no refusal.
        let mut s = system();
        s.geometry.push(SketchGeometry::Line {
            start: PointValues {
                x: Value(2),
                y: Value(3),
            },
            end: PointValues {
                x: Value(4),
                y: Value(5),
            },
        });
        s.constraints.push(SolverConstraint::TangentOn {
            tag: Tag(4),
            a: Geometry(0),
            point: PointOf::End(Geometry(0)),
            b: Geometry(3),
            angle: Value(17),
        });
        s.check().unwrap();
    }

    #[test]
    fn a_symmetry_of_two_points_drawn_at_one_place_is_refused() {
        // The solver measures a symmetry by the line between its points: none for two at
        // one place. The circle's centre moved onto the line's start, (0, 0).
        for (word, about) in [
            ("a symmetry about a line", Some(Geometry(0))),
            ("a symmetry about a point", None),
        ] {
            let (parameter, message) = refused(|s| {
                s.values[4] = 0.0;
                s.values[5] = 0.0;
                let (a, b) = (PointOf::Start(Geometry(0)), PointOf::Center(Geometry(1)));
                s.constraints.push(match about {
                    Some(line) => SolverConstraint::SymmetricLine {
                        tag: Tag(4),
                        a,
                        b,
                        line,
                    },
                    None => SolverConstraint::SymmetricPoint {
                        tag: Tag(4),
                        a,
                        b,
                        center: PointOf::End(Geometry(0)),
                    },
                });
            });
            assert_eq!(parameter, "constraints");
            assert_eq!(
                message,
                format!(
                    "constraint 4 (tag 4): {word} of the start of geometry 0 and the centre of geometry 1, drawn at one place, (0, 0): the solver measures it by the line between them"
                )
            );
        }
        // A midpoint's two points are a line's ends, which have length: it measures by the
        // line it's on.
        let mut s = system();
        s.values[4] = 0.0;
        s.values[5] = 0.0;
        s.constraints.push(SolverConstraint::MidpointOnLine {
            tag: Tag(4),
            a: PointOf::Start(Geometry(0)),
            b: PointOf::Center(Geometry(1)),
            line: Geometry(0),
        });
        s.check().unwrap();
    }

    #[test]
    fn a_tangency_at_an_end_names_an_end_of_its_first_curve_and_a_fixed_angle() {
        let tangent_on = |point: PointOf, angle: Value| SolverConstraint::TangentOn {
            tag: Tag(4),
            a: Geometry(0),
            point,
            b: Geometry(1),
            angle,
        };
        let mut s = system();
        s.constraints
            .push(tangent_on(PointOf::End(Geometry(0)), Value(16)));
        s.check().unwrap();
        let (_, message) = refused(|s| {
            s.constraints
                .push(tangent_on(PointOf::Center(Geometry(1)), Value(16)))
        });
        assert_eq!(
            message,
            "constraint 4 (tag 4): a tangency at an end's point must be an end of geometry 0, got the centre of geometry 1"
        );
        let (_, message) = refused(|s| {
            s.constraints
                .push(tangent_on(PointOf::End(Geometry(0)), Value(3)))
        });
        assert_eq!(
            message,
            "constraint 4 (tag 4): a tangency at an end's value 3 is an unknown: a dimension's value must be fixed"
        );
        // A circle has no ends to be tangent at.
        let (_, message) = refused(|s| {
            s.constraints.push(SolverConstraint::TangentOn {
                tag: Tag(4),
                a: Geometry(1),
                point: PointOf::Center(Geometry(1)),
                b: Geometry(0),
                angle: Value(16),
            })
        });
        assert_eq!(
            message,
            "constraint 4 (tag 4): a tangency at an end needs a line or an arc, but geometry 1 is a circle"
        );
    }

    #[test]
    fn geometry_a_solution_cant_build_is_named_by_its_place() {
        // `system()`: a line (0, 0) to (10, 0), a circle about (0, 5) of radius 2, and an arc
        // about (1, 1) of radius 1 from (2, 1) to (1, 2), each built.
        let s = system();
        assert!(s.unbuildable(&s.values).is_empty());
        let spoilt = |at: usize, v: f64| {
            let mut values = s.values.clone();
            values[at] = v;
            s.unbuildable(&values)
        };
        // The line's end on its start: no length.
        let mut values = s.values.clone();
        values[2] = 0.0;
        assert_eq!(s.unbuildable(&values), [Geometry(0)]);
        // A circle's radius of 0, below 0, and not a number; past a kilometre.
        for r in [0.0, -1.0, f64::NAN, 1e6 + 1.0] {
            assert_eq!(spoilt(6, r), [Geometry(1)], "{r}");
        }
        // An arc's radius of 0; its end on its start; an angle out of reach.
        assert_eq!(spoilt(9, 0.0), [Geometry(2)]);
        let mut values = s.values.clone();
        (values[14], values[15]) = (2.0, 1.0);
        assert_eq!(s.unbuildable(&values), [Geometry(2)]);
        assert_eq!(spoilt(11, f64::INFINITY), [Geometry(2)]);
        // A point past a kilometre, in the line and so in it.
        assert_eq!(spoilt(0, -2e6), [Geometry(0)]);
        // Each named once, in order.
        let mut values = s.values.clone();
        (values[2], values[6], values[9]) = (0.0, 0.0, 0.0);
        assert_eq!(
            s.unbuildable(&values),
            [Geometry(0), Geometry(1), Geometry(2)]
        );
    }
}
