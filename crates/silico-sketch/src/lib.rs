//! The PlaneGCS bridge and the sketch worker (ARCHITECTURE.md, Process model). The only
//! crate that links PlaneGCS (`third_party/planegcs`, FreeCAD 1.1.3's): every other crate
//! reaches it through the worker process (invariant 1), so no other crate may depend on
//! this one.
//!
//! The bridge builds a PlaneGCS system from a [`SketchSystem`] exactly as given, diagnoses
//! it, and solves it, or keeps it for a drag ([`Drag`]); what that means is the core's to
//! decide (docs/ports/sketcher.md).

// FFI crate: `unsafe` is permitted here and nowhere else (see ARCHITECTURE.md).
#![allow(unsafe_code)]

pub mod worker;

use silico_protocol::{
    Algorithm, DragBegin, Failure, InvalidParameter, Moved, PointOf, Refusal, SketchGeometry,
    SketchSystem, SolveStatus, Solved, SolverConstraint, Tag, Value, check_drag_target,
    unbuildable, within_reach,
};

#[cxx::bridge(namespace = "silico_sketch")]
mod ffi {
    /// The kinds of geometry, as [`SketchGeometry`](super::SketchGeometry) has them.
    #[repr(u8)]
    enum GeometryKind {
        Point,
        Line,
        Circle,
        Arc,
    }

    /// A piece of geometry: its kind and its values, in
    /// [`SketchGeometry::values`](super::SketchGeometry::values)' order.
    struct GeometryIn {
        kind: GeometryKind,
        values: Vec<u32>,
    }

    /// Which point of a piece of geometry a constraint names, the geometry itself, or
    /// nothing (a constraint with one operand).
    #[repr(u8)]
    enum Position {
        None,
        Geometry,
        Point,
        Start,
        End,
        Center,
    }

    /// The kinds of constraint, as [`SolverConstraint`](super::SolverConstraint) has them.
    #[repr(u8)]
    enum ConstraintKind {
        ArcRules,
        Coincident,
        Horizontal,
        Vertical,
        DistanceX,
        DistanceY,
        Distance,
        PointOnLine,
        Parallel,
        Perpendicular,
        Angle,
        EqualLength,
        Radius,
        HorizontalPoints,
        VerticalPoints,
        PointLineDistance,
        PointOnCircle,
        EqualRadius,
        TangentLine,
        TangentCircles,
        TangentOn,
        AngleViaPoint,
        SymmetricLine,
        MidpointOnLine,
        SymmetricPoint,
    }

    /// A constraint: its kind, its tag (0 for an arc's rules), what it names (`b_position`
    /// `None` for a constraint of one operand, and `c_position` for one of two, whose
    /// operands are then never read), its value (if it has one; otherwise 0, never read), and
    /// a choice of two (a tangency of circles' `internal`; otherwise false, never read).
    struct ConstraintIn {
        kind: ConstraintKind,
        tag: u32,
        a_position: Position,
        a: u32,
        b_position: Position,
        b: u32,
        c_position: Position,
        c: u32,
        value: u32,
        flag: bool,
    }

    /// PlaneGCS's algorithms.
    #[repr(u8)]
    enum AlgorithmIn {
        DogLeg,
        LevenbergMarquardt,
        Bfgs,
        AnchoredSqp,
    }

    /// How a solve ended: PlaneGCS's `SolveStatus` (it never returns its fourth,
    /// `SuccessfulSolutionInvalid`, which only FreeCAD's sketch sets).
    #[repr(u8)]
    enum Status {
        Success,
        Converged,
        Failed,
    }

    struct SolveInput {
        values: Vec<f64>,
        unknowns: Vec<u32>,
        geometry: Vec<GeometryIn>,
        constraints: Vec<ConstraintIn>,
        algorithms: Vec<AlgorithmIn>,
    }

    /// What PlaneGCS's diagnosis found, as [`Solved`](super::Solved) has it.
    struct Diagnosed {
        dofs: i32,
        conflicting: Vec<i32>,
        redundant: Vec<i32>,
        partially_redundant: Vec<i32>,
        free: Vec<u32>,
        no_equations: bool,
    }

    /// One algorithm's solve: PlaneGCS's status, and every value of its solution unless it
    /// failed (otherwise none).
    struct Attempt {
        status: Status,
        values: Vec<f64>,
    }

