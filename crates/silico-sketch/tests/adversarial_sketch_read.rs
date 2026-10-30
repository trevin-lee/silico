//! break-it on the `sketch` read (docs/design/m5.0-app.md, section 7, *`sketch` (M5.10)*),
//! through the sessions' call by name (`Sessions::call`), each sketch solved by PlaneGCS in
//! this crate's worker: what the read says of a sketch the solve refuses, against what
//! `check` of the same model says of it; each plane's frame right handed and orthonormal; a
//! reference dimension stored wrong and points stored off their solve, read as solved; reads
//! cancelled as they start, over and over, and the next read still right; reads at once,
//! byte for byte alike; and hostile requests. Expected values: by hand, from the schema's own
//! words, and from `check`.

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

/// Long enough for any read here, so one that hangs fails instead.
const HANG: Duration = Duration::from_secs(60);

struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn fixture() -> Fixture {
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
    Fixture { dir, sessions }
}

impl Fixture {
    fn open(&self, name: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        let opened = self.call(
            "open",
            &json!({"path": path.to_str().unwrap(), "create": false}).to_string(),
        );
        assert!(opened.ok, "{}", opened.text);
        path
    }

    fn call(&self, name: &str, arguments: &str) -> Answered {
        match self.sessions.call(name, arguments) {
            Call::Running(calling) => calling
                .wait_timeout(HANG)
                .unwrap_or_else(|_| panic!("{name} didn't answer in {HANG:?}")),
            Call::Unknown => panic!("no command {name}"),
        }
    }

    fn envelope(&self, name: &str, arguments: &str) -> Value {
        serde_json::from_str(&self.call(name, arguments).text).unwrap()
    }

    fn read(&self, model: &str, sketch: &str) -> Value {
        self.envelope(
            "sketch",
            &json!({"session": "s1", "model": model, "sketch": sketch}).to_string(),
        )
    }

    fn check(&self) -> Value {
        self.envelope(
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
        )
    }
}

fn document(features: &[&str]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

/// A sketch `k7f2qx3m` on `plane` of these lists (each the JSON inside its brackets).
fn sketch_on(plane: &str, points: &str, lines: &str, circles: &str, constraints: &str) -> String {
    format!(
        r#"{{"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "{plane}", "points": [{points}], "lines": [{lines}], "circles": [{circles}], "arcs": [], "constraints": [{constraints}]}}"#
    )
}

fn sketch(points: &str, lines: &str, circles: &str, constraints: &str) -> String {
    sketch_on("xy", points, lines, circles, constraints)
}

const TWO_POINTS: &str = r#"{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 3, "construction": false}"#;
const LINE: &str =
    r#"{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}"#;

/// The codes a refused solve gives, as the schema lists them for `solve_problem`.
const SOLVE_CODES: [&str; 4] = [
    "over_constrained",
    "solve_failed",
    "invalid_result",
    "invalid_input",
];

/// The state the design gives a solve refused with `code` (section 7: `over_constrained`
/// is `over_defined`, `solve_failed` `no_solution`, `invalid_result` `invalid`; and the
/// schema's `invalid_input`, `invalid`).
fn refused_state(code: &str) -> &'static str {
    match code {
        "over_constrained" => "over_defined",
        "solve_failed" => "no_solution",
        _ => "invalid",
    }
}

