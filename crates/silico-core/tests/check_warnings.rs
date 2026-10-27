//! `check`'s warnings (docs/design/m4.1-command-api.md, *Warnings*, decision 17; the
//! maintainer's `fingerprint_missing`, 2026-10-26), through a session on the fake kernel and
//! the fake sketch solver, which solves a sketch where it's drawn unless told otherwise.
//! Expected values are by hand:
//! - `ends_not_joined` holds to FreeCAD's rule for joining a sketch's wires: two ends join
//!   when `gp_Pnt::Distance`, the square root of dx² + dy², each step a rounded double, is
//!   strictly less than 1e-7 mm (`BRepLib_MakeWire::Add`, `l < tolerance`, each vertex's
//!   tolerance `Precision::Confusion()`). So: 4e-8 apart joins; 2e-7 doesn't; 1e-7 along
//!   an axis doesn't (1e-7 squared and rooted is 1e-7 exactly, not less); the double below
//!   1e-7 does (9.999999999999998e-8); 3e-8 by 4e-8 is 5e-8 exactly, and joins; 6e-8 by 8e-8
//!   is 1e-7 exactly, and doesn't; ends at 10 and 10.0000001 along x join, their difference
//!   as doubles being 9.99999993922529e-8 (docs/ports/profile.md measured FreeCAD joining
//!   ends "1e-7 apart along an axis" for that reason).
//! - `constraints_redundant` names the constraints the solver calls redundant, where each
//!   holds on the solution (decision 5); one that doesn't is a problem, and so is a conflict.
//! - `fingerprint_missing` is once a feature that built with no fingerprint stored, none
//!   after `format` writes it, none for a feature that failed or was blocked, and none for a
//!   sketch, which holds none.
//! - The first 100 are listed and all counted; the order is by timeline, then code
//!   (`ends_not_joined`, `constraints_redundant`, `fingerprint_missing`), then, for ends, by
//!   the pair of points in ID order.

use std::path::PathBuf;
use std::sync::Arc;

use silico_core::cache::Cache;
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::command::{CheckAnswer, CheckModel, SessionId, Sessions};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{Op, Primitive};
use silico_core::sketch::SketchSolver;
use silico_core::{Error, ErrorCode, WarningCode};
use silico_protocol::Tag;
use tempfile::TempDir;

use crate::sessions::{answer, id};

/// FreeCAD's words for the cause of `ends_not_joined`, with the distance as it's written.
fn apart(distance: &str) -> String {
    format!("the ends are {distance} mm apart, within FreeCAD's joining distance of 1e-7 mm")
}

struct Fixture {
    dir: TempDir,
    kernel: Arc<FakeKernel>,
    sketches: FakeSketches,
    sessions: Sessions<Arc<FakeKernel>>,
}

/// Sessions on the fake kernel and solver, with `text` open as s1.
fn opened(text: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = FakeSketches::new();
    let solver = sketches.clone();
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    );
    let f = Fixture {
        dir,
        kernel,
        sketches,
        sessions,
    };
    std::fs::write(f.path(), text).unwrap();
    let opened = answer(f.sessions.open(f.path(), false));
    assert_eq!(opened.session.unwrap().id, s1());
    f
}

fn s1() -> SessionId {
    SessionId::parse("s1").unwrap()
}

impl Fixture {
    fn path(&self) -> PathBuf {
        self.dir.path().join("part.silico")
    }

    /// The check of the current revision.
    fn check(&self) -> CheckAnswer {
        let checked = answer(
            self.sessions
                .check(&s1(), CheckModel::Current { fresh: false }),
        );
        // No kernel fault (a document the gate refuses has no model to have one).
        assert_eq!(checked.model.and_then(|model| model.fault), None);
        checked.result.unwrap()
    }
}

/// Each warning listed: its code, feature, elements, and cause.
fn said(answer: &CheckAnswer) -> Vec<(WarningCode, String, Vec<String>, Option<String>)> {
    answer
        .warnings
        .listed()
        .iter()
        .map(|w| {
            (
                w.code(),
                w.feature().unwrap().to_owned(),
                w.elements().to_vec(),
                w.cause().map(str::to_owned),
            )
        })
        .collect()
}

