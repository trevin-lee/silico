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
    within_reach,
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
    }

    /// A constraint: its kind, its tag (0 for an arc's rules), what it names (`b_position`
    /// `None` for a constraint of one operand, whose `b` is then never read), and its value
    /// (if it has one; otherwise 0, never read).
    struct ConstraintIn {
        kind: ConstraintKind,
        tag: u32,
        a_position: Position,
        a: u32,
        b_position: Position,
        b: u32,
        value: u32,
    }

    /// PlaneGCS's algorithms.
    #[repr(u8)]
    enum AlgorithmIn {
        DogLeg,
        LevenbergMarquardt,
        Bfgs,
    }

    /// How a solve ended: PlaneGCS's `SolveStatus` (it never returns its fourth,
    /// `SuccessfulSolutionInvalid`, which only FreeCAD's sketch sets), and the bridge's
    /// reading of a Converged that can only be its redundancy check.
    #[repr(u8)]
    enum Status {
        Success,
        Converged,
        Failed,
        /// See [`SolveStatus::SolvedExceptRedundant`](super::SolveStatus).
        SolvedExceptRedundant,
    }

    struct SolveInput {
        values: Vec<f64>,
        unknowns: Vec<u32>,
        geometry: Vec<GeometryIn>,
        constraints: Vec<ConstraintIn>,
        algorithms: Vec<AlgorithmIn>,
    }

    /// What PlaneGCS found, as [`Solved`](super::Solved) has it.
    struct SolveOutput {
        dofs: i32,
        conflicting: Vec<i32>,
        redundant: Vec<i32>,
        partially_redundant: Vec<i32>,
        free: Vec<u32>,
        no_equations: bool,
        status: Status,
        /// The index in `algorithms` of the one that solved (Success or
        /// SolvedExceptRedundant), or -1.
        succeeded: i32,
        values: Vec<f64>,
        /// Each tagged constraint's residual, by tag in increasing order.
        residual_tags: Vec<u32>,
        residuals: Vec<f64>,
    }

    /// A point a drag pins (never `None` or `Geometry`), its geometry, and the weight of its
    /// pin's two rows (1, or less to let it give way first). The first pin is the dragged
    /// point, held to the cursor; the rest are held where they start.
    struct Pin {
        position: Position,
        geometry: u32,
        scale: f64,
    }

    /// What a drag's start found: the degrees of freedom, and the tags of the constraints
    /// in conflict (none, or there's no drag).
    struct DragDiagnosis {
        dofs: i32,
        conflicting: Vec<i32>,
    }

    /// How a drag's move ended: PlaneGCS's status (never `SolvedExceptRedundant`), and every
    /// value when it's `Success` (otherwise none).
    struct MoveOutput {
        status: Status,
        values: Vec<f64>,
    }

    unsafe extern "C++" {
        include!("silico-sketch/cpp/bridge.h");

        fn solve(input: &SolveInput) -> Result<SolveOutput>;

        /// A drag under way: the sketch's PlaneGCS system, kept between moves.
        type DragSession;
        fn drag_begin(input: &SolveInput, pins: &[Pin]) -> Result<UniquePtr<DragSession>>;
        fn drag_diagnosis(session: &DragSession) -> Result<DragDiagnosis>;
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
/// each of its algorithms in turn until one solves. A solution out of reach is refused.
pub fn solve(system: &SketchSystem) -> Result<Solved, Failure> {
    prepare_eigen();
    let out = ffi::solve(&encode(system)).map_err(bridge_threw)?;
    check_reach(&out.values)?;
    Ok(Solved {
        dofs: out.dofs,
        conflicting: tags(&out.conflicting)?,
        redundant: tags(&out.redundant)?,
        partially_redundant: tags(&out.partially_redundant)?,
        free: out.free.iter().map(|&v| Value(v)).collect(),
        no_equations: out.no_equations,
        status: match out.status {
            ffi::Status::Success => SolveStatus::Success,
            ffi::Status::Converged => SolveStatus::Converged,
            ffi::Status::Failed => SolveStatus::Failed,
            ffi::Status::SolvedExceptRedundant => SolveStatus::SolvedExceptRedundant,
            _ => return Err(internal("the bridge returned a status Rust doesn't know")),
        },
        algorithm: usize::try_from(out.succeeded)
            .ok()
            .and_then(|i| system.algorithms.get(i).copied()),
        values: out.values,
        residuals: out
            .residual_tags
            .iter()
            .zip(&out.residuals)
            .map(|(&tag, &r)| (Tag(tag), r.is_finite().then_some(r)))
            .collect(),
    })
}

/// A drag under way (docs/ports/sketcher.md, Dragging): the sketch as PlaneGCS holds it,
/// diagnosed once at the start, with the dragged point pinned to the cursor by rows of
/// lower priority than the sketch's constraints. Every move solves from the values at the
/// start, never from the last move's, so a move's result is a function of the start and
/// the cursor alone: frames dropped or reordered can't change where the sketch ends up.
///
/// FreeCAD 1.1.3 instead starts again from the current values once the cursor has gone 20
/// times its first step from where it started (`RecalculateInitialSolutionWhileMovingPoint`),
/// but measures from a point its GUI never sets (`initToPoint`), so whether it does depends
/// on earlier drags; Silico doesn't port that.
pub struct Drag {
    session: cxx::UniquePtr<ffi::DragSession>,
    /// The sketch's geometry, to check each move's against.
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
        prepare_eigen();
        let pins: Vec<ffi::Pin> = pins(&begin.system, begin.point)
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
        let session = ffi::drag_begin(&encode(&begin.system), &pins).map_err(bridge_threw)?;
        let diagnosis = match session.as_ref() {
            Some(s) => ffi::drag_diagnosis(s).map_err(bridge_threw)?,
            None => return Err(internal("the bridge started no drag")),
        };
        let conflicting = tags(&diagnosis.conflicting)?;
        let drag = conflicting.is_empty().then(|| Drag {
            session,
            geometry: begin.system.geometry.clone(),
        });
        Ok(DragStart {
            dofs: diagnosis.dofs,
            conflicting,
            drag,
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
        // A solution stands only if its geometry can be built.
        let status = if status == SolveStatus::Success && !buildable(&self.geometry, &out.values) {
            SolveStatus::SolvedInvalid
        } else {
            status
        };
        let values = if status == SolveStatus::Success {
            check_reach(&out.values)?;
            Some(out.values)
        } else {
            None
        };
        Ok(Moved { status, values })
    }
}

/// The pins a drag of `p` holds, with their weights, as FreeCAD's `Sketch::initMove` pins
/// one point: the point itself; and for an arc's start or end, the arc's centre too, where
/// it starts, a hundred times more weakly, so the end moves before the centre does
/// (`4d7a9435c1`). `p` must name a point its geometry has.
fn pins(system: &SketchSystem, p: PointOf) -> Vec<(PointOf, f64)> {
    let mut pins = vec![(p, 1.0)];
    if let (Some(SketchGeometry::Arc { .. }), PointOf::Start(g) | PointOf::End(g)) =
        (system.geometry.get(p.geometry().0 as usize), p)
    {
        pins.push((PointOf::Center(g), 0.01));
    }
    pins
}

/// Whether geometry solved to `values` can be built, as FreeCAD's `Sketch::updateGeometry`
/// finds when it makes its curves: a line has length to the solver (dx² + dy² isn't 0, the
/// sketch worker's check; FreeCAD's `GeomLineSegment::setPoints`, "Both points are equal"),
/// and a circle or an arc a radius above 0 (FreeCAD's `gp_Circ` refuses one below 0 and
/// keeps 0; Silico draws nothing of no size).
fn buildable(geometry: &[SketchGeometry], values: &[f64]) -> bool {
    let at = |v: Value| values.get(v.0 as usize).copied().unwrap_or(f64::NAN);
    geometry.iter().all(|g| match *g {
        SketchGeometry::Line { start, end } => {
            let (dx, dy) = (at(end.x) - at(start.x), at(end.y) - at(start.y));
            dx * dx + dy * dy != 0.0
        }
        SketchGeometry::Circle { radius, .. } | SketchGeometry::Arc { radius, .. } => {
            at(radius) > 0.0
        }
        SketchGeometry::Point(_) => true,
    })
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
            .map(|a| match a {
                Algorithm::DogLeg => ffi::AlgorithmIn::DogLeg,
                Algorithm::LevenbergMarquardt => ffi::AlgorithmIn::LevenbergMarquardt,
                Algorithm::Bfgs => ffi::AlgorithmIn::Bfgs,
            })
            .collect(),
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
    let (kind, a, b, value) = match *c {
        SolverConstraint::ArcRules { arc } => (K::ArcRules, whole(arc), none, 0),
        SolverConstraint::Coincident { a, b, .. } => (K::Coincident, point(a), point(b), 0),
        SolverConstraint::Horizontal { line, .. } => (K::Horizontal, whole(line), none, 0),
        SolverConstraint::Vertical { line, .. } => (K::Vertical, whole(line), none, 0),
        SolverConstraint::DistanceX { a, b, value, .. } => {
            (K::DistanceX, point(a), point(b), value.0)
        }
        SolverConstraint::DistanceY { a, b, value, .. } => {
            (K::DistanceY, point(a), point(b), value.0)
        }
        SolverConstraint::Distance { a, b, value, .. } => {
            (K::Distance, point(a), point(b), value.0)
        }
        SolverConstraint::PointOnLine { point: p, line, .. } => {
            (K::PointOnLine, point(p), whole(line), 0)
        }
        SolverConstraint::Parallel { a, b, .. } => (K::Parallel, whole(a), whole(b), 0),
        SolverConstraint::Perpendicular { a, b, .. } => (K::Perpendicular, whole(a), whole(b), 0),
        SolverConstraint::Angle { a, b, value, .. } => (K::Angle, whole(a), whole(b), value.0),
        SolverConstraint::EqualLength { a, b, .. } => (K::EqualLength, whole(a), whole(b), 0),
        SolverConstraint::Radius { circle, value, .. } => (K::Radius, whole(circle), none, value.0),
    };
    ffi::ConstraintIn {
        kind,
        tag,
        a_position: a.0,
        a: a.1,
        b_position: b.0,
        b: b.1,
        value,
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
