//! break-it for `sketch_split`, `sketch_trim`, `sketch_unjoin`, and `sketch_merge`
//! (docs/design/m5.0-app.md, section 7, *The sketch edits*), through the sessions' own
//! command by name, as a transport calls it (`Sessions::call`), on the fake kernel and the
//! fake sketch solver: hostile requests (numbers past f64, finite ones whose arithmetic
//! overflows, positions of the wrong arity, fixed references and other kinds where a curve
//! or a point goes, IDs a constraint of the sketch holds), core-drawn IDs undone and redone,
//! a member ID two sketches share where a stored name reaches only one of them, a cancel
//! during a merge's second solve, two geometry edits at one revision at once, and a merge
//! whose solve before it failed. After each refusal the session is as it was and the next
//! edit runs. Expected values by hand from the design's rules, and, for the merge's
//! judgment, FreeCAD main's `shouldRejectCoincidentSolve` (`cb2780e2fa`,
//! `CommandConstraints.cpp`), which compares the diagnostics a solve finds whether or not
//! the solver then converges.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Call, SessionCommand, SessionId, Sessions};
use silico_core::document::ElementId;
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::sketch::SketchSolver;
use silico_protocol::{SketchGeometry, SketchSystem, SolveStatus, Solved};
use tempfile::TempDir;

use crate::bracket::{F0000001, H0000001, H0000002, P0000001, S0000001, S0000002, S0000003};
use crate::sessions::{HANG, answer};

fn s1() -> SessionId {
    SessionId::parse("s1").unwrap()
}

fn document(features: &[&str]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

fn bracket() -> String {
    document(&[S0000001, P0000001, S0000002, H0000001, S0000003])
}

struct Fixture {
    _dir: TempDir,
    sketches: FakeSketches,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn opened(text: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = FakeSketches::new();
    let solver = sketches.clone();
    let sessions = Sessions::with_stand_ins(
        kernel,
        move |cache: &Path| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(solver.clone()) as Box<dyn SketchSolver>),
    );
    let path = dir.path().join("part.silico");
    std::fs::write(&path, text).unwrap();
    let id = answer(sessions.open(&path, false)).session.unwrap().id;
    assert_eq!(id, s1());
    Fixture {
        _dir: dir,
        sketches,
        sessions,
    }
}

impl Fixture {
    fn call(&self, name: &str, arguments: &str) -> Value {
        let answered = match self.sessions.call(name, arguments) {
            Call::Running(calling) => calling
                .wait_timeout(HANG)
                .unwrap_or_else(|_| panic!("{name} didn't answer in {HANG:?}")),
            Call::Unknown => panic!("no command {name}"),
        };
        let envelope: Value = serde_json::from_str(&answered.text).unwrap();
        assert_eq!(envelope["ok"], answered.ok, "{}", answered.text);
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
            .status(&s1())
            .session
            .unwrap()
            .revision
            .number()
    }

    fn undo(&self) {
        let at = self.sessions.status(&s1()).session.unwrap().revision;
        answer(self.sessions.undo(&s1(), at, None)).result.unwrap();
    }

    fn redo(&self) {
        let at = self.sessions.status(&s1()).session.unwrap().revision;
        answer(self.sessions.redo(&s1(), at, None)).result.unwrap();
    }

    fn begin(&self) {
        answer(self.sessions.begin(&s1())).result.unwrap();
    }

    fn text(&self) -> String {
        answer(self.sessions.text(&s1(), None)).result.unwrap().text
    }

    fn checked(&self) -> Value {
        self.ran(
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
        )
    }

    /// The next edit after a refusal runs: a split of the base line at its middle, the
    /// session's revision then one on.
    fn next_edit_runs(&self) {
        let at = self.revision();
        let result = self.ran(
            "sketch_split",
            &split(at, "s0000001", "na000001", json!([[30, 0]]), json!(null)),
        );
        assert_eq!(result["changed"], true, "{result}");
        assert_eq!(self.revision(), at + 1);
    }
}

fn split(revision: u64, sketch: &str, curve: &str, at: Value, ids: Value) -> String {
    json!({"session": "s1", "revision": revision, "sketch": sketch, "curve": curve, "at": at, "ids": ids}).to_string()
}

fn trim(revision: u64, sketch: &str, curve: &str, pick: Value, ids: Value) -> String {
    json!({"session": "s1", "revision": revision, "sketch": sketch, "curve": curve, "pick": pick, "ids": ids}).to_string()
}

fn unjoin(revision: u64, sketch: &str, point: &str, curve: &str, ids: Value) -> String {
    json!({"session": "s1", "revision": revision, "sketch": sketch, "point": point, "curve": curve, "ids": ids}).to_string()
}

fn merge(revision: u64, sketch: &str, points: Value) -> String {
    json!({"session": "s1", "revision": revision, "sketch": sketch, "points": points}).to_string()
}

/// The one error of `errors`, failing on more or none.
fn only(errors: &Value) -> &Value {
    let [error] = errors.as_array().unwrap().as_slice() else {
        panic!("not one error: {errors}")
    };
    error
}

fn points_sent(system: &SketchSystem) -> usize {
    system
        .geometry
        .iter()
        .filter(|g| matches!(g, SketchGeometry::Point(_)))
        .count()
}

// ------------------------------------------------------------------ the requests' numbers

#[test]
fn a_split_position_past_f64_is_refused_at_its_component_by_the_reader() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    // Written raw: 1e400 is no f64, and -1e400 neither.
    let request = r#"{"session": "s1", "revision": 1, "sketch": "s0000001", "curve": "na000001", "at": [[1e400, -1e400]], "ids": null}"#;
    let errors = f.refused("sketch_split", request);
    let parameters: Vec<&str> = errors
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["parameter"].as_str().unwrap())
        .collect();
    assert_eq!(parameters, ["at.0.0", "at.0.1"], "{errors}");
    for error in errors.as_array().unwrap() {
        assert_eq!(error["code"], "invalid_input", "{error}");
    }
    assert_eq!((f.text(), f.revision()), (text, 1));
    f.next_edit_runs();
}