/// A document of `features`.
fn document(features: &[String]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

fn point(id: &str, x: f64, y: f64) -> String {
    format!(r#"{{"id": "{id}", "x": {x}, "y": {y}, "construction": false}}"#)
}

fn line(id: &str, from: &str, to: &str, construction: bool) -> String {
    format!(r#"{{"id": "{id}", "from": "{from}", "to": "{to}", "construction": {construction}}}"#)
}

/// A sketch `id` on xy of these lists, each the JSON inside its brackets.
fn sketch(
    id: &str,
    points: &[String],
    lines: &[String],
    arcs: &[String],
    constraints: &str,
) -> String {
    format!(
        r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{}], "lines": [{}], "circles": [], "arcs": [{}], "constraints": [{constraints}]}}"#,
        points.join(", "),
        lines.join(", "),
        arcs.join(", ")
    )
}

/// A box `id`, 4 by 2 by 1, at x `x`, with no fingerprint.
fn block(id: &str, x: u32) -> String {
    format!(
        r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Block", "position": [{x}, 0, 0], "length": 4, "width": 2, "height": 1}}"#
    )
}

/// Line na000001 from (−10, 0) to (0, 0), and line nb000001 from `at` to (10, 5): the second
/// line's start is the one near the first's end.
fn two_lines(at: (f64, f64)) -> String {
    sketch(
        "s0000001",
        &[
            point("pa000001", -10.0, 0.0),
            point("pa000002", 0.0, 0.0),
            point("pa000003", at.0, at.1),
            point("pa000004", 10.0, 5.0),
        ],
        &[
            line("na000001", "pa000001", "pa000002", false),
            line("nb000001", "pa000003", "pa000004", false),
        ],
        &[],
        "",
    )
}

#[test]
fn ends_closer_than_freecads_joining_distance_warn_and_others_dont() {
    let next_below = f64::from_bits(1e-7_f64.to_bits() - 1);
    assert_eq!(next_below.to_string(), "0.00000009999999999999998");
    for (at, warns) in [
        ((4e-8, 0.0), Some("4e-8")),
        ((2e-7, 0.0), None),
        ((1e-7, 0.0), None),
        ((next_below, 0.0), Some("9.999999999999998e-8")),
        ((3e-8, 4e-8), Some("5e-8")),
        ((6e-8, 8e-8), None),
        ((-4e-8, 0.0), Some("4e-8")),
        ((0.0, -1e-7), None),
        ((0.0, 0.0), Some("0")),
    ] {
        let f = opened(&document(&[two_lines(at)]));
        let checked = f.check();
        assert!(
            checked.problems.is_empty(),
            "{at:?}: {:?}",
            checked.problems
        );
        match warns {
            Some(distance) => {
                assert_eq!(
                    said(&checked),
                    [(
                        WarningCode::EndsNotJoined,
                        "s0000001".to_owned(),
                        vec!["na000001".to_owned(), "nb000001".to_owned()],
                        Some(apart(distance)),
                    )],
                    "{at:?}"
                );
                assert_eq!(checked.warnings.total(), 1);
            }
            None => assert!(checked.warnings.is_empty(), "{at:?}: {:?}", said(&checked)),
        }
    }
}

#[test]
fn ends_a_tenth_of_a_micron_apart_along_an_axis_away_from_the_origin_join_as_freecad_joins_them() {
    // 10.0000001 − 10 is 9.99999993922529e-8 as doubles: under 1e-7, so FreeCAD joins them.
    assert_eq!(10.0000001_f64 - 10.0, 9.99999993922529e-8);
    let text = document(&[sketch(
        "s0000001",
        &[
            point("pa000001", 0.0, 0.0),
            point("pa000002", 10.0, 0.0),
            point("pa000003", 10.0000001, 0.0),
            point("pa000004", 20.0, 5.0),
        ],
        &[
            line("na000001", "pa000001", "pa000002", false),
            line("nb000001", "pa000003", "pa000004", false),
        ],
        &[],
        "",
    )]);
    let checked = opened(&text).check();
    assert_eq!(
        said(&checked),
        [(
            WarningCode::EndsNotJoined,
            "s0000001".to_owned(),
            vec!["na000001".to_owned(), "nb000001".to_owned()],
            Some(apart("9.99999993922529e-8")),
        )]
    );
}

