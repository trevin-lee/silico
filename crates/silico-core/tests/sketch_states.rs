//! Each sketch member's definition state (docs/ports/sketcher.md, *Definition states*, as M5's
//! open item 32 decided it): `Translation::read_diagnosed` on answers the fake sketch
//! solver gives on purpose, and `sketch_states::states`' rules on what it reads, and on
//! sketches the translation refuses. Every value's place is worked out by hand from the translation's layout (the origin
//! and the axes' ends, 0..6; each point's x and y, by ID; each circle's radius; each arc's
//! radius and angles; then the dimensions' values), and every state from the rules. The same
//! states through the sketch worker are silico-sketch's tests/sketch_states.rs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::fixture;

use fixture::{arc, circle, line, point};

use std::collections::{BTreeMap, BTreeSet};

use silico_core::document::{ElementId, Operation, Sketch, read, validate};
use silico_core::fake::FakeSketches;
use silico_core::sketch::{Answer, Diagnosed, Diagnosis, Translation};
use silico_core::sketch_states::{
    ConstraintState, EMPTY_DOFS, EMPTY_STATE, MemberState, SketchState, SketchStates, states,
};
use silico_core::{Error, ErrorCode};
use silico_protocol::{SketchSystem, SolveStatus, Solved, Tag, Value};

use ConstraintState::{Conflicting, PartiallyRedundant, Redundant, Satisfied, Unsolved};
use MemberState::{FullyDefined as Fully, OverDefined as Over, UnderDefined as Under};

/// A sketch of these lists, through the gate.
fn sketch_of(
    points: &[String],
    lines: &[String],
    circles: &[String],
    arcs: &[String],
    constraints: &[&str],
) -> Sketch {
    let text = fixture::document(&fixture::fields(
        &points.join(", "),
        &lines.join(", "),
        &circles.join(", "),
        &arcs.join(", "),
        &constraints.join(", "),
    ));
    let doc = read(text.as_bytes()).unwrap();
    validate(&doc).unwrap();
    match &doc.features[0].operation {
        Operation::Sketch(s) => s.clone(),
        other => panic!("{other:?}"),
    }
}

/// `sketch` solved by the fake, its answer `answer(system)`, as recompute solves it: the
/// answer that stands, its diagnosis, and its states.
fn solved_by(
    sketch: &Sketch,
    answer: impl Fn(&SketchSystem) -> Solved + Send + Sync + 'static,
) -> (Translation, Answer, Diagnosis, SketchStates) {
    let fake = FakeSketches::new();
    fake.answer(move |system| Ok(answer(system)));
    let translation = Translation::new(sketch).unwrap();
    let answer = translation.solve(&mut fake.clone()).unwrap();
    let read = answer.diagnosed().unwrap();
    let got = states(sketch, &read).unwrap();
    (translation, answer, read.diagnosis().clone(), got)
}

fn id(s: &str) -> ElementId {
    ElementId::new(s).unwrap()
}

fn ids(list: &[&str]) -> BTreeSet<ElementId> {
    list.iter().map(|s| id(s)).collect()
}

fn members<S: Copy>(list: &[(&str, S)]) -> BTreeMap<ElementId, S> {
    list.iter().map(|&(s, state)| (id(s), state)).collect()
}

const P1_AT_ORIGIN: [&str; 2] = [
    r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
    r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
];

#[test]
fn an_empty_sketch_is_fully_defined_with_no_degrees_whatever_planegcs_counts() {
    // PlaneGCS counts −1 for no unknowns; item 32 reads an empty sketch as fully defined, 0
    // left, no problem, whatever the answer says (FreeCAD 1.1.3's "Empty sketch", DoF 0).
    let sketch = sketch_of(&[], &[], &[], &[], &[]);
    let (_, _, diagnosis, got) = solved_by(&sketch, |system| Solved {
        dofs: -1,
        no_equations: true,
        ..FakeSketches::as_sent(system)
    });
    assert_eq!(diagnosis.dofs, -1);
    assert_eq!((EMPTY_STATE, EMPTY_DOFS), (SketchState::FullyDefined, 0));
    let empty = SketchStates {
        state: SketchState::FullyDefined,
        dofs: 0,
        points: BTreeMap::new(),
        lines: BTreeMap::new(),
        circles: BTreeMap::new(),
        arcs: BTreeMap::new(),
        constraints: BTreeMap::new(),
        solve_problem: None,
    };
    assert_eq!(got, empty);
    // Even an outcome that's a sketch's error doesn't make an empty sketch anything else.
    let refused = Error::new(ErrorCode::SolveFailed, "no solver could meet it");
    assert_eq!(
        states(&sketch, &Diagnosed::untranslated(&sketch, refused)).unwrap(),
        empty
    );
}

