//! break-it for `check`'s warnings (`silico_core::warnings`): sketches at FreeCAD's joining
//! distance from both sides, a kilometre out, clusters of coincident ends, a hub of hundreds
//! of curves, a sketch at the unknowns' limit, thousands of redundant constraints, a solve
//! that moves ends together, and documents with problems and warnings at once; and a
//! property test of the clusters and the first hundred against a brute-force reference.
//!
//! Expected values are independent of the code under test:
//! - FreeCAD 1.1.3 (the oracle's, on OCCT 7.9.3) as the oracle: each sketch of two lines
//!   built with `Part.LineSegment` and `Sketcher::SketchObject`, recomputed, and its
//!   `Shape.Vertexes` counted (3: the ends joined; 4: not), on 2026-10-26. Within ulps of
//!   1e-7 FreeCAD's verdict comes from the vertices it builds, not the points; there the
//!   tests pin Silico's rule, the points' distance, and record FreeCAD's.
//! - By hand: a kilometre out a double's spacing is 2^-33 mm (1.16e-10), so 1e6 − 6e-8 is
//!   515 spacings below 1e6 (5.995389074087143e-8 apart) and 1e6 − 1e-7 is 859
//!   (1.00000761449337e-7 apart).
//! - The error contract's convention (`profile.rs`, `sweep.rs`, `constraints_redundant`): a
//!   list of elements is its first `PROBLEMS_LISTED`, and `elements_total` counts them all.
//! - The property test's reference is every pair of ends compared directly, √(dx² + dy²)
//!   < 1e-7 (the design's rule), closed transitively into clusters, sorted by their least
//!   point ID: no sweep, no union-find, no selection.

use std::sync::Arc;
use std::time::{Duration, Instant};

use proptest::prelude::*;
use silico_core::cache::Cache;
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::command::{CheckAnswer, CheckModel, SessionId, Sessions};
use silico_core::document::PROBLEMS_LISTED;
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::sketch::SketchSolver;
use silico_core::{ErrorCode, WarningCode};
use silico_protocol::Tag;
use tempfile::TempDir;

use crate::sessions::answer;

struct Fixture {
    _dir: TempDir,
    sketches: FakeSketches,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn s1() -> SessionId {
    SessionId::parse("s1").unwrap()
}

/// The fake solver's answer to each sketch.
type Answer = Box<
    dyn Fn(&silico_protocol::SketchSystem) -> Result<silico_protocol::Solved, silico_core::Error>
        + Send
        + Sync,
>;

/// Sessions on the fake kernel and solver, with `text` open as s1; the solver answering
/// with `answer`'s, if given, from the first solve.
fn opened_with(text: &str, answer_with: Option<Answer>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = FakeSketches::new();
    if let Some(f) = answer_with {
        sketches.answer(f);
    }
    let solver = sketches.clone();
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    );
    let path = dir.path().join("part.silico");
    std::fs::write(&path, text).unwrap();
    let opened = answer(sessions.open(&path, false));
    assert_eq!(opened.session.unwrap().id, s1());
    Fixture {
        _dir: dir,
        sketches,
        sessions,
    }
}

fn opened(text: &str) -> Fixture {
    opened_with(text, None)
}

impl Fixture {
    fn check(&self) -> CheckAnswer {
        let checked = answer(
            self.sessions
                .check(&s1(), CheckModel::Current { fresh: false }),
        );
        assert_eq!(checked.model.and_then(|model| model.fault), None);
        checked.result.unwrap()
    }
}

fn document(features: &[String]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

fn point(id: &str, x: f64, y: f64) -> String {
    format!(r#"{{"id": "{id}", "x": {x}, "y": {y}, "construction": false}}"#)
}

fn line(id: &str, from: &str, to: &str) -> String {
    format!(r#"{{"id": "{id}", "from": "{from}", "to": "{to}", "construction": false}}"#)
}

fn sketch(id: &str, points: &[String], lines: &[String], constraints: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{}], "lines": [{}], "circles": [], "arcs": [], "constraints": [{constraints}]}}"#,
        points.join(", "),
        lines.join(", "),
    )
}

