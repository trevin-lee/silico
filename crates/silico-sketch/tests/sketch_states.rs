//! `silico_core::sketch_states` through the sketch worker (docs/ports/sketcher.md,
//! *Definition states*): each probe case of the note solved by PlaneGCS, its diagnosis read
//! back with its outcome (`Answer::diagnosed`), and every member's state checked against the state
//! worked out by hand from the geometry and the note's rules, with FreeCAD 1.1.3's degrees of
//! freedom and constraint lists where the note's probe ran.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::fixture::{arc, circle, document, fields, line, line_as, point};
use super::systems::Rng;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use proptest::prelude::*;
use silico_core::ErrorCode;
use silico_core::document::{ElementId, Operation, Sketch, read, validate};
use silico_core::sketch::{
    Answer, Diagnosed, Diagnosis, EXTENT_TOLERANCE, LEAST_TANGENCY, REDUNDANCY_GAP_MM,
    SketchSolver, SketchWorker, TANGENCY_TOLERANCE, Translation,
};
use silico_core::sketch_states::{ConstraintState, MemberState, SketchState, SketchStates, states};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

use ConstraintState::{Conflicting, Redundant, Satisfied, Unsolved};
use MemberState::{FullyDefined as Fully, OverDefined as Over, UnderDefined as Under};

/// A sketch of these lists, through the gate.
fn sketch_of(
    points: &[String],
    lines: &[String],
    circles: &[String],
    arcs: &[String],
    constraints: &[&str],
) -> Sketch {
    let text = document(&fields(
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

/// `sketch` solved by the worker, as recompute solves it, with its diagnosis and states.
fn solved(sketch: &Sketch) -> (Answer, Diagnosis, SketchStates) {
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let translation = Translation::new(sketch).unwrap();
    let answer = translation.solve(&mut worker).unwrap();
    let read = answer.diagnosed().unwrap();
    let states = states(sketch, read).unwrap();
    let diagnosis = read.diagnosis().clone();
    (answer, diagnosis, states)
}

fn members<S: Copy>(list: &[(&str, S)]) -> BTreeMap<ElementId, S> {
    list.iter()
        .map(|&(id, s)| (ElementId::new(id).unwrap(), s))
        .collect()
}

/// The states expected: the solve's problem's code (none for a solved sketch), the
/// sketch's state and degrees of freedom, and each list whole, member by member.
struct Want<'a> {
    problem: Option<ErrorCode>,
    state: SketchState,
    dofs: i32,
    points: &'a [(&'a str, MemberState)],
    lines: &'a [(&'a str, MemberState)],
    circles: &'a [(&'a str, MemberState)],
    arcs: &'a [(&'a str, MemberState)],
    constraints: &'a [(&'a str, ConstraintState)],
}

/// That `got` is `want`. An `over_constrained` problem names exactly the constraints that
/// are `conflicting`.
#[track_caller]
fn expect(got: &SketchStates, want: Want<'_>) {
    let Want {
        problem,
        state,
        dofs,
        points,
        lines,
        circles,
        arcs,
        constraints,
    } = want;
    assert_eq!(
        got.solve_problem.as_ref().map(|e| e.code()),
        problem,
        "{got:#?}"
    );
    if problem == Some(ErrorCode::OverConstrained) {
        let named: BTreeSet<&str> = got
            .solve_problem
            .iter()
            .flat_map(|e| e.elements())
            .map(String::as_str)
            .collect();
        let conflicting: BTreeSet<&str> = got
            .constraints
            .iter()
            .filter(|(_, s)| **s == Conflicting)
            .map(|(id, _)| id.as_str())
            .collect();
        assert_eq!(named, conflicting);
    }
    assert_eq!(got.state, state, "{got:#?}");
    assert_eq!(got.dofs, dofs, "{got:#?}");
    assert_eq!(got.points, members(points), "points");
    assert_eq!(got.lines, members(lines), "lines");
    assert_eq!(got.circles, members(circles), "circles");
    assert_eq!(got.arcs, members(arcs), "arcs");
    assert_eq!(got.constraints, members(constraints), "constraints");
}

const FROM_AT_ORIGIN: [&str; 2] = [
    r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
    r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
];
const HORIZONTAL: &str = r#"{"id": "k0000003", "type": "horizontal", "line": "n0000001"}"#;
const LENGTH_10: &str =
    r#"{"id": "k0000004", "type": "length", "line": "n0000001", "value": 10, "driving": true}"#;

fn line_points(to: (f64, f64)) -> Vec<String> {
    vec![point("p0000001", 0.0, 0.0), point("p0000002", to.0, to.1)]
}

fn one_line() -> Vec<String> {
    vec![line("n0000001", "p0000001", "p0000002")]
}

#[test]
fn an_empty_sketch_is_fully_defined_with_no_degrees_of_freedom() {
    // FreeCAD 1.1.3: "Empty sketch", DoF 0, FullyConstrained. PlaneGCS counts −1 for no
    // unknowns, which isn't over-defined.
    let sketch = sketch_of(&[], &[], &[], &[], &[]);
    let (_, _, got) = solved(&sketch);
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::FullyDefined,
            dofs: 0,
            points: &[],
            lines: &[],
            circles: &[],
            arcs: &[],
            constraints: &[],
        },
    );
}

#[test]
fn a_free_line_and_its_ends_are_under_defined_two_degrees_a_point() {
    // No equations: every value free (FreeCAD's reading). Two points, 2 each: 4 (1.1.3: 4).
    let sketch = sketch_of(&line_points((10.0, 3.0)), &one_line(), &[], &[], &[]);
    let (answer, _, got) = solved(&sketch);
    assert!(answer.outcome().is_ok());
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::UnderDefined,
            dofs: 4,
            points: &[("p0000001", Under), ("p0000002", Under)],
            lines: &[("n0000001", Under)],
            circles: &[],
            arcs: &[],
            constraints: &[],
        },
    );
}

#[test]
fn a_lone_free_point_is_under_defined_two_degrees_not_empty() {
    // Points and nothing else: no curve, no constraint, but not an empty sketch. One point,
    // its 2 values free (FreeCAD 1.1.3: a lone point, DoF 2).
    let sketch = sketch_of(&[point("p0000001", 3.0, 4.0)], &[], &[], &[], &[]);
    let (answer, _, got) = solved(&sketch);
    assert!(answer.outcome().is_ok());
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::UnderDefined,
            dofs: 2,
            points: &[("p0000001", Under)],
            lines: &[],
            circles: &[],
            arcs: &[],
            constraints: &[],
        },
    );
}

#[test]
fn a_line_from_the_origin_with_its_end_free_has_its_start_fully_defined_and_the_rest_under() {
    // `from` placed by x and y from the origin: its 2 values fixed; `to`'s 2 free, so the
    // line is under-defined, by `to`, and 2 degrees are left. FreeCAD would draw both ends
    // free (a point takes its curve's state there).
    let sketch = sketch_of(
        &line_points((10.0, 3.0)),
        &one_line(),
        &[],
        &[],
        &FROM_AT_ORIGIN,
    );
    let (_, _, got) = solved(&sketch);
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::UnderDefined,
            dofs: 2,
            points: &[("p0000001", Fully), ("p0000002", Under)],
            lines: &[("n0000001", Under)],
            circles: &[],
            arcs: &[],
            constraints: &[("k0000001", Satisfied), ("k0000002", Satisfied)],
        },
    );
}

#[test]
fn a_line_from_the_origin_horizontal_with_its_length_is_fully_defined_throughout() {
    // 4 unknowns, 4 independent equations (x and y of `from`, horizontal, length): 0 left.
    let constraints = [FROM_AT_ORIGIN[0], FROM_AT_ORIGIN[1], HORIZONTAL, LENGTH_10];
    let sketch = sketch_of(
        &line_points((10.0, 0.0)),
        &one_line(),
        &[],
        &[],
        &constraints,
    );
    let (_, _, got) = solved(&sketch);
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::FullyDefined,
            dofs: 0,
            points: &[("p0000001", Fully), ("p0000002", Fully)],
            lines: &[("n0000001", Fully)],
            circles: &[],
            arcs: &[],
            constraints: &[
                ("k0000001", Satisfied),
                ("k0000002", Satisfied),
                ("k0000003", Satisfied),
                ("k0000004", Satisfied),
            ],
        },
    );
}

#[test]
fn a_construction_line_takes_the_same_states_as_a_line() {
    // Construction geometry is solved like any other: the end-free line's case again.
    let sketch = sketch_of(
        &line_points((10.0, 3.0)),
        &[line_as("n0000001", "p0000001", "p0000002", true)],
        &[],
        &[],
        &FROM_AT_ORIGIN,
    );
    let (_, _, got) = solved(&sketch);
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::UnderDefined,
            dofs: 2,
            points: &[("p0000001", Fully), ("p0000002", Under)],
            lines: &[("n0000001", Under)],
            circles: &[],
            arcs: &[],
            constraints: &[("k0000001", Satisfied), ("k0000002", Satisfied)],
        },
    );
}

#[test]
fn a_repeated_horizontal_is_redundant_and_leaves_the_line_and_its_ends_fully_defined() {
    // A second horizontal on the dimensioned line: one equation repeating another, which
    // agrees, so the sketch solves (M3.3). The later of the two by tag is the redundant one
    // (PlaneGCS's popularity contest, as FreeCAD's). A horizontal is a relation, so its
    // repetition over-defines nothing (addendum 2, SolidWorks' FAQ: a redundant relation
    // leaves the sketch fully defined): re-derived by the new rule, 4 unknowns (the two ends'
    // x and y), 5 rows of rank 4 (from's x and y, horizontal, length), 0 left, so no value
    // free (a free value is a column past the rank: 4 − 4 = 0 of them), both ends and the
    // line fully defined, and the sketch too.
    let constraints = [
        FROM_AT_ORIGIN[0],
        FROM_AT_ORIGIN[1],
        HORIZONTAL,
        LENGTH_10,
        r#"{"id": "k0000005", "type": "horizontal", "line": "n0000001"}"#,
    ];
    let sketch = sketch_of(
        &line_points((10.0, 0.0)),
        &one_line(),
        &[],
        &[],
        &constraints,
    );
    let (answer, _, got) = solved(&sketch);
    let solution = answer.outcome().unwrap();
    assert_eq!(
        solution
            .redundant
            .keys()
            .map(ElementId::as_str)
            .collect::<Vec<_>>(),
        ["k0000005"]
    );
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::FullyDefined,
            dofs: 0,
            points: &[("p0000001", Fully), ("p0000002", Fully)],
            lines: &[("n0000001", Fully)],
            circles: &[],
            arcs: &[],
            constraints: &[
                ("k0000001", Satisfied),
                ("k0000002", Satisfied),
                ("k0000003", Satisfied),
                ("k0000004", Satisfied),
                ("k0000005", Redundant),
            ],
        },
    );
}

#[test]
fn two_lines_sharing_a_point_give_it_one_state_and_each_line_its_own() {
    // The dimensioned line, and a second from its end to a free point: the shared point is
    // fixed by the first line's constraints, so fully defined whichever line uses it; the
    // second line is under defined by its free end; 2 left (1.1.3: 2).
    let constraints = [FROM_AT_ORIGIN[0], FROM_AT_ORIGIN[1], HORIZONTAL, LENGTH_10];
    let mut points = line_points((10.0, 0.0));
    points.push(point("p0000003", 14.0, 6.0));
    let lines = [
        line("n0000001", "p0000001", "p0000002"),
        line("n0000002", "p0000002", "p0000003"),
    ];
    let sketch = sketch_of(&points, &lines, &[], &[], &constraints);
    let (_, _, got) = solved(&sketch);
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::UnderDefined,
            dofs: 2,
            points: &[
                ("p0000001", Fully),
                ("p0000002", Fully),
                ("p0000003", Under),
            ],
            lines: &[("n0000001", Fully), ("n0000002", Under)],
            circles: &[],
            arcs: &[],
            constraints: &[
                ("k0000001", Satisfied),
                ("k0000002", Satisfied),
                ("k0000003", Satisfied),
                ("k0000004", Satisfied),
            ],
        },
    );
}

#[test]
fn shared_points_between_construction_lines_take_the_same_states() {
    let constraints = [FROM_AT_ORIGIN[0], FROM_AT_ORIGIN[1], HORIZONTAL, LENGTH_10];
    let points = [
        r#"{"id": "p0000001", "x": 0, "y": 0, "construction": true}"#.to_owned(),
        r#"{"id": "p0000002", "x": 10, "y": 0, "construction": true}"#.to_owned(),
        r#"{"id": "p0000003", "x": 14, "y": 6, "construction": true}"#.to_owned(),
    ];
    let lines = [
        line_as("n0000001", "p0000001", "p0000002", true),
        line_as("n0000002", "p0000002", "p0000003", true),
    ];
    let sketch = sketch_of(&points, &lines, &[], &[], &constraints);
    let (_, _, got) = solved(&sketch);
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::UnderDefined,
            dofs: 2,
            points: &[
                ("p0000001", Fully),
                ("p0000002", Fully),
                ("p0000003", Under),
            ],
            lines: &[("n0000001", Fully), ("n0000002", Under)],
            circles: &[],
            arcs: &[],
            constraints: &[
                ("k0000001", Satisfied),
                ("k0000002", Satisfied),
                ("k0000003", Satisfied),
                ("k0000004", Satisfied),
            ],
        },
    );
}

#[test]
fn a_circle_with_only_its_centre_fixed_is_under_defined_by_its_radius() {
    // Centre placed from the origin: its 2 values fixed; the radius free: 1 left (1.1.3: the
    // circle centred on the root point, DoF 1, its edge dependent).
    let sketch = sketch_of(
        &[point("p0000001", 0.0, 0.0)],
        &[],
        &[circle("c0000001", "p0000001", 5.0)],
        &[],
        &FROM_AT_ORIGIN,
    );
    let (_, _, got) = solved(&sketch);
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::UnderDefined,
            dofs: 1,
            points: &[("p0000001", Fully)],
            lines: &[],
            circles: &[("c0000001", Under)],
            arcs: &[],
            constraints: &[("k0000001", Satisfied), ("k0000002", Satisfied)],
        },
    );
}

#[test]
fn a_circle_with_only_its_radius_fixed_is_under_defined_by_its_centre() {
    // The radius dimensioned, the centre free: 2 left, the centre under defined, and the
    // circle by it.
    let sketch = sketch_of(
        &[point("p0000001", 3.0, 4.0)],
        &[],
        &[circle("c0000001", "p0000001", 5.0)],
        &[],
        &[
            r#"{"id": "k0000001", "type": "radius", "curve": "c0000001", "value": 5, "driving": true}"#,
        ],
    );
    let (_, _, got) = solved(&sketch);
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::UnderDefined,
            dofs: 2,
            points: &[("p0000001", Under)],
            lines: &[],
            circles: &[("c0000001", Under)],
            arcs: &[],
            constraints: &[("k0000001", Satisfied)],
        },
    );
}

/// An arc about p0000001 at the origin, radius 5, from (5, 0) to (0, 5).
fn quarter_arc() -> (Vec<String>, Vec<String>) {
    (
        vec![
            point("p0000001", 0.0, 0.0),
            point("p0000002", 5.0, 0.0),
            point("p0000003", 0.0, 5.0),
        ],
        vec![arc("a0000001", "p0000001", "p0000002", "p0000003")],
    )
}

#[test]
fn an_arc_with_only_its_centre_and_radius_fixed_has_its_ends_and_itself_under_defined() {
    // An arc's 5 degrees (centre, radius, two angles; its ends follow by its rules): centre
    // and radius fixed leave its two angles, so its ends are free along the circle. Centre
    // fully defined; ends, and the arc, under defined; 2 left (1.1.3: 2, edge, start, and
    // end dependent, the centre not).
    let (points, arcs) = quarter_arc();
    let constraints = [
        FROM_AT_ORIGIN[0],
        FROM_AT_ORIGIN[1],
        r#"{"id": "k0000003", "type": "radius", "curve": "a0000001", "value": 5, "driving": true}"#,
    ];
    let sketch = sketch_of(&points, &[], &[], &arcs, &constraints);
    let (_, _, got) = solved(&sketch);
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::UnderDefined,
            dofs: 2,
            points: &[
                ("p0000001", Fully),
                ("p0000002", Under),
                ("p0000003", Under),
            ],
            lines: &[],
            circles: &[],
            arcs: &[("a0000001", Under)],
            constraints: &[
                ("k0000001", Satisfied),
                ("k0000002", Satisfied),
                ("k0000003", Satisfied),
            ],
        },
    );
}