#[test]
fn two_lines_through_a_twice_placed_point_take_its_freedom_not_its_over_definition() {
    // p1 (0, 0) is placed by x and y from the origin and again level with it (a
    // horizontal_points, PlaneGCS's redundant, tag 3); p2 (10, 0) and p3 (0, 10) are each
    // placed. Lines p1–p2 and p1–p3. Values: p1 6, 7; p2 8, 9; p3 10, 11; nothing free.
    // The redundant constraint reads the origin (fixed, no member) and p1: p1 over defined.
    // It names no line, and the lines' points are all fixed: both lines fully defined (item
    // 32's members' rule: a curve takes its points' freedom, not their over-definition).
    let sketch = sketch_of(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 10.0, 0.0),
            point("p0000003", 0.0, 10.0),
        ],
        &[
            line("n0000001", "p0000001", "p0000002"),
            line("n0000002", "p0000001", "p0000003"),
        ],
        &[],
        &[],
        &[
            P1_AT_ORIGIN[0],
            P1_AT_ORIGIN[1],
            r#"{"id": "k0000003", "type": "horizontal_points", "a": "origin", "b": "p0000001"}"#,
            r#"{"id": "k0000004", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 10, "driving": true}"#,
            r#"{"id": "k0000005", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true}"#,
            r#"{"id": "k0000006", "type": "distance_x", "a": "origin", "b": "p0000003", "value": 0, "driving": true}"#,
            r#"{"id": "k0000007", "type": "distance_y", "a": "origin", "b": "p0000003", "value": 10, "driving": true}"#,
        ],
    );
    let (_, answer, diagnosis, got) = solved_by(&sketch, |system| Solved {
        redundant: vec![Tag(3)],
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(system)
    });
    assert!(answer.outcome.is_ok(), "{:?}", answer.outcome);
    assert_eq!(diagnosis.redundant, ids(&["k0000003"]));
    assert_eq!(
        diagnosis.points_read,
        BTreeMap::from([(id("k0000003"), ids(&["p0000001"]))])
    );
    assert_eq!(got.state, SketchState::OverDefined);
    assert_eq!(got.dofs, 0);
    assert_eq!(got.solve_problem, None);
    assert_eq!(
        got.points,
        members(&[("p0000001", Over), ("p0000002", Fully), ("p0000003", Fully)])
    );
    assert_eq!(
        got.lines,
        members(&[("n0000001", Fully), ("n0000002", Fully)])
    );
    assert_eq!(
        got.constraints,
        members(&[
            ("k0000001", Satisfied),
            ("k0000002", Satisfied),
            ("k0000003", Redundant),
            ("k0000004", Satisfied),
            ("k0000005", Satisfied),
            ("k0000006", Satisfied),
            ("k0000007", Satisfied),
        ])
    );
}

