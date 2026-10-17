//! The PlaneGCS bridge and the sketch worker (ARCHITECTURE.md, Process model). The only
//! crate that links PlaneGCS (`third_party/planegcs`, FreeCAD 1.1.3's): every other crate
//! reaches it through the worker process (invariant 1), so no other crate may depend on
//! this one.
//!
//! The bridge builds a PlaneGCS system from a [`SketchSystem`] exactly as given, diagnoses
//! it, and solves it; what that means is the core's to decide (docs/ports/sketcher.md).

// FFI crate: `unsafe` is permitted here and nowhere else (see ARCHITECTURE.md).
#![allow(unsafe_code)]

pub mod worker;

use silico_protocol::{
    Algorithm, Failure, PointOf, Refusal, SketchGeometry, SketchSystem, SolveStatus, Solved,
    SolverConstraint, Tag, Value, within_reach,
};

#[cxx::bridge(namespace = "silico_sketch")]
mod ffi {
    /// A [`SketchSystem`](super::SketchSystem), flattened (see [`encode`](super::encode)).
    struct SolveInput {
        values: Vec<f64>,
        unknowns: Vec<u32>,
        geometry: Vec<u32>,
        constraints: Vec<u32>,
        algorithm: u8,
    }

    /// What PlaneGCS found, as [`Solved`](super::Solved) has it, its status a number.
    struct SolveOutput {
        dofs: i32,
        conflicting: Vec<i32>,
        redundant: Vec<i32>,
        partially_redundant: Vec<i32>,
        free: Vec<u32>,
        status: i32,
        values: Vec<f64>,
        /// Each tagged constraint's residual, by tag in increasing order.
        residual_tags: Vec<u32>,
        residuals: Vec<f64>,
    }

    unsafe extern "C++" {
        include!("silico-sketch/cpp/bridge.h");

        fn solve(input: &SolveInput) -> Result<SolveOutput>;
        fn fp_trap_mask() -> u32;
        fn disable_fp_traps() -> Result<()>;
        fn fix_eigen_cache_sizes();

        #[cfg(feature = "test_ops")]
        fn debug_throw_runtime_error(message: &str) -> Result<()>;
        #[cfg(feature = "test_ops")]
        fn debug_raise_sigsegv();
    }
}

/// Where geometry and constraints sit in the flattened encoding the C++ side reads.
const GEOMETRY_STRIDE: usize = 10;
const CONSTRAINT_STRIDE: usize = 8;