#[test]
fn an_arc_whose_centre_and_ends_are_all_placed_repeats_one_dimension_and_stays_fully_defined_itself()
 {
    // Three points fix 6 values against the arc's 5 (1.1.3: DoF 0, redundant [5]): one of
    // the six dimensions is redundant, it agrees (the arc is drawn on them), and, a driving
    // dimension, it over-defines (addendum 2): the point it reads is over defined, and so is
    // the sketch. No dimension names the arc, so the arc isn't over defined: it
    // takes its points' freedom, none, not their over-definition. The sketch is over defined.
    let (points, arcs) = quarter_arc();
    let constraints = [
        FROM_AT_ORIGIN[0],
        FROM_AT_ORIGIN[1],
        r#"{"id": "k0000003", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 5, "driving": true}"#,
        r#"{"id": "k0000004", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true}"#,
        r#"{"id": "k0000005", "type": "distance_x", "a": "origin", "b": "p0000003", "value": 0, "driving": true}"#,
        r#"{"id": "k0000006", "type": "distance_y", "a": "origin", "b": "p0000003", "value": 5, "driving": true}"#,
    ];
    let sketch = sketch_of(&points, &[], &[], &arcs, &constraints);
    let (_, diagnosis, got) = solved(&sketch);
    assert_eq!(got.state, SketchState::OverDefined);
    assert_eq!(got.dofs, 0);
    let flagged: Vec<(&ElementId, &ConstraintState)> = got
        .constraints
        .iter()
        .filter(|(_, s)| s.flagged())
        .collect();
    assert_eq!(flagged.len(), 1, "{got:#?}");
    let (repeated, state) = flagged[0];
    assert_eq!(*state, Redundant);
    // Each dimension reads the origin, fixed, and one point: that point is over defined.
    let placed = match repeated.as_str() {
        "k0000001" | "k0000002" => "p0000001",
        "k0000003" | "k0000004" => "p0000002",
        "k0000005" | "k0000006" => "p0000003",
        other => panic!("{other}"),
    };
    assert_eq!(
        diagnosis.points_depended_on[repeated]
            .iter()
            .map(ElementId::as_str)
            .collect::<Vec<_>>(),
        [placed]
    );
    for (id, state) in &got.points {
        let want = if id.as_str() == placed { Over } else { Fully };
        assert_eq!(*state, want, "{id}");
    }
    assert_eq!(got.arcs, members(&[("a0000001", Fully)]));
}

#[test]
fn a_point_with_only_reference_x_and_y_is_under_defined_and_its_references_satisfied() {
    // Reference dimensions are never sent: no driving rows at all, so every value is free
    // (1.1.3: DoF 2, everything dependent). Both references satisfied.
    let sketch = sketch_of(
        &[point("p0000001", 3.0, 4.0)],
        &[],
        &[],
        &[],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 3, "driving": false}"#,
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 4, "driving": false}"#,
        ],
    );
    let (_, _, got) = solved(&sketch);
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::UnderDefined,
            dofs: 2,
            points: &[("p0000001", Under)],
            lines: &[],
            circles: &[],
            arcs: &[],
            constraints: &[("k0000001", Satisfied), ("k0000002", Satisfied)],
        },
    );
}

#[test]
fn a_point_on_the_x_axis_is_under_defined_with_one_degree_left() {
    // y fixed by the axis, x free: 1 left (1.1.3: 1).
    let sketch = sketch_of(
        &[point("p0000001", 3.0, 0.0)],
        &[],
        &[],
        &[],
        &[r#"{"id": "k0000001", "type": "on", "point": "p0000001", "curve": "x_axis"}"#],
    );
    let (_, _, got) = solved(&sketch);
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::UnderDefined,
            dofs: 1,
            points: &[("p0000001", Under)],
            lines: &[],
            circles: &[],
            arcs: &[],
            constraints: &[("k0000001", Satisfied)],
        },
    );
}

#[test]
fn two_lengths_that_conflict_over_define_their_line_and_leave_a_free_line_under_defined() {
    // Lengths 10 and 12 on the horizontal line from the origin, a free line beside it
    // (1.1.3: conflicting [3, 4], DoF 4). The two lengths conflict; the horizontal and the
    // placing dimensions don't; each length reads both ends and names the line, so those
    // are over defined; the free line and its ends under defined. The sketch is over defined,
    // not no_solution (item 32: a conflict is over defined), its members as stored, their
    // states from the conflict; the constraints not in it unsolved (item 32's `unsolved`: no
    // solution to be met on). Degrees: 8 unknowns, rank 4 (the first line's), so 4.
    let constraints = [
        FROM_AT_ORIGIN[0],
        FROM_AT_ORIGIN[1],
        HORIZONTAL,
        LENGTH_10,
        r#"{"id": "k0000005", "type": "length", "line": "n0000001", "value": 12, "driving": true}"#,
    ];
    let mut points = line_points((10.0, 0.0));
    points.push(point("p0000003", 0.0, 5.0));
    points.push(point("p0000004", 10.0, 8.0));
    let lines = [
        line("n0000001", "p0000001", "p0000002"),
        line("n0000002", "p0000003", "p0000004"),
    ];
    let sketch = sketch_of(&points, &lines, &[], &[], &constraints);
    let (answer, _, got) = solved(&sketch);
    let error = answer.outcome().unwrap_err();
    assert_eq!(error.code(), ErrorCode::OverConstrained);
    expect(
        &got,
        Want {
            problem: Some(ErrorCode::OverConstrained),
            state: SketchState::OverDefined,
            dofs: 4,
            points: &[
                ("p0000001", Over),
                ("p0000002", Over),
                ("p0000003", Under),
                ("p0000004", Under),
            ],
            lines: &[("n0000001", Over), ("n0000002", Under)],
            circles: &[],
            arcs: &[],
            constraints: &[
                ("k0000001", Unsolved),
                ("k0000002", Unsolved),
                ("k0000003", Unsolved),
                ("k0000004", Conflicting),
                ("k0000005", Conflicting),
            ],
        },
    );
}

#[test]
fn a_redundant_length_that_disagrees_past_decision_5_conflicts_as_data() {
    // A horizontal line from the origin, its length 10, and its ends' x 10.000005 apart: the
    // same equation twice to first order, 5e-6 apart, within PlaneGCS's 1e-5, so PlaneGCS
    // calls the later one redundant; decision 5's tolerance here is 1e-8 mm, so the read
    // refuses it as over_constrained, naming it. The diagnosis carries it as conflicting;
    // the length it repeats isn't flagged (PlaneGCS doesn't say which it repeats), so it's
    // unsolved with the rest, the sketch refused. It reads
    // both ends, which are over defined, but names no line (a distance_x names points), so
    // the line isn't.
    let constraints = [
        FROM_AT_ORIGIN[0],
        FROM_AT_ORIGIN[1],
        HORIZONTAL,
        LENGTH_10,
        r#"{"id": "k0000005", "type": "distance_x", "a": "p0000001", "b": "p0000002", "value": 10.000005, "driving": true}"#,
    ];
    let sketch = sketch_of(
        &line_points((10.0, 0.0)),
        &one_line(),
        &[],
        &[],
        &constraints,
    );
    let (answer, diagnosis, got) = solved(&sketch);
    let error = answer.outcome().unwrap_err();
    assert_eq!(error.code(), ErrorCode::OverConstrained);
    assert!(
        answer.solved().conflicting.is_empty(),
        "{:?}",
        answer.solved()
    );
    assert_eq!(error.elements(), ["k0000005"]);
    assert_eq!(
        diagnosis
            .conflicting
            .iter()
            .map(ElementId::as_str)
            .collect::<Vec<_>>(),
        ["k0000005"]
    );
    expect(
        &got,
        Want {
            problem: Some(ErrorCode::OverConstrained),
            state: SketchState::OverDefined,
            dofs: 0,
            points: &[("p0000001", Over), ("p0000002", Over)],
            lines: &[("n0000001", Fully)],
            circles: &[],
            arcs: &[],
            constraints: &[
                ("k0000001", Unsolved),
                ("k0000002", Unsolved),
                ("k0000003", Unsolved),
                ("k0000004", Unsolved),
                ("k0000005", Conflicting),
            ],
        },
    );
}

#[test]
fn a_line_made_horizontal_and_vertical_is_invalid_its_members_from_the_diagnosis() {
    // Both rows hold only with the ends on one place: a line of no length, which can't be
    // built (1.1.3: "Both points are equal" from every solver), so `invalid_result` and the
    // sketch invalid. Diagnosed where it's drawn, (0, 0) to (10, 3): rows (0, −1, 0, 1) and
    // (−1, 0, 1, 0), independent, rank 2 of 4: both ends free to move together, so every
    // value is free, every member under defined, both constraints unsolved (refused), 2 left.
    let sketch = sketch_of(
        &line_points((10.0, 3.0)),
        &one_line(),
        &[],
        &[],
        &[
            HORIZONTAL,
            r#"{"id": "k0000004", "type": "vertical", "line": "n0000001"}"#,
        ],
    );
    let (answer, _, got) = solved(&sketch);
    let error = answer.outcome().unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidResult);
    assert!(
        error.elements().contains(&"n0000001".to_owned()),
        "{error:?}"
    );
    expect(
        &got,
        Want {
            problem: Some(ErrorCode::InvalidResult),
            state: SketchState::Invalid,
            dofs: 2,
            points: &[("p0000001", Under), ("p0000002", Under)],
            lines: &[("n0000001", Under)],
            circles: &[],
            arcs: &[],
            constraints: &[("k0000003", Unsolved), ("k0000004", Unsolved)],
        },
    );
}

#[test]
fn a_triangle_whose_sides_cant_close_has_no_solution_and_can_still_turn() {
    // A corner placed at the origin, sides 1, 1, and 10: no triangle has them, so no solver
    // places it (1.1.3: solve −1, DoF 1). Diagnosed as drawn: 6 unknowns, 5 independent
    // equations, the one left a turn about the placed corner, which moves both other
    // corners: the placed corner fully defined, the others and every side under defined;
    // every constraint unsolved, none flagged.
    let points = [
        point("p0000001", 0.0, 0.0),
        point("p0000002", 1.0, 0.0),
        point("p0000003", 0.5, 0.8),
    ];
    let lines = [
        line("n0000001", "p0000001", "p0000002"),
        line("n0000002", "p0000002", "p0000003"),
        line("n0000003", "p0000003", "p0000001"),
    ];
    let constraints = [
        FROM_AT_ORIGIN[0],
        FROM_AT_ORIGIN[1],
        r#"{"id": "k0000003", "type": "length", "line": "n0000001", "value": 1, "driving": true}"#,
        r#"{"id": "k0000004", "type": "length", "line": "n0000002", "value": 1, "driving": true}"#,
        r#"{"id": "k0000005", "type": "length", "line": "n0000003", "value": 10, "driving": true}"#,
    ];
    let sketch = sketch_of(&points, &lines, &[], &[], &constraints);
    let (answer, _, got) = solved(&sketch);
    let error = answer.outcome().unwrap_err();
    assert_eq!(error.code(), ErrorCode::SolveFailed);
    expect(
        &got,
        Want {
            problem: Some(ErrorCode::SolveFailed),
            state: SketchState::NoSolution,
            dofs: 1,
            points: &[
                ("p0000001", Fully),
                ("p0000002", Under),
                ("p0000003", Under),
            ],
            lines: &[
                ("n0000001", Under),
                ("n0000002", Under),
                ("n0000003", Under),
            ],
            circles: &[],
            arcs: &[],
            constraints: &[
                ("k0000001", Unsolved),
                ("k0000002", Unsolved),
                ("k0000003", Unsolved),
                ("k0000004", Unsolved),
                ("k0000005", Unsolved),
            ],
        },
    );
}

#[test]
fn an_arcs_ends_mirrored_about_the_y_axis_its_centre_placed_there_repeat_the_centres_x() {
    // The arc-symmetric case: an arc about the origin (its centre placed by dimensions, not
    // held on the axis by the sketch itself, so Silico sends the symmetric's both rows:
    // perpendicular, and midpoint on the axis), its ends (5, 5) and (−5, 5) mirrored about
    // the y axis. By hand: two points of a circle mirrored about a line put the centre on
    // the line (the chord's perpendicular bisector), so the centre's x dimension and the
    // symmetric's perpendicular row repeat each other: one of the two is flagged, and the
    // sketch is over defined. 9 unknowns, rank 7 (the arc's 4 rules, the 2 dimensions, the
    // symmetric's midpoint row): 2 left, the radius and the ends' turn.
    //
    // PlaneGCS flags the dimension, wholly redundant, in either tag order: one row against
    // the symmetric's two, its contest removing the constraint with fewer rows on a tie in
    // popularity, order-independent (item 32's addendum, A). The flagged `distance_x` is a
    // driving dimension, so under addendum 2 it still over-defines: the sketch over defined,
    // and the centre, which it reads, over defined;
    // the ends, free along the circle, under defined, as is the arc, which nothing flagged
    // names.
    for symmetric in ["k0000000", "k0000003"] {
        let points = [
            point("p0000001", 0.0, 0.0),
            point("p0000002", 5.0, 5.0),
            point("p0000003", -5.0, 5.0),
        ];
        let arcs = [arc("a0000001", "p0000001", "p0000002", "p0000003")];
        let mirrored = format!(
            r#"{{"id": "{symmetric}", "type": "symmetric", "a": "p0000002", "b": "p0000003", "about": "y_axis"}}"#
        );
        let constraints = [FROM_AT_ORIGIN[0], FROM_AT_ORIGIN[1], mirrored.as_str()];
        let sketch = sketch_of(&points, &[], &[], &arcs, &constraints);
        let (answer, _, got) = solved(&sketch);
        assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
        expect(
            &got,
            Want {
                problem: None,
                state: SketchState::OverDefined,
                dofs: 2,
                points: &[("p0000001", Over), ("p0000002", Under), ("p0000003", Under)],
                lines: &[],
                circles: &[],
                arcs: &[("a0000001", Under)],
                constraints: &[
                    ("k0000001", Redundant),
                    ("k0000002", Satisfied),
                    (symmetric, Satisfied),
                ],
            },
        );
    }
}

/// docs/design/m3.2-sketch-format.md's rectangle, as ARCHITECTURE.md's example holds it:
/// (0, 0), (40, 0), (40, 20), (0, 20), its bottom 6t7v8w9x and top j8k9m0n1 horizontal, its
/// right a0b1c2d3 and left p2q3r4s5 vertical, 40 by 20, its first corner at the origin; and
/// `extra` after its eight constraints.
fn rectangle(extra: &[&str]) -> Sketch {
    let points = [
        point("0a1b2c3d", 0.0, 0.0),
        point("2p3q4r5s", 0.0, 20.0),
        point("4e5f6g7h", 40.0, 0.0),
        point("8j9k0m1n", 40.0, 20.0),
    ];
    let lines = [
        line("6t7v8w9x", "0a1b2c3d", "4e5f6g7h"),
        line("a0b1c2d3", "4e5f6g7h", "8j9k0m1n"),
        line("j8k9m0n1", "8j9k0m1n", "2p3q4r5s"),
        line("p2q3r4s5", "2p3q4r5s", "0a1b2c3d"),
    ];
    let constraints = [
        r#"{"id": "c0d1e2f3", "type": "horizontal", "line": "6t7v8w9x"}"#,
        r#"{"id": "g4h5j6k7", "type": "length", "line": "6t7v8w9x", "value": 40, "driving": true}"#,
        r#"{"id": "m8n9p0q1", "type": "distance_x", "a": "origin", "b": "0a1b2c3d", "value": 0, "driving": true}"#,
        r#"{"id": "r2s3t4v5", "type": "vertical", "line": "a0b1c2d3"}"#,
        r#"{"id": "w6x7y8z9", "type": "length", "line": "a0b1c2d3", "value": 20, "driving": true}"#,
        r#"{"id": "x0y1z2a3", "type": "horizontal", "line": "j8k9m0n1"}"#,
        r#"{"id": "y4z5a6b7", "type": "distance_y", "a": "origin", "b": "0a1b2c3d", "value": 0, "driving": true}"#,
        r#"{"id": "z8a9b0c1", "type": "vertical", "line": "p2q3r4s5"}"#,
    ];
    let mut constraints = constraints.to_vec();
    constraints.extend(extra);
    sketch_of(&points, &lines, &[], &[], &constraints)
}