#[test]
fn a_failed_solve_has_no_solution_its_members_from_the_diagnosis_its_constraints_unsolved() {
    // A line from p1, placed, to p2 (10, 3), free: values p1 6, 7, p2 8, 9; the fake finds
    // 8 and 9 free and no algorithm solving. No solution: p1 fully defined, p2 and the line
    // under defined; neither constraint flagged, so both unsolved (item 32's `unsolved`).
    let sketch = sketch_of(
        &[point("p0000001", 0.0, 0.0), point("p0000002", 10.0, 3.0)],
        &[line("n0000001", "p0000001", "p0000002")],
        &[],
        &[],
        &P1_AT_ORIGIN,
    );
    let (_, _, _, got) = solved_by(&sketch, |system| Solved {
        dofs: 2,
        free: vec![Value(8), Value(9)],
        status: SolveStatus::Failed,
        algorithm: None,
        ..FakeSketches::as_sent(system)
    });
    assert_eq!(got.state, SketchState::NoSolution);
    assert_eq!(got.dofs, 2);
    assert_eq!(
        got.solve_problem.as_ref().map(Error::code),
        Some(ErrorCode::SolveFailed)
    );
    assert_eq!(
        got.points,
        members(&[("p0000001", Fully), ("p0000002", Under)])
    );
    assert_eq!(got.lines, members(&[("n0000001", Under)]));
    assert_eq!(
        got.constraints,
        members(&[("k0000001", Unsolved), ("k0000002", Unsolved)])
    );
}

/// A line from p1, placed at the origin, to p2 (10, 0), horizontal, 10 long, and `extra`
/// (k0000005).
fn ten_long(extra: &str) -> Sketch {
    sketch_of(
        &[point("p0000001", 0.0, 0.0), point("p0000002", 10.0, 0.0)],
        &[line("n0000001", "p0000001", "p0000002")],
        &[],
        &[],
        &[
            P1_AT_ORIGIN[0],
            P1_AT_ORIGIN[1],
            r#"{"id": "k0000003", "type": "horizontal", "line": "n0000001"}"#,
            r#"{"id": "k0000004", "type": "length", "line": "n0000001", "value": 10, "driving": true}"#,
            extra,
        ],
    )
}

#[test]
fn a_conflict_is_over_defined_its_dofs_as_given_and_a_constraint_listed_twice_reads_conflicting() {
    // The fake finds the two lengths (tags 4 and 5) in conflict, the second redundant too,
    // the horizontal (tag 3) partially redundant, and −1 degrees. Conflicting comes first,
    // so both lengths conflicting; the horizontal partially redundant; the placing
    // dimensions unsolved, the sketch refused. Each length and the horizontal read both
    // ends and name the line: all over defined. dofs −1, PlaneGCS's number as given; the
    // problem `over_constrained`, naming exactly the conflicting two.
    let sketch = ten_long(
        r#"{"id": "k0000005", "type": "length", "line": "n0000001", "value": 12, "driving": true}"#,
    );
    let (_, _, _, got) = solved_by(&sketch, |system| Solved {
        dofs: -1,
        conflicting: vec![Tag(4), Tag(5)],
        redundant: vec![Tag(5)],
        partially_redundant: vec![Tag(3)],
        ..FakeSketches::as_sent(system)
    });
    assert_eq!(got.state, SketchState::OverDefined);
    assert_eq!(got.dofs, -1);
    let problem = got.solve_problem.as_ref().unwrap();
    assert_eq!(problem.code(), ErrorCode::OverConstrained);
    assert_eq!(problem.elements(), ["k0000004", "k0000005"]);
    assert_eq!(
        got.constraints,
        members(&[
            ("k0000001", Unsolved),
            ("k0000002", Unsolved),
            ("k0000003", PartiallyRedundant),
            ("k0000004", Conflicting),
            ("k0000005", Conflicting),
        ])
    );
    assert_eq!(
        got.points,
        members(&[("p0000001", Over), ("p0000002", Over)])
    );
    assert_eq!(got.lines, members(&[("n0000001", Over)]));
}