#[test]
fn every_refused_solve_reads_as_check_reports_it_on_the_same_model() {
    // Each sketch alone in its document, which `check` of the current model reports on;
    // the read of that model gives the same solve problem, or none where check has none,
    // and the state the design maps it to.
    let cases: Vec<(&str, String)> = vec![
        (
            "two driving lengths that differ",
            sketch(
                TWO_POINTS,
                LINE,
                "",
                r#"{"id": "k0000001", "type": "length", "line": "n0000001", "value": 10, "driving": true}, {"id": "k0000002", "type": "length", "line": "n0000001", "value": 12, "driving": true}"#,
            ),
        ),
        (
            "a horizontal given twice",
            sketch(
                TWO_POINTS,
                LINE,
                "",
                r#"{"id": "k0000001", "type": "horizontal", "line": "n0000001"}, {"id": "k0000002", "type": "horizontal", "line": "n0000001"}"#,
            ),
        ),
        (
            "a line held horizontal and vertical",
            sketch(
                TWO_POINTS,
                LINE,
                "",
                r#"{"id": "k0000001", "type": "horizontal", "line": "n0000001"}, {"id": "k0000002", "type": "vertical", "line": "n0000001"}"#,
            ),
        ),
        (
            "two points drawn at one place, a distance between them",
            sketch(
                r#"{"id": "p0000001", "x": 5, "y": 5, "construction": false}, {"id": "p0000002", "x": 5, "y": 5, "construction": false}"#,
                "",
                "",
                r#"{"id": "k0000001", "type": "distance", "a": "p0000001", "b": "p0000002", "value": 3, "driving": true}"#,
            ),
        ),
        (
            "a fixed point held on a fixed circle it's off",
            sketch(
                r#"{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 20, "y": 0, "construction": false}"#,
                "",
                r#"{"id": "c0000001", "center": "p0000001", "radius": 5, "construction": false}"#,
                r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000003", "type": "diameter", "curve": "c0000001", "value": 10, "driving": true}, {"id": "k0000004", "type": "distance_x", "a": "origin", "b": "p0000002", "value": 20, "driving": true}, {"id": "k0000005", "type": "distance_y", "a": "origin", "b": "p0000002", "value": 0, "driving": true}, {"id": "k0000006", "type": "on", "point": "p0000002", "curve": "c0000001"}"#,
            ),
        ),
        (
            "a circle drawn tiny, sized huge",
            sketch(
                r#"{"id": "p0000001", "x": 0, "y": 0, "construction": false}"#,
                "",
                r#"{"id": "c0000001", "center": "p0000001", "radius": 0.001, "construction": false}"#,
                r#"{"id": "k0000001", "type": "diameter", "curve": "c0000001", "value": 2000000, "driving": true}"#,
            ),
        ),
    ];
    for (name, text) in cases {
        let f = fixture();
        f.open("case.silico", &document(&[&text]));
        let checked = f.check();
        assert_eq!(checked["ok"], true, "{name}: {checked}");
        let on_sketch: Vec<Value> = checked["result"]["problems"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| {
                p["feature"] == "k7f2qx3m"
                    && SOLVE_CODES.contains(&p["code"].as_str().unwrap_or_default())
            })
            .cloned()
            .collect();
        let read = f.read("current", "k7f2qx3m");
        assert_eq!(read["ok"], true, "{name}: {read}");
        let result = &read["result"];
        match on_sketch.first() {
            None => assert_eq!(
                result["solve_problem"],
                Value::Null,
                "{name}: check reports no solve problem: {checked}"
            ),
            Some(problem) => {
                assert_eq!(&result["solve_problem"], problem, "{name}");
                let code = problem["code"].as_str().unwrap();
                assert_eq!(result["state"], refused_state(code), "{name}");
                assert_eq!(result["regions"], json!([]), "{name}");
                assert_eq!(result["profile_problems"], json!([]), "{name}");
                assert_eq!(result["profile_problems_total"], 0, "{name}");
            }
        }
        // Every member has a state (the schema's: none is null).
        for list in ["points", "lines", "circles", "arcs", "constraints"] {
            for member in result[list].as_array().unwrap() {
                assert!(member["state"].is_string(), "{name}: {member}");
            }
        }
    }
}

#[test]
fn each_planes_frame_is_orthonormal_and_its_normal_is_x_cross_y() {
    // The schema: "the directions of its x, its y, and its normal (x × y, about which
    // counter-clockwise turns), each a unit vector", the origin the global origin. A
    // circle's region is counter-clockwise about that normal.
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let vector = |v: &Value| -> [f64; 3] {
        let v = v.as_array().unwrap();
        [
            v[0].as_f64().unwrap(),
            v[1].as_f64().unwrap(),
            v[2].as_f64().unwrap(),
        ]
    };
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    for plane in ["xy", "xz", "yz"] {
        let f = fixture();
        f.open(
            "plane.silico",
            &document(&[&sketch_on(plane, "", "", "", "")]),
        );
        let read = f.read("current", "k7f2qx3m");
        let frame = &read["result"]["plane"];
        assert_eq!(frame["origin"], json!([0, 0, 0]), "{plane}");
        let (x, y, n) = (
            vector(&frame["x"]),
            vector(&frame["y"]),
            vector(&frame["normal"]),
        );
        assert_eq!(cross(x, y), n, "{plane}: {frame}");
        assert_eq!(
            (dot(x, x), dot(y, y), dot(x, y)),
            (1.0, 1.0, 0.0),
            "{plane}"
        );
    }
}