#[test]
fn a_fully_defined_rectangle_has_no_member_under_defined() {
    // The rectangle: 8 unknowns, 8 independent equations. Every member fully defined, every
    // constraint satisfied (FreeCAD's CreateRectangleSketch: DoF 0).
    let sketch = rectangle(&[]);
    let (_, _, got) = solved(&sketch);
    assert_eq!((got.state, got.dofs), (SketchState::FullyDefined, 0));
    let every = |m: &BTreeMap<ElementId, MemberState>| m.values().all(|&s| s == Fully);
    assert!(every(&got.points) && every(&got.lines), "{got:#?}");
    assert_eq!(got.points.len(), 4);
    assert_eq!(got.lines.len(), 4);
    assert!(got.constraints.values().all(|&s| s == Satisfied));
    assert_eq!(got.constraints.len(), 8);
}

/// The rectangle's eight constraints, each `state`, and `extra`'s.
fn rectangle_constraints<'a>(
    state: ConstraintState,
    extra: &[(&'a str, ConstraintState)],
) -> Vec<(&'a str, ConstraintState)> {
    let mut out: Vec<(&str, ConstraintState)> = [
        "c0d1e2f3", "g4h5j6k7", "m8n9p0q1", "r2s3t4v5", "w6x7y8z9", "x0y1z2a3", "y4z5a6b7",
        "z8a9b0c1",
    ]
    .into_iter()
    .map(|id| (id, state))
    .collect();
    out.extend_from_slice(extra);
    out
}

const RECTANGLE_POINTS: [&str; 4] = ["0a1b2c3d", "2p3q4r5s", "4e5f6g7h", "8j9k0m1n"];
const RECTANGLE_LINES: [&str; 4] = ["6t7v8w9x", "a0b1c2d3", "j8k9m0n1", "p2q3r4s5"];
/// The perpendicular of the rectangle's top and left, after its eight constraints by tag.
const FAQ_PERPENDICULAR: &str =
    r#"{"id": "zz000001", "type": "perpendicular", "a": "j8k9m0n1", "b": "p2q3r4s5"}"#;
/// An angle of 90° between the rectangle's bottom and right, the last tag.
const FAQ_ANGLE: &str = r#"{"id": "zz000002", "type": "angle", "a": "6t7v8w9x", "b": "a0b1c2d3", "value": 90, "driving": true}"#;

#[test]
fn solidworks_faq_a_redundant_relation_leaves_the_rectangle_fully_defined() {
    // A port of SolidWorks' FAQ on sketch relations ("Why can I not have redundant dimensions
    // or distance mates?"), its first two captions: "Fully-defined sketch." (the rectangle,
    // above) and "Redundant perpendicular relation added. Sketch is still fully defined."
    // By hand: the top is horizontal (x0y1z2a3) and the left vertical (z8a9b0c1), so their
    // perpendicular repeats what those two fix: its row's gradient, the dot product of the
    // top's direction (−40, 0) and the left's (0, −20), normalised, is a combination of the
    // horizontal's and the vertical's alone. 9 rows of rank 8 for 8 unknowns: 0 left, no
    // value past the rank, every point and line fully defined. Of the one dependent group
    // {horizontal, vertical, perpendicular}, each in one group and one row, PlaneGCS removes
    // the latest tag (1.1.3 GCS.cpp's contest, rule 3): the perpendicular, redundant. A
    // perpendicular is a relation, so it over-defines nothing (addendum 2): the sketch is
    // fully defined, as SolidWorks says.
    let sketch = rectangle(&[FAQ_PERPENDICULAR]);
    let (answer, _, got) = solved(&sketch);
    assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
    let points: Vec<_> = RECTANGLE_POINTS.iter().map(|&p| (p, Fully)).collect();
    let lines: Vec<_> = RECTANGLE_LINES.iter().map(|&l| (l, Fully)).collect();
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::FullyDefined,
            dofs: 0,
            points: &points,
            lines: &lines,
            circles: &[],
            arcs: &[],
            constraints: &rectangle_constraints(Satisfied, &[("zz000001", Redundant)]),
        },
    );
}

#[test]
fn solidworks_faq_a_redundant_angle_dimension_then_over_defines_the_rectangle() {
    // The FAQ's third caption: "Redundant dimension added. Sketch is over defined." An angle
    // of 90° between the bottom and the right, after the perpendicular: a dimension "which,
    // changing to something other than 90°, would conflict with the relations". By hand: the
    // angle's row, the turn from the bottom's direction to the right's, has a gradient that
    // combines the bottom's horizontal (c0d1e2f3) and the right's vertical (r2s3t4v5) alone,
    // so it repeats them: 10 rows of rank 8, still 0 left, two dependent groups,
    // {top's horizontal, left's vertical, perpendicular} and {bottom's horizontal, right's
    // vertical, angle}, sharing nothing. Each constraint is in one group with one row, so
    // PlaneGCS removes the latest tag first, the angle, which settles its group, then the
    // perpendicular, which settles the other: both `redundant`. The perpendicular, a
    // relation, over-defines nothing; the angle, a driving dimension, does (addendum 2): it
    // depends on both ends of both lines (the bottom's 0a1b2c3d and 4e5f6g7h, the right's
    // 4e5f6g7h and 8j9k0m1n), over defined, and names both lines, over defined; the fourth
    // corner 2p3q4r5s and the top and left, which it neither reads nor names, keep their
    // freedom, none: fully defined. The sketch is over defined.
    let sketch = rectangle(&[FAQ_PERPENDICULAR, FAQ_ANGLE]);
    let (answer, _, got) = solved(&sketch);
    assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::OverDefined,
            dofs: 0,
            points: &[
                ("0a1b2c3d", Over),
                ("2p3q4r5s", Fully),
                ("4e5f6g7h", Over),
                ("8j9k0m1n", Over),
            ],
            lines: &[
                ("6t7v8w9x", Over),
                ("a0b1c2d3", Over),
                ("j8k9m0n1", Fully),
                ("p2q3r4s5", Fully),
            ],
            circles: &[],
            arcs: &[],
            constraints: &rectangle_constraints(
                Satisfied,
                &[("zz000001", Redundant), ("zz000002", Redundant)],
            ),
        },
    );
}

#[test]
fn a_redundant_angle_at_the_corner_a_perpendicular_holds_reads_by_dofs_when_planegcs_names_the_vertical()
 {
    // The one divergence from SolidWorks addendum 2 leaves (docs/ports/sketcher.md,
    // *Definition states*): the perpendicular and the angle both at the bottom-right corner.
    // By hand: PlaneGCS's perpendicular and angle rows are normalised, their gradients'
    // entries about 1/40 and 1/20 against the horizontal's and vertical's 1, so the full
    // pivoting of its QR takes every unit row first and leaves those two past the rank, each
    // repeating the bottom's horizontal (c0d1e2f3) and the right's vertical (r2s3t4v5): the
    // groups {horizontal, vertical, perpendicular} and {horizontal, vertical, angle}. The
    // horizontal and the vertical are each in two, the most popular (1.1.3 GCS.cpp's
    // contest, rule 1), tied on rows, so the later tag of the two is removed: the vertical,
    // which settles both groups. It's a relation, so nothing over-defines: the sketch reads
    // by `dofs`, 0, fully defined, every member fully defined, the angle satisfied, where
    // SolidWorks would call the sketch over defined. An edit of the angle that breaks it is
    // a conflict that edit's solve reports.
    let perpendicular =
        r#"{"id": "zz000001", "type": "perpendicular", "a": "6t7v8w9x", "b": "a0b1c2d3"}"#;
    let sketch = rectangle(&[perpendicular, FAQ_ANGLE]);
    let (answer, _, got) = solved(&sketch);
    assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
    let points: Vec<_> = RECTANGLE_POINTS.iter().map(|&p| (p, Fully)).collect();
    let lines: Vec<_> = RECTANGLE_LINES.iter().map(|&l| (l, Fully)).collect();
    let mut constraints = rectangle_constraints(
        Satisfied,
        &[("zz000001", Satisfied), ("zz000002", Satisfied)],
    );
    constraints[3].1 = Redundant;
    assert_eq!(constraints[3].0, "r2s3t4v5");
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::FullyDefined,
            dofs: 0,
            points: &points,
            lines: &lines,
            circles: &[],
            arcs: &[],
            constraints: &constraints,
        },
    );
}

#[test]
fn a_redundant_distance_over_defines_the_line_it_repeats_the_length_of() {
    // The dimensioned line from the origin, horizontal, 10 long, and a driving distance of
    // 10 between its ends (k0000005). By hand: the distance's row, |p2 − p1| − 10, has the
    // length's gradient exactly, (−1, 0) at p1 and (1, 0) at p2, so the two are one group
    // (the horizontal's gradient, in the ends' y, and the placing x and y are independent of
    // them): 5 rows of rank 4, 0 left. Each one row, in one group: PlaneGCS removes the later
    // tag, the distance, `redundant`. A driving dimension over-defines (addendum 2): it
    // depends on both ends, over defined; it names only points, so the line, its ends fixed,
    // is fully defined (a curve takes its points' freedom, not their over-definition). The
    // sketch is over defined.
    let constraints = [
        FROM_AT_ORIGIN[0],
        FROM_AT_ORIGIN[1],
        HORIZONTAL,
        LENGTH_10,
        r#"{"id": "k0000005", "type": "distance", "a": "p0000001", "b": "p0000002", "value": 10, "driving": true}"#,
    ];
    let sketch = sketch_of(
        &line_points((10.0, 0.0)),
        &one_line(),
        &[],
        &[],
        &constraints,
    );
    let (answer, _, got) = solved(&sketch);
    assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::OverDefined,
            dofs: 0,
            points: &[("p0000001", Over), ("p0000002", Over)],
            lines: &[("n0000001", Fully)],
            circles: &[],
            arcs: &[],
            constraints: &[
                ("k0000001", Satisfied),
                ("k0000002", Satisfied),
                ("k0000003", Satisfied),
                ("k0000004", Satisfied),
                ("k0000005", Redundant),
            ],
        },
    );
}

/// A circle of radius 5 about p1, placed at the origin (k0000001, k0000002); p2 placed at
/// (5, 0) (k0000003, k0000004); p2 `on` the circle, and a driving radius of 5, by these IDs.
fn circle_through_a_placed_point(on: &str, radius: &str) -> Sketch {
    let on = format!(r#"{{"id": "{on}", "type": "on", "point": "p0000002", "curve": "c0000001"}}"#);
    let radius = format!(
        r#"{{"id": "{radius}", "type": "radius", "curve": "c0000001", "value": 5, "driving": true}}"#
    );
    let constraints = [
        FROM_AT_ORIGIN[0],
        FROM_AT_ORIGIN[1],
        r#"{"id": "k0000003", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 5, "driving": true}"#,
        r#"{"id": "k0000004", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true}"#,
        on.as_str(),
        radius.as_str(),
    ];
    sketch_of(
        &line_points((5.0, 0.0)),
        &[],
        &[circle("c0000001", "p0000001", 5.0)],
        &[],
        &constraints,
    )
}

#[test]
fn a_redundant_radius_over_defines_its_circle_and_a_redundant_on_in_its_place_doesnt() {
    // The centre and a point of the circle both placed, so the `on` fixes the radius and a
    // radius dimension repeats it. By hand: 5 unknowns (p1's x and y, p2's, the radius), 6
    // rows of rank 5, 0 left. The `on`'s gradient, p2 − p1 over its length minus the
    // radius's, is (1, 0) at p2, (−1, 0) at p1, −1 in the radius: p2's x row less p1's x row
    // less the radius row. So the one group is {k0000001, k0000003, on, radius}, each one row
    // in one group, and PlaneGCS removes the latest tag of the four.
    //
    // The radius latest (k0000006): it's `redundant`, a driving dimension, so it over-defines
    // (addendum 2): it names the circle, over defined, and reads no point, so both points
    // keep their freedom, none: fully defined. The sketch is over defined.
    let sketch = circle_through_a_placed_point("k0000005", "k0000006");
    let (answer, _, got) = solved(&sketch);
    assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
    let constraints = |on, radius| {
        vec![
            ("k0000001", Satisfied),
            ("k0000002", Satisfied),
            ("k0000003", Satisfied),
            ("k0000004", Satisfied),
            ("k0000005", on),
            ("k0000006", radius),
        ]
    };
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::OverDefined,
            dofs: 0,
            points: &[("p0000001", Fully), ("p0000002", Fully)],
            lines: &[],
            circles: &[("c0000001", Over)],
            arcs: &[],
            constraints: &constraints(Satisfied, Redundant),
        },
    );
    // The `on` latest (k0000006, the radius k0000005): the `on` is `redundant`, a relation,
    // so nothing over-defines and the sketch reads by `dofs`, 0: every member fully defined,
    // the radius satisfied (the mixed group's divergence from SolidWorks, which would call
    // the repeated radius over defining whichever is named).
    let sketch = circle_through_a_placed_point("k0000006", "k0000005");
    let (answer, _, got) = solved(&sketch);
    assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::FullyDefined,
            dofs: 0,
            points: &[("p0000001", Fully), ("p0000002", Fully)],
            lines: &[],
            circles: &[("c0000001", Fully)],
            arcs: &[],
            constraints: &constraints(Satisfied, Redundant)
                .into_iter()
                .map(|(id, s)| (id, if id == "k0000005" { Satisfied } else { s }))
                .collect::<Vec<_>>(),
        },
    );
}

#[test]
fn a_reference_dimension_beside_a_redundant_relation_is_satisfied_and_the_sketch_reads_by_dofs() {
    // The line from the origin, horizontal twice (k0000003, k0000005), its far end's x free,
    // and a reference length (k0000004, driving false) measuring it. By hand: 4 unknowns;
    // the rows the placing x and y, and the two horizontals, one repeating the other (the
    // same gradient, the ends' y), rank 3, 1 left. p2's x is in no row, a zero column, the
    // one value past the rank: p2 and the line under defined, p1 fully defined. The later
    // horizontal is `redundant`, a relation, over-defining nothing (addendum 2), so the
    // sketch reads by `dofs`: under defined. The reference is never sent, so never flagged:
    // `satisfied` on the solved sketch.
    let constraints = [
        FROM_AT_ORIGIN[0],
        FROM_AT_ORIGIN[1],
        HORIZONTAL,
        r#"{"id": "k0000004", "type": "length", "line": "n0000001", "value": 10, "driving": false}"#,
        r#"{"id": "k0000005", "type": "horizontal", "line": "n0000001"}"#,
    ];
    let sketch = sketch_of(
        &line_points((10.0, 0.0)),
        &one_line(),
        &[],
        &[],
        &constraints,
    );
    let (answer, _, got) = solved(&sketch);
    assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::UnderDefined,
            dofs: 1,
            points: &[("p0000001", Fully), ("p0000002", Under)],
            lines: &[("n0000001", Under)],
            circles: &[],
            arcs: &[],
            constraints: &[
                ("k0000001", Satisfied),
                ("k0000002", Satisfied),
                ("k0000003", Satisfied),
                ("k0000004", Satisfied),
                ("k0000005", Redundant),
            ],
        },
    );
}