#[test]
fn decision_5s_disagreeing_redundancy_goes_into_the_conflicting_list_the_read_unchanged() {
    // A distance_x of 10.5 between the line's ends, which the fake calls redundant (tag 5)
    // and leaves at 10 apart: a gap of 0.5 mm, past decision 5's 1e-8, so the read refuses
    // it as over_constrained, naming it. The diagnosis lists it conflicting, and redundant
    // as PlaneGCS listed it; its state is conflicting. It reads both ends (over defined) and
    // names points only, so the line, its ends fixed, is fully defined.
    let sketch = ten_long(
        r#"{"id": "k0000005", "type": "distance_x", "a": "p0000001", "b": "p0000002", "value": 10.5, "driving": true}"#,
    );
    let (translation, answer, diagnosis, got) = solved_by(&sketch, |system| Solved {
        redundant: vec![Tag(5)],
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(system)
    });
    let error = answer.outcome.as_ref().unwrap_err();
    assert_eq!(error.code(), ErrorCode::OverConstrained);
    assert_eq!(error.elements(), ["k0000005"]);
    // The read gives the same error it gave before the diagnosis was kept.
    assert_eq!(
        translation.read(&answer.solved).as_ref().unwrap_err(),
        error
    );
    assert_eq!(diagnosis.conflicting, ids(&["k0000005"]));
    assert_eq!(diagnosis.redundant, ids(&["k0000005"]));
    assert_eq!(
        diagnosis.points_read,
        BTreeMap::from([(id("k0000005"), ids(&["p0000001", "p0000002"]))])
    );
    assert_eq!(got.state, SketchState::OverDefined);
    assert_eq!(got.solve_problem.as_ref(), Some(error));
    assert_eq!(
        got.constraints,
        members(&[
            ("k0000001", Unsolved),
            ("k0000002", Unsolved),
            ("k0000003", Unsolved),
            ("k0000004", Unsolved),
            ("k0000005", Conflicting),
        ])
    );
    assert_eq!(
        got.points,
        members(&[("p0000001", Over), ("p0000002", Over)])
    );
    assert_eq!(got.lines, members(&[("n0000001", Fully)]));
}

#[test]
fn an_agreeing_redundancy_is_kept_and_read_redundant_not_conflicting() {
    // The same distance_x at 10, the fake calling it redundant: met, kept, the sketch solved
    // and over defined, nothing conflicting.
    let sketch = ten_long(
        r#"{"id": "k0000005", "type": "distance_x", "a": "p0000001", "b": "p0000002", "value": 10, "driving": true}"#,
    );
    let (_, answer, diagnosis, got) = solved_by(&sketch, |system| Solved {
        redundant: vec![Tag(5)],
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(system)
    });
    assert!(answer.outcome.is_ok(), "{:?}", answer.outcome);
    assert!(diagnosis.conflicting.is_empty());
    assert_eq!(got.state, SketchState::OverDefined);
    assert_eq!(got.solve_problem, None);
    assert_eq!(got.constraints[&id("k0000005")], Redundant);
    assert_eq!(got.constraints[&id("k0000003")], Satisfied);
}

#[test]
fn the_diagnosis_names_each_free_value_by_the_member_it_belongs_to() {
    // Points p1 (0, 0), p2 (5, 0), p3 (0, 5): values 6..12; circle c1 about p1, radius 5:
    // 12; arc a1 about p1 from p2 to p3: radius 13, angles 14 and 15. A point is free by its
    // x or y, a circle by its radius, an arc by its radius or either angle; with no
    // equations, every one.
    let sketch = sketch_of(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 5.0, 0.0),
            point("p0000003", 0.0, 5.0),
        ],
        &[],
        &[circle("c0000001", "p0000001", 5.0)],
        &[arc("a0000001", "p0000001", "p0000002", "p0000003")],
        &[],
    );
    let translation = Translation::new(&sketch).unwrap();
    assert_eq!(
        translation.system.unknowns,
        (6..16).map(Value).collect::<Vec<_>>()
    );
    let free = |values: &[u32], no_equations: bool| {
        let solved = Solved {
            free: values.iter().copied().map(Value).collect(),
            no_equations,
            ..FakeSketches::as_sent(&translation.system)
        };
        translation
            .read_diagnosed(&solved)
            .unwrap()
            .diagnosis()
            .free
            .clone()
    };
    assert_eq!(free(&[7], false), ids(&["p0000001"]));
    assert_eq!(free(&[8], false), ids(&["p0000002"]));
    assert_eq!(free(&[11], false), ids(&["p0000003"]));
    assert_eq!(free(&[12], false), ids(&["c0000001"]));
    assert_eq!(free(&[13], false), ids(&["a0000001"]));
    assert_eq!(free(&[15], false), ids(&["a0000001"]));
    assert_eq!(free(&[], false), ids(&[]));
    assert_eq!(
        free(&[], true),
        ids(&["a0000001", "c0000001", "p0000001", "p0000002", "p0000003"])
    );
}