#[test]
fn a_split_at_the_largest_finite_coordinates_is_refused_as_off_the_curve_not_split() {
    // |p − a| overflows to infinity, so the projection's distance isn't a number a
    // comparison can pass on: refused, at at.0, as too far to measure.
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    for at in [
        json!([[f64::MAX, f64::MAX]]),
        json!([[f64::MAX, -f64::MAX]]),
        json!([[-f64::MAX, 0.0]]),
        json!([[30.0, f64::MAX]]),
    ] {
        let errors = f.refused(
            "sketch_split",
            &split(1, "s0000001", "na000001", at.clone(), json!(null)),
        );
        let error = only(&errors);
        assert_eq!(error["code"], "invalid_input", "{at}: {error}");
        assert_eq!(error["parameter"], "at.0", "{at}: {error}");
        assert_eq!(error["feature"], "s0000001", "{at}: {error}");
        assert_eq!(error["elements"], json!(["na000001"]), "{at}: {error}");
    }
    assert_eq!((f.text(), f.revision()), (text, 1));
    f.next_edit_runs();
}

#[test]
fn a_circle_split_at_the_largest_finite_coordinates_is_refused_not_made_an_arc_of_nan() {
    // A circle's position is taken by its angle about the centre (atan2 of overflowed
    // differences); its projection back onto the circle can't be the far position, so the
    // split is refused at the first position too far from it.
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    let errors = f.refused(
        "sketch_split",
        &split(
            1,
            "s0000002",
            "cb000001",
            json!([[f64::MAX, f64::MAX], [40, 14.75]]),
            json!(null),
        ),
    );
    let error = only(&errors);
    assert_eq!(error["code"], "invalid_input", "{error}");
    assert_eq!(error["parameter"], "at.0", "{error}");
    assert_eq!((f.text(), f.revision()), (text, 1));
}

