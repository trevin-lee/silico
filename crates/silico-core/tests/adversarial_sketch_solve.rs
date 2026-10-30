//! /break-it on M3.3's core side (silico_core::sketch): replies a broken or hostile sketch
//! worker could send, and the errors a solve's failures come back as. The worker is a
//! stand-in here, so each reply is exactly the one under test. Each break they exposed is
//! fixed, and marked where it was found.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::fixture;

use silico_core::document::{Checked, FeatureId, Operation, Sketch, read};
use silico_core::sketch::{SketchSolver, Translation, solve};
use silico_core::{Error, ErrorCode};
use silico_protocol::{Algorithm, SketchSystem, SolveStatus, Solved, Tag};

/// p1 (0, 0) to p2 (10, 0), a line, its start held at the origin, horizontal, 10 long.
fn ten() -> String {
    fixture::document(&fixture::fields(
        r#"{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 0, "construction": false}"#,
        r#"{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}"#,
        "",
        "",
        r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000003", "type": "horizontal", "line": "n0000001"}, {"id": "k0000004", "type": "length", "line": "n0000001", "value": 10, "driving": true}"#,
    ))
}

fn sketch_of(text: &str) -> Sketch {
    match &read(text.as_bytes()).unwrap().features[0].operation {
        Operation::Sketch(s) => s.clone(),
        other => panic!("{other:?}"),
    }
}

/// What a worker would answer to `system`, solved to `values`.
fn answer(system: &SketchSystem, values: Vec<f64>, status: SolveStatus) -> Solved {
    let mut tags: Vec<Tag> = system.constraints.iter().filter_map(|c| c.tag()).collect();
    tags.sort_unstable();
    let solution = matches!(
        status,
        SolveStatus::Success | SolveStatus::SolvedExceptRedundant | SolveStatus::SolvedInvalid
    );
    Solved {
        dofs: 0,
        conflicting: Vec::new(),
        redundant: Vec::new(),
        partially_redundant: Vec::new(),
        dependencies: Vec::new(),
        free: Vec::new(),
        no_equations: false,
        status,
        algorithm: solution.then_some(Algorithm::DogLeg),
        values,
        residuals: tags.into_iter().map(|t| (t, Some(0.0))).collect(),
    }
}

/// How a stand-in worker answers a system.
type Answer = Box<dyn FnMut(&SketchSystem) -> Result<Solved, Error> + Send>;

/// A stand-in worker.
struct Standing(Answer);

impl SketchSolver for Standing {
    fn solve(&mut self, system: SketchSystem) -> Result<Solved, Error> {
        (self.0)(&system)
    }
}

fn solve_with(
    f: impl FnMut(&SketchSystem) -> Result<Solved, Error> + Send + 'static,
) -> Result<silico_core::sketch::SketchSolution, Error> {
    let checked = Checked::new(read(ten().as_bytes()).unwrap()).unwrap();
    solve(
        &checked,
        &FeatureId::new(fixture::FEATURE).unwrap(),
        &mut Standing(Box::new(f)),
    )
}

#[test]
fn a_solver_that_fails_outright_is_reported_against_the_sketch() {
    // The worker's own failures (a solution out of reach, a timeout, a crash) come back from
    // SketchSolver::solve as errors. Found by break-it, fixed: `solve` passed them on
    // without the feature, so the error didn't say which sketch; it names it now.
    for code in [
        ErrorCode::KernelFailed,
        ErrorCode::WorkerTimeout,
        ErrorCode::WorkerCrashed,
    ] {
        let e = solve_with(move |_| Err(Error::new(code, "the worker gave up"))).unwrap_err();
        assert_eq!(e.code(), code);
        assert_eq!(e.feature(), Some(fixture::FEATURE), "{code:?}: {e}");
    }
}

#[test]
fn a_success_whose_line_has_no_length_is_not_taken_as_a_solution() {
    // A worker that says Success with both ends of the line at (0, 0): geometry the sketch
    // can't hold (the gate refuses a line of no length, so a solution written back as this
    // would never open). The worker is meant to pass such solutions over; a reply that
    // doesn't is one the core can't trust. Found by break-it, fixed: the core took it as
    // solved; it now checks every solution can be built, whatever the worker says.
    let got = solve_with(|s| {
        let mut v = s.values.clone();
        // Values 6..10 are p1 and p2 (after the origin and the axes' ends).
        v[6..10].copy_from_slice(&[0.0, 0.0, 0.0, 0.0]);
        Ok(answer(s, v, SolveStatus::Success))
    });
    match got {
        Ok(s) => panic!("a line of no length taken as solved: {:?}", s.points),
        Err(e) => assert!(
            matches!(
                e.code(),
                ErrorCode::ProtocolError | ErrorCode::InvalidResult
            ),
            "{e}"
        ),
    }
}

