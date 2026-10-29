//! /break-it on `Translation::solve`'s solve again from the solution (as first written, a
//! solve that dropped constraints was solved again from the solver's own values until it
//! didn't move, and that answer kept), with a stand-in solver that answers as a function of
//! where it starts, as PlaneGCS does (it diagnoses where the sketch is drawn). A load is
//! M3.4's: the same translation started from the solution a save kept. The claim under
//! attack: the answer that stands "is what every later load gives". silico-sketch's
//! tests/adversarial_redundant_resolve.rs has the same breaks through the real worker.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::stand_in;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use silico_core::document::{ElementId, Operation, Sketch, read};
use silico_core::sketch::{Answer, MAX_LOAD_RESOLVES, SketchSolver, Translation};
use silico_core::sketch_states::ConstraintState::{Redundant, Unsolved};
use silico_core::sketch_states::{SketchState, states};
use silico_core::{Error, ErrorCode};
use silico_protocol::{SolveStatus, Tag};
use stand_in::{answer, by_start, conflict, dropping};

/// The stand-in's drawing, translated.
fn translation() -> Translation {
    let Operation::Sketch(s) = &read(stand_in::drawing().as_bytes()).unwrap().features[0].operation
    else {
        panic!("not a sketch")
    };
    let t = Translation::new(s).unwrap();
    assert_eq!(t.system.values[11], 3.0, "p3's y");
    t
}

/// What an answer says: Ok with the redundant constraints it names, or the refusal's code
/// and elements; and p3's y, as bits.
type Says = (Result<Vec<String>, (ErrorCode, Vec<String>)>, u64);

/// `t` solved with `solver` (a save), then solved again from the solution it kept (a load).
fn save_and_load(t: &Translation, solver: &mut dyn SketchSolver) -> (Answer, Answer) {
    let save = t.solve(solver).unwrap();
    let mut load = t.clone();
    load.system.values = save.solved().values.clone();
    let loaded = load.solve(solver).unwrap();
    (save, loaded)
}

/// What `a` says.
fn says(a: &Answer) -> Says {
    (
        match a.outcome() {
            Ok(s) => Ok(s
                .redundant
                .keys()
                .map(|id| id.as_str().to_owned())
                .collect()),
            Err(e) => Err((e.code(), e.elements().to_vec())),
        },
        a.solved().values[11].to_bits(),
    )
}

#[test]
fn a_solution_called_conflicting_from_itself_isnt_kept_as_solved() {
    // Found by break-it on that first solve again, fixed: diagnosed on the drawing (p3 at y
    // 3), PlaneGCS drops k0000003 and solves, p3 at y 4: Ok. Solved again from y 4 it
    // doesn't move, but diagnosed there it calls k0000001 and k0000003 conflicting. It kept
    // the first answer, Ok, so every load, starting at y 4, was refused. The real worker
    // did this to an equilateral triangle 999 m out (silico-sketch's
    // an_equilateral_triangle_far_out_is_refused_on_save_as_its_load_refuses_it). Now the
    // load's answer stands: the save and the load say the same, refused.
    let t = translation();
    let (mut s, starts) = by_start(|system| {
        Ok(if system.values[11] == 3.0 {
            dropping(system, 3, 4.0)
        } else {
            conflict(system, system.values[11])
        })
    });
    let (save, load) = save_and_load(&t, &mut s);
    assert_eq!(*starts.lock().unwrap(), [3.0, 4.0, 4.0, 4.0]);
    assert_eq!(says(&save), says(&load));
}

#[test]
fn the_redundant_constraints_a_save_names_are_those_its_load_names() {
    // Found by break-it on that first solve again, fixed: as above, but solved from y 4
    // PlaneGCS drops k0000001 instead and solves. The save named k0000003 (the drawing's
    // pick), every load k0000001; the real worker did it to a unit square, a slot, and a
    // triangle drawn a little off (silico-sketch's
    // *_names_the_same_redundant_constraints_*). Now the save names the load's.
    let t = translation();
    let (mut s, _) = by_start(|system| {
        Ok(if system.values[11] == 3.0 {
            dropping(system, 3, 4.0)
        } else {
            dropping(system, 1, system.values[11])
        })
    });
    let (save, load) = save_and_load(&t, &mut s);
    assert_eq!(says(&save), says(&load));
}