    /// A point a drag pins (never `None` or `Geometry`), its geometry, and the weight of its
    /// pin's two rows (1, or less to let it give way first). The first pin is the dragged
    /// point, held to the cursor; the rest are held where they start.
    struct Pin {
        position: Position,
        geometry: u32,
        scale: f64,
    }

    /// What a diagnosis found: the degrees of freedom, and the tags of the constraints in
    /// conflict.
    struct Diagnosis {
        dofs: i32,
        conflicting: Vec<i32>,
    }

    /// How a drag's move ended: PlaneGCS's status, and every value when it's `Success`
    /// (otherwise none).
    struct MoveOutput {
        status: Status,
        values: Vec<f64>,
    }

    unsafe extern "C++" {
        include!("silico-sketch/cpp/bridge.h");

        /// A sketch built and diagnosed once, to be solved by one algorithm after another.
        type Solver;
        fn solver_new(input: &SolveInput) -> Result<UniquePtr<Solver>>;
        // A `Result`, as every other call here that can fail has: what it does now can't
        // throw (it pushes plain integers into `rust::Vec`s, whose growth is `noexcept` and
        // runs out of memory by aborting, as Rust's allocations do), but C++ added to it
        // later that throws comes back as a refusal, not `std::terminate` across the bridge.
        fn solver_diagnosis(solver: &Solver) -> Result<Diagnosed>;
        fn solver_try(solver: Pin<&mut Solver>, algorithm: AlgorithmIn) -> Result<Attempt>;
        fn solver_residuals(
            solver: Pin<&mut Solver>,
            values: &[f64],
            tags: &[u32],
        ) -> Result<Vec<f64>>;

        /// A drag under way: the PlaneGCS system of the part a drag moves, kept between moves.
        type DragSession;
        fn drag_begin(input: &SolveInput, pins: &[Pin]) -> Result<UniquePtr<DragSession>>;
        fn diagnose(input: &SolveInput) -> Result<Diagnosis>;
        fn drag_move(session: Pin<&mut DragSession>, x: f64, y: f64) -> Result<MoveOutput>;

        fn fp_trap_mask() -> u32;
        fn disable_fp_traps() -> Result<()>;
        fn fix_eigen_cache_sizes();

        #[cfg(feature = "test_ops")]
        fn debug_throw_runtime_error(message: &str) -> Result<()>;
        #[cfg(feature = "test_ops")]
        fn debug_raise_sigsegv();
        #[cfg(feature = "test_ops")]
        fn debug_enable_fp_traps() -> Result<()>;
        #[cfg(feature = "test_ops")]
        fn debug_throw_bad_alloc() -> Result<()>;
        #[cfg(feature = "test_ops")]
        fn debug_eigen_cache_sizes() -> Vec<u64>;
    }
}

/// Fixes Eigen's cache sizes before any product it computes, in whatever process solves.
fn prepare_eigen() {
    static EIGEN_CACHES: std::sync::Once = std::sync::Once::new();
    EIGEN_CACHES.call_once(ffi::fix_eigen_cache_sizes);
}

/// Tags as PlaneGCS reported them, which are never negative for the core's constraints.
fn tags(tags: &[i32]) -> Result<Vec<Tag>, Failure> {
    tags.iter()
        .map(|&t| u32::try_from(t).map(Tag))
        .collect::<Result<_, _>>()
        .map_err(|_| internal("PlaneGCS reported a negative tag"))
}

/// A value refused by the protocol's check, as the worker refuses it.
pub fn invalid(e: InvalidParameter) -> Failure {
    Failure::new(Refusal::InvalidParameter {
        parameter: e.parameter.to_owned(),
        message: e.message,
    })
}

