//! break-it on the sketch read's definition states (`silico_core::sketch_states`,
//! `Translation::diagnosis`): diagnoses that name what the sketch doesn't have, an outcome
//! and a diagnosis that disagree, worker replies of garbage handed to the diagnosis without
//! `check_reply`, the largest sketches the gate takes, sketches the gate never saw, list
//! order, and the claim that any translation of a sketch reads a reply alike. Expected
//! values by hand from the rules (docs/ports/sketcher.md, *Definition states*, as M5's open
//! item 32 decided them).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::fixture;

use fixture::{arc, circle, line, point};

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use silico_core::document::{ElementId, Operation, Sketch, read, validate};
use silico_core::fake::FakeSketches;
use silico_core::sketch::{Diagnosis, SketchSolution, Translation};
use silico_core::sketch_states::{ConstraintState, MemberState, SketchState, SketchStates, states};
use silico_core::{Error, ErrorCode};
use silico_protocol::{Algorithm, SolveStatus, Solved, Tag, Value};

use ConstraintState::{Conflicting, Redundant, Satisfied};
use MemberState::{FullyDefined as Fully, OverDefined as Over, UnderDefined as Under};

fn text(
    points: &[String],
    lines: &[String],
    circles: &[String],
    arcs: &[String],
    constraints: &[String],
) -> String {
    fixture::document(&fixture::fields(
        &points.join(", "),
        &lines.join(", "),
        &circles.join(", "),
        &arcs.join(", "),
        &constraints.join(", "),
    ))
}

/// The sketch `text` reads as, whether or not the gate takes it.
fn read_sketch(text: &str) -> Sketch {
    match &read(text.as_bytes()).unwrap().features[0].operation {
        Operation::Sketch(s) => s.clone(),
        other => panic!("{other:?}"),
    }
}