#[test]
fn a_solution_that_settles_one_load_past_the_bound_is_refused_as_not_settling() {
    // Moving 3 → 4 → 5 → 6 → 7, then settled at 7: MAX_LOAD_RESOLVES (3) loads reach 7 but
    // can't see that it's settled. The first (y 4) stood when loads that didn't settle left
    // the first answer standing, and every load moved it; now the save is refused as a
    // solution that doesn't settle on load (the council's finding), which writes nothing.
    // Pinned so a change of MAX_LOAD_RESOLVES or of the rule is seen.
    let t = translation();
    let (mut s, starts) = by_start(|system| {
        let y = system.values[11];
        Ok(dropping(system, 3, if y < 7.0 { y + 1.0 } else { y }))
    });
    let save = t.solve(&mut s).unwrap();
    assert_eq!(starts.lock().unwrap().len(), 1 + MAX_LOAD_RESOLVES);
    assert_eq!(
        save.into_outcome().unwrap_err().code(),
        ErrorCode::SolveFailed,
        "refused as not settling"
    );
}

#[test]
fn a_solution_that_flips_between_two_places_is_bounded_and_refused() {
    // 3 → 4 → 5 → 4 → 5: two solutions, each solving to the other. At most
    // 1 + MAX_LOAD_RESOLVES solves, then refused as not settling.
    let t = translation();
    let (mut s, starts) = by_start(|system| {
        let y = system.values[11];
        Ok(dropping(system, 3, if y == 4.0 { 5.0 } else { 4.0 }))
    });
    let save = t.solve(&mut s).unwrap();
    assert_eq!(*starts.lock().unwrap(), [3.0, 4.0, 5.0, 4.0]);
    assert_eq!(
        save.into_outcome().unwrap_err().code(),
        ErrorCode::SolveFailed
    );
}

#[test]
fn a_crash_on_the_last_solve_again_is_the_error_and_the_next_solve_is_clean() {
    // The solves again are the worker's like the first: a crash on the third is the
    // solve's error (worker_crashed), not the first answer; and the solver used again
    // after it solves a sketch as before.
    let t = translation();
    let calls = Arc::new(Mutex::new(0usize));
    let c = Arc::clone(&calls);
    let (mut s, _) = by_start(move |system| {
        let mut n = c.lock().unwrap();
        *n += 1;
        let y = system.values[11];
        match *n {
            4 => Err(Error::new(
                ErrorCode::WorkerCrashed,
                "the sketch worker died",
            )),
            _ if *n > 4 => Ok(answer(system, 4.0, SolveStatus::Success)),
            _ => Ok(dropping(system, 3, y + 1.0)),
        }
    });
    let e = t.solve(&mut s).unwrap_err();
    assert_eq!(e.code(), ErrorCode::WorkerCrashed);
    let next = t.solve(&mut s).unwrap();
    assert_eq!(says(&next), (Ok(Vec::new()), 4f64.to_bits()));
}

#[test]
fn a_partially_redundant_constraint_alone_is_solved_again_too() {
    // Dropped as partially redundant only: the solve moves from y 4 to 5 and stays; 5
    // stands, as for a redundant one.
    let t = translation();
    let (mut s, starts) = by_start(|system| {
        let y = system.values[11];
        let mut a = answer(
            system,
            if y == 3.0 { 4.0 } else { 5.0 },
            SolveStatus::SolvedExceptRedundant,
        );
        a.partially_redundant = vec![Tag(3)];
        Ok(a)
    });
    let save = t.solve(&mut s).unwrap();
    assert_eq!(*starts.lock().unwrap(), [3.0, 4.0, 5.0]);
    // Kept apart from the redundant: none of those, and it partially redundant.
    assert_eq!(says(&save), (Ok(vec![]), 5f64.to_bits()));
    let partly: Vec<&str> = save
        .outcome()
        .unwrap()
        .partially_redundant
        .keys()
        .map(|id| id.as_str())
        .collect();
    assert_eq!(partly, ["k0000003"]);
}

