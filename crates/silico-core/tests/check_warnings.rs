//! `check`'s warnings (docs/design/m4.1-command-api.md, *Warnings*, decision 17; the
//! maintainer's `fingerprint_missing`, 2026-10-26), through a session on the fake kernel and
//! the fake sketch solver, which solves a sketch where it's drawn unless told otherwise.
//! Expected values are by hand, or FreeCAD 1.1.3's (the oracle's, run headless on
//! 2026-10-26):
//! - `ends_not_joined` holds to FreeCAD's rule for joining a sketch's wires: two ends join
//!   when `gp_Pnt::Distance`, the square root of dx² + dy², each step a rounded double, is
//!   strictly less than 1e-7 mm (`BRepLib_MakeWire::Add`, `l < tolerance`, each vertex's
//!   tolerance `Precision::Confusion()`). So: 4e-8 apart joins; 2e-7 doesn't; 1e-7 along
//!   an axis doesn't (1e-7 squared and rooted is 1e-7 exactly, not less); the double below
//!   1e-7 does (9.999999999999998e-8); 3e-8 by 4e-8 is 5e-8 exactly, and joins; 6e-8 by 8e-8
//!   is 1e-7 exactly, and doesn't; ends at 10 and 10.0000001 along x join, their difference
//!   as doubles being 9.99999993922529e-8 (docs/ports/profile.md measured FreeCAD joining
//!   ends "1e-7 apart along an axis" for that reason). Points that join, and so on through
//!   each other, are one cluster, as FreeCAD's wire join chains them (`BRepLib_MakeWire`
//!   grows the tolerance of the vertex it joins an end to; FreeCAD's `SketchAnalysis` groups
//!   only the ends near the first of a run sorted by position, each axis apart, without
//!   chaining), and one warning, as `SketchAnalysis` reports a group, not pairs
//!   (c036128aa0): its points, in ID order, then the curves ending at them, in ID order,
//!   each once, the first 100 of the list named and all counted; its suggestion to lengthen
//!   first a curve with both its ends in the cluster, as the profile's refusal says.
//! - FreeCAD's own tests, ported: `TestSketchValidateCoincidents.testSingleMissingCoincidentCase`
//!   (a rectangle closing 3.552713678800501e-15 mm short: FreeCAD finds 1 missing
//!   coincidence), and `TestSketcherSolver.testThreeLinesWithCoincidences_1`/`_2` (three
//!   ends at one place made coincident: none missing). Silico adds the same three ends not
//!   made coincident, with FreeCAD 1.1.3's count for them, run headless: 2 missing, k − 1
//!   for a cluster of k, which Silico says as one warning naming the three.
//! - `constraints_redundant` names the constraints the solver calls redundant, where each
//!   holds on the solution (decision 5); one that doesn't is a problem, and so is a
//!   conflict. Those it calls partially redundant are a warning of their own, which doesn't
//!   advise removing them: FreeCAD 1.1.3 calls the midpoint of the port review's sketch
//!   partially redundant (`solve()` 0, `RedundantConstraints` [], `PartiallyRedundantConstraints`
//!   [1], fully constrained), and removing it frees the point along the line.
//! - `fingerprint_missing` is once a feature that built with no fingerprint stored, none
//!   after `format` writes it, none for a feature that failed, was blocked, or couldn't be
//!   measured, none for a sketch, which holds none, and none for a feature an open
//!   transaction's edits reached, whose commit writes it (decision 28); but once a feature
//!   carried from a commit that couldn't write its record has built, outside a transaction,
//!   where `format` writes it.
//! - The first 100 are listed and all counted; the order is by timeline, then code
//!   (`ends_not_joined`, `constraints_redundant`, `fingerprint_missing`), then, for ends, by
//!   cluster, by its least point ID.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use silico_core::cache::Cache;
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::command::{CheckAnswer, CheckModel, Revision, SessionId, Sessions};
use silico_core::document::json::Node;
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{Op, Primitive};
use silico_core::sketch::SketchSolver;
use silico_core::{Error, ErrorCode, WarningCode};
use silico_protocol::Tag;
use tempfile::TempDir;

use crate::sessions::{answer, id};

/// The cause of `ends_not_joined` for two points, with the distance as it's written.
fn apart(distance: &str) -> String {
    format!("the points are {distance} mm apart, closer than FreeCAD's joining distance of 1e-7 mm")
}

/// The cause of `ends_not_joined` for more than two points, with the largest distance.
fn at_most(distance: &str) -> String {
    format!(
        "the points are at most {distance} mm apart, each closer than FreeCAD's joining \
         distance of 1e-7 mm to another of them"
    )
}

/// `names` as owned strings.
fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| (*s).to_owned()).collect()
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