/// The sketch `text` reads as, through the gate.
fn gated(text: &str) -> Sketch {
    validate(&read(text.as_bytes()).unwrap()).unwrap();
    read_sketch(text)
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

fn solution(dofs: i32) -> SketchSolution {
    SketchSolution {
        points: BTreeMap::new(),
        radii: BTreeMap::new(),
        references: BTreeMap::new(),
        dofs,
        free: BTreeSet::new(),
        redundant: BTreeMap::new(),
        partially_redundant: BTreeMap::new(),
        algorithm: Algorithm::DogLeg,
    }
}

fn c(s: &str) -> String {
    s.to_owned()
}

/// A line p1 (0, 0)–p2 (10, 0), p1 placed (k1, k2), horizontal (k3).
fn placed_line() -> Sketch {
    gated(&text(
        &[point("p0000001", 0.0, 0.0), point("p0000002", 10.0, 0.0)],
        &[line("n0000001", "p0000001", "p0000002")],
        &[],
        &[],
        &[
            c(
                r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            ),
            c(
                r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            ),
            c(r#"{"id": "k0000003", "type": "horizontal", "line": "n0000001"}"#),
        ],
    ))
}

#[test]
fn a_diagnosis_naming_what_the_sketch_doesnt_have_changes_nothing_it_does_have() {
    // Free, flagged, and read IDs that aren't the sketch's, or are another kind's (a line
    // listed free, a point listed conflicting, a line read as a point): the states are the
    // clean diagnosis's, member for member, and list only the sketch's own.
    let sketch = placed_line();
    let clean = Diagnosis {
        dofs: 1,
        free: ids(&["p0000002"]),
        ..Diagnosis::default()
    };
    let junk = Diagnosis {
        dofs: 1,
        free: ids(&["p0000002", "n0000001", "zzzzzzzz"]),
        conflicting: ids(&["p0000001", "qqqqqqqq"]),
        redundant: ids(&["n0000001"]),
        partially_redundant: ids(&["zzzzzzzz"]),
        points_read: BTreeMap::from([
            (id("qqqqqqqq"), ids(&["p0000001", "p0000002"])),
            (id("k0000003"), ids(&["n0000001", "zzzzzzzz"])),
        ]),
    };
    let ok = solution(1);
    let want = states(&sketch, &clean, Ok(&ok));
    assert_eq!(states(&sketch, &junk, Ok(&ok)), want);
    assert_eq!(
        want.points,
        members(&[("p0000001", Fully), ("p0000002", Under)])
    );
    assert_eq!(want.lines, members(&[("n0000001", Under)]));
    assert_eq!(want.state, SketchState::UnderDefined);
}

#[test]
fn a_solved_outcome_beside_a_conflicting_constraint_never_reads_fully_defined() {
    // The rules take the diagnosis and the outcome apart, so a caller can hand over a pair
    // that disagree: a solution, and a constraint listed conflicting. The decision's
    // invariants (fully_defined ⇒ nothing flagged; over_defined ⇐ something flagged) must
    // hold whatever the pair: a flagged constraint makes the sketch over defined. Found by
    // break-it, fixed: only a kept repetition counted, so this read fully_defined.
    let sketch = placed_line();
    let diagnosis = Diagnosis {
        dofs: 0,
        conflicting: ids(&["k0000003"]),
        points_read: BTreeMap::from([(id("k0000003"), ids(&["p0000001", "p0000002"]))]),
        ..Diagnosis::default()
    };
    let got = states(&sketch, &diagnosis, Ok(&solution(0)));
    assert_eq!(got.constraints[&id("k0000003")], Conflicting);
    assert_eq!(got.state, SketchState::OverDefined, "{got:#?}");
}

/// A sketch of every kind of member and many kinds of constraint: a line, a circle, an arc
/// (a quarter of radius 5 about p1), a free point; placed, horizontal, a radius, an `on`, a
/// tangency at the arc's end, an arc's ends' symmetry about the y axis, a reference.
fn mixed() -> Sketch {
    gated(&text(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 5.0, 0.0),
            point("p0000003", 0.0, 5.0),
            point("p0000004", 10.0, 5.0),
            point("p0000005", 3.0, 3.0),
        ],
        &[line("n0000001", "p0000003", "p0000004")],
        &[circle("c0000001", "p0000005", 2.0)],
        &[arc("a0000001", "p0000001", "p0000002", "p0000003")],
        &[
            c(
                r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            ),
            c(
                r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            ),
            c(r#"{"id": "k0000003", "type": "horizontal", "line": "n0000001"}"#),
            c(
                r#"{"id": "k0000004", "type": "radius", "curve": "c0000001", "value": 2, "driving": true}"#,
            ),
            c(r#"{"id": "k0000005", "type": "on", "point": "p0000005", "curve": "a0000001"}"#),
            c(
                r#"{"id": "k0000006", "type": "tangent_at", "a": "a0000001", "b": "n0000001", "at": "p0000003", "direction": "same"}"#,
            ),
            c(
                r#"{"id": "k0000007", "type": "symmetric", "a": "p0000002", "b": "p0000004", "about": "y_axis"}"#,
            ),
            c(
                r#"{"id": "k0000008", "type": "length", "line": "n0000001", "value": 10, "driving": false}"#,
            ),
        ],
    ))
}

#[test]
fn a_diagnosis_of_any_garbage_reply_is_data_or_a_structured_error_never_a_panic() {
    // Replies no worker would send and check_reply would refuse, handed to the diagnosis
    // straight: no values, too few, NaN, infinities, the largest doubles; every status;
    // free values out of range, fixed, repeated, unsorted; tags 0, unsent, past i32::MAX,
    // repeated. Each is a diagnosis (whose states then compute) or a protocol error.
    let sketch = mixed();
    let translation = Translation::new(&sketch).unwrap();
    let n = translation.system.values.len();
    let sent: Vec<Tag> = (1..=7).map(Tag).collect();
    let valueses = [
        Vec::new(),
        translation.system.values[..n - 1].to_vec(),
        vec![f64::NAN; n],
        vec![f64::INFINITY; n],
        vec![f64::MAX; n],
        vec![-0.0; n],
    ];
    let statuses = [
        SolveStatus::Success,
        SolveStatus::SolvedExceptRedundant,
        SolveStatus::SolvedInvalid,
        SolveStatus::Converged,
        SolveStatus::Failed,
    ];
    let frees = [
        vec![Value(u32::MAX), Value(0), Value(9), Value(9), Value(7)],
        translation.system.unknowns.clone(),
    ];
    let tag_lists = [
        Vec::new(),
        sent.clone(),
        vec![Tag(3), Tag(3), Tag(1)],
        vec![Tag(0)],
        vec![Tag(u32::MAX)],
        vec![Tag(i32::MAX as u32 + 1)],
    ];
    let mut cases = 0;
    for values in &valueses {
        for &status in &statuses {
            for free in &frees {
                for tags in &tag_lists {
                    for which in 0..3 {
                        let mut solved = Solved {
                            dofs: i32::MIN,
                            conflicting: Vec::new(),
                            redundant: Vec::new(),
                            partially_redundant: Vec::new(),
                            free: free.clone(),
                            no_equations: which == 2,
                            status,
                            algorithm: Some(Algorithm::DogLeg),
                            values: values.clone(),
                            residuals: Vec::new(),
                        };
                        match which {
                            0 => solved.conflicting = tags.clone(),
                            1 => solved.redundant = tags.clone(),
                            _ => solved.partially_redundant = tags.clone(),
                        }
                        cases += 1;
                        match translation.diagnosis(&solved) {
                            Ok(diagnosis) => {
                                let read = translation.read(&solved);
                                let got = states(&sketch, &diagnosis, read.as_ref());
                                assert_eq!(got.points.len(), 5);
                                assert_eq!(got.constraints.len(), 8);
                                assert_eq!(got.dofs, i32::MIN);
                            }
                            Err(e) => assert_eq!(e.code(), ErrorCode::ProtocolError, "{e}"),
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 6 * 5 * 2 * 6 * 3);
}

#[test]
fn ten_thousand_repeated_horizontals_are_diagnosed_and_stated_at_once() {
    // The gate's most constraints: one line given `horizontal` 10,000 times, the fake calling
    // every one after the first redundant. The diagnosis and the states take one pass each:
    // the first satisfied, the rest redundant, both ends and the line over defined.
    let mut constraints: Vec<String> = Vec::new();
    for i in 0..10_000 {
        constraints.push(format!(
            r#"{{"id": "k{i:07}", "type": "horizontal", "line": "n0000001"}}"#
        ));
    }
    let sketch = gated(&text(
        &[point("p0000001", 0.0, 0.0), point("p0000002", 10.0, 0.0)],
        &[line("n0000001", "p0000001", "p0000002")],
        &[],
        &[],
        &constraints,
    ));
    let translation = Translation::new(&sketch).unwrap();
    let solved = Solved {
        dofs: 3,
        redundant: (2..=10_000).map(Tag).collect(),
        free: translation.system.unknowns.clone(),
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(&translation.system)
    };
    let started = Instant::now();
    let diagnosis = translation.diagnosis(&solved).unwrap();
    let read = translation.read(&solved);
    let got = states(&sketch, &diagnosis, read.as_ref());
    let took = started.elapsed();
    assert!(took < Duration::from_secs(10), "{took:?}");
    assert!(read.is_ok(), "{read:?}");
    assert_eq!(got.state, SketchState::OverDefined);
    assert_eq!(got.constraints[&id("k0000000")], Satisfied);
    assert_eq!(
        got.constraints
            .values()
            .filter(|&&s| s == Redundant)
            .count(),
        9_999
    );
    assert_eq!(
        got.points,
        members(&[("p0000001", Over), ("p0000002", Over)])
    );
    assert_eq!(got.lines, members(&[("n0000001", Over)]));
}

#[test]
fn a_hundred_and_fifty_conflicting_constraints_are_all_named_and_all_conflicting() {
    // Past the hundred an error's contract lists: the error keeps every element, so the
    // over_constrained problem names exactly the conflicting set.
    let mut constraints = vec![c(
        r#"{"id": "j0000000", "type": "horizontal", "line": "n0000001"}"#,
    )];
    for i in 0..150 {
        constraints.push(format!(
            r#"{{"id": "k{i:07}", "type": "length", "line": "n0000001", "value": {}, "driving": true}}"#,
            10 + i
        ));
    }
    let sketch = gated(&text(
        &[point("p0000001", 0.0, 0.0), point("p0000002", 10.0, 0.0)],
        &[line("n0000001", "p0000001", "p0000002")],
        &[],
        &[],
        &constraints,
    ));
    let translation = Translation::new(&sketch).unwrap();
    // The horizontal is tag 1 (j sorts first), the lengths 2..=151.
    let solved = Solved {
        conflicting: (2..=151).map(Tag).collect(),
        ..FakeSketches::as_sent(&translation.system)
    };
    let diagnosis = translation.diagnosis(&solved).unwrap();
    let read = translation.read(&solved);
    let got = states(&sketch, &diagnosis, read.as_ref());
    let problem = got.solve_problem.as_ref().unwrap();
    assert_eq!(problem.code(), ErrorCode::OverConstrained);
    let named: BTreeSet<&str> = problem.elements().iter().map(String::as_str).collect();
    let conflicting: BTreeSet<&str> = got
        .constraints
        .iter()
        .filter(|(_, s)| **s == Conflicting)
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(conflicting.len(), 150);
    assert_eq!(named, conflicting);
    assert_eq!(got.constraints[&id("j0000000")], ConstraintState::Unsolved);
}

#[test]
fn the_states_dont_depend_on_the_order_the_sketch_holds_its_lists() {
    // A sketch read from a file not in ID order holds its lists as read; the states, by ID,
    // are the same.
    let sketch = mixed();
    let mut reversed = sketch.clone();
    reversed.points.reverse();
    reversed.lines.reverse();
    reversed.circles.reverse();
    reversed.arcs.reverse();
    reversed.constraints.reverse();
    let translation = Translation::new(&sketch).unwrap();
    let solved = Solved {
        redundant: vec![Tag(5)],
        partially_redundant: vec![Tag(7)],
        free: vec![Value(14)],
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(&translation.system)
    };
    let diagnosis = translation.diagnosis(&solved).unwrap();
    let again = Translation::new(&reversed)
        .unwrap()
        .diagnosis(&solved)
        .unwrap();
    assert_eq!(again, diagnosis);
    let ok = solution(1);
    assert_eq!(
        states(&reversed, &diagnosis, Ok(&ok)),
        states(&sketch, &diagnosis, Ok(&ok))
    );
}

#[test]
fn a_sketch_the_gate_never_saw_is_stated_without_a_panic() {
    // Curves naming points it doesn't have, IDs held twice, a constraint on nothing: the
    // states list each ID once, and nothing panics.
    let sketch = read_sketch(&text(
        &[point("p0000001", 0.0, 0.0), point("p0000001", 1.0, 0.0)],
        &[
            line("n0000001", "p0000001", "zzzzzzzz"),
            line("n0000001", "qqqqqqqq", "p0000001"),
        ],
        &[circle("c0000001", "yyyyyyyy", 1.0)],
        &[arc("a0000001", "xxxxxxxx", "p0000001", "wwwwwwww")],
        &[c(
            r#"{"id": "k0000001", "type": "horizontal", "line": "vvvvvvvv"}"#,
        )],
    ));
    let diagnosis = Diagnosis {
        conflicting: ids(&["k0000001"]),
        points_read: BTreeMap::from([(id("k0000001"), ids(&["p0000001"]))]),
        ..Diagnosis::default()
    };
    let refused = Error::new(ErrorCode::OverConstrained, "refused");
    let got: SketchStates = states(&sketch, &diagnosis, Err(&refused));
    assert_eq!(got.points, members(&[("p0000001", Over)]));
    assert_eq!(got.lines, members(&[("n0000001", Fully)]));
    assert_eq!(got.circles, members(&[("c0000001", Fully)]));
    assert_eq!(got.arcs, members(&[("a0000001", Fully)]));
    assert_eq!(got.state, SketchState::OverDefined);
}

/// `sketch` with its points moved by (dx, dy), and its circles' radii scaled by `scale`.
fn moved(sketch: &Sketch, dx: f64, dy: f64, scale: f64) -> Sketch {
    use silico_core::document::Coordinate;
    let mut out = sketch.clone();
    for p in &mut out.points {
        p.x = Coordinate::new(p.x.mm() + dx).unwrap();
        p.y = Coordinate::new(p.y.mm() + dy).unwrap();
    }
    for c in &mut out.circles {
        c.radius = Coordinate::new(c.radius.mm() * scale).unwrap();
    }
    out
}

#[test]
fn any_translation_of_a_sketch_sends_the_same_rows_and_reads_a_reply_alike() {
    // Translation::diagnosis reads a load's answer with the stored sketch's translation, on
    // the claim that rows are decided from the references alone: the same sketch drawn
    // elsewhere sends the same constraints, the same fixed values, and so diagnoses one
    // reply the same, decision 5's disagreeing included. A tangency at the arc's end (its
    // angle chosen from the references), the arc's ends' symmetry, and a redundant length
    // drawn off by a millimetre.
    let base = mixed();
    let mut with_length = base.clone();
    let extra = read_sketch(&text(
        &[],
        &[],
        &[],
        &[],
        &[c(
            r#"{"id": "k0000009", "type": "length", "line": "n0000001", "value": 11, "driving": true}"#,
        )],
    ));
    with_length.constraints.extend(extra.constraints);
    let here = Translation::new(&with_length).unwrap();
    let there = Translation::new(&moved(&with_length, 0.25, -0.5, 1.5)).unwrap();
    assert_eq!(here.system.constraints, there.system.constraints);
    assert_eq!(here.system.unknowns, there.system.unknowns);
    assert_eq!(here.system.geometry, there.system.geometry);
    let fixed = |t: &Translation| -> Vec<u64> {
        (0..t.system.values.len())
            .filter(|i| t.system.unknowns.binary_search(&Value(*i as u32)).is_err())
            .map(|i| t.system.values[i].to_bits())
            .collect()
    };
    assert_eq!(fixed(&here), fixed(&there));
    for redundant in [vec![Tag(3)], vec![Tag(8)], vec![Tag(3), Tag(8)]] {
        let reply = Solved {
            redundant,
            status: SolveStatus::SolvedExceptRedundant,
            ..FakeSketches::as_sent(&there.system)
        };
        assert_eq!(here.diagnosis(&reply), there.diagnosis(&reply));
    }
}

#[test]
fn a_flagged_tangency_at_an_arcs_end_governs_every_point_of_the_arc_and_the_line() {
    // As SketchSystem::reads gives it (the decision's 3): a tangency where an arc and a line
    // meet reads both curves whole, so flagged it over-defines the arc's centre and both its
    // ends, and both the line's ends, and names both curves. The free point and the circle
    // about it, read by nothing flagged, keep their freedom.
    let sketch = mixed();
    let translation = Translation::new(&sketch).unwrap();
    let solved = Solved {
        redundant: vec![Tag(6)],
        free: vec![Value(14), Value(15), Value(16)],
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(&translation.system)
    };
    let diagnosis = translation.diagnosis(&solved).unwrap();
    assert_eq!(
        diagnosis.points_read[&id("k0000006")],
        ids(&["p0000001", "p0000002", "p0000003", "p0000004"])
    );
    let got = states(&sketch, &diagnosis, Ok(&solution(3)));
    assert_eq!(
        got.points,
        members(&[
            ("p0000001", Over),
            ("p0000002", Over),
            ("p0000003", Over),
            ("p0000004", Over),
            ("p0000005", Under),
        ])
    );
    assert_eq!(got.lines, members(&[("n0000001", Over)]));
    assert_eq!(got.arcs, members(&[("a0000001", Over)]));
    assert_eq!(got.circles, members(&[("c0000001", Under)]));
}

#[test]
fn an_empty_translations_diagnosis_is_empty_and_its_states_fully_defined() {
    let sketch = gated(&text(&[], &[], &[], &[], &[]));
    let translation = Translation::new(&sketch).unwrap();
    let solved = Solved {
        dofs: -1,
        no_equations: true,
        ..FakeSketches::as_sent(&translation.system)
    };
    let diagnosis = translation.diagnosis(&solved).unwrap();
    assert_eq!(
        diagnosis,
        Diagnosis {
            dofs: -1,
            ..Diagnosis::default()
        }
    );
    let got = states(&sketch, &diagnosis, translation.read(&solved).as_ref());
    assert_eq!(
        (got.state, got.dofs, got.solve_problem),
        (SketchState::FullyDefined, 0, None)
    );
}