/// Line na000001 from `p0` to `a`, and line nb000001 from `b` to `c`: FreeCAD's sketch of
/// the oracle runs (a's end vertex is `p0 + dir · length`, b's start vertex is b exactly).
fn two_lines(p0: (f64, f64), a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> String {
    document(&[sketch(
        "s0000001",
        &[
            point("pa000001", p0.0, p0.1),
            point("pa000002", a.0, a.1),
            point("pa000003", b.0, b.1),
            point("pa000004", c.0, c.1),
        ],
        &[
            line("na000001", "pa000001", "pa000002"),
            line("nb000001", "pa000003", "pa000004"),
        ],
        "",
    )])
}

fn ends_warnings(checked: &CheckAnswer) -> Vec<(Vec<String>, Option<String>)> {
    checked
        .warnings
        .listed()
        .iter()
        .filter(|w| w.code() == WarningCode::EndsNotJoined)
        .map(|w| (w.elements().to_vec(), w.cause().map(str::to_owned)))
        .collect()
}

fn apart(distance: &str) -> String {
    format!("the points are {distance} mm apart, closer than FreeCAD's joining distance of 1e-7 mm")
}

fn at_most(distance: &str) -> String {
    format!(
        "the points are at most {distance} mm apart, each closer than FreeCAD's joining \
         distance of 1e-7 mm to another of them"
    )
}

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| (*s).to_owned()).collect()
}

/// `n` clusters of two points 4e-8 apart, cluster i's at x = 10i: point pa{i} ends line
/// na{i}, out to (10i, 5), and pb{i} ends nb{i}, out to (10i, −5): its points and its lines.
fn two_point_clusters(n: usize) -> (Vec<String>, Vec<String>) {
    let mut points = Vec::new();
    let mut lines = Vec::new();
    for i in 0..n {
        let x = 10.0 * i as f64;
        points.push(point(&format!("pa{i:06}"), x, 0.0));
        points.push(point(&format!("pb{i:06}"), x + 4e-8, 0.0));
        points.push(point(&format!("py{i:06}"), x, -5.0));
        points.push(point(&format!("pz{i:06}"), x, 5.0));
        lines.push(line(
            &format!("na{i:06}"),
            &format!("pa{i:06}"),
            &format!("pz{i:06}"),
        ));
        lines.push(line(
            &format!("nb{i:06}"),
            &format!("pb{i:06}"),
            &format!("py{i:06}"),
        ));
    }
    (points, lines)
}

// ---------------------------------------------------------------------------------------
// At the joining distance, against FreeCAD.

#[test]
fn ends_just_past_1e_7_apart_dont_warn_though_freecads_vertices_join_them() {
    // Found by break-it: FreeCAD joins the vertices of the edges it builds, and a line's end
    // vertex is `start + dir · length` (GC_MakeSegment's trimmed Geom_Line), a few ulps from
    // the `to` point, so FreeCAD 1.1.3 joins these ends (sketch Shape: 3 vertexes) though the
    // points are 1.0000000043308699e-7 apart. Silico measures the points the file holds (as
    // M3.5's profile does, and as a fix moves them), deterministic on every platform where
    // FreeCAD's vertices aren't (its arithmetic, an FMA or not, differs between them): no
    // warning, by the module's documented rule, within ulps of 1e-7 apart from FreeCAD's.
    let text = two_lines(
        (9.650744846336764, -29.487811607103097),
        (0.08312242246510948, -71.64912734306486),
        (0.08312252246510991, -71.64912734306486),
        (5.08312252246511, -64.64912734306486),
    );
    // √(dx² + dy²) of the points, by hand from the coordinates: dy is 0, dx their difference.
    assert_eq!(
        0.08312252246510991_f64 - 0.08312242246510948,
        1.0000000043308699e-7
    );
    let checked = opened(&text).check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    assert!(
        ends_warnings(&checked).is_empty(),
        "{:?}",
        ends_warnings(&checked)
    );
}

#[test]
fn ends_just_under_1e_7_apart_warn_though_freecads_vertices_dont_join_them() {
    // The converse, found by break-it: the points are 9.999999539545001e-8 apart, which
    // warns by the module's rule, though FreeCAD 1.1.3 doesn't join them (sketch Shape: 4
    // vertexes): the line's end vertex, `p0 + dir · length`, is ulps further from b.
    let text = two_lines(
        (-70.70591843363746, 15.667017604898106),
        (2.730178869006579, 7.899891398153173),
        (2.7301789690065745, 7.899891398153173),
        (7.730178969006575, 14.899891398153173),
    );
    assert_eq!(
        2.7301789690065745_f64 - 2.730178869006579,
        9.999999539545001e-8
    );
    let checked = opened(&text).check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    assert_eq!(
        ends_warnings(&checked),
        [(
            strings(&["pa000002", "pa000003", "na000001", "nb000001"]),
            Some(apart("9.999999539545001e-8"))
        )]
    );
}