/// A warning as [`said`] gives it: its code, feature, elements, and cause.
type Said = (WarningCode, String, Vec<String>, Option<String>);

/// Each warning listed: its code, feature, elements, and cause.
fn said(answer: &CheckAnswer) -> Vec<Said> {
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
                        strings(&["pa000002", "pa000003", "na000001", "nb000001"]),
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
            strings(&["pa000002", "pa000003", "na000001", "nb000001"]),
            Some(apart("9.99999993922529e-8")),
        )]
    );
}

#[test]
fn ends_not_joined_is_the_designs_example_word_for_word() {
    // The design's sketch s0000002: curves pb000003 and pb000007, ending at points pa000002
    // and pa000004, 4e-8 apart.
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
            strings(&["pa000002", "pa000004", "aa000001", "nb000001"]),
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
    assert_eq!(
        warning.suggestion(),
        Some(
            "remove the constraints named, or make a dimension among them a reference \
             (`driving: false`), keeping the sketch as constrained as it is"
        )
    );

    // One alone is said in the singular.
    let f = opened(&document(&[held_three_times(0.0)]));
    f.sketches.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        solved.redundant = vec![Tag(3)];
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

    // Partially redundant is a warning of its own, after the redundant one, under the same
    // code, with no advice to remove them: FreeCAD shows them apart, and removing one frees
    // what only it fixes.
    let f = opened(&document(&[held_three_times(0.0)]));
    f.sketches.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        solved.redundant = vec![Tag(2)];
        solved.partially_redundant = vec![Tag(1), Tag(3)];
        Ok(solved)
    });
    let checked = f.check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    let [full, partly] = checked.warnings.listed() else {
        panic!("{:?}", said(&checked));
    };
    assert_eq!(full.elements(), ["k0000002"]);
    assert!(
        full.message()
            .starts_with("a constraint of the sketch is redundant")
    );
    assert_eq!(partly.code(), WarningCode::ConstraintsRedundant);
    assert_eq!(partly.feature(), Some("s0000001"));
    assert_eq!(partly.elements(), ["k0000001", "k0000003"]);
    assert_eq!(partly.elements_total(), 2);
    assert_eq!(
        partly.message(),
        "2 constraints of the sketch are partly redundant: only part of what each fixes is \
         repeated by its other constraints, and that part agrees with them"
    );
    assert_eq!(
        partly.cause(),
        Some(
            "the solver solved the sketch without the equations of theirs that repeat others, \
             and each holds on the solution"
        )
    );
    assert_eq!(
        partly.suggestion(),
        Some(
            "review the constraints named: each also fixes what no other constraint does, so \
             removing one would loosen the sketch"
        )
    );
    assert_eq!(checked.warnings.total(), 2);

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
        Some("format the document to store it")
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
fn clusters_past_the_hundredth_are_counted_and_the_hundred_listed_are_the_first_by_point_id() {
    // 105 clusters of two points 4e-8 apart, cluster i at x = 2000 − 10i: their x falls as
    // their IDs rise, so the sweep, which goes by x, meets them last first. Listed: the
    // clusters of the least IDs, 0 to 99, in that order; all 105 counted. The points are
    // written in reverse, so the file's order isn't the IDs' either.
    let mut points = Vec::new();
    let mut lines = Vec::new();
    for i in (0..105).rev() {
        let x = 2000.0 - 10.0 * i as f64;
        points.push(point(&format!("pa{i:06}"), x, 0.0));
        points.push(point(&format!("pb{i:06}"), x + 4e-8, 0.0));
        points.push(point(&format!("py{i:06}"), x, -5.0));
        points.push(point(&format!("pz{i:06}"), x, 5.0));
        lines.push(line(
            &format!("na{i:06}"),
            &format!("pa{i:06}"),
            &format!("pz{i:06}"),
            false,
        ));
        lines.push(line(
            &format!("nb{i:06}"),
            &format!("pb{i:06}"),
            &format!("py{i:06}"),
            false,
        ));
    }
    let checked = opened(&document(&[sketch("s0000001", &points, &lines, &[], "")])).check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    assert_eq!(checked.warnings.total(), 105);
    let listed: Vec<Vec<String>> = checked
        .warnings
        .listed()
        .iter()
        .map(|w| w.elements().to_vec())
        .collect();
    let expected: Vec<Vec<String>> = (0..100)
        .map(|i| {
            vec![
                format!("pa{i:06}"),
                format!("pb{i:06}"),
                format!("na{i:06}"),
                format!("nb{i:06}"),
            ]
        })
        .collect();
    assert_eq!(listed, expected);
}