#[test]
fn a_reference_dimension_stored_wrong_and_points_stored_off_their_solve_read_as_solved() {
    // A line fixed from the origin, horizontal, 10 long; its end stored at (7, 2), and a
    // reference length of it stored as 99. By hand: the end is solved to (10, 0), the
    // reference measures 10, and is satisfied (a reference dimension always is); 4 unknowns,
    // 4 equations: fully defined.
    let f = fixture();
    let points = r#"{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 7, "y": 2, "construction": false}"#;
    let constraints = r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000003", "type": "horizontal", "line": "n0000001"}, {"id": "k0000004", "type": "length", "line": "n0000001", "value": 10, "driving": true}, {"id": "k0000005", "type": "length", "line": "n0000001", "value": 99, "driving": false}"#;
    f.open(
        "reference.silico",
        &document(&[&sketch(points, LINE, "", constraints)]),
    );
    let read = f.read("current", "k7f2qx3m");
    let result = &read["result"];
    assert_eq!(
        (&result["state"], &result["dofs"]),
        (&json!("fully_defined"), &json!(0)),
        "{read}"
    );
    assert_eq!(
        result["points"][1],
        json!({"id": "p0000002", "x": 10, "y": 0, "construction": false, "state": "fully_defined"})
    );
    assert_eq!(
        result["constraints"][4],
        json!({"id": "k0000005", "type": "length", "line": "n0000001", "value": 10, "driving": false, "state": "satisfied"})
    );
    // The published model is the same one: the same bytes.
    assert_eq!(f.read("published", "k7f2qx3m")["result"], *result);
}

/// A rectangle 30 by 20 from the origin, fully defined, its result the baseline.
fn rectangle() -> String {
    let points = r#"{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 30, "y": 0, "construction": false}, {"id": "p0000003", "x": 30, "y": 20, "construction": false}, {"id": "p0000004", "x": 0, "y": 20, "construction": false}"#;
    let lines = r#"{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}, {"id": "n0000002", "from": "p0000002", "to": "p0000003", "construction": false}, {"id": "n0000003", "from": "p0000003", "to": "p0000004", "construction": false}, {"id": "n0000004", "from": "p0000004", "to": "p0000001", "construction": false}"#;
    let constraints = r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000003", "type": "horizontal", "line": "n0000001"}, {"id": "k0000004", "type": "vertical", "line": "n0000002"}, {"id": "k0000005", "type": "horizontal", "line": "n0000003"}, {"id": "k0000006", "type": "vertical", "line": "n0000004"}, {"id": "k0000007", "type": "length", "line": "n0000001", "value": 30, "driving": true}, {"id": "k0000008", "type": "length", "line": "n0000002", "value": 20, "driving": true}"#;
    document(&[&sketch(points, lines, "", constraints)])
}

