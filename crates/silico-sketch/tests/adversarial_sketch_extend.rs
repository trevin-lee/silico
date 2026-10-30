//! break-it for `sketch_extend` (docs/design/m5.0-app.md, section 7, `sketch_extend`; open
//! item 12c) on PlaneGCS: through the sessions' own command called by name, as a transport
//! calls it, on the fake kernel, each sketch solved by this crate's worker. Malformed
//! requests (every field of `by` wrong in turn), points at and beside the curve's ends, arcs
//! turned to a whole turn and past it, the next curve of a circle, a point, or a construction
//! line, an end held on an axis or shared with an arc, a file whose stored coordinates drift
//! from its solve, two extends pipelined against one revision, and a sketch past a drag's
//! limits; after every refusal, the session as it was and the next extend run.
//!
//! Expected values by hand: where a point on a line or a circle is, which field a request
//! got wrong, and that a saved sketch solves to its file (M3.4: no `drift` problem).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Answered, Call, Sessions};
use silico_core::fake::FakeKernel;
use silico_core::sketch::{SketchSolver, SketchWorker};
use tempfile::TempDir;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

const HANG: Duration = Duration::from_secs(60);

struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
    session: std::cell::RefCell<String>,
}

fn document(features: &[&str]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

fn sketch(
    points: &[String],
    lines: &[String],
    circles: &[String],
    arcs: &[String],
    constraints: &[&str],
) -> String {
    format!(
        r#"{{"id": "s0000001", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{}], "lines": [{}], "circles": [{}], "arcs": [{}], "constraints": [{}]}}"#,
        points.join(", "),
        lines.join(", "),
        circles.join(", "),
        arcs.join(", "),
        constraints.join(", ")
    )
}

fn p(id: &str, x: f64, y: f64) -> String {
    format!(r#"{{"id": "{id}", "x": {x:?}, "y": {y:?}, "construction": false}}"#)
}

fn l(id: &str, from: &str, to: &str) -> String {
    format!(r#"{{"id": "{id}", "from": "{from}", "to": "{to}", "construction": false}}"#)
}

fn l_construction(id: &str, from: &str, to: &str) -> String {
    format!(r#"{{"id": "{id}", "from": "{from}", "to": "{to}", "construction": true}}"#)
}

fn c(id: &str, center: &str, radius: f64) -> String {
    format!(
        r#"{{"id": "{id}", "center": "{center}", "radius": {radius:?}, "construction": false}}"#
    )
}

fn a(id: &str, center: &str, start: &str, end: &str) -> String {
    format!(
        r#"{{"id": "{id}", "center": "{center}", "start": "{start}", "end": "{end}", "construction": false}}"#
    )
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
    let f = Fixture {
        dir,
        sessions,
        session: std::cell::RefCell::new(String::new()),
    };
    std::fs::write(f.path(), text).unwrap();
    f.open();
    f
}

impl Fixture {
    fn path(&self) -> PathBuf {
        self.dir.path().join("part.silico")
    }

    fn open(&self) {
        let opened = self.call(
            "open",
            &json!({"path": self.path().to_str().unwrap(), "create": false}).to_string(),
        );
        assert_eq!(opened["ok"], true, "{opened}");
        *self.session.borrow_mut() = opened["result"]["session"]["id"]
            .as_str()
            .or_else(|| opened["session"]["id"].as_str())
            .unwrap()
            .to_owned();
    }

    fn close(&self) {
        let closed = self.call("close", r#"{"session": "s1"}"#);
        assert_eq!(closed["ok"], true, "{closed}");
    }

    /// `arguments` with the open session's ID for its `session`, if it has one.
    fn with_session(&self, arguments: &str) -> String {
        let Ok(mut value) = serde_json::from_str::<Value>(arguments) else {
            return arguments.to_owned();
        };
        if value.get("session").is_some_and(|s| s == "s1") {
            value["session"] = json!(self.session.borrow().as_str());
        }
        value.to_string()
    }

    fn call(&self, name: &str, arguments: &str) -> Value {
        let arguments = &self.with_session(arguments);
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
        assert_eq!(envelope["ok"], false, "{arguments} → {envelope}");
        envelope["errors"].clone()
    }

    fn revision(&self) -> u64 {
        let status = self.call("status", r#"{"session": "s1"}"#);
        status["session"]["revision"]
            .as_u64()
            .unwrap_or_else(|| panic!("{status}"))
    }

    fn text(&self) -> String {
        self.ran("text", r#"{"session": "s1", "feature": null}"#)["text"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn problems(&self) -> Value {
        self.ran(
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
        )["problems"]
            .clone()
    }

    /// Sketch s0000001's points as `text` holds them, by ID.
    fn points_of(text: &str) -> Vec<(String, f64, f64)> {
        let document: Value = serde_json::from_str(text).unwrap();
        document["features"][0]["points"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["id"].as_str().unwrap().to_owned(),
                    p["x"].as_f64().unwrap(),
                    p["y"].as_f64().unwrap(),
                )
            })
            .collect()
    }

    fn point(&self, id: &str) -> [f64; 2] {
        let (_, x, y) = Self::points_of(&self.text())
            .into_iter()
            .find(|(p, _, _)| p == id)
            .unwrap_or_else(|| panic!("no point {id}"));
        [x, y]
    }

    fn saved_point(&self, id: &str) -> [f64; 2] {
        let text = std::fs::read_to_string(self.path()).unwrap();
        let (_, x, y) = Self::points_of(&text)
            .into_iter()
            .find(|(p, _, _)| p == id)
            .unwrap_or_else(|| panic!("no point {id}"));
        [x, y]
    }
}

fn extend(revision: u64, curve: &str, end: &str, by: Value) -> String {
    json!({"session": "s1", "revision": revision, "sketch": "s0000001", "curve": curve, "end": end, "by": by}).to_string()
}

/// A free line n000000a from (0, 0) to (10, 0), a line n000000b across x = 15 from
/// (15, -5) to (15, 5), an arc r000000a about (0, 20), radius 10, a quarter turn from
/// (10, 20) to (0, 30), and a circle c000000a about (40, 0) of radius 5: nothing
/// constrained.
fn free() -> String {
    document(&[&sketch(
        &[
            p("p0000001", 0.0, 0.0),
            p("p0000002", 10.0, 0.0),
            p("p0000003", 15.0, -5.0),
            p("p0000004", 15.0, 5.0),
            p("p0000005", 0.0, 20.0),
            p("p0000006", 10.0, 20.0),
            p("p0000007", 0.0, 30.0),
            p("p0000008", 40.0, 0.0),
        ],
        &[
            l("n000000a", "p0000001", "p0000002"),
            l("n000000b", "p0000003", "p0000004"),
        ],
        &[c("c000000a", "p0000008", 5.0)],
        &[a("r000000a", "p0000005", "p0000006", "p0000007")],
        &[],
    )])
}

/// Every refusal names a parameter and suggests something; the session is as it was.
#[track_caller]
fn refused_as(f: &Fixture, request: &str, parameter: &str, before: &(u64, String)) -> Value {
    let errors = f.refused("sketch_extend", request);
    let first = &errors[0];
    assert_eq!(
        first["parameter"], parameter,
        "{request}: the wrong parameter named: {errors}"
    );
    // A shape's refusal ("by must be an object, got a string") is its own correction, as
    // every command's reader words one; any other says what to do.
    assert!(
        first["suggestion"].is_string() || first["message"].as_str().unwrap().contains(" must be "),
        "{request}: no suggestion: {errors}"
    );
    assert_eq!(
        &(f.revision(), f.text()),
        before,
        "{request}: the session changed"
    );
    errors
}

#[test]
fn every_malformed_field_of_a_request_is_refused_naming_it_and_the_next_extend_runs() {
    let f = opened(&free());
    f.ran("begin", r#"{"session": "s1"}"#);
    let before = (f.revision(), f.text());
    let at = before.0;
    let cases: Vec<(String, &str)> = vec![
        (extend(at, "n000000a", "end", json!({"type": "length"})), "by.length"),
        (extend(at, "n000000a", "end", json!({})), "by.type"),
        (extend(at, "n000000a", "end", json!("next_curve")), "by"),
        (extend(at, "n000000a", "end", json!(null)), "by"),
        (extend(at, "n000000a", "end", json!([2])), "by"),
        (extend(at, "n000000a", "end", json!({"type": "length", "length": "2"})), "by.length"),
        (extend(at, "n000000a", "end", json!({"type": "length", "length": null})), "by.length"),
        (extend(at, "n000000a", "end", json!({"type": "length", "length": 2, "angle": 3})), "by.angle"),
        (extend(at, "n000000a", "end", json!({"type": "next_curve", "length": 2})), "by.length"),
        (extend(at, "n000000a", "end", json!({"type": "point"})), "by.point"),
        (extend(at, "n000000a", "end", json!({"type": "point", "point": [12, 0, 0]})), "by.point"),
        (extend(at, "n000000a", "end", json!({"type": "point", "point": [12]})), "by.point"),
        (extend(at, "n000000a", "end", json!({"type": "point", "point": [12, "0"]})), "by.point.1"),
        (extend(at, "n000000a", "end", json!({"type": "point", "point": {"x": 12, "y": 0}})), "by.point"),
        (extend(at, "n000000a", "end", json!({"type": "Length", "length": 2})), "by.type"),
        (extend(at, "n000000a", "end", json!({"type": null, "length": 2})), "by.type"),
        (extend(at, "n000000a", "mid", json!({"type": "length", "length": 2})), "end"),
        (extend(at, "n000000a", "END", json!({"type": "length", "length": 2})), "end"),
        (extend(at, "n000000a", "", json!({"type": "length", "length": 2})), "end"),
        (extend(at, "", "end", json!({"type": "length", "length": 2})), "curve"),
        (extend(at, "N000000A", "end", json!({"type": "length", "length": 2})), "curve"),
        (extend(at, "n000000z", "end", json!({"type": "length", "length": 2})), "curve"),
        (extend(at, "p0000002", "end", json!({"type": "length", "length": 2})), "curve"),
        (extend(at, "p0000002", "end", json!({"type": "next_curve"})), "curve"),
        (extend(at, "origin", "end", json!({"type": "next_curve"})), "curve"),
        (extend(at, "x_axis", "end", json!({"type": "length", "length": 2})), "curve"),
        (extend(at, "c000000a", "end", json!({"type": "next_curve"})), "curve"),
        (extend(at, "c000000a", "end", json!({"type": "point", "point": [45, 0]})), "curve"),
        (extend(at, "n000000a", "end", json!({"type": "angle", "angle": 10})), "by"),
        (extend(at, "r000000a", "end", json!({"type": "length", "length": 1})), "by"),
        (
            json!({"session": "s1", "revision": at, "sketch": "s0000009", "curve": "n000000a", "end": "end", "by": {"type": "length", "length": 1}}).to_string(),
            "sketch",
        ),
        (
            json!({"session": "s1", "revision": 0, "sketch": "s0000001", "curve": "n000000a", "end": "end", "by": {"type": "length", "length": 1}}).to_string(),
            "revision",
        ),
        (
            json!({"session": "s1", "revision": at, "sketch": "s0000001", "curve": "n000000a", "end": "end", "by": {"type": "length", "length": 1}, "extra": 1}).to_string(),
            "extra",
        ),
        (
            json!({"session": "s1", "revision": at, "sketch": "s0000001", "end": "end", "by": {"type": "length", "length": 1}}).to_string(),
            "curve",
        ),
    ];
    for (request, parameter) in &cases {
        refused_as(&f, request, parameter, &before);
    }
    // The next valid extend runs: by 2.5, to (12.5, 0) by hand.
    let result = f.ran(
        "sketch_extend",
        &extend(
            at,
            "n000000a",
            "end",
            json!({"type": "length", "length": 2.5}),
        ),
    );
    assert_eq!(result["modified"], json!(["p0000002"]), "{result}");
    assert_eq!(f.point("p0000002"), [12.5, 0.0]);
}

#[test]
fn a_mistyped_kind_of_by_is_refused_once_not_with_its_value_called_unknown_too() {
    // `lenght` for `length`: the one fault is the kind. Its `length` field is what the
    // caller meant; reporting it as an unknown field as well sends an agent to delete the
    // value it got right.
    let f = opened(&free());
    f.ran("begin", r#"{"session": "s1"}"#);
    let before = (f.revision(), f.text());
    let errors = refused_as(
        &f,
        &extend(
            before.0,
            "n000000a",
            "end",
            json!({"type": "lenght", "length": 2}),
        ),
        "by.type",
        &before,
    );
    // BREAKS: the reader stops at the bad `type` and leaves `length` unread, so it's
    // refused as well, "by.length isn't a field of this request", suggesting "leave it out".
    assert_eq!(errors.as_array().unwrap().len(), 1, "{errors}");
}

#[test]
fn increments_the_numbers_cant_hold_are_refused_naming_by() {
    let f = opened(&free());
    f.ran("begin", r#"{"session": "s1"}"#);
    let before = (f.revision(), f.text());
    let at = before.0;
    for by in [
        // Under a nanometre: a document's length is whole nanometres.
        r#"{"type": "length", "length": 1e-10}"#,
        r#"{"type": "length", "length": 0.0000005}"#,
        // Past any length a file holds, and past any f64.
        r#"{"type": "length", "length": 1e400}"#,
        r#"{"type": "length", "length": 1e300}"#,
        // An angle under a nanodegree, and past any a file holds.
        r#"{"type": "angle", "angle": 1e-12}"#,
        r#"{"type": "angle", "angle": 1e300}"#,
        // Points past the sketch's reach, finite and not.
        r#"{"type": "point", "point": [1e300, 0]}"#,
        r#"{"type": "point", "point": [1e400, 0]}"#,
        r#"{"type": "point", "point": [10000000, 0]}"#,
    ] {
        let curve = if by.contains("angle") {
            "r000000a"
        } else {
            "n000000a"
        };
        let request = format!(
            r#"{{"session": "s1", "revision": {at}, "sketch": "s0000001", "curve": "{curve}", "end": "end", "by": {by}}}"#
        );
        let errors = f.refused("sketch_extend", &request);
        let parameter = errors[0]["parameter"].as_str().unwrap_or("");
        assert!(
            parameter.starts_with("by"),
            "{by}: names {parameter}: {errors}"
        );
        assert!(
            errors[0]["suggestion"].is_string()
                || errors[0]["message"].as_str().unwrap().contains(" must be "),
            "{by}: {errors}"
        );
        assert_eq!((f.revision(), f.text()), before, "{by}");
    }
    // A length past the reach the drag's target can go: 1e6 mm on a 10 mm line.
    let errors = f.refused(
        "sketch_extend",
        &extend(
            at,
            "n000000a",
            "end",
            json!({"type": "length", "length": 1000000}),
        ),
    );
    assert_eq!(errors[0]["parameter"], "by", "{errors}");
    assert_eq!(errors[0]["elements"], json!(["n000000a"]), "{errors}");
    assert_eq!((f.revision(), f.text()), before);
}

#[test]
fn points_at_beside_and_behind_a_lines_ends_are_refused_or_land_exactly() {
    let f = opened(&free());
    f.ran("begin", r#"{"session": "s1"}"#);
    let before = (f.revision(), f.text());
    let at = before.0;
    // The end itself, its y a negative zero: the same place.
    let errors = refused_as(
        &f,
        &extend(
            at,
            "n000000a",
            "end",
            json!({"type": "point", "point": [10, -0.0]}),
        ),
        "by",
        &before,
    );
    assert!(
        errors[0]["message"]
            .as_str()
            .unwrap()
            .contains("changes nothing"),
        "{errors}"
    );
    // The other end: a line of nothing.
    refused_as(
        &f,
        &extend(
            at,
            "n000000a",
            "end",
            json!({"type": "point", "point": [0, 0]}),
        ),
        "by",
        &before,
    );
    // Behind the other end: flipped.
    refused_as(
        &f,
        &extend(
            at,
            "n000000a",
            "end",
            json!({"type": "point", "point": [-5, 0]}),
        ),
        "by",
        &before,
    );
    // Off the line by 2e-7: refused.
    refused_as(
        &f,
        &extend(
            at,
            "n000000a",
            "end",
            json!({"type": "point", "point": [12, 0.0000002]}),
        ),
        "by",
        &before,
    );
    // Its start, to its end's far side: flipped.
    refused_as(
        &f,
        &extend(
            at,
            "n000000a",
            "start",
            json!({"type": "point", "point": [20, 0]}),
        ),
        "by",
        &before,
    );
    // Past the end, exactly where named.
    f.ran(
        "sketch_extend",
        &extend(
            at,
            "n000000a",
            "end",
            json!({"type": "point", "point": [13.25, 0]}),
        ),
    );
    assert_eq!(f.point("p0000002"), [13.25, 0.0]);
    assert_eq!(f.point("p0000001"), [0.0, 0.0]);
}

#[test]
fn an_arc_turned_to_a_whole_turn_or_past_it_or_back_to_nothing_is_refused() {
    let f = opened(&free());
    f.ran("begin", r#"{"session": "s1"}"#);
    let before = (f.revision(), f.text());
    let at = before.0;
    for (angle, end) in [
        (270.0, "end"),
        (270.0, "start"),
        (269.9999999, "end"),
        (360.0, "end"),
        (630.0, "end"),
        (-90.0, "end"),
        (-90.0, "start"),
        (-450.0, "end"),
        (-89.999999999, "end"),
    ] {
        refused_as(
            &f,
            &extend(
                at,
                "r000000a",
                end,
                json!({"type": "angle", "angle": angle}),
            ),
            "by",
            &before,
        );
    }
    // To its own centre, and to its other end: off its circle, and nothing left.
    refused_as(
        &f,
        &extend(
            at,
            "r000000a",
            "end",
            json!({"type": "point", "point": [0, 20]}),
        ),
        "by",
        &before,
    );
    refused_as(
        &f,
        &extend(
            at,
            "r000000a",
            "end",
            json!({"type": "point", "point": [10, 20]}),
        ),
        "by",
        &before,
    );
    // To (-10, 20) on its circle, a half turn's end, exactly.
    f.ran(
        "sketch_extend",
        &extend(
            at,
            "r000000a",
            "end",
            json!({"type": "point", "point": [-10, 20]}),
        ),
    );
    assert_eq!(f.point("p0000007"), [-10.0, 20.0]);
    assert_eq!(f.problems(), json!([]));
}

#[test]
fn a_construction_line_extends_to_a_circle_its_end_on_the_circle_and_the_sketch_solves_to_it() {
    // A construction line along y = 0.3 from (0, 0.3) to (1, 0.3), towards a circle about
    // (10, 0) of radius 5: it meets the circle at x = 10 − √(25 − 0.09), by hand.
    let text = document(&[&sketch(
        &[
            p("p0000001", 0.0, 0.3),
            p("p0000002", 1.0, 0.3),
            p("p0000003", 10.0, 0.0),
        ],
        &[l_construction("n000000a", "p0000001", "p0000002")],
        &[c("c000000a", "p0000003", 5.0)],
        &[],
        &[],
    )]);
    let f = opened(&text);
    f.ran(
        "sketch_extend",
        &extend(1, "n000000a", "end", json!({"type": "next_curve"})),
    );
    let end = f.saved_point("p0000002");
    let expected = 10.0 - libm::sqrt(25.0 - 0.09);
    assert!(
        (end[0] - expected).abs() < 1e-12,
        "{end:?} against {expected}"
    );
    assert_eq!(end[1], 0.3);
    let off = libm::hypot(end[0] - 10.0, end[1]) - 5.0;
    assert!(off.abs() <= 1e-7, "{off}");
    assert_eq!(f.problems(), json!([]));
}

#[test]
fn an_end_held_on_an_axis_extended_past_it_saves_a_sketch_that_solves_to_its_file() {
    // n000000a from (-10, -10) to (0, 0), its end held on the y axis. Lengthened by 2 along
    // it, the end's target, (√2, √2), is off the axis: the drag slides the end up the axis
    // as far as it can, so the extend is made. The saved sketch must solve to its own
    // file (no drift), with the end on the axis (x = 0), as its constraint holds it.
    for inside in [false, true] {
        let text = document(&[&sketch(
            &[p("p0000001", -10.0, -10.0), p("p0000002", 0.0, 0.0)],
            &[l("n000000a", "p0000001", "p0000002")],
            &[],
            &[],
            &[r#"{"id": "k0000001", "type": "on", "point": "p0000002", "curve": "y_axis"}"#],
        )]);
        let f = opened(&text);
        assert_eq!(f.problems(), json!([]));
        if inside {
            f.ran("begin", r#"{"session": "s1"}"#);
        }
        let envelope = f.call(
            "sketch_extend",
            &extend(1, "n000000a", "end", json!({"type": "length", "length": 2})),
        );
        // Made, not refused: the drag slides the end up the axis, so it moves.
        assert_eq!(envelope["ok"], true, "inside {inside}: {envelope}");
        if inside {
            f.ran("commit", r#"{"session": "s1"}"#);
        }
        assert_eq!(f.problems(), json!([]), "inside {inside}: {envelope}");
        f.close();
        f.open();
        assert_eq!(f.problems(), json!([]), "inside {inside}: reopened");
        let end = f.saved_point("p0000002");
        assert!(
            end[0].abs() <= 1e-9,
            "inside {inside}: the end left the axis: {end:?}"
        );
    }
}

#[test]
fn a_line_extended_at_an_end_an_arc_shares_saves_a_sketch_that_solves_to_its_file_and_lists_what_moved()
 {
    // n000000a from (0, 0) to (10, 0); r000000a about (10, 5) from (10, 0), its start the
    // line's end, a half turn to (10, 10). Lengthened by 2, the line's end goes to (12, 0)
    // and the arc follows: whatever the drag does with it, the saved sketch solves to its
    // file, and every point the file changes is in `modified`.
    let text = document(&[&sketch(
        &[
            p("p0000001", 0.0, 0.0),
            p("p0000002", 10.0, 0.0),
            p("p0000003", 10.0, 5.0),
            p("p0000004", 10.0, 10.0),
        ],
        &[l("n000000a", "p0000001", "p0000002")],
        &[],
        &[a("r000000a", "p0000003", "p0000002", "p0000004")],
        &[],
    )]);
    let mut texts = Vec::new();
    for _ in 0..2 {
        let f = opened(&text);
        let before = Fixture::points_of(&std::fs::read_to_string(f.path()).unwrap());
        let result = f.ran(
            "sketch_extend",
            &extend(1, "n000000a", "end", json!({"type": "length", "length": 2})),
        );
        assert_eq!(f.saved_point("p0000002"), [12.0, 0.0], "{result}");
        let after = Fixture::points_of(&std::fs::read_to_string(f.path()).unwrap());
        let moved: Vec<&String> = before
            .iter()
            .zip(&after)
            .filter(|(b, a)| b.1.to_bits() != a.1.to_bits() || b.2.to_bits() != a.2.to_bits())
            .map(|(b, _)| &b.0)
            .collect();
        let modified: Vec<String> = result["modified"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap().to_owned())
            .collect();
        for id in &moved {
            assert!(modified.contains(id), "{id} moved, not listed: {result}");
        }
        assert_eq!(f.problems(), json!([]));
        f.close();
        f.open();
        assert_eq!(f.problems(), json!([]), "reopened");
        texts.push(std::fs::read(f.path()).unwrap());
    }
    // The same request on the same file, twice: the same bytes.
    assert_eq!(texts[0], texts[1]);
}

#[test]
fn an_extend_of_a_file_whose_coordinates_drift_lists_every_point_it_writes() {
    // n000000b held horizontal but stored from (0, 5) to (10, 5.5): its solve moves an end
    // to meet it (the file drifts). n000000a is free. The extend begins from the sketch as
    // solved and written back, so what it writes has n000000b's ends where the solve put
    // them, not where the file had them. Inside a transaction, every point the edit changes
    // in the session's text must be in `modified`; outside one, every point the save
    // changes in the file must be in `modified`, or the commit must say it re-solved the
    // sketch (`resolved`), or a client trusting the result misses one.
    let text = document(&[&sketch(
        &[
            p("p0000001", 0.0, 0.0),
            p("p0000002", 10.0, 0.0),
            p("p0000003", 0.0, 5.0),
            p("p0000004", 10.0, 5.5),
        ],
        &[
            l("n000000a", "p0000001", "p0000002"),
            l("n000000b", "p0000003", "p0000004"),
        ],
        &[],
        &[],
        &[r#"{"id": "k0000001", "type": "horizontal", "line": "n000000b"}"#],
    )]);
    let moved = |before: &str, after: &str| -> Vec<String> {
        Fixture::points_of(before)
            .iter()
            .zip(&Fixture::points_of(after))
            .filter(|(b, a)| b.1.to_bits() != a.1.to_bits() || b.2.to_bits() != a.2.to_bits())
            .map(|(b, _)| b.0.clone())
            .collect()
    };
    let listed = |result: &Value| -> Vec<String> {
        result["modified"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap().to_owned())
            .collect()
    };
    let mut unlisted = Vec::new();
    for inside in [true, false] {
        let f = opened(&text);
        let (before, result, after) = if inside {
            f.ran("begin", r#"{"session": "s1"}"#);
            let before = f.text();
            let result = f.ran(
                "sketch_extend",
                &extend(1, "n000000a", "end", json!({"type": "length", "length": 1})),
            );
            // Begun from the solve written back: n000000b's end where the solve put it,
            // level with its start at y = 5, and listed with the extended end.
            let after = f.text();
            let end = Fixture::points_of(&after)
                .into_iter()
                .find(|(id, _, _)| id == "p0000004")
                .map(|(_, x, y)| [x, y]);
            assert_eq!(end, Some([10.0, 5.0]), "{after}");
            assert_eq!(listed(&result), ["p0000002", "p0000004"], "{result}");
            (before, result, after)
        } else {
            let before = std::fs::read_to_string(f.path()).unwrap();
            let result = f.ran(
                "sketch_extend",
                &extend(1, "n000000a", "end", json!({"type": "length", "length": 1})),
            );
            (before, result, std::fs::read_to_string(f.path()).unwrap())
        };
        let resolved = result["commit"]["resolved"]
            .as_array()
            .is_some_and(|r| r.contains(&json!("s0000001")));
        for id in moved(&before, &after) {
            if !listed(&result).contains(&id) && !resolved {
                unlisted.push(format!("inside {inside}: {id} changed, unlisted: {result}"));
            }
        }
    }
    // BREAKS: `modified` is measured against the solved sketch the extend began from, not
    // the sketch the document held, so n000000b's end p0000004, moved from (10, 5.5) to
    // (10, 5) by the begin's solve, is in neither `modified` nor (outside a transaction,
    // the saved text being already solved) the commit's `resolved`.
    assert!(unlisted.is_empty(), "{unlisted:#?}");
}

#[test]
fn two_extends_pipelined_against_one_revision_apply_once() {
    let f = opened(&free());
    f.ran("begin", r#"{"session": "s1"}"#);
    let at = f.revision();
    let request = f.with_session(&extend(
        at,
        "n000000a",
        "end",
        json!({"type": "length", "length": 2.5}),
    ));
    let first = match f.sessions.call("sketch_extend", &request) {
        Call::Running(calling) => calling,
        Call::Unknown => panic!("no sketch_extend"),
    };
    let second = match f.sessions.call("sketch_extend", &request) {
        Call::Running(calling) => calling,
        Call::Unknown => panic!("no sketch_extend"),
    };
    let answers: Vec<Value> = [first, second]
        .into_iter()
        .map(|c| {
            let Ok(answered) = c.wait_timeout(HANG) else {
                panic!("an extend didn't answer in {HANG:?}")
            };
            serde_json::from_str(&answered.text).unwrap()
        })
        .collect();
    let ok = answers.iter().filter(|a| a["ok"] == true).count();
    assert_eq!(ok, 1, "{answers:?}");
    let refused = answers.iter().find(|a| a["ok"] == false).unwrap();
    assert_eq!(refused["errors"][0]["code"], "conflict", "{refused}");
    assert_eq!(f.point("p0000002"), [12.5, 0.0]);
}

#[test]
fn a_line_in_a_sketch_past_a_drags_limits_extends_in_good_time() {
    // 250 free lines, 1,000 unknowns: past MAX_DRAG_UNKNOWNS (256), which bounds a drag's
    // moves, not an extend's one move (open item 12c), and within a solve's 1,024. Each line
    // from (0, 3i) to (10, 3i).
    let mut points = Vec::new();
    let mut lines = Vec::new();
    for i in 0..250 {
        let y = f64::from(i) * 3.0;
        points.push(p(&format!("p{:07}", 2 * i + 1), 0.0, y));
        points.push(p(&format!("p{:07}", 2 * i + 2), 10.0, y));
        lines.push(l(
            &format!("n{:07}", i + 1),
            &format!("p{:07}", 2 * i + 1),
            &format!("p{:07}", 2 * i + 2),
        ));
    }
    let f = opened(&document(&[&sketch(&points, &lines, &[], &[], &[])]));
    f.ran("begin", r#"{"session": "s1"}"#);
    let started = Instant::now();
    let result = f.ran(
        "sketch_extend",
        &extend(1, "n0000150", "end", json!({"type": "length", "length": 2})),
    );
    let took = started.elapsed();
    assert_eq!(result["modified"], json!(["p0000300"]), "{result}");
    assert_eq!(f.point("p0000300"), [12.0, 447.0]);
    assert!(took < Duration::from_secs(20), "took {took:?}");
}

#[test]
fn an_arcs_end_its_constraints_hold_turned_settles_as_freecads_extend_and_solve_do() {
    // r000000a about (0, 20), radius 10, from (10, 20) to (0, 30), its end held at (0, 30)
    // by a horizontal and a vertical distance from the origin. Turned on by 30°, the end is
    // written at (−5, 28.66…), which its constraints pull back: the commit's solve settles
    // it with the end at (0, 30) and the centre and the start moved by microns. FreeCAD
    // 1.1.3 does the same (`extend(0, radians(30), 2)`, then `solve()`, in freecadcmd): its
    // centre (−0.003829600514179188, 19.995896891677567) and start (10.000273394437956,
    // 20.00001200728184), the values below. Decided (2026-10-30): an extend is refused only
    // when it changes nothing. The held line can't lengthen, so it's refused; the held arc
    // changes (its centre and start move to make room), so it's made, as FreeCAD's is.
    let text = document(&[&sketch(
        &[
            p("p0000005", 0.0, 20.0),
            p("p0000006", 10.0, 20.0),
            p("p0000007", 0.0, 30.0),
        ],
        &[],
        &[],
        &[a("r000000a", "p0000005", "p0000006", "p0000007")],
        &[
            r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000007", "value": 0, "driving": true}"#,
            r#"{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000007", "value": 30, "driving": true}"#,
        ],
    )]);
    let f = opened(&text);
    assert_eq!(f.problems(), json!([]));
    let result = f.ran(
        "sketch_extend",
        &extend(1, "r000000a", "end", json!({"type": "angle", "angle": 30})),
    );
    assert_eq!(result["modified"], json!(["p0000007"]), "{result}");
    assert_eq!(
        result["commit"]["resolved"],
        json!(["s0000001"]),
        "{result}"
    );
    let close = |got: [f64; 2], want: [f64; 2]| {
        assert!(
            (got[0] - want[0]).abs() < 1e-9 && (got[1] - want[1]).abs() < 1e-9,
            "{got:?} against FreeCAD's {want:?}"
        );
    };
    close(f.saved_point("p0000007"), [0.0, 30.0]);
    close(
        f.saved_point("p0000005"),
        [-0.003829600514179188, 19.995896891677567],
    );
    close(
        f.saved_point("p0000006"),
        [10.000273394437956, 20.00001200728184],
    );
    assert_eq!(f.problems(), json!([]));
}

#[test]
fn a_line_with_a_length_extended_to_the_next_curve_slides_its_end_onto_it() {
    // n000000a from (0, 0) to (10, 0), its length 10 and horizontal; n000000b across x = 15.
    // To the next curve, the line slides (its constraints win): its end at (15, 0) exactly,
    // its start at (5, 0); the sketch solves to its file.
    let text = document(&[&sketch(
        &[
            p("p0000001", 0.0, 0.0),
            p("p0000002", 10.0, 0.0),
            p("p0000003", 15.0, -5.0),
            p("p0000004", 15.0, 5.0),
        ],
        &[
            l("n000000a", "p0000001", "p0000002"),
            l("n000000b", "p0000003", "p0000004"),
        ],
        &[],
        &[],
        &[
            r#"{"id": "k0000001", "type": "length", "line": "n000000a", "value": 10, "driving": true}"#,
            r#"{"id": "k0000002", "type": "horizontal", "line": "n000000a"}"#,
        ],
    )]);
    let f = opened(&text);
    let result = f.ran(
        "sketch_extend",
        &extend(1, "n000000a", "end", json!({"type": "next_curve"})),
    );
    assert_eq!(
        result["modified"],
        json!(["p0000001", "p0000002"]),
        "{result}"
    );
    assert_eq!(f.saved_point("p0000002"), [15.0, 0.0]);
    let start = f.saved_point("p0000001");
    assert!(
        (start[0] - 5.0).abs() < 1e-9 && start[1].abs() < 1e-9,
        "{start:?}"
    );
    assert_eq!(f.problems(), json!([]));
}

#[test]
fn an_extend_rolled_back_leaves_the_text_and_the_file_as_they_were_and_the_next_runs() {
    let f = opened(&free());
    let file = std::fs::read(f.path()).unwrap();
    f.ran("begin", r#"{"session": "s1"}"#);
    let text = f.text();
    f.ran(
        "sketch_extend",
        &extend(1, "r000000a", "end", json!({"type": "angle", "angle": 45})),
    );
    f.ran("rollback", r#"{"session": "s1"}"#);
    assert_eq!(f.text(), text);
    assert_eq!(std::fs::read(f.path()).unwrap(), file);
    let at = f.revision();
    f.ran(
        "sketch_extend",
        &extend(
            at,
            "n000000a",
            "start",
            json!({"type": "length", "length": 1}),
        ),
    );
    assert_eq!(f.saved_point("p0000001"), [-1.0, 0.0]);
}

mod wire {
    //! The request read back from what it writes, for every increment a request holds: any
    //! whole nanometre, any whole nanodegree, any finite coordinate (subnormals, −0, the
    //! largest), each bit for bit.
    use proptest::prelude::*;
    use silico_core::command::wire::sketch_edits::SketchExtendRequest;
    use silico_core::command::wire::{Layout, ToWire, write};
    use silico_core::command::{ExtendBy, Revision};
    use silico_core::document::json::read;
    use silico_core::document::{Angle, Coordinate, Length};

    fn edgy_f64() -> impl Strategy<Value = f64> {
        prop_oneof![
            Just(0.0),
            Just(-0.0),
            Just(5e-324),
            Just(-5e-324),
            Just(f64::MIN_POSITIVE),
            Just(f64::MAX),
            Just(f64::MIN),
            Just(1e-7),
            Just(1.0000000000000002e-7),
            Just(0.1 + 0.2),
            any::<f64>().prop_filter("finite", |v| v.is_finite()),
            (-1e6..1e6f64),
        ]
    }

    fn by() -> impl Strategy<Value = ExtendBy> {
        prop_oneof![
            // i64::MIN is no value a request holds: the reader takes magnitudes up to i64::MAX
            // either way, so a client can't write it.
            prop_oneof![
                Just(i64::MIN + 1),
                Just(i64::MAX),
                Just(0),
                Just(-1),
                Just(1),
                (i64::MIN + 1..=i64::MAX)
            ]
            .prop_map(|nm| ExtendBy::Length(Length::from_nm(nm))),
            prop_oneof![
                Just(i64::MIN + 1),
                Just(i64::MAX),
                Just(0),
                Just(-1),
                (i64::MIN + 1..=i64::MAX)
            ]
            .prop_map(|nd| ExtendBy::Angle(Angle::from_ndeg(nd))),
            (edgy_f64(), edgy_f64()).prop_map(|(x, y)| ExtendBy::Point([
                Coordinate::new(x).unwrap(),
                Coordinate::new(y).unwrap()
            ])),
            Just(ExtendBy::NextCurve),
        ]
    }

    fn bits(by: &ExtendBy) -> (u8, i64, u64, u64) {
        match by {
            ExtendBy::Length(l) => (0, l.nm(), 0, 0),
            ExtendBy::Angle(a) => (1, a.ndeg(), 0, 0),
            ExtendBy::Point([x, y]) => (2, 0, x.mm().to_bits(), y.mm().to_bits()),
            ExtendBy::NextCurve => (3, 0, 0, 0),
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

        #[test]
        fn every_request_reads_back_as_written(by in by(), start in any::<bool>()) {
            let text = format!(
                r#"{{"session": "s1", "revision": 7, "sketch": "s0000001", "curve": "na000002", "end": "{}", "by": {{"type": "next_curve"}}}}"#,
                if start { "start" } else { "end" }
            );
            let mut request = SketchExtendRequest::read(&read(text.as_bytes()).unwrap()).unwrap();
            request.by = by;
            assert_eq!(request.revision, Revision::from_number(7).unwrap());
            let written = write(&request.to_wire(), Layout::Canonical).unwrap();
            let again = match SketchExtendRequest::read(&read(written.as_bytes()).unwrap()) {
                Ok(again) => again,
                Err(problems) => panic!("{written} refused: {problems:?}"),
            };
            prop_assert_eq!(bits(&again.by), bits(&by), "{}", written);
            prop_assert_eq!(again.end, request.end);
        }
    }
}

#[test]
fn a_line_at_the_end_of_a_chain_of_a_solves_largest_component_answers_in_good_time() {
    // 511 lines chained end to end along y = 0, each horizontal: 512 points, 1,024 unknowns,
    // one component, a solve's limit (open item 12c: "a line's one move runs under a solve's
    // time, up to 1,024 unknowns"). The last line lengthened by 1: the answer, Ok or a
    // refusal saying what to do, comes in good time, and the session runs the next command.
    let mut points = Vec::new();
    let mut lines = Vec::new();
    let mut constraints = Vec::new();
    for i in 0..512 {
        points.push(p(&format!("p{:07}", i + 1), f64::from(i), 0.0));
    }
    for i in 0..511 {
        lines.push(l(
            &format!("n{:07}", i + 1),
            &format!("p{:07}", i + 1),
            &format!("p{:07}", i + 2),
        ));
        constraints.push(format!(
            r#"{{"id": "k{:07}", "type": "horizontal", "line": "n{:07}"}}"#,
            i + 1,
            i + 1
        ));
    }
    let constraints: Vec<&str> = constraints.iter().map(String::as_str).collect();
    let f = opened(&document(&[&sketch(
        &points,
        &lines,
        &[],
        &[],
        &constraints,
    )]));
    let gated = f.call("status", r#"{"session": "s1"}"#);
    assert_eq!(gated["result"]["gated"], true, "{gated}");
    f.ran("begin", r#"{"session": "s1"}"#);
    let started = Instant::now();
    let envelope = f.call(
        "sketch_extend",
        &extend(1, "n0000511", "end", json!({"type": "length", "length": 1})),
    );
    let took = started.elapsed();
    if envelope["ok"] == true {
        assert_eq!(f.point("p0000512"), [512.0, 0.0]);
    } else {
        assert!(
            envelope["errors"][0]["suggestion"].is_string(),
            "{envelope}"
        );
    }
    assert!(took < Duration::from_secs(30), "took {took:?}: {envelope}");
    f.ran("rollback", r#"{"session": "s1"}"#);
}

#[test]
fn a_line_at_the_end_of_a_chain_of_parallels_on_a_diagonal_answers_in_good_time() {
    // As above, but the 511 lines on y = x, each parallel to the one before: every unknown
    // coupled to its neighbours in both x and y. Lengthened by 1.5 along y = x, the end goes
    // from (511, 511) about 1.06 up and across.
    let mut points = Vec::new();
    let mut lines = Vec::new();
    let mut constraints = Vec::new();
    for i in 0..512 {
        points.push(p(&format!("p{:07}", i + 1), f64::from(i), f64::from(i)));
    }
    for i in 0..511 {
        lines.push(l(
            &format!("n{:07}", i + 1),
            &format!("p{:07}", i + 1),
            &format!("p{:07}", i + 2),
        ));
        if i > 0 {
            constraints.push(format!(
                r#"{{"id": "k{:07}", "type": "parallel", "a": "n{:07}", "b": "n{:07}"}}"#,
                i,
                i,
                i + 1
            ));
        }
    }
    let constraints: Vec<&str> = constraints.iter().map(String::as_str).collect();
    let f = opened(&document(&[&sketch(
        &points,
        &lines,
        &[],
        &[],
        &constraints,
    )]));
    let gated = f.call("status", r#"{"session": "s1"}"#);
    assert_eq!(gated["result"]["gated"], true, "{gated}");
    f.ran("begin", r#"{"session": "s1"}"#);
    let started = Instant::now();
    let envelope = f.call(
        "sketch_extend",
        &extend(
            1,
            "n0000511",
            "end",
            json!({"type": "length", "length": 1.5}),
        ),
    );
    let took = started.elapsed();
    if envelope["ok"] == false {
        assert!(
            envelope["errors"][0]["suggestion"].is_string(),
            "{envelope}"
        );
    }
    assert!(took < Duration::from_secs(30), "took {took:?}: {envelope}");
    f.ran("rollback", r#"{"session": "s1"}"#);
}