#[test]
fn ends_a_kilometre_out_warn_by_their_doubles_spacing() {
    // 515 spacings of 2^-33 below 1e6, on both sides of the origin, along x and along y:
    // FreeCAD joins (3 vertexes; the lines are axis-aligned, so its vertices are the points).
    let km = 1e6;
    let near = 999999.99999994_f64;
    assert_eq!(km - near, 515.0 * (1.0 / 8_589_934_592.0));
    for (p0, a, b, c) in [
        ((km - 10.0, km), (km, km), (near, km), (km - 5.0, km - 7.0)),
        (
            (-km + 10.0, -km),
            (-km, -km),
            (-near, -km),
            (-km + 5.0, -km + 7.0),
        ),
        ((km, km - 10.0), (km, km), (km, near), (km - 5.0, km - 7.0)),
        (
            (-km, 10.0 - km),
            (-km, -km),
            (-km, -near),
            (-km + 5.0, -km + 7.0),
        ),
    ] {
        let checked = opened(&two_lines(p0, a, b, c)).check();
        assert!(checked.problems.is_empty(), "{:?}", checked.problems);
        assert_eq!(
            ends_warnings(&checked),
            [(
                strings(&["pa000002", "pa000003", "na000001", "nb000001"]),
                Some(apart("5.995389074087143e-8"))
            )],
            "{b:?}"
        );
    }
    // 859 spacings: 1.00000761449337e-7 apart; FreeCAD doesn't join (4 vertexes).
    let far = 999999.9999999_f64;
    assert_eq!(km - far, 859.0 * (1.0 / 8_589_934_592.0));
    for (p0, a, b, c) in [
        ((km - 10.0, km), (km, km), (far, km), (km - 5.0, km - 7.0)),
        (
            (-km, 10.0 - km),
            (-km, -km),
            (-km, -far),
            (-km + 5.0, -km + 7.0),
        ),
    ] {
        let checked = opened(&two_lines(p0, a, b, c)).check();
        assert!(checked.problems.is_empty(), "{:?}", checked.problems);
        assert!(ends_warnings(&checked).is_empty(), "{b:?}");
    }
}

#[test]
fn ends_a_kilometre_apart_in_x_with_one_y_never_pair_and_a_column_of_ends_finds_its_one_pair() {
    // 200 lines whose near ends all sit on x = 0 (the sweep's window can't cut them off by
    // x), a millimetre apart in y, and one more whose end is 3e-8 above the 100th: one
    // warning, that pair. And two ends at x = ±1e6 on y = 0: never a pair.
    let mut points = Vec::new();
    let mut lines = Vec::new();
    for i in 0..200 {
        points.push(point(&format!("pa{i:06}"), 0.0, i as f64));
        points.push(point(&format!("pz{i:06}"), 50.0, i as f64));
        lines.push(line(
            &format!("na{i:06}"),
            &format!("pa{i:06}"),
            &format!("pz{i:06}"),
        ));
    }
    points.push(point("pb000000", 0.0, 100.00000003));
    points.push(point("pb000001", -50.0, 100.0));
    lines.push(line("nb000000", "pb000000", "pb000001"));
    points.push(point("pc000000", 1e6, 0.0));
    points.push(point("pc000001", 999990.0, 0.0));
    points.push(point("pc000002", -1e6, 0.0));
    points.push(point("pc000003", -999990.0, 0.0));
    lines.push(line("nc000000", "pc000000", "pc000001"));
    lines.push(line("nc000001", "pc000002", "pc000003"));
    let checked = opened(&document(&[sketch("s0000001", &points, &lines, "")])).check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    let said = ends_warnings(&checked);
    assert_eq!(said.len(), 1, "{said:?}");
    assert_eq!(said[0].0, ["pa000100", "pb000000", "na000100", "nb000000"]);
    // 100.00000003 − 100 as doubles: 3e-8 · 2^46 = 2111062.3 spacings of 2^-46 at 100,
    // so 2111062 · 2^-46.
    assert_eq!(
        100.00000003_f64 - 100.0,
        2111062.0 * (1.0 / 70_368_744_177_664.0)
    );
    assert_eq!(said[0].1, Some(apart("2.999999537678377e-8")));
}