#[test]
fn a_clusters_points_come_first_then_its_curves_sorted_by_id() {
    // Points pa000001 and pa000002, 4e-8 apart: the lesser point ends the curve of the
    // greater ID (nz000009), the greater the lesser (na000001), and a third curve
    // (nm000005) runs between them both's far ends, no part of it. Named: the points in ID
    // order, then the curves in ID order, whatever point each ends at.
    let text = document(&[sketch(
        "s0000001",
        &[
            point("pa000001", 0.0, 0.0),
            point("pa000002", 4e-8, 0.0),
            point("pz000001", 0.0, 10.0),
            point("pz000002", 10.0, 0.0),
        ],
        &[
            line("nz000009", "pz000001", "pa000001", false),
            line("na000001", "pa000002", "pz000002", false),
            line("nm000005", "pz000001", "pz000002", false),
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
            strings(&["pa000001", "pa000002", "na000001", "nz000009"]),
            Some(apart("4e-8")),
        )]
    );
    let warning = &checked.warnings.listed()[0];
    assert_eq!(warning.elements_total(), 4);
    assert_eq!(
        warning.message(),
        "2 points that end curves are at one place but aren't one point, so the profile is \
         open there"
    );
    assert_eq!(
        warning.suggestion(),
        Some("share one point: make the curves named end at one of the points named")
    );
}

#[test]
fn ends_joined_through_each_other_are_one_cluster_its_cause_their_farthest() {
    // pa000001 at 0, pa000002 at 6e-8, pa000003 at 1.2e-7 along x: 1 joins 2 and 2 joins 3,
    // though 1 and 3 are 1.2e-7 apart: one cluster of three, as FreeCAD's wire join chains
    // them (FreeCAD 1.1.3, run headless on these ends: one vertex at 6e-8 with a tolerance
    // grown to 1.6e-7, 6 vertices, not 7; its `SketchAnalysis` doesn't chain, and at 1e-7
    // groups 0 and 6e-8 alone), its cause their largest distance, 1.2e-7 as doubles'
    // differences: 1.2e-7 − 0 is 1.2e-7 exactly. A fourth point, pa000004, 2e-7 past
    // pa000003 (3.2e-7), joins none, as FreeCAD keeps it apart.
    let text = document(&[sketch(
        "s0000001",
        &[
            point("pa000001", 0.0, 0.0),
            point("pa000002", 6e-8, 0.0),
            point("pa000003", 1.2e-7, 0.0),
            point("pa000004", 3.2e-7, 0.0),
            point("pz000001", -5.0, 5.0),
            point("pz000002", 0.0, 5.0),
            point("pz000003", 5.0, 5.0),
            point("pz000004", 5.0, -5.0),
        ],
        &[
            line("na000001", "pa000001", "pz000001", false),
            line("na000002", "pa000002", "pz000002", false),
            line("na000003", "pa000003", "pz000003", false),
            line("na000004", "pa000004", "pz000004", false),
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
            strings(&[
                "pa000001", "pa000002", "pa000003", "na000001", "na000002", "na000003"
            ]),
            Some(at_most("1.2e-7")),
        )]
    );
    assert_eq!(
        checked.warnings.listed()[0].message(),
        "3 points that end curves are at one place but aren't one point, so the profile is \
         open there"
    );
}

#[test]
fn a_curve_with_both_ends_in_a_cluster_is_named_once_and_its_suggestion_is_to_lengthen_it_first() {
    // Line na000001 from pa000001 at 0 to pa000002 at 1.5e-7 along x (longer than 1e-7, so
    // the gate takes it); line na000002 from pa000003 at 7.5e-8, 7.5e-8 from each (1.5e-7
    // halved, exactly, so both differences are 7.5e-8 exactly), to pz000001. One cluster of
    // the three, its cause 1.5e-7 (1.5e-7 − 0), its curves na000001, once though both its
    // ends are in it, and na000002: 5 named. Sharing one point would shrink na000001 to
    // nothing, so the suggestion is first the profile's for those ends, "lengthen line
    // na000001, or ...".
    let text = document(&[sketch(
        "s0000001",
        &[
            point("pa000001", 0.0, 0.0),
            point("pa000002", 1.5e-7, 0.0),
            point("pa000003", 7.5e-8, 0.0),
            point("pz000001", 0.0, 10.0),
        ],
        &[
            line("na000001", "pa000001", "pa000002", false),
            line("na000002", "pa000003", "pz000001", false),
        ],
        &[],
        "",
    )]);
    let checked = opened(&text).check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    assert_eq!(
        said(&checked),
        [(
            WarningCode::EndsNotJoined,
            "s0000001".to_owned(),
            strings(&["pa000001", "pa000002", "pa000003", "na000001", "na000002"]),
            Some(at_most("1.5e-7")),
        )]
    );
    let warning = &checked.warnings.listed()[0];
    assert_eq!(warning.elements_total(), 5);
    assert_eq!(
        warning.suggestion(),
        Some(
            "lengthen line na000001, or share one point: make the curves named end at one of \
             the points named"
        )
    );
}

#[test]
fn every_curve_with_both_ends_in_a_cluster_is_to_be_lengthened_in_id_order() {
    // Two short lines across one cluster, their IDs against their points' order: nz000002
    // from pa000001 at 0 to pa000003 at 1.5e-7, na000009 from pa000002 at 7.5e-8 to
    // pa000004 at 2.25e-7 (each 1.5e-7 long; pa000002 and pa000003, 7.5e-8 apart, chain
    // them), and an arc nm000005 from pa000004 round to pz000001, whose end, not both, is in
    // the cluster. Named lengthened: both lines, by ID, with their nouns.
    let arc = r#"{"id": "nm000005", "center": "pz000002", "start": "pa000004", "end": "pz000001", "construction": false}"#;
    let text = document(&[sketch(
        "s0000001",
        &[
            point("pa000001", 0.0, 0.0),
            point("pa000002", 7.5e-8, 0.0),
            point("pa000003", 1.5e-7, 0.0),
            point("pa000004", 2.25e-7, 0.0),
            point("pz000001", 10.000000225, 10.0),
            point("pz000002", 10.000000225, 0.0),
        ],
        &[
            line("nz000002", "pa000001", "pa000003", false),
            line("na000009", "pa000002", "pa000004", false),
        ],
        &[arc.to_owned()],
        "",
    )]);
    let checked = opened(&text).check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    let [warning] = checked.warnings.listed() else {
        panic!("{:?}", said(&checked));
    };
    assert_eq!(
        warning.elements(),
        strings(&[
            "pa000001", "pa000002", "pa000003", "pa000004", "na000009", "nm000005", "nz000002"
        ])
    );
    assert_eq!(
        warning.suggestion(),
        Some(
            "lengthen line na000009 and line nz000002, or share one point: make the curves \
             named end at one of the points named"
        )
    );
}

#[test]
fn a_near_full_arc_with_both_ends_in_a_cluster_is_given_the_gates_advice_for_an_arc() {
    // Arc na000001 of radius 10 about pz000002 (0, 0), counter-clockwise from pa000002
    // (10, 1.5e-7) round to pa000001 (10, 0): a near-full arc whose ends are 1.5e-7 apart
    // (the gate refuses only 1e-7 or less). Line na000002 from pa000003 (10, 7.5e-8), 7.5e-8
    // from each end, to pz000001 (20, 10). One cluster of the three; the arc has both its
    // ends in it, so sharing one point would close it into a circle, and lengthening it
    // would make it overlap itself (the port review, #189): the advice is the sketch gate's
    // for an arc whose ends are one place, "draw a circle instead, or open the arc"
    // (sketch_gate.rs), naming the arc.
    let arc = r#"{"id": "na000001", "center": "pz000002", "start": "pa000002", "end": "pa000001", "construction": false}"#;
    let text = document(&[sketch(
        "s0000001",
        &[
            point("pa000001", 10.0, 0.0),
            point("pa000002", 10.0, 1.5e-7),
            point("pa000003", 10.0, 7.5e-8),
            point("pz000001", 20.0, 10.0),
            point("pz000002", 0.0, 0.0),
        ],
        &[line("na000002", "pa000003", "pz000001", false)],
        &[arc.to_owned()],
        "",
    )]);
    let checked = opened(&text).check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    assert_eq!(
        said(&checked),
        [(
            WarningCode::EndsNotJoined,
            "s0000001".to_owned(),
            strings(&["pa000001", "pa000002", "pa000003", "na000001", "na000002"]),
            Some(at_most("1.5e-7")),
        )]
    );
    assert_eq!(
        checked.warnings.listed()[0].suggestion(),
        Some("for arc na000001, draw a circle instead, or open the arc")
    );
}

#[test]
fn a_cluster_with_a_line_and_an_arc_both_collapsing_gives_each_its_advice_lines_first() {
    // The arc above, its ends pa000001 (10, 0) and pa000002 (10, 1.5e-7), now na000009; line
    // na000001 from pa000003 (10, 7.5e-8) to pa000004 (10.00000015, 7.5e-8), 1.5e-7 long,
    // both its ends in the cluster (pa000004 is 7.5e-8 from pa000005); line na000002 from
    // pa000005 (10.000000075, 7.5e-8), 7.5e-8 from each of na000001's ends, to pz000001.
    // Sharing one point would collapse both the arc and na000001: na000001 is to be
    // lengthened, then the arc is given the gate's advice, and no advice to share a point,
    // which would collapse them.
    let arc = r#"{"id": "na000009", "center": "pz000002", "start": "pa000002", "end": "pa000001", "construction": false}"#;
    let text = document(&[sketch(
        "s0000001",
        &[
            point("pa000001", 10.0, 0.0),
            point("pa000002", 10.0, 1.5e-7),
            point("pa000003", 10.0, 7.5e-8),
            point("pa000004", 10.00000015, 7.5e-8),
            point("pa000005", 10.000000075, 7.5e-8),
            point("pz000001", 20.0, 10.0),
            point("pz000002", 0.0, 0.0),
        ],
        &[
            line("na000001", "pa000003", "pa000004", false),
            line("na000002", "pa000005", "pz000001", false),
        ],
        &[arc.to_owned()],
        "",
    )]);
    let checked = opened(&text).check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    let [warning] = checked.warnings.listed() else {
        panic!("{:?}", said(&checked));
    };
    assert_eq!(
        warning.elements(),
        strings(&[
            "pa000001", "pa000002", "pa000003", "pa000004", "pa000005", "na000001", "na000002",
            "na000009"
        ])
    );
    assert_eq!(
        warning.suggestion(),
        Some("lengthen line na000001; for arc na000009, draw a circle instead, or open the arc")
    );
}

/// FreeCAD's `CreateThreeLinesWithCommonPoint` (TestSketcherSolver.py): three lines, the
/// first two ending and the third starting at (−55.600571, −9.387639); `shared`, at one
/// point, pa000002; otherwise each at its own point there, pa000002, pa000004, pa000005.
fn three_lines(shared: bool) -> String {
    let (b, c) = if shared {
        ("pa000002", "pa000002")
    } else {
        ("pa000004", "pa000005")
    };
    let mut points = vec![
        point("pa000001", -55.965607, -9.864289),
        point("pa000002", -55.600571, -9.387639),
        point("pa000003", -55.735817, -9.067246),
        point("pa000006", -55.058266, -9.677831),
    ];
    if !shared {
        points.push(point("pa000004", -55.600571, -9.387639));
        points.push(point("pa000005", -55.600571, -9.387639));
    }
    document(&[sketch(
        "s0000001",
        &points,
        &[
            line("na000001", "pa000001", "pa000002", false),
            line("na000002", "pa000003", b, false),
            line("na000003", c, "pa000006", false),
        ],
        &[],
        "",
    )])
}

#[test]
fn freecads_three_lines_with_coincidences_warn_of_nothing_and_without_them_once() {
    // testThreeLinesWithCoincidences_1 and _2: the three ends made coincident, FreeCAD finds
    // no missing coincidence (detectMissingPointOnPointConstraints 0); here, one point the
    // three curves share: no warning.
    let checked = opened(&three_lines(true)).check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    assert!(checked.warnings.is_empty(), "{:?}", said(&checked));
    // Silico's own case, not FreeCAD's tests': the same ends without the coincidences, for
    // which FreeCAD 1.1.3, run headless, finds 2 missing (k − 1 for k = 3); here the three
    // points at one place are one cluster: one warning naming them, then the three curves.
    let checked = opened(&three_lines(false)).check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    assert_eq!(
        said(&checked),
        [(
            WarningCode::EndsNotJoined,
            "s0000001".to_owned(),
            strings(&[
                "pa000002", "pa000004", "pa000005", "na000001", "na000002", "na000003"
            ]),
            Some(at_most("0")),
        )]
    );
}

#[test]
fn freecads_single_missing_coincidence_is_one_warning() {
    // TestSketchValidateCoincidents.testSingleMissingCoincidentCase: a rectangle drawn from
    // (−47.680691, 18.824165000000004) down, across, and up to (51.132679, 18.824165), then
    // back along y = 18.824165 to (−47.680691, 18.824165), one ulp of 2^-48 short of where
    // it started (FreeCAD's coincidence of the third line's end and the fourth's start is a
    // point they share here, pa000004). FreeCAD 1.1.3 finds 1 missing coincidence, the
    // points 3.552713678800501e-15 apart.
    // 2^-48, a double's spacing between 16 and 32.
    let ulp = 1.0 / 281_474_976_710_656.0_f64;
    assert_eq!(18.824165000000004_f64 - 18.824165, ulp);
    assert_eq!(ulp, 3.552713678800501e-15);
    let horizontal =
        |k: &str, l: &str| format!(r#"{{"id": "{k}", "type": "horizontal", "line": "{l}"}}"#);
    let vertical =
        |k: &str, l: &str| format!(r#"{{"id": "{k}", "type": "vertical", "line": "{l}"}}"#);
    let text = document(&[sketch(
        "s0000001",
        &[
            point("pa000001", -47.680691, 18.824165000000004),
            point("pa000002", -47.680691, -27.346279),
            point("pa000003", 51.132679, -27.346279),
            point("pa000004", 51.132679, 18.824165),
            point("pa000005", -47.680691, 18.824165),
        ],
        &[
            line("na000001", "pa000001", "pa000002", false),
            line("na000002", "pa000002", "pa000003", false),
            line("na000003", "pa000003", "pa000004", false),
            line("na000004", "pa000004", "pa000005", false),
        ],
        &[],
        &[
            vertical("k0000001", "na000001"),
            vertical("k0000002", "na000003"),
            horizontal("k0000003", "na000002"),
            horizontal("k0000004", "na000004"),
        ]
        .join(", "),
    )]);
    let checked = opened(&text).check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    assert_eq!(
        said(&checked),
        [(
            WarningCode::EndsNotJoined,
            "s0000001".to_owned(),
            strings(&["pa000001", "pa000005", "na000001", "na000004"]),
            Some(apart("3.552713678800501e-15")),
        )]
    );
}

#[test]
fn warnings_are_in_timeline_order_then_by_code_then_by_cluster() {
    // Sketch s0000001: two clusters of two points (a: pa000001 near pa000004, b: pa000002
    // near pa000003; by their least IDs, b (2) comes after a (1)), drawn with b's at the
    // lesser x, and two redundant horizontals; then a box with no fingerprint; then sketch
    // s0000003 with one cluster.
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
    assert_eq!(
        said(&checked),
        [
            (
                WarningCode::EndsNotJoined,
                "s0000001".to_owned(),
                strings(&["pa000001", "pa000004", "na000001", "na000003"]),
                Some(apart("6e-8")),
            ),
            (
                WarningCode::EndsNotJoined,
                "s0000001".to_owned(),
                strings(&["pa000002", "pa000003", "na000001", "na000002"]),
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
                strings(&["pa000002", "pa000003", "na000001", "na000002"]),
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

/// The port review's sketch: line na000001 from (−5, 0) to (5, 0), its ends held by
/// distances from the origin (k0000004 to k0000007), and point pa000003 at (0, 0), held at
/// the line's midpoint (k0000001), at y 0 (k0000002), and on the line (k0000003). The
/// midpoint's own equation along y repeats the other two; its equation along the line
/// doesn't.
fn midpoint_held_twice() -> String {
    let constraints = [
        r#"{"id": "k0000001", "type": "midpoint", "point": "pa000003", "line": "na000001"}"#,
        r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "pa000003", "value": 0, "driving": true}"#,
        r#"{"id": "k0000003", "type": "on", "point": "pa000003", "curve": "na000001"}"#,
        r#"{"id": "k0000004", "type": "distance_x", "a": "origin", "b": "pa000001", "value": -5, "driving": true}"#,
        r#"{"id": "k0000005", "type": "distance_y", "a": "origin", "b": "pa000001", "value": 0, "driving": true}"#,
        r#"{"id": "k0000006", "type": "distance_x", "a": "origin", "b": "pa000002", "value": 5, "driving": true}"#,
        r#"{"id": "k0000007", "type": "distance_y", "a": "origin", "b": "pa000002", "value": 0, "driving": true}"#,
    ];
    sketch(
        "s0000001",
        &[
            point("pa000001", -5.0, 0.0),
            point("pa000002", 5.0, 0.0),
            point("pa000003", 0.0, 0.0),
        ],
        &[line("na000001", "pa000001", "pa000002", false)],
        &[],
        &constraints.join(", "),
    )
}

#[test]
fn freecads_partially_redundant_midpoint_is_warned_of_without_advice_to_remove_it() {
    // FreeCAD 1.1.3, the same sketch (its constraints in the same order): solve() 0,
    // RedundantConstraints [], PartiallyRedundantConstraints [1] (the midpoint), fully
    // constrained. The fake solver answers as PlaneGCS does (silico-sketch's document tests
    // solve it through the worker): the midpoint, tag 1, partially redundant; nothing
    // redundant; no freedom left. One warning: the partial one, naming the midpoint.
    let f = opened(&document(&[midpoint_held_twice()]));
    f.sketches.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        solved.partially_redundant = vec![Tag(1)];
        solved.dofs = 0;
        solved.free = Vec::new();
        Ok(solved)
    });
    let checked = f.check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    let [warning] = checked.warnings.listed() else {
        panic!("{:?}", said(&checked));
    };
    assert_eq!(warning.code(), WarningCode::ConstraintsRedundant);
    assert_eq!(warning.elements(), ["k0000001"]);
    assert_eq!(
        warning.message(),
        "a constraint of the sketch is partly redundant: only part of what it fixes is \
         repeated by its other constraints, and that part agrees with them"
    );
    assert!(
        !warning.suggestion().unwrap().contains("remove the"),
        "{warning}"
    );
    assert_eq!(checked.warnings.total(), 1);
}

#[test]
fn a_feature_a_transactions_edits_reached_isnt_warned_of_for_its_fingerprint_its_commit_writes() {
    // k7f2qx3m, saved with no fingerprint, warns. In a transaction, box m0000002 is added:
    // format is refused there, and the commit writes its fingerprint (decision 28), so it
    // doesn't warn, now or after; k7f2qx3m, which no edit reached, warns until format.
    let f = opened(&document(&[block("k7f2qx3m", 0)]));
    let only_k7 = [(
        WarningCode::FingerprintMissing,
        "k7f2qx3m".to_owned(),
        vec![],
        None,
    )];
    assert_eq!(said(&f.check()), only_k7);
    answer(f.sessions.begin(&s1())).result.unwrap();
    let revision = f.sessions.status(&s1()).session.unwrap().revision;
    let added: Node = silico_core::document::json::read(block("m0000002", 10).as_bytes()).unwrap();
    answer(f.sessions.add_feature(&s1(), revision, added))
        .result
        .unwrap();
    let added_at = f.sessions.status(&s1()).session.unwrap().revision;
    assert_ne!(added_at, revision);
    let checked = f.check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    assert_eq!(said(&checked), only_k7);
    // The published model is the edit's revision's too (the check above waited for its
    // rebuild): a published check spares m0000002 as well.
    assert_eq!(said(&published_check(&f, added_at)), only_k7);
    let committed = answer(f.sessions.commit(&s1())).result.unwrap();
    assert_eq!(committed.refingerprinted, [id("m0000002")]);
    assert_eq!(said(&f.check()), only_k7);
    answer(f.sessions.format(&s1(), None, true)).result.unwrap();
    assert!(f.check().warnings.is_empty());
}

/// A feature carried from a commit that couldn't fingerprint it, built since: in a
/// transaction, box m0000002 (7 long) is added while the kernel refuses it, so its commit
/// writes the file with no fingerprint for it, which stays carried (decision 28); then the
/// kernel makes it, and a fresh check builds it, warning of k7f2qx3m and m0000002. Gives the
/// fixture and those two warnings.
fn carried_and_built() -> (Fixture, [Said; 2]) {
    let refusing = Arc::new(AtomicBool::new(true));
    let f = opened(&document(&[block("k7f2qx3m", 0)]));
    let refuses = Arc::clone(&refusing);
    f.kernel.refuse(move |op| {
        (refuses.load(Ordering::SeqCst)
            && matches!(op, Op::MakePrimitive(Primitive::Box { dx, .. }) if *dx == 7.0))
        .then(|| Error::new(ErrorCode::KernelFailed, "the box can't be made"))
    });
    let k7 = (
        WarningCode::FingerprintMissing,
        "k7f2qx3m".to_owned(),
        vec![],
        None,
    );
    answer(f.sessions.begin(&s1())).result.unwrap();
    let revision = f.sessions.status(&s1()).session.unwrap().revision;
    let added = r#"{"id": "m0000002", "type": "box", "v": 1, "name": "Block", "position": [10, 0, 0], "length": 7, "width": 2, "height": 1}"#;
    let added: Node = silico_core::document::json::read(added.as_bytes()).unwrap();
    answer(f.sessions.add_feature(&s1(), revision, added))
        .result
        .unwrap();
    let committed = answer(f.sessions.commit(&s1())).result.unwrap();
    assert!(committed.written);
    assert!(committed.refingerprinted.is_empty(), "{committed:?}");
    let checked = f.check();
    assert_eq!(checked.problems.total(), 1, "{:?}", checked.problems);
    assert_eq!(checked.problems.errors()[0].feature(), Some("m0000002"));
    assert_eq!(said(&checked), std::slice::from_ref(&k7));
    refusing.store(false, Ordering::SeqCst);
    let fresh = CheckModel::Current { fresh: true };
    let checked = answer(f.sessions.check(&s1(), fresh)).result.unwrap();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    let m2 = (
        WarningCode::FingerprintMissing,
        "m0000002".to_owned(),
        vec![],
        None,
    );
    assert_eq!(said(&checked), [k7.clone(), m2.clone()]);
    (f, [k7, m2])
}

/// The published check of s1, its model's revision asserted to be `revision`.
fn published_check(f: &Fixture, revision: Revision) -> CheckAnswer {
    let published = answer(f.sessions.check(&s1(), CheckModel::Published));
    assert_eq!(published.model.unwrap().revision, revision);
    published.result.unwrap()
}

#[test]
fn a_feature_carried_from_a_commit_that_couldnt_fingerprint_it_warns_once_it_builds_outside_a_transaction()
 {
    // Outside a transaction nothing writes the carried feature's fingerprint until the next
    // save, and format can, so it warns, in the current model's check and the published
    // one's; format stores it, and none warns.
    let (f, [k7, m2]) = carried_and_built();
    let published = answer(f.sessions.check(&s1(), CheckModel::Published));
    assert_eq!(said(&published.result.unwrap()), [k7, m2]);
    answer(f.sessions.format(&s1(), None, true)).result.unwrap();
    assert!(f.check().warnings.is_empty());
}

#[test]
fn a_carried_feature_isnt_warned_of_in_a_published_check_once_a_transaction_begins() {
    // Found by the review (A3-1): right after begin, the published model is the one rebuilt
    // outside the transaction. Whether a commit writes the carried feature's record is the
    // session's now: one is open, format is refused, and the commit writes it, so neither
    // check warns of m0000002 (the published one did, advising format).
    let (f, [k7, _]) = carried_and_built();
    answer(f.sessions.begin(&s1())).result.unwrap();
    let revision = f.sessions.status(&s1()).session.unwrap().revision;
    assert_eq!(said(&f.check()), std::slice::from_ref(&k7));
    assert_eq!(
        said(&published_check(&f, revision)),
        std::slice::from_ref(&k7)
    );
}

#[test]
fn a_carried_feature_is_warned_of_in_a_published_check_after_a_commit_that_writes_nothing() {
    // Found by the review (A3-1): in a transaction, box m0000003 is added and checked, then
    // removed and checked, so the published model is the transaction's; its commit writes
    // nothing. Outside the transaction now, format writes the carried feature's record, so
    // both checks warn of m0000002 (the published one didn't).
    let (f, [k7, m2]) = carried_and_built();
    answer(f.sessions.begin(&s1())).result.unwrap();
    let revision = f.sessions.status(&s1()).session.unwrap().revision;
    let added: Node = silico_core::document::json::read(block("m0000003", 20).as_bytes()).unwrap();
    answer(f.sessions.add_feature(&s1(), revision, added))
        .result
        .unwrap();
    assert_eq!(said(&f.check()), std::slice::from_ref(&k7));
    let revision = f.sessions.status(&s1()).session.unwrap().revision;
    answer(f.sessions.remove_feature(&s1(), revision, id("m0000003")))
        .result
        .unwrap();
    assert_eq!(said(&f.check()), std::slice::from_ref(&k7));
    let committed = answer(f.sessions.commit(&s1())).result.unwrap();
    assert!(!committed.written, "{committed:?}");
    let revision = f.sessions.status(&s1()).session.unwrap().revision;
    let both = [k7, m2];
    assert_eq!(said(&f.check()), both);
    assert_eq!(said(&published_check(&f, revision)), both);
}

#[test]
fn a_feature_whose_solid_couldnt_be_measured_isnt_warned_of_for_its_fingerprint() {
    // The kernel refuses every Summarize: the box built, but couldn't be measured (a
    // problem), so a save has no fingerprint to store: no warning.
    let f = opened(&document(&[block("k7f2qx3m", 0)]));
    f.kernel.refuse(|op| {
        matches!(op, Op::Summarize { .. })
            .then(|| Error::new(ErrorCode::KernelFailed, "Summarize failed"))
    });
    let checked = answer(f.sessions.check(&s1(), CheckModel::Current { fresh: true }))
        .result
        .unwrap();
    assert_eq!(checked.problems.total(), 1, "{:?}", checked.problems);
    assert_eq!(checked.problems.errors()[0].feature(), Some("k7f2qx3m"));
    assert!(checked.warnings.is_empty(), "{:?}", said(&checked));
}

#[test]
fn fingerprint_missing_on_the_wire_names_its_parameter() {
    let checked = opened(&document(&[block("k7f2qx3m", 0)])).check();
    let [warning] = checked.warnings.listed() else {
        panic!("{:?}", said(&checked));
    };
    assert_eq!(
        write(&warning.to_wire(), Layout::Canonical).unwrap(),
        "{\"code\": \"fingerprint_missing\", \"message\": \"k7f2qx3m built, but its file \
         stores no fingerprint, so a change in what it builds isn't caught as drift\", \
         \"feature\": \"k7f2qx3m\", \"elements\": [], \"elements_total\": 0, \"parameter\": \
         \"fingerprint\", \"cause\": null, \"suggestion\": \"format the document to store \
         it\", \"candidates\": []}"
    );
}