#[test]
fn a_flagged_constraints_points_are_those_its_rows_read() {
    // Two lines p1–p2 and p3–p4 and a circle about p5: a parallel reads both lines whole,
    // four points; a radius only the radius, none; an on of p5 on line 1 reads p5 and that
    // line's ends.
    let sketch = sketch_of(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 10.0, 0.0),
            point("p0000003", 0.0, 5.0),
            point("p0000004", 10.0, 5.0),
            point("p0000005", 4.0, 0.0),
        ],
        &[
            line("n0000001", "p0000001", "p0000002"),
            line("n0000002", "p0000003", "p0000004"),
        ],
        &[circle("c0000001", "p0000005", 2.0)],
        &[],
        &[
            r#"{"id": "k0000001", "type": "parallel", "a": "n0000001", "b": "n0000002"}"#,
            r#"{"id": "k0000002", "type": "radius", "curve": "c0000001", "value": 2, "driving": true}"#,
            r#"{"id": "k0000003", "type": "on", "point": "p0000005", "curve": "n0000001"}"#,
        ],
    );
    let translation = Translation::new(&sketch).unwrap();
    let solved = Solved {
        redundant: vec![Tag(1), Tag(2), Tag(3)],
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(&translation.system)
    };
    let read = translation.read_diagnosed(&solved).unwrap();
    assert_eq!(
        read.diagnosis().points_read,
        BTreeMap::from([
            (
                id("k0000001"),
                ids(&["p0000001", "p0000002", "p0000003", "p0000004"])
            ),
            (id("k0000002"), ids(&[])),
            (id("k0000003"), ids(&["p0000001", "p0000002", "p0000005"])),
        ])
    );
}

#[test]
fn a_diagnosis_naming_a_tag_never_sent_is_a_protocol_error() {
    let sketch = ten_long(
        r#"{"id": "k0000005", "type": "length", "line": "n0000001", "value": 10, "driving": true}"#,
    );
    let translation = Translation::new(&sketch).unwrap();
    for solved in [
        Solved {
            redundant: vec![Tag(99)],
            ..FakeSketches::as_sent(&translation.system)
        },
        Solved {
            conflicting: vec![Tag(0)],
            ..FakeSketches::as_sent(&translation.system)
        },
    ] {
        let error = translation.read_diagnosed(&solved).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ProtocolError, "{error}");
        // The read it's the second half of says the same.
        assert_eq!(translation.read(&solved).unwrap_err(), error);
    }
}

/// p1 (3, 4) with a reference x from the origin, k0000001, and a driving y, k0000002.
fn referenced_point() -> Sketch {
    sketch_of(
        &[point("p0000001", 3.0, 4.0)],
        &[],
        &[],
        &[],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 3, "driving": false}"#,
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 4, "driving": true}"#,
        ],
    )
}

#[test]
fn a_reference_dimension_is_never_flagged_satisfied_when_solved_and_unsolved_when_refused() {
    // p1's values are 6 and 7. Solved, x free: the reference satisfied, the point under
    // defined. Refused (no algorithm solving): both constraints unsolved.
    let sketch = referenced_point();
    let translation = Translation::new(&sketch).unwrap();
    let reply = |status, algorithm| Solved {
        dofs: 1,
        free: vec![Value(6)],
        status,
        algorithm,
        ..FakeSketches::as_sent(&translation.system)
    };
    let solved = reply(
        SolveStatus::Success,
        Some(silico_protocol::Algorithm::DogLeg),
    );
    let got = states(&sketch, &translation.read_diagnosed(&solved).unwrap()).unwrap();
    assert_eq!(
        got.constraints,
        members(&[("k0000001", Satisfied), ("k0000002", Satisfied)])
    );
    assert_eq!(got.points, members(&[("p0000001", Under)]));
    assert_eq!(got.state, SketchState::UnderDefined);
    let failed = reply(SolveStatus::Failed, None);
    let got = states(&sketch, &translation.read_diagnosed(&failed).unwrap()).unwrap();
    assert_eq!(got.state, SketchState::NoSolution);
    assert_eq!(
        got.constraints,
        members(&[("k0000001", Unsolved), ("k0000002", Unsolved)])
    );
    // A reply listing the reference (tag 1) in conflict, which check_reply refuses as a tag
    // never sent, handed to the read straight: the reference still isn't flagged, and its
    // rows, none, over-define nothing.
    let listed = Solved {
        conflicting: vec![Tag(1)],
        ..solved
    };
    let read = translation.read_diagnosed(&listed).unwrap();
    assert_eq!(
        read.diagnosis().points_read,
        BTreeMap::from([(id("k0000001"), ids(&[]))])
    );
    let got = states(&sketch, &read).unwrap();
    assert_eq!(
        got.constraints,
        members(&[("k0000001", Unsolved), ("k0000002", Unsolved)])
    );
    assert_eq!(got.points, members(&[("p0000001", Under)]));
}

