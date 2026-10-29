//! break-it on the sketch read's definition states (`silico_core::sketch_states`,
//! `Translation::read_diagnosed`): replies that name what the sketch doesn't have, a reply
//! whose lists and status disagree, worker replies of garbage handed to the read without
//! `check_reply`, the largest sketches the gate takes, sketches the gate never saw, list
//! order, and the claim that any translation of a sketch reads a reply alike. Expected
//! values by hand from the rules (docs/ports/sketcher.md, *Definition states*, as M5.0's
//! open item 32 decided them).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::fixture;

use fixture::{arc, circle, line, point};

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::document::{ElementId, Operation, Sketch, read, validate};
use silico_core::fake::FakeSketches;
use silico_core::sketch::{Diagnosed, Diagnosis, Translation};
use silico_core::sketch_states::{ConstraintState, MemberState, SketchState, states};
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
fn a_reply_naming_values_the_sketch_doesnt_have_free_changes_nothing_it_does_have() {
    // Free values that aren't the sketch's unknowns (fixed, past the end, repeated, out of
    // order), handed to the read without check_reply: the states are the clean reply's,
    // member for member, and list only the sketch's own. p2's x is value 8.
    let sketch = placed_line();
    let translation = Translation::new(&sketch).unwrap();
    let reply = |free: Vec<Value>| Solved {
        dofs: 1,
        free,
        ..FakeSketches::as_sent(&translation.system)
    };
    let read = |solved: &Solved| states(&sketch, &translation.read_diagnosed(solved).unwrap());
    let want = read(&reply(vec![Value(8)])).unwrap();
    let junk = reply(vec![
        Value(u32::MAX),
        Value(8),
        Value(0),
        Value(8),
        Value(1_000),
        Value(2),
    ]);
    assert_eq!(read(&junk).unwrap(), want);
    assert_eq!(
        want.points,
        members(&[("p0000001", Fully), ("p0000002", Under)])
    );
    assert_eq!(want.lines, members(&[("n0000001", Under)]));
    assert_eq!(want.state, SketchState::UnderDefined);
    // Flags naming tags the sketch doesn't have are a reply the core can't read: no states.
    for solved in [
        Solved {
            conflicting: vec![Tag(9)],
            ..reply(Vec::new())
        },
        Solved {
            partially_redundant: vec![Tag(9)],
            status: SolveStatus::SolvedExceptRedundant,
            ..reply(Vec::new())
        },
    ] {
        let e = translation.read_diagnosed(&solved).unwrap_err();
        assert_eq!(e.code(), ErrorCode::ProtocolError, "{e}");
    }
}