/// Diagnoses `system`, which must have passed [`SketchSystem::check`], and solves it with
/// each of its algorithms in turn, each from the values it gives, until one solves on
/// geometry that can be built ([`SketchSystem::unbuildable`]): FreeCAD's
/// `Sketch::internalSolve`, which undoes a solution its geometry refuses and tries the next
/// (`fc128d357d`); a solution out of reach is one of those.
///
/// With equations dropped as redundant, PlaneGCS's `System::solve` checks the dropped ones on
/// the values it started from, before its solution is written back, and turns `Success` into
/// `Converged` if they miss there; so whether it says `Success` depends on where the sketch
/// was drawn, not on the solution. A solve with equations dropped is reported as solved
/// except them either way: everything it kept is met, and the dropped ones' residuals on the
/// solution say whether they agree, the core's to judge (docs/roadmap/m3-sketcher.md,
/// decision 5). DogLeg and Levenberg–Marquardt end only in `Success` or `Failed` (GCS.cpp,
/// `solve_DL` and `solve_LM`), so their `Converged` is that check: that holds while every
/// constraint is driving with a tag of 0 or more, so there's no auxiliary subsystem. BFGS
/// and anchored SQP can end `Converged` of their own accord, so theirs stays unsolved, as
/// FreeCAD reads it.
pub fn solve(system: &SketchSystem) -> Result<Solved, Failure> {
    prepare_eigen();
    let mut solver = ffi::solver_new(&encode(system)).map_err(bridge_threw)?;
    let diagnosed = ffi::solver_diagnosis(&solver).map_err(bridge_threw)?;
    let conflicting = tags(&diagnosed.conflicting)?;
    let redundant = tags(&diagnosed.redundant)?;
    let partially_redundant = tags(&diagnosed.partially_redundant)?;
    let dropped = !redundant.is_empty() || !partially_redundant.is_empty();
    let mut status = SolveStatus::Failed;
    let mut solution: Option<(Algorithm, Vec<f64>)> = None;
    let mut invalid: Option<(Algorithm, Vec<f64>)> = None;
    // FreeCAD doesn't solve a sketch whose constraints conflict; the worker does, as M3.1's
    // did, and the core reads the conflict first.
    for &algorithm in &system.algorithms {
        let attempt =
            ffi::solver_try(solver.pin_mut(), encode_algorithm(algorithm)).map_err(bridge_threw)?;
        let solved = match attempt.status {
            ffi::Status::Success => true,
            ffi::Status::Converged => {
                dropped && matches!(algorithm, Algorithm::DogLeg | Algorithm::LevenbergMarquardt)
            }
            ffi::Status::Failed => false,
            _ => return Err(internal("the bridge returned a status Rust doesn't know")),
        };
        if !solved {
            status = if attempt.status == ffi::Status::Converged {
                SolveStatus::Converged
            } else {
                SolveStatus::Failed
            };
            continue;
        }
        // A solution out of reach can't be built either: the next solve couldn't start
        // from it.
        if check_reach(&attempt.values).is_ok() && system.unbuildable(&attempt.values).is_empty() {
            status = if dropped {
                SolveStatus::SolvedExceptRedundant
            } else {
                SolveStatus::Success
            };
            solution = Some((algorithm, attempt.values));
            break;
        }
        invalid.get_or_insert((algorithm, attempt.values));
    }
    if solution.is_none() && invalid.is_some() {
        status = SolveStatus::SolvedInvalid;
    }
    let (algorithm, values) = match solution.or(invalid) {
        Some((algorithm, values)) => (Some(algorithm), values),
        None => (None, system.values.clone()),
    };
    // Every tagged constraint's residual on those values: PlaneGCS solves only the parts
    // with unknowns, so this is how a constraint among fixed values alone shows whether it
    // holds.
    let mut tagged: Vec<u32> = system
        .constraints
        .iter()
        .filter_map(|c| c.tag().map(|t| t.0))
        .collect();
    tagged.sort_unstable();
    let residuals =
        ffi::solver_residuals(solver.pin_mut(), &values, &tagged).map_err(bridge_threw)?;
    Ok(Solved {
        dofs: diagnosed.dofs,
        conflicting,
        redundant,
        partially_redundant,
        free: diagnosed.free.iter().map(|&v| Value(v)).collect(),
        no_equations: diagnosed.no_equations,
        status,
        algorithm,
        values,
        residuals: tagged
            .iter()
            .zip(&residuals)
            .map(|(&tag, &r)| (Tag(tag), r.is_finite().then_some(r)))
            .collect(),
    })
}

