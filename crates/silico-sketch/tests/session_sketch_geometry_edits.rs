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

/// The over-defined profile's conflict, as the edit `edit` ("split") refuses with it.
fn conflict(edit: &str) -> Value {
    json!([{
        "code": "over_constrained",
        "message": format!("after the {edit}, the sketch's constraints conflict: these 2 can't all hold"),
        "feature": "s0000001",
        "elements": ["ka000009", "ka00000d"],
        "elements_total": 2,
        "parameter": null,
        "cause": null,
        "suggestion": "remove or change one of the constraints named",
        "candidates": []
    }])
}

#[test]
fn a_split_trim_or_unjoin_whose_solve_conflicts_is_refused_as_data_inside_a_transaction_and_out() {
    for inside in [true, false] {
        let f = bracket();
        assert_eq!(f.problems(), []);
        if inside {
            f.ran("begin", r#"{"session": "s1"}"#);
        }
        f.over_define();
        let at = f.revision();
        let text = f.text();
        let file = std::fs::read(f.path()).unwrap();
        for (command, request) in [
            (
                "sketch_split",
                json!({"session": "s1", "revision": at, "sketch": "s0000001", "curve": "na000003", "at": [[30, 5]], "ids": null}),
            ),
            (
                // Nothing cuts the upright's inner edge but its neighbours, at its ends: the
                // trim removes it whole, and the base line's two lengths still conflict.
                "sketch_trim",
                json!({"session": "s1", "revision": at, "sketch": "s0000001", "curve": "na000004", "pick": [5, 30], "ids": null}),
            ),
            (
                "sketch_unjoin",
                json!({"session": "s1", "revision": at, "sketch": "s0000001", "point": "pa000004", "curve": "na000004", "ids": null}),
            ),
        ] {
            let edit = command.trim_start_matches("sketch_");
            assert_eq!(
                f.refused(command, &request.to_string()),
                conflict(edit),
                "{command}, inside: {inside}"
            );
            assert_eq!(f.revision(), at, "{command}");
            assert_eq!(f.text(), text, "{command}");
            assert_eq!(std::fs::read(f.path()).unwrap(), file, "{command}");
        }
    }
}

#[test]
fn a_split_of_the_over_defined_base_line_makes_both_lengths_distances_of_its_outer_ends_which_conflict()
 {
    // Split, each length of the base line becomes a distance of its outer ends, under its
    // ID (§3's table): 60 and 61 between pa000001 and pa000002, which can't both hold.
    let f = bracket();
    f.ran("begin", r#"{"session": "s1"}"#);
    f.over_define();
    let at = f.revision();
    let text = f.text();
    let refused = f.refused(
        "sketch_split",
        &json!({"session": "s1", "revision": at, "sketch": "s0000001", "curve": "na000001", "at": [[30, 0]], "ids": null}).to_string(),
    );
    assert_eq!(refused, conflict("split"));
    assert_eq!((f.revision(), f.text()), (at, text));
}

#[test]
fn a_merge_that_leaves_the_conflict_the_sketch_had_is_no_worse_and_is_made() {
    // The unjoin first, on the clean profile; then the second length; then the merge, whose
    // solve finds the same two lengths in conflict as the solve before it did.
    let f = bracket();
    f.ran("begin", r#"{"session": "s1"}"#);
    let unjoined = f.ran(
        "sketch_unjoin",
        &json!({"session": "s1", "revision": 1, "sketch": "s0000001", "point": "pa000004", "curve": "na000004", "ids": ["pa000008"]}).to_string(),
    );
    assert_eq!(unjoined["added"], json!(["pa000008"]));
    f.over_define();
    let merged = f.ran(
        "sketch_merge",
        &json!({"session": "s1", "revision": 3, "sketch": "s0000001", "points": ["pa000004", "pa000008"]}).to_string(),
    );
    assert_eq!(
        merged,
        json!({"sketch": "s0000001", "added": [], "modified": ["na000004"], "removed": ["pa000008"], "changed": true, "commit": null})
    );
    // And a split now, which is no merge, is refused for the conflict, as the common rules
    // have it.
    assert_eq!(
        f.refused(
            "sketch_split",
            &json!({"session": "s1", "revision": 4, "sketch": "s0000001", "curve": "na000003", "at": [[30, 5]], "ids": null}).to_string(),
        ),
        conflict("split")
    );
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
            "cause": "a merge whose solve is worse than the one before it is refused, as FreeCAD's Coincident command rolls it back",
            "suggestion": "remove the constraints named first, or merge other points",
            "candidates": []
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