#[test]
fn a_solved_invalid_with_nothing_invalid_names_something() {
    // SolvedInvalid whose values build every piece: the core's error names what can't be
    // built, from its own reading. With nothing to name, the error is an InvalidResult with
    // no elements, which tells the user nothing. Either the reply is refused as untrustworthy,
    // or the error names an element.
    let e =
        solve_with(|s| Ok(answer(s, s.values.clone(), SolveStatus::SolvedInvalid))).unwrap_err();
    // Found by break-it, fixed: it was invalid_result with elements []; a claim of geometry
    // that can't be built, with none, is a reply the core can't trust.
    assert!(
        e.code() == ErrorCode::ProtocolError || !e.elements().is_empty(),
        "{e} / {:?}",
        e.elements()
    );
}

/// The words of `text` that spell a value that isn't a finite number, as Rust, C++, or
/// JavaScript writes one (JSON has none: serde writes a non-finite number as `null`).
fn non_finite_words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|w| {
            matches!(
                *w,
                "NaN" | "nan" | "-NaN" | "inf" | "-inf" | "Infinity" | "-Infinity"
            )
        })
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_solution_that_isnt_a_number_names_what_cant_be_built_and_goes_no_further() {
    // A reply that calls its solution one that can't be built carries the solver's values,
    // which may not be numbers (since M3.3): they name what can't be built, and reach no
    // output. Each unknown in turn, and all at once, not a number or infinite: the error is
    // the unbuildable one, naming geometry, and neither its words nor its JSON (what `check
    // --json` writes) hold a value that isn't a number.
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for which in [None, Some(0), Some(1), Some(2), Some(3)] {
            let e = solve_with(move |s| {
                let mut v = s.values.clone();
                for (k, u) in s.unknowns.iter().enumerate() {
                    if which.is_none_or(|w| w == k) {
                        v[u.0 as usize] = bad;
                    }
                }
                Ok(answer(s, v, SolveStatus::SolvedInvalid))
            })
            .unwrap_err();
            assert_eq!(e.code(), ErrorCode::InvalidResult, "{bad} {which:?}: {e}");
            // Geometry, by its IDs: the points that aren't numbers, and the line through them.
            assert!(!e.elements().is_empty(), "{bad} {which:?}: {e}");
            assert!(
                e.elements()
                    .iter()
                    .all(|id| ["p0000001", "p0000002", "n0000001"].contains(&id.as_str())),
                "{bad} {which:?}: {:?}",
                e.elements()
            );
            let json = serde_json::to_string(&e).unwrap();
            assert_eq!(non_finite_words(&json), Vec::<String>::new(), "{json}");
            assert_eq!(
                non_finite_words(&e.to_string()),
                Vec::<String>::new(),
                "{e}"
            );
        }
    }
}

#[test]
fn replies_with_values_that_arent_numbers_are_refused() {
    for bad in [f64::NAN, f64::INFINITY, -f64::INFINITY, 1e6 + 1e-9, -1e300] {
        let e = solve_with(move |s| {
            let mut v = s.values.clone();
            v[8] = bad;
            Ok(answer(s, v, SolveStatus::Success))
        })
        .unwrap_err();
        assert_eq!(e.code(), ErrorCode::ProtocolError, "{bad}: {e}");
        assert_eq!(e.feature(), Some(fixture::FEATURE));
    }
}

#[test]
fn replies_naming_tag_zero_or_past_the_constraints_are_refused() {
    for tags in [
        vec![Tag(0)],
        vec![Tag(5)],
        vec![Tag(u32::MAX)],
        vec![Tag(2), Tag(2)],
        vec![Tag(3), Tag(1)],
    ] {
        for which in 0..3 {
            let tags = tags.clone();
            let e = solve_with(move |s| {
                let mut a = answer(s, s.values.clone(), SolveStatus::Success);
                match which {
                    0 => a.conflicting = tags.clone(),
                    1 => a.redundant = tags.clone(),
                    _ => a.partially_redundant = tags.clone(),
                }
                Ok(a)
            })
            .unwrap_err();
            assert_eq!(e.code(), ErrorCode::ProtocolError, "{which} {e}");
        }
    }
}

#[test]
fn a_redundant_tag_also_listed_partially_redundant_is_counted_once() {
    // PlaneGCS lists a constraint in one list or the other; a reply listing one in both is
    // odd, and must not report it twice.
    let s = solve_with(|s| {
        let mut a = answer(s, s.values.clone(), SolveStatus::SolvedExceptRedundant);
        a.redundant = vec![Tag(3)];
        a.partially_redundant = vec![Tag(3)];
        Ok(a)
    })
    .unwrap();
    assert_eq!(s.redundant.len(), 1, "{s:?}");
}

#[test]
fn the_translation_is_the_same_bytes_for_the_same_sketch_in_any_order() {
    // The sketch's lists in reverse: the translation sorts by ID, so the system is identical.
    let forward = sketch_of(&ten());
    let mut reversed = forward.clone();
    reversed.points.reverse();
    reversed.lines.reverse();
    reversed.constraints.reverse();
    let (a, b) = (
        Translation::new(&forward).unwrap().system,
        Translation::new(&reversed).unwrap().system,
    );
    assert_eq!(
        serde_json::to_vec(&a).unwrap(),
        serde_json::to_vec(&b).unwrap()
    );
}
