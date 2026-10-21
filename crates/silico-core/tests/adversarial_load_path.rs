//! /break-it on `Translation::solve`'s check by load (as first written, only a solve that
//! dropped constraints or was called over-constrained was checked by a load, and where no
//! load settled the first answer stood): the solution is written into the sketch as its
//! file holds it and solved as a load solves it, until a load leaves it where it started;
//! that load's answer stands, and a load that reaches no solution from a solution a save
//! would write stands refused. The claim under attack: "the answer a solve keeps is the one
//! its load gives". A refusal is always its own load (a save that refuses leaves the file
//! as drawn, and the load solves the same drawing), so the attack is on every path that
//! answers Ok: the save's solution written through the file, read back, gated, and solved
//! again as M3.4's load does, must give the same answer. The stand-in solver answers as a
//! function of where it starts (p3's y), as PlaneGCS diagnoses where the sketch is drawn.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/sketch.rs"]
mod fixture;
#[path = "common/stand_in.rs"]
mod stand_in;

use silico_core::document::{Checked, FeatureId, Operation, read, write};
use silico_core::sketch::{MAX_LOAD_RESOLVES, SketchSolution, SketchSolver, solve};
use silico_core::{Error, ErrorCode};
use silico_protocol::SolveStatus;
use stand_in::{answer, by_start, conflict, dropping};

/// The stand-in's drawing, read and gated.
fn drawing() -> Checked {
    Checked::new(read(stand_in::drawing().as_bytes()).unwrap()).unwrap()
}

fn feature() -> FeatureId {
    FeatureId::new(fixture::FEATURE).unwrap()
}

/// `doc` as a save writes it once `solution` is its sketch's solve, through the file's
/// text, read back and gated: what a load opens.
fn saved(doc: &Checked, solution: &SketchSolution) -> Checked {
    let mut document = doc.document().clone();
    let Operation::Sketch(sketch) = &mut document.features[0].operation else {
        panic!("not a sketch")
    };
    *sketch = solution.written(sketch);
    Checked::new(read(write(&document).unwrap().as_bytes()).unwrap()).unwrap()
}

/// What an answer says: Ok with p3's y (bits) and the redundant constraints it names, or
/// the refusal's code and elements.
type Says = Result<(u64, Vec<String>), (ErrorCode, Vec<String>)>;

fn says(r: &Result<SketchSolution, Error>) -> Says {
    match r {
        Ok(s) => Ok((
            s.point(&silico_core::document::ElementId::new("p0000003").unwrap())
                .unwrap()[1]
                .to_bits(),
            s.redundant
                .keys()
                .map(|id| id.as_str().to_owned())
                .collect(),
        )),
        Err(e) => Err((e.code(), e.elements().to_vec())),
    }
}

/// The save's answer and the load's: the drawing solved, and, if it solved, the file a save
/// writes of it solved again. A refusal saves nothing: the load solves the drawing again.
fn save_and_load(solver: &mut dyn SketchSolver) -> (Says, Says) {
    let doc = drawing();
    let save = solve(&doc, &feature(), solver);
    let load = match &save {
        Ok(s) => solve(&saved(&doc, s), &feature(), solver),
        Err(_) => solve(&doc, &feature(), solver),
    };
    (says(&save), says(&load))
}

#[test]
fn ok_from_the_drawing_then_moved_and_refused_then_no_solution_is_refused_on_save_as_on_load() {
    // Found by break-it on that first check by load, fixed: from the drawing (y 3) the
    // solve drops k0000003 and solves to y 4: Ok. The load from y 4 (what that save writes)
    // is called conflicting and moves to y 5; the load from y 5 reaches no solution. It
    // returned the first answer, Ok at y 4, and every load of that file was refused. Now a
    // drawing that solved keeps only a solution its own file gives back; here none does, so
    // the save is refused as the last load was, writes nothing, and its load solves the
    // drawing again and refuses alike.
    let (mut s, starts) = by_start(|system| {
        let y = system.values[11];
        Ok(if y == 3.0 {
            dropping(system, 3, 4.0)
        } else if y == 4.0 {
            conflict(system, 5.0)
        } else {
            answer(system, y, SolveStatus::Failed)
        })
    });
    let (save, load) = save_and_load(&mut s);
    assert_eq!(*starts.lock().unwrap(), [3.0, 4.0, 5.0, 3.0, 4.0, 5.0]);
    assert!(save.is_err(), "{save:?}");
    assert_eq!(save, load, "the save's answer and its load's");
}

