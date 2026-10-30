//! Hostile calls of `sketch_add` and `sketch_remove` (docs/design/m5.0-app.md, section 7),
//! through the sessions' own command by name, as a transport calls it (`Sessions::call`),
//! on the fake kernel and the fake sketch solver: members the document's reader refuses,
//! IDs repeated across the batch's lists, numbers past f64, batches past the solver's
//! limits by orders of magnitude, a list of members to remove a hundred thousand long, two
//! edits at one revision at once, an edit and its inverse, everything removed from a padded
//! sketch, and a line's midpoint at the origin. After each refusal, the session is as it was
//! and the next edit runs. Expected values: by hand from the design's rules and the port
//! note's (docs/ports/sketch-edit.md, decision 7), and FreeCAD's store of a midpoint
//! (`Sketcher::Symmetric` of a line's ends, whatever the point).

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Answered, Call, Calling, Revision, SessionId, Sessions};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::sketch::SketchSolver;
use tempfile::TempDir;

use crate::bracket::{H0000001, P0000001, S0000001, S0000002, S0000003};
use crate::sessions::{HANG, answer};

/// The time a batch past every limit may take to be refused: what a hundred thousand
/// members take linearly (under two seconds in a debug build) dozens of times over, so a
/// loaded runner never reaches it; a quadratic pass over them (1e10 steps) always does.
const BOUNDED: Duration = HANG;

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
    dir: TempDir,
    _sketches: FakeSketches,
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
    answer(sessions.open(&path, false)).session.unwrap();
    Fixture {
        dir,
        _sketches: sketches,
        sessions,
    }
}

impl Fixture {
    fn calling(&self, name: &str, arguments: &str) -> Calling {
        match self.sessions.call(name, arguments) {
            Call::Running(calling) => calling,
            Call::Unknown => panic!("no command {name}"),
        }
    }

    fn answered(&self, name: &str, arguments: &str) -> Answered {
        self.calling(name, arguments)
            .wait_timeout(HANG)
            .unwrap_or_else(|_| panic!("{name} didn't answer in {HANG:?}"))
    }

    fn call(&self, name: &str, arguments: &str) -> Value {
        let answer = self.answered(name, arguments);
        serde_json::from_str(&answer.text).unwrap()
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

    /// The session's revision, as status's envelope gives it.
    fn revision(&self) -> u64 {
        let envelope = self.call("status", r#"{"session": "s1"}"#);
        envelope["session"]["revision"]
            .as_u64()
            .unwrap_or_else(|| panic!("{envelope}"))
    }

    fn begin(&self) {
        self.ran("begin", r#"{"session": "s1"}"#);
    }

    fn text(&self) -> String {
        self.ran("text", r#"{"session": "s1", "feature": null}"#)["text"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn sketch(&self, id: &str) -> Value {
        let document: Value = serde_json::from_str(&self.text()).unwrap();
        document["features"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == id)
            .cloned()
            .unwrap()
    }

    fn file(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("part.silico")).unwrap()
    }

    /// The session is as `text` and `revision` were, and an edit still runs on it: the
    /// next valid add, a point no one uses, is taken and makes the next revision.
    fn unharmed(&self, text: &str, revision: u64, sketch: &str) {
        assert_eq!(self.text(), text, "a refused edit changed the document");
        assert_eq!(
            self.revision(),
            revision,
            "a refused edit moved the revision"
        );
        let added = self.ran(
            "sketch_add",
            &add(
                revision,
                sketch,
                [
                    r#"{"id": "zz000001", "x": 1, "y": 2, "construction": true}"#,
                    "",
                    "",
                    "",
                    "",
                ],
            ),
        );
        assert_eq!(added["added"], json!(["zz000001"]));
        assert_eq!(self.revision(), revision + 1);
    }
}

fn add(revision: u64, sketch: &str, lists: [&str; 5]) -> String {
    let [points, lines, circles, arcs, constraints] = lists;
    format!(
        r#"{{"session": "s1", "revision": {revision}, "sketch": "{sketch}", "points": [{points}], "lines": [{lines}], "circles": [{circles}], "arcs": [{arcs}], "constraints": [{constraints}]}}"#
    )
}

fn remove(revision: u64, sketch: &str, members: &[&str]) -> String {
    json!({"session": "s1", "revision": revision, "sketch": sketch, "members": members}).to_string()
}

/// The `parameter`s of `errors`, in order.
fn parameters(errors: &Value) -> Vec<&str> {
    errors
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["parameter"].as_str().unwrap_or("<none>"))
        .collect()
}

/// Every error names the sketch, has a suggestion, and is `invalid_input`.
fn all_name(errors: &Value, sketch: &str) {
    for e in errors.as_array().unwrap() {
        assert_eq!(e["code"], "invalid_input", "{e}");
        assert_eq!(e["feature"], sketch, "{e}");
        assert!(e["suggestion"].is_string(), "no suggestion: {e}");
    }
}

// ---- Members the reader refuses ----

#[test]
fn a_member_that_isnt_an_object_is_refused_at_its_place_naming_the_sketch() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    let errors = f.refused("sketch_add", &add(1, "s0000002", ["5", "", "", "", ""]));
    assert_eq!(parameters(&errors), ["points.0"], "{errors}");
    all_name(&errors, "s0000002");
    f.unharmed(&text, 1, "s0000002");
}

#[test]
fn a_member_with_no_id_or_a_malformed_one_is_refused_at_its_place() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    for id in [
        r#""id": "PB000003", "#,
        r#""id": "origin", "#,
        r#""id": "", "#,
        r#""id": 7, "#,
        r#""id": "pb0000031", "#,
        "",
    ] {
        let point = format!(r#"{{{id}"x": 1, "y": 1, "construction": false}}"#);
        let errors = f.refused("sketch_add", &add(1, "s0000002", [&point, "", "", "", ""]));
        assert_eq!(errors.as_array().unwrap().len(), 1, "{point}: {errors}");
        assert_eq!(parameters(&errors), ["points.0.id"], "{point}: {errors}");
        all_name(&errors, "s0000002");
    }
    f.unharmed(&text, 1, "s0000002");
}

#[test]
fn a_member_with_a_key_given_twice_is_refused_by_the_strict_reader() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    let answer = f.answered(
        "sketch_add",
        &add(
            1,
            "s0000002",
            [
                r#"{"id": "pb000003", "x": 1, "x": 2, "y": 1, "construction": false}"#,
                "",
                "",
                "",
                "",
            ],
        ),
    );
    assert!(!answer.ok, "{}", answer.text);
    f.unharmed(&text, 1, "s0000002");
}