#[test]
fn a_reply_listing_a_conflict_beside_a_solution_reads_over_defined_never_fully_defined() {
    // A reply that says it solved and lists a conflict: the read refuses the conflict first,
    // so the pair the states read is a refusal, not a solution beside a conflict (break-it
    // found that pair reading fully_defined when the states took the two apart; now they
    // can't be, and a flagged constraint on a solved sketch reads over defined besides).
    let sketch = placed_line();
    let translation = Translation::new(&sketch).unwrap();
    let solved = Solved {
        dofs: 0,
        conflicting: vec![Tag(3)],
        status: SolveStatus::Success,
        ..FakeSketches::as_sent(&translation.system)
    };
    let read = translation.read_diagnosed(&solved).unwrap();
    assert_eq!(
        read.outcome().unwrap_err().code(),
        ErrorCode::OverConstrained
    );
    let got = states(&sketch, &read).unwrap();
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
                        match translation
                            .read_diagnosed(&solved)
                            .and_then(|read| states(&sketch, &read))
                        {
                            Ok(got) => {
                                assert_eq!(got.points.len(), 5);
                                assert_eq!(got.constraints.len(), 8);
                                assert_eq!(got.dofs, i32::MIN);
                            }
                            Err(e) => assert!(
                                matches!(e.code(), ErrorCode::ProtocolError | ErrorCode::Internal),
                                "{e}"
                            ),
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
    let read = translation.read_diagnosed(&solved).unwrap();
    let got = states(&sketch, &read).unwrap();
    let took = started.elapsed();
    assert!(took < Duration::from_secs(10), "{took:?}");
    assert!(read.outcome().is_ok(), "{:?}", read.outcome());
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
    let got = states(&sketch, &translation.read_diagnosed(&solved).unwrap()).unwrap();
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
    let read = translation.read_diagnosed(&solved).unwrap();
    let again = Translation::new(&reversed)
        .unwrap()
        .read_diagnosed(&solved)
        .unwrap();
    assert_eq!(again.diagnosis(), read.diagnosis());
    assert_eq!(states(&reversed, &read), states(&sketch, &read));
}

#[test]
fn a_sketch_the_gate_never_saw_is_stated_without_a_panic() {
    // Curves naming points it doesn't have, IDs held twice, a constraint on nothing: the
    // translation calls it the gate's bug (internal), which is no sketch state, so it has
    // none; its diagnosis before its constraints counts every member once by ID, and nothing
    // panics.
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
    let refused = Translation::new(&sketch).unwrap_err();
    // One point, one circle, one arc, each once: 2 + 1 + 3, less the arc's 4 rules.
    assert_eq!(
        Diagnosis::untranslated(&sketch),
        Diagnosis {
            dofs: 2,
            free: ids(&["a0000001", "c0000001", "p0000001"]),
            ..Diagnosis::default()
        }
    );
    assert_eq!(refused.code(), ErrorCode::Internal, "{refused}");
    let read = Diagnosed::untranslated(&sketch).unwrap();
    assert_eq!(read.outcome(), Err(&refused));
    assert_eq!(read.diagnosis(), &Diagnosis::untranslated(&sketch));
    assert_eq!(states(&sketch, &read), Err(refused));
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

/// [`mixed`] with a driving length of `value` on its line (k0000009, tag 9; the reference
/// length k0000008, tag 8, is never sent).
fn with_length(value: &str) -> Sketch {
    let mut sketch = mixed();
    let extra = read_sketch(&text(
        &[],
        &[],
        &[],
        &[],
        &[format!(
            r#"{{"id": "k0000009", "type": "length", "line": "n0000001", "value": {value}, "driving": true}}"#
        )],
    ));
    sketch.constraints.extend(extra.constraints);
    sketch
}

#[test]
fn any_translation_of_a_sketch_sends_the_same_rows_and_reads_a_reply_alike() {
    // Translation::read_diagnosed reads a load's answer with the stored sketch's
    // translation, on the claim that rows are decided from the references alone: the same
    // sketch drawn elsewhere sends the same constraints, the same fixed values, and so
    // diagnoses one reply the same, decision 5's disagreeing included. A tangency at the
    // arc's end (its angle chosen from the references), the arc's ends' symmetry, and a
    // redundant length drawn off by a millimetre (tag 9), past any window.
    let sketch = with_length("11");
    let here = Translation::new(&sketch).unwrap();
    let there = Translation::new(&moved(&sketch, 0.25, -0.5, 1.5)).unwrap();
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
    let reply = |redundant: Vec<Tag>| Solved {
        redundant,
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(&there.system)
    };
    // The horizontal (tag 3) agrees, kept; the length (tag 9) disagrees, conflicting.
    for (redundant, conflicting) in [
        (vec![Tag(3)], ids(&[])),
        (vec![Tag(9)], ids(&["k0000009"])),
        (vec![Tag(3), Tag(9)], ids(&["k0000009"])),
    ] {
        let reply = reply(redundant);
        let (a, b) = (
            here.read_diagnosed(&reply).unwrap(),
            there.read_diagnosed(&reply).unwrap(),
        );
        assert_eq!(a.diagnosis(), b.diagnosis());
        assert_eq!(a.diagnosis().conflicting, conflicting);
    }
    // The reference's tag, never sent, listed: a reply the core can't read, from either.
    for t in [&here, &there] {
        let e = t.read_diagnosed(&reply(vec![Tag(8)])).unwrap_err();
        assert_eq!(e.code(), ErrorCode::ProtocolError, "{e}");
    }
}

#[test]
fn a_reply_is_read_on_its_own_coordinates_however_its_sketch_was_drawn() {
    // Decision 5's window is the extent of the reply's coordinates, not the drawing's: the
    // drawing (moved, its line 10 long, the sketch about 11.2 mm across, a window of
    // 1.1e-8 mm) given a reply with every point a thousand times as far from the origin
    // (its line 10,000 long, the sketch 11,180 mm across, a window of 1.1e-5 mm), and a
    // repeated length of 10,000.000005: a gap of 5e-6 mm, within the reply's window and
    // past the drawing's. Kept, from either translation.
    let sketch = with_length("10000.000005");
    let here = Translation::new(&sketch).unwrap();
    let there = Translation::new(&moved(&sketch, 0.25, -0.5, 1.5)).unwrap();
    let mut solved = Solved {
        redundant: vec![Tag(9)],
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(&there.system)
    };
    for g in &there.system.geometry {
        if let silico_protocol::SketchGeometry::Point(p) = g {
            for v in [p.x, p.y] {
                solved.values[v.0 as usize] *= 1000.0;
            }
        }
    }
    for t in [&here, &there] {
        let read = t.read_diagnosed(&solved).unwrap();
        let solution = read.outcome().unwrap();
        assert!(
            solution.redundant.contains_key(&id("k0000009")),
            "{solution:?}"
        );
        assert!(read.diagnosis().conflicting.is_empty());
        assert_eq!(read.diagnosis().redundant, ids(&["k0000009"]));
    }
    // A repetition 5e-5 mm off, past the reply's window too: conflicting, from either.
    let sketch = with_length("10000.00005");
    let here = Translation::new(&sketch).unwrap();
    let there = Translation::new(&moved(&sketch, 0.25, -0.5, 1.5)).unwrap();
    let mut far = solved.clone();
    let k9 = far.values.iter().position(|&v| v == 10000.000005).unwrap();
    far.values[k9] = 10000.00005;
    for t in [&here, &there] {
        let read = t.read_diagnosed(&far).unwrap();
        assert_eq!(read.diagnosis().conflicting, ids(&["k0000009"]));
    }
}

#[test]
fn a_flagged_tangency_at_an_arcs_end_governs_what_its_equation_depends_on_not_the_arcs_far_end() {
    // A tangency where the arc and the line meet at p3 (k0000006, tag 6): its equation is the
    // angle between the curves' normals there (1.1.3's CalculateNormal: a line's both ends,
    // an arc's centre), so flagged it over-defines p3, the line's ends (p3, p4), and the
    // arc's centre (p1), and names both curves; the arc's far end, p2, which the equation
    // never has, keeps its freedom: fixed in this reply (only p5's values, 14 and 15, and
    // the circle's radius, 16, are free), so fully defined (item 32's addendum, B;
    // `SketchSystem::reads`, the arc whole, would have marked it). As drawn the tangency
    // isn't met (the arc arrives at p3 along −x, the line leaves along +x), so the read
    // refuses it as disagreeing: conflicting, the rest unsolved. The free point and the
    // circle about it, read by nothing flagged, keep their freedom.
    let sketch = mixed();
    let translation = Translation::new(&sketch).unwrap();
    let solved = Solved {
        redundant: vec![Tag(6)],
        free: vec![Value(14), Value(15), Value(16)],
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(&translation.system)
    };
    let read = translation.read_diagnosed(&solved).unwrap();
    assert_eq!(
        read.diagnosis().points_depended_on[&id("k0000006")],
        ids(&["p0000001", "p0000003", "p0000004"])
    );
    let got = states(&sketch, &read).unwrap();
    assert_eq!(
        got.points,
        members(&[
            ("p0000001", Over),
            ("p0000002", Fully),
            ("p0000003", Over),
            ("p0000004", Over),
            ("p0000005", Under),
        ])
    );
    assert_eq!(got.constraints[&id("k0000006")], Conflicting);
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
    let read = translation.read_diagnosed(&solved).unwrap();
    assert_eq!(
        read.diagnosis(),
        &Diagnosis {
            dofs: -1,
            ..Diagnosis::default()
        }
    );
    let got = states(&sketch, &read).unwrap();
    assert_eq!(
        (got.state, got.dofs, got.solve_problem),
        (SketchState::FullyDefined, 0, None)
    );
}
