//! Each sketch member's definition state (docs/ports/sketcher.md, *Definition states*, as M5's
//! open item 32 decided it): `Translation::read_diagnosed` on answers the fake sketch
//! solver gives on purpose, and `sketch_states::states`' rules on what it reads, and on
//! sketches the translation refuses. Every value's place is worked out by hand from the
//! translation's layout (the origin and the axes' ends, 0..6; each point's x and y, by ID;
//! each circle's radius; each arc's radius and angles; then the dimensions' values), and every
//! state from the rules. The same states through the sketch worker are silico-sketch's
//! tests/sketch_states.rs.
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
use silico_protocol::{
    Geometry, PointValues, SketchGeometry, SketchSystem, SolveStatus, Solved, SolverConstraint,
    Tag, Value,
};

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
    let got = states(sketch, read).unwrap();
    let diagnosis = read.diagnosis().clone();
    (translation, answer, diagnosis, got)
}

/// `sketch`, which the translation refuses, read with the fake: its geometry alone diagnosed
/// as `answer` says, beside the refusal. What the fake was sent too.
fn untranslated_by(
    sketch: &Sketch,
    answer: impl Fn(&SketchSystem) -> Solved + Send + Sync + 'static,
) -> (Diagnosed, Vec<SketchSystem>) {
    let fake = FakeSketches::new();
    fake.answer(move |system| Ok(answer(system)));
    let read = Diagnosed::untranslated(sketch, &mut fake.clone())
        .unwrap()
        .unwrap();
    (read, fake.requests())
}

/// A reply to a sketch's geometry alone: `dofs` left, every unknown free.
fn every_unknown_free(dofs: i32) -> impl Fn(&SketchSystem) -> Solved + Send + Sync + 'static {
    move |system| Solved {
        dofs,
        free: system.unknowns.clone(),
        ..FakeSketches::as_sent(system)
    }
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
        dependencies: BTreeSet::new(),
        solve_problem: None,
    };
    assert_eq!(got, empty);
    // Even an outcome that's a sketch's error doesn't make an empty sketch anything else: no
    // algorithm solving it, refused as solve_failed.
    let (_, answer, _, got) = solved_by(&sketch, |system| Solved {
        dofs: -1,
        no_equations: true,
        status: SolveStatus::Failed,
        algorithm: None,
        ..FakeSketches::as_sent(system)
    });
    assert_eq!(
        answer.outcome().map_err(Error::code).unwrap_err(),
        ErrorCode::SolveFailed
    );
    assert_eq!(got, empty);
    // And an empty sketch always translates: no refused translation to state, and nothing
    // sent to diagnose one.
    let fake = FakeSketches::new();
    assert_eq!(
        Diagnosed::untranslated(&sketch, &mut fake.clone()),
        Ok(None)
    );
    assert!(fake.requests().is_empty());
}

/// p1 (0, 0) placed by x and y from the origin and again by `again` (k0000003); p2 (10, 0)
/// and p3 (0, 10) each placed; lines p1–p2 and p1–p3.
fn twice_placed_point(again: &str) -> Sketch {
    sketch_of(
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
            again,
            r#"{"id": "k0000004", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 10, "driving": true}"#,
            r#"{"id": "k0000005", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true}"#,
            r#"{"id": "k0000006", "type": "distance_x", "a": "origin", "b": "p0000003", "value": 0, "driving": true}"#,
            r#"{"id": "k0000007", "type": "distance_y", "a": "origin", "b": "p0000003", "value": 10, "driving": true}"#,
        ],
    )
}