#[test]
fn a_point_on_two_circles_touching_at_it_keeps_its_freedom_beside_the_redundant_on() {
    // Two fully dimensioned circles of radius 5, centres 10 apart, touching at (5, 0), and a
    // point on both (1.1.3: DoF 1, redundant [7], the second `on`). At the point the two
    // circles' normals line up, so to first order the two `on`s are one equation: one is
    // redundant (the first-order rule), the point keeps a degree it can't use, 8 unknowns,
    // rank 7. The redundant `on` reads the point and its circle's centre and radius
    // (`PointOnCircle`), and names the circle, but it's a relation, so under addendum 2 it
    // over-defines nothing: every member reads by its freedom, and the sketch by `dofs`.
    // Re-derived by hand: both centres and radii are fixed by their own dimensions (p1 by
    // k1, k2; p2 by k4, k5; the radii by k3, k6), all pivots. Each `on`'s gradient in the
    // point is the unit vector from its circle's centre, (1, 0) and (−1, 0) at (5, 0): both
    // zero in the point's y, a zero column, which is the one value past the rank (8 − 7 =
    // 1). So the point is under defined, both centres and both circles fully defined, and
    // the sketch under defined with its 1 degree left (FreeCAD 1.1.3's DoF 1).
    let points = [
        point("p0000001", 0.0, 0.0),
        point("p0000002", 10.0, 0.0),
        point("p0000003", 5.0, 0.0),
    ];
    let circles = [
        circle("c0000001", "p0000001", 5.0),
        circle("c0000002", "p0000002", 5.0),
    ];
    let constraints = [
        FROM_AT_ORIGIN[0],
        FROM_AT_ORIGIN[1],
        r#"{"id": "k0000003", "type": "radius", "curve": "c0000001", "value": 5, "driving": true}"#,
        r#"{"id": "k0000004", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 10, "driving": true}"#,
        r#"{"id": "k0000005", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true}"#,
        r#"{"id": "k0000006", "type": "radius", "curve": "c0000002", "value": 5, "driving": true}"#,
        r#"{"id": "k0000007", "type": "on", "point": "p0000003", "curve": "c0000001"}"#,
        r#"{"id": "k0000008", "type": "on", "point": "p0000003", "curve": "c0000002"}"#,
    ];
    let sketch = sketch_of(&points, &[], &circles, &[], &constraints);
    let (_, diagnosis, got) = solved(&sketch);
    let depended_on: Vec<&str> = diagnosis.points_depended_on[&ElementId::new("k0000008").unwrap()]
        .iter()
        .map(ElementId::as_str)
        .collect();
    assert_eq!(depended_on, ["p0000002", "p0000003"]);
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::UnderDefined,
            dofs: 1,
            points: &[
                ("p0000001", Fully),
                ("p0000002", Fully),
                ("p0000003", Under),
            ],
            lines: &[],
            circles: &[("c0000001", Fully), ("c0000002", Fully)],
            arcs: &[],
            constraints: &[
                ("k0000001", Satisfied),
                ("k0000002", Satisfied),
                ("k0000003", Satisfied),
                ("k0000004", Satisfied),
                ("k0000005", Satisfied),
                ("k0000006", Satisfied),
                ("k0000007", Satisfied),
                ("k0000008", Redundant),
            ],
        },
    );
}

#[test]
fn a_doubled_radius_over_defines_its_circle_and_never_its_centre() {
    // A radius reads only the radius: the second one, redundant, a driving dimension, so it
    // over-defines (addendum 2): it names the circle (over defined), the sketch is over
    // defined, and it reads no point, so the centre, placed by its own dimensions, stays fully
    // defined. 3 unknowns, rank 3.
    let constraints = [
        FROM_AT_ORIGIN[0],
        FROM_AT_ORIGIN[1],
        r#"{"id": "k0000003", "type": "radius", "curve": "c0000001", "value": 5, "driving": true}"#,
        r#"{"id": "k0000004", "type": "radius", "curve": "c0000001", "value": 5, "driving": true}"#,
    ];
    let sketch = sketch_of(
        &[point("p0000001", 0.0, 0.0)],
        &[],
        &[circle("c0000001", "p0000001", 5.0)],
        &[],
        &constraints,
    );
    let (_, diagnosis, got) = solved(&sketch);
    assert!(diagnosis.points_depended_on[&ElementId::new("k0000004").unwrap()].is_empty());
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::OverDefined,
            dofs: 0,
            points: &[("p0000001", Fully)],
            lines: &[],
            circles: &[("c0000001", Over)],
            arcs: &[],
            constraints: &[
                ("k0000001", Satisfied),
                ("k0000002", Satisfied),
                ("k0000003", Satisfied),
                ("k0000004", Redundant),
            ],
        },
    );
}

/// An arc about p0000001 at the origin, radius 5, from (3, 4) to (−3, 4): its ends mirror
/// images about the y axis.
fn mirrored_arc() -> (Vec<String>, Vec<String>) {
    (
        vec![
            point("p0000001", 0.0, 0.0),
            point("p0000002", 3.0, 4.0),
            point("p0000003", -3.0, 4.0),
        ],
        vec![arc("a0000001", "p0000001", "p0000002", "p0000003")],
    )
}

const ARC_RADIUS: &str =
    r#"{"id": "k0000008", "type": "radius", "curve": "a0000001", "value": 5, "driving": true}"#;
const ARC_START_Y: &str = r#"{"id": "k0000009", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 4, "driving": true}"#;

#[test]
fn an_arcs_ends_mirrored_about_a_line_its_centre_ends_are_one_row_and_fully_defined() {
    // The centre is an end of the line (0, 0)–(0, 10): Silico sends the midpoint row alone
    // (365af33fc6, decided from the references). 11 unknowns (4 points, the arc's radius
    // and angles), 11 equations (the arc's 4 rules; the centre's x and y; the line
    // vertical, 10 long; the midpoint row; the radius; the start's y): none left, none
    // repeated.
    let (mut points, arcs) = mirrored_arc();
    points.push(point("p0000004", 0.0, 10.0));
    let lines = [line("n0000001", "p0000001", "p0000004")];
    let constraints = [
        FROM_AT_ORIGIN[0],
        FROM_AT_ORIGIN[1],
        r#"{"id": "k0000003", "type": "vertical", "line": "n0000001"}"#,
        r#"{"id": "k0000004", "type": "length", "line": "n0000001", "value": 10, "driving": true}"#,
        r#"{"id": "k0000005", "type": "symmetric", "a": "p0000002", "b": "p0000003", "about": "n0000001"}"#,
        ARC_RADIUS,
        ARC_START_Y,
    ];
    let sketch = sketch_of(&points, &lines, &[], &arcs, &constraints);
    let (_, _, got) = solved(&sketch);
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::FullyDefined,
            dofs: 0,
            points: &[
                ("p0000001", Fully),
                ("p0000002", Fully),
                ("p0000003", Fully),
                ("p0000004", Fully),
            ],
            lines: &[("n0000001", Fully)],
            circles: &[],
            arcs: &[("a0000001", Fully)],
            constraints: &[
                ("k0000001", Satisfied),
                ("k0000002", Satisfied),
                ("k0000003", Satisfied),
                ("k0000004", Satisfied),
                ("k0000005", Satisfied),
                ("k0000008", Satisfied),
                ("k0000009", Satisfied),
            ],
        },
    );
}

/// The line (0, −5)–(0, 10), placed, vertical, 15 long, and `holder` (k0000005) putting the
/// arc's centre on it, then the symmetric of the arc's ends about it (k0000006), its radius,
/// and its start's y.
fn arc_held_on_a_line(holder: &str) -> Sketch {
    let (mut points, arcs) = mirrored_arc();
    points.push(point("p0000004", 0.0, -5.0));
    points.push(point("p0000005", 0.0, 10.0));
    let lines = [line("n0000001", "p0000004", "p0000005")];
    let constraints = [
        r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000004", "value": 0, "driving": true}"#,
        r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000004", "value": -5, "driving": true}"#,
        r#"{"id": "k0000003", "type": "vertical", "line": "n0000001"}"#,
        r#"{"id": "k0000004", "type": "length", "line": "n0000001", "value": 15, "driving": true}"#,
        holder,
        r#"{"id": "k0000006", "type": "symmetric", "a": "p0000002", "b": "p0000003", "about": "n0000001"}"#,
        r#"{"id": "k0000007", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
        ARC_RADIUS,
        ARC_START_Y,
    ];
    sketch_of(&points, &lines, &[], &arcs, &constraints)
}

#[test]
fn an_arcs_ends_mirrored_about_a_line_its_centre_is_on_are_one_row_and_fully_defined() {
    // The centre held on the line by an `on`: one row again. 13 unknowns, 13 equations.
    let sketch = arc_held_on_a_line(
        r#"{"id": "k0000005", "type": "on", "point": "p0000001", "curve": "n0000001"}"#,
    );
    let (_, _, got) = solved(&sketch);
    assert_eq!(
        (got.state, got.dofs),
        (SketchState::FullyDefined, 0),
        "{got:#?}"
    );
    assert!(got.points.values().all(|&s| s == Fully), "{got:#?}");
    assert_eq!(got.arcs, members(&[("a0000001", Fully)]));
    assert!(
        got.constraints.values().all(|&s| s == Satisfied),
        "{got:#?}"
    );
}

#[test]
fn an_arcs_ends_mirrored_about_a_line_a_one_row_constraint_puts_its_centre_on_lose_the_holder() {
    // The centre put on the line by a `vertical_points` with the line's end: not one of the
    // two holds the translation recognises, so the symmetric's both rows are sent, 14
    // equations for 13 unknowns, one repeating. Of the dependent group PlaneGCS removes the
    // constraint with fewer rows on a tie in popularity (1.1.3 GCS.cpp's contest): the
    // one-row holder, redundant. It's a relation, so under addendum 2 it over-defines
    // nothing; re-derived: 13 unknowns, rank 13 (14 rows, one repeating), 0 left, so no value
    // is past the rank and every point, the line, and the arc are fully defined, and so is
    // the sketch, the holder still listed redundant.
    let sketch = arc_held_on_a_line(
        r#"{"id": "k0000005", "type": "vertical_points", "a": "p0000004", "b": "p0000001"}"#,
    );
    let (answer, _, got) = solved(&sketch);
    assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::FullyDefined,
            dofs: 0,
            points: &[
                ("p0000001", Fully),
                ("p0000002", Fully),
                ("p0000003", Fully),
                ("p0000004", Fully),
                ("p0000005", Fully),
            ],
            lines: &[("n0000001", Fully)],
            circles: &[],
            arcs: &[("a0000001", Fully)],
            constraints: &[
                ("k0000001", Satisfied),
                ("k0000002", Satisfied),
                ("k0000003", Satisfied),
                ("k0000004", Satisfied),
                ("k0000005", Redundant),
                ("k0000006", Satisfied),
                ("k0000007", Satisfied),
                ("k0000008", Satisfied),
                ("k0000009", Satisfied),
            ],
        },
    );
}

#[test]
fn an_arcs_ends_mirrored_about_a_line_a_midpoint_puts_its_centre_on_flag_the_symmetry() {
    // The centre held at the line's middle by a `midpoint` (two rows, like the symmetric):
    // the symmetric's both rows are sent, 14 equations for 13 unknowns, and the two tie on
    // rows, so PlaneGCS removes the later by tag: the symmetric, every row of it dependent,
    // so wholly redundant (item 32's addendum, A: `redundant` when both its rows are in the
    // dependent set, `partially_redundant` when one is). A symmetric is a relation, so under
    // addendum 2 it over-defines nothing; re-derived: 13 unknowns, rank 13, 0 left, every
    // value a pivot, so every point, the line, the arc, and the sketch fully defined, the
    // symmetry still listed redundant.
    let (mut points, arcs) = mirrored_arc();
    points.push(point("p0000004", 0.0, -5.0));
    points.push(point("p0000005", 0.0, 5.0));
    let lines = [line("n0000001", "p0000004", "p0000005")];
    let constraints = [
        r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000004", "value": 0, "driving": true}"#,
        r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000004", "value": -5, "driving": true}"#,
        r#"{"id": "k0000003", "type": "vertical", "line": "n0000001"}"#,
        r#"{"id": "k0000004", "type": "length", "line": "n0000001", "value": 10, "driving": true}"#,
        r#"{"id": "k0000005", "type": "midpoint", "point": "p0000001", "line": "n0000001"}"#,
        r#"{"id": "k0000006", "type": "symmetric", "a": "p0000002", "b": "p0000003", "about": "n0000001"}"#,
        ARC_RADIUS,
        ARC_START_Y,
    ];
    let sketch = sketch_of(&points, &lines, &[], &arcs, &constraints);
    let (answer, _, got) = solved(&sketch);
    assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::FullyDefined,
            dofs: 0,
            points: &[
                ("p0000001", Fully),
                ("p0000002", Fully),
                ("p0000003", Fully),
                ("p0000004", Fully),
                ("p0000005", Fully),
            ],
            lines: &[("n0000001", Fully)],
            circles: &[],
            arcs: &[("a0000001", Fully)],
            constraints: &[
                ("k0000001", Satisfied),
                ("k0000002", Satisfied),
                ("k0000003", Satisfied),
                ("k0000004", Satisfied),
                ("k0000005", Satisfied),
                ("k0000006", Redundant),
                ("k0000008", Satisfied),
                ("k0000009", Satisfied),
            ],
        },
    );
}

#[test]
fn a_half_circles_ends_left_unmirrored_are_invalid_naming_the_symmetry() {
    // A half circle about the origin, its centre `on` the y axis, from 30° to 210°, its ends
    // symmetric about the y axis: one row, the chord's midpoint on the axis, which on a half
    // circle is the centre again, so the solve can leave the ends unmirrored; the core checks
    // the symmetry whole and refuses it, an invalid result naming it (adversarial_document's
    // break-it case). The sketch invalid, its problem naming the symmetric.
    let r = 10.0;
    let (a0, a1) = (30f64.to_radians(), 210f64.to_radians());
    let points = [
        point("p0000001", 0.0, 0.0),
        point("p0000002", r * libm::cos(a0), r * libm::sin(a0)),
        point("p0000003", r * libm::cos(a1), r * libm::sin(a1)),
    ];
    let arcs = [arc("a0000001", "p0000001", "p0000002", "p0000003")];
    let constraints = [
        r#"{"id": "k0000001", "type": "on", "point": "p0000001", "curve": "y_axis"}"#,
        r#"{"id": "k0000002", "type": "symmetric", "a": "p0000002", "b": "p0000003", "about": "y_axis"}"#,
    ];
    let sketch = sketch_of(&points, &[], &[], &arcs, &constraints);
    let (_, _, got) = solved(&sketch);
    assert_eq!(got.state, SketchState::Invalid, "{got:#?}");
    let problem = got.solve_problem.as_ref().unwrap();
    assert_eq!(problem.code(), ErrorCode::InvalidResult);
    assert_eq!(problem.elements(), ["k0000002"]);
    // Diagnosed as drawn: 9 unknowns, the arc's 4 rules, the `on`, and the midpoint row,
    // independent there: 3 left, every member free, nothing flagged, both constraints
    // unsolved.
    expect(
        &got,
        Want {
            problem: Some(ErrorCode::InvalidResult),
            state: SketchState::Invalid,
            dofs: 3,
            points: &[
                ("p0000001", Under),
                ("p0000002", Under),
                ("p0000003", Under),
            ],
            lines: &[],
            circles: &[],
            arcs: &[("a0000001", Under)],
            constraints: &[("k0000001", Unsolved), ("k0000002", Unsolved)],
        },
    );
}

// --- Item 32's invariants, over generated sketches -----------------------------------------