#[test]
fn a_constraint_listed_redundant_and_partially_redundant_is_redundant() {
    // The driving y (tag 2), met where drawn, listed both ways: kept, redundant.
    let sketch = referenced_point();
    let translation = Translation::new(&sketch).unwrap();
    let reply = |free: Vec<Value>| Solved {
        dofs: 1,
        redundant: vec![Tag(2)],
        partially_redundant: vec![Tag(2)],
        free,
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(&translation.system)
    };
    let got = states(
        &sketch,
        &translation.read_diagnosed(&reply(Vec::new())).unwrap(),
    )
    .unwrap();
    assert_eq!(got.constraints[&id("k0000002")], Redundant);
    assert_eq!(got.points, members(&[("p0000001", Over)]));
    // Over-definition before freedom: the point free in x and read by a flagged row.
    let got = states(
        &sketch,
        &translation.read_diagnosed(&reply(vec![Value(6)])).unwrap(),
    )
    .unwrap();
    assert_eq!(got.points, members(&[("p0000001", Over)]));
    assert_eq!(got.state, SketchState::OverDefined);
}

#[test]
fn the_sketch_state_is_the_problems_code_then_the_flags_then_the_dofs() {
    // Item 32's table, for one point with a driving y. The code first, whatever the
    // diagnosis: a refused sketch's state is its code's, invalid_input (a translation's or a
    // load's refusal of geometry that can't be held) invalid like an invalid result.
    let sketch = referenced_point();
    for (code, want) in [
        (ErrorCode::OverConstrained, SketchState::OverDefined),
        (ErrorCode::SolveFailed, SketchState::NoSolution),
        (ErrorCode::InvalidResult, SketchState::Invalid),
        (ErrorCode::InvalidInput, SketchState::Invalid),
    ] {
        let error = Error::new(code, "refused");
        let got = states(&sketch, &Diagnosed::untranslated(&sketch, error.clone())).unwrap();
        assert_eq!(got.state, want, "{code:?}");
        assert_eq!(got.solve_problem.as_ref(), Some(&error));
        // The point's 2 unknowns, no equations.
        assert_eq!(got.dofs, 2);
    }
    // A refusal with a constraint flagged reads its code's state, not over defined: no
    // algorithm solving, the y listed partially redundant.
    let translation = Translation::new(&sketch).unwrap();
    let reply = |dofs, partially_redundant: Vec<Tag>, status| Solved {
        dofs,
        partially_redundant,
        status,
        algorithm: (status != SolveStatus::Failed).then_some(silico_protocol::Algorithm::DogLeg),
        ..FakeSketches::as_sent(&translation.system)
    };
    let state = |solved: Solved| {
        let got = states(&sketch, &translation.read_diagnosed(&solved).unwrap()).unwrap();
        (got.state, got.dofs, got.solve_problem.map(|e| e.code()))
    };
    assert_eq!(
        state(reply(3, vec![Tag(2)], SolveStatus::Failed)),
        (SketchState::NoSolution, 3, Some(ErrorCode::SolveFailed))
    );
    // Solved: a kept repetition, then the degrees left.
    assert_eq!(
        state(reply(3, vec![Tag(2)], SolveStatus::SolvedExceptRedundant)),
        (SketchState::OverDefined, 3, None)
    );
    assert_eq!(
        state(reply(3, Vec::new(), SolveStatus::Success)),
        (SketchState::UnderDefined, 3, None)
    );
    for dofs in [0, -2] {
        assert_eq!(
            state(reply(dofs, Vec::new(), SolveStatus::Success)),
            (SketchState::FullyDefined, dofs, None),
            "{dofs}"
        );
    }
}

