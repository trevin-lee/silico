//! break-it on the sketch read's definition states (`silico_core::sketch_states`,
//! `Translation::read_diagnosed`): replies that name what the sketch doesn't have, a reply
//! whose lists and status disagree, worker replies of garbage handed to the read without
//! `check_reply`, the largest sketches the gate takes, sketches the gate never saw, list
//! order, and the claim that any translation of a sketch reads a reply alike. Expected
//! values by hand from the rules (docs/ports/sketcher.md, *Definition states*, as M5.0's
//! open item 32 decided them).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::fixture;
use super::within;

use fixture::{arc, circle, line, point};
use within::within;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::document::{ElementId, Operation, Sketch, read, validate};
use silico_core::fake::FakeSketches;
use silico_core::sketch::{
    DEPENDENCIES_LISTED, DEPENDENCY_IDS_LISTED, Dependencies, Dependency, Diagnosed, Diagnosis,
    Translation,
};
use silico_core::sketch_states::{ConstraintState, MemberState, SketchState, SketchStates, states};
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

/// The gate's most constraints: one line given `horizontal` 10,000 times, `k0000000` to
/// `k0009999`, tags 1 to 10,000.
fn ten_thousand_horizontals() -> Sketch {
    let mut constraints: Vec<String> = Vec::new();
    for i in 0..10_000 {
        constraints.push(format!(
            r#"{{"id": "k{i:07}", "type": "horizontal", "line": "n0000001"}}"#
        ));
    }
    gated(&text(
        &[point("p0000001", 0.0, 0.0), point("p0000002", 10.0, 0.0)],
        &[line("n0000001", "p0000001", "p0000002")],
        &[],
        &[],
        &constraints,
    ))
}

/// What [`ten_thousand_horizontals`] reads, the fake calling every horizontal after the first
/// redundant, every unknown free, 3 left: the horizontals are relations, which a dependency
/// doesn't over-define (addendum 4), so the first reads satisfied, the rest redundant as
/// listed, and the members and the sketch under defined.
#[track_caller]
fn expect_horizontals_read(read: &Diagnosed, got: &SketchStates) {
    assert!(read.outcome().is_ok(), "{:?}", read.outcome());
    assert_eq!((got.state, got.dofs), (SketchState::UnderDefined, 3));
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
        members(&[("p0000001", Under), ("p0000002", Under)])
    );
    assert_eq!(got.lines, members(&[("n0000001", Under)]));
}