/// A drag under way (docs/ports/sketcher.md, Dragging): the part of the sketch its pins
/// move ([`DragBegin::part`]) as PlaneGCS holds it, every other value held where it began
/// (the whole sketch diagnosed at the start for conflicts and degrees of freedom), with the
/// dragged point pinned to the cursor by rows of lower priority than the sketch's
/// constraints. Every move solves from the values at the
/// start, never from the last move's, so a move's result is a function of the start and
/// the cursor alone: frames dropped or reordered can't change where the sketch ends up.
///
/// FreeCAD 1.1.3 instead starts again from the current values once the cursor has gone 20
/// times its first step from where it started (`RecalculateInitialSolutionWhileMovingPoint`),
/// but measures from a point its GUI never sets (`initToPoint`), so whether it does depends
/// on earlier drags; Silico doesn't port that.
pub struct Drag {
    session: cxx::UniquePtr<ffi::DragSession>,
    /// The geometry the drag moves (any with a value in its part), to check each move's
    /// against.
    geometry: Vec<SketchGeometry>,
}

/// What a drag's start found: the diagnosis, and the drag, unless constraints conflict.
pub struct DragStart {
    /// Degrees of freedom left: negative when over-constrained.
    pub dofs: i32,
    /// Constraints in conflict, by tag, in increasing order. FreeCAD doesn't drag a sketch
    /// with any, so then there's no drag.
    pub conflicting: Vec<Tag>,
    pub drag: Option<Drag>,
}

impl Drag {
    /// Checks `begin` ([`DragBegin::check`]), builds and diagnoses its sketch, and, unless
    /// constraints conflict, pins its point where it is.
    pub fn begin(begin: &DragBegin) -> Result<DragStart, Failure> {
        begin.check().map_err(invalid)?;
        Self::start(begin, begin.pins())
    }

    /// Checks `begin` as one move ([`DragBegin::check_move`]: a solve's limits, not the
    /// drag's), and starts it as [`Drag::begin`] does, for that move alone, its pins
    /// FreeCAD's `initMove`'s ([`DragBegin::pins`]), as FreeCAD's `extend` drags a line.
    pub fn begin_once(begin: &DragBegin) -> Result<DragStart, Failure> {
        begin.check_move().map_err(invalid)?;
        Self::start(begin, begin.pins())
    }

    /// Builds and diagnoses `begin`'s sketch, checked, and, unless constraints conflict,
    /// holds `pins` where they are.
    fn start(begin: &DragBegin, pins: Vec<(PointOf, f64)>) -> Result<DragStart, Failure> {
        prepare_eigen();
        // The whole sketch's diagnosis, as FreeCAD's: a conflict anywhere refuses the drag.
        let whole = ffi::diagnose(&encode(&begin.system)).map_err(bridge_threw)?;
        let conflicting = tags(&whole.conflicting)?;
        if !conflicting.is_empty() {
            return Ok(DragStart {
                dofs: whole.dofs,
                conflicting,
                drag: None,
            });
        }
        // The session holds only the part the pins move; the rest stays where it is.
        let part = begin.part();
        let pins: Vec<ffi::Pin> = pins
            .into_iter()
            .map(|(p, scale)| {
                let (position, geometry) = point(p);
                ffi::Pin {
                    position,
                    geometry,
                    scale,
                }
            })
            .collect();
        let session = ffi::drag_begin(&encode(&part), &pins).map_err(bridge_threw)?;
        if session.is_null() {
            return Err(internal("the bridge started no drag"));
        }
        let moved = |g: &SketchGeometry| {
            g.values()
                .into_iter()
                .any(|v| part.unknowns.binary_search(&v).is_ok())
        };
        Ok(DragStart {
            dofs: whole.dofs,
            conflicting,
            drag: Some(Drag {
                session,
                geometry: part.geometry.iter().copied().filter(moved).collect(),
            }),
        })
    }

    /// Moves the cursor to `to` ([`check_drag_target`]) and solves from the drag's start
    /// with DogLeg alone, as FreeCAD drags. A solution out of reach is refused.
    pub fn move_to(&mut self, to: [f64; 2]) -> Result<Moved, Failure> {
        check_drag_target(to).map_err(invalid)?;
        let Some(session) = self.session.as_mut() else {
            return Err(internal("the drag has no session"));
        };
        let out = ffi::drag_move(session, to[0], to[1]).map_err(bridge_threw)?;
        let status = match out.status {
            ffi::Status::Success => SolveStatus::Success,
            ffi::Status::Converged => SolveStatus::Converged,
            ffi::Status::Failed => SolveStatus::Failed,
            _ => return Err(internal("the bridge returned a status a move can't have")),
        };
        // A solution out of reach is refused, as M3.7 does (the drag goes on from its last
        // good frame); one in reach stands only if its geometry can be built.
        if status == SolveStatus::Success {
            check_reach(&out.values)?;
        }
        let status = if status == SolveStatus::Success
            && !unbuildable(&self.geometry, &out.values).is_empty()
        {
            SolveStatus::SolvedInvalid
        } else {
            status
        };
        let values = (status == SolveStatus::Success).then_some(out.values);
        Ok(Moved { status, values })
    }
}

