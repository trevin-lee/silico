//! `sketch_split`, `sketch_trim`, `sketch_unjoin`, and `sketch_merge` on PlaneGCS
//! (docs/design/m5.0-app.md, section 7, *The sketch edits*, and section 13's SD7 tests):
//! through the sessions' own command called by name, as a transport calls it
//! (`Sessions::call`), on the fake kernel, each sketch solved by this crate's worker. The
//! rule they hold to, SD7's for an edit that writes geometry: the sketch's solve of what the
//! edit wrote judges it, and a conflict refuses it as data, the session as it was; a merge is
//! refused only where its solve is worse than the one before it (FreeCAD main's
//! `shouldRejectCoincidentSolve`, `cb2780e2fa`).
//!
//! Expected values by hand: the bracket's L profile is fully defined, so a second length on
//! its base line, `na000001`, at 61 mm where `ka000009` holds it at 60, conflicts, PlaneGCS
//! naming the pair (as silico-sketch's tests/session_sketch_edits.rs pins it). A split,
//! trim, or unjoin of the profile so over-defined leaves those two lengths of the base line
//! (or, split, of its outer ends) in conflict; a merge that leaves the same conflict as
//! before it is no worse. Two lines each starting at a point locked at the origin
//! (`distance_x` and `distance_y` of 0 from it), merged there, hold the one point twice
//! over: the second lock repeats the first, a redundancy the solve before didn't have
//! (1.1.3's `merge_two_locked_ends`, which main's rollback refuses).

use super::bracket::{P0000001, S0000001, S0000002};

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Answered, Call, Sessions};
use silico_core::fake::FakeKernel;
use silico_core::sketch::{SketchSolver, SketchWorker};
use tempfile::TempDir;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

/// Long enough for any command here, so one that hangs fails instead.
const HANG: Duration = Duration::from_secs(60);

/// The second length of the L profile's base line, 61 mm where `ka000009` holds 60.
const SECOND_LENGTH: &str =
    r#"{"id": "ka00000d", "type": "length", "line": "na000001", "value": 61, "driving": true}"#;

/// Sessions on a fake kernel, each sketch solved by the sketch worker, with `text` open as
/// s1.
struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn opened(text: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sessions = Sessions::with_stand_ins(
        kernel,
        |cache: &Path| Cache::unsealed(cache),
        || {
            SketchWorker::start_worker(Path::new(WORKER))
                .map(|worker| Box::new(worker) as Box<dyn SketchSolver>)
        },
    );
    let f = Fixture { dir, sessions };
    std::fs::write(f.path(), text).unwrap();
    let opened = f.call(
        "open",
        &json!({"path": f.path().to_str().unwrap(), "create": false}).to_string(),
    );
    assert_eq!(opened["ok"], true, "{opened}");
    f
}

/// The bracket's L profile, its pad, and the base's holes.
fn bracket() -> Fixture {
    opened(&document(&[S0000001, P0000001, S0000002]))
}