#[test]
fn a_reply_naming_a_million_tags_in_its_dependencies_is_checked_read_and_stated_promptly() {
    // The reply's groups, the tags in each, and the first tag of its first group.
    const GROUPS: u32 = 1_000;
    const GROUP_TAGS: u32 = 1_001;
    const FIRST_TAG: u32 = 8_001;
    // Robustness review, round 1: the read found each group's tags among the rows sent by a
    // scan, for every tag, so a reply PlaneGCS's bounds allow, 9,000 groups of 1,001 tags
    // on the gate's 10,000 constraints, took 12 minutes in the core, after the worker's
    // timeout. Here the ten thousand horizontals and 1,000 groups of 1,001 tags, group i the
    // tags from 8,001 + i (each sorted, the groups increasing, every tag sent: a reply
    // check_reply takes), 1,001,000 tags, each near the end of the rows a scan walks; the
    // fake calls every horizontal after the first redundant. Checked, read, and stated in
    // 10 s of CPU (about 1.5 s in a debug build; the scan took more than 10). A smaller
    // reply than the review's, whose 9,009,000 tags take 13 s of CPU in a debug build even
    // looked up at once, the work of reading them.
    //
    // Round 2: the read made every tag an ID, a million Strings here, about a gigabyte for
    // the review's reply. It lists the first 100 groups' first 100 IDs and counts the rest
    // as tags, so it holds 100 × 100 IDs of the groups, and one for each constraint in any
    // (2,000 here), however many tags the reply names: the cut sizes below are all it keeps.
    // By hand: tag t is k{t − 1}, so group i names k{8,000 + i} to k{9,000 + i}; the
    // first 100 listed, k0008000 to k0008099 the first, k0008099 to k0008198 the 100th,
    // each of 1,001; 1,000 groups; every tag from 8,001 to 10,000 in some group, so k0008000
    // to k0009999, 2,000 IDs, dependent. The states are the horizontals' own.
    let sketch = ten_thousand_horizontals();
    let translation = Translation::new(&sketch).unwrap();
    let solved = Solved {
        dofs: 3,
        redundant: (2..=10_000).map(Tag).collect(),
        dependencies: (0..GROUPS)
            .map(|i| {
                (FIRST_TAG + i..FIRST_TAG + i + GROUP_TAGS)
                    .map(Tag)
                    .collect()
            })
            .collect(),
        free: translation.system.unknowns.clone(),
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(&translation.system)
    };
    let (read, got) = within(Duration::from_secs(10), move || {
        translation.check_reply(&solved).unwrap();
        let read = translation.read_diagnosed(&solved).unwrap();
        let got = states(&sketch, &read).unwrap();
        (read, got)
    });
    let named = |from: u32| -> Vec<ElementId> {
        (from..from + 100)
            .map(|i| id(&format!("k{i:07}")))
            .collect()
    };
    let dependencies = &read.diagnosis().dependencies;
    assert_eq!((DEPENDENCIES_LISTED, DEPENDENCY_IDS_LISTED), (100, 100));
    assert_eq!(dependencies.groups.len(), 100);
    assert_eq!(dependencies.groups_total, 1_000);
    assert!(
        dependencies
            .groups
            .iter()
            .all(|g| g.constraints.len() == 100 && g.constraints_total == 1_001),
        "{:?}",
        dependencies
            .groups
            .iter()
            .map(|g| (g.constraints.len(), g.constraints_total))
            .collect::<Vec<_>>()
    );
    assert_eq!(dependencies.groups[0].constraints, named(8_000));
    assert_eq!(dependencies.groups[99].constraints, named(8_099));
    let dependent: BTreeSet<ElementId> = (8_000..10_000).map(|i| id(&format!("k{i:07}"))).collect();
    assert_eq!(dependencies.dependent, dependent);
    assert_eq!(&got.dependencies, dependencies);
    expect_horizontals_read(&read, &got);
}