#[test]
fn reads_cancelled_as_they_start_leave_the_next_read_right() {
    // Each published read cancelled at once: whether its stop lands before, during, or
    // after its read, the next read, of the current model, is answered and gives the
    // baseline byte for byte.
    let f = fixture();
    f.open("rectangle.silico", &rectangle());
    let baseline = f.read("current", "k7f2qx3m");
    assert_eq!(baseline["ok"], true, "{baseline}");
    assert_eq!(baseline["result"]["state"], "fully_defined");
    let arguments = r#"{"session": "s1", "model": "published", "sketch": "k7f2qx3m"}"#;
    for round in 0..25 {
        let Call::Running(calling) = f.sessions.call("sketch", arguments) else {
            panic!("no command sketch");
        };
        if round % 5 != 0 {
            let cancelled = f.call("cancel", r#"{"session": "s1", "command": "sketch"}"#);
            let _ = cancelled;
        }
        let Ok(answered) = calling.wait_timeout(HANG) else {
            panic!("sketch didn't answer in {HANG:?}");
        };
        let envelope: Value = serde_json::from_str(&answered.text).unwrap();
        if envelope["ok"] == true {
            assert_eq!(envelope["result"], baseline["result"], "round {round}");
        } else {
            assert_eq!(envelope["errors"][0]["code"], "cancelled", "round {round}");
        }
        let next = f.read("current", "k7f2qx3m");
        assert_eq!(next, baseline, "round {round}");
    }
}

#[test]
fn reads_at_once_answer_byte_for_byte_alike() {
    // Eight published reads at once, and a current one: every answer the same bytes (each
    // reads the model's one solve of the sketch).
    let f = fixture();
    f.open("rectangle.silico", &rectangle());
    assert!(f.read("current", "k7f2qx3m")["ok"] == true);
    let baseline = f.call(
        "sketch",
        r#"{"session": "s1", "model": "published", "sketch": "k7f2qx3m"}"#,
    );
    assert!(baseline.ok, "{}", baseline.text);
    let running: Vec<_> = (0..8)
        .map(|_| {
            match f.sessions.call(
                "sketch",
                r#"{"session": "s1", "model": "published", "sketch": "k7f2qx3m"}"#,
            ) {
                Call::Running(calling) => calling,
                Call::Unknown => panic!("no command sketch"),
            }
        })
        .collect();
    for calling in running {
        let Ok(answered) = calling.wait_timeout(HANG) else {
            panic!("sketch didn't answer in {HANG:?}");
        };
        assert_eq!(answered.text, baseline.text);
    }
}

#[test]
fn hostile_sketch_requests_are_refused_as_data_and_bounded() {
    let f = fixture();
    f.open("rectangle.silico", &rectangle());
    let refused = |arguments: &str| -> Value {
        let answered = f.call("sketch", arguments);
        assert!(!answered.ok, "{arguments}");
        assert!(
            answered.text.len() < 4096,
            "a refusal of {} bytes for a request of {}",
            answered.text.len(),
            arguments.len()
        );
        serde_json::from_str(&answered.text).unwrap()
    };
    let parameter = |envelope: &Value| envelope["errors"][0]["parameter"].clone();
    // An ID a megabyte long: refused naming `sketch`, its answer not a megabyte.
    let long = format!(
        r#"{{"session": "s1", "model": "current", "sketch": "{}"}}"#,
        "k".repeat(1 << 20)
    );
    assert_eq!(parameter(&refused(&long)), "sketch");
    // The words a constraint names the sketch's own frame by, and a point's ID: none a
    // feature's.
    for word in ["origin", "x_axis", "y_axis", "p0000001"] {
        let envelope =
            refused(&json!({"session": "s1", "model": "current", "sketch": word}).to_string());
        assert_eq!(parameter(&envelope), "sketch", "{word}: {envelope}");
        assert_eq!(envelope["errors"][0]["code"], "invalid_input");
    }
    // An ID with a trailing space, an escaped NUL, and a lone surrogate.
    for word in ["k7f2qx3m ", "k7f2qx3\\u0000", "k7f2qx3\\ud800"] {
        let arguments = format!(r#"{{"session": "s1", "model": "current", "sketch": "{word}"}}"#);
        let envelope = refused(&arguments);
        assert_eq!(
            envelope["errors"][0]["code"], "invalid_input",
            "{arguments}"
        );
    }
    // `sketch` given twice, alike: refused, never the last one read.
    let twice = refused(
        r#"{"session": "s1", "model": "current", "sketch": "k7f2qx3m", "sketch": "k7f2qx3m"}"#,
    );
    assert_eq!(twice["errors"][0]["code"], "invalid_input", "{twice}");
    // Arguments that aren't an object.
    for arguments in ["[]", "\"k7f2qx3m\"", "null", "1"] {
        let envelope = refused(arguments);
        assert_eq!(
            envelope["errors"][0]["code"], "invalid_input",
            "{arguments}"
        );
    }
    // And the session reads its sketch after all of it.
    let read = f.read("current", "k7f2qx3m");
    assert_eq!(read["ok"], true, "{read}");
}

#[test]
fn a_closed_polygon_of_five_hundred_free_points_reads_as_one_region_in_bounded_time() {
    // 500 points round a square, joined in order and back: 1,000 unknowns (the
    // solver's limit 1,024), none constrained, so 1,000 left; one region, its outer loop
    // every line, counter-clockwise (the points go round counter-clockwise from n0000001).
    let n = 500;
    // Round a square 125 on a side, a point each millimetre, counter-clockwise from the
    // origin.
    let points: Vec<String> = (0..n)
        .map(|i| {
            let (side, along) = (i / 125, i % 125);
            let (x, y) = match side {
                0 => (along, 0),
                1 => (125, along),
                2 => (125 - along, 125),
                _ => (0, 125 - along),
            };
            format!(
                r#"{{"id": "p{:07}", "x": {x}, "y": {y}, "construction": false}}"#,
                i + 1
            )
        })
        .collect();
    let lines: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"id": "n{:07}", "from": "p{:07}", "to": "p{:07}", "construction": false}}"#,
                i + 1,
                i + 1,
                (i + 1) % n + 1
            )
        })
        .collect();
    let f = fixture();
    f.open(
        "polygon.silico",
        &document(&[&sketch(&points.join(", "), &lines.join(", "), "", "")]),
    );
    let started = std::time::Instant::now();
    let read = f.read("current", "k7f2qx3m");
    let took = started.elapsed();
    let result = &read["result"];
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(
        (&result["state"], &result["dofs"]),
        (&json!("under_defined"), &json!(1000))
    );
    let expected: Vec<String> = (1..=n).map(|i| format!("n{i:07}")).collect();
    assert_eq!(result["regions"], json!([{"outer": expected, "holes": []}]));
    assert!(took < Duration::from_secs(30), "{took:?}");
}
