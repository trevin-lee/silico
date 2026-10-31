//! `sketch_remove`, `sketch_add`, `sketch_split`, and `sketch_join` under a stored name, on
//! the real kernel
//! (docs/design/m5.0-app.md, section 7, *What a result lists*): a fillet on
//! tests/golden/plate-with-boss.silico (read, never written) stores
//! `p0000001.pad.Edge(G s0000001.sketch.Edge.na000001)`, a name made from the plate
//! outline's line `na000001`. A line removed and another added with a fresh ID leaves that
//! name finding nothing, said as `reference_broken` and never re-pointed; a line added with
//! the removed one's ID is that member to every stored name, as a `replace_text` keeping
//! the ID is. Sketches are solved as stored (none has a constraint), by the fake solver.
//!
//! Expected values by hand: the plate is 60 x 30 x 8, about the origin; its two holes, of
//! radius 3 and 4 at y = 0, run through it; the boss, a cylinder of radius 5 from z = 8 to
//! 14, stands on its centre, so the part before the fillet is its own mirror in y = 0, its
//! centre of mass at y = 0. A fillet of radius 1 along a 60 mm edge of the plate takes
//! (1 - pi/4) 60 mm^3, whose centroid is (10 - 3 pi) / (3 (4 - pi)) mm in from each face
//! it rounds; so the part's centre of mass moves off y = 0, away from the edge, by that
//! volume times its centroid's y over what's left. On the y = -15 edge, which `na000001`
//! (from (-30, -15) to (30, -15)) sweeps, it moves to +y; on the y = +15 edge, to the same
//! distance in -y. In x the part isn't its own mirror: the hole at x = -15 is of radius 3,
//! the one at x = +15 of radius 4, so its centre of mass is at x = 15 pi 8 (9 - 16) over its
//! volume, wherever along y the fillet is (it runs the plate's whole length, its centroid at
//! x = 0).

use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Answered, Call, Sessions};
use silico_core::fake::FakeSketches;
use silico_core::pool::KernelPool;
use silico_core::sketch::SketchSolver;
use tempfile::TempDir;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

/// Long enough for any command here, so one that hangs fails instead.
const HANG: Duration = Duration::from_secs(120);

/// The name the fillet stores.
const NAME: &str = "p0000001.pad.Edge(G s0000001.sketch.Edge.na000001)";

/// The golden part, as committed.
fn golden() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/plate-with-boss.silico");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Sessions on kernel workers, each sketch solved as stored, in a scratch directory.
struct Fixture {
    dir: TempDir,
    sessions: Sessions<KernelPool>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let pool = KernelPool::start_on_demand(Path::new(WORKER), 2).unwrap();
    let sessions = Sessions::with_stand_ins(
        pool,
        move |cache: &Path| Cache::with_secret(cache, &secret),
        || Ok(Box::new(FakeSketches::new()) as Box<dyn SketchSolver>),
    );
    Fixture { dir, sessions }
}

impl Fixture {
    fn path(&self, file: &str) -> PathBuf {
        self.dir.path().join(file)
    }

    /// `text` written as `file` and opened: the session's ID.
    fn open(&self, file: &str, text: &str) -> String {
        std::fs::write(self.path(file), text).unwrap();
        let opened = self.call(
            "open",
            &json!({"path": self.path(file).to_str().unwrap(), "create": false}).to_string(),
        );
        assert_eq!(opened["ok"], true, "{opened}");
        opened["session"]["id"].as_str().unwrap().to_owned()
    }

    /// The envelope the command `name` answers to `arguments`, as a transport has it.
    fn call(&self, name: &str, arguments: &str) -> Value {
        let answer: Answered = match self.sessions.call(name, arguments) {
            Call::Running(calling) => calling
                .wait_timeout(HANG)
                .unwrap_or_else(|_| panic!("{name} didn't answer in {HANG:?}")),
            Call::Unknown => panic!("no command {name}"),
        };
        serde_json::from_str(&answer.text).unwrap()
    }

    /// The result of a command that ran.
    fn ran(&self, name: &str, arguments: &str) -> Value {
        let envelope = self.call(name, arguments);
        assert_eq!(envelope["ok"], true, "{name}: {envelope}");
        envelope["result"].clone()
    }