/// A sketch of a few points on a small grid (so points line up, mirror, and coincide often),
/// lines between them, circles about them, arcs of radius 5 through Pythagorean points
/// (their ends exact), and constraints of every kind the menu has, each at the drawn value
/// (agreeing), a little off it (decision 5's disagreement), or well off (a conflict), driving
/// or a reference. None if the gate refuses it.
fn generated(seed: u64) -> Option<Sketch> {
    let mut rng = Rng(seed);
    let mut points: Vec<(String, [f64; 2])> = Vec::new();
    let n = 2 + rng.below(4) as usize;
    for i in 0..n {
        let xy = [rng.below(9) as f64 - 4.0, rng.below(9) as f64 - 4.0];
        points.push((format!("p00000{i:02}"), xy));
    }
    const OFFSETS: [[f64; 2]; 12] = [
        [5.0, 0.0],
        [4.0, 3.0],
        [3.0, 4.0],
        [0.0, 5.0],
        [-3.0, 4.0],
        [-4.0, 3.0],
        [-5.0, 0.0],
        [-4.0, -3.0],
        [-3.0, -4.0],
        [0.0, -5.0],
        [3.0, -4.0],
        [4.0, -3.0],
    ];
    let mut arcs = Vec::new();
    for i in 0..rng.below(2) as usize {
        let c = rng.below(n as u64) as usize;
        let (s, e) = (rng.below(12) as usize, rng.below(12) as usize);
        if s == e {
            continue;
        }
        let at = |o: [f64; 2]| [points[c].1[0] + o[0], points[c].1[1] + o[1]];
        let (start, end) = (at(OFFSETS[s]), at(OFFSETS[e]));
        let (sid, eid) = (format!("p00001{i}0"), format!("p00001{i}1"));
        points.push((sid.clone(), start));
        points.push((eid.clone(), end));
        arcs.push((format!("a00000{i:02}"), points[c].0.clone(), sid, eid));
    }
    let mut lines = Vec::new();
    for i in 0..rng.below(4) as usize {
        let (a, b) = (
            rng.below(points.len() as u64) as usize,
            rng.below(points.len() as u64) as usize,
        );
        if a != b {
            lines.push((format!("n00000{i:02}"), a, b));
        }
    }
    let mut circles = Vec::new();
    for i in 0..rng.below(2) as usize {
        let c = rng.below(n as u64) as usize;
        circles.push((format!("c00000{i:02}"), c, 1.0 + rng.below(4) as f64));
    }
    let length = |a: [f64; 2], b: [f64; 2]| {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        (dx * dx + dy * dy).sqrt()
    };
    let mut constraints = Vec::new();
    for i in 0..rng.below(9) as usize {
        let id = format!("k00000{i:02}");
        // The drawn value, a little off it, or well off it.
        // Rounded to the file's nanometre, so an irrational length reads.
        let value = |v: f64, rng: &mut Rng| {
            let v = match rng.below(6) {
                0 => v + 0.000005,
                1 => v + 1.0,
                _ => v,
            };
            (v * 1e6).round() / 1e6
        };
        let driving = rng.below(5) != 0;
        let p = |rng: &mut Rng| rng.below(points.len() as u64) as usize;
        let pair = |rng: &mut Rng, n: usize| {
            let a = rng.below(n as u64) as usize;
            (a, (a + 1 + rng.below(n as u64 - 1) as usize) % n)
        };
        let text = match rng.below(16) {
            0 | 1 if !lines.is_empty() => {
                let l = &lines[rng.below(lines.len() as u64) as usize];
                let kind = if rng.below(2) == 0 {
                    "horizontal"
                } else {
                    "vertical"
                };
                format!(r#"{{"id": "{id}", "type": "{kind}", "line": "{}"}}"#, l.0)
            }
            2 | 3 => {
                let (a, b) = pair(&mut rng, points.len());
                let from_origin = rng.below(2) == 0;
                let (an, av) = if from_origin {
                    ("origin".to_owned(), [0.0, 0.0])
                } else {
                    (points[a].0.clone(), points[a].1)
                };
                let axis = rng.below(2) as usize;
                let kind = if axis == 0 {
                    "distance_x"
                } else {
                    "distance_y"
                };
                let v = value(points[b].1[axis] - av[axis], &mut rng);
                format!(
                    r#"{{"id": "{id}", "type": "{kind}", "a": "{an}", "b": "{}", "value": {v}, "driving": {driving}}}"#,
                    points[b].0
                )
            }
            4 if !lines.is_empty() => {
                let l = &lines[rng.below(lines.len() as u64) as usize];
                let v = value(length(points[l.1].1, points[l.2].1), &mut rng);
                format!(
                    r#"{{"id": "{id}", "type": "length", "line": "{}", "value": {v}, "driving": {driving}}}"#,
                    l.0
                )
            }
            5 if !circles.is_empty() || !arcs.is_empty() => {
                let (name, r) = if !circles.is_empty() && (arcs.is_empty() || rng.below(2) == 0) {
                    let c = &circles[rng.below(circles.len() as u64) as usize];
                    (c.0.clone(), c.2)
                } else {
                    (arcs[rng.below(arcs.len() as u64) as usize].0.clone(), 5.0)
                };
                let v = value(r, &mut rng);
                format!(
                    r#"{{"id": "{id}", "type": "radius", "curve": "{name}", "value": {v}, "driving": {driving}}}"#
                )
            }
            6 => {
                let q = &points[p(&mut rng)].0;
                let curve = match rng.below(4) {
                    0 => "x_axis".to_owned(),
                    1 => "y_axis".to_owned(),
                    2 if !lines.is_empty() => {
                        lines[rng.below(lines.len() as u64) as usize].0.clone()
                    }
                    _ if !circles.is_empty() => {
                        circles[rng.below(circles.len() as u64) as usize].0.clone()
                    }
                    _ => "x_axis".to_owned(),
                };
                format!(r#"{{"id": "{id}", "type": "on", "point": "{q}", "curve": "{curve}"}}"#)
            }
            7 | 8 if lines.len() >= 2 => {
                let (a, b) = pair(&mut rng, lines.len());
                let kind = ["parallel", "perpendicular", "equal"][rng.below(3) as usize];
                format!(
                    r#"{{"id": "{id}", "type": "{kind}", "a": "{}", "b": "{}"}}"#,
                    lines[a].0, lines[b].0
                )
            }
            9 => {
                let (a, b) = pair(&mut rng, points.len());
                let kind = if rng.below(2) == 0 {
                    "horizontal_points"
                } else {
                    "vertical_points"
                };
                format!(
                    r#"{{"id": "{id}", "type": "{kind}", "a": "{}", "b": "{}"}}"#,
                    points[a].0, points[b].0
                )
            }
            10 => {
                let (a, b) = pair(&mut rng, points.len());
                let about = match rng.below(3) {
                    0 => "y_axis".to_owned(),
                    1 if !lines.is_empty() => {
                        lines[rng.below(lines.len() as u64) as usize].0.clone()
                    }
                    _ => "x_axis".to_owned(),
                };
                format!(
                    r#"{{"id": "{id}", "type": "symmetric", "a": "{}", "b": "{}", "about": "{about}"}}"#,
                    points[a].0, points[b].0
                )
            }
            11 if !arcs.is_empty() => {
                // An arc's own ends about an axis or a line: the symmetric-arc case.
                let a = &arcs[rng.below(arcs.len() as u64) as usize];
                let about = if rng.below(2) == 0 || lines.is_empty() {
                    "y_axis".to_owned()
                } else {
                    lines[rng.below(lines.len() as u64) as usize].0.clone()
                };
                format!(
                    r#"{{"id": "{id}", "type": "symmetric", "a": "{}", "b": "{}", "about": "{about}"}}"#,
                    a.2, a.3
                )
            }
            13 if !lines.is_empty() || !arcs.is_empty() => {
                // A tangency at an end of a line or an arc, on any other curve.
                let ends: Vec<(String, String, String)> = lines
                    .iter()
                    .map(|l| (l.0.clone(), points[l.1].0.clone(), points[l.2].0.clone()))
                    .chain(arcs.iter().map(|a| (a.0.clone(), a.2.clone(), a.3.clone())))
                    .collect();
                let (a, start, end) = &ends[rng.below(ends.len() as u64) as usize];
                let at = if rng.below(2) == 0 { start } else { end };
                let others: Vec<String> = lines
                    .iter()
                    .map(|l| l.0.clone())
                    .chain(circles.iter().map(|c| c.0.clone()))
                    .chain(arcs.iter().map(|a| a.0.clone()))
                    .filter(|o| o != a)
                    .collect();
                let b = if others.is_empty() {
                    "x_axis".to_owned()
                } else {
                    others[rng.below(others.len() as u64) as usize].clone()
                };
                let direction = if rng.below(2) == 0 {
                    "same"
                } else {
                    "opposite"
                };
                format!(
                    r#"{{"id": "{id}", "type": "tangent_on", "a": "{a}", "at": "{at}", "b": "{b}", "direction": "{direction}"}}"#
                )
            }
            14 => {
                // A tangency where two curves, lines or arcs, share an end.
                let ends: Vec<(String, [String; 2])> = lines
                    .iter()
                    .map(|l| (l.0.clone(), [points[l.1].0.clone(), points[l.2].0.clone()]))
                    .chain(
                        arcs.iter()
                            .map(|a| (a.0.clone(), [a.2.clone(), a.3.clone()])),
                    )
                    .collect();
                let mut shared = Vec::new();
                for (i, (x, xe)) in ends.iter().enumerate() {
                    for (y, ye) in &ends[i + 1..] {
                        for at in xe.iter().filter(|p| ye.contains(p)) {
                            shared.push((x.clone(), y.clone(), at.clone()));
                        }
                    }
                }
                if shared.is_empty() {
                    continue;
                }
                let (a, b, at) = &shared[rng.below(shared.len() as u64) as usize];
                let direction = if rng.below(2) == 0 {
                    "same"
                } else {
                    "opposite"
                };
                format!(
                    r#"{{"id": "{id}", "type": "tangent_at", "a": "{a}", "b": "{b}", "at": "{at}", "direction": "{direction}"}}"#
                )
            }
            15 if circles.len() + arcs.len() >= 2 => {
                // Two round curves' radii equal.
                let round: Vec<String> = circles
                    .iter()
                    .map(|c| c.0.clone())
                    .chain(arcs.iter().map(|a| a.0.clone()))
                    .collect();
                let (a, b) = pair(&mut rng, round.len());
                format!(
                    r#"{{"id": "{id}", "type": "equal", "a": "{}", "b": "{}"}}"#,
                    round[a], round[b]
                )
            }
            _ => {
                let (a, b) = pair(&mut rng, points.len());
                let v = value(length(points[a].1, points[b].1), &mut rng);
                format!(
                    r#"{{"id": "{id}", "type": "distance", "a": "{}", "b": "{}", "value": {v}, "driving": {driving}}}"#,
                    points[a].0, points[b].0
                )
            }
        };
        constraints.push(text);
    }
    let point_texts: Vec<String> = points
        .iter()
        .map(|(id, [x, y])| point(id, *x, *y))
        .collect();
    let line_texts: Vec<String> = lines
        .iter()
        .map(|(id, a, b)| line(id, &points[*a].0, &points[*b].0))
        .collect();
    let circle_texts: Vec<String> = circles
        .iter()
        .map(|(id, c, r)| circle(id, &points[*c].0, *r))
        .collect();
    let arc_texts: Vec<String> = arcs.iter().map(|(id, c, a, b)| arc(id, c, a, b)).collect();
    let text = document(&fields(
        &point_texts.join(", "),
        &line_texts.join(", "),
        &circle_texts.join(", "),
        &arc_texts.join(", "),
        &constraints.join(", "),
    ));
    let doc = read(text.as_bytes()).ok()?;
    validate(&doc).ok()?;
    match &doc.features[0].operation {
        Operation::Sketch(s) => Some(s.clone()),
        _ => None,
    }
}

/// Addendum 2's dimensions, by the file's word: the forms with a value (M5.0 open item 32,
/// addendum 2, §3). Every other form is a relation.
const DIMENSIONS: [&str; 8] = [
    "length",
    "distance",
    "distance_x",
    "distance_y",
    "point_line_distance",
    "angle",
    "radius",
    "diameter",
];

/// Whether `c`, in `state`, over-defines, as addendum 2's §4 states it: conflicting, or a
/// dimension (by its word, [`DIMENSIONS`]) redundant or partially redundant.
fn over_defining_by_kind(
    c: &silico_core::document::SketchConstraint,
    state: ConstraintState,
) -> bool {
    use ConstraintState::PartiallyRedundant;
    state == Conflicting
        || (matches!(state, Redundant | PartiallyRedundant)
            && DIMENSIONS.contains(&c.form.type_name()))
}

/// Item 32's invariants on one sketch's states, solved by `worker`, as addendum 2's §4
/// revises them: fully_defined ⇒ every member fully defined, no constraint conflicting, and
/// no dimension redundant or partially redundant (a redundant relation may be there);
/// under_defined ⇒ dofs > 0, some member under defined, and nothing over-defining (a
/// redundant relation may be there); over_defined ⇒ some constraint conflicting or some
/// dimension redundant or partially redundant, and some member over defined; a member over
/// defined exactly when some constraint over-defines; an `over_constrained` problem's
/// elements are the conflicting constraints; a solve's problem exactly when the sketch is
/// refused, every constraint not flagged then unsolved; every satisfied driving constraint
/// met on the solution, its residual within decision 5's least window over the whole sketch
/// (no smaller than its part's), and every redundant or partially redundant one kept within
/// the most any part's window can be (its gap, the read's own, at most that window or
/// [`TANGENCY_TOLERANCE`] over [`LEAST_TANGENCY`]); and, solved, the diagnosis saying what
/// the solution says of the degrees of freedom, the free members, and the repetitions kept.
/// A sketch the translation refuses reads as [`untranslated_invariants`] says.
fn invariants_hold(sketch: &Sketch, worker: &mut SketchWorker) -> Result<(), String> {
    let Ok(translation) = Translation::new(sketch) else {
        return untranslated_invariants(sketch, worker);
    };
    let answer = match translation.solve(worker) {
        Ok(answer) => answer,
        // A solver that couldn't answer isn't a state: nothing to check.
        Err(_) => return Ok(()),
    };
    let read = answer.diagnosed().map_err(|e| format!("diagnosis: {e}"))?;
    let diagnosis = read.diagnosis();
    let got = states(sketch, read).map_err(|e| format!("states: {e}"))?;
    if let Ok(solution) = answer.outcome() {
        let kept = |m: &BTreeMap<ElementId, f64>| m.keys().cloned().collect::<BTreeSet<_>>();
        let partly: BTreeSet<ElementId> = diagnosis
            .partially_redundant
            .difference(&diagnosis.redundant)
            .cloned()
            .collect();
        if solution.dofs != diagnosis.dofs
            || solution.free != diagnosis.free
            || kept(&solution.redundant) != diagnosis.redundant
            || kept(&solution.partially_redundant) != partly
            || !diagnosis.conflicting.is_empty()
        {
            return Err(format!(
                "the diagnosis and the solution disagree: {diagnosis:#?} {solution:#?}"
            ));
        }
    }
    let members = || {
        got.points
            .values()
            .chain(got.lines.values())
            .chain(got.circles.values())
            .chain(got.arcs.values())
    };
    let over_defining = sketch
        .constraints
        .iter()
        .any(|c| over_defining_by_kind(c, got.constraints[&c.id]));
    let fail = |what: &str| Err(format!("{what}: {got:#?}"));
    match got.state {
        SketchState::FullyDefined => {
            if members().any(|&s| s != Fully) || over_defining {
                return fail(
                    "fully defined, with a member not, or a conflict or repeated dimension",
                );
            }
        }
        SketchState::UnderDefined => {
            if got.dofs <= 0 || !members().any(|&s| s == Under) || over_defining {
                return fail("under defined, with no freedom, or something over-defining");
            }
        }
        SketchState::OverDefined => {
            if !over_defining || !members().any(|&s| s == Over) {
                return fail("over defined, with nothing over-defining or no member over defined");
            }
        }
        SketchState::NoSolution | SketchState::Invalid => {}
    }
    if members().any(|&s| s == Over) != over_defining {
        return fail("a member over defined, but not exactly when something over-defines");
    }
    if got.solve_problem.is_some() != answer.outcome().is_err() {
        return fail("a problem exactly when refused");
    }
    if let Some(problem) = &got.solve_problem {
        if problem.code() == ErrorCode::OverConstrained {
            let named: BTreeSet<&str> = problem.elements().iter().map(String::as_str).collect();
            let conflicting: BTreeSet<&str> = got
                .constraints
                .iter()
                .filter(|(_, s)| **s == Conflicting)
                .map(|(id, _)| id.as_str())
                .collect();
            if named != conflicting {
                return fail("over_constrained's elements aren't the conflicting set");
            }
        }
        if got.constraints.values().any(|&s| s == Satisfied) {
            return fail("satisfied on a refused sketch");
        }
    } else if got.constraints.values().any(|&s| s == Unsolved) {
        return fail("unsolved on a solved sketch");
    }
    if answer.outcome().is_ok() {
        // Every satisfied driving constraint met where the solve put it.
        let values = &answer.solved().values;
        let xy: Vec<[f64; 2]> = translation
            .system
            .geometry
            .iter()
            .filter_map(|g| match g {
                silico_protocol::SketchGeometry::Point(p) => {
                    Some([values[p.x.0 as usize], values[p.y.0 as usize]])
                }
                _ => None,
            })
            .collect();
        let (lo, hi) = xy
            .iter()
            .fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), p| {
                (
                    [lo[0].min(p[0]), lo[1].min(p[1])],
                    [hi[0].max(p[0]), hi[1].max(p[1])],
                )
            });
        let extent = ((hi[0] - lo[0]) * (hi[0] - lo[0]) + (hi[1] - lo[1]) * (hi[1] - lo[1])).sqrt();
        let window = REDUNDANCY_GAP_MM.max(EXTENT_TOLERANCE * extent);
        for (i, c) in sketch_sorted(sketch).iter().enumerate() {
            if got.constraints[&c.id] != Satisfied || c.form.driving() == Some(false) {
                continue;
            }
            let tag = silico_protocol::Tag(i as u32 + 1);
            let residual = answer.solved().residuals.iter().find(|(t, _)| *t == tag);
            match residual {
                Some((_, Some(r))) if r.abs() <= window => {}
                other => return Err(format!("{} satisfied but {other:?} off: {got:#?}", c.id)),
            }
        }
        // Every redundant or partially redundant constraint kept, its gap (decision 5's, in
        // millimetres) within the most a part's window can be: a part's extent is at most the
        // whole's, and its tangency term at most TANGENCY_TOLERANCE / LEAST_TANGENCY.
        let solution = answer.outcome().map_err(|e| e.to_string())?;
        let most = window.max(TANGENCY_TOLERANCE / LEAST_TANGENCY);
        for (id, state) in &got.constraints {
            let gap = match state {
                Redundant => solution.redundant.get(id),
                ConstraintState::PartiallyRedundant => solution.partially_redundant.get(id),
                _ => continue,
            };
            match gap {
                Some(&g) if g.is_finite() && (0.0..=most).contains(&g) => {}
                other => return Err(format!("{id} {state:?} but kept {other:?}: {got:#?}")),
            }
        }
    }
    Ok(())
}