#[test]
fn a_trim_picked_at_the_largest_finite_coordinates_is_refused_or_trims_a_piece_never_nan() {
    // A pick need not lie on the curve (FreeCAD trims by where it's picked); a pick whose
    // projection overflows is refused at `pick`. Either way, nothing non-finite is written.
    let f = opened(&bracket());
    f.begin();
    for pick in [
        json!([f64::MAX, f64::MAX]),
        json!([-f64::MAX, f64::MAX]),
        json!([f64::MAX, 0]),
    ] {
        let envelope = f.call(
            "sketch_trim",
            &trim(
                f.revision(),
                "s0000001",
                "na000001",
                pick.clone(),
                json!(null),
            ),
        );
        if envelope["ok"] == false {
            let error = only(&envelope["errors"]);
            assert_eq!(error["code"], "invalid_input", "{pick}: {error}");
            assert_eq!(error["parameter"], "pick", "{pick}: {error}");
        } else {
            let text = f.text();
            assert!(
                !text.contains("inf") && !text.contains("NaN") && !text.contains("nan"),
                "{text}"
            );
            f.undo();
        }
    }
}

#[test]
fn positions_of_the_wrong_arity_or_type_are_refused_each_at_its_place() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    for (at, parameter) in [
        (json!([[30]]), "at.0"),
        (json!([[30, 0, 0]]), "at.0"),
        (json!([30, 0]), "at.0"),
        (json!([["30", 0]]), "at.0.0"),
        (json!([[30, null]]), "at.0.1"),
        (json!([[true, false]]), "at.0.0"),
        (json!({"x": 30, "y": 0}), "at"),
        (json!("30,0"), "at"),
        (json!(null), "at"),
    ] {
        let errors = f.refused(
            "sketch_split",
            &split(1, "s0000001", "na000001", at.clone(), json!(null)),
        );
        assert_eq!(errors[0]["code"], "invalid_input", "{at}: {errors}");
        assert_eq!(errors[0]["parameter"], parameter, "{at}: {errors}");
    }
    for (pick, parameter) in [
        (json!([30]), "pick"),
        (json!([[30, 0]]), "pick"),
        (json!([30, 0, 1]), "pick"),
        (json!([30, "0"]), "pick.1"),
        (json!(null), "pick"),
    ] {
        let errors = f.refused(
            "sketch_trim",
            &trim(1, "s0000001", "na000001", pick.clone(), json!(null)),
        );
        assert_eq!(errors[0]["code"], "invalid_input", "{pick}: {errors}");
        assert_eq!(errors[0]["parameter"], parameter, "{pick}: {errors}");
    }
    assert_eq!((f.text(), f.revision()), (text, 1));
    f.next_edit_runs();
}

#[test]
fn a_hundred_thousand_split_positions_are_refused_in_bounded_time_naming_the_count() {
    let f = opened(&bracket());
    f.begin();
    let at: Vec<[f64; 2]> = (0..100_000).map(|i| [f64::from(i) * 1e-4, 0.0]).collect();
    let started = Instant::now();
    let errors = f.refused(
        "sketch_split",
        &split(1, "s0000001", "na000001", json!(at), json!(null)),
    );
    assert!(started.elapsed() < Duration::from_secs(20));
    let error = only(&errors);
    assert_eq!(
        error["message"], "a line splits at one position, got 100000",
        "{error}"
    );
    assert_eq!(error["parameter"], "at");
}

// ------------------------------------------------------------------ what goes where