// ---------------------------------------------------------------------------------------
// Many warnings, many elements.

#[test]
fn a_cluster_of_256_ends_at_the_unknowns_limit_is_one_warning_naming_its_first_hundred() {
    // 256 lines, each from its own point at the origin to its own far point: 512 points,
    // 1,024 unknowns, the most a sketch may have. One cluster of 256 points: one warning, its
    // 256 points then 256 curves, the first hundred (all points, pa000000 to pa000099)
    // named, all 512 counted.
    let mut points = Vec::new();
    let mut lines = Vec::new();
    for i in (0..256).rev() {
        points.push(point(&format!("pa{i:06}"), 0.0, 0.0));
        points.push(point(&format!("pz{i:06}"), 10.0, i as f64));
        lines.push(line(
            &format!("na{i:06}"),
            &format!("pa{i:06}"),
            &format!("pz{i:06}"),
        ));
    }
    let text = document(&[sketch("s0000001", &points, &lines, "")]);
    let f = opened(&text);
    let started = Instant::now();
    let checked = f.check();
    let took = started.elapsed();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    assert_eq!(checked.warnings.total(), 1);
    let warning = &checked.warnings.listed()[0];
    let expected: Vec<String> = (0..100).map(|i| format!("pa{i:06}")).collect();
    assert_eq!(warning.elements(), expected);
    assert_eq!(warning.elements_total(), 512);
    assert_eq!(warning.cause(), Some(at_most("0").as_str()));
    assert!(
        warning
            .message()
            .starts_with("256 points that end curves are at one place")
    );
    assert!(took < Duration::from_secs(20), "{took:?}");
    // The wire holds to the snapshot.
    let value: serde_json::Value =
        serde_json::from_str(&write(&checked.to_wire(), Layout::Canonical).unwrap()).unwrap();
    assert_eq!(value["warnings"].as_array().unwrap().len(), 1);
    assert_eq!(
        value["warnings"][0]["elements"].as_array().unwrap().len(),
        100
    );
    assert_eq!(value["warnings"][0]["elements_total"], 512);
    assert_eq!(value["warnings_total"], 1);
}

#[test]
fn a_hub_of_500_curves_near_another_point_lists_its_first_hundred_curves_all_counted() {
    // BREAKS: hub pa000000 at the origin, hub pa000001 4e-8 from it, each with a line to
    // each of 250 far points: one warning, about 2 points and 500 curves. `ends_not_joined`
    // put all 500 in `elements`, where every other list of elements in the contract is its
    // first hundred with `elements_total` counting the rest (constraints_redundant's, the
    // profile's, a sweep's): a star of hubs (15 at one place, 7,000 lines) made 100
    // warnings of ~1,000 elements each on the wire and on the CLI's stderr.
    let mut points = vec![point("pa000000", 0.0, 0.0), point("pa000001", 4e-8, 0.0)];
    let mut lines = Vec::new();
    for i in 0..250 {
        points.push(point(
            &format!("pz{i:06}"),
            100.0 + i as f64,
            50.0 + 2.0 * i as f64,
        ));
        lines.push(line(&format!("na{i:06}"), "pa000000", &format!("pz{i:06}")));
        lines.push(line(&format!("nb{i:06}"), "pa000001", &format!("pz{i:06}")));
    }
    let checked = opened(&document(&[sketch("s0000001", &points, &lines, "")])).check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    // The far points are distinct (none of them pair): one warning, its two points, then
    // the first 98 of the 500 curves by ID (na000000 to na000097), all 502 counted.
    assert_eq!(checked.warnings.total(), 1, "{:?}", ends_warnings(&checked));
    let warning = &checked.warnings.listed()[0];
    let mut expected = strings(&["pa000000", "pa000001"]);
    expected.extend((0..98).map(|i| format!("na{i:06}")));
    assert_eq!(warning.elements(), expected);
    assert_eq!(warning.elements().len(), PROBLEMS_LISTED);
    assert_eq!(warning.elements_total(), 502);
}