/// What the bridge threw, as a refusal.
fn bridge_threw(e: cxx::Exception) -> Failure {
    thrown(e.what())
}

/// An exception's words as a refusal: running out of memory is a shortage, as the kernel
/// worker reports it; anything else is a bug in the bridge (PlaneGCS itself never throws).
fn thrown(what: &str) -> Failure {
    let refusal = if what == "std::bad_alloc" {
        Refusal::OutOfMemory
    } else {
        Refusal::SolverThrew
    };
    Failure {
        refusal,
        cause: Some(what.to_owned()),
    }
}

/// A solution must be one the next solve can start from to leave the worker: finite and
/// within reach. PlaneGCS checks nothing.
fn check_reach(values: &[f64]) -> Result<(), Failure> {
    match values.iter().position(|&v| !within_reach(v)) {
        None => Ok(()),
        Some(at) => Err(Failure::new(Refusal::SolutionOutOfReach {
            value: at as u32,
        })),
    }
}

fn internal(what: &str) -> Failure {
    Failure::new(Refusal::Internal {
        what: what.to_owned(),
    })
}

/// The trap-enable bits that are set: 0 when every floating-point trap is off.
pub fn fp_trap_mask() -> u32 {
    ffi::fp_trap_mask()
}

/// Switches every floating-point trap off.
pub fn disable_fp_traps() -> Result<(), String> {
    ffi::disable_fp_traps().map_err(|e| e.what().to_owned())
}

/// `system` as the C++ side reads it.
fn encode(system: &SketchSystem) -> ffi::SolveInput {
    let geometry = system
        .geometry
        .iter()
        .map(|g| ffi::GeometryIn {
            kind: match g {
                SketchGeometry::Point(_) => ffi::GeometryKind::Point,
                SketchGeometry::Line { .. } => ffi::GeometryKind::Line,
                SketchGeometry::Circle { .. } => ffi::GeometryKind::Circle,
                SketchGeometry::Arc { .. } => ffi::GeometryKind::Arc,
            },
            values: g.values().iter().map(|v| v.0).collect(),
        })
        .collect();
    ffi::SolveInput {
        values: system.values.clone(),
        unknowns: system.unknowns.iter().map(|v| v.0).collect(),
        geometry,
        constraints: system.constraints.iter().map(encode_constraint).collect(),
        algorithms: system
            .algorithms
            .iter()
            .map(|&a| encode_algorithm(a))
            .collect(),
    }
}

fn encode_algorithm(a: Algorithm) -> ffi::AlgorithmIn {
    match a {
        Algorithm::DogLeg => ffi::AlgorithmIn::DogLeg,
        Algorithm::LevenbergMarquardt => ffi::AlgorithmIn::LevenbergMarquardt,
        Algorithm::Bfgs => ffi::AlgorithmIn::Bfgs,
        Algorithm::AnchoredSqp => ffi::AlgorithmIn::AnchoredSqp,
    }
}

fn point(p: PointOf) -> (ffi::Position, u32) {
    match p {
        PointOf::Point(g) => (ffi::Position::Point, g.0),
        PointOf::Start(g) => (ffi::Position::Start, g.0),
        PointOf::End(g) => (ffi::Position::End, g.0),
        PointOf::Center(g) => (ffi::Position::Center, g.0),
    }
}