fn document(features: &[&str]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

impl Fixture {
    fn path(&self) -> PathBuf {
        self.dir.path().join("bracket.silico")
    }

    fn call(&self, name: &str, arguments: &str) -> Value {
        let answer: Answered = match self.sessions.call(name, arguments) {
            Call::Running(calling) => calling
                .wait_timeout(HANG)
                .unwrap_or_else(|_| panic!("{name} didn't answer in {HANG:?}")),
            Call::Unknown => panic!("no command {name}"),
        };
        let envelope: Value = serde_json::from_str(&answer.text).unwrap();
        assert_eq!(envelope["ok"], answer.ok, "{}", answer.text);
        envelope
    }

    fn ran(&self, name: &str, arguments: &str) -> Value {
        let envelope = self.call(name, arguments);
        assert_eq!(envelope["ok"], true, "{envelope}");
        envelope["result"].clone()
    }

    fn refused(&self, name: &str, arguments: &str) -> Value {
        let envelope = self.call(name, arguments);
        assert_eq!(envelope["ok"], false, "{envelope}");
        envelope["errors"].clone()
    }

    fn revision(&self) -> u64 {
        self.sessions
            .status(&silico_core::command::SessionId::parse("s1").unwrap())
            .session
            .unwrap()
            .revision
            .number()
    }

    fn text(&self) -> String {
        self.ran("text", r#"{"session": "s1", "feature": null}"#)["text"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// The L profile's second length added, applied though it over-defines (SD7).
    fn over_define(&self) {
        let added = self.call(
            "sketch_add",
            &format!(
                r#"{{"session": "s1", "revision": {}, "sketch": "s0000001", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": [{SECOND_LENGTH}]}}"#,
                self.revision()
            ),
        );
        assert_eq!(added["ok"], true, "{added}");
    }

    /// The current model's problems, by code and feature, in order.
    fn problems(&self) -> Vec<(String, Option<String>)> {
        let checked = self.ran(
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
        );
        checked["problems"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["code"].as_str().unwrap().to_owned(),
                    p["feature"].as_str().map(str::to_owned),
                )
            })
            .collect()
    }
}

/// Why an edit whose solve is worse than before is refused.
const WORSE: &str = "an edit that writes geometry is refused where the sketch's solve after it is worse than before it (a conflict the sketch had already isn't the edit's), as FreeCAD's Coincident command rolls one back";

#[test]
fn an_edit_far_from_a_conflict_the_sketch_had_is_made_inside_a_transaction_and_out() {
    // The over-defined L (dofs -1 on PlaneGCS: nothing free, the base's two lengths in
    // conflict): a split of the leg's top, a trim of the upright's inside edge (nothing cuts
    // it but its neighbours: removed whole), and an unjoin at the inside corner each leave
    // the conflict as it was or lighter, never worse, so each is made (SD7, refined: a
    // conflict the sketch had already isn't the edit's).
    for inside in [true, false] {
        for (command, request) in [
            (
                "sketch_split",
                json!({"session": "s1", "sketch": "s0000001", "curve": "na000003", "at": [[30, 5]], "ids": null}),
            ),
            (
                "sketch_trim",
                json!({"session": "s1", "sketch": "s0000001", "curve": "na000004", "pick": [5, 30], "ids": null}),
            ),
            (
                "sketch_unjoin",
                json!({"session": "s1", "sketch": "s0000001", "point": "pa000004", "curve": "na000004", "ids": null}),
            ),
        ] {
            let f = bracket();
            assert_eq!(f.problems(), []);
            if inside {
                f.ran("begin", r#"{"session": "s1"}"#);
            }
            f.over_define();
            let mut request = request;
            request["revision"] = json!(f.revision());
            let made = f.ran(command, &request.to_string());
            assert_eq!(made["changed"], true, "{command}, inside: {inside}");
            // The sketch still conflicts, as it did: the base's two lengths.
            let problems = f.problems();
            assert!(
                problems.contains(&("over_constrained".to_owned(), Some("s0000001".to_owned()))),
                "{command}: {problems:?}"
            );
        }
    }
}

#[test]
fn a_split_of_the_over_defined_base_line_makes_both_lengths_distances_of_its_outer_ends_which_still_conflict()
 {
    // Split, each length of the base line becomes a distance of its outer ends, under its
    // ID (§3's table): 60 and 61 between pa000001 and pa000002, which can't both hold, the
    // same pair as before the split. The rank is the same, and nothing new is flagged, so the
    // split is made (SD7, refined: a conflict the sketch had already isn't the edit's). The
    // pair is matched by ID, which the retype keeps; FreeCAD's rollback matches flagged
    // constraints by their content (`getConstraintDiagnosticIdentity`: type, geometry,
    // positions, value), by which a length retyped to a distance would be a new one.
    let f = bracket();
    f.ran("begin", r#"{"session": "s1"}"#);
    f.over_define();
    let at = f.revision();
    let made = f.ran(
        "sketch_split",
        &json!({"session": "s1", "revision": at, "sketch": "s0000001", "curve": "na000001", "at": [[30, 0]], "ids": ["pa000007", "na000007", "ka00000e"]}).to_string(),
    );
    assert_eq!(
        made["modified"],
        json!(["na000001", "ka000009", "ka00000d"])
    );
    let document: Value = serde_json::from_str(&f.text()).unwrap();
    let constraints = document["features"][0]["constraints"].as_array().unwrap();
    for (id, value) in [("ka000009", 60), ("ka00000d", 61)] {
        let constraint = constraints.iter().find(|c| c["id"] == id).unwrap();
        assert_eq!(
            constraint,
            &json!({"id": id, "type": "distance", "a": "pa000001", "b": "pa000002", "value": value, "driving": true})
        );
    }
    let checked = f.ran(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    let conflict = checked["problems"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["feature"] == "s0000001")
        .unwrap()
        .clone();
    assert_eq!(
        (&conflict["code"], &conflict["elements"]),
        (&json!("over_constrained"), &json!(["ka000009", "ka00000d"])),
        "{checked}"
    );
}

#[test]
fn a_merge_that_takes_a_conflicting_sketch_past_its_last_freedom_is_refused_as_main_rolls_it_back()
{
    // The review's case, reproduced on FreeCAD 1.1.3: the L, its inside corner unjoined, a
    // second length of 61 on the base line, then the corner merged back. Before: 14 unknowns,
    // rank 12, dofs 2, the lengths in conflict (FreeCAD's ConflictingConstraints, 2). After:
    // 12 unknowns, nothing free, dofs -1 (Overconstrained, 3): worse, and refused.
    let f = bracket();
    f.ran("begin", r#"{"session": "s1"}"#);
    let unjoined = f.ran(
        "sketch_unjoin",
        &json!({"session": "s1", "revision": 1, "sketch": "s0000001", "point": "pa000004", "curve": "na000004", "ids": ["pa000008"]}).to_string(),
    );
    assert_eq!(unjoined["added"], json!(["pa000008"]));
    f.over_define();
    let read = f.ran(
        "sketch",
        r#"{"session": "s1", "model": "current", "sketch": "s0000001"}"#,
    );
    assert_eq!(read["dofs"], 2, "{read}");
    let text = f.text();
    let refused = f.refused(
        "sketch_merge",
        &json!({"session": "s1", "revision": 3, "sketch": "s0000001", "points": ["pa000004", "pa000008"]}).to_string(),
    );
    assert_eq!(
        refused,
        json!([{
            "code": "over_constrained",
            "message": "after the merge, the sketch's constraints conflict: these 2 can't all hold",
            "feature": "s0000001",
            "elements": ["ka000009", "ka00000d"],
            "elements_total": 2,
            "parameter": null,
            "cause": WORSE,
            "suggestion": "remove or change one of the constraints named",
            "candidates": [],
            "advice": null
        }])
    );
    assert_eq!((f.revision(), f.text()), (3, text));
}

#[test]
fn a_merge_that_leaves_a_conflict_the_sketch_had_with_freedom_both_sides_is_no_worse_and_is_made() {
    // The same unjoined, over-defined L, and two free lines apart from it whose ends touch
    // (q2 and q3, at one place): merged, the conflict is the one the sketch had, its rank
    // the same (dofs 2 + 8 before, 2 + 6 after, both free), so the merge is made.
    let f = bracket();
    f.ran("begin", r#"{"session": "s1"}"#);
    f.ran(
        "sketch_unjoin",
        &json!({"session": "s1", "revision": 1, "sketch": "s0000001", "point": "pa000004", "curve": "na000004", "ids": ["pa000008"]}).to_string(),
    );
    f.over_define();
    let added = f.ran(
        "sketch_add",
        r#"{"session": "s1", "revision": 3, "sketch": "s0000001", "points": [{"id": "qa000001", "x": 100, "y": 0, "construction": false}, {"id": "qa000002", "x": 110, "y": 0, "construction": false}, {"id": "qa000003", "x": 110, "y": 0, "construction": false}, {"id": "qa000004", "x": 110, "y": 10, "construction": false}], "lines": [{"id": "nq000001", "from": "qa000001", "to": "qa000002", "construction": false}, {"id": "nq000002", "from": "qa000003", "to": "qa000004", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#,
    );
    assert_eq!(added["changed"], true);
    let merged = f.ran(
        "sketch_merge",
        &json!({"session": "s1", "revision": 4, "sketch": "s0000001", "points": ["qa000002", "qa000003"]}).to_string(),
    );
    assert_eq!(
        merged,
        json!({"sketch": "s0000001", "added": [], "modified": ["nq000002"], "removed": ["qa000003"], "changed": true, "commit": null})
    );
}

#[test]
fn freecads_conflicting_coincident_selection_is_refused_as_a_merge_the_text_as_it_was() {
    // TestCoincidentCommandGui.py's test_conflicting_coincident_selection_is_rejected
    // (FreeCAD main, cb2780e2fa): two construction lines, (0, 0)-(10, 0) and (20, 0)-(30, 0),
    // the first's end held at x = 10, the second's start at x = 20; their coincidence
    // conflicts, so main's command adds nothing. As a merge, the first's end kept: the two
    // distance_x of one point, 10 and 20, can't both hold.
    let sketch = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Two", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": true}, {"id": "pa000002", "x": 10, "y": 0, "construction": true}, {"id": "pa000003", "x": 20, "y": 0, "construction": true}, {"id": "pa000004", "x": 30, "y": 0, "construction": true}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": true}, {"id": "na000002", "from": "pa000003", "to": "pa000004", "construction": true}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "distance_x", "a": "origin", "b": "pa000002", "value": 10, "driving": true}, {"id": "ka000002", "type": "distance_x", "a": "origin", "b": "pa000003", "value": 20, "driving": true}]}"#;
    let f = opened(&document(&[sketch]));
    assert_eq!(f.problems(), []);
    f.ran("begin", r#"{"session": "s1"}"#);
    let text = f.text();
    let refused = f.refused(
        "sketch_merge",
        &json!({"session": "s1", "revision": 1, "sketch": "s0000001", "points": ["pa000002", "pa000003"]}).to_string(),
    );
    assert_eq!(refused[0]["code"], "over_constrained", "{refused}");
    assert_eq!(refused[0]["elements"], json!(["ka000001", "ka000002"]));
    assert_eq!((f.revision(), f.text()), (1, text));
    assert_eq!(f.problems(), []);
}

#[test]
fn freecads_failed_tangent_substitution_is_refused_as_a_merge_the_text_as_it_was() {
    // TestCoincidentCommandGui.py's test_failed_tangent_substitution_is_rolled_back: a line
    // from (-10, 0) to (10, 0), held horizontal, its start at (-10, 0), 20 long; an arc of
    // radius 5 about (0, 5), its centre held at x = 0, edge tangent to the line (on its
    // left). Merging the line's end with the arc's start makes the tangency an end tangency
    // there (open item 13), which can't hold with the line's end 10 from the centre's line:
    // main's command rolls the substitution back. As a merge, the line's end kept: refused,
    // the text as it was. FreeCAD's fixture also holds the arc's ends at x = 5 cos 0.5 and
    // 5 cos 2.5 (its DistanceX of the arc's start and end): here as distance_x of the ends
    // from the origin, each value the nearest nanometre a document holds (4.387913 and
    // -4.005718), the ends drawn exactly there.
    let start = [4.387913, 5.0 + libm::sqrt(25.0 - 4.387913 * 4.387913)];
    let end = [-4.005718, 5.0 + libm::sqrt(25.0 - 4.005718 * 4.005718)];
    let sketch = format!(
        r#"{{"id": "s0000001", "type": "sketch", "v": 1, "name": "Tangent", "plane": "xy", "points": [{{"id": "pa000001", "x": -10, "y": 0, "construction": false}}, {{"id": "pa000002", "x": 10, "y": 0, "construction": false}}, {{"id": "pa000003", "x": 0, "y": 5, "construction": false}}, {{"id": "pa000004", "x": {}, "y": {}, "construction": false}}, {{"id": "pa000005", "x": {}, "y": {}, "construction": false}}], "lines": [{{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}}], "circles": [], "arcs": [{{"id": "aa000001", "center": "pa000003", "start": "pa000004", "end": "pa000005", "construction": false}}], "constraints": [{{"id": "ka000001", "type": "horizontal", "line": "na000001"}}, {{"id": "ka000002", "type": "distance_x", "a": "origin", "b": "pa000001", "value": -10, "driving": true}}, {{"id": "ka000003", "type": "distance_y", "a": "origin", "b": "pa000001", "value": 0, "driving": true}}, {{"id": "ka000004", "type": "length", "line": "na000001", "value": 20, "driving": true}}, {{"id": "ka000005", "type": "distance_x", "a": "origin", "b": "pa000003", "value": 0, "driving": true}}, {{"id": "ka000006", "type": "radius", "curve": "aa000001", "value": 5, "driving": true}}, {{"id": "ka000007", "type": "tangent", "a": "na000001", "b": "aa000001", "side": "left"}}, {{"id": "ka000008", "type": "distance_x", "a": "origin", "b": "pa000004", "value": 4.387913, "driving": true}}, {{"id": "ka000009", "type": "distance_x", "a": "origin", "b": "pa000005", "value": -4.005718, "driving": true}}]}}"#,
        start[0], start[1], end[0], end[1]
    );
    let f = opened(&document(&[&sketch]));
    let checked = f.ran(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    let sketch_problems: Vec<&Value> = checked["problems"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["code"] != "drift")
        .collect();
    assert!(sketch_problems.is_empty(), "{checked}");
    f.ran("begin", r#"{"session": "s1"}"#);
    let text = f.text();
    let refused = f.refused(
        "sketch_merge",
        &json!({"session": "s1", "revision": 1, "sketch": "s0000001", "points": ["pa000002", "pa000004"]}).to_string(),
    );
    // The tangent, now at the shared end (ka000007, kept under its ID), with the line's
    // holds, the centre's, the radius, and the arc start's hold, now on the line's end
    // (ka000008): seven that can't all hold. The arc end's hold (ka000009) isn't among them.
    assert_eq!(refused[0]["code"], "over_constrained", "{refused}");
    assert_eq!(
        refused[0]["elements"],
        json!([
            "ka000001", "ka000002", "ka000004", "ka000005", "ka000006", "ka000007", "ka000008"
        ])
    );
    assert_eq!(refused[0]["cause"], WORSE);
    assert_eq!((f.revision(), f.text()), (1, text));
}

/// Two lines, `na` from `pa` (at the origin) to (10, 0) and `nb` from `pb` (at the origin)
/// to (0, 10), each start locked at the origin by its own `distance_x` and `distance_y` of 0,
/// and each line's far end fixed by a horizontal or vertical and a length: fully defined,
/// nothing redundant, as long as `pa` and `pb` are two points.
fn two_locked_ends() -> String {
    let sketch = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Locked ends", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 10, "y": 0, "construction": false}, {"id": "pb000001", "x": 0, "y": 0, "construction": false}, {"id": "pb000002", "x": 0, "y": 10, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "nb000001", "from": "pb000001", "to": "pb000002", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "ka000001", "type": "distance_x", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000002", "type": "distance_y", "a": "origin", "b": "pa000001", "value": 0, "driving": true}, {"id": "ka000003", "type": "horizontal", "line": "na000001"}, {"id": "ka000004", "type": "length", "line": "na000001", "value": 10, "driving": true}, {"id": "kb000001", "type": "distance_x", "a": "origin", "b": "pb000001", "value": 0, "driving": true}, {"id": "kb000002", "type": "distance_y", "a": "origin", "b": "pb000001", "value": 0, "driving": true}, {"id": "kb000003", "type": "vertical", "line": "nb000001"}, {"id": "kb000004", "type": "length", "line": "nb000001", "value": 10, "driving": true}]}"#;
    document(&[sketch])
}

#[test]
fn a_merge_of_two_locked_ends_makes_a_lock_redundant_and_is_refused_as_main_rolls_it_back() {
    let f = opened(&two_locked_ends());
    assert_eq!(f.problems(), []);
    f.ran("begin", r#"{"session": "s1"}"#);
    let text = f.text();
    let refused = f.refused(
        "sketch_merge",
        &json!({"session": "s1", "revision": 1, "sketch": "s0000001", "points": ["pa000001", "pb000001"]}).to_string(),
    );
    let [error] = refused.as_array().unwrap().as_slice() else {
        panic!("{refused}")
    };
    // One lock's two constraints repeat the other's: PlaneGCS flags the second lock, kb's,
    // as FreeCAD 1.1.3's solve lists the second lock redundant (`merge_two_locked_ends`).
    let elements = json!(["kb000001", "kb000002"]);
    assert_eq!(
        error,
        &json!({
            "code": "over_constrained",
            "message": "after the merge, these 2 constraints repeat others, as the sketch's solve before it found none of them do",
            "feature": "s0000001",
            "elements": elements,
            "elements_total": 2,
            "parameter": null,
            "cause": WORSE,
            "suggestion": "remove the constraints named, then make the merge again",
            "candidates": [],
            "advice": null
        })
    );
    assert_eq!((f.revision(), f.text()), (1, text));
    // One lock removed first, the merge is made: the shared point held by the other, the
    // sketch fully defined (by hand: 3 points, 6 unknowns; the lock's 2, the horizontal and
    // the vertical 1 each, the lengths 1 each).
    f.ran(
        "sketch_remove",
        &json!({"session": "s1", "revision": 1, "sketch": "s0000001", "members": ["kb000001", "kb000002"]}).to_string(),
    );
    let merged = f.ran(
        "sketch_merge",
        &json!({"session": "s1", "revision": 2, "sketch": "s0000001", "points": ["pa000001", "pb000001"]}).to_string(),
    );
    assert_eq!(merged["removed"], json!(["pb000001"]));
    let read = f.ran(
        "sketch",
        r#"{"session": "s1", "model": "current", "sketch": "s0000001"}"#,
    );
    assert_eq!(
        (&read["state"], &read["dofs"]),
        (&json!("fully_defined"), &json!(0)),
        "{read}"
    );
}

#[test]
fn a_merge_of_touching_ends_solves_and_its_points_are_one() {
    // The ends ends_not_joined names, 4e-8 mm apart, merged with the least ID kept: the
    // profile closes, and check warns no more.
    let profile = S0000001
        .replacen(
            r#"{"id": "pa000006", "x": 0, "y": 50, "construction": false}"#,
            r#"{"id": "pa000006", "x": 0, "y": 50, "construction": false}, {"id": "pa000007", "x": 5, "y": 5.00000004, "construction": false}"#,
            1,
        )
        .replacen(
            r#"{"id": "na000004", "from": "pa000004", "to": "pa000005", "construction": false}"#,
            r#"{"id": "na000004", "from": "pa000007", "to": "pa000005", "construction": false}"#,
            1,
        );
    let f = opened(&document(&[&profile, P0000001]));
    let checked = f.ran(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    let warned: Vec<&Value> = checked["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["code"] == "ends_not_joined")
        .collect();
    assert_eq!(warned.len(), 1, "{checked}");
    assert_eq!(warned[0]["elements"], json!(["pa000004", "pa000007"]));
    let merged = f.ran(
        "sketch_merge",
        &json!({"session": "s1", "revision": 1, "sketch": "s0000001", "points": ["pa000004", "pa000007"]}).to_string(),
    );
    assert_eq!(merged["removed"], json!(["pa000007"]));
    assert_eq!(merged["commit"]["written"], true);
    let checked = f.ran(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    assert!(
        !checked["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "ends_not_joined"),
        "{checked}"
    );
    assert_eq!(checked["problems"], json!([]), "{checked}");
}

#[test]
fn a_split_then_a_join_solves_cleanly_both_ways_and_gives_the_sketch_back() {
    // On PlaneGCS, inside a transaction. The bracket's L, fully defined (dofs 0), split at
    // the middle of na000003: one freedom more, the split point's slide along the line,
    // nothing conflicting or redundant; joined at the split point, fully defined again, the
    // sketch as it was before the split. The base's hole cb000001 (radius 2.75 about
    // (40, 12), its diameter held) split at (42.75, 12) and (37.25, 12): its two points'
    // slides round the circle free (dofs 2); joined at the second, the arc ending there
    // keeps the circle's ID, and the circle is as it was, fully defined.
    let f = bracket();
    f.ran("begin", r#"{"session": "s1"}"#);
    let read = |sketch: &str| {
        f.ran(
            "sketch",
            &json!({"session": "s1", "model": "current", "sketch": sketch}).to_string(),
        )
    };
    let members = |read: &Value| {
        let mut read = read.clone();
        read.as_object_mut().unwrap().remove("dofs");
        read.as_object_mut().unwrap().remove("state");
        read
    };
    for (sketch, curve, at, point, freed) in [
        ("s0000001", "na000003", json!([[30, 5]]), "pa000007", 1),
        (
            "s0000002",
            "cb000001",
            json!([[42.75, 12], [37.25, 12]]),
            "pb000004",
            2,
        ),
    ] {
        let before = read(sketch);
        assert_eq!(
            (&before["state"], &before["dofs"]),
            (&json!("fully_defined"), &json!(0))
        );
        let ids = if freed == 1 {
            json!(["pa000007", "na000007", "ka00000d"])
        } else {
            json!(["pb000003", "pb000004", "cb000003"])
        };
        let split = f.ran(
            "sketch_split",
            &json!({"session": "s1", "revision": f.revision(), "sketch": sketch, "curve": curve, "at": at, "ids": ids}).to_string(),
        );
        assert_eq!(split["changed"], true, "{split}");
        let split_read = read(sketch);
        assert_eq!(
            (&split_read["state"], &split_read["dofs"]),
            (&json!("under_defined"), &json!(freed)),
            "{split_read}"
        );
        for constraint in split_read["constraints"].as_array().unwrap() {
            assert_eq!(constraint["state"], "satisfied", "{constraint}");
        }
        assert_eq!(f.problems(), [], "{sketch}");
        let joined = f.ran(
            "sketch_join",
            &json!({"session": "s1", "revision": f.revision(), "sketch": sketch, "point": point})
                .to_string(),
        );
        assert_eq!(joined["modified"][0], curve, "{joined}");
        assert_eq!(joined.get("curve"), None, "{joined}");
        let after = read(sketch);
        assert_eq!(
            (&after["state"], &after["dofs"]),
            (&json!("fully_defined"), &json!(0))
        );
        assert_eq!(members(&after), members(&before), "{sketch}");
        assert_eq!(f.problems(), [], "{sketch}");
    }
}