#[test]
fn ends_not_joined_is_the_designs_example_word_for_word() {
    // The design's sketch s0000002: curves pb000003 and pb000007, their ends 4e-8 apart.
    let text = document(&[sketch(
        "s0000002",
        &[
            point("pa000001", -5.0, 0.0),
            point("pa000002", 0.0, 0.0),
            point("pa000004", 4e-8, 0.0),
            point("pa000005", 5.0, 5.0),
        ],
        &[
            line("pb000003", "pa000001", "pa000002", false),
            line("pb000007", "pa000004", "pa000005", false),
        ],
        &[],
        "",
    )]);
    let checked = opened(&text).check();
    let [warning] = checked.warnings.listed() else {
        panic!("{:?}", said(&checked));
    };
    let written = write(&warning.to_wire(), Layout::Canonical).unwrap() + "\n";
    let example = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/contract/commands/examples/warning/ends-not-joined.json"),
    )
    .unwrap();
    assert_eq!(written, example);
    // And the check's whole result, warning and all, holds to check's snapshot.
    let schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/contract/commands/check.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let result = write(&checked.to_wire(), Layout::Canonical).unwrap();
    let value: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(
        jsonschema::validator_for(&schema["result"])
            .unwrap()
            .is_valid(&value),
        "{result}"
    );
    assert_eq!(value["warnings_total"], 1);
    assert_eq!(value["warnings"][0]["code"], "ends_not_joined");
}