    /// The current model's problems.
    fn problems(&self, session: &str) -> Vec<Value> {
        self.ran(
            "check",
            &json!({"session": session, "model": "current", "fresh": false}).to_string(),
        )["problems"]
            .as_array()
            .unwrap()
            .clone()
    }

    /// The fillet's solid's volume and centre of mass, in the current model.
    fn fillet(&self, session: &str) -> (f64, [f64; 3]) {
        let measured = self.ran(
            "mass_properties",
            &json!({"session": session, "model": "current", "feature": "f0000001"}).to_string(),
        );
        let body = &measured["bodies"][0];
        assert_eq!(body["feature"], "f0000001", "{measured}");
        let at = |i: usize| body["center_of_mass"][i].as_f64().unwrap();
        (body["volume"].as_f64().unwrap(), [at(0), at(1), at(2)])
    }
}

/// A `sketch_remove` of the plate outline's `members`.
fn remove(session: &str, revision: u64, members: &[&str]) -> String {
    json!({"session": session, "revision": revision, "sketch": "s0000001", "members": members})
        .to_string()
}

/// A `sketch_add` of `lines` (each a line's JSON) to the plate outline.
fn add_lines(session: &str, revision: u64, lines: &str) -> String {
    format!(
        r#"{{"session": "{session}", "revision": {revision}, "sketch": "s0000001", "points": [], "lines": [{lines}], "circles": [], "arcs": [], "constraints": []}}"#
    )
}

/// The fillet's `edges`, as `text` stores them.
fn stored_edges(text: &str) -> Value {
    let document: Value = serde_json::from_str(text).unwrap();
    document["features"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "f0000001")
        .unwrap()["edges"]
        .clone()
}

/// The fillet's volume and its centre of mass's x and y, with the fillet on the plate's
/// edge at y = `edge` (-15 or 15), by hand (the module's note).
fn by_hand(edge: f64) -> (f64, f64, f64) {
    let part = 60.0 * 30.0 * 8.0 - PI * (9.0 + 16.0) * 8.0 + PI * 25.0 * 6.0;
    let taken = (1.0 - PI / 4.0) * 60.0;
    let inset = (10.0 - 3.0 * PI) / (3.0 * (4.0 - PI));
    let centroid = edge - edge.signum() * inset;
    let volume = part - taken;
    let x = 15.0 * PI * 8.0 * (9.0 - 16.0) / volume;
    (volume, x, -taken * centroid / volume)
}

#[test]
fn the_golden_fillet_rounds_the_edge_its_line_sweeps() {
    // The check the tests below lean on: unedited, the name finds the y = -15 edge.
    let f = fixture();
    let s = f.open("plate.silico", &golden());
    assert_eq!(f.problems(&s), Vec::<Value>::new());
    let (volume, at) = f.fillet(&s);
    let (want, x, y) = by_hand(-15.0);
    assert!((volume - want).abs() < 1e-5, "{volume} vs {want}");
    assert!(y > 0.013 && y < 0.014, "{y}");
    assert!((at[1] - y).abs() < 1e-5, "{at:?} vs {y}");
    assert!((at[0] - x).abs() < 1e-5, "{at:?} vs {x}");
}