fn encode_constraint(c: &SolverConstraint) -> ffi::ConstraintIn {
    use ffi::ConstraintKind as K;
    let tag = c.tag().map_or(0, |t| t.0);
    let whole = |g: silico_protocol::Geometry| (ffi::Position::Geometry, g.0);
    let none = (ffi::Position::None, 0);
    let (kind, a, b, c3, value, flag) = match *c {
        SolverConstraint::ArcRules { arc } => (K::ArcRules, whole(arc), none, none, 0, false),
        SolverConstraint::Coincident { a, b, .. } => {
            (K::Coincident, point(a), point(b), none, 0, false)
        }
        SolverConstraint::Horizontal { line, .. } => {
            (K::Horizontal, whole(line), none, none, 0, false)
        }
        SolverConstraint::Vertical { line, .. } => (K::Vertical, whole(line), none, none, 0, false),
        SolverConstraint::DistanceX { a, b, value, .. } => {
            (K::DistanceX, point(a), point(b), none, value.0, false)
        }
        SolverConstraint::DistanceY { a, b, value, .. } => {
            (K::DistanceY, point(a), point(b), none, value.0, false)
        }
        SolverConstraint::Distance { a, b, value, .. } => {
            (K::Distance, point(a), point(b), none, value.0, false)
        }
        SolverConstraint::PointOnLine { point: p, line, .. } => {
            (K::PointOnLine, point(p), whole(line), none, 0, false)
        }
        SolverConstraint::Parallel { a, b, .. } => {
            (K::Parallel, whole(a), whole(b), none, 0, false)
        }
        SolverConstraint::Perpendicular { a, b, .. } => {
            (K::Perpendicular, whole(a), whole(b), none, 0, false)
        }
        SolverConstraint::Angle { a, b, value, .. } => {
            (K::Angle, whole(a), whole(b), none, value.0, false)
        }
        SolverConstraint::EqualLength { a, b, .. } => {
            (K::EqualLength, whole(a), whole(b), none, 0, false)
        }
        SolverConstraint::Radius { circle, value, .. } => {
            (K::Radius, whole(circle), none, none, value.0, false)
        }
        SolverConstraint::HorizontalPoints { a, b, .. } => {
            (K::HorizontalPoints, point(a), point(b), none, 0, false)
        }
        SolverConstraint::VerticalPoints { a, b, .. } => {
            (K::VerticalPoints, point(a), point(b), none, 0, false)
        }
        SolverConstraint::PointLineDistance {
            point: p,
            line,
            value,
            ..
        } => (
            K::PointLineDistance,
            point(p),
            whole(line),
            none,
            value.0,
            false,
        ),
        SolverConstraint::PointOnCircle {
            point: p, circle, ..
        } => (K::PointOnCircle, point(p), whole(circle), none, 0, false),
        SolverConstraint::EqualRadius { a, b, .. } => {
            (K::EqualRadius, whole(a), whole(b), none, 0, false)
        }
        SolverConstraint::TangentLine { line, circle, .. } => {
            (K::TangentLine, whole(line), whole(circle), none, 0, false)
        }
        SolverConstraint::TangentCircles { a, b, internal, .. } => {
            (K::TangentCircles, whole(a), whole(b), none, 0, internal)
        }
        SolverConstraint::TangentOn {
            a,
            point: p,
            b,
            angle,
            ..
        } => (K::TangentOn, whole(a), whole(b), point(p), angle.0, false),
        SolverConstraint::AngleViaPoint {
            a,
            b,
            point: p,
            angle,
            ..
        } => (
            K::AngleViaPoint,
            whole(a),
            whole(b),
            point(p),
            angle.0,
            false,
        ),
        SolverConstraint::SymmetricLine { a, b, line, .. } => {
            (K::SymmetricLine, point(a), point(b), whole(line), 0, false)
        }
        SolverConstraint::MidpointOnLine { a, b, line, .. } => {
            (K::MidpointOnLine, point(a), point(b), whole(line), 0, false)
        }
        SolverConstraint::SymmetricPoint { a, b, center, .. } => (
            K::SymmetricPoint,
            point(a),
            point(b),
            point(center),
            0,
            false,
        ),
    };
    ffi::ConstraintIn {
        kind,
        tag,
        a_position: a.0,
        a: a.1,
        b_position: b.0,
        b: b.1,
        c_position: c3.0,
        c: c3.1,
        value,
        flag,
    }
}

/// Operations that exist only to test the bridge, built with the `test_ops` feature
/// (which this crate's own tests turn on, and nothing else should).
#[cfg(feature = "test_ops")]
pub mod test_ops {
    use super::ffi;