#[test]
fn only_profile_curves_ends_that_arent_one_point_warn() {
    // A construction line's end 4e-8 from a profile line's doesn't warn; nor do two profile
    // lines that share their points.
    let text = document(&[sketch(
        "s0000001",
        &[
            point("pa000001", -10.0, 0.0),
            point("pa000002", 0.0, 0.0),
            point("pa000003", 4e-8, 0.0),
            point("pa000004", 0.0, 10.0),
            point("pa000005", 5.0, 5.0),
        ],
        &[
            line("na000001", "pa000001", "pa000002", false),
            line("na000002", "pa000003", "pa000004", true),
            line("na000003", "pa000004", "pa000005", false),
            line("na000004", "pa000005", "pa000004", false),
        ],
        &[],
        "",
    )]);
    let checked = opened(&text).check();
    assert!(checked.warnings.is_empty(), "{:?}", said(&checked));

    // An arc's end near a line's: the arc about (5, 5) from (5, 0) counter-clockwise to
    // (10, 5), and a line from 4e-8 above (5, 0) out to (0, -5).
    let text = document(&[sketch(
        "s0000001",
        &[
            point("pa000001", 5.0, 5.0),
            point("pa000002", 5.0, 0.0),
            point("pa000003", 10.0, 5.0),
            point("pa000004", 5.0, 4e-8),
            point("pa000005", 0.0, -5.0),
        ],
        &[line("nb000001", "pa000004", "pa000005", false)],
        &[r#"{"id": "aa000001", "center": "pa000001", "start": "pa000002", "end": "pa000003", "construction": false}"#.to_owned()],
        "",
    )]);
    let checked = opened(&text).check();
    assert_eq!(
        said(&checked),
        [(
            WarningCode::EndsNotJoined,
            "s0000001".to_owned(),
            vec!["aa000001".to_owned(), "nb000001".to_owned()],
            Some(apart("4e-8")),
        )]
    );
}

/// Line na000001 from (0, 0) to (10, `rise`), held horizontal three times (k0000001,
/// k0000002, k0000003): tags 1, 2, and 3, in ID order.
fn held_three_times(rise: f64) -> String {
    let horizontal =
        |k: &str| format!(r#"{{"id": "{k}", "type": "horizontal", "line": "na000001"}}"#);
    sketch(
        "s0000001",
        &[point("pa000001", 0.0, 0.0), point("pa000002", 10.0, rise)],
        &[line("na000001", "pa000001", "pa000002", false)],
        &[],
        &[
            horizontal("k0000001"),
            horizontal("k0000002"),
            horizontal("k0000003"),
        ]
        .join(", "),
    )
}

#[test]
fn agreeing_redundant_constraints_warn_naming_each_and_disagreeing_or_conflicting_ones_are_problems()
 {
    // The solver calls the second and third horizontals redundant; the line is horizontal,
    // so each holds: one warning, naming both.
    let f = opened(&document(&[held_three_times(0.0)]));
    f.sketches.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        solved.redundant = vec![Tag(2), Tag(3)];
        Ok(solved)
    });
    let checked = f.check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    let [warning] = checked.warnings.listed() else {
        panic!("{:?}", said(&checked));
    };
    assert_eq!(warning.code(), WarningCode::ConstraintsRedundant);
    assert_eq!(warning.code().as_str(), "constraints_redundant");
    assert_eq!(warning.feature(), Some("s0000001"));
    assert_eq!(warning.elements(), ["k0000002", "k0000003"]);
    assert_eq!(warning.elements_total(), 2);
    assert_eq!(
        warning.message(),
        "2 constraints of the sketch are redundant: they repeat what its other constraints \
         already fix, and agree with them"
    );
    assert_eq!(warning.parameter(), None);
    assert!(
        warning
            .suggestion()
            .unwrap()
            .starts_with("remove the constraints named")
    );

    // One alone is said in the singular; partially redundant counts as redundant.
    let f = opened(&document(&[held_three_times(0.0)]));
    f.sketches.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        solved.partially_redundant = vec![Tag(3)];
        Ok(solved)
    });
    let checked = f.check();
    let [warning] = checked.warnings.listed() else {
        panic!("{:?}", said(&checked));
    };
    assert_eq!(warning.elements(), ["k0000003"]);
    assert_eq!(
        warning.message(),
        "a constraint of the sketch is redundant: it repeats what its other constraints \
         already fix, and agrees with them"
    );

    // The line rising 1 in 10: the redundant horizontal is 1 mm off on the solution, so it
    // disagrees: a problem, over_constrained, and no warning.
    let f = opened(&document(&[held_three_times(1.0)]));
    f.sketches.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        solved.redundant = vec![Tag(3)];
        Ok(solved)
    });
    let checked = f.check();
    assert_eq!(checked.problems.total(), 1, "{:?}", checked.problems);
    assert_eq!(
        checked.problems.errors()[0].code(),
        ErrorCode::OverConstrained
    );
    assert!(checked.warnings.is_empty(), "{:?}", said(&checked));

    // A conflict: a problem, and no warning.
    let f = opened(&document(&[held_three_times(0.0)]));
    f.sketches.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        solved.conflicting = vec![Tag(1), Tag(2)];
        solved.dofs = -1;
        Ok(solved)
    });
    let checked = f.check();
    assert_eq!(checked.problems.total(), 1, "{:?}", checked.problems);
    assert_eq!(
        checked.problems.errors()[0].code(),
        ErrorCode::OverConstrained
    );
    assert_eq!(
        checked.problems.errors()[0].elements(),
        ["k0000001", "k0000002"]
    );
    assert!(checked.warnings.is_empty(), "{:?}", said(&checked));
}