/// One line p1 (0, 0)–p2 (10, 0) given `horizontal` `n` times, `k0000000` on (tags 1 to
/// `n`), and then a driving `distance_y` of 0 from p1 to p2, `k{n}` (tag `n` + 1): its one row
/// is p2's y less p1's, as each horizontal's is.
fn horizontals_and_a_height(n: u32) -> Sketch {
    let mut constraints: Vec<String> = (0..n)
        .map(|i| format!(r#"{{"id": "k{i:07}", "type": "horizontal", "line": "n0000001"}}"#))
        .collect();
    constraints.push(format!(
        r#"{{"id": "k{n:07}", "type": "distance_y", "a": "p0000001", "b": "p0000002", "value": 0, "driving": true}}"#
    ));
    gated(&text(
        &[point("p0000001", 0.0, 0.0), point("p0000002", 10.0, 0.0)],
        &[line("n0000001", "p0000001", "p0000002")],
        &[],
        &[],
        &constraints,
    ))
}

/// `horizontals_and_a_height(n)` read from a reply with `groups`, PlaneGCS dropping every
/// horizontal and keeping the height, every unknown free and 3 left.
fn read_height_beside(n: u32, groups: Vec<Vec<Tag>>) -> (Diagnosed, SketchStates) {
    let sketch = horizontals_and_a_height(n);
    let translation = Translation::new(&sketch).unwrap();
    let solved = Solved {
        dofs: 3,
        redundant: (1..=n).map(Tag).collect(),
        dependencies: groups,
        free: translation.system.unknowns.clone(),
        status: SolveStatus::SolvedExceptRedundant,
        ..FakeSketches::as_sent(&translation.system)
    };
    translation.check_reply(&solved).unwrap();
    let read = translation.read_diagnosed(&solved).unwrap();
    let got = states(&sketch, &read).unwrap();
    (read, got)
}

/// `k{from}` to `k{to − 1}`.
fn k_range(from: u32, to: u32) -> Vec<ElementId> {
    (from..to).map(|i| id(&format!("k{i:07}"))).collect()
}

/// What a solved sketch with the height `k0000200` over-defining reads (addendum 4): the
/// height redundant, though PlaneGCS kept it; the horizontals redundant as dropped; p1 and
/// p2, which its row reads, over defined; the line, which it doesn't name, under defined
/// by its free ends; the sketch over defined.
#[track_caller]
fn expect_height_over_defines(read: &Diagnosed, got: &SketchStates) {
    assert!(read.outcome().is_ok(), "{:?}", read.outcome());
    assert!(!read.diagnosis().redundant.contains(&id("k0000200")));
    assert_eq!(got.constraints[&id("k0000200")], Redundant);
    assert_eq!((got.state, got.dofs), (SketchState::OverDefined, 3));
    assert_eq!(
        got.points,
        members(&[("p0000001", Over), ("p0000002", Over)])
    );
    assert_eq!(got.lines, members(&[("n0000001", Under)]));
    assert_eq!(
        read.diagnosis().points_depended_on.get(&id("k0000200")),
        Some(&ids(&["p0000001", "p0000002"]))
    );
}

#[test]
fn a_group_of_more_than_a_hundred_lists_its_first_hundred_counts_them_all_and_its_201st_over_defines()
 {
    // Invariant 9's cut, on a group: 200 horizontals and the height k0000200 (tag 201), all
    // one row, so one dependency of all 201 (tags 1 to 201), PlaneGCS dropping the 200
    // horizontals and keeping the height. By hand: the group lists its first 100 IDs in ID
    // order, k0000000 to k0000099, and counts 201; one group; all 201 dependent. The height
    // is 201st in its group, past the cut, and still over-defines: a driving dimension in a
    // dependency (addendum 4), wherever in it.
    let (read, got) = read_height_beside(200, vec![(1..=201).map(Tag).collect()]);
    let want = Dependencies {
        groups: vec![Dependency {
            constraints: k_range(0, 100),
            constraints_total: 201,
        }],
        groups_total: 1,
        dependent: k_range(0, 201).into_iter().collect(),
    };
    assert_eq!(read.diagnosis().dependencies, want);
    assert_eq!(got.dependencies, want);
    assert!(
        !want.groups[0].constraints.contains(&id("k0000200")),
        "the height is past the cut"
    );
    expect_height_over_defines(&read, &got);
    // Every horizontal is a relation, dropped: redundant, over-defining nothing.
    assert!(
        k_range(0, 200)
            .iter()
            .all(|k| got.constraints[k] == Redundant),
        "{got:#?}"
    );
}

#[test]
fn more_than_a_hundred_groups_list_the_first_hundred_count_them_all_and_one_in_the_150th_over_defines()
 {
    // Invariant 9's cut, on the groups: 150 groups, group i (i from 0 to 148) the
    // horizontals k{i} and k{i + 1} (tags i + 1 and i + 2), and the 150th the horizontal
    // k0000149 and the height k0000200 (tags 150 and 201), in increasing order as the
    // solver gives them; tags 151 to 200 in none. By hand: the first 100 listed, group i
    // [k{i}, k{i + 1}], each of 2; 150 groups; dependent k0000000 to k0000149 and the height.
    // The height is only in the 150th group, past the cut, and still over-defines.
    let mut groups: Vec<Vec<Tag>> = (0..149).map(|i| vec![Tag(i + 1), Tag(i + 2)]).collect();
    groups.push(vec![Tag(150), Tag(201)]);
    let (read, got) = read_height_beside(200, groups);
    let dependencies = &read.diagnosis().dependencies;
    assert_eq!(dependencies.groups.len(), 100);
    assert_eq!(dependencies.groups_total, 150);
    for (i, g) in (0u32..).zip(&dependencies.groups) {
        assert_eq!(g.constraints, k_range(i, i + 2), "group {i}");
        assert_eq!(g.constraints_total, 2, "group {i}");
    }
    let mut dependent: BTreeSet<ElementId> = k_range(0, 150).into_iter().collect();
    dependent.insert(id("k0000200"));
    assert_eq!(dependencies.dependent, dependent);
    assert_eq!(&got.dependencies, dependencies);
    expect_height_over_defines(&read, &got);
}

#[test]
fn two_groups_alike_in_their_first_hundred_are_both_listed_each_with_its_total() {
    // A chain closed twice: two dependencies that share their first 100 constraints and
    // differ past them, tags 1 to 100 and 101, and tags 1 to 100 and 102 (k0000000 to
    // k0000099, then k0000100 or k0000101), on 150 horizontals and the height. A set of the
    // cut groups would keep one; the read lists both, alike, each of 101; 2 groups.
    let first: Vec<Tag> = (1..=101).map(Tag).collect();
    let mut second: Vec<Tag> = (1..=100).map(Tag).collect();
    second.push(Tag(102));
    let (read, _) = read_height_beside(150, vec![first, second]);
    let alike = Dependency {
        constraints: k_range(0, 100),
        constraints_total: 101,
    };
    assert_eq!(
        read.diagnosis().dependencies,
        Dependencies {
            groups: vec![alike.clone(), alike],
            groups_total: 2,
            dependent: k_range(0, 102).into_iter().collect(),
        }
    );
}

#[test]
fn the_groups_are_listed_in_the_solvers_order_each_in_id_order_cut_or_not() {
    // The reply's groups, in increasing order as the solver gives them, each before any
    // longer one it begins: [1, 2], [1, 2, 3], [1 to 100, 102], [1 to 100, 103] (alike once
    // cut), [5], [151] (the height, k0000150), on 150 horizontals. By hand, tag t is
    // k{t − 1}: [k0000000, k0000001] of 2, [k0000000, k0000001, k0000002] of 3, the cut
    // groups, each k0000000 to k0000099 of 101, [k0000004] of 1, [k0000150] of 1, in that
    // order; 6 groups; dependent k0000000 to k0000099, k0000101, k0000102, and k0000150.
    let cut = |last: u32| -> Vec<Tag> { (1..=100).chain([last]).map(Tag).collect() };
    let groups = vec![
        vec![Tag(1), Tag(2)],
        vec![Tag(1), Tag(2), Tag(3)],
        cut(102),
        cut(103),
        vec![Tag(5)],
        vec![Tag(151)],
    ];
    let (read, _) = read_height_beside(150, groups);
    let group = |constraints: Vec<ElementId>, constraints_total: u64| Dependency {
        constraints,
        constraints_total,
    };
    let mut dependent: BTreeSet<ElementId> = k_range(0, 100).into_iter().collect();
    dependent.extend(ids(&["k0000101", "k0000102", "k0000150"]));
    assert_eq!(
        read.diagnosis().dependencies,
        Dependencies {
            groups: vec![
                group(k_range(0, 2), 2),
                group(k_range(0, 3), 3),
                group(k_range(0, 100), 101),
                group(k_range(0, 100), 101),
                group(vec![id("k0000004")], 1),
                group(vec![id("k0000150")], 1),
            ],
            groups_total: 6,
            dependent,
        }
    );
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
    // can't be, and a conflicting constraint on a solved sketch over-defines besides).
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
                            dependencies: Vec::new(),
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
    // the first satisfied, the rest redundant. A redundant relation over-defines nothing
    // (addendum 2), so the members and the sketch read by the freedom the reply gives: every
    // unknown free, 3 left, both ends, the line, and the sketch under defined.
    let sketch = ten_thousand_horizontals();
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
    expect_horizontals_read(&read, &got);
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
    // Its dependencies hold the flagged tags, each beside another (k0000005 with
    // k0000001, k0000007 with k0000003), so the groups are read by ID too.
    let solved = Solved {
        redundant: vec![Tag(5)],
        partially_redundant: vec![Tag(7)],
        dependencies: vec![vec![Tag(1), Tag(5)], vec![Tag(3), Tag(7)]],
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
    assert_eq!(
        read.diagnosis().dependencies,
        Dependencies {
            groups: vec![
                Dependency {
                    constraints: vec![id("k0000001"), id("k0000005")],
                    constraints_total: 2,
                },
                Dependency {
                    constraints: vec![id("k0000003"), id("k0000007")],
                    constraints_total: 2,
                },
            ],
            groups_total: 2,
            dependent: ids(&["k0000001", "k0000003", "k0000005", "k0000007"]),
        }
    );
    assert_eq!(states(&reversed, &read), states(&sketch, &read));
}

#[test]
fn a_sketch_the_gate_never_saw_is_stated_without_a_panic() {
    // Curves naming points it doesn't have, IDs held twice, a constraint on nothing: the
    // translation calls it the gate's bug (internal), which is no sketch state. Its geometry
    // can't be sent alone either, for the same reason, so the read of its refused
    // translation is that error: no states, nothing sent, and nothing panics.
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
    assert_eq!(refused.code(), ErrorCode::Internal, "{refused}");
    let fake = FakeSketches::new();
    assert_eq!(
        Diagnosis::untranslated(&sketch, &mut fake.clone()).as_ref(),
        Err(&refused)
    );
    assert_eq!(
        Diagnosed::untranslated(&sketch, &mut fake.clone()),
        Err(refused)
    );
    assert!(fake.requests().is_empty());
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

// --- Addendum 2: a redundant relation doesn't over-define, a redundant dimension does ------

/// Every form of constraint, each met exactly on [`every_form`]'s drawing, and whether it's
/// a dimension: the list in docs/ports/sketcher.md (*Definition states*, *An over-defining
/// constraint*), by hand, not `Form::is_dimension`.
const EVERY_FORM: [(&str, bool); 21] = [
    (r#""type": "horizontal", "line": "n0000001""#, false),
    (r#""type": "vertical", "line": "n0000003""#, false),
    (
        r#""type": "horizontal_points", "a": "p0000001", "b": "p0000002""#,
        false,
    ),
    (
        r#""type": "vertical_points", "a": "p0000001", "b": "p0000003""#,
        false,
    ),
    (
        r#""type": "length", "line": "n0000001", "value": 10, "driving": true"#,
        true,
    ),
    (
        r#""type": "distance", "a": "p0000001", "b": "p0000002", "value": 10, "driving": true"#,
        true,
    ),
    (
        r#""type": "distance_x", "a": "origin", "b": "p0000002", "value": 10, "driving": true"#,
        true,
    ),
    (
        r#""type": "distance_y", "a": "origin", "b": "p0000003", "value": 5, "driving": true"#,
        true,
    ),
    (
        r#""type": "point_line_distance", "point": "p0000003", "line": "n0000001", "value": 5, "side": "left", "driving": true"#,
        true,
    ),
    (
        r#""type": "on", "point": "p0000013", "curve": "n0000001""#,
        false,
    ),
    (
        r#""type": "parallel", "a": "n0000001", "b": "n0000002""#,
        false,
    ),
    (
        r#""type": "perpendicular", "a": "n0000001", "b": "n0000003""#,
        false,
    ),
    (
        r#""type": "angle", "a": "n0000001", "b": "n0000003", "value": 90, "driving": true"#,
        true,
    ),
    (
        r#""type": "radius", "curve": "c0000001", "value": 5, "driving": true"#,
        true,
    ),
    (
        r#""type": "diameter", "curve": "c0000001", "value": 10, "driving": true"#,
        true,
    ),
    (
        r#""type": "equal", "a": "n0000001", "b": "n0000002""#,
        false,
    ),
    (
        r#""type": "tangent", "a": "n0000002", "b": "c0000001", "side": "right""#,
        false,
    ),
    (
        r#""type": "tangent_on", "a": "n0000004", "at": "p0000006", "b": "c0000001", "direction": "opposite""#,
        false,
    ),
    (
        r#""type": "tangent_at", "a": "n0000005", "b": "a0000001", "at": "p0000009", "direction": "same""#,
        false,
    ),
    (
        r#""type": "symmetric", "a": "p0000001", "b": "p0000002", "about": "p0000013""#,
        false,
    ),
    (
        r#""type": "midpoint", "point": "p0000013", "line": "n0000001""#,
        false,
    ),
];

/// A drawing every form of [`EVERY_FORM`] is met on exactly: the bottom n1 (0, 0)–(10, 0),
/// the top n2 (0, 5)–(10, 5), the left n3 (0, 0)–(0, 5); a circle c1 of radius 5 about
/// (20, 0), its top (20, 5) the start of n4 to (30, 5); a quarter arc a1 about (40, 0) from
/// (45, 0) to (40, 5), where n5 leaves along −x to (32, 5); and p13, n1's middle. With the
/// forms `picked` by index, k0000001 the first picked (so tag 1), and so on.
fn every_form(picked: &[usize]) -> Sketch {
    let constraints: Vec<String> = picked
        .iter()
        .enumerate()
        .map(|(k, &i)| format!(r#"{{"id": "k{:07}", {}}}"#, k + 1, EVERY_FORM[i].0))
        .collect();
    gated(&text(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 10.0, 0.0),
            point("p0000003", 0.0, 5.0),
            point("p0000004", 10.0, 5.0),
            point("p0000005", 20.0, 0.0),
            point("p0000006", 20.0, 5.0),
            point("p0000007", 40.0, 0.0),
            point("p0000008", 45.0, 0.0),
            point("p0000009", 40.0, 5.0),
            point("p0000011", 30.0, 5.0),
            point("p0000012", 32.0, 5.0),
            point("p0000013", 5.0, 0.0),
        ],
        &[
            line("n0000001", "p0000001", "p0000002"),
            line("n0000002", "p0000003", "p0000004"),
            line("n0000003", "p0000001", "p0000003"),
            line("n0000004", "p0000006", "p0000011"),
            line("n0000005", "p0000009", "p0000012"),
        ],
        &[circle("c0000001", "p0000005", 5.0)],
        &[arc("a0000001", "p0000007", "p0000008", "p0000009")],
        &constraints,
    ))
}

#[test]
fn every_form_alone_in_a_dependency_over_defines_exactly_when_it_carries_a_value() {
    // Each form alone on the drawing, which meets it exactly, in a dependency of its own (tag
    // 1), the fake listing it redundant, then partially redundant, then not at all (kept),
    // with nothing free and 0 left: solved, so a relation (13 forms) leaves the sketch fully
    // defined, every member fully defined, and a dimension (8) makes it over defined, with
    // some member over defined (each reads a point or names a curve), and reads redundant
    // even where the solver kept it or listed it partially redundant (addendum 4: a driving
    // dimension in a dependency is redundant), where a relation listed partially redundant
    // reads so. Listed conflicting, any form is refused, over defined, and over-defines what
    // it governs.
    for (i, &(form, dimension)) in EVERY_FORM.iter().enumerate() {
        let sketch = every_form(&[i]);
        let translation = Translation::new(&sketch).unwrap();
        for (redundant, partially_redundant, conflicting) in [
            (vec![Tag(1)], vec![], vec![]),
            (vec![], vec![Tag(1)], vec![]),
            (vec![], vec![], vec![]),
            (vec![], vec![], vec![Tag(1)]),
        ] {
            let listed_conflicting = !conflicting.is_empty();
            let listed_partially = !partially_redundant.is_empty();
            let kept =
                redundant.is_empty() && partially_redundant.is_empty() && !listed_conflicting;
            let solved = Solved {
                dofs: 0,
                free: Vec::new(),
                redundant,
                partially_redundant,
                dependencies: vec![vec![Tag(1)]],
                conflicting,
                status: if kept {
                    SolveStatus::Success
                } else {
                    SolveStatus::SolvedExceptRedundant
                },
                ..FakeSketches::as_sent(&translation.system)
            };
            let read = translation.read_diagnosed(&solved).unwrap();
            // The drawing meets the form, so only a listed conflict refuses the solve.
            assert_eq!(
                read.outcome().is_err(),
                listed_conflicting,
                "{form}: {:?}",
                read.outcome()
            );
            let got = states(&sketch, &read).unwrap();
            let members: Vec<MemberState> = got
                .points
                .values()
                .chain(got.lines.values())
                .chain(got.circles.values())
                .chain(got.arcs.values())
                .copied()
                .collect();
            let over = listed_conflicting || dimension;
            let want = if over {
                SketchState::OverDefined
            } else {
                SketchState::FullyDefined
            };
            assert_eq!(got.state, want, "{form} {solved:?}");
            assert_eq!(members.contains(&Over), over, "{form}: {got:#?}");
            if !over {
                assert!(members.iter().all(|&s| s == Fully), "{form}: {got:#?}");
            }
            if kept {
                let state = if dimension {
                    Redundant
                } else {
                    ConstraintState::Satisfied
                };
                assert_eq!(
                    got.constraints.values().collect::<Vec<_>>(),
                    [&state],
                    "{form}"
                );
            }
            if listed_partially {
                let state = if dimension {
                    Redundant
                } else {
                    ConstraintState::PartiallyRedundant
                };
                assert_eq!(
                    got.constraints.values().collect::<Vec<_>>(),
                    [&state],
                    "{form}"
                );
            }
        }
        // Listed redundant, but in no dependency (a reply PlaneGCS never gives: what it drops
        // is from a dependency): the state follows the list, and over-definition the
        // dependencies alone, so nothing over-defines.
        let solved = Solved {
            dofs: 0,
            free: Vec::new(),
            redundant: vec![Tag(1)],
            status: SolveStatus::SolvedExceptRedundant,
            ..FakeSketches::as_sent(&translation.system)
        };
        let got = states(&sketch, &translation.read_diagnosed(&solved).unwrap()).unwrap();
        assert_eq!(got.state, SketchState::FullyDefined, "{form}: {got:#?}");
        assert_eq!(
            got.constraints.values().collect::<Vec<_>>(),
            [&Redundant],
            "{form}"
        );
    }
}

#[test]
fn a_reply_listing_a_reference_dimension_redundant_is_a_protocol_error_not_an_over_definition() {
    // A reference x (tag 1, never sent) is a dimension, so if it were ever flagged redundant
    // the rule would over-define by it. It can't be: a reply listing it redundant, or
    // partially redundant, names a tag the core never sent, a reply it can't read
    // (protocol_error), so there are no states to over-define. (Listed conflicting, the
    // read takes it and the reference stays unflagged: silico-core's tests/sketch_states.rs.)
    let sketch = gated(&text(
        &[point("p0000001", 3.0, 4.0)],
        &[],
        &[],
        &[],
        &[
            c(
                r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 3, "driving": false}"#,
            ),
            c(
                r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 4, "driving": true}"#,
            ),
        ],
    ));
    let translation = Translation::new(&sketch).unwrap();
    for (redundant, partially_redundant, dependencies) in [
        (vec![Tag(1)], vec![], vec![]),
        (vec![], vec![Tag(1)], vec![]),
        (vec![], vec![], vec![vec![Tag(1)]]),
        (vec![], vec![], vec![vec![Tag(1), Tag(2)]]),
    ] {
        let solved = Solved {
            dofs: 1,
            free: vec![Value(6)],
            redundant,
            partially_redundant,
            dependencies,
            status: SolveStatus::SolvedExceptRedundant,
            ..FakeSketches::as_sent(&translation.system)
        };
        let e = translation.read_diagnosed(&solved).unwrap_err();
        assert_eq!(e.code(), ErrorCode::ProtocolError, "{e}");
    }
}

#[test]
fn a_redundant_relation_beside_a_conflicting_dimension_over_defines_only_what_the_conflict_reads() {
    // The placed horizontal line, and a circle of radius 5 about p3 (20, 0) with a radius of
    // 5 (k0000004). The fake lists the horizontal (tag 3) redundant and the radius (tag 4)
    // conflicting: refused, over defined. The radius reads no point and names the circle:
    // the circle over defined, its centre by its freedom (fixed here: nothing is free),
    // fully defined. The horizontal, a relation, over-defines nothing: the line and both its
    // ends fully defined. The horizontal stays redundant, the radius conflicting, the placing
    // dimensions unsolved.
    let sketch = gated(&text(
        &[
            point("p0000001", 0.0, 0.0),
            point("p0000002", 10.0, 0.0),
            point("p0000003", 20.0, 0.0),
        ],
        &[line("n0000001", "p0000001", "p0000002")],
        &[circle("c0000001", "p0000003", 5.0)],
        &[],
        &[
            c(
                r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            ),
            c(
                r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}"#,
            ),
            c(r#"{"id": "k0000003", "type": "horizontal", "line": "n0000001"}"#),
            c(
                r#"{"id": "k0000004", "type": "radius", "curve": "c0000001", "value": 5, "driving": true}"#,
            ),
        ],
    ));
    let translation = Translation::new(&sketch).unwrap();
    let solved = Solved {
        dofs: -1,
        redundant: vec![Tag(3)],
        conflicting: vec![Tag(4)],
        ..FakeSketches::as_sent(&translation.system)
    };
    let got = states(&sketch, &translation.read_diagnosed(&solved).unwrap()).unwrap();
    assert_eq!(got.state, SketchState::OverDefined);
    assert_eq!(
        got.solve_problem.as_ref().map(|e| e.code()),
        Some(ErrorCode::OverConstrained)
    );
    assert_eq!(
        got.constraints,
        members(&[
            ("k0000001", ConstraintState::Unsolved),
            ("k0000002", ConstraintState::Unsolved),
            ("k0000003", Redundant),
            ("k0000004", Conflicting),
        ])
    );
    assert_eq!(
        got.points,
        members(&[
            ("p0000001", Fully),
            ("p0000002", Fully),
            ("p0000003", Fully)
        ])
    );
    assert_eq!(got.lines, members(&[("n0000001", Fully)]));
    assert_eq!(got.circles, members(&[("c0000001", Over)]));
}

#[test]
fn every_form_together_flagged_any_way_over_defines_by_addendum_4() {
    // All 21 forms on the drawing at once, each listed by the fake as nothing (0),
    // redundant (1), partially redundant (2), conflicting (3), or nothing but in a
    // dependency PlaneGCS kept (4), every one but the nothings in a dependency of its own,
    // with every unknown free (2 left) or none (0 left). Half the cases are relations only:
    // no dimension in a dependency and no conflict, each relation nothing, redundant,
    // partially redundant, or kept in a dependency, the cases where the rule differs from
    // "anything in a dependency over-defines". By the rule (docs/ports/sketcher.md,
    // *Definition states*, *An over-defining constraint*: M5.0 open item 32's addendum 4),
    // by hand: over defined when any is conflicting or any dimension is in a dependency;
    // then some member over defined. Otherwise by the degrees left, no member over defined,
    // every member under defined with everything free and fully defined with nothing.
    use proptest::strategy::Strategy;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let strategy = (
        proptest::collection::vec(0u8..5, 21),
        proptest::bool::ANY,
        proptest::bool::ANY,
    )
        .prop_map(|(flags, relations_only, free)| {
            let flags: Vec<u8> = if relations_only {
                (0..21)
                    .map(|i| {
                        if EVERY_FORM[i].1 {
                            0
                        } else {
                            [0, 1, 2, 4][usize::from(flags[i] % 4)]
                        }
                    })
                    .collect()
            } else {
                flags
            };
            (flags, free)
        });
    // The cases where a relation is listed, nothing over-defines, and the sketch reads by
    // its degrees: under defined (everything free) and fully defined (nothing free).
    let (under, fully) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let mut runner = proptest::test_runner::TestRunner::new(proptest::test_runner::Config {
        cases: 1000,
        failure_persistence: None,
        ..proptest::test_runner::Config::default()
    });
    runner
        .run(&strategy, |(flags, free)| {
            let picked: Vec<usize> = (0..21).collect();
            let sketch = every_form(&picked);
            let translation = Translation::new(&sketch).unwrap();
            let tags = |want: u8| -> Vec<Tag> {
                (0..21)
                    .filter(|&i| flags[i] == want)
                    .map(|i| Tag(i as u32 + 1))
                    .collect()
            };
            let solved = Solved {
                dofs: if free { 2 } else { 0 },
                free: if free {
                    translation.system.unknowns.clone()
                } else {
                    Vec::new()
                },
                redundant: tags(1),
                partially_redundant: tags(2),
                dependencies: (0..21)
                    .filter(|&i| flags[i] != 0)
                    .map(|i| vec![Tag(i as u32 + 1)])
                    .collect(),
                conflicting: tags(3),
                status: SolveStatus::SolvedExceptRedundant,
                ..FakeSketches::as_sent(&translation.system)
            };
            let got = states(&sketch, &translation.read_diagnosed(&solved).unwrap()).unwrap();
            let over = (0..21).any(|i| flags[i] == 3 || (flags[i] != 0 && EVERY_FORM[i].1));
            let members: Vec<MemberState> = got
                .points
                .values()
                .chain(got.lines.values())
                .chain(got.circles.values())
                .chain(got.arcs.values())
                .copied()
                .collect();
            let want = match (over, free) {
                (true, _) => SketchState::OverDefined,
                (false, true) => SketchState::UnderDefined,
                (false, false) => SketchState::FullyDefined,
            };
            proptest::prop_assert_eq!(got.state, want, "{:?}", flags);
            proptest::prop_assert_eq!(members.contains(&Over), over, "{:?} {:#?}", flags, got);
            if !over {
                let each = if free { Under } else { Fully };
                proptest::prop_assert!(
                    members.iter().all(|&s| s == each),
                    "{:?} {:#?}",
                    flags,
                    got
                );
                if flags.iter().any(|&f| f != 0) {
                    let arm = if free { &under } else { &fully };
                    arm.fetch_add(1, Ordering::Relaxed);
                }
            }
            Ok(())
        })
        .unwrap();
    // About 250 of each expected (half the cases relations only, a relation flagged in all
    // but 1 in 2^26 of them, 13 relations each nothing 1 in 4, half of those free); 100 is
    // far below what chance gives.
    let (under, fully) = (under.into_inner(), fully.into_inner());
    assert!(under >= 100 && fully >= 100, "under {under}, fully {fully}");
}