#[test]
fn a_circle_given_in_the_points_list_is_refused_for_its_fields_there() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    let errors = f.refused(
        "sketch_add",
        &add(
            1,
            "s0000002",
            [
                r#"{"id": "cb000009", "center": "pb000001", "radius": 3, "construction": false}"#,
                "",
                "",
                "",
                "",
            ],
        ),
    );
    all_name(&errors, "s0000002");
    for p in parameters(&errors) {
        assert!(p.starts_with("points.cb000009."), "{errors}");
    }
    f.unharmed(&text, 1, "s0000002");
}

#[test]
fn numbers_past_f64_and_below_its_least_are_refused_or_kept_exactly() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    // Past f64's greatest: refused at the coordinate, never infinity.
    for x in ["1e309", "-1e999", "1e400"] {
        let point = format!(r#"{{"id": "pb000003", "x": {x}, "y": 1, "construction": false}}"#);
        let errors = f.refused("sketch_add", &add(1, "s0000002", [&point, "", "", "", ""]));
        assert_eq!(parameters(&errors), ["points.pb000003.x"], "{x}: {errors}");
        all_name(&errors, "s0000002");
    }
    // Tokens JSON hasn't.
    for x in ["NaN", "Infinity", "-Infinity", "0x10", "+1", ".5", "1."] {
        let point = format!(r#"{{"id": "pb000003", "x": {x}, "y": 1, "construction": false}}"#);
        let answer = f.answered("sketch_add", &add(1, "s0000002", [&point, "", "", "", ""]));
        assert!(!answer.ok, "{x}: {}", answer.text);
    }
    f.unharmed(&text, 1, "s0000002");
}

#[test]
fn a_value_nested_deeper_than_the_reader_takes_is_refused_without_overflowing() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    for depth in [60, 64, 100_000] {
        let x = format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
        let point = format!(r#"{{"id": "pb000003", "x": {x}, "y": 1, "construction": false}}"#);
        let answer = f.answered("sketch_add", &add(1, "s0000002", [&point, "", "", "", ""]));
        assert!(!answer.ok, "depth {depth}: {}", answer.text);
    }
    f.unharmed(&text, 1, "s0000002");
}

#[test]
fn one_id_given_to_a_point_and_a_line_of_the_batch_is_refused_at_the_line() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    let errors = f.refused(
        "sketch_add",
        &add(
            1,
            "s0000001",
            [
                r#"{"id": "xx000001", "x": 1, "y": 1, "construction": false}"#,
                r#"{"id": "xx000001", "from": "pa000001", "to": "xx000001", "construction": false}"#,
                "",
                "",
                "",
            ],
        ),
    );
    assert_eq!(parameters(&errors), ["lines.0.id"], "{errors}");
    all_name(&errors, "s0000001");
    assert!(
        errors[0]["message"]
            .as_str()
            .unwrap()
            .contains("earlier in this request"),
        "{errors}"
    );
    f.unharmed(&text, 1, "s0000001");
}