#[test]
fn a_feature_built_with_no_fingerprint_warns_once_until_format_stores_it() {
    let f = opened(&document(&[block("k7f2qx3m", 0)]));
    let checked = f.check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    let [warning] = checked.warnings.listed() else {
        panic!("{:?}", said(&checked));
    };
    assert_eq!(checked.warnings.total(), 1);
    assert_eq!(warning.code(), WarningCode::FingerprintMissing);
    assert_eq!(warning.code().as_str(), "fingerprint_missing");
    assert_eq!(
        warning.message(),
        "k7f2qx3m built, but its file stores no fingerprint, so a change in what it builds \
         isn't caught as drift"
    );
    assert_eq!(warning.feature(), Some("k7f2qx3m"));
    assert_eq!(warning.elements(), [] as [&str; 0]);
    assert_eq!(warning.parameter(), Some("fingerprint"));
    assert_eq!(warning.cause(), None);
    assert_eq!(
        warning.suggestion(),
        Some("save the document (format or commit) to store it")
    );
    // The published model says the same.
    let published = answer(f.sessions.check(&s1(), CheckModel::Published));
    assert_eq!(published.result.unwrap().warnings, checked.warnings);
    // Formatted, the fingerprint is in the file, and nothing warns.
    let formatted = answer(f.sessions.format(&s1(), None, true)).result.unwrap();
    assert_eq!(formatted.refingerprinted, [id("k7f2qx3m")]);
    assert!(
        std::fs::read_to_string(f.path())
            .unwrap()
            .contains("\"fingerprint\"")
    );
    let checked = f.check();
    assert!(checked.problems.is_empty() && checked.warnings.is_empty());
    let published = answer(f.sessions.check(&s1(), CheckModel::Published));
    assert!(published.result.unwrap().warnings.is_empty());
}