#[test]
fn an_error_that_is_no_sketch_state_has_no_states_and_is_the_error() {
    // A worker's, a reply the core can't read, a bug's: they say nothing about the sketch, so
    // the read fails with them (item 32's addendum, D), never no_solution.
    let sketch = referenced_point();
    for code in [
        ErrorCode::WorkerCrashed,
        ErrorCode::WorkerTimeout,
        ErrorCode::WorkerUnavailable,
        ErrorCode::ProtocolError,
        ErrorCode::Internal,
        ErrorCode::ResourceExhausted,
    ] {
        let error = Error::new(code, "the worker went away");
        let got = states(&sketch, &Diagnosed::untranslated(&sketch, error.clone()));
        assert_eq!(got, Err(error), "{code:?}");
    }
    // An empty sketch too: its state is decided before any solve, but a read of it that
    // failed is still that failure.
    let empty = sketch_of(&[], &[], &[], &[], &[]);
    let crashed = Error::new(ErrorCode::WorkerCrashed, "gone");
    assert_eq!(
        states(&empty, &Diagnosed::untranslated(&empty, crashed.clone())),
        Err(crashed)
    );
}

#[test]
fn a_worker_that_crashes_gives_no_answer_and_no_states() {
    // The fake crashing: the solve is the error, so there's no answer to diagnose; the read
    // of the sketch fails with that error, as the states of it would.
    let sketch = referenced_point();
    let fake = FakeSketches::new();
    fake.answer(|_| {
        Err(Error::new(
            ErrorCode::WorkerCrashed,
            "the sketch worker crashed",
        ))
    });
    let error = Translation::new(&sketch)
        .unwrap()
        .solve(&mut fake.clone())
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::WorkerCrashed);
    assert_eq!(
        states(&sketch, &Diagnosed::untranslated(&sketch, error.clone())),
        Err(error)
    );
}