#[test]
fn two_lines_through_a_twice_dimensioned_point_take_its_freedom_not_its_over_definition() {
    // p1 placed again by a second distance_y of 0 from the origin (PlaneGCS's redundant, tag
    // 3). Values: p1 6, 7; p2 8, 9; p3 10, 11; nothing free. By hand, the two distance_y's
    // rows are both p1's y, one dependency of the two (tags 2 and 3), as the fake says. Both
    // are driving dimensions in it, so both read redundant and over-define (addendum 4):
    // each reads the origin (fixed, no member) and p1, so p1 is over defined, and so is the
    // sketch. They name no line, and the lines' points are all fixed: both lines fully
    // defined (item 32's members' rule: a curve takes its points' freedom, not their
    // over-definition).
    let sketch = twice_placed_point(
        r#"{"id": "k0000003", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
    );
    let (_, answer, diagnosis, got) = solved_by(&sketch, |system| Solved {
        redundant: vec![Tag(3)],
        dependent_groups: vec![vec![Tag(2), Tag(3)]],
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(system)
    });
    assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
    assert_eq!(diagnosis.redundant, ids(&["k0000003"]));
    assert_eq!(
        diagnosis.dependencies,
        BTreeSet::from([ids(&["k0000002", "k0000003"])])
    );
    assert_eq!(got.dependencies, diagnosis.dependencies);
    assert_eq!(
        diagnosis.points_depended_on,
        BTreeMap::from([
            (id("k0000002"), ids(&["p0000001"])),
            (id("k0000003"), ids(&["p0000001"]))
        ])
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
            ("k0000002", Redundant),
            ("k0000003", Redundant),
            ("k0000004", Satisfied),
            ("k0000005", Satisfied),
            ("k0000006", Satisfied),
            ("k0000007", Satisfied),
        ])
    );
}