#[test]
fn a_failed_or_blocked_feature_and_a_sketch_never_warn_of_a_fingerprint() {
    // The kernel refuses the box 3 long: it fails, the fuse using it is blocked, and the
    // sketch holds no fingerprint: none of them warns.
    let fuse = r#"{"id": "f0000003", "type": "fuse", "v": 1, "name": "Both", "base": "k7f2qx3m", "tool": "m0000002"}"#;
    let f = opened(&document(&[
        block("k7f2qx3m", 0).replace(r#""length": 4"#, r#""length": 3"#),
        block("m0000002", 10),
        fuse.to_owned(),
        sketch("s0000004", &[], &[], &[], ""),
    ]));
    f.kernel.refuse(|op| match op {
        Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx == 3.0 => {
            Some(Error::new(ErrorCode::KernelFailed, "refused"))
        }
        _ => None,
    });
    let checked = f.check();
    assert_eq!(checked.problems.total(), 1, "{:?}", checked.problems);
    assert_eq!(checked.problems.errors()[0].feature(), Some("k7f2qx3m"));
    // The other box built, with no fingerprint: it alone warns.
    assert_eq!(
        said(&checked),
        [(
            WarningCode::FingerprintMissing,
            "m0000002".to_owned(),
            vec![],
            None
        )]
    );
}

#[test]
fn warnings_past_the_hundredth_are_counted_not_listed() {
    // 101 boxes without fingerprints: the first hundred listed, in timeline order, all 101
    // counted.
    let ids: Vec<String> = (0..101).map(|i| format!("b{i:07}")).collect();
    let boxes: Vec<String> = ids
        .iter()
        .zip(0..)
        .map(|(id, i)| block(id, i * 10))
        .collect();
    let checked = opened(&document(&boxes)).check();
    assert_eq!(checked.warnings.total(), 101);
    let listed: Vec<&str> = checked
        .warnings
        .listed()
        .iter()
        .map(|w| w.feature().unwrap())
        .collect();
    assert_eq!(listed, ids[..100]);
}

#[test]
fn ends_at_one_place_are_each_pair_listed_by_their_points_ids_to_the_hundredth_all_counted() {
    // Fifteen lines, each from its own point at the origin out to its own far point: 15 · 14
    // / 2 = 105 pairs of ends 0 apart. The first hundred pairs by their points' IDs are
    // listed: all but (11, 13), (11, 14), (12, 13), (12, 14), (13, 14), counting from 0. The
    // points are written in reverse, so the file's order isn't the IDs'.
    let near = |i: usize| format!("pa{i:06}");
    let far = |i: usize| format!("pz{i:06}");
    let curve = |i: usize| format!("na{i:06}");
    let mut points = Vec::new();
    let mut lines = Vec::new();
    for i in (0..15).rev() {
        points.push(point(&near(i), 0.0, 0.0));
        points.push(point(&far(i), 10.0, i as f64));
        lines.push(line(&curve(i), &near(i), &far(i), false));
    }
    let checked = opened(&document(&[sketch("s0000001", &points, &lines, &[], "")])).check();
    assert_eq!(checked.warnings.total(), 105);
    let mut expected = Vec::new();
    for a in 0..15 {
        for b in a + 1..15 {
            expected.push(vec![curve(a), curve(b)]);
        }
    }
    expected.truncate(100);
    assert_eq!(expected.last().unwrap(), &vec![curve(11), curve(12)]);
    let listed: Vec<Vec<String>> = checked
        .warnings
        .listed()
        .iter()
        .map(|w| w.elements().to_vec())
        .collect();
    assert_eq!(listed, expected);
    assert!(
        checked
            .warnings
            .listed()
            .iter()
            .all(|w| w.cause() == Some(apart("0").as_str()))
    );
}

#[test]
fn warnings_are_in_timeline_order_then_by_code_then_by_points() {
    // Sketch s0000001: two pairs of ends (a: pa000001 near pa000004, b: pa000002 near
    // pa000003; by IDs, b's pair (2, 3) comes after a's (1, 4)), drawn with b's at the
    // lesser x, and two redundant horizontals; then a box with no fingerprint; then sketch
    // s0000003 with one pair.
    let first = sketch(
        "s0000001",
        &[
            point("pa000001", 5.0, 0.0),
            point("pa000002", -5.0, 0.0),
            point("pa000003", -5.0, 3e-8),
            point("pa000004", 5.0, -6e-8),
            point("pa000005", 0.0, 9.0),
        ],
        &[
            line("na000001", "pa000002", "pa000001", false),
            line("na000002", "pa000003", "pa000005", false),
            line("na000003", "pa000004", "pa000005", false),
        ],
        &[],
        r#"{"id": "k0000001", "type": "horizontal", "line": "na000001"}, {"id": "k0000002", "type": "horizontal", "line": "na000001"}"#,
    );
    let third = sketch(
        "s0000003",
        &[
            point("pa000001", 0.0, 0.0),
            point("pa000002", 1.0, 0.0),
            point("pa000003", 1.0, 2e-8),
            point("pa000004", 1.0, 1.0),
        ],
        &[
            line("na000001", "pa000001", "pa000002", false),
            line("na000002", "pa000003", "pa000004", false),
        ],
        &[],
        "",
    );
    let f = opened(&document(&[first, block("k7f2qx3m", 0), third]));
    f.sketches.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        if !system.constraints.is_empty() {
            solved.redundant = vec![Tag(2)];
        }
        Ok(solved)
    });
    let checked = f.check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    let strings = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        said(&checked),
        [
            (
                WarningCode::EndsNotJoined,
                "s0000001".to_owned(),
                strings(&["na000001", "na000003"]),
                Some(apart("6e-8")),
            ),
            (
                WarningCode::EndsNotJoined,
                "s0000001".to_owned(),
                strings(&["na000001", "na000002"]),
                Some(apart("3e-8")),
            ),
            (
                WarningCode::ConstraintsRedundant,
                "s0000001".to_owned(),
                strings(&["k0000002"]),
                Some(
                    "the solver solved the sketch without the equations they repeat, and each \
                     holds on the solution"
                        .to_owned()
                ),
            ),
            (
                WarningCode::FingerprintMissing,
                "k7f2qx3m".to_owned(),
                vec![],
                None
            ),
            (
                WarningCode::EndsNotJoined,
                "s0000003".to_owned(),
                strings(&["na000001", "na000002"]),
                Some(apart("2e-8")),
            ),
        ]
    );
}

#[test]
fn a_document_the_gate_refuses_has_its_problems_and_no_warnings() {
    // A box of no length: refused by the gate, so there's no model to warn of.
    let text = document(&[block("k7f2qx3m", 0).replace(r#""length": 4"#, r#""length": 0"#)]);
    let checked = opened(&text).check();
    assert_eq!(checked.problems.total(), 1, "{:?}", checked.problems);
    assert!(checked.warnings.is_empty());
}
