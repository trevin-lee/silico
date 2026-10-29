//! For tests of `Translation::solve`'s checks by load: a stand-in sketch solver that answers
//! as a function of where it starts, as PlaneGCS diagnoses where the sketch is drawn, and
//! the one sketch it's given. Included with `#[path]` beside `common/sketch.rs` as `fixture`.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use silico_core::Error;
use silico_core::sketch::SketchSolver;
use silico_protocol::{Algorithm, SketchSystem, SolveStatus, Solved, Tag};

/// p1 (0, 0) to p2 (10, 0), line n1 horizontal (k0000001), 10 long (k0000002), and
/// horizontal again (k0000003); p3 at (4, 3), free: values 10 and 11, which the stand-in
/// moves.
pub fn drawing() -> String {
    crate::fixture::document(&crate::fixture::fields(
        r#"{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 0, "construction": false}, {"id": "p0000003", "x": 4, "y": 3, "construction": false}"#,
        r#"{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}"#,
        "",
        "",
        r#"{"id": "k0000001", "type": "horizontal", "line": "n0000001"}, {"id": "k0000002", "type": "length", "line": "n0000001", "value": 10, "driving": true}, {"id": "k0000003", "type": "horizontal", "line": "n0000001"}"#,
    ))
}

/// What a worker that solved `system` to p3's y at `y` would answer, with `status`: every
/// constraint met, and an algorithm exactly where `status` gives a solution (as the core's
/// reply check requires).
pub fn answer(system: &SketchSystem, y: f64, status: SolveStatus) -> Solved {
    let mut values = system.values.clone();
    values[11] = y;
    let solution = matches!(
        status,
        SolveStatus::Success | SolveStatus::SolvedExceptRedundant | SolveStatus::SolvedInvalid
    );
    Solved {
        dofs: 0,
        conflicting: Vec::new(),
        redundant: Vec::new(),
        partially_redundant: Vec::new(),
        dependent_groups: Vec::new(),
        free: Vec::new(),
        no_equations: false,
        status,
        algorithm: solution.then_some(Algorithm::DogLeg),
        values,
        residuals: (1..=3).map(|t| (Tag(t), Some(0.0))).collect(),
    }
}

/// Solved to p3's y at `y`, the constraint tagged `tag` dropped as redundant.
pub fn dropping(system: &SketchSystem, tag: u32, y: f64) -> Solved {
    let mut a = answer(system, y, SolveStatus::SolvedExceptRedundant);
    a.redundant = vec![Tag(tag)];
    a
}

/// Solved to p3's y at `y`, k0000001 and k0000003 called conflicting.
pub fn conflict(system: &SketchSystem, y: f64) -> Solved {
    let mut a = answer(system, y, SolveStatus::Success);
    a.conflicting = vec![Tag(1), Tag(3)];
    a.dofs = -1;
    a
}

/// How a stand-in answers a system it's sent.
pub type Answering = Box<dyn Fn(&SketchSystem) -> Result<Solved, Error> + Send>;

/// A stand-in that answers as a function of where it starts (p3's y), and records each
/// start.
pub struct ByStart {
    answer: Answering,
    starts: Arc<Mutex<Vec<f64>>>,
}

impl SketchSolver for ByStart {
    fn solve(&mut self, system: SketchSystem) -> Result<Solved, Error> {
        self.starts.lock().unwrap().push(system.values[11]);
        (self.answer)(&system)
    }
}

/// A stand-in answering with `f`, and the starts it's sent.
pub fn by_start(
    f: impl Fn(&SketchSystem) -> Result<Solved, Error> + Send + 'static,
) -> (ByStart, Arc<Mutex<Vec<f64>>>) {
    let starts = Arc::new(Mutex::new(Vec::new()));
    (
        ByStart {
            answer: Box::new(f),
            starts: Arc::clone(&starts),
        },
        starts,
    )
}