/// A sketch [`Translation::new`] refuses (two points drawn at one place where a constraint is
/// measured by the line between them, an arc that sweeps nothing), its geometry alone
/// diagnosed by `worker`: `invalid`, the refusal as its problem, an `invalid_input`; every
/// member under defined and every constraint unsolved; and `dofs` the sketch's unknowns less
/// its arcs' rules, counted by hand from its lists, 2 a point, 1 a circle, 3 an arc less its
/// 4 rules, which PlaneGCS always sends (M5.0 open item 32, the sketch the translation
/// refuses): none repeats another, since the generator gives each arc ends of its own, so no
/// two arcs share a distance.
fn untranslated_invariants(sketch: &Sketch, worker: &mut SketchWorker) -> Result<(), String> {
    let read = Diagnosed::untranslated(sketch, worker)
        .map_err(|e| format!("diagnosis: {e}"))?
        .ok_or("translated after all")?;
    let refusal = read.outcome().err().ok_or("refused with a solution")?;
    if refusal.code() != ErrorCode::InvalidInput {
        return Err(format!("the translation refuses with {refusal:?}"));
    }
    let got = states(sketch, &read).map_err(|e| format!("states: {e}"))?;
    let dofs =
        2 * sketch.points.len() as i64 + sketch.circles.len() as i64 + 3 * sketch.arcs.len() as i64
            - 4 * sketch.arcs.len() as i64;
    let all_under = got
        .points
        .values()
        .chain(got.lines.values())
        .chain(got.circles.values())
        .chain(got.arcs.values())
        .all(|&s| s == Under);
    if got.state != SketchState::Invalid
        || got.solve_problem.as_ref() != Some(refusal)
        || i64::from(got.dofs) != dofs
        || !all_under
        || got.constraints.values().any(|&s| s != Unsolved)
        || got.constraints.len() != sketch.constraints.len()
    {
        return Err(format!("untranslated: {got:#?}"));
    }
    Ok(())
}

/// The sketch's constraints in ID order: a constraint's tag is its place there, plus one.
fn sketch_sorted(sketch: &Sketch) -> Vec<silico_core::document::SketchConstraint> {
    let mut constraints = sketch.constraints.clone();
    constraints.sort_by(|a, b| a.id.cmp(&b.id));
    constraints
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, .. ProptestConfig::default() })]

    #[test]
    fn the_decisions_invariants_hold_on_generated_sketches(seed in any::<u64>()) {
        let Some(sketch) = generated(seed) else {
            return Ok(());
        };
        thread_local!(static W: std::cell::RefCell<SketchWorker> = std::cell::RefCell::new(
            SketchWorker::start_worker(Path::new(WORKER)).unwrap()
        ));
        let got = W.with(|w| invariants_hold(&sketch, &mut w.borrow_mut()));
        prop_assert!(got.is_ok(), "seed {seed}: {}", got.unwrap_err());
    }
}

#[test]
fn the_sketch_the_persisted_seed_drew_before_the_tangencies_holds_the_invariants() {
    // sketch_states.proptest-regressions keeps seed 2832580052307869825, found when the
    // generator drew a constraint's kind from 13; drawing from 16 (the tangencies and equal
    // radii) changed what that seed builds. This is the sketch it built then, regenerated
    // from that generator exactly. 7 points, 14 unknowns; the circle's radius, 1; the arc's
    // radius and angles, 3: 18. Rows: the arc's 4 rules; k0000000's distance_y, 1 (y of
    // p0000101 − y of p0000004 = 6, as drawn); k0000001 a reference, none; k0000002's
    // vertical_points, 1 (x of p0000000 = x of p0000003); k0000003's vertical, 1 (x of
    // p0000004 = x of p0000100); k0000004's symmetry about the y axis, 2 (p0000001 the
    // mirror of p0000004). Each row brings in a value no earlier one has, so they're
    // independent: 18 − 9 = 9 left. Every member keeps a free value (p0000000 its y,
    // p0000004 its x along the chain to p0000100 and p0000001, the circle its radius) and
    // nothing repeats: under defined, every member under defined, every constraint
    // satisfied.
    let points = [
        point("p0000000", -2.0, -4.0),
        point("p0000001", -1.0, 0.0),
        point("p0000002", 4.0, 0.0),
        point("p0000003", 3.0, -2.0),
        point("p0000004", 3.0, -2.0),
        point("p0000100", 7.0, -4.0),
        point("p0000101", 1.0, 4.0),
    ];
    let lines = [
        line("n0000000", "p0000002", "p0000000"),
        line("n0000001", "p0000004", "p0000100"),
    ];
    let circles = [circle("c0000000", "p0000003", 1.0)];
    let arcs = [arc("a0000000", "p0000002", "p0000100", "p0000101")];
    let constraints = [
        r#"{"id": "k0000000", "type": "distance_y", "a": "p0000004", "b": "p0000101", "value": 6, "driving": true}"#,
        r#"{"id": "k0000001", "type": "distance_y", "a": "origin", "b": "p0000004", "value": -1, "driving": false}"#,
        r#"{"id": "k0000002", "type": "vertical_points", "a": "p0000000", "b": "p0000003"}"#,
        r#"{"id": "k0000003", "type": "vertical", "line": "n0000001"}"#,
        r#"{"id": "k0000004", "type": "symmetric", "a": "p0000004", "b": "p0000001", "about": "y_axis"}"#,
    ];
    let sketch = sketch_of(&points, &lines, &circles, &arcs, &constraints);
    let (_, _, got) = solved(&sketch);
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::UnderDefined,
            dofs: 9,
            points: &[
                ("p0000000", Under),
                ("p0000001", Under),
                ("p0000002", Under),
                ("p0000003", Under),
                ("p0000004", Under),
                ("p0000100", Under),
                ("p0000101", Under),
            ],
            lines: &[("n0000000", Under), ("n0000001", Under)],
            circles: &[("c0000000", Under)],
            arcs: &[("a0000000", Under)],
            constraints: &[
                ("k0000000", Satisfied),
                ("k0000001", Satisfied),
                ("k0000002", Satisfied),
                ("k0000003", Satisfied),
                ("k0000004", Satisfied),
            ],
        },
    );
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    invariants_hold(&sketch, &mut worker).unwrap();
}

#[test]
fn the_generator_reaches_every_sketch_state_and_every_constraint_state() {
    // So the property above isn't vacuous: over these seeds, every sketch state and every
    // constraint state occurs, and most sketches pass the gate.
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let mut sketch_states = BTreeSet::new();
    let mut constraint_states = BTreeSet::new();
    let mut gated = 0;
    for seed in 0..400u64 {
        let Some(sketch) = generated(seed) else {
            continue;
        };
        gated += 1;
        let Ok(translation) = Translation::new(&sketch) else {
            continue;
        };
        let Ok(answer) = translation.solve(&mut worker) else {
            continue;
        };
        let got = states(&sketch, answer.diagnosed().unwrap()).unwrap();
        sketch_states.insert(got.state);
        constraint_states.extend(got.constraints.values().copied());
    }
    assert!(gated >= 200, "{gated} of 400 pass the gate");
    // Addendum 2's two sides, solved, so its invariants are checked on both: sketches whose
    // only repetitions are relations, read by their degrees of freedom (fully and under
    // defined both reached), and sketches with a repeated dimension, over defined.
    let (mut relation_only, mut dimension) = (BTreeSet::new(), 0);
    for seed in 0..4000u64 {
        let Some(sketch) = generated(seed) else {
            continue;
        };
        let Ok(translation) = Translation::new(&sketch) else {
            continue;
        };
        let Ok(answer) = translation.solve(&mut worker) else {
            continue;
        };
        if answer.outcome().is_err() {
            continue;
        }
        let got = states(&sketch, answer.diagnosed().unwrap()).unwrap();
        let repeated = |dimension: bool| {
            sketch.constraints.iter().any(|c| {
                got.constraints[&c.id].flagged()
                    && DIMENSIONS.contains(&c.form.type_name()) == dimension
            })
        };
        if repeated(true) {
            dimension += 1;
        } else if repeated(false) {
            relation_only.insert(got.state);
        }
    }
    assert!(
        dimension >= 5,
        "{dimension} solved with a repeated dimension"
    );
    // The generator draws too few constraints to fix a sketch whole with a relation to
    // spare, so a fully defined one with a repeated relation is the rectangle's to reach
    // (each_repetition_of_the_rectangle_alone_reads_by_its_kind, below).
    assert!(
        relation_only.contains(&SketchState::UnderDefined),
        "the states of sketches whose only repetitions are relations: {relation_only:?}"
    );
    // And some the translation refuses, so their invariants are checked too.
    let untranslated = (0..4000u64)
        .filter_map(generated)
        .filter(|sketch| Translation::new(sketch).is_err())
        .count();
    assert!(
        untranslated >= 10,
        "{untranslated} of 4000 refused by the translation"
    );
    // Five of each: every state of both kinds reached.
    assert_eq!(sketch_states.len(), 5, "{sketch_states:?}");
    assert_eq!(constraint_states.len(), 5, "{constraint_states:?}");
}

/// Constraints that repeat what the rectangle already fixes, and agree, by ID after its
/// eight (so by tag too): 12 relations, then 6 dimensions.
const RECTANGLE_REPEATS: [(&str, bool); 18] = [
    (
        r#"{"id": "zz000001", "type": "horizontal", "line": "6t7v8w9x"}"#,
        false,
    ),
    (
        r#"{"id": "zz000002", "type": "horizontal", "line": "j8k9m0n1"}"#,
        false,
    ),
    (
        r#"{"id": "zz000003", "type": "vertical", "line": "a0b1c2d3"}"#,
        false,
    ),
    (
        r#"{"id": "zz000004", "type": "vertical", "line": "p2q3r4s5"}"#,
        false,
    ),
    (
        r#"{"id": "zz000005", "type": "perpendicular", "a": "6t7v8w9x", "b": "a0b1c2d3"}"#,
        false,
    ),
    (
        r#"{"id": "zz000006", "type": "perpendicular", "a": "j8k9m0n1", "b": "p2q3r4s5"}"#,
        false,
    ),
    (
        r#"{"id": "zz000007", "type": "perpendicular", "a": "6t7v8w9x", "b": "p2q3r4s5"}"#,
        false,
    ),
    (
        r#"{"id": "zz000008", "type": "parallel", "a": "6t7v8w9x", "b": "j8k9m0n1"}"#,
        false,
    ),
    (
        r#"{"id": "zz000009", "type": "parallel", "a": "a0b1c2d3", "b": "p2q3r4s5"}"#,
        false,
    ),
    (
        r#"{"id": "zz000010", "type": "horizontal_points", "a": "0a1b2c3d", "b": "4e5f6g7h"}"#,
        false,
    ),
    (
        r#"{"id": "zz000011", "type": "vertical_points", "a": "4e5f6g7h", "b": "8j9k0m1n"}"#,
        false,
    ),
    (
        r#"{"id": "zz000012", "type": "equal", "a": "6t7v8w9x", "b": "j8k9m0n1"}"#,
        false,
    ),
    (
        r#"{"id": "zz000013", "type": "length", "line": "j8k9m0n1", "value": 40, "driving": true}"#,
        true,
    ),
    (
        r#"{"id": "zz000014", "type": "length", "line": "p2q3r4s5", "value": 20, "driving": true}"#,
        true,
    ),
    (
        r#"{"id": "zz000015", "type": "distance_x", "a": "origin", "b": "4e5f6g7h", "value": 40, "driving": true}"#,
        true,
    ),
    (
        r#"{"id": "zz000016", "type": "distance_y", "a": "origin", "b": "8j9k0m1n", "value": 20, "driving": true}"#,
        true,
    ),
    (
        r#"{"id": "zz000017", "type": "angle", "a": "6t7v8w9x", "b": "a0b1c2d3", "value": 90, "driving": true}"#,
        true,
    ),
    (
        r#"{"id": "zz000018", "type": "distance", "a": "0a1b2c3d", "b": "4e5f6g7h", "value": 40, "driving": true}"#,
        true,
    ),
];

/// The rectangle with the repetitions `mask`'s bits pick.
fn rectangle_repeating(mask: u32) -> Sketch {
    let extra: Vec<&str> = RECTANGLE_REPEATS
        .iter()
        .enumerate()
        .filter(|(i, _)| mask >> i & 1 == 1)
        .map(|(_, (c, _))| *c)
        .collect();
    rectangle(&extra)
}

#[test]
fn each_repetition_of_the_rectangle_alone_reads_by_its_kind() {
    // By hand, for each repetition alone: the rectangle's eight rows are independent, so one
    // more that agrees leaves 9 rows of rank 8, 0 left, and one dependent group, which has
    // the repetition in it (the eight alone are independent). Every constraint of it is one
    // row, in one group: PlaneGCS removes the latest tag (1.1.3 GCS.cpp's contest, rule 3),
    // the repetition's, `redundant`, every other satisfied. Addendum 2: a relation leaves
    // the sketch by `dofs`, fully defined, every member fully defined; a driving dimension
    // makes it over defined, with some member over defined (each dimension here reads a
    // point of the rectangle).
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    for (i, &(text, dimension)) in RECTANGLE_REPEATS.iter().enumerate() {
        let sketch = rectangle_repeating(1 << i);
        let translation = Translation::new(&sketch).unwrap();
        let answer = translation.solve(&mut worker).unwrap();
        assert!(answer.outcome().is_ok(), "{text}: {:?}", answer.outcome());
        let got = states(&sketch, answer.diagnosed().unwrap()).unwrap();
        let id = ElementId::new(&format!("zz{:06}", i + 1)).unwrap();
        let flagged: Vec<(&ElementId, &ConstraintState)> = got
            .constraints
            .iter()
            .filter(|(_, s)| s.flagged())
            .collect();
        assert_eq!(flagged, [(&id, &Redundant)], "{text}");
        assert_eq!(got.dofs, 0, "{text}");
        let members = || got.points.values().chain(got.lines.values());
        if dimension {
            assert_eq!(got.state, SketchState::OverDefined, "{text}");
            assert!(members().any(|&s| s == Over), "{text}: {got:#?}");
        } else {
            assert_eq!(got.state, SketchState::FullyDefined, "{text}");
            assert!(members().all(|&s| s == Fully), "{text}: {got:#?}");
        }
        invariants_hold(&sketch, &mut worker).unwrap();
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, .. ProptestConfig::default() })]

    #[test]
    fn addendum_2s_invariants_hold_on_the_rectangle_repeated_any_way(
        mask in 0u32..(1 << RECTANGLE_REPEATS.len())
    ) {
        // Any set of repetitions that agree: the sketch solves, 0 left, and the invariants
        // hold whichever constraints PlaneGCS's contest names (its own or the rectangle's).
        let sketch = rectangle_repeating(mask);
        thread_local!(static W: std::cell::RefCell<SketchWorker> = std::cell::RefCell::new(
            SketchWorker::start_worker(Path::new(WORKER)).unwrap()
        ));
        let got = W.with(|w| {
            let mut w = w.borrow_mut();
            let answer = Translation::new(&sketch).unwrap().solve(&mut *w).unwrap();
            let read = answer.diagnosed().unwrap();
            let got = states(&sketch, read).unwrap();
            if answer.outcome().is_err() || got.dofs != 0 {
                return Err(format!("{:?} {got:#?}", answer.outcome()));
            }
            invariants_hold(&sketch, &mut w)
        });
        prop_assert!(got.is_ok(), "mask {mask:#x}: {}", got.unwrap_err());
    }
}