#[test]
fn a_negative_zero_turned_positive_by_the_solve_again_settles_on_the_positive() {
    // The file keeps -0 and 0 apart, and so does the loop: a solve that turns -0 into 0
    // (a zero step added) has moved, and the next, from 0, settles: 0 stands, which a load
    // from 0 gives back.
    let t = translation();
    let (mut s, starts) = by_start(|system| {
        let y = system.values[11];
        Ok(dropping(system, 3, if y == 3.0 { -0.0 } else { y + 0.0 }))
    });
    let (save, load) = save_and_load(&t, &mut s);
    assert_eq!(save.solved().values[11].to_bits(), 0f64.to_bits());
    assert_eq!(says(&save), says(&load));
    assert_eq!(starts.lock().unwrap().len(), 3 + 2);
}

/// The stand-in's drawing, as a sketch.
fn drawing() -> Sketch {
    let Operation::Sketch(s) = &read(stand_in::drawing().as_bytes()).unwrap().features[0].operation
    else {
        panic!("not a sketch")
    };
    s.clone()
}

#[test]
fn a_load_that_never_settles_keeps_the_first_solves_diagnosis_beside_its_failure() {
    // The first solve drops k0000003 and moves p3 from y 3 to 4; each load from there moves
    // it a millimetre more, so none of the MAX_LOAD_RESOLVES (3) settles, and the solve
    // fails, solve_failed, with no answer of the loads' own. The diagnosis beside that error
    // is the first solve's: k0000003 redundant. So the sketch reads no_solution, its two
    // constraints unsolved and the redundant one still redundant, not a diagnosis lost with
    // the load (Answer::diagnosed an error) nor one of nothing.
    let t = translation();
    let (mut s, starts) = by_start(|system| {
        let y = system.values[11];
        Ok(dropping(system, 3, if y < 7.0 { y + 1.0 } else { y }))
    });
    let save = t.solve(&mut s).unwrap();
    assert_eq!(*starts.lock().unwrap(), [3.0, 4.0, 5.0, 6.0]);
    let read = save
        .diagnosed()
        .expect("the first solve's diagnosis is kept");
    assert_eq!(read.outcome().unwrap_err().code(), ErrorCode::SolveFailed);
    assert_eq!(read.diagnosis().redundant, ids(&["k0000003"]));
    let got = states(&drawing(), read).unwrap();
    assert_eq!(got.state, SketchState::NoSolution);
    assert_eq!(
        got.constraints.values().copied().collect::<Vec<_>>(),
        [Unsolved, Unsolved, Redundant]
    );
}

#[test]
fn a_load_the_gate_refuses_keeps_the_first_solves_diagnosis_beside_its_refusal() {
    // The first solve drops k0000003 and puts p2 1e-8 mm from p1, its residuals all met:
    // line n1 is then shorter than the gate's 1e-7 mm, so the file can't be saved or
    // loaded (`holdable`), and the solve is refused, invalid_result naming the line and its
    // ends, before any load solves. The diagnosis beside it is the first solve's, k0000003
    // redundant: the sketch reads invalid, its two constraints unsolved and the redundant
    // one still redundant.
    let t = translation();
    assert_eq!(t.system.values[6..10], [0.0, 0.0, 10.0, 0.0], "p1, p2");
    let (mut s, starts) = by_start(|system| {
        let mut solved = dropping(system, 3, 3.0);
        solved.values[8] = 1e-8;
        Ok(solved)
    });
    let save = t.solve(&mut s).unwrap();
    assert_eq!(*starts.lock().unwrap(), [3.0]);
    let read = save
        .diagnosed()
        .expect("the first solve's diagnosis is kept");
    let refusal = read.outcome().unwrap_err();
    assert_eq!(
        (refusal.code(), refusal.elements()),
        (
            ErrorCode::InvalidResult,
            &[
                "n0000001".to_owned(),
                "p0000001".to_owned(),
                "p0000002".to_owned()
            ][..]
        ),
        "{refusal}"
    );
    assert_eq!(read.diagnosis().redundant, ids(&["k0000003"]));
    let got = states(&drawing(), read).unwrap();
    assert_eq!(got.state, SketchState::Invalid);
    assert_eq!(
        got.constraints.values().copied().collect::<Vec<_>>(),
        [Unsolved, Unsolved, Redundant]
    );
}

fn ids(list: &[&str]) -> BTreeSet<ElementId> {
    list.iter().map(|s| ElementId::new(s).unwrap()).collect()
}