#[test]
fn an_id_another_sketch_holds_is_the_editeds_to_give() {
    // IDs are each sketch's own: s0000003's pb000001 doesn't stop s0000001 from having one.
    let f = opened(&bracket());
    f.begin();
    let s3 = f.sketch("s0000003");
    let added = f.ran(
        "sketch_add",
        &add(
            1,
            "s0000001",
            [
                r#"{"id": "pb000001", "x": 30, "y": 30, "construction": true}"#,
                "",
                "",
                "",
                "",
            ],
        ),
    );
    assert_eq!(added["added"], json!(["pb000001"]));
    assert_eq!(f.sketch("s0000003"), s3);
}

#[test]
fn a_constraint_naming_a_member_of_another_sketch_is_refused_as_naming_no_member() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    let errors = f.refused(
        "sketch_add",
        &add(
            1,
            "s0000001",
            [
                "",
                "",
                "",
                "",
                r#"{"id": "kz000001", "type": "horizontal", "line": "cb000001"}"#,
            ],
        ),
    );
    assert_eq!(errors.as_array().unwrap().len(), 1, "{errors}");
    all_name(&errors, "s0000001");
    assert!(
        errors[0]["parameter"]
            .as_str()
            .unwrap()
            .starts_with("constraints.kz000001"),
        "{errors}"
    );
    f.unharmed(&text, 1, "s0000001");
}

// ---- Curves that make nothing ----

#[test]
fn a_line_from_a_point_to_itself_and_an_arc_whose_ends_are_its_centre_are_refused() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    let errors = f.refused(
        "sketch_add",
        &add(
            1,
            "s0000001",
            [
                "",
                r#"{"id": "nz000001", "from": "pa000002", "to": "pa000002", "construction": false}"#,
                "",
                r#"{"id": "az000001", "center": "pa000003", "start": "pa000003", "end": "pa000003", "construction": false}"#,
                "",
            ],
        ),
    );
    all_name(&errors, "s0000001");
    let at = parameters(&errors);
    assert!(
        at.iter().any(|p| p.starts_with("lines.nz000001")),
        "{errors}"
    );
    assert!(
        at.iter().any(|p| p.starts_with("arcs.az000001")),
        "{errors}"
    );
    f.unharmed(&text, 1, "s0000001");
}

#[test]
fn a_circle_centred_on_a_point_a_line_ends_at_is_taken_as_shared() {
    // A circle's centre may be any point, a line's end among them (shared points).
    let f = opened(&bracket());
    f.begin();
    let added = f.ran(
        "sketch_add",
        &add(
            1,
            "s0000001",
            [
                "",
                "",
                r#"{"id": "cz000001", "center": "pa000004", "radius": 1, "construction": false}"#,
                "",
                "",
            ],
        ),
    );
    assert_eq!(added["added"], json!(["cz000001"]));
    // pa000004 is now two lines' end and a circle's centre: refused, said as both.
    let errors = f.refused("sketch_remove", &remove(2, "s0000001", &["pa000004"]));
    assert_eq!(
        errors[0]["cause"],
        "a point that curves use is those curves' end or centre"
    );
}

// ---- Batches far past the limits ----