// --- What a flagged constraint governs: its equations' support (item 32's addendum, B) ----

#[test]
fn a_repeated_tangent_at_depends_on_the_lines_ends_the_arcs_centre_and_the_joint_and_over_defines_none()
 {
    // A quarter arc about p1 (0, 0) from p2 (5, 0) to p3 (0, 5), and a line from p3 to
    // p4 (−10, 5), leaving p3 along −x as the arc arrives there: tangent at p3, given twice.
    // 11 unknowns (4 points, the arc's radius and angles), 6 rows (the arc's 4 rules, the two
    // tangencies), the second repeating the first: rank 5, 6 left. Of two alike, PlaneGCS
    // removes the later tag: k0000002 redundant. Its equation is the angle between the
    // curves' normals at p3, which reads the line's ends and the arc's centre (1.1.3's
    // CalculateNormal) and never the arc's far end. A tangency is a relation, so under
    // addendum 2 it over-defines nothing (re-derived: before, p1, p3, p4 and both curves
    // were over defined by it): every member by its freedom, the sketch by `dofs`, 6, under
    // defined. By hand: p4's x and y are in the tangency rows alone, of rank 1, so at most
    // one is a pivot and p4 is under defined, and so is the line; the arc's 9 values in all
    // (its centre, ends, radius, and angles) are in 5 rows of rank 5 at most, so 4 at least
    // are past the rank and the arc is under defined. Which of p1's, p2's, and p3's values
    // are past the rank is the full pivoting of PlaneGCS's QR, not a count by hand: each is
    // as the worker's free values give it, and none is over defined.
    let points = [
        point("p0000001", 0.0, 0.0),
        point("p0000002", 5.0, 0.0),
        point("p0000003", 0.0, 5.0),
        point("p0000004", -10.0, 5.0),
    ];
    let lines = [line("n0000001", "p0000003", "p0000004")];
    let arcs = [arc("a0000001", "p0000001", "p0000002", "p0000003")];
    let constraints = [
        r#"{"id": "k0000001", "type": "tangent_at", "a": "n0000001", "b": "a0000001", "at": "p0000003", "direction": "same"}"#,
        r#"{"id": "k0000002", "type": "tangent_at", "a": "n0000001", "b": "a0000001", "at": "p0000003", "direction": "same"}"#,
    ];
    let sketch = sketch_of(&points, &lines, &[], &arcs, &constraints);
    let (answer, diagnosis, got) = solved(&sketch);
    assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
    let depended_on: BTreeSet<&str> = diagnosis.points_depended_on
        [&ElementId::new("k0000002").unwrap()]
        .iter()
        .map(ElementId::as_str)
        .collect();
    assert_eq!(
        depended_on,
        BTreeSet::from(["p0000001", "p0000003", "p0000004"])
    );
    let by_freedom = |p: &str| {
        if diagnosis.free.contains(&ElementId::new(p).unwrap()) {
            Under
        } else {
            Fully
        }
    };
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::UnderDefined,
            dofs: 6,
            points: &[
                ("p0000001", by_freedom("p0000001")),
                ("p0000002", by_freedom("p0000002")),
                ("p0000003", by_freedom("p0000003")),
                ("p0000004", Under),
            ],
            lines: &[("n0000001", Under)],
            circles: &[],
            arcs: &[("a0000001", Under)],
            constraints: &[("k0000001", Satisfied), ("k0000002", Redundant)],
        },
    );
}

#[test]
fn a_repeated_equal_of_two_circles_leaves_both_circles_and_centres_as_their_freedom_gives() {
    // Circles of radius 5 about p1 (0, 0) and p2 (20, 0), made equal twice: 6 unknowns (two
    // centres, two radii), one row repeated, rank 1, 5 left. The later equal redundant; its
    // equation is r1 − r2, which no centre is in. An `equal` is a relation, so under addendum
    // 2 it over-defines nothing; re-derived: both centres' columns are zero, so both centres
    // are free (under defined), and each circle has a free centre, so both circles are under
    // defined too, whichever radius is the pivot; the sketch under defined, 5 left.
    let points = [point("p0000001", 0.0, 0.0), point("p0000002", 20.0, 0.0)];
    let circles = [
        circle("c0000001", "p0000001", 5.0),
        circle("c0000002", "p0000002", 5.0),
    ];
    let constraints = [
        r#"{"id": "k0000001", "type": "equal", "a": "c0000001", "b": "c0000002"}"#,
        r#"{"id": "k0000002", "type": "equal", "a": "c0000001", "b": "c0000002"}"#,
    ];
    let sketch = sketch_of(&points, &[], &circles, &[], &constraints);
    let (answer, diagnosis, got) = solved(&sketch);
    assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
    assert!(diagnosis.points_depended_on[&ElementId::new("k0000002").unwrap()].is_empty());
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::UnderDefined,
            dofs: 5,
            points: &[("p0000001", Under), ("p0000002", Under)],
            lines: &[],
            circles: &[("c0000001", Under), ("c0000002", Under)],
            arcs: &[],
            constraints: &[("k0000001", Satisfied), ("k0000002", Redundant)],
        },
    );
}

#[test]
fn a_conflicting_tangent_on_an_arc_leaves_the_arcs_own_ends_under_defined() {
    // The port review's proof. A half arc about p1, held at the origin, radius 10, from p2
    // (10, 0) to p3 (−10, 0); a line from p4, held at (−20, 10.5), ending at p5 tangent on the
    // arc; and p5 held 5 above the origin, which no tangent from p4 touches: a conflict. The
    // tangency's rows (p5 on the arc's circle: p5, the centre, the radius; the normals' angle:
    // p5, the line's ends, the centre) never read the arc's ends or angles, which only the
    // arc's own rules touch: p2 and p3 slide along the circle, under defined, though the
    // tangency is flagged.
    let points = [
        point("p0000001", 0.0, 0.0),
        point("p0000002", 10.0, 0.0),
        point("p0000003", -10.0, 0.0),
        point("p0000004", -20.0, 10.5),
        point("p0000005", 0.5, 10.2),
    ];
    let lines = [line("n0000001", "p0000004", "p0000005")];
    let arcs = [arc("a0000001", "p0000001", "p0000002", "p0000003")];
    let constraints = [
        FROM_AT_ORIGIN[0],
        FROM_AT_ORIGIN[1],
        r#"{"id": "k0000003", "type": "radius", "curve": "a0000001", "value": 10, "driving": true}"#,
        r#"{"id": "k0000004", "type": "distance_x", "a": "origin", "b": "p0000004", "value": -20, "driving": true}"#,
        r#"{"id": "k0000005", "type": "distance_y", "a": "origin", "b": "p0000004", "value": 10.5, "driving": true}"#,
        r#"{"id": "k0000006", "type": "tangent_on", "a": "n0000001", "at": "p0000005", "b": "a0000001", "direction": "same"}"#,
        r#"{"id": "k0000007", "type": "distance_y", "a": "origin", "b": "p0000005", "value": 5, "driving": true}"#,
    ];
    let sketch = sketch_of(&points, &lines, &[], &arcs, &constraints);
    let (_, _, got) = solved(&sketch);
    assert_eq!(
        got.solve_problem.as_ref().map(|e| e.code()),
        Some(ErrorCode::OverConstrained),
        "{got:#?}"
    );
    assert_eq!(got.state, SketchState::OverDefined);
    let at = |id: &str| ElementId::new(id).unwrap();
    assert_eq!(got.constraints[&at("k0000006")], Conflicting, "{got:#?}");
    assert_eq!(got.constraints[&at("k0000007")], Conflicting, "{got:#?}");
    assert_eq!(got.points[&at("p0000005")], Over, "{got:#?}");
    assert_eq!(got.points[&at("p0000002")], Under, "{got:#?}");
    assert_eq!(got.points[&at("p0000003")], Under, "{got:#?}");
    // Named by the flagged tangency.
    assert_eq!(got.arcs, members(&[("a0000001", Over)]));
    assert_eq!(got.lines, members(&[("n0000001", Over)]));
}

/// That every row `translation` sends depends on no value its parameter list doesn't have,
/// and on its whole list for every form but the three whose equations read less: a
/// tangency at an end, an angle at a point (never an arc's angles, nor its ends but the
/// point), and equal radii (never a centre).
fn depends_on_within_reads(translation: &Translation) -> Result<(), String> {
    use silico_protocol::SolverConstraint as S;
    let system = &translation.system;
    let arc_own = |g: silico_protocol::Geometry| match system.geometry.get(g.0 as usize) {
        Some(silico_protocol::SketchGeometry::Arc {
            start_angle,
            end_angle,
            ..
        }) => vec![*start_angle, *end_angle],
        _ => Vec::new(),
    };
    let center = |g: silico_protocol::Geometry| match system.geometry.get(g.0 as usize) {
        Some(
            silico_protocol::SketchGeometry::Circle { center, .. }
            | silico_protocol::SketchGeometry::Arc { center, .. },
        ) => vec![center.x, center.y],
        _ => Vec::new(),
    };
    for c in &system.constraints {
        let (depends, reads) = (system.depends_on(c), system.reads(c));
        let list: BTreeSet<_> = reads.iter().copied().collect();
        if let Some(v) = depends.iter().find(|v| !list.contains(v)) {
            return Err(format!("{c:?} depends on {v:?}, not in {reads:?}"));
        }
        let never = match *c {
            S::TangentOn { a, b, .. } | S::AngleViaPoint { a, b, .. } => {
                [arc_own(a), arc_own(b)].concat()
            }
            S::EqualRadius { a, b, .. } => [center(a), center(b)].concat(),
            _ if depends != reads => return Err(format!("{c:?}: {depends:?} ≠ {reads:?}")),
            _ => Vec::new(),
        };
        if let Some(v) = never.iter().find(|v| depends.contains(v)) {
            return Err(format!("{c:?} depends on {v:?}"));
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, .. ProptestConfig::default() })]

    #[test]
    fn what_a_row_depends_on_is_within_its_parameter_list_on_generated_sketches(
        seed in any::<u64>()
    ) {
        let Some(sketch) = generated(seed) else {
            return Ok(());
        };
        let Ok(translation) = Translation::new(&sketch) else {
            return Ok(());
        };
        let got = depends_on_within_reads(&translation);
        prop_assert!(got.is_ok(), "seed {seed}: {}", got.unwrap_err());
    }
}

#[test]
fn the_generator_sends_each_form_whose_equations_read_less_than_its_list() {
    // So the property above isn't vacuous for the three: over these seeds, each is sent, and
    // each at least once on a round curve, where it reads less than its list.
    use silico_protocol::SolverConstraint as S;
    let mut narrower = BTreeSet::new();
    for seed in 0..400u64 {
        let Some(sketch) = generated(seed) else {
            continue;
        };
        let Ok(translation) = Translation::new(&sketch) else {
            continue;
        };
        let system = &translation.system;
        for c in &system.constraints {
            let set = |v: Vec<silico_protocol::Value>| v.into_iter().collect::<BTreeSet<_>>();
            if set(system.depends_on(c)) == set(system.reads(c)) {
                continue;
            }
            narrower.insert(match c {
                S::TangentOn { .. } => "tangent_on",
                S::AngleViaPoint { .. } => "tangent_at",
                S::EqualRadius { .. } => "equal",
                other => panic!("{other:?} reads less than its list"),
            });
        }
    }
    assert_eq!(
        narrower,
        BTreeSet::from(["equal", "tangent_at", "tangent_on"])
    );
}

// --- FreeCAD's fully constrained sketches (tests/src/Mod/PartDesign/App/Pipe.cpp) ---------

#[test]
fn pipes_circle_centred_on_the_root_point_with_a_diameter_is_fully_defined() {
    // Pipe.cpp's circle sketch: a circle of radius 1 centred on the root point (here its
    // centre placed at the origin by x and y) with a diameter of 1, FullyConstrained in
    // FreeCAD. 3 unknowns (the centre, the radius), 3 equations: none left.
    let sketch = sketch_of(
        &[point("p0000001", 0.0, 0.0)],
        &[],
        &[circle("c0000001", "p0000001", 1.0)],
        &[],
        &[
            FROM_AT_ORIGIN[0],
            FROM_AT_ORIGIN[1],
            r#"{"id": "k0000003", "type": "diameter", "curve": "c0000001", "value": 1, "driving": true}"#,
        ],
    );
    let (answer, _, got) = solved(&sketch);
    assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::FullyDefined,
            dofs: 0,
            points: &[("p0000001", Fully)],
            lines: &[],
            circles: &[("c0000001", Fully)],
            arcs: &[],
            constraints: &[
                ("k0000001", Satisfied),
                ("k0000002", Satisfied),
                ("k0000003", Satisfied),
            ],
        },
    );
}

#[test]
fn pipes_vertical_line_from_the_root_point_with_a_length_is_fully_defined() {
    // Pipe.cpp's line sketch: a line from the root point to (0, 1), vertical, 1 long,
    // FullyConstrained in FreeCAD. 4 unknowns, 4 equations: none left.
    let sketch = sketch_of(
        &line_points((0.0, 1.0)),
        &one_line(),
        &[],
        &[],
        &[
            FROM_AT_ORIGIN[0],
            FROM_AT_ORIGIN[1],
            r#"{"id": "k0000003", "type": "vertical", "line": "n0000001"}"#,
            r#"{"id": "k0000004", "type": "length", "line": "n0000001", "value": 1, "driving": true}"#,
        ],
    );
    let (answer, _, got) = solved(&sketch);
    assert!(answer.outcome().is_ok(), "{:?}", answer.outcome());
    expect(
        &got,
        Want {
            problem: None,
            state: SketchState::FullyDefined,
            dofs: 0,
            points: &[("p0000001", Fully), ("p0000002", Fully)],
            lines: &[("n0000001", Fully)],
            circles: &[],
            arcs: &[],
            constraints: &[
                ("k0000001", Satisfied),
                ("k0000002", Satisfied),
                ("k0000003", Satisfied),
                ("k0000004", Satisfied),
            ],
        },
    );
}

#[test]
fn the_conflicting_coincidence_of_two_lines_held_apart_is_over_defined_as_stored() {
    // TestCoincidentCommandGui.py's conflicting coincidence, as Silico's M3.3 port has it
    // (silico-sketch's tests/document.rs): FreeCAD holds two lines' ends at x 10 and x 20 and
    // then makes them coincident, three constraints in conflict; with shared points the
    // coincidence is the one point the two lines meet at, held at both, so the two holds
    // conflict. 6 unknowns, the two rows on p2's x of rank 1: 5 left. Both holds conflicting;
    // p2, which they read, over defined; the lines, named by neither, and their free far ends
    // under defined; the sketch over defined, as stored.
    let sketch = sketch_of(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 10.0, 0.0),
            point("p0000003", 30.0, 0.0),
        ],
        &[
            line("n0000001", "p0000001", "p0000002"),
            line("n0000002", "p0000002", "p0000003"),
        ],
        &[],
        &[],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 10, "driving": true}"#,
            r#"{"id": "k0000002", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 20, "driving": true}"#,
        ],
    );
    let (_, _, got) = solved(&sketch);
    expect(
        &got,
        Want {
            problem: Some(ErrorCode::OverConstrained),
            state: SketchState::OverDefined,
            dofs: 5,
            points: &[("p0000001", Under), ("p0000002", Over), ("p0000003", Under)],
            lines: &[("n0000001", Under), ("n0000002", Under)],
            circles: &[],
            arcs: &[],
            constraints: &[("k0000001", Conflicting), ("k0000002", Conflicting)],
        },
    );
}

/// PlaneGCS's degrees of freedom for `sketch` where it's drawn: its translation sent to the
/// worker once, and the reply's diagnosis, which PlaneGCS takes before any algorithm runs,
/// read; none if the translation refuses it. (A solve's answer is its load's, diagnosed
/// where the solve put the drawing, an arc's ends on its circle.)
fn planegcs_dofs(sketch: &Sketch, worker: &mut SketchWorker) -> Option<i32> {
    let translation = Translation::new(sketch).ok()?;
    let solved = worker.solve(translation.system.clone()).unwrap();
    Some(
        translation
            .read_diagnosed(&solved)
            .unwrap()
            .diagnosis()
            .dofs,
    )
}