#[test]
fn a_line_removed_and_another_added_with_a_fresh_id_breaks_the_name_never_re_pointing_it() {
    // The new line runs where the old one did, but it isn't the old one: the name, made from
    // na000001, finds nothing (`reference_broken`, at the fillet's `edges.0`, naming it), and
    // the save keeps the name as it was.
    let f = fixture();
    let s = f.open("plate.silico", &golden());
    assert_eq!(
        f.ran("begin", &json!({"session": s}).to_string())["base"],
        1
    );
    f.ran("sketch_remove", &remove(&s, 1, &["na000001"]));
    let added = f.ran(
        "sketch_add",
        &add_lines(
            &s,
            2,
            r#"{"id": "na00000z", "from": "pa000001", "to": "pa000002", "construction": false}"#,
        ),
    );
    assert_eq!(added["added"], json!(["na00000z"]));
    let problems = f.problems(&s);
    let broken: Vec<&Value> = problems
        .iter()
        .filter(|p| p["feature"] == "f0000001")
        .collect();
    let [broken] = broken.as_slice() else {
        panic!("{problems:?}")
    };
    assert_eq!(
        (
            &broken["code"],
            &broken["parameter"],
            &broken["elements"],
            &broken["elements_total"]
        ),
        (
            &json!("reference_broken"),
            &json!("edges.0"),
            &json!([NAME]),
            &json!(1)
        ),
        "{broken}"
    );
    f.ran(
        "commit",
        &json!({"session": s, "path": null, "replace": false}).to_string(),
    );
    let saved = std::fs::read_to_string(f.path("plate.silico")).unwrap();
    assert_eq!(stored_edges(&saved), json!([NAME]));
    assert_eq!(stored_edges(&saved), stored_edges(&golden()));
    assert!(saved.contains(r#"{"id": "na00000z", "from": "pa000001", "to": "pa000002""#));
}

#[test]
fn a_line_re_added_with_a_removed_ones_id_is_that_line_to_the_name_as_a_replace_text_keeping_it_is()
{
    // The design's section 7 (`sketch_add`, decided 2026-10-29): the saved file is the
    // truth, so an ID removed and added again is the writer's choice of identity, the same
    // document as a `replace_text` that moves the line under its ID. na000001 re-added across the top,
    // from (30, 15) to (-30, 15), where na000003 ran (removed, and its place taken by
    // na00000y along the bottom): the name now finds the y = +15 edge, by either route.
    let f = fixture();
    let a = f.open("a.silico", &golden());
    f.ran("begin", &json!({"session": a}).to_string());
    f.ran("sketch_remove", &remove(&a, 1, &["na000003", "na000001"]));
    f.ran(
        "sketch_add",
        &add_lines(
            &a,
            2,
            r#"{"id": "na000001", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na00000y", "from": "pa000001", "to": "pa000002", "construction": false}"#,
        ),
    );
    assert_eq!(f.problems(&a), Vec::<Value>::new());
    f.ran(
        "commit",
        &json!({"session": a, "path": null, "replace": false}).to_string(),
    );

    // The same document by its text: s0000001's lines (the first in the file; the boss's
    // sketch holds lines of the same IDs, untouched) with na000001 moved and na000003's
    // place taken by na00000y.
    let golden = golden();
    let line = |id: &str, from: &str, to: &str| {
        format!(r#"{{"id": "{id}", "from": "{from}", "to": "{to}", "construction": false}}"#)
    };
    let before = [
        line("na000001", "pa000001", "pa000002"),
        line("na000002", "pa000002", "pa000003"),
        line("na000003", "pa000003", "pa000004"),
        line("na000004", "pa000004", "pa000001"),
    ]
    .join(",\n        ");
    let after = [
        line("na000001", "pa000003", "pa000004"),
        line("na000002", "pa000002", "pa000003"),
        line("na000004", "pa000004", "pa000001"),
        line("na00000y", "pa000001", "pa000002"),
    ]
    .join(",\n        ");
    assert!(golden.find(&before).unwrap() < golden.find(r#""id": "s0000003""#).unwrap());
    let text = golden.replacen(&before, &after, 1);
    assert_ne!(text, golden);
    let b = f.open("b.silico", &golden);
    f.ran(
        "replace_text",
        &json!({"session": b, "revision": 1, "text": text}).to_string(),
    );
    assert_eq!(f.problems(&b), Vec::<Value>::new());

    // One document, byte for byte; one solid; the name finding the y = +15 edge in each.
    let saved_a = std::fs::read_to_string(f.path("a.silico")).unwrap();
    let saved_b = std::fs::read_to_string(f.path("b.silico")).unwrap();
    assert_eq!(saved_a, saved_b);
    assert_eq!(stored_edges(&saved_a), json!([NAME]));
    let (want, x, y) = by_hand(15.0);
    for session in [&a, &b] {
        let (volume, at) = f.fillet(session);
        assert!(
            (volume - want).abs() < 1e-5,
            "{session}: {volume} vs {want}"
        );
        assert!(y < -0.013 && y > -0.014, "{y}");
        assert!((at[1] - y).abs() < 1e-5, "{session}: {at:?} vs {y}");
        assert!((at[0] - x).abs() < 1e-5, "{session}: {at:?} vs {x}");
    }
    assert_eq!(f.fillet(&a), f.fillet(&b));
}

/// The pad's element the fillet's name finds, in the current model: its start and its end.
fn named_edge(f: &Fixture, session: &str) -> ([f64; 3], [f64; 3]) {
    edge_named(f, session, NAME)
}

/// The pad's element `name` finds, in the current model: its start and its end.
fn edge_named(f: &Fixture, session: &str, name: &str) -> ([f64; 3], [f64; 3]) {
    let listed = f.ran(
        "elements",
        &json!({"session": session, "model": "current", "feature": "p0000001", "after": null, "limit": 1000}).to_string(),
    );
    assert_eq!(listed["next"], Value::Null, "{listed}");
    let found: Vec<&Value> = listed["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["name"] == name || e["aliases"].as_array().unwrap().iter().any(|a| a == name))
        .collect();
    let [edge] = found.as_slice() else {
        panic!("{name}: {found:?}")
    };
    assert_eq!(edge["kind"], "edge", "{edge}");
    let point = |key: &str| {
        let at = |i: usize| edge[key][i].as_f64().unwrap();
        [at(0), at(1), at(2)]
    };
    (point("start"), point("end"))
}

#[test]
fn a_split_lists_the_fillet_whose_name_its_line_reaches_and_the_name_then_finds_the_start_side_piece()
 {
    // docs/design/m5.0-app.md, open item 11: na000001, from (-30, -15) to (30, -15), split
    // at (0, -15): the start-side piece keeps its ID, so the name made from it finds the
    // edge that piece sweeps, from x = -30 to 0, half of what it found; the split's
    // `references` lists the fillet, and the name stored is as it was (never re-pointed).
    let f = fixture();
    let s = f.open("plate.silico", &golden());
    let (start, end) = named_edge(&f, &s);
    let mut xs = [start[0], end[0]];
    xs.sort_by(f64::total_cmp);
    assert_eq!(xs, [-30.0, 30.0], "{start:?} {end:?}");
    f.ran("begin", &json!({"session": s}).to_string());
    let split = f.ran(
        "sketch_split",
        &json!({"session": s, "revision": 1, "sketch": "s0000001", "curve": "na000001", "at": [[0, -15]], "ids": ["pa000005", "na000005", "ka000001"]}).to_string(),
    );
    assert_eq!(
        split,
        json!({"sketch": "s0000001", "pieces": ["na000001", "na000005"], "added": ["pa000005", "na000005", "ka000001"], "modified": ["na000001"], "removed": [], "references": ["f0000001"], "changed": true, "commit": null})
    );
    assert_eq!(f.problems(&s), Vec::<Value>::new());
    let (start, end) = named_edge(&f, &s);
    let mut xs = [start[0], end[0]];
    xs.sort_by(f64::total_cmp);
    assert_eq!(xs, [-30.0, 0.0], "{start:?} {end:?}");
    assert_eq!((start[1], end[1]), (-15.0, -15.0));
    assert_eq!(start[2], end[2]);
    f.ran(
        "commit",
        &json!({"session": s, "path": null, "replace": false}).to_string(),
    );
    let saved = std::fs::read_to_string(f.path("plate.silico")).unwrap();
    assert_eq!(stored_edges(&saved), json!([NAME]));
}

/// The text of `session`'s document, as it holds it.
fn text_of(f: &Fixture, session: &str) -> String {
    f.ran(
        "text",
        &json!({"session": session, "feature": null}).to_string(),
    )["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A split of na000001 at (0, -15), whose IDs are given: the kept piece na000001 from
/// (-30, -15) to pa000005, the other, na000005, on to (30, -15), held by ka000001.
fn split_at_the_middle(f: &Fixture, session: &str) {
    f.ran(
        "sketch_split",
        &json!({"session": session, "revision": 1, "sketch": "s0000001", "curve": "na000001", "at": [[0, -15]], "ids": ["pa000005", "na000005", "ka000001"]}).to_string(),
    );
}

/// Its join at the split point, the result checked: na000001 whole again, na000005, the
/// point, and the tangent_at gone, and the fillet listed.
fn joined_at_the_middle(f: &Fixture, session: &str) {
    let joined = f.ran(
        "sketch_join",
        &json!({"session": session, "revision": 2, "sketch": "s0000001", "point": "pa000005"})
            .to_string(),
    );
    assert_eq!(
        joined,
        json!({"sketch": "s0000001", "added": [], "modified": ["na000001"], "removed": ["pa000005", "na000005", "ka000001"], "references": ["f0000001"], "changed": true, "commit": null})
    );
}

#[test]
fn a_name_the_kept_piece_made_finds_the_whole_edge_again_after_the_join() {
    // The split leaves the name made from na000001 finding the edge from x = -30 to 0; the
    // join makes na000001 run from (-30, -15) to (30, -15) again, so the name finds the
    // whole edge, x from -30 to 30, and the fillet rounds the whole of it: the volume and
    // centre of mass by hand, as unedited (by_hand(-15)), and the text as before the split.
    let f = fixture();
    let s = f.open("plate.silico", &golden());
    f.ran("begin", &json!({"session": s}).to_string());
    let before = text_of(&f, &s);
    split_at_the_middle(&f, &s);
    assert_eq!(f.problems(&s), Vec::<Value>::new());
    let (start, end) = named_edge(&f, &s);
    let mut xs = [start[0], end[0]];
    xs.sort_by(f64::total_cmp);
    assert_eq!(xs, [-30.0, 0.0], "{start:?} {end:?}");
    joined_at_the_middle(&f, &s);
    assert_eq!(f.problems(&s), Vec::<Value>::new());
    assert_eq!(text_of(&f, &s), before);
    let (start, end) = named_edge(&f, &s);
    let mut xs = [start[0], end[0]];
    xs.sort_by(f64::total_cmp);
    assert_eq!(xs, [-30.0, 30.0], "{start:?} {end:?}");
    assert_eq!((start[1], end[1]), (-15.0, -15.0));
    assert_eq!(start[2], end[2]);
    let (volume, at) = f.fillet(&s);
    let (want, x, y) = by_hand(-15.0);
    assert!((volume - want).abs() < 1e-5, "{volume} vs {want}");
    assert!((at[1] - y).abs() < 1e-5, "{at:?} vs {y}");
    assert!((at[0] - x).abs() < 1e-5, "{at:?} vs {x}");
}

#[test]
fn a_name_the_dropped_piece_made_is_broken_by_the_join_never_re_pointed() {
    // The fillet stores a name made from na000005, the piece the split makes and the join
    // drops. Before the split it finds nothing; after it, the edge na000005 sweeps, x from
    // 0 to 30; after the join, nothing again: `reference_broken` at the fillet's `edges.0`,
    // naming it, and the name the save keeps is the one stored, never re-pointed to the
    // joined line's edge.
    const DROPPED: &str = "p0000001.pad.Edge(G s0000001.sketch.Edge.na000005)";
    let golden = golden();
    let text = golden.replacen(NAME, DROPPED, 1);
    assert_ne!(text, golden);
    let f = fixture();
    let s = f.open("plate.silico", &text);
    let broken = |f: &Fixture| {
        let problems = f.problems(&s);
        let broken: Vec<&Value> = problems
            .iter()
            .filter(|p| p["feature"] == "f0000001")
            .collect();
        let [broken] = broken.as_slice() else {
            panic!("{problems:?}")
        };
        assert_eq!(
            (
                &broken["code"],
                &broken["parameter"],
                &broken["elements"],
                &broken["elements_total"]
            ),
            (
                &json!("reference_broken"),
                &json!("edges.0"),
                &json!([DROPPED]),
                &json!(1)
            ),
            "{broken}"
        );
    };
    broken(&f);
    f.ran("begin", &json!({"session": s}).to_string());
    split_at_the_middle(&f, &s);
    assert_eq!(f.problems(&s), Vec::<Value>::new());
    let (start, end) = edge_named(&f, &s, DROPPED);
    let mut xs = [start[0], end[0]];
    xs.sort_by(f64::total_cmp);
    assert_eq!(xs, [0.0, 30.0], "{start:?} {end:?}");
    joined_at_the_middle(&f, &s);
    broken(&f);
    f.ran(
        "commit",
        &json!({"session": s, "path": null, "replace": false}).to_string(),
    );
    let saved = std::fs::read_to_string(f.path("plate.silico")).unwrap();
    assert_eq!(stored_edges(&saved), json!([DROPPED]));
}