#[test]
fn ok_from_the_drawing_then_unbuildable_then_no_solution_is_refused_on_save_as_on_load() {
    // Found by break-it on that first check by load, fixed as above: the load from y 4
    // reaches a solution that can't be built (line n1 collapsed to no length:
    // invalid_result) and moves; the load from there reaches nothing. It saved the first,
    // Ok at y 4, and every load from y 4 was refused invalid_result. Now the save is
    // refused too.
    let (mut s, _) = by_start(|system| {
        let y = system.values[11];
        Ok(if y == 3.0 {
            dropping(system, 3, 4.0)
        } else if y == 4.0 {
            let mut a = dropping(system, 3, 5.0);
            (a.values[8], a.values[9]) = (0.0, 0.0);
            a
        } else {
            answer(system, y, SolveStatus::Failed)
        })
    });
    let (save, load) = save_and_load(&mut s);
    assert!(save.is_err(), "{save:?}");
    assert_eq!(save, load, "the save's answer and its load's");
}

#[test]
fn ok_from_the_drawing_whose_load_the_worker_refuses_as_a_start_is_refused_on_save_as_on_load() {
    // Found by break-it on that first check by load, fixed (reachable only if the worker's
    // own check refuses a start the core built from a solution it checked): the load from y
    // 4 is refused invalid_input by the worker; It saved the first answer, Ok at y 4, and
    // every load of that file got the same invalid_input. Now the save gets it too, and
    // writes nothing.
    let (mut s, _) = by_start(|system| {
        let y = system.values[11];
        if y == 3.0 {
            Ok(dropping(system, 3, 4.0))
        } else {
            Err(Error::new(
                ErrorCode::InvalidInput,
                "the worker refuses this start",
            ))
        }
    });
    let (save, load) = save_and_load(&mut s);
    assert_eq!(save, Err((ErrorCode::InvalidInput, Vec::new())));
    assert_eq!(save, load, "the save's answer and its load's");
}

#[test]
fn a_clean_solve_whose_load_is_called_conflicting_is_refused_on_save_as_on_load() {
    // Found by break-it on that first check by load, fixed (the trigger, not the loop): a
    // first solve that dropped nothing and wasn't called over-constrained was never solved
    // again as a load solves it, so a sketch PlaneGCS diagnoses clean on the drawing and
    // conflicting on its solution (a rank that drops at the solution: a point held on a
    // circle and on a line the solution makes tangent to it) saved Ok and loaded refused.
    // Now every solve that reaches a solution is checked by a load.
    let (mut s, _) = by_start(|system| {
        let y = system.values[11];
        Ok(if y == 3.0 {
            answer(system, 4.0, SolveStatus::Success)
        } else {
            conflict(system, y)
        })
    });
    let (save, load) = save_and_load(&mut s);
    assert_eq!(save, load, "the save's answer and its load's");
}

#[test]
fn every_refusal_the_loop_gives_is_its_own_load() {
    // Survives: a refusal saves nothing, so its load solves the same drawing and gives the
    // same refusal, whatever the loop did (settled conflict, no solution from a would-be
    // save, a load's invalid result that settles).
    for script in 0..3 {
        let (mut s, _) = by_start(move |system| {
            let y = system.values[11];
            Ok(match (script, y) {
                (_, 3.0) => dropping(system, 3, 4.0),
                (0, _) => conflict(system, y),
                (1, _) => answer(system, y, SolveStatus::Failed),
                (_, _) => {
                    let mut a = dropping(system, 3, y);
                    (a.values[8], a.values[9]) = (0.0, 0.0);
                    a
                }
            })
        });
        let (save, load) = save_and_load(&mut s);
        assert!(save.is_err(), "script {script}: {save:?}");
        assert_eq!(save, load, "script {script}");
    }
}

#[test]
fn a_load_that_settles_saves_what_every_load_gives() {
    // Survives: moved by each load until one settles, within MAX_LOAD_RESOLVES: the settled
    // load's answer is saved, and the load of that file gives it, bit for bit, naming the
    // same redundant constraint; and the load of that file is solved once (a solve that
    // leaves its file as drawn is its own load).
    for settle in 4..(4 + MAX_LOAD_RESOLVES as u32) {
        let (mut s, starts) = by_start(move |system| {
            let y = system.values[11];
            let to = if y < settle as f64 { y + 1.0 } else { y };
            Ok(dropping(
                system,
                if (to as u32).is_multiple_of(2) { 1 } else { 3 },
                to,
            ))
        });
        let (save, load) = save_and_load(&mut s);
        assert!(save.is_ok(), "settle {settle}: {save:?}");
        assert_eq!(save, load, "settle {settle}");
        let n = starts.lock().unwrap().len();
        let save_solves = 1 + (settle as usize - 3);
        assert_eq!(n, save_solves + 1, "settle {settle}: {:?}", starts.lock());
    }
}