/// Five points in general position, and for each, as a centre, three arcs end to end through
/// the other four, in ID order: 15 arcs, reading all 10 distances between the points.
fn five_centres() -> (Vec<String>, Vec<String>) {
    let ids = ["p0000000", "p0000001", "p0000002", "p0000003", "p0000004"];
    let at = [
        (0.0, 0.0),
        (7.0, 1.0),
        (2.0, 9.0),
        (-5.0, 3.0),
        (-1.0, -6.0),
    ];
    let points = ids
        .iter()
        .zip(at)
        .map(|(id, (x, y))| point(id, x, y))
        .collect();
    let mut arcs = Vec::new();
    for c in 0..5 {
        let others: Vec<usize> = (0..5).filter(|&o| o != c).collect();
        for w in others.windows(2) {
            arcs.push(arc(
                &format!("a{:07}", arcs.len()),
                ids[c],
                ids[w[0]],
                ids[w[1]],
            ));
        }
    }
    (points, arcs)
}

#[test]
fn an_arcs_rules_repeat_where_planegcs_finds_them_repeat_counted_by_hand() {
    // Geometry alone, no constraints: the arcs' rules are the only rows. Each count is by
    // hand: 2 a point, 3 an arc (its radius and angles), less the rules' rank, 3 an arc (on
    // its own radius and angles) and the rank of what's left of each arc's four, one row,
    // d(C, S) − d(C, E). PlaneGCS's own count, from the worker, must be it: of the sketch's
    // solve, and of its geometry alone as a refused translation's is diagnosed
    // (Diagnosis::untranslated).
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let centre_and = |ends: &[(&str, f64, f64)]| {
        let mut points = vec![point("p0000000", 0.0, 0.0)];
        points.extend(ends.iter().map(|&(id, x, y)| point(id, x, y)));
        points
    };
    let quarter = centre_and(&[("p0000001", 5.0, 0.0), ("p0000002", 0.0, 5.0)]);
    let halves = centre_and(&[("p0000001", 5.0, 0.0), ("p0000002", -5.0, 0.0)]);
    let ring = centre_and(&[
        ("p0000001", 5.0, 0.0),
        ("p0000002", -3.0, 4.0),
        ("p0000003", -3.0, -4.0),
    ]);
    let triangle = vec![
        point("p0000000", 0.0, 0.0),
        point("p0000001", 6.0, 0.0),
        point("p0000002", 3.0, 5.0),
    ];
    let copies = |n: usize| -> Vec<String> {
        (0..n)
            .map(|i| arc(&format!("a{i:07}"), "p0000000", "p0000001", "p0000002"))
            .collect()
    };
    let (five, fifteen) = five_centres();
    let cases: Vec<(&str, Vec<String>, Vec<String>, i32)> = vec![
        // 3 points, 6; 1 arc, 3; rank 3 + 1: 5, FreeCAD 1.1.3's DoF for a lone arc.
        ("a lone arc", quarter.clone(), copies(1), 5),
        // A whole circle of two halves on its centre and two points: 6 + 6; each half's
        // row d(p0, p1) − d(p0, p2) and d(p0, p2) − d(p0, p1), the one the other's
        // negative: rank 6 + 1, so 5, FreeCAD 1.1.3's for two halves sharing their points
        // (10 unshared), not 4.
        (
            "two halves",
            halves,
            vec![
                arc("a0000000", "p0000000", "p0000001", "p0000002"),
                arc("a0000001", "p0000000", "p0000002", "p0000001"),
            ],
            5,
        ),
        // Three arcs end to end round one centre, a whole circle: 8 + 9; rows
        // d1 − d2, d2 − d3, d3 − d1 (d the distance from the centre), summing to nothing:
        // rank 9 + 2, so 6: the centre, the radius, and where the three ends sit on it.
        (
            "a ring of three",
            ring,
            vec![
                arc("a0000000", "p0000000", "p0000001", "p0000002"),
                arc("a0000001", "p0000000", "p0000002", "p0000003"),
                arc("a0000002", "p0000000", "p0000003", "p0000001"),
            ],
            6,
        ),
        // One arc drawn n times on its three points: 6 + 3n; n rows, each
        // d(p0, p1) − d(p0, p2): rank 3n + 1, so 5 for every n (the count subtracting 4 an
        // arc read 6 − n: 4, 3, and −1).
        ("an arc drawn twice", quarter.clone(), copies(2), 5),
        ("an arc drawn three times", quarter.clone(), copies(3), 5),
        ("an arc drawn seven times", quarter, copies(7), 5),
        // A triangle, each corner the centre of an arc through the other two: 6 + 9; rows
        // d01 − d02, d12 − d10, d20 − d21, three distances, each row one's difference from
        // another, summing to nothing round them: rank 9 + 2, so 4 (an equilateral
        // triangle's place, turn, and size), not the 3 of counting each centre apart.
        (
            "a triangle of centres",
            triangle,
            vec![
                arc("a0000000", "p0000000", "p0000001", "p0000002"),
                arc("a0000001", "p0000001", "p0000002", "p0000000"),
                arc("a0000002", "p0000002", "p0000000", "p0000001"),
            ],
            4,
        ),
        // Five centres, 15 arcs through all 10 distances, one graph: 10 + 45, its rows'
        // rank 9 by the distances, but the points only move 2·5 − 3 = 7 ways that change a
        // distance: rank 45 + 7, so 3 (10 − 7), the drawing's place and turn and nothing
        // else.
        ("five centres", five, fifteen, 3),
    ];
    for (name, points, arcs, want) in cases {
        let sketch = sketch_of(&points, &[], &[], &arcs, &[]);
        assert_eq!(
            planegcs_dofs(&sketch, &mut worker),
            Some(want),
            "{name}: PlaneGCS"
        );
        assert_eq!(
            Diagnosis::untranslated(&sketch, &mut worker).unwrap().dofs,
            want,
            "{name}: the geometry alone"
        );
    }
}

/// Each `(id, x, y)` a point, each `(id, centre, start, end)` an arc, by number: `p0000003`
/// is 3, `a0000010` is 10.
fn numbered(points: &[(f64, f64)], arcs: &[(u32, u32, u32, u32)]) -> Sketch {
    let points: Vec<String> = points
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| point(&format!("p{i:07}"), x, y))
        .collect();
    let arcs: Vec<String> = arcs
        .iter()
        .map(|&(i, c, s, e)| {
            arc(
                &format!("a{i:07}"),
                &format!("p{c:07}"),
                &format!("p{s:07}"),
                &format!("p{e:07}"),
            )
        })
        .collect();
    sketch_of(&points, &[], &[], &arcs, &[])
}

#[test]
fn arcs_whose_distances_repeat_past_any_count_of_them_are_ranked_as_planegcs_ranks_them() {
    // Drawings the generator found where a count from the arcs' graph alone (each centre's
    // cycles, shared distances, and 2n − 3 a joined set of n points) reads a degree of
    // freedom fewer than PlaneGCS: repetitions only the drawing's numbers show. Each want is
    // PlaneGCS's, through the worker, diagnosed where drawn, of the solve and of the geometry
    // alone; and, the drawing in line moved a micron off it, the graph count again.
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let cases = [
        (
            // Six points, ten arcs (the graph count: 3).
            "six points",
            numbered(
                &[
                    (-6.32, 9.48),
                    (-6.797, 2.729),
                    (-2.865, -3.819),
                    (2.409, -1.526),
                    (-9.963, -9.677),
                    (-2.193, 3.969),
                ],
                &[
                    (0, 5, 2, 3),
                    (1, 2, 1, 3),
                    (2, 1, 2, 5),
                    (5, 3, 0, 1),
                    (6, 1, 5, 3),
                    (7, 5, 3, 0),
                    (8, 2, 4, 5),
                    (9, 2, 3, 5),
                    (10, 1, 0, 2),
                    (11, 1, 3, 5),
                ],
            ),
            4,
        ),
        (
            // Seven points, twelve arcs (the graph count: 4).
            "seven points",
            numbered(
                &[
                    (8.57, -7.217),
                    (-7.009, -1.588),
                    (-1.109, 6.793),
                    (4.315, -6.874),
                    (7.02, 9.101),
                    (3.025, -9.053),
                    (-5.959, -9.752),
                ],
                &[
                    (1, 6, 4, 1),
                    (3, 5, 1, 2),
                    (4, 4, 5, 0),
                    (5, 4, 6, 1),
                    (6, 2, 6, 5),
                    (7, 4, 1, 2),
                    (8, 2, 6, 5),
                    (13, 2, 1, 3),
                    (14, 4, 2, 6),
                    (16, 2, 1, 6),
                    (17, 6, 2, 4),
                    (18, 4, 0, 2),
                ],
            ),
            5,
        ),
        (
            // On the grid: p2, p3, and p4 in one line, x = 0, so from p4 the ends p2 and p3
            // are one direction (the graph count: 3).
            "points in line",
            numbered(
                &[
                    (-4.0, -3.0),
                    (1.0, 4.0),
                    (0.0, 4.0),
                    (0.0, 1.0),
                    (0.0, -3.0),
                ],
                &[
                    (0, 0, 1, 2),
                    (5, 2, 4, 0),
                    (7, 4, 2, 0),
                    (8, 2, 1, 3),
                    (9, 4, 0, 3),
                    (10, 4, 1, 0),
                    (11, 2, 0, 1),
                ],
            ),
            4,
        ),
        (
            // The tests reviewer's (round 4): p3 moved 1 µm off x = 0, so from p4 the ends p2
            // and p3 are no longer one direction, and the repetition is gone: the graph count,
            // 3, PlaneGCS's too (its pivot for it about 1e-7 of the largest, far above its
            // 1e-13 threshold).
            "points a micron off line",
            numbered(
                &[
                    (-4.0, -3.0),
                    (1.0, 4.0),
                    (0.0, 4.0),
                    (1e-6, 1.0),
                    (0.0, -3.0),
                ],
                &[
                    (0, 0, 1, 2),
                    (5, 2, 4, 0),
                    (7, 4, 2, 0),
                    (8, 2, 1, 3),
                    (9, 4, 0, 3),
                    (10, 4, 1, 0),
                    (11, 2, 0, 1),
                ],
            ),
            3,
        ),
    ];
    for (name, sketch, want) in cases {
        assert_eq!(
            planegcs_dofs(&sketch, &mut worker),
            Some(want),
            "{name}: PlaneGCS"
        );
        assert_eq!(
            Diagnosis::untranslated(&sketch, &mut worker).unwrap().dofs,
            want,
            "{name}: the geometry alone"
        );
    }
}

#[test]
fn a_drawing_a_hair_from_a_repetition_is_counted_as_freecad_counts_it() {
    // The port reviewer's (round 4): A (10, 0), B (0, 0), C (5, 1e-12), D (10, 10), E (10, 5),
    // and arcs about A from B to D, about A from E to C, about B from E to C, and about E from
    // D to B. With C on AB (d = 0) the arcs' distance rows repeat once, and 1e-12 off it the
    // repetition's pivot is below PlaneGCS's threshold: FreeCAD 1.1.3 counts 7, as at d = 0
    // (10 + 12 − (12 + 3)). Beside an arc of radius 1000 about (0, 100) from (1000, 100) to
    // (0, 1100), which sweeps a quarter and shares nothing, FreeCAD 1.1.3 counts 12: the
    // big arc's 5, and the drawing's 7 still. PlaneGCS's own threshold decides it, so the
    // worker gives FreeCAD's count, of the solve and of the geometry alone.
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let near = [
        (10.0, 0.0),
        (0.0, 0.0),
        (5.0, 1e-12),
        (10.0, 10.0),
        (10.0, 5.0),
    ];
    let arcs = [(0, 0, 1, 3), (1, 0, 4, 2), (2, 1, 4, 2), (3, 4, 3, 1)];
    let mut far = near.to_vec();
    far.extend([(0.0, 100.0), (1000.0, 100.0), (0.0, 1100.0)]);
    let mut with_big = arcs.to_vec();
    with_big.push((4, 5, 6, 7));
    for (name, sketch, want) in [
        ("alone", numbered(&near, &arcs), 7),
        ("beside a big arc", numbered(&far, &with_big), 12),
    ] {
        assert_eq!(
            planegcs_dofs(&sketch, &mut worker),
            Some(want),
            "{name}: PlaneGCS"
        );
        assert_eq!(
            Diagnosis::untranslated(&sketch, &mut worker).unwrap().dofs,
            want,
            "{name}: the geometry alone"
        );
    }
}

#[test]
fn a_lone_arc_that_sweeps_nothing_has_five_degrees_as_freecad_counts_a_lone_arc() {
    // An arc about p1 (0, 0) from p2 (5, 0) to p3 (8, 0), both ends at angle 0: it sweeps
    // nothing, so the translation refuses it, and its geometry alone is diagnosed by the
    // worker, both its angles its start's. By hand: its centre and ends, 3 points, 6
    // coordinates; its radius and two angles, 3; its 4 rules (each end on its circle at its
    // angle, in x and y), which PlaneGCS always sends, tag 0, none repeating another (a lone
    // arc shares nothing): 6 + 3 − 4 = 5, FreeCAD 1.1.3's DoF for a lone arc, though this one
    // is drawn with its end off its circle. Every member under defined, from PlaneGCS's own
    // free values: the arc and its three points.
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
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
    let read = Diagnosed::untranslated(&lone, &mut worker)
        .unwrap()
        .unwrap();
    let refusal = read.outcome().unwrap_err();
    assert_eq!(
        (refusal.code(), refusal.elements()),
        (ErrorCode::InvalidInput, &["a0000001".to_owned()][..])
    );
    let free: BTreeSet<ElementId> = ["a0000001", "p0000001", "p0000002", "p0000003"]
        .map(|id| ElementId::new(id).unwrap())
        .into();
    assert_eq!(
        read.diagnosis(),
        &Diagnosis {
            dofs: 5,
            free,
            ..Diagnosis::default()
        }
    );
    expect(
        &states(&lone, &read).unwrap(),
        Want {
            problem: Some(ErrorCode::InvalidInput),
            state: SketchState::Invalid,
            dofs: 5,
            points: &[
                ("p0000001", Under),
                ("p0000002", Under),
                ("p0000003", Under),
            ],
            lines: &[],
            circles: &[],
            arcs: &[("a0000001", Under)],
            constraints: &[],
        },
    );
}

#[test]
fn two_halves_beside_an_arc_that_sweeps_nothing_are_counted_by_the_worker_as_by_hand() {
    // Two halves of a circle about p1 (0, 0) on p2 (5, 0) and p3 (−5, 0), and an arc about
    // p1 from p2 to p4 (8, 0), both at angle 0, which sweeps nothing, so the translation
    // refuses the sketch. By hand: 4 points and 3 arcs, 8 + 9; less each arc's 3 on its own
    // radius and angles, and the rank of the rows left, d12 − d13, d13 − d12, d12 − d14, the
    // first two one the other's negative: 9 + 2, so 6 (the count subtracting 4 an arc read
    // 5). Through the worker, its geometry alone; every member under defined.
    let mut worker = SketchWorker::start_worker(Path::new(WORKER)).unwrap();
    let refused = sketch_of(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 5.0, 0.0),
            point("p0000003", -5.0, 0.0),
            point("p0000004", 8.0, 0.0),
        ],
        &[],
        &[],
        &[
            arc("a0000001", "p0000001", "p0000002", "p0000003"),
            arc("a0000002", "p0000001", "p0000003", "p0000002"),
            arc("a0000003", "p0000001", "p0000002", "p0000004"),
        ],
        &[],
    );
    let read = Diagnosed::untranslated(&refused, &mut worker)
        .unwrap()
        .unwrap();
    assert_eq!(read.outcome().unwrap_err().elements(), ["a0000003"]);
    expect(
        &states(&refused, &read).unwrap(),
        Want {
            problem: Some(ErrorCode::InvalidInput),
            state: SketchState::Invalid,
            dofs: 6,
            points: &[
                ("p0000001", Under),
                ("p0000002", Under),
                ("p0000003", Under),
                ("p0000004", Under),
            ],
            lines: &[],
            circles: &[],
            arcs: &[
                ("a0000001", Under),
                ("a0000002", Under),
                ("a0000003", Under),
            ],
            constraints: &[],
        },
    );
}