    /// Throws `std::runtime_error` with `message` on the C++ side.
    pub fn throw_runtime_error(message: &str) -> Result<(), String> {
        ffi::debug_throw_runtime_error(message).map_err(|e| e.what().to_owned())
    }

    /// Crashes the process with SIGSEGV, as a fault inside PlaneGCS would.
    pub fn raise_sigsegv() {
        ffi::debug_raise_sigsegv();
    }

    /// Turns floating-point traps on. Linux keeps them on; some Apple silicon ignores the
    /// trap-enable bits (they read back as 0), and some keeps them.
    pub fn enable_fp_traps() -> Result<(), String> {
        ffi::debug_enable_fp_traps().map_err(|e| e.what().to_owned())
    }

    /// The cache sizes Eigen blocks its matrix products by: L1, L2, L3, in bytes.
    pub fn eigen_cache_sizes() -> Vec<u64> {
        ffi::debug_eigen_cache_sizes()
    }

    /// Throws `std::bad_alloc`, as a C++ allocation that fails does, and reads it as a
    /// solve does.
    pub fn throw_bad_alloc() -> Result<(), super::Failure> {
        ffi::debug_throw_bad_alloc().map_err(super::bridge_threw)
    }
}

#[cfg(test)]
mod tests {
    use silico_protocol::Refusal;

    #[test]
    fn a_solution_out_of_reach_is_refused_naming_the_first_value_that_is() {
        assert!(super::check_reach(&[0.0, -1e6, 1e6, f64::MIN_POSITIVE, -0.0]).is_ok());
        assert!(super::check_reach(&[]).is_ok());
        for (values, at) in [
            (vec![f64::NAN], 0),
            (vec![1.0, 2.0, f64::INFINITY, f64::NAN], 2),
            (vec![1.0, f64::NEG_INFINITY], 1),
            (vec![1.0, 1e6, 1.000_000_1e6], 2),
            (vec![-1e300], 0),
        ] {
            let failure = super::check_reach(&values).unwrap_err();
            assert_eq!(failure.refusal, Refusal::SolutionOutOfReach { value: at });
            assert_eq!(failure.cause, None);
        }
    }

    #[test]
    fn a_solve_fixes_the_cache_sizes_eigen_blocks_its_products_by() {
        // Eigen asks an x86 CPU for its caches and blocks matrix products deeper than its
        // L1 allows by them, so two machines would sum in different orders; a solve fixes
        // them first to the sizes it assumes where it can't ask (ARM's). On ARM, then, a
        // missing fix leaves the same sizes, and this is caught on x86 (the CI runner):
        // the M3.7 mutation run's one miss.
        use silico_protocol::{Algorithm, PointValues, SketchGeometry, SketchSystem, Value};
        let system = SketchSystem {
            values: vec![0.0, 0.0],
            unknowns: vec![Value(0), Value(1)],
            geometry: vec![SketchGeometry::Point(PointValues {
                x: Value(0),
                y: Value(1),
            })],
            constraints: Vec::new(),
            algorithms: vec![Algorithm::DogLeg],
        };
        super::solve(&system).unwrap();
        assert_eq!(
            super::test_ops::eigen_cache_sizes(),
            [16 * 1024, 512 * 1024, 512 * 1024]
        );
    }

    #[test]
    fn running_out_of_memory_is_a_shortage_and_anything_else_the_bridges_bug() {
        // libc++'s std::bad_alloc, thrown through the bridge.
        let failure = super::test_ops::throw_bad_alloc().unwrap_err();
        assert_eq!(
            (failure.refusal, failure.cause.as_deref()),
            (Refusal::OutOfMemory, Some("std::bad_alloc"))
        );
        for what in [
            "silico-sketch bridge: value 7 out of range",
            "std::bad_alloc!",
            "",
        ] {
            assert_eq!(super::thrown(what).refusal, Refusal::SolverThrew, "{what}");
        }
    }

    /// The unit-test binary links PlaneGCS and the bridge, so it must import no inexact
    /// platform math either (ARCHITECTURE.md, Deterministic kernel).
    #[test]
    fn unit_test_binary_imports_no_platform_math() {
        super::imports::assert_this_binary_imports_no_platform_math();
    }
}

#[cfg(test)]
#[path = "../../silico-kernel/tests/common/imports.rs"]
mod imports;