#[test]
fn a_sketch_the_translation_refuses_is_invalid_every_member_free_and_its_constraints_unsolved() {
    // Two refusals no solver sees (item 32's addendum, C). First a symmetry of p1 and p2,
    // both drawn at (3, 4), about p3: the translation refuses it, invalid_input. The
    // diagnosis of no equations: 3 points, 6 unknowns; every member under defined; the
    // symmetry unsolved.
    let symmetric = sketch_of(
        &[
            point("p0000001", 3.0, 4.0),
            point("p0000002", 3.0, 4.0),
            point("p0000003", 0.0, 0.0),
        ],
        &[line("n0000001", "p0000001", "p0000003")],
        &[],
        &[],
        &[
            r#"{"id": "k0000001", "type": "symmetric", "a": "p0000001", "b": "p0000002", "about": "p0000003"}"#,
        ],
    );
    let refusal = Translation::new(&symmetric).unwrap_err();
    assert_eq!(
        (refusal.code(), refusal.elements()),
        (ErrorCode::InvalidInput, &["k0000001".to_owned()][..])
    );
    assert_eq!(
        Diagnosis::no_equations(&symmetric),
        Diagnosis {
            dofs: 6,
            free: ids(&["p0000001", "p0000002", "p0000003"]),
            ..Diagnosis::default()
        }
    );
    let got = states(
        &symmetric,
        &Diagnosed::untranslated(&symmetric, refusal.clone()),
    )
    .unwrap();
    assert_eq!(
        got,
        SketchStates {
            state: SketchState::Invalid,
            dofs: 6,
            points: members(&[
                ("p0000001", Under),
                ("p0000002", Under),
                ("p0000003", Under)
            ]),
            lines: members(&[("n0000001", Under)]),
            circles: BTreeMap::new(),
            arcs: BTreeMap::new(),
            constraints: members(&[("k0000001", Unsolved)]),
            solve_problem: Some(refusal),
        }
    );
    // Then an arc from (10, 0) to (15, 0) about (5, 0), both ends at angle 0, so it sweeps
    // nothing, and a circle about its centre with its radius fixed: 3 points, 6 unknowns,
    // the circle's radius 1, the arc's radius and angles 3: 10.
    let arc_and_circle = sketch_of(
        &[
            point("p0000001", 5.0, 0.0),
            point("p0000002", 10.0, 0.0),
            point("p0000003", 15.0, 0.0),
        ],
        &[],
        &[circle("c0000001", "p0000001", 2.0)],
        &[arc("a0000001", "p0000001", "p0000002", "p0000003")],
        &[
            r#"{"id": "k0000001", "type": "radius", "curve": "c0000001", "value": 2, "driving": true}"#,
        ],
    );
    let refusal = Translation::new(&arc_and_circle).unwrap_err();
    assert_eq!(
        (refusal.code(), refusal.elements()),
        (ErrorCode::InvalidInput, &["a0000001".to_owned()][..])
    );
    let got = states(
        &arc_and_circle,
        &Diagnosed::untranslated(&arc_and_circle, refusal.clone()),
    )
    .unwrap();
    assert_eq!(got.state, SketchState::Invalid);
    assert_eq!(got.dofs, 10);
    assert_eq!(got.solve_problem, Some(refusal));
    assert_eq!(got.circles, members(&[("c0000001", Under)]));
    assert_eq!(got.arcs, members(&[("a0000001", Under)]));
    assert!(got.points.values().all(|&s| s == Under), "{got:#?}");
    assert_eq!(got.constraints, members(&[("k0000001", Unsolved)]));
}

#[test]
fn an_answer_pairs_its_own_diagnosis_with_its_outcome() {
    // What recompute's answer carries is what reading its reply gives, read once: the same
    // diagnosis and outcome as read_diagnosed of the reply, and read's outcome the second
    // half of it.
    let sketch = ten_long(
        r#"{"id": "k0000005", "type": "distance_x", "a": "p0000001", "b": "p0000002", "value": 10.5, "driving": true}"#,
    );
    let (translation, answer, _, _) = solved_by(&sketch, |system| Solved {
        redundant: vec![Tag(5)],
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(system)
    });
    let read = translation.read_diagnosed(&answer.solved).unwrap();
    assert_eq!(answer.diagnosed().unwrap(), read);
    assert_eq!(read.outcome(), answer.outcome.as_ref());
    assert_eq!(read.diagnosis().conflicting, ids(&["k0000005"]));
    assert_eq!(translation.read(&answer.solved), answer.outcome);
}

#[test]
fn each_states_word_is_the_one_the_decision_spells() {
    let sketch: Vec<&str> = [
        SketchState::UnderDefined,
        SketchState::FullyDefined,
        SketchState::OverDefined,
        SketchState::NoSolution,
        SketchState::Invalid,
    ]
    .map(SketchState::as_str)
    .to_vec();
    assert_eq!(
        sketch,
        [
            "under_defined",
            "fully_defined",
            "over_defined",
            "no_solution",
            "invalid"
        ]
    );
    assert_eq!(
        [Under, Fully, Over].map(MemberState::as_str),
        ["under_defined", "fully_defined", "over_defined"]
    );
    assert_eq!(
        [
            Satisfied,
            Conflicting,
            Redundant,
            PartiallyRedundant,
            Unsolved
        ]
        .map(ConstraintState::as_str),
        [
            "satisfied",
            "conflicting",
            "redundant",
            "partially_redundant",
            "unsolved"
        ]
    );
    assert_eq!(
        [
            Satisfied,
            Conflicting,
            Redundant,
            PartiallyRedundant,
            Unsolved
        ]
        .map(ConstraintState::flagged),
        [false, true, true, true, false]
    );
}