#[test]
fn a_point_placed_again_by_a_relation_stays_fully_defined_its_relation_redundant() {
    // The same point placed again by a horizontal_points with the origin, a relation, which
    // the fake calls redundant (tag 3) as PlaneGCS would. A redundant relation over-defines
    // nothing (addendum 2; SolidWorks' FAQ: "Redundant perpendicular relation added. Sketch
    // is still fully defined."): nothing free, 0 left, so the sketch, p1, and every member
    // fully defined, by their freedom; the relation still listed redundant.
    let sketch = twice_placed_point(
        r#"{"id": "k0000003", "type": "horizontal_points", "a": "origin", "b": "p0000001"}"#,
    );
    let (_, answer, diagnosis, got) = solved_by(&sketch, |system| Solved {
        redundant: vec![Tag(3)],
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(system)
    });
    assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
    assert_eq!(diagnosis.redundant, ids(&["k0000003"]));
    assert_eq!(
        diagnosis.points_depended_on,
        BTreeMap::from([(id("k0000003"), ids(&["p0000001"]))])
    );
    assert_eq!((got.state, got.dofs), (SketchState::FullyDefined, 0));
    assert_eq!(got.solve_problem, None);
    assert_eq!(
        got.points,
        members(&[
            ("p0000001", Fully),
            ("p0000002", Fully),
            ("p0000003", Fully)
        ])
    );
    assert_eq!(
        got.lines,
        members(&[("n0000001", Fully), ("n0000002", Fully)])
    );
    assert_eq!(got.constraints[&id("k0000003")], Redundant);
    // With p1's x free (value 6) and a degree left, the same relation leaves the sketch and
    // p1 under defined, by the freedom: still nothing over defined.
    let (_, _, _, got) = solved_by(&sketch, |system| Solved {
        dofs: 1,
        free: vec![Value(6)],
        redundant: vec![Tag(3)],
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(system)
    });
    assert_eq!((got.state, got.dofs), (SketchState::UnderDefined, 1));
    assert_eq!(
        got.points,
        members(&[
            ("p0000001", Under),
            ("p0000002", Fully),
            ("p0000003", Fully)
        ])
    );
    assert_eq!(
        got.lines,
        members(&[("n0000001", Under), ("n0000002", Under)])
    );
    assert_eq!(got.constraints[&id("k0000003")], Redundant);
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
    // dimensions unsolved, the sketch refused. Each length, conflicting, reads both ends and
    // names the line: all over defined (the horizontal, a partially redundant relation,
    // over-defines nothing, addendum 2, but the lengths already cover what it reads). dofs
    // −1, PlaneGCS's number as given; the problem `over_constrained`, naming exactly the
    // conflicting two.
    let sketch = ten_long(
        r#"{"id": "k0000005", "type": "length", "line": "n0000001", "value": 12, "driving": true}"#,
    );
    let (_, _, _, got) = solved_by(&sketch, |system| Solved {
        dofs: -1,
        conflicting: vec![Tag(4), Tag(5)],
        redundant: vec![Tag(5)],
        partially_redundant: vec![Tag(3)],
        dependent_groups: Vec::new(),
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
    let error = answer.outcome().unwrap_err();
    assert_eq!(error.code(), ErrorCode::OverConstrained);
    assert_eq!(error.elements(), ["k0000005"]);
    // The read gives the same error it gave before the diagnosis was kept.
    assert_eq!(
        translation.read(answer.solved()).as_ref().unwrap_err(),
        error
    );
    assert_eq!(diagnosis.conflicting, ids(&["k0000005"]));
    assert_eq!(diagnosis.redundant, ids(&["k0000005"]));
    assert_eq!(
        diagnosis.points_depended_on,
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
    // The same distance_x at 10, the fake calling it redundant, in one dependency with the
    // length it repeats (by hand: both rows are p2's x less p1's at the drawing): met, kept,
    // the sketch solved and over defined (a driving dimension in a dependency over-defines,
    // addendum 4), nothing conflicting. The length, kept, is in the dependency too, so it
    // reads redundant as well; the horizontal, in none, satisfied.
    let sketch = ten_long(
        r#"{"id": "k0000005", "type": "distance_x", "a": "p0000001", "b": "p0000002", "value": 10, "driving": true}"#,
    );
    let (_, answer, diagnosis, got) = solved_by(&sketch, |system| Solved {
        redundant: vec![Tag(5)],
        dependent_groups: vec![vec![Tag(4), Tag(5)]],
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(system)
    });
    assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
    assert!(diagnosis.conflicting.is_empty());
    assert_eq!(got.state, SketchState::OverDefined);
    assert_eq!(got.solve_problem, None);
    assert_eq!(got.constraints[&id("k0000005")], Redundant);
    assert_eq!(got.constraints[&id("k0000004")], Redundant);
    assert_eq!(got.constraints[&id("k0000003")], Satisfied);
    // What PlaneGCS dropped is still its list alone: the solution keeps the distance_x with
    // its gap, and the length, kept by the solver, has none.
    let solution = answer.outcome().unwrap();
    assert_eq!(
        solution.redundant.keys().collect::<Vec<_>>(),
        [&id("k0000005")]
    );
}

#[test]
fn a_dimension_in_a_dependency_over_defines_whichever_constraint_of_it_planegcs_drops() {
    // M5.0 open item 32's addendum 4. The ten-long line (p1 at the origin by k1 and k2, the
    // horizontal k3, the length k4), its ends' y given again by a horizontal_points (k5) and
    // their x by a distance_x of 10 (k6). By hand, at the drawing: k5's row is k3's (both
    // p2's y less p1's), and k6's is k4's (both p2's x less p1's), so two dependencies, {k3,
    // k5} and {k4, k6}, as the fake says. Whichever of each PlaneGCS drops, the states are
    // the same: both dimensions read redundant (each a driving dimension in a dependency),
    // and over-define what they read and name (p1, p2, and, named by the length, the line);
    // the relations read what the solver lists, the dropped one redundant and the kept one
    // satisfied (a kept relation in a dependency isn't marked, as SolidWorks' FAQ has it).
    // The read lists the two dependencies, each sorted, sorted.
    let sketch = sketch_of(
        &[point("p0000001", 0.0, 0.0), point("p0000002", 10.0, 0.0)],
        &[line("n0000001", "p0000001", "p0000002")],
        &[],
        &[],
        &[
            P1_AT_ORIGIN[0],
            P1_AT_ORIGIN[1],
            r#"{"id": "k0000003", "type": "horizontal", "line": "n0000001"}"#,
            r#"{"id": "k0000004", "type": "length", "line": "n0000001", "value": 10, "driving": true}"#,
            r#"{"id": "k0000005", "type": "horizontal_points", "a": "p0000001", "b": "p0000002"}"#,
            r#"{"id": "k0000006", "type": "distance_x", "a": "p0000001", "b": "p0000002", "value": 10, "driving": true}"#,
        ],
    );
    let dependencies = BTreeSet::from([
        ids(&["k0000003", "k0000005"]),
        ids(&["k0000004", "k0000006"]),
    ]);
    for (dropped, relations) in [
        (vec![Tag(5), Tag(6)], [Satisfied, Redundant]),
        (vec![Tag(3), Tag(4)], [Redundant, Satisfied]),
        (vec![Tag(5), Tag(4)], [Satisfied, Redundant]),
    ] {
        let listed = dropped.clone();
        let (_, answer, diagnosis, got) = solved_by(&sketch, move |system| Solved {
            redundant: {
                let mut r = listed.clone();
                r.sort();
                r
            },
            dependent_groups: vec![vec![Tag(3), Tag(5)], vec![Tag(4), Tag(6)]],
            status: SolveStatus::SolvedExceptRedundant,
            ..FakeSketches::as_sent(system)
        });
        assert!(
            answer.outcome().is_ok(),
            "{dropped:?}: {:?}",
            answer.outcome()
        );
        assert_eq!(diagnosis.dependencies, dependencies, "{dropped:?}");
        assert_eq!(got.dependencies, dependencies, "{dropped:?}");
        assert_eq!(got.state, SketchState::OverDefined, "{dropped:?}");
        assert_eq!(
            got.constraints,
            members(&[
                ("k0000001", Satisfied),
                ("k0000002", Satisfied),
                ("k0000003", relations[0]),
                ("k0000004", Redundant),
                ("k0000005", relations[1]),
                ("k0000006", Redundant),
            ]),
            "{dropped:?}"
        );
        assert_eq!(
            got.points,
            members(&[("p0000001", Over), ("p0000002", Over)]),
            "{dropped:?}"
        );
        assert_eq!(got.lines, members(&[("n0000001", Over)]), "{dropped:?}");
    }
    // The relations' dependency alone (the distance_x left out): the length is in none, so
    // nothing over-defines and the sketch reads by its degrees, 0, every member fully
    // defined; the dropped relation redundant, the kept one satisfied, the dependency listed.
    let relations_only = Sketch {
        constraints: sketch.constraints[..5].to_vec(),
        ..sketch.clone()
    };
    let (_, _, _, got) = solved_by(&relations_only, |system| Solved {
        redundant: vec![Tag(5)],
        dependent_groups: vec![vec![Tag(3), Tag(5)]],
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(system)
    });
    assert_eq!(got.state, SketchState::FullyDefined);
    assert_eq!(
        got.dependencies,
        BTreeSet::from([ids(&["k0000003", "k0000005"])])
    );
    assert_eq!(got.constraints[&id("k0000003")], Satisfied);
    assert_eq!(got.constraints[&id("k0000004")], Satisfied);
    assert_eq!(got.constraints[&id("k0000005")], Redundant);
    assert!(
        got.points
            .values()
            .chain(got.lines.values())
            .all(|&s| s == Fully),
        "{got:#?}"
    );
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
fn a_flagged_constraints_points_are_those_its_equations_depend_on() {
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
        read.diagnosis().points_depended_on,
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
        read.diagnosis().points_depended_on,
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
    // The driving y (tag 2), met where drawn, listed both ways, in a dependency of its own:
    // kept, redundant.
    let sketch = referenced_point();
    let translation = Translation::new(&sketch).unwrap();
    let reply = |free: Vec<Value>| Solved {
        dofs: 1,
        redundant: vec![Tag(2)],
        partially_redundant: vec![Tag(2)],
        dependent_groups: vec![vec![Tag(2)]],
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

/// p1 and p2 both drawn at (3, 4), symmetric about p3 (0, 0), and a line from p1 to p3: the
/// translation refuses the symmetry (the solver measures it by the line between p1 and p2),
/// invalid_input naming it.
fn collapsed_symmetry() -> Sketch {
    sketch_of(
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
    )
}

#[test]
fn the_sketch_state_is_the_problems_code_then_the_over_defining_constraints_then_the_dofs() {
    // Item 32's table, for one point with a driving y, each refusal read from a reply that
    // gives it. The code first, whatever the diagnosis: every reply says 0 degrees left and
    // nothing free, which solved would read fully defined, and the conflict's lists only its
    // one constraint. p1's x is value 6.
    let sketch = referenced_point();
    let translation = Translation::new(&sketch).unwrap();
    let as_sent = FakeSketches::as_sent(&translation.system);
    let mut far = as_sent.values.clone();
    // 2 km out: past the kilometre a solution may reach, so it can't be built.
    far[6] = 2.0e6;
    let solved = Solved {
        dofs: 0,
        free: Vec::new(),
        ..as_sent
    };
    for (reply, code, want) in [
        (
            Solved {
                conflicting: vec![Tag(2)],
                ..solved.clone()
            },
            ErrorCode::OverConstrained,
            SketchState::OverDefined,
        ),
        (
            Solved {
                status: SolveStatus::Failed,
                algorithm: None,
                ..solved.clone()
            },
            ErrorCode::SolveFailed,
            SketchState::NoSolution,
        ),
        (
            Solved {
                values: far,
                ..solved.clone()
            },
            ErrorCode::InvalidResult,
            SketchState::Invalid,
        ),
    ] {
        let read = translation.read_diagnosed(&reply).unwrap();
        let got = states(&sketch, &read).unwrap();
        assert_eq!(got.state, want, "{code:?}");
        assert_eq!(got.solve_problem.as_ref().map(Error::code), Some(code));
        assert_eq!(got.solve_problem.as_ref(), read.outcome().err());
        assert_eq!(got.dofs, 0, "{code:?}");
    }
    // invalid_input, a translation's refusal of geometry that can't be held, is invalid like
    // an invalid result: the collapsed symmetry, its geometry alone diagnosed with 6 left (the
    // fake's, as PlaneGCS counts its 3 points' 6 unknowns), none of its constraints sent.
    let symmetric = collapsed_symmetry();
    let (read, _) = untranslated_by(&symmetric, every_unknown_free(6));
    let got = states(&symmetric, &read).unwrap();
    assert_eq!(got.state, SketchState::Invalid);
    assert_eq!(
        got.solve_problem.as_ref().map(Error::code),
        Some(ErrorCode::InvalidInput)
    );
    assert_eq!(got.dofs, 6);
    // A refusal with a constraint flagged reads its code's state, not over defined: no
    // algorithm solving, the y listed partially redundant, in a dependency of its own.
    let reply = |dofs, partially_redundant: Vec<Tag>, status| Solved {
        dofs,
        dependent_groups: if partially_redundant.is_empty() {
            Vec::new()
        } else {
            vec![partially_redundant.clone()]
        },
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
    // Solved: a driving dimension in a dependency (the y, a distance_y), then the degrees
    // left.
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
fn a_sketch_worker_that_fails_is_the_solves_own_error_with_no_answer_to_state() {
    // A worker that crashes, hangs, can't start, or runs short says nothing about the
    // sketch: the solve passes its error on, as it came, so there's no answer, and so no
    // diagnosis and no states, never no_solution (item 32's addendum, D).
    let sketch = referenced_point();
    for code in [
        ErrorCode::WorkerCrashed,
        ErrorCode::WorkerTimeout,
        ErrorCode::WorkerUnavailable,
        ErrorCode::ResourceExhausted,
    ] {
        let fake = FakeSketches::new();
        fake.answer(move |_| Err(Error::new(code, "the sketch worker went away")));
        let error = Translation::new(&sketch)
            .unwrap()
            .solve(&mut fake.clone())
            .unwrap_err();
        assert_eq!(
            (error.code(), error.message()),
            (code, "the sketch worker went away"),
            "{error}"
        );
    }
}

#[test]
fn an_error_that_is_no_sketch_state_has_no_states_and_is_the_error() {
    // A reply the core can't read, a bug's: they say nothing about the sketch, so the read
    // fails with them (item 32's addendum, D), never no_solution. Each through the path that
    // gives it; a worker that fails is the test above. A reply the core can't read (the
    // worker calls its solution unbuildable, and every piece of it can be built): the
    // answer's outcome is the protocol error, and it has no diagnosis, so no states.
    let sketch = referenced_point();
    let fake = FakeSketches::new();
    fake.answer(|system| {
        Ok(Solved {
            status: SolveStatus::SolvedInvalid,
            algorithm: Some(silico_protocol::Algorithm::DogLeg),
            ..FakeSketches::as_sent(system)
        })
    });
    let answer = Translation::new(&sketch)
        .unwrap()
        .solve(&mut fake.clone())
        .unwrap();
    let error = answer.diagnosed().unwrap_err();
    assert_eq!(error.code(), ErrorCode::ProtocolError, "{error}");
    assert_eq!(answer.outcome(), Err(error));
    // A bug's: a sketch that never passed the gate, a line to a point it doesn't have, which
    // the translation refuses as internal. Its geometry can't be sent alone either, for the
    // same reason, so the read of its refused translation is that error, and nothing is sent.
    let mut broken = collapsed_symmetry();
    broken.lines[0].to = id("p0000009");
    let refusal = Translation::new(&broken).unwrap_err();
    assert_eq!(refusal.code(), ErrorCode::Internal, "{refusal}");
    let fake = FakeSketches::new();
    assert_eq!(
        Diagnosed::untranslated(&broken, &mut fake.clone()),
        Err(refusal)
    );
    assert!(fake.requests().is_empty());
}

#[test]
fn a_solver_that_fails_on_a_refused_translations_geometry_is_the_reads_error() {
    // A refused translation's diagnosis is the worker's, of its geometry alone, so a worker
    // that fails says nothing about the sketch: the read fails with its error, as it came
    // (item 32's addendum, D), never a state.
    let symmetric = collapsed_symmetry();
    for code in [
        ErrorCode::WorkerCrashed,
        ErrorCode::WorkerTimeout,
        ErrorCode::WorkerUnavailable,
        ErrorCode::ResourceExhausted,
    ] {
        let fake = FakeSketches::new();
        fake.answer(move |_| Err(Error::new(code, "the sketch worker went away")));
        let error = Diagnosed::untranslated(&symmetric, &mut fake.clone()).unwrap_err();
        assert_eq!(
            (error.code(), error.message()),
            (code, "the sketch worker went away"),
            "{error}"
        );
    }
    // A reply that can't be trusted, one listing a tag where none was sent, or moving a
    // fixed value (the origin's x, value 0): a protocol error, the read's.
    let replies: [fn(&SketchSystem) -> Solved; 2] = [
        |system: &SketchSystem| Solved {
            redundant: vec![Tag(1)],
            ..FakeSketches::as_sent(system)
        },
        |system: &SketchSystem| {
            let mut values = system.values.clone();
            values[0] = 1.0;
            Solved {
                values,
                ..FakeSketches::as_sent(system)
            }
        },
    ];
    for reply in replies {
        let fake = FakeSketches::new();
        fake.answer(move |system| Ok(reply(system)));
        let error = Diagnosed::untranslated(&symmetric, &mut fake.clone()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ProtocolError, "{error}");
    }
}

#[test]
fn a_sketch_the_translation_refuses_is_invalid_every_member_free_and_its_constraints_unsolved() {
    // Two refusals no solve is sent (item 32's addendum, C), each with its geometry alone
    // diagnosed by the solver: the fake's answers, PlaneGCS's counts (silico-sketch's
    // tests/sketch_states.rs has the worker's). First a symmetry of p1 and p2, both drawn at
    // (3, 4), about p3: the translation refuses it, invalid_input. Sent: the 3 points' 6
    // unknowns, values 6 to 11, and no row, the symmetry not among them; PlaneGCS's answer
    // for no rows, 6 left and no equations, so every member under defined; the symmetry
    // unsolved.
    let symmetric = collapsed_symmetry();
    let refusal = Translation::new(&symmetric).unwrap_err();
    assert_eq!(
        (refusal.code(), refusal.elements()),
        (ErrorCode::InvalidInput, &["k0000001".to_owned()][..])
    );
    let (read, sent) = untranslated_by(&symmetric, |system| Solved {
        dofs: 6,
        no_equations: true,
        ..FakeSketches::as_sent(system)
    });
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].unknowns, (6..12).map(Value).collect::<Vec<_>>());
    assert_eq!(sent[0].constraints, []);
    assert_eq!(
        read.diagnosis(),
        &Diagnosis {
            dofs: 6,
            free: ids(&["p0000001", "p0000002", "p0000003"]),
            ..Diagnosis::default()
        }
    );
    let got = states(&symmetric, &read).unwrap();
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
            dependencies: BTreeSet::new(),
            solve_problem: Some(refusal),
        }
    );
    // The freedom is the reply's, not the core's: a reply finding p1's x alone free leaves
    // p2 and p3 fully defined, and the line, through p1, under defined.
    let (read, _) = untranslated_by(&symmetric, |system| Solved {
        dofs: 6,
        free: vec![Value(6)],
        ..FakeSketches::as_sent(system)
    });
    let got = states(&symmetric, &read).unwrap();
    assert_eq!(
        got.points,
        members(&[
            ("p0000001", Under),
            ("p0000002", Fully),
            ("p0000003", Fully)
        ])
    );
    assert_eq!(got.lines, members(&[("n0000001", Under)]));
    // Then an arc from (10, 0) to (15, 0) about (5, 0), both ends at angle 0, so it sweeps
    // nothing, and a circle about its centre with its radius fixed. Sent: the points, values
    // 6 to 11; the circle's radius, 12, as drawn, 2; the arc's radius, its start's distance
    // from its centre, 5, and both its angles its start's, 0, values 13 to 15; and the arc's
    // rules alone (geometry 7: the origin, the axes, 3 points, the circle, then the arc). The
    // fake counts 6 left, as PlaneGCS does (6 + 1 + 3 − 4).
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
    let (read, sent) = untranslated_by(&arc_and_circle, every_unknown_free(6));
    assert_eq!(sent[0].values[12..], [2.0, 5.0, 0.0, 0.0]);
    assert_eq!(sent[0].unknowns, (6..16).map(Value).collect::<Vec<_>>());
    assert_eq!(
        sent[0].constraints,
        [SolverConstraint::ArcRules { arc: Geometry(7) }]
    );
    let got = states(&arc_and_circle, &read).unwrap();
    assert_eq!(got.state, SketchState::Invalid);
    assert_eq!(got.dofs, 6);
    assert_eq!(got.solve_problem, Some(refusal));
    assert_eq!(got.circles, members(&[("c0000001", Under)]));
    assert_eq!(got.arcs, members(&[("a0000001", Under)]));
    assert!(got.points.values().all(|&s| s == Under), "{got:#?}");
    assert_eq!(got.constraints, members(&[("k0000001", Unsolved)]));
}

#[test]
fn a_lone_arc_that_sweeps_nothing_is_sent_at_its_starts_angle_at_both_ends() {
    // An arc about p1 (0, 0) from p2 (5, 0) to p3 (8, 0), both ends at angle 0: it sweeps
    // nothing, so the translation refuses it, and its geometry alone is sent to be diagnosed.
    // Values: the points 6 to 11; the arc's radius, 5, its start's distance from its centre
    // (not its end's, 8), and its angles, both its start's, 0, 12 to 14. Geometry: the origin
    // and the axes, 0 to 2; the points, 3 to 5; the arc, 6, its rules the one row sent. The
    // count, 5, is the worker's (silico-sketch's tests/sketch_states.rs); the fake's is that.
    let lone = sketch_of(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 5.0, 0.0),
            point("p0000003", 8.0, 0.0),
        ],
        &[],
        &[],
        &[arc("a0000001", "p0000001", "p0000002", "p0000003")],
        &[],
    );
    let refusal = Translation::new(&lone).unwrap_err();
    assert_eq!(
        (refusal.code(), refusal.elements()),
        (ErrorCode::InvalidInput, &["a0000001".to_owned()][..])
    );
    let (read, sent) = untranslated_by(&lone, every_unknown_free(5));
    let at = |x, y| PointValues {
        x: Value(x),
        y: Value(y),
    };
    assert_eq!(
        sent[0].values[6..],
        [0.0, 0.0, 5.0, 0.0, 8.0, 0.0, 5.0, 0.0, 0.0]
    );
    assert_eq!(sent[0].unknowns, (6..15).map(Value).collect::<Vec<_>>());
    assert_eq!(
        sent[0].geometry[6..],
        [SketchGeometry::Arc {
            center: at(6, 7),
            radius: Value(12),
            start_angle: Value(13),
            end_angle: Value(14),
            start: at(8, 9),
            end: at(10, 11),
        }]
    );
    assert_eq!(
        sent[0].constraints,
        [SolverConstraint::ArcRules { arc: Geometry(6) }]
    );
    assert_eq!(
        read.diagnosis(),
        &Diagnosis {
            dofs: 5,
            free: ids(&["a0000001", "p0000001", "p0000002", "p0000003"]),
            ..Diagnosis::default()
        }
    );
    let got = states(&lone, &read).unwrap();
    assert_eq!(
        got,
        SketchStates {
            state: SketchState::Invalid,
            dofs: 5,
            points: members(&[
                ("p0000001", Under),
                ("p0000002", Under),
                ("p0000003", Under)
            ]),
            lines: BTreeMap::new(),
            circles: BTreeMap::new(),
            arcs: members(&[("a0000001", Under)]),
            constraints: BTreeMap::new(),
            dependencies: BTreeSet::new(),
            solve_problem: Some(refusal),
        }
    );
}

#[test]
fn a_sketchs_geometry_alone_is_sent_as_its_translation_sends_it_without_its_constraints() {
    // What Diagnosis::untranslated sends is Translation::new's system, its constraints'
    // rows and their values dropped: a quarter arc about p1 (0, 0) from p2 (5, 0) to p3
    // (0, 5), with a radius and a horizontal line p1–p2, which translates. The same points,
    // arc, line, values, and unknowns, and the arc's rules its one row; one algorithm.
    let sketch = sketch_of(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 5.0, 0.0),
            point("p0000003", 0.0, 5.0),
        ],
        &[line("n0000001", "p0000001", "p0000002")],
        &[],
        &[arc("a0000001", "p0000001", "p0000002", "p0000003")],
        &[
            r#"{"id": "k0000001", "type": "radius", "curve": "a0000001", "value": 5, "driving": true}"#,
            r#"{"id": "k0000002", "type": "horizontal", "line": "n0000001"}"#,
        ],
    );
    let translated = Translation::new(&sketch).unwrap().system;
    let fake = FakeSketches::new();
    let diagnosis = Diagnosis::untranslated(&sketch, &mut fake.clone()).unwrap();
    let sent = fake.requests();
    assert_eq!(sent.len(), 1);
    let alone = &sent[0];
    // 6 fixed, 6 point coordinates, the arc's 3: the radius's value, 5, comes after them.
    assert_eq!(alone.values.len(), 15);
    assert_eq!(alone.values[..], translated.values[..15]);
    assert_eq!(translated.values[15..], [5.0]);
    assert_eq!(alone.unknowns, translated.unknowns);
    assert_eq!(alone.geometry, translated.geometry);
    // One algorithm, not the translation's four: only the diagnosis is read.
    assert_eq!(alone.algorithms, [silico_protocol::Algorithm::DogLeg]);
    assert_eq!(translated.algorithms.len(), 4);
    assert_eq!(
        alone.constraints,
        [SolverConstraint::ArcRules { arc: Geometry(7) }]
    );
    assert_eq!(
        translated
            .constraints
            .iter()
            .filter(|c| c.tag().is_none())
            .collect::<Vec<_>>(),
        [&SolverConstraint::ArcRules { arc: Geometry(7) }]
    );
    // The fake's as-sent answer: 0 left, nothing free.
    assert_eq!(diagnosis, Diagnosis::default());
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
    let read = translation.read_diagnosed(answer.solved()).unwrap();
    assert_eq!(answer.diagnosed(), Ok(&read));
    assert_eq!(read.outcome(), answer.outcome());
    assert_eq!(read.diagnosis().conflicting, ids(&["k0000005"]));
    assert_eq!(translation.read(answer.solved()).as_ref(), answer.outcome());
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

/// A solver that answers with `reply` and counts how often it's distrusted.
struct CountsDistrust {
    reply: fn(&SketchSystem) -> Result<Solved, Error>,
    distrusted: usize,
}

impl silico_core::sketch::SketchSolver for CountsDistrust {
    fn solve(&mut self, system: SketchSystem) -> Result<Solved, Error> {
        (self.reply)(&system)
    }

    fn distrust(&mut self) {
        self.distrusted += 1;
    }
}

#[test]
fn a_refused_translations_untrusted_reply_distrusts_the_worker_and_no_other_does() {
    // As every solve does: a reply that can't be trusted (a tag listed where no constraint
    // was sent) distrusts the worker, once; a good reply and a worker's own error don't.
    let symmetric = collapsed_symmetry();
    let mut bad = CountsDistrust {
        reply: |system| {
            Ok(Solved {
                redundant: vec![Tag(1)],
                ..FakeSketches::as_sent(system)
            })
        },
        distrusted: 0,
    };
    let error = Diagnosed::untranslated(&symmetric, &mut bad).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ProtocolError);
    assert_eq!(bad.distrusted, 1);
    let mut good = CountsDistrust {
        reply: |system| Ok(FakeSketches::as_sent(system)),
        distrusted: 0,
    };
    assert!(
        Diagnosed::untranslated(&symmetric, &mut good)
            .unwrap()
            .is_some()
    );
    assert_eq!(good.distrusted, 0);
    let mut crashed = CountsDistrust {
        reply: |_| Err(Error::new(ErrorCode::WorkerCrashed, "gone")),
        distrusted: 0,
    };
    assert_eq!(
        Diagnosed::untranslated(&symmetric, &mut crashed)
            .unwrap_err()
            .code(),
        ErrorCode::WorkerCrashed
    );
    assert_eq!(crashed.distrusted, 0);
}

#[test]
fn a_refused_translations_geometry_is_sent_for_its_diagnosis_alone() {
    // One algorithm, the least the protocol takes: only the diagnosis is read.
    let mut fake = FakeSketches::new();
    Diagnosed::untranslated(&collapsed_symmetry(), &mut fake)
        .unwrap()
        .unwrap();
    let sent = fake.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].algorithms, [silico_protocol::Algorithm::DogLeg]);
}