/// Diagnoses and solves `system`, which must have passed [`SketchSystem::check`]. A
/// solution with a value that isn't finite is refused.
pub fn solve(system: &SketchSystem) -> Result<Solved, Failure> {
    // Before any product Eigen computes, in whatever process solves.
    static EIGEN_CACHES: std::sync::Once = std::sync::Once::new();
    EIGEN_CACHES.call_once(ffi::fix_eigen_cache_sizes);
    let out = ffi::solve(&encode(system)).map_err(|e| Failure {
        refusal: Refusal::SolverThrew,
        cause: Some(e.what().to_owned()),
    })?;
    check_reach(&out.values)?;
    let tags = |tags: &[i32]| -> Result<Vec<Tag>, Failure> {
        tags.iter()
            .map(|&t| u32::try_from(t).map(Tag))
            .collect::<Result<_, _>>()
            .map_err(|_| internal("PlaneGCS reported a negative tag"))
    };
    Ok(Solved {
        dofs: out.dofs,
        conflicting: tags(&out.conflicting)?,
        redundant: tags(&out.redundant)?,
        partially_redundant: tags(&out.partially_redundant)?,
        free: out.free.iter().map(|&v| Value(v)).collect(),
        status: match out.status {
            0 => SolveStatus::Success,
            1 => SolveStatus::Converged,
            2 => SolveStatus::Failed,
            other => return Err(internal(&format!("PlaneGCS returned status {other}"))),
        },
        values: out.values,
        residuals: out
            .residual_tags
            .iter()
            .zip(&out.residuals)
            .map(|(&tag, &r)| (Tag(tag), r.is_finite().then_some(r)))
            .collect(),
    })
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

/// `system` flattened for the C++ side: geometry as `[kind, values...]` and constraints as
/// `[kind, tag, a's position, a, b's position, b, value, 0]`, each padded to its stride.
fn encode(system: &SketchSystem) -> ffi::SolveInput {
    let mut geometry = Vec::with_capacity(system.geometry.len() * GEOMETRY_STRIDE);
    for g in &system.geometry {
        let (kind, values): (u32, Vec<Value>) = match *g {
            SketchGeometry::Point(p) => (0, vec![p.x, p.y]),
            SketchGeometry::Line { start, end } => (1, vec![start.x, start.y, end.x, end.y]),
            SketchGeometry::Circle { center, radius } => (2, vec![center.x, center.y, radius]),
            SketchGeometry::Arc {
                center,
                radius,
                start_angle,
                end_angle,
                start,
                end,
            } => (
                3,
                vec![
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
            ),
        };
        let at = geometry.len();
        geometry.push(kind);
        geometry.extend(values.iter().map(|v| v.0));
        geometry.resize(at + GEOMETRY_STRIDE, 0);
    }
    let mut constraints = Vec::with_capacity(system.constraints.len() * CONSTRAINT_STRIDE);
    for c in &system.constraints {
        constraints.extend(encode_constraint(c));
    }
    ffi::SolveInput {
        values: system.values.clone(),
        unknowns: system.unknowns.iter().map(|v| v.0).collect(),
        geometry,
        constraints,
        algorithm: match system.algorithm {
            Algorithm::DogLeg => 0,
            Algorithm::LevenbergMarquardt => 1,
            Algorithm::Bfgs => 2,
        },
    }
}

/// Positions as the C++ side numbers them: 0 for the geometry itself.
fn point(p: PointOf) -> [u32; 2] {
    match p {
        PointOf::Point(g) => [1, g.0],
        PointOf::Start(g) => [2, g.0],
        PointOf::End(g) => [3, g.0],
        PointOf::Center(g) => [4, g.0],
    }
}

fn encode_constraint(c: &SolverConstraint) -> [u32; CONSTRAINT_STRIDE] {
    let tag = c.tag().map_or(0, |t| t.0);
    let points = |kind: u32, a: PointOf, b: PointOf, value: u32| {
        let [ap, a] = point(a);
        let [bp, b] = point(b);
        [kind, tag, ap, a, bp, b, value, 0]
    };
    let two = |kind: u32, a: u32, b: u32, value: u32| [kind, tag, 0, a, 0, b, value, 0];
    match *c {
        SolverConstraint::ArcRules { arc } => two(0, arc.0, 0, 0),
        SolverConstraint::Coincident { a, b, .. } => points(1, a, b, 0),
        SolverConstraint::Horizontal { line, .. } => two(2, line.0, 0, 0),
        SolverConstraint::Vertical { line, .. } => two(3, line.0, 0, 0),
        SolverConstraint::DistanceX { a, b, value, .. } => points(4, a, b, value.0),
        SolverConstraint::DistanceY { a, b, value, .. } => points(5, a, b, value.0),
        SolverConstraint::Distance { a, b, value, .. } => points(6, a, b, value.0),
        SolverConstraint::PointOnLine { point: p, line, .. } => {
            let [pp, p] = point(p);
            [7, tag, pp, p, 0, line.0, 0, 0]
        }
        SolverConstraint::Parallel { a, b, .. } => two(8, a.0, b.0, 0),
        SolverConstraint::Perpendicular { a, b, .. } => two(9, a.0, b.0, 0),
        SolverConstraint::Angle { a, b, value, .. } => two(10, a.0, b.0, value.0),
        SolverConstraint::EqualLength { a, b, .. } => two(11, a.0, b.0, 0),
        SolverConstraint::Radius { circle, value, .. } => two(12, circle.0, 0, value.0),
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