#[test]
fn five_thousand_redundant_constraints_are_one_warning_naming_the_first_hundred() {
    // One line held horizontal 5,000 times; the solver keeps the first and calls the rest
    // redundant, each holding: one warning, its first 100 by ID, all 4,999 counted.
    let constraints: Vec<String> = (1..=5000)
        .map(|i| format!(r#"{{"id": "k{i:07}", "type": "horizontal", "line": "na000001"}}"#))
        .collect();
    let text = document(&[sketch(
        "s0000001",
        &[point("pa000001", 0.0, 0.0), point("pa000002", 10.0, 0.0)],
        &[line("na000001", "pa000001", "pa000002")],
        &constraints.join(", "),
    )]);
    let f = opened_with(
        &text,
        Some(Box::new(|system| {
            let mut solved = FakeSketches::as_sent(system);
            let mut tags: Vec<Tag> = system.constraints.iter().filter_map(|c| c.tag()).collect();
            tags.sort_unstable();
            solved.redundant = tags[1..].to_vec();
            Ok(solved)
        })),
    );
    let checked = f.check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    assert_eq!(checked.warnings.total(), 1);
    let w = &checked.warnings.listed()[0];
    assert_eq!(w.code(), WarningCode::ConstraintsRedundant);
    assert_eq!(w.elements_total(), 4999);
    let expected: Vec<String> = (2..=101).map(|i| format!("k{i:07}")).collect();
    assert_eq!(w.elements(), expected);
    assert!(
        w.message()
            .starts_with("4999 constraints of the sketch are redundant")
    );
    // Said once on the CLI's terms: its elements, "and 4899 more".
    assert!(w.to_string().contains("and 4899 more"), "{w}");
    let _ = &f.sketches;
}

#[test]
fn warnings_of_many_features_and_clusters_keep_timeline_order_past_the_hundredth() {
    // A box with no fingerprint, then a sketch of 105 clusters, then another box: 107
    // counted, the first box listed first, then the first 99 clusters; the last box counted,
    // not listed.
    let block = |id: &str, x: u32| {
        format!(
            r#"{{"id": "{id}", "type": "box", "v": 1, "name": "B", "position": [{x}, 0, 0], "length": 4, "width": 2, "height": 1}}"#
        )
    };
    let (points, lines) = two_point_clusters(105);
    let text = document(&[
        block("b0000001", 0),
        sketch("s0000001", &points, &lines, ""),
        block("b0000002", 20),
    ]);
    let checked = opened(&text).check();
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    assert_eq!(checked.warnings.total(), 107);
    let listed = checked.warnings.listed();
    assert_eq!(listed.len(), 100);
    assert_eq!(listed[0].code(), WarningCode::FingerprintMissing);
    assert_eq!(listed[0].feature(), Some("b0000001"));
    assert!(
        listed[1..]
            .iter()
            .all(|w| w.code() == WarningCode::EndsNotJoined)
    );
    assert_eq!(
        listed[99].elements(),
        ["pa000098", "pb000098", "na000098", "nb000098"]
    );
}

// ---------------------------------------------------------------------------------------
// Problems and warnings together; solves that move ends.

#[test]
fn a_pad_refused_for_ends_not_shared_and_its_sketch_warned_of_them() {
    // A triangle whose last side ends 4e-8 from where the first starts: the pad's profile is
    // refused (a problem of the pad), and the sketch is warned of, both naming the ends.
    let text = document(&[
        sketch(
            "s0000001",
            &[
                point("pa000000", 0.0, 0.0),
                point("pb000000", 20.0, 0.0),
                point("pc000000", 0.0, 10.0),
                point("pd000000", 4e-8, 0.0),
            ],
            &[
                line("na000000", "pa000000", "pb000000"),
                line("nb000000", "pb000000", "pc000000"),
                line("nc000000", "pc000000", "pd000000"),
            ],
            "",
        ),
        r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#.to_owned(),
    ]);
    let checked = opened(&text).check();
    assert_eq!(checked.problems.total(), 1, "{:?}", checked.problems);
    assert_eq!(checked.problems.errors()[0].feature(), Some("p0000001"));
    assert_eq!(checked.warnings.total(), 1);
    assert_eq!(
        ends_warnings(&checked),
        [(
            strings(&["pa000000", "pd000000", "na000000", "nc000000"]),
            Some(apart("4e-8"))
        )]
    );
}

/// The fake solver's answer: the sketch as sent, but every value `from` is moved to `to`
/// (none, on a solve that starts from a solution where it's moved already).
fn moved(
    from: f64,
    to: f64,
) -> impl Fn(&silico_protocol::SketchSystem) -> Result<silico_protocol::Solved, silico_core::Error>
+ Send
+ Sync {
    move |system| {
        let mut solved = FakeSketches::as_sent(system);
        for v in &mut solved.values {
            if *v == from {
                *v = to;
            }
        }
        Ok(solved)
    }
}

#[test]
fn a_solve_that_moves_an_end_near_anothers_drifts_and_warns_from_where_it_solved() {
    // Stored 0.75 from the other end; the solve puts it 5e-8 away: drift (a problem), and the
    // warning measures the solved place.
    let text = two_lines((-10.0, 0.0), (0.0, 0.0), (0.75, 0.0), (10.0, 5.0));
    let f = opened_with(&text, Some(Box::new(moved(0.75, 5e-8))));
    let checked = f.check();
    assert!(
        checked
            .problems
            .errors()
            .iter()
            .any(|e| e.code() == ErrorCode::Drift && e.feature() == Some("s0000001")),
        "{:?}",
        checked.problems
    );
    assert_eq!(
        ends_warnings(&checked),
        [(
            strings(&["pa000002", "pa000003", "na000001", "nb000001"]),
            Some(apart("5e-8"))
        )]
    );
}

#[test]
fn a_solve_that_collapses_a_lone_line_never_warns_of_one_curve_with_advice_it_cant_follow() {
    // A line from (0, 0) to (7.25, 0) that the solve shrinks to 5e-8: whatever the core makes
    // of it, a warning that two curves' ends aren't shared must name two curves; "make one
    // curve end at the other's point" can't be followed for one.
    let text = document(&[sketch(
        "s0000001",
        &[point("pa000001", 0.0, 0.0), point("pa000002", 7.25, 0.0)],
        &[line("na000001", "pa000001", "pa000002")],
        "",
    )]);
    let f = opened_with(&text, Some(Box::new(moved(7.25, 5e-8))));
    let checked = f.check();
    // The core refuses the solve as geometry its file can't hold, naming the line.
    assert_eq!(checked.problems.total(), 1, "{:?}", checked.problems);
    assert_eq!(
        checked.problems.errors()[0].code(),
        ErrorCode::InvalidResult
    );
    assert_eq!(checked.problems.errors()[0].feature(), Some("s0000001"));
    // A sketch that didn't solve has no solution to warn of.
    assert!(
        checked.warnings.is_empty(),
        "{:?}; problems {:?}",
        ends_warnings(&checked),
        checked.problems
    );
}

#[test]
fn a_document_the_gate_refuses_has_no_warnings_though_the_same_ends_warn_once_admitted() {
    // Warn, then a refused revision on disk reopened (no model: no warnings), then fixed.
    let good = two_lines((-10.0, 0.0), (0.0, 0.0), (4e-8, 0.0), (10.0, 5.0));
    let f = opened(&good);
    assert_eq!(f.check().warnings.total(), 1);
    let bad = good.replace(r#""x": 10, "y": 5"#, r#""x": 2000000, "y": 5"#);
    assert_ne!(bad, good);
    let g = opened(&bad);
    let checked = g.check();
    assert!(checked.problems.total() >= 1);
    assert!(checked.warnings.is_empty());
    assert_eq!(f.check().warnings.total(), 1);
}

// ---------------------------------------------------------------------------------------
// The sweep and the first hundred against every pair, compared directly.

/// Offsets around the joining distance, each side of it and exactly at it.
const OFFSETS: [f64; 12] = [
    0.0,
    3e-8,
    4e-8,
    5e-8,
    6e-8,
    8e-8,
    9.999999999999998e-8,
    1e-7,
    1.0000000000000002e-7,
    1.5e-7,
    2e-7,
    2.0000000000000004e-7,
];

const CENTRES: [(f64, f64); 4] = [
    (0.0, 0.0),
    (999999.0, -5.0),
    (-999999.9, 999999.9),
    (0.1, 0.2),
];

fn joined(a: (f64, f64), b: (f64, f64)) -> Option<f64> {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    let d = (dx * dx + dy * dy).sqrt();
    (d < 1e-7).then_some(d)
}

/// A distance as a cause writes it.
fn shown(d: f64) -> String {
    if d == 0.0 {
        "0".to_owned()
    } else {
        format!("{d:e}")
    }
}

/// An end of a line in the property test: its point's ID, where it is, and its line's ID.
type End = (String, (f64, f64), String);

/// What `ends_not_joined` should say of `ends`, worked out directly: every pair compared,
/// clusters closed transitively by search, each warning its points then its curves (sorted,
/// each once), the first 100 of them named and all counted, and the largest distance
/// between two of its points; the clusters by their least point ID.
fn clusters_directly(ends: &[End]) -> Vec<(Vec<String>, u64, String)> {
    let n = ends.len();
    let mut cluster_of: Vec<Option<usize>> = vec![None; n];
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for start in 0..n {
        if cluster_of[start].is_some() {
            continue;
        }
        let c = clusters.len();
        cluster_of[start] = Some(c);
        let mut members = vec![start];
        let mut k = 0;
        while k < members.len() {
            let i = members[k];
            for j in 0..n {
                if cluster_of[j].is_none() && joined(ends[i].1, ends[j].1).is_some() {
                    cluster_of[j] = Some(c);
                    members.push(j);
                }
            }
            k += 1;
        }
        clusters.push(members);
    }
    let mut said: Vec<(Vec<String>, u64, String)> = Vec::new();
    for members in clusters.iter().filter(|m| m.len() >= 2) {
        let mut points: Vec<String> = members.iter().map(|&i| ends[i].0.clone()).collect();
        points.sort();
        let mut curves: Vec<String> = members.iter().map(|&i| ends[i].2.clone()).collect();
        curves.sort();
        curves.dedup();
        let mut farthest = 0.0_f64;
        for &i in members {
            for &j in members {
                let (a, b) = (ends[i].1, ends[j].1);
                let (dx, dy) = (a.0 - b.0, a.1 - b.1);
                farthest = farthest.max((dx * dx + dy * dy).sqrt());
            }
        }
        let cause = if points.len() == 2 {
            apart(&shown(farthest))
        } else {
            at_most(&shown(farthest))
        };
        let total = (points.len() + curves.len()) as u64;
        let mut named = points;
        named.extend(curves);
        named.truncate(PROBLEMS_LISTED);
        said.push((named, total, cause));
    }
    said.sort();
    said
}

/// Centres of the property test's second kind of sketch: two grids of 60, one for each end
/// of its lines, 10 mm apart and far from each other, so no line is short.
fn grid(side: usize, k: usize) -> (f64, f64) {
    let x0 = if side == 0 { -2000.0 } else { 2000.0 };
    (x0 + 10.0 * (k % 10) as f64, 10.0 * (k / 10) as f64)
}

fn offset(o: usize, negative: bool) -> f64 {
    if negative { -OFFSETS[o] } else { OFFSETS[o] }
}

fn sketch_of(ends: &[End], lines_of: &[(String, String, String)], rotate: usize) -> String {
    let mut points: Vec<String> = ends
        .iter()
        .map(|(id, (x, y), _)| point(id, *x, *y))
        .collect();
    let mut lines: Vec<String> = lines_of
        .iter()
        .map(|(id, from, to)| line(id, from, to))
        .collect();
    let (pk, lk) = (rotate % points.len(), rotate % lines.len());
    points.rotate_left(pk);
    lines.rotate_left(lk);
    document(&[sketch("s0000001", &points, &lines, "")])
}

/// The listed warnings, each its elements, its total, and its cause, and their total.
fn said_of(checked: &CheckAnswer) -> (Vec<(Vec<String>, u64, String)>, u64) {
    let listed = checked
        .warnings
        .listed()
        .iter()
        .map(|w| {
            (
                w.elements().to_vec(),
                w.elements_total(),
                w.cause().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    (listed, checked.warnings.total())
}

/// The near end of each of a few lines at a few centres, each nudged by offsets around the
/// joining distance; the far ends apart.
fn few() -> impl Strategy<Value = Vec<(usize, usize, bool, usize, bool)>> {
    prop::collection::vec(
        (
            0usize..CENTRES.len(),
            0usize..OFFSETS.len(),
            any::<bool>(),
            0usize..OFFSETS.len(),
            any::<bool>(),
        ),
        2..40,
    )
}

/// Both ends of each of many lines, each at one of 60 centres of its side's grid, nudged
/// along x as `few`'s are along both axes: enough clusters (more than 100 in most cases,
/// by a simulation of this draw) to pass the hundred listed.
fn many() -> impl Strategy<Value = Vec<((usize, usize, bool), (usize, usize, bool))>> {
    let end = (0usize..60, 0usize..OFFSETS.len(), any::<bool>());
    prop::collection::vec((end.clone(), end), 200..=250)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, ..ProptestConfig::default() })]

    #[test]
    fn ends_not_joined_matches_clusters_worked_out_directly(
        ends in few(),
        rotate in 0usize..40,
    ) {
        let mut all: Vec<End> = Vec::new();
        let mut lines = Vec::new();
        for (i, &(c, ox, sx, oy, sy)) in ends.iter().enumerate() {
            let (cx, cy) = CENTRES[c];
            let (near, far, curve) = (format!("pa{i:06}"), format!("pz{i:06}"), format!("na{i:06}"));
            all.push((near.clone(), (cx + offset(ox, sx), cy + offset(oy, sy)), curve.clone()));
            all.push((far.clone(), (500.0 + 3.0 * i as f64, 400.0 + 7.0 * i as f64), curve.clone()));
            lines.push((curve, near, far));
        }
        let checked = opened(&sketch_of(&all, &lines, rotate)).check();
        prop_assert!(checked.problems.is_empty(), "{:?}", checked.problems);
        let expected = clusters_directly(&all);
        let (listed, total) = said_of(&checked);
        prop_assert_eq!(total, expected.len() as u64);
        prop_assert_eq!(listed, expected.into_iter().take(PROBLEMS_LISTED).collect::<Vec<_>>());
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn many_clusters_past_the_hundredth_match_those_worked_out_directly(
        lines_ends in many(),
        rotate in 0usize..500,
    ) {
        let mut all: Vec<End> = Vec::new();
        let mut lines = Vec::new();
        for (i, (a, b)) in lines_ends.iter().enumerate() {
            let curve = format!("na{i:06}");
            let mut ids = Vec::new();
            for (side, &(c, ox, sx)) in [a, b].into_iter().enumerate() {
                let (cx, cy) = grid(side, c);
                let id = format!("p{}{i:06}", ["a", "b"][side]);
                all.push((id.clone(), (cx + offset(ox, sx), cy), curve.clone()));
                ids.push(id);
            }
            lines.push((curve, ids[0].clone(), ids[1].clone()));
        }
        let checked = opened(&sketch_of(&all, &lines, rotate)).check();
        prop_assert!(checked.problems.is_empty(), "{:?}", checked.problems);
        let expected = clusters_directly(&all);
        let (listed, total) = said_of(&checked);
        prop_assert_eq!(total, expected.len() as u64);
        prop_assert_eq!(listed, expected.into_iter().take(PROBLEMS_LISTED).collect::<Vec<_>>());
    }
}

#[test]
fn checks_at_once_current_fresh_and_published_say_the_same_warnings_in_the_same_bytes() {
    // 105 clusters and a box with no fingerprint: 106 warnings, asked for by 8 threads at
    // once, each current, current with fresh, and published, in turns.
    let (mut points, mut lines) = two_point_clusters(105);
    points.reverse();
    lines.reverse();
    let text = document(&[
        sketch("s0000001", &points, &lines, ""),
        r#"{"id": "b0000002", "type": "box", "v": 1, "name": "B", "position": [50, 0, 0], "length": 4, "width": 2, "height": 1}"#.to_owned(),
    ]);
    let f = opened(&text);
    let first = write(&f.check().to_wire(), Layout::Canonical).unwrap();
    let said: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|t| {
                let f = &f;
                scope.spawn(move || {
                    let model = match t % 3 {
                        0 => CheckModel::Current { fresh: false },
                        1 => CheckModel::Current { fresh: true },
                        _ => CheckModel::Published,
                    };
                    let checked = answer(f.sessions.check(&s1(), model)).result.unwrap();
                    write(&checked.to_wire(), Layout::Canonical).unwrap()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for text in &said {
        assert_eq!(text, &first);
    }
    let value: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(value["warnings_total"], 106);
}