#[test]
fn fixed_references_and_members_of_other_kinds_where_a_curve_or_a_point_goes_are_refused() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    // A fixed reference isn't a member these edits take: the reader refuses it.
    for curve in ["x_axis", "y_axis", "origin"] {
        let errors = f.refused(
            "sketch_split",
            &split(1, "s0000001", curve, json!([[30, 0]]), json!(null)),
        );
        assert_eq!(errors[0]["parameter"], "curve", "{curve}: {errors}");
        let errors = f.refused(
            "sketch_trim",
            &trim(1, "s0000001", curve, json!([30, 0]), json!(null)),
        );
        assert_eq!(errors[0]["parameter"], "curve", "{curve}: {errors}");
    }
    let errors = f.refused(
        "sketch_merge",
        &merge(1, "s0000001", json!(["pa000001", "origin"])),
    );
    assert_eq!(errors[0]["parameter"], "points.1", "{errors}");
    let errors = f.refused(
        "sketch_unjoin",
        &unjoin(1, "s0000001", "origin", "na000001", json!(null)),
    );
    assert_eq!(errors[0]["parameter"], "point", "{errors}");
    // A point where a curve goes, a constraint where a curve goes, a line where a point
    // goes, a point where a curve goes for unjoin: each refused at its field, naming it.
    let errors = f.refused(
        "sketch_split",
        &split(1, "s0000001", "pa000001", json!([[30, 0]]), json!(null)),
    );
    assert_eq!(errors[0]["parameter"], "curve", "{errors}");
    assert_eq!(errors[0]["elements"], json!(["pa000001"]), "{errors}");
    let errors = f.refused(
        "sketch_trim",
        &trim(1, "s0000001", "ka000001", json!([30, 0]), json!(null)),
    );
    assert_eq!(errors[0]["parameter"], "curve", "{errors}");
    assert_eq!(errors[0]["elements"], json!(["ka000001"]), "{errors}");
    let errors = f.refused(
        "sketch_merge",
        &merge(1, "s0000001", json!(["pa000001", "na000001"])),
    );
    assert_eq!(errors[0]["parameter"], "points.1", "{errors}");
    let errors = f.refused(
        "sketch_unjoin",
        &unjoin(1, "s0000001", "na000001", "pa000001", json!(null)),
    );
    assert_eq!(errors[0]["code"], "invalid_input", "{errors}");
    assert!(
        errors[0]["parameter"] == "point" || errors[0]["parameter"] == "curve",
        "{errors}"
    );
    // A member of another sketch: the holes' centre, named in the profile.
    let errors = f.refused(
        "sketch_merge",
        &merge(1, "s0000001", json!(["pa000001", "pb000001"])),
    );
    assert_eq!(errors[0]["parameter"], "points.1", "{errors}");
    assert_eq!(errors[0]["feature"], "s0000001", "{errors}");
    assert_eq!((f.text(), f.revision()), (text, 1));
    f.next_edit_runs();
}

#[test]
fn a_merge_of_no_points_one_point_or_one_point_twice_is_refused_as_data() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    for points in [json!([]), json!(["pa000001"])] {
        let errors = f.refused("sketch_merge", &merge(1, "s0000001", points.clone()));
        let error = only(&errors);
        assert_eq!(error["code"], "invalid_input", "{points}: {error}");
        assert_eq!(error["feature"], "s0000001", "{points}: {error}");
        assert_eq!(error["parameter"], "points", "{points}: {error}");
    }
    let errors = f.refused(
        "sketch_merge",
        &merge(1, "s0000001", json!(["pa000001", "pa000001"])),
    );
    let error = only(&errors);
    assert_eq!(
        error["message"],
        "point pa000001 is named twice: it's already one point"
    );
    assert_eq!(error["parameter"], "points.1");
    // Named twice two hundred thousand times: refused at its first repeat, in bounded time.
    let many: Vec<&str> = std::iter::repeat_n("pa000001", 200_000).collect();
    let started = Instant::now();
    let errors = f.refused("sketch_merge", &merge(1, "s0000001", json!(many)));
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(only(&errors)["parameter"], "points.1");
    assert_eq!((f.text(), f.revision()), (text, 1));
    f.next_edit_runs();
}

#[test]
fn an_id_given_that_a_constraint_of_the_sketch_holds_is_refused_at_its_place() {
    // A sketch's IDs are one namespace: a new point can't take a constraint's ID.
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    let errors = f.refused(
        "sketch_unjoin",
        &unjoin(1, "s0000001", "pa000004", "na000004", json!(["ka000001"])),
    );
    let error = only(&errors);
    assert_eq!(
        error["message"], "the sketch already has a member ka000001",
        "{error}"
    );
    assert_eq!(error["parameter"], "ids.0");
    let errors = f.refused(
        "sketch_unjoin",
        &unjoin(1, "s0000001", "pa000004", "na000004", json!([])),
    );
    assert_eq!(
        only(&errors)["message"],
        "the unjoin needs 1 new IDs, got 0"
    );
    assert_eq!((f.text(), f.revision()), (text, 1));
    // Another sketch's member's ID is this one's to give.
    let result = f.ran(
        "sketch_unjoin",
        &unjoin(1, "s0000001", "pa000004", "na000004", json!(["pb000001"])),
    );
    assert_eq!(result["added"], json!(["pb000001"]));
}