#[test]
fn a_batch_of_a_hundred_thousand_points_is_refused_in_bounded_time() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    let points: Vec<String> = (0..100_000)
        .map(|i| format!(r#"{{"id": "q{i:07}", "x": 1, "y": 1, "construction": false}}"#))
        .collect();
    let started = Instant::now();
    let errors = f.refused(
        "sketch_add",
        &add(1, "s0000002", [&points.join(", "), "", "", "", ""]),
    );
    let took = started.elapsed();
    assert!(took < BOUNDED, "took {took:?}");
    all_name(&errors, "s0000002");
    assert!(parameters(&errors).contains(&"points"), "{errors}");
    f.unharmed(&text, 1, "s0000002");
}

#[test]
fn a_batch_of_a_hundred_thousand_constraints_on_one_line_is_refused_in_bounded_time() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    let constraints: Vec<String> = (0..100_000)
        .map(|i| format!(r#"{{"id": "k{i:07}", "type": "horizontal", "line": "na000001"}}"#))
        .collect();
    let started = Instant::now();
    let errors = f.refused(
        "sketch_add",
        &add(1, "s0000001", ["", "", "", "", &constraints.join(", ")]),
    );
    let took = started.elapsed();
    assert!(took < BOUNDED, "took {took:?}");
    all_name(&errors, "s0000001");
    assert!(parameters(&errors).contains(&"constraints"), "{errors}");
    f.unharmed(&text, 1, "s0000001");
}

#[test]
fn a_batch_of_a_hundred_thousand_tangents_between_two_circles_is_refused_in_bounded_time() {
    // Pairs of curves are what a gate checks pairwise: the tangent rules (#290).
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    let constraints: Vec<String> = (0..100_000)
        .map(|i| {
            if i % 2 == 0 {
                format!(
                    r#"{{"id": "k{i:07}", "type": "tangent_on", "a": "cb000001", "b": "cb000002", "side": "outside"}}"#
                )
            } else {
                format!(r#"{{"id": "k{i:07}", "type": "equal", "a": "cb000001", "b": "cb000002"}}"#)
            }
        })
        .collect();
    let started = Instant::now();
    let errors = f.refused(
        "sketch_add",
        &add(1, "s0000002", ["", "", "", "", &constraints.join(", ")]),
    );
    let took = started.elapsed();
    assert!(took < BOUNDED, "took {took:?}");
    all_name(&errors, "s0000002");
    f.unharmed(&text, 1, "s0000002");
}

#[test]
fn a_remove_of_two_hundred_thousand_unknown_members_lists_a_hundred_and_counts_them_all() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    let unknown: Vec<String> = (0..200_000).map(|i| format!("q{i:07}")).collect();
    let members: Vec<&str> = unknown.iter().map(String::as_str).collect();
    let started = Instant::now();
    let envelope = f.call("sketch_remove", &remove(1, "s0000001", &members));
    let took = started.elapsed();
    assert!(took < BOUNDED, "took {took:?}");
    assert_eq!(envelope["ok"], false);
    let errors = envelope["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 100, "{}", errors.len());
    assert_eq!(envelope["total"], 200_000, "{}", envelope["total"]);
    f.unharmed(&text, 1, "s0000001");
}

#[test]
fn a_remove_of_one_line_named_two_hundred_thousand_times_removes_it_once() {
    let f = opened(&bracket());
    f.begin();
    let members = vec!["na000003"; 200_000];
    let started = Instant::now();
    let removed = f.ran("sketch_remove", &remove(1, "s0000001", &members));
    assert!(started.elapsed() < BOUNDED);
    assert_eq!(removed["removed"], json!(["na000003", "ka000003"]));
}

#[test]
fn a_remove_of_the_origin_two_hundred_thousand_times_says_it_once_at_its_first_place() {
    // Each problem once, at the first place that names it, as an unknown member's is: the
    // origin named 200,000 times is one problem, not 200,000.
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    let members = vec!["origin"; 200_000];
    let started = Instant::now();
    let envelope = f.call("sketch_remove", &remove(1, "s0000001", &members));
    assert!(started.elapsed() < BOUNDED);
    assert_eq!(envelope["ok"], false);
    assert_eq!(
        envelope["errors"],
        json!([{
            "code": "invalid_input",
            "message": "origin is one of the sketch's fixed references, which can't be removed",
            "feature": "s0000001",
            "elements": [],
            "elements_total": 0,
            "parameter": "members.0",
            "cause": null,
            "suggestion": "remove the constraints that name it instead",
            "candidates": [], "advice": null
        }])
    );
    assert_eq!(envelope["total"], 1);
    f.unharmed(&text, 1, "s0000001");
}

// ---- Edits racing ----

#[test]
fn two_adds_at_one_revision_called_at_once_are_one_edit_and_one_conflict() {
    let f = opened(&bracket());
    f.begin();
    let one = add(
        1,
        "s0000002",
        [
            r#"{"id": "pz000001", "x": 1, "y": 1, "construction": true}"#,
            "",
            "",
            "",
            "",
        ],
    );
    let two = add(
        1,
        "s0000002",
        [
            r#"{"id": "pz000002", "x": 2, "y": 2, "construction": true}"#,
            "",
            "",
            "",
            "",
        ],
    );
    let a = f.calling("sketch_add", &one);
    let b = f.calling("sketch_add", &two);
    let Ok(a) = a.wait_timeout(HANG) else {
        panic!("hung")
    };
    let Ok(b) = b.wait_timeout(HANG) else {
        panic!("hung")
    };
    assert!(a.ok, "{}", a.text);
    assert!(!b.ok, "{}", b.text);
    let b: Value = serde_json::from_str(&b.text).unwrap();
    assert_eq!(b["errors"][0]["code"], "conflict", "{b}");
    let points = f.sketch("s0000002")["points"].clone();
    let ids: Vec<&str> = points
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["pb000001", "pb000002", "pz000001"]);
    assert_eq!(f.revision(), 2);
}

// ---- An edit and its inverse ----

#[test]
fn a_batch_added_then_removed_inside_a_transaction_is_the_document_it_was() {
    let f = opened(&bracket());
    f.begin();
    // The file's text, as opened, isn't canonical; the edits' is: compared as documents,
    // then the second edit's canonical text against a canonical write of the first's.
    let document: Value = serde_json::from_str(&f.text()).unwrap();
    let added = f.ran(
        "sketch_add",
        &add(
            1,
            "s0000001",
            [
                r#"{"id": "pz000001", "x": 30, "y": 20, "construction": false}"#,
                r#"{"id": "nz000001", "from": "pa000003", "to": "pz000001", "construction": false}"#,
                r#"{"id": "cz000001", "center": "pz000001", "radius": 2, "construction": false}"#,
                "",
                r#"{"id": "kz000001", "type": "horizontal", "line": "nz000001"}, {"id": "kz000002", "type": "radius", "curve": "cz000001", "value": 2, "driving": true}"#,
            ],
        ),
    );
    assert_eq!(
        added["added"],
        json!(["pz000001", "nz000001", "cz000001", "kz000001", "kz000002"])
    );
    // The line and the circle together take their point (no curve uses it after), and
    // every constraint on what goes; pa000003 stays, the bracket's lines' end.
    let removed = f.ran(
        "sketch_remove",
        &remove(2, "s0000001", &["nz000001", "cz000001"]),
    );
    assert_eq!(
        removed["removed"],
        json!(["pz000001", "nz000001", "cz000001", "kz000001", "kz000002"])
    );
    let after = f.text();
    assert_eq!(serde_json::from_str::<Value>(&after).unwrap(), document);
    assert_eq!(f.revision(), 3);
    // And byte for byte: an empty add (no change) leaves the same canonical text.
    f.ran("sketch_add", &add(3, "s0000001", ["", "", "", "", ""]));
    assert_eq!(f.text(), after);
}

#[test]
fn everything_removed_from_a_padded_sketch_is_applied_and_check_says_why_the_pad_fails() {
    // SD7: an edit isn't refused for what the rebuild then finds. The pad's sketch, emptied
    // of everything, is applied; check reports the pad's failure as data, naming it; and
    // undo has the sketch back byte for byte.
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    let s1 = f.sketch("s0000001");
    let mut all: Vec<String> = Vec::new();
    for list in ["points", "lines", "constraints"] {
        for m in s1[list].as_array().unwrap() {
            all.push(m["id"].as_str().unwrap().to_owned());
        }
    }
    let members: Vec<&str> = all.iter().map(String::as_str).collect();
    let removed = f.ran("sketch_remove", &remove(1, "s0000001", &members));
    assert_eq!(
        removed["removed"].as_array().unwrap().len(),
        24,
        "{removed}"
    );
    let emptied = f.sketch("s0000001");
    for list in ["points", "lines", "circles", "arcs", "constraints"] {
        assert_eq!(emptied[list], json!([]), "{list}");
    }
    let checked = f.call(
        "check",
        r#"{"session": "s1", "model": "current", "fresh": false}"#,
    );
    // The pad fails, as data naming it: its sketch has no curves (profile.rs's first
    // refusal, its advice in the cause), one refusal (sweep.rs's `unprofiled`), its parameter
    // the pad's `sketch`, no element at fault. The sketch itself isn't failed.
    assert_eq!(checked["ok"], true, "{checked}");
    let problems = checked["result"]["problems"].as_array().unwrap();
    let pad = problems
        .iter()
        .find(|p| p["feature"] == "p0000001")
        .unwrap_or_else(|| panic!("{checked}"));
    assert_eq!(
        *pad,
        json!({
            "code": "invalid_input",
            "message": "pad p0000001 sweeps sketch s0000001, which makes no profile (1 refusal)",
            "feature": "p0000001",
            "elements": [],
            "elements_total": 0,
            "parameter": "sketch",
            "cause": "the sketch has no curves to make a profile of (draw a closed outline of lines, arcs, or circles that aren't construction geometry)",
            "suggestion": "fix sketch s0000001 where the cause says, or sweep another sketch",
            "candidates": [], "advice": null
        })
    );
    assert!(
        problems.iter().all(|p| p["feature"] != "s0000001"),
        "{checked}"
    );
    let at = Revision::from_number(f.revision()).unwrap();
    answer(f.sessions.undo(&SessionId::parse("s1").unwrap(), at, None))
        .result
        .unwrap();
    assert_eq!(f.text(), text);
}

#[test]
fn a_sketch_edit_outside_a_transaction_saves_the_file_the_session_holds() {
    let f = opened(&bracket());
    let before = f.file();
    let errors = f.refused(
        "sketch_add",
        &add(
            1,
            "s0000002",
            [
                r#"{"id": "pb000003", "x": 1, "y": 1, "construction": false}"#,
                "",
                r#"{"id": "cb000003", "center": "pb000003", "radius": -1, "construction": false}"#,
                "",
                "",
            ],
        ),
    );
    all_name(&errors, "s0000002");
    assert_eq!(f.file(), before, "a refused edit wrote the file");
    let result = f.ran(
        "sketch_remove",
        &remove(1, "s0000002", &["kb000006", "kb000005"]),
    );
    assert_eq!(result["removed"], json!(["kb000005", "kb000006"]));
    assert!(result["commit"].is_object(), "{result}");
    assert_eq!(f.file(), f.text());
}

// ---- The midpoint a removed line leaves ----

/// A sketch with lines a-b, b-c, and c-a on the xy plane: removing a-b keeps a and b (the
/// other two lines use them), the case the midpoint's conversion is for.
fn triangle(midpoint_point: &str) -> String {
    let sketch = format!(
        r#"{{"id": "s0000009", "type": "sketch", "v": 1, "name": "T", "plane": "xy", "points": [{{"id": "p0000001", "x": -10, "y": 0, "construction": false}}, {{"id": "p0000002", "x": 10, "y": 0, "construction": false}}, {{"id": "p0000003", "x": 0, "y": 10, "construction": false}}, {{"id": "p0000004", "x": 0, "y": 0, "construction": false}}], "lines": [{{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}}, {{"id": "n0000002", "from": "p0000002", "to": "p0000003", "construction": false}}, {{"id": "n0000003", "from": "p0000003", "to": "p0000001", "construction": false}}], "circles": [], "arcs": [], "constraints": [{{"id": "k0000001", "type": "midpoint", "point": "{midpoint_point}", "line": "n0000001"}}]}}"#
    );
    document(&[&sketch])
}

#[test]
fn a_removed_lines_midpoint_on_a_point_becomes_a_symmetric_of_its_ends() {
    let f = opened(&triangle("p0000004"));
    f.begin();
    let removed = f.ran("sketch_remove", &remove(1, "s0000009", &["n0000001"]));
    assert_eq!(removed["modified"], json!(["k0000001"]));
    assert_eq!(removed["removed"], json!(["n0000001"]));
    assert_eq!(
        f.sketch("s0000009")["constraints"],
        json!([{"id": "k0000001", "type": "symmetric", "a": "p0000001", "b": "p0000002", "about": "p0000004"}])
    );
}

// Broke, fixed: a line's midpoint at the origin was dropped with the line, where one at a
// sketch point becomes a symmetric of the line's ends about it. FreeCAD 1.1.3 keeps both
// alike (run here: a triangle's base deleted with `delGeometry`, its midpoint a `Symmetric`
// of the base's ends about a point gives `Symmetric(1, 2, 0, 1, 2, 1)`, about the root
// point `Symmetric(1, 2, 0, 1, -1, 1)`), and remove's own rule ("while the point and both
// ends stay") holds for the origin, which always stays. The conversion asked
// `point.element()`, which a fixed reference hasn't (document/sketch_edit.rs, remove's
// `Form::Midpoint` arm), so the ends lost the symmetry about the origin that held them.
#[test]
fn a_removed_lines_midpoint_on_the_origin_becomes_a_symmetric_about_the_origin() {
    let f = opened(&triangle("origin"));
    f.begin();
    let removed = f.ran("sketch_remove", &remove(1, "s0000009", &["n0000001"]));
    assert_eq!(
        f.sketch("s0000009")["constraints"],
        json!([{"id": "k0000001", "type": "symmetric", "a": "p0000001", "b": "p0000002", "about": "origin"}]),
        "{removed}"
    );
    assert_eq!(removed["modified"], json!(["k0000001"]));
    assert_eq!(removed["removed"], json!(["n0000001"]));
}

// ---- Keys no member has ----

#[test]
fn a_member_with_a_megabyte_long_unknown_key_is_refused_in_a_short_answer() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    let key = "k".repeat(1 << 20);
    let point =
        format!(r#"{{"id": "pb000003", "x": 1, "y": 1, "construction": false, "{key}": 1}}"#);
    let answer = f.answered("sketch_add", &add(1, "s0000002", [&point, "", "", "", ""]));
    assert!(!answer.ok);
    // The refusal names where the key is; it needn't say the key a megabyte long, and an
    // answer the response limit refuses would say nothing of the member at all.
    assert!(answer.text.len() < 64 << 10, "{} bytes", answer.text.len());
    let envelope: Value = serde_json::from_str(&answer.text).unwrap();
    let errors = &envelope["errors"];
    all_name(errors, "s0000002");
    assert!(
        errors[0]["parameter"]
            .as_str()
            .unwrap()
            .starts_with("points.pb000003"),
        "{}",
        &answer.text[..answer.text.len().min(2000)]
    );
    f.unharmed(&text, 1, "s0000002");
}

#[test]
fn a_member_with_a_hundred_thousand_unknown_keys_is_refused_in_bounded_time() {
    let f = opened(&bracket());
    f.begin();
    let text = f.text();
    let keys: Vec<String> = (0..100_000).map(|i| format!(r#""u{i}": 1"#)).collect();
    let point = format!(
        r#"{{"id": "pb000003", "x": 1, "y": 1, "construction": false, {}}}"#,
        keys.join(", ")
    );
    let started = Instant::now();
    let errors = f.refused("sketch_add", &add(1, "s0000002", [&point, "", "", "", ""]));
    assert!(started.elapsed() < BOUNDED, "took {:?}", started.elapsed());
    all_name(&errors, "s0000002");
    f.unharmed(&text, 1, "s0000002");
}

// ---- Orders ----

#[test]
fn added_is_in_the_order_given_and_the_document_writes_each_list_by_id() {
    let f = opened(&bracket());
    f.begin();
    let added = f.ran(
        "sketch_add",
        &add(
            1,
            "s0000002",
            [
                r#"{"id": "pz000009", "x": 1, "y": 1, "construction": true}, {"id": "pa000001", "x": 2, "y": 2, "construction": true}"#,
                "",
                "",
                "",
                r#"{"id": "kz000002", "type": "distance_x", "a": "origin", "b": "pz000009", "value": 1, "driving": true}, {"id": "ka000001", "type": "distance_y", "a": "origin", "b": "pa000001", "value": 2, "driving": true}"#,
            ],
        ),
    );
    assert_eq!(
        added["added"],
        json!(["pz000009", "pa000001", "kz000002", "ka000001"])
    );
    let sketch = f.sketch("s0000002");
    let ids = |list: &str| -> Vec<String> {
        sketch[list]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        ids("points"),
        ["pa000001", "pb000001", "pb000002", "pz000009"]
    );
    assert_eq!(
        ids("constraints"),
        [
            "ka000001", "kb000001", "kb000002", "kb000003", "kb000004", "kb000005", "kb000006",
            "kz000002"
        ]
    );
    // A lone point removed takes the constraints on it; removed by kind, then by ID.
    let removed = f.ran(
        "sketch_remove",
        &remove(2, "s0000002", &["pz000009", "pa000001"]),
    );
    assert_eq!(
        removed["removed"],
        json!(["pa000001", "pz000009", "ka000001", "kz000002"])
    );
}