// ------------------------------------------------------------------ drawn IDs

#[test]
fn ids_the_core_drew_are_kept_by_undo_and_redo_byte_for_byte_never_drawn_again() {
    let f = opened(&bracket());
    f.begin();
    let before = f.text();
    let split = f.ran(
        "sketch_split",
        &split(1, "s0000001", "na000001", json!([[30, 0]]), json!(null)),
    );
    let after = f.text();
    let unjoined = f.ran(
        "sketch_unjoin",
        &unjoin(2, "s0000001", "pa000004", "na000004", json!(null)),
    );
    let unjoined_text = f.text();
    // Every ID drawn is fresh across both edits.
    let mut drawn: Vec<String> = split["added"]
        .as_array()
        .unwrap()
        .iter()
        .chain(unjoined["added"].as_array().unwrap())
        .map(|id| id.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(drawn.len(), 4);
    for id in &drawn {
        assert!(ElementId::new(id).is_some(), "{id}");
        assert!(!before.contains(&format!("\"{id}\"")), "{id} was held");
    }
    drawn.sort();
    drawn.dedup();
    assert_eq!(drawn.len(), 4, "{drawn:?}");
    f.undo();
    assert_eq!(f.text(), after);
    f.undo();
    assert_eq!(f.text(), before);
    f.redo();
    assert_eq!(f.text(), after);
    f.redo();
    assert_eq!(f.text(), unjoined_text);
    // And the document the drawn IDs make passes the gate and rebuilds: its one problem
    // the pad's, of the corner the unjoin opened, at the drawn point.
    let checked = f.checked();
    let problem = only(&checked["problems"]);
    assert_eq!(problem["feature"], "p0000001", "{checked}");
    let point = unjoined["added"][0].as_str().unwrap();
    let mut named = vec![point, "pa000004"];
    named.sort();
    assert_eq!(problem["elements"], json!(named), "{checked}");
}

#[test]
fn a_trim_needing_many_new_ids_drawn_by_the_core_gives_each_once() {
    // The port note's trim example, as the design's `sketch_trim`: its pick between the
    // two cuts at x = 3 and x = 7 on nd000001, which the given-IDs test pins as needing a
    // new point, a new piece, the copies, and an on; drawn, each fresh and distinct.
    let point = |id: &str, x: i32, y: i32| {
        format!(r#"{{"id": "{id}", "x": {x}, "y": {y}, "construction": false}}"#)
    };
    let line = |id: &str, from: &str, to: &str| {
        format!(r#"{{"id": "{id}", "from": "{from}", "to": "{to}", "construction": false}}"#)
    };
    let sketch = format!(
        r#"{{"id": "s0000004", "type": "sketch", "v": 1, "name": "Trim", "plane": "xy", "points": [{}], "lines": [{}], "circles": [], "arcs": [], "constraints": [{}]}}"#,
        [
            point("pd000001", 0, 0),
            point("pd000002", 10, 0),
            point("pd000003", 3, -2),
            point("pd000004", 3, 2),
            point("pd000005", 7, 0),
            point("pd000006", 7, 3),
        ]
        .join(", "),
        [
            line("nd000001", "pd000001", "pd000002"),
            line("nd000002", "pd000003", "pd000004"),
            line("nd000003", "pd000005", "pd000006"),
        ]
        .join(", "),
        [
            r#"{"id": "kd000001", "type": "horizontal", "line": "nd000001"}"#,
            r#"{"id": "kd000002", "type": "length", "line": "nd000001", "value": 10, "driving": true}"#,
            r#"{"id": "kd000004", "type": "on", "point": "pd000005", "curve": "nd000001"}"#,
        ]
        .join(", ")
    );
    let f = opened(&document(&[&sketch]));
    f.begin();
    let before = f.text();
    let result = f.ran(
        "sketch_trim",
        &trim(1, "s0000004", "nd000001", json!([5, 0]), json!(null)),
    );
    let added: Vec<String> = result["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect();
    assert!(!added.is_empty(), "{result}");
    let mut distinct = added.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), added.len(), "{added:?}");
    for id in &added {
        assert!(!before.contains(&format!("\"{id}\"")), "{id} was held");
    }
    let checked = f.checked();
    assert_eq!(checked["problems"], json!([]), "{checked}");
}

// ------------------------------------------------------------------ references

#[test]
fn a_split_lists_only_features_whose_name_reaches_its_own_sketchs_member_not_a_namesake() {
    // s0000002 and s0000003 both hold a circle cb000001 about a point pb000001. A fillet
    // whose stored name is made from s0000003's cb000001 is reached by a split of
    // s0000003's circle, never by a split of s0000002's, which only shares its ID.
    let named = r#"{"id": "f0000002", "type": "fillet", "v": 1, "name": "Upright hole edge", "base": "f0000001", "edges": ["h0000002.pocket.Edge(G s0000003.sketch.Edge.cb000001)"], "radius": 0.5}"#;
    let f = opened(&document(&[
        S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001, named,
    ]));
    f.begin();
    let result = f.ran(
        "sketch_split",
        &split(
            1,
            "s0000002",
            "cb000001",
            json!([[42.75, 12], [37.25, 12]]),
            json!(null),
        ),
    );
    assert_eq!(result["references"], json!([]), "{result}");
    let result = f.ran(
        "sketch_split",
        &split(
            2,
            "s0000003",
            "cb000001",
            json!([[14.75, 35], [9.25, 35]]),
            json!(null),
        ),
    );
    assert_eq!(result["references"], json!(["f0000002"]), "{result}");
}

// ------------------------------------------------------------------ timing

#[test]
fn a_cancel_during_a_merges_second_solve_the_one_before_it_stops_it_leaving_the_session() {
    // The merged sketch (seven points sent, the origin among them) solves with a
    // redundancy, so the merge solves the sketch before it too (eight): that second solve is
    // held, then cancelled.
    let f = opened(&bracket());
    f.begin();
    f.ran(
        "sketch_unjoin",
        &unjoin(1, "s0000001", "pa000004", "na000004", json!(["pa000008"])),
    );
    f.checked();
    let text = f.text();
    let holder = f.sketches.clone();
    f.sketches.answer(move |system| {
        let mut solved = FakeSketches::as_sent(system);
        if points_sent(system) == 7 {
            solved.redundant = system
                .constraints
                .iter()
                .find_map(|c| c.tag())
                .into_iter()
                .collect();
            // The next solve, the one before the merge, is held.
            holder.hold();
        }
        Ok(solved)
    });
    let request = merge(2, "s0000001", json!(["pa000004", "pa000008"]));
    let running = match f.sessions.call("sketch_merge", &request) {
        Call::Running(calling) => calling,
        Call::Unknown => panic!("no sketch_merge"),
    };
    assert!(
        f.sketches.wait_held(HANG),
        "the merge's second solve never came"
    );
    let held = f.sketches.requests();
    assert_eq!(
        points_sent(held.last().unwrap()),
        8,
        "the held solve isn't the one before"
    );
    let cancelled = f
        .sessions
        .cancel(&s1(), SessionCommand::SketchMerge)
        .result
        .unwrap();
    assert!(cancelled.stopped);
    let Ok(answered) = running.wait_timeout(HANG) else {
        panic!("sketch_merge didn't answer in {HANG:?}")
    };
    let envelope: Value = serde_json::from_str(&answered.text).unwrap();
    assert_eq!(envelope["ok"], false, "{envelope}");
    assert_eq!(only(&envelope["errors"])["code"], "cancelled", "{envelope}");
    f.sketches.release();
    f.sketches
        .answer(|system| Ok(FakeSketches::as_sent(system)));
    assert_eq!((f.text(), f.revision()), (text, 2));
    // The merge then runs.
    let merged = f.ran("sketch_merge", &request);
    assert_eq!(merged["removed"], json!(["pa000008"]));
}

#[test]
fn a_split_and_a_merge_called_at_once_against_one_revision_are_one_edit_and_one_conflict() {
    let f = opened(&bracket());
    f.begin();
    f.ran(
        "sketch_unjoin",
        &unjoin(1, "s0000001", "pa000004", "na000004", json!(["pa000008"])),
    );
    let base = f.text();
    let calls = [
        (
            "sketch_split",
            split(2, "s0000001", "na000001", json!([[30, 0]]), json!(null)),
        ),
        (
            "sketch_merge",
            merge(2, "s0000001", json!(["pa000004", "pa000008"])),
        ),
    ];
    let running: Vec<_> = calls
        .iter()
        .map(|(name, arguments)| match f.sessions.call(name, arguments) {
            Call::Running(calling) => calling,
            Call::Unknown => panic!("no {name}"),
        })
        .collect();
    let answers: Vec<Value> = running
        .into_iter()
        .map(|r| {
            let answered = r.wait_timeout(HANG).unwrap_or_else(|_| panic!("hung"));
            serde_json::from_str(&answered.text).unwrap()
        })
        .collect();
    let ok: Vec<bool> = answers.iter().map(|a| a["ok"] == true).collect();
    assert_eq!(ok.iter().filter(|o| **o).count(), 1, "{answers:?}");
    let lost = answers.iter().find(|a| a["ok"] == false).unwrap();
    assert_eq!(only(&lost["errors"])["code"], "conflict", "{lost}");
    assert_eq!(f.revision(), 3);
    assert_ne!(f.text(), base);
    // Undone, the session is at the base text exactly.
    f.undo();
    assert_eq!(f.text(), base);
}

// ------------------------------------------------------------------ the merge's judgment

/// A solve whose solver ran out of iterations, with the first constraint sent flagged
/// redundant by its diagnosis, as PlaneGCS diagnoses before it solves (FreeCAD's
/// `SketchObject::solve` keeps `lastRedundant` whatever `solve()` then returns).
fn failing_with_first_redundant(system: &SketchSystem) -> Solved {
    let mut solved = FakeSketches::as_sent(system);
    solved.redundant = system
        .constraints
        .iter()
        .find_map(|c| c.tag())
        .into_iter()
        .collect();
    solved.status = SolveStatus::Failed;
    solved.algorithm = None;
    solved
}

/// A solve that converges, with the first constraint sent flagged redundant.
fn solving_with_first_redundant(system: &SketchSystem) -> Solved {
    let mut solved = FakeSketches::as_sent(system);
    solved.redundant = system
        .constraints
        .iter()
        .find_map(|c| c.tag())
        .into_iter()
        .collect();
    solved
}

#[test]
fn a_merge_whose_solve_before_failed_with_the_same_redundancy_is_no_worse_and_is_made() {
    // Before: the unjoined profile (eight points sent) diagnosed with ka000001 redundant,
    // then its solve failing (FreeCAD's SolverError, severity 4). After: the merged profile
    // (seven) solves, ka000001 still redundant (RedundantConstraints, severity 1). FreeCAD
    // main's shouldRejectCoincidentSolve: not Success; 1 > 4 is false; the redundant
    // identities after ({ka000001}) are all among those before ({ka000001}): no new
    // diagnostic, so the coincidence stands.
    // BREAKS: Silico's `flagged` reads no redundancy from a failed solve (its error carries
    // none), so ka000001 counts as new and the merge is refused with "the constraints named
    // repeat others, as the sketch's solve before it found none of them do", though the
    // solve before found exactly that one.
    let f = opened(&bracket());
    f.begin();
    f.ran(
        "sketch_unjoin",
        &unjoin(1, "s0000001", "pa000004", "na000004", json!(["pa000008"])),
    );
    f.checked();
    f.sketches.answer(|system| {
        Ok(match points_sent(system) {
            8 => failing_with_first_redundant(system),
            7 => solving_with_first_redundant(system),
            _ => FakeSketches::as_sent(system),
        })
    });
    let envelope = f.call(
        "sketch_merge",
        &merge(2, "s0000001", json!(["pa000004", "pa000008"])),
    );
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["result"]["removed"], json!(["pa000008"]));
}
