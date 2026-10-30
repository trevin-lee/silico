//! The `sketch` read (docs/design/m5.0-app.md, section 7, *`sketch` (M5.10)*), through the
//! sessions' own command called by name, as a transport calls it (`Sessions::call`), on the
//! fake kernel, each sketch solved by PlaneGCS in this crate's worker: the design's example
//! byte for byte, and each state, solve problem, and profile the read gives, every value by
//! hand (the degrees of freedom counted from the unknowns and the equations, the states by
//! docs/ports/sketcher.md's rules, the profile's refusals as M3.5 words them), and the
//! refusals as exact JSON.

use super::bracket::{H0000001, P0000001, S0000001, S0000002, S0000003};

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::command::{Answered, Call, ModelChoice, SessionId, Sessions};
use silico_core::document::FeatureId;
use silico_core::fake::FakeKernel;
use silico_core::sketch::{SketchSolver, SketchWorker};
use tempfile::TempDir;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-sketch-worker");

/// Long enough for any read here, so one that hangs fails instead.
const HANG: Duration = Duration::from_secs(60);

/// Sessions on a fake kernel, each sketch solved by the sketch worker, in a scratch
/// directory.
struct Fixture {
    dir: TempDir,
    kernel: Arc<FakeKernel>,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sessions = Sessions::with_stand_ins(
        Arc::clone(&kernel),
        |cache: &Path| Cache::unsealed(cache),
        || {
            SketchWorker::start_worker(Path::new(WORKER))
                .map(|worker| Box::new(worker) as Box<dyn SketchSolver>)
        },
    );
    Fixture {
        dir,
        kernel,
        sessions,
    }
}

impl Fixture {
    /// `text`, written as `name` in the scratch directory, opened through `open`: the first
    /// session, `s1`.
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

    /// The command `name`'s answer to `arguments`, as a transport has it.
    fn call(&self, name: &str, arguments: &str) -> Answered {
        match self.sessions.call(name, arguments) {
            Call::Running(calling) => calling
                .wait_timeout(HANG)
                .unwrap_or_else(|_| panic!("{name} didn't answer in {HANG:?}")),
            Call::Unknown => panic!("no command {name}"),
        }
    }

    /// The envelope `sketch` answers for `sketch` of the model `model`, read through `call`.
    fn read(&self, model: &str, sketch: &str) -> Value {
        let arguments = json!({"session": "s1", "model": model, "sketch": sketch}).to_string();
        let answer = self.call("sketch", &arguments);
        let envelope: Value = serde_json::from_str(&answer.text).unwrap();
        assert_eq!(envelope["ok"], answer.ok, "{}", answer.text);
        envelope
    }

    /// The result of a `sketch` read of the current model that ran.
    fn result(&self, sketch: &str) -> Value {
        let envelope = self.read("current", sketch);
        assert_eq!(envelope["ok"], true, "{envelope}");
        envelope["result"].clone()
    }
}

fn s1() -> SessionId {
    SessionId::parse("s1").unwrap()
}

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

/// A document of `features`, each its JSON.
fn document(features: &[&str]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

/// A sketch `k7f2qx3m` on xy of these lists (each the JSON inside its brackets).
fn sketch(points: &str, lines: &str, circles: &str, constraints: &str) -> String {
    format!(
        r#"{{"id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{points}], "lines": [{lines}], "circles": [{circles}], "arcs": [], "constraints": [{constraints}]}}"#
    )
}

/// The core's committed example `name` (`sketch/result`).
fn example(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../silico-core/tests/contract/commands/examples")
        .join(format!("{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The bracket's first four features (M4.9's, as its session added them): the L profile
/// on xz, the pad, the base's holes on xy, and the pocket through them; and its upright's
/// holes on yz.
fn bracket() -> String {
    document(&[S0000001, P0000001, S0000002, H0000001, S0000003])
}

const XY: &str = r#"{"origin": [0, 0, 0], "x": [1, 0, 0], "y": [0, 1, 0], "normal": [0, 0, 1]}"#;

/// The error contract's object, every field given.
fn error(code: &str, message: &str, elements: &[&str], suggestion: &str) -> Value {
    json!({
        "code": code,
        "message": message,
        "feature": "k7f2qx3m",
        "elements": elements,
        "elements_total": elements.len(),
        "parameter": null,
        "cause": null,
        "suggestion": suggestion,
        "candidates": [],
        "advice": null,
    })
}

/// M3.5's refusal of an open end: `point` ends `line` and no other profile curve.
fn open_end(point: &str, line: &str) -> Value {
    error(
        "invalid_input",
        &format!(
            "point {point} ends line {line} and no other profile curve, so the profile is open there"
        ),
        &[point, line],
        "end another curve at the point to close the outline, or make the curve construction geometry",
    )
}

#[test]
fn the_brackets_base_holes_read_as_the_design_gives_them_byte_for_byte() {
    // The design's example. By hand: two centres and two radii, 6 unknowns; the diameter
    // fixes cb000001's radius at 5.5 / 2 = 2.75, the equal cb000002's at it, the two
    // distances from the origin place pb000001 at (40, 12), the vertical_points puts
    // pb000002 at x 40, and the distance_y 16 above it, at y 28: 6 independent equations, 0
    // left, every member fully defined and every constraint satisfied. Its profile: each
    // circle a loop of its own, apart, so two regions with no holes, by their curves' IDs.
    let f = fixture();
    f.open("bracket.silico", &bracket());
    let answer = f.call("sketch", &example("sketch/request"));
    assert!(answer.ok, "{}", answer.text);
    let envelope: Value = serde_json::from_str(&answer.text).unwrap();
    let designed: Value = serde_json::from_str(&example("sketch/result")).unwrap();
    assert_eq!(envelope["result"], designed);
    assert_eq!(
        envelope["model"],
        json!({"revision": 1, "of": "current", "fault": null})
    );
    // The typed command, written as the API writes it: the design's text, byte for byte.
    let typed = f
        .sessions
        .sketch(&s1(), ModelChoice::Current, id("s0000002"))
        .wait_timeout(HANG)
        .unwrap_or_else(|_| panic!("sketch didn't answer"))
        .result
        .unwrap();
    assert_eq!(
        write(&typed.to_wire(), Layout::Canonical).unwrap() + "\n",
        example("sketch/result")
    );
    // The published model's read is the same sketch, solved the same.
    let published = f.read("published", "s0000002");
    assert_eq!(published["result"], designed);
    assert_eq!(
        published["model"],
        json!({"revision": 1, "of": "published", "fault": null})
    );
}

#[test]
fn the_brackets_other_sketches_give_their_planes_frames_and_the_l_profiles_one_region() {
    // xz: the sketch's x along +X, its y along +Z, its normal x × y = −Y; yz: x along +Y,
    // y along +Z, normal +X (D6). The L profile: 12 unknowns, 12 independent equations
    // (three horizontals, three verticals, the corner at the origin, four lengths), 0 left;
    // its one loop from na000001 (the smallest ID), (0, 0) to (60, 0), up, back, up, back,
    // and down to the start: counter-clockwise, so in that order.
    let f = fixture();
    f.open("bracket.silico", &bracket());
    let l = f.result("s0000001");
    assert_eq!(
        l["plane"],
        json!({"origin": [0, 0, 0], "x": [1, 0, 0], "y": [0, 0, 1], "normal": [0, -1, 0]})
    );
    assert_eq!(
        (&l["state"], &l["dofs"]),
        (&json!("fully_defined"), &json!(0))
    );
    assert_eq!(
        l["regions"],
        json!([{"outer": ["na000001", "na000002", "na000003", "na000004", "na000005", "na000006"], "holes": []}])
    );
    assert_eq!(
        l["points"][3],
        json!({"id": "pa000004", "x": 5, "y": 5, "construction": false, "state": "fully_defined"})
    );
    let upright = f.result("s0000003");
    assert_eq!(
        upright["plane"],
        json!({"origin": [0, 0, 0], "x": [0, 1, 0], "y": [0, 0, 1], "normal": [1, 0, 0]})
    );
    assert_eq!(upright["state"], "fully_defined");
    assert_eq!(
        upright["regions"],
        json!([{"outer": ["cb000001"], "holes": []}, {"outer": ["cb000002"], "holes": []}])
    );
}

#[test]
fn a_free_line_is_under_defined_by_four_degrees_and_open_at_both_ends() {
    // No equations: two points of 2 values each, all free: 4 left (FreeCAD 1.1.3: 4). Every
    // member under defined; the solve leaves free values where they're stored. Its profile
    // is open at each end, M3.5's refusal for each, in point order.
    let f = fixture();
    let points = r#"{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 3, "construction": false}"#;
    let lines =
        r#"{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}"#;
    f.open("line.silico", &document(&[&sketch(points, lines, "", "")]));
    assert_eq!(
        f.result("k7f2qx3m"),
        json!({
            "sketch": "k7f2qx3m",
            "plane": serde_json::from_str::<Value>(XY).unwrap(),
            "state": "under_defined",
            "dofs": 4,
            "points": [
                {"id": "p0000001", "x": 0, "y": 0, "construction": false, "state": "under_defined"},
                {"id": "p0000002", "x": 10, "y": 3, "construction": false, "state": "under_defined"},
            ],
            "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false, "state": "under_defined"}],
            "circles": [],
            "arcs": [],
            "constraints": [],
            "regions": [],
            "profile_problems": [open_end("p0000001", "n0000001"), open_end("p0000002", "n0000001")],
            "profile_problems_total": 2,
            "solve_problem": null,
        })
    );
}

#[test]
fn two_driving_lengths_that_differ_over_define_the_line_given_as_stored() {
    // Lengths 10 and 12 on one line, its end stored at (11, 0.5), off both: the two rows'
    // gradients are both the line's direction, so they conflict (PlaneGCS's conflicting
    // pair) and the solve is refused as over_constrained, naming both. Over defined, not
    // no_solution (open item 32). Each length reads both ends and names the line: all over
    // defined. 4 unknowns, rank 1: 3 left. Members as stored (no solution moved them),
    // no profile looked for, and the problem is the one `check` reports for the sketch.
    let f = fixture();
    let points = r#"{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 11, "y": 0.5, "construction": false}"#;
    let lines =
        r#"{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}"#;
    let constraints = r#"{"id": "k0000001", "type": "length", "line": "n0000001", "value": 10, "driving": true}, {"id": "k0000002", "type": "length", "line": "n0000001", "value": 12, "driving": true}"#;
    f.open(
        "conflict.silico",
        &document(&[&sketch(points, lines, "", constraints)]),
    );
    let conflict = error(
        "over_constrained",
        "the sketch's constraints conflict: these 2 can't all hold",
        &["k0000001", "k0000002"],
        "remove or change one of the constraints named",
    );
    assert_eq!(
        f.result("k7f2qx3m"),
        json!({
            "sketch": "k7f2qx3m",
            "plane": serde_json::from_str::<Value>(XY).unwrap(),
            "state": "over_defined",
            "dofs": 3,
            "points": [
                {"id": "p0000001", "x": 0, "y": 0, "construction": false, "state": "over_defined"},
                {"id": "p0000002", "x": 11, "y": 0.5, "construction": false, "state": "over_defined"},
            ],
            "lines": [{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false, "state": "over_defined"}],
            "circles": [],
            "arcs": [],
            "constraints": [
                {"id": "k0000001", "type": "length", "line": "n0000001", "value": 10, "driving": true, "state": "conflicting"},
                {"id": "k0000002", "type": "length", "line": "n0000001", "value": 12, "driving": true, "state": "conflicting"},
            ],
            "regions": [],
            "profile_problems": [],
            "profile_problems_total": 0,
            "solve_problem": conflict,
        })
    );
    // The same error `check` reports for the sketch: the read solves it as the rebuild did.
    let checked: Value = serde_json::from_str(
        &f.call(
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
        )
        .text,
    )
    .unwrap();
    assert_eq!(checked["result"]["problems"], json!([conflict]));
}

/// docs/design/m3.2-sketch-format.md's rectangle (40 by 20, its first corner at the
/// origin), fully defined by its eight constraints, then `extra`'s, as silico-sketch's
/// states tests draw it (tests/sketch_states.rs).
fn rectangle(extra: &[&str], circles: &str) -> String {
    let points = r#"{"id": "0a1b2c3d", "x": 0, "y": 0, "construction": false}, {"id": "2p3q4r5s", "x": 0, "y": 20, "construction": false}, {"id": "4e5f6g7h", "x": 40, "y": 0, "construction": false}, {"id": "8j9k0m1n", "x": 40, "y": 20, "construction": false}"#;
    let lines = r#"{"id": "6t7v8w9x", "from": "0a1b2c3d", "to": "4e5f6g7h", "construction": false}, {"id": "a0b1c2d3", "from": "4e5f6g7h", "to": "8j9k0m1n", "construction": false}, {"id": "j8k9m0n1", "from": "8j9k0m1n", "to": "2p3q4r5s", "construction": false}, {"id": "p2q3r4s5", "from": "2p3q4r5s", "to": "0a1b2c3d", "construction": false}"#;
    let mut constraints = vec![
        r#"{"id": "c0d1e2f3", "type": "horizontal", "line": "6t7v8w9x"}"#,
        r#"{"id": "g4h5j6k7", "type": "length", "line": "6t7v8w9x", "value": 40, "driving": true}"#,
        r#"{"id": "m8n9p0q1", "type": "distance_x", "a": "origin", "b": "0a1b2c3d", "value": 0, "driving": true}"#,
        r#"{"id": "r2s3t4v5", "type": "vertical", "line": "a0b1c2d3"}"#,
        r#"{"id": "w6x7y8z9", "type": "length", "line": "a0b1c2d3", "value": 20, "driving": true}"#,
        r#"{"id": "x0y1z2a3", "type": "horizontal", "line": "j8k9m0n1"}"#,
        r#"{"id": "y4z5a6b7", "type": "distance_y", "a": "origin", "b": "0a1b2c3d", "value": 0, "driving": true}"#,
        r#"{"id": "z8a9b0c1", "type": "vertical", "line": "p2q3r4s5"}"#,
    ];
    constraints.extend(extra);
    sketch(points, lines, circles, &constraints.join(", "))
}

/// The perpendicular of the rectangle's top and left: a relation its horizontal and
/// vertical already fix.
const PERPENDICULAR: &str =
    r#"{"id": "zz000001", "type": "perpendicular", "a": "j8k9m0n1", "b": "p2q3r4s5"}"#;
/// An angle of 90° between its bottom and right: a dimension its horizontal and vertical
/// already fix.
const ANGLE: &str = r#"{"id": "zz000002", "type": "angle", "a": "6t7v8w9x", "b": "a0b1c2d3", "value": 90, "driving": true}"#;

/// Each member of `list`'s state, by ID, in the read's order.
fn states(result: &Value, list: &str) -> Vec<(String, String)> {
    result[list]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["id"].as_str().unwrap().to_owned(),
                m["state"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|&(id, state)| (id.to_owned(), state.to_owned()))
        .collect()
}

/// The rectangle's eight constraints, each `satisfied`, and `extra`'s.
fn rectangle_constraints(extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut out: Vec<(&str, &str)> = [
        "c0d1e2f3", "g4h5j6k7", "m8n9p0q1", "r2s3t4v5", "w6x7y8z9", "x0y1z2a3", "y4z5a6b7",
        "z8a9b0c1",
    ]
    .into_iter()
    .map(|id| (id, "satisfied"))
    .collect();
    out.extend_from_slice(extra);
    pairs(&out)
}

#[test]
fn a_redundant_relation_leaves_the_rectangle_fully_defined_and_a_redundant_dimension_over_defines_it()
 {
    // SolidWorks' FAQ on sketch relations, its captions: "Redundant perpendicular relation
    // added. Sketch is still fully defined." and "Redundant dimension added. Sketch is over
    // defined." By hand: the top is horizontal and the left vertical, so their perpendicular
    // repeats them; the bottom is horizontal and the right vertical, so a 90° angle between
    // them repeats those. 8 unknowns, rank 8 either way: 0 left. PlaneGCS drops the latest
    // tag of each repetition, the perpendicular, then the angle: each redundant. The
    // perpendicular, a relation, over-defines nothing, so every member stays fully defined
    // and the sketch is fully defined; the angle, a driving dimension, over-defines what it
    // reads and names: the bottom's and the right's ends (0a1b2c3d, 4e5f6g7h, 8j9k0m1n) and
    // the two lines; the fourth corner, the top, and the left, fully defined. Either way the
    // one region, its outer loop from the bottom (the smallest ID), counter-clockwise.
    let f = fixture();
    f.open(
        "relation.silico",
        &document(&[&rectangle(&[PERPENDICULAR], "")]),
    );
    let relation = f.result("k7f2qx3m");
    assert_eq!(
        (
            &relation["state"],
            &relation["dofs"],
            &relation["solve_problem"]
        ),
        (&json!("fully_defined"), &json!(0), &Value::Null)
    );
    let fully = |ids: &[&str]| -> Vec<(String, String)> {
        ids.iter()
            .map(|id| ((*id).to_owned(), "fully_defined".to_owned()))
            .collect()
    };
    assert_eq!(
        states(&relation, "points"),
        fully(&["0a1b2c3d", "2p3q4r5s", "4e5f6g7h", "8j9k0m1n"])
    );
    assert_eq!(
        states(&relation, "lines"),
        fully(&["6t7v8w9x", "a0b1c2d3", "j8k9m0n1", "p2q3r4s5"])
    );
    assert_eq!(
        states(&relation, "constraints"),
        rectangle_constraints(&[("zz000001", "redundant")])
    );
    let outline = json!([{"outer": ["6t7v8w9x", "a0b1c2d3", "j8k9m0n1", "p2q3r4s5"], "holes": []}]);
    assert_eq!(relation["regions"], outline);

    let f = fixture();
    f.open(
        "dimension.silico",
        &document(&[&rectangle(&[PERPENDICULAR, ANGLE], "")]),
    );
    let dimension = f.result("k7f2qx3m");
    assert_eq!(
        (
            &dimension["state"],
            &dimension["dofs"],
            &dimension["solve_problem"]
        ),
        (&json!("over_defined"), &json!(0), &Value::Null)
    );
    assert_eq!(
        states(&dimension, "points"),
        pairs(&[
            ("0a1b2c3d", "over_defined"),
            ("2p3q4r5s", "fully_defined"),
            ("4e5f6g7h", "over_defined"),
            ("8j9k0m1n", "over_defined"),
        ])
    );
    assert_eq!(
        states(&dimension, "lines"),
        pairs(&[
            ("6t7v8w9x", "over_defined"),
            ("a0b1c2d3", "over_defined"),
            ("j8k9m0n1", "fully_defined"),
            ("p2q3r4s5", "fully_defined"),
        ])
    );
    assert_eq!(
        states(&dimension, "constraints"),
        rectangle_constraints(&[("zz000001", "redundant"), ("zz000002", "redundant")])
    );
    // The angle as the document writes it, with its state.
    assert_eq!(
        dimension["constraints"][9],
        json!({"id": "zz000002", "type": "angle", "a": "6t7v8w9x", "b": "a0b1c2d3", "value": 90, "driving": true, "state": "redundant"})
    );
    assert_eq!(dimension["regions"], outline);
}

#[test]
fn the_rectangle_given_a_second_width_that_disagrees_reads_negative_degrees_of_freedom() {
    // The rectangle's bottom given a second driving length, 50 against its 40: the two
    // lengths conflict, and nothing in the rectangle is free. PlaneGCS then counts the
    // equations left in place of their rank, none of them redundant: 8 unknowns less 9
    // equations, -1 (silico-sketch's tests/solve.rs, two widths that disagree). Over
    // defined, the problem the two lengths' conflict, both conflicting.
    const SECOND: &str =
        r#"{"id": "zz000003", "type": "length", "line": "6t7v8w9x", "value": 50, "driving": true}"#;
    let f = fixture();
    f.open("widths.silico", &document(&[&rectangle(&[SECOND], "")]));
    let read = f.result("k7f2qx3m");
    assert_eq!(
        (&read["state"], &read["dofs"]),
        (&json!("over_defined"), &json!(-1)),
        "{read}"
    );
    assert_eq!(
        read["solve_problem"],
        error(
            "over_constrained",
            "the sketch's constraints conflict: these 2 can't all hold",
            &["g4h5j6k7", "zz000003"],
            "remove or change one of the constraints named",
        )
    );
    let constraints = states(&read, "constraints");
    assert_eq!(constraints.len(), 9, "{constraints:?}");
    for (id, state) in &constraints {
        let want = if id == "g4h5j6k7" || id == "zz000003" {
            "conflicting"
        } else {
            "unsolved"
        };
        assert_eq!(state, want, "{id}: {constraints:?}");
    }
}

#[test]
fn a_circle_inside_the_rectangle_is_its_regions_hole() {
    // The rectangle, and a free circle of radius 5 about (20, 10) inside it: one region,
    // the rectangle's outline with the circle a hole (M3.5's nesting). The circle's centre
    // and radius free: 3 left, both under defined; the rectangle still fully defined.
    let f = fixture();
    let circle = r#"{"id": "cc000001", "center": "pc000001", "radius": 5, "construction": false}"#;
    let text = rectangle(&[], circle).replace(
        r#""points": ["#,
        r#""points": [{"id": "pc000001", "x": 20, "y": 10, "construction": false}, "#,
    );
    f.open("holed.silico", &document(&[&text]));
    let holed = f.result("k7f2qx3m");
    assert_eq!(
        (&holed["state"], &holed["dofs"]),
        (&json!("under_defined"), &json!(3))
    );
    assert_eq!(
        holed["regions"],
        json!([{"outer": ["6t7v8w9x", "a0b1c2d3", "j8k9m0n1", "p2q3r4s5"], "holes": [["cc000001"]]}])
    );
    assert_eq!(
        holed["circles"],
        json!([{"id": "cc000001", "center": "pc000001", "radius": 5, "construction": false, "state": "under_defined"}])
    );
    assert_eq!(holed["points"][4]["id"], "pc000001");
    assert_eq!(holed["points"][4]["state"], "under_defined");
    assert_eq!(holed["profile_problems"], json!([]));
}

#[test]
fn an_empty_sketch_is_fully_defined_with_no_degrees_of_freedom_and_no_curves_for_a_profile() {
    // Open item 32: empty is fully_defined, 0 left (PlaneGCS's −1 for no unknowns is never
    // read). M3.5's first refusal: no curves.
    let f = fixture();
    f.open("empty.silico", &document(&[&sketch("", "", "", "")]));
    assert_eq!(
        f.result("k7f2qx3m"),
        json!({
            "sketch": "k7f2qx3m",
            "plane": serde_json::from_str::<Value>(XY).unwrap(),
            "state": "fully_defined",
            "dofs": 0,
            "points": [],
            "lines": [],
            "circles": [],
            "arcs": [],
            "constraints": [],
            "regions": [],
            "profile_problems": [error(
                "invalid_input",
                "the sketch has no curves to make a profile of",
                &[],
                "draw a closed outline of lines, arcs, or circles that aren't construction geometry",
            )],
            "profile_problems_total": 1,
            "solve_problem": null,
        })
    );
}

#[test]
fn an_open_chain_makes_no_profile_its_open_ends_named() {
    // Two lines joined at p0000002, fully defined from the origin (two placing distances,
    // a horizontal and a vertical, two lengths: 6 unknowns, 6 equations): no region, and
    // M3.5's open end at each of the chain's two ends, in point order.
    let f = fixture();
    let points = r#"{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 0, "construction": false}, {"id": "p0000003", "x": 10, "y": 5, "construction": false}"#;
    let lines = r#"{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}, {"id": "n0000002", "from": "p0000002", "to": "p0000003", "construction": false}"#;
    let constraints = r#"{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}, {"id": "k0000003", "type": "horizontal", "line": "n0000001"}, {"id": "k0000004", "type": "vertical", "line": "n0000002"}, {"id": "k0000005", "type": "length", "line": "n0000001", "value": 10, "driving": true}, {"id": "k0000006", "type": "length", "line": "n0000002", "value": 5, "driving": true}"#;
    f.open(
        "chain.silico",
        &document(&[&sketch(points, lines, "", constraints)]),
    );
    let chain = f.result("k7f2qx3m");
    assert_eq!(
        (&chain["state"], &chain["dofs"]),
        (&json!("fully_defined"), &json!(0))
    );
    assert_eq!(chain["regions"], json!([]));
    assert_eq!(
        chain["profile_problems"],
        json!([
            open_end("p0000001", "n0000001"),
            open_end("p0000003", "n0000002")
        ])
    );
    assert_eq!(chain["solve_problem"], Value::Null);
}

#[test]
fn the_published_model_and_the_current_each_read_their_own_sketch() {
    // A horizontal line from the origin, 10 long, beside a box. In a transaction, the text
    // replaced: the line 12 long (its end stored there) and the box 41 long, whose new step
    // the fake kernel holds, so the edit's rebuild can't publish. The published read gives
    // revision 1's sketch at once; once the step is let go, the current read gives revision
    // 2's.
    let text = |length: u32| {
        let points = format!(
            r#"{{"id": "p0000001", "x": 0, "y": 0, "construction": false}}, {{"id": "p0000002", "x": {length}, "y": 0, "construction": false}}"#
        );
        let lines =
            r#"{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}"#;
        let constraints = format!(
            r#"{{"id": "k0000001", "type": "distance_x", "a": "origin", "b": "p0000001", "value": 0, "driving": true}}, {{"id": "k0000002", "type": "distance_y", "a": "origin", "b": "p0000001", "value": 0, "driving": true}}, {{"id": "k0000003", "type": "horizontal", "line": "n0000001"}}, {{"id": "k0000004", "type": "length", "line": "n0000001", "value": {length}, "driving": true}}"#
        );
        let block = format!(
            r#"{{"id": "b0000001", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": {}, "width": 20, "height": 10}}"#,
            length + 29
        );
        document(&[&sketch(&points, lines, "", &constraints), &block])
    };
    let f = fixture();
    f.open("edited.silico", &text(10));
    let first = f.result("k7f2qx3m");
    assert_eq!(first["points"][1]["x"], 10);
    let held = f.kernel.hold(|_| true);
    assert!(f.call("begin", r#"{"session": "s1"}"#).ok);
    let replace = json!({"session": "s1", "revision": 1, "text": text(12)}).to_string();
    let replaced = f.call("replace_text", &replace);
    assert!(replaced.ok, "{}", replaced.text);
    let published = f.read("published", "k7f2qx3m");
    assert_eq!(
        published["model"],
        json!({"revision": 1, "of": "published", "fault": null})
    );
    assert_eq!(published["session"]["revision"], 2);
    assert_eq!(published["result"], first);
    held.release();
    let current = f.read("current", "k7f2qx3m");
    assert_eq!(
        current["model"],
        json!({"revision": 2, "of": "current", "fault": null})
    );
    assert_eq!(
        current["result"]["points"][1],
        json!({"id": "p0000002", "x": 12, "y": 0, "construction": false, "state": "fully_defined"})
    );
    assert_eq!(current["result"]["constraints"][3]["value"], 12);
    assert_eq!(current["result"]["state"], "fully_defined");
}

#[test]
fn a_feature_that_isnt_a_sketch_and_an_unknown_one_are_refused_naming_sketch() {
    let f = fixture();
    let path = f.open("bracket.silico", &bracket());
    let session = json!({"id": "s1", "path": path.to_str().unwrap(), "revision": 1, "transaction": false, "reloaded": false});
    let refused = |message: &str| {
        json!({
            "ok": false,
            "session": session,
            "model": {"revision": 1, "of": "current", "fault": null},
            "errors": [{
                "code": "invalid_input",
                "message": message,
                "feature": null,
                "elements": [],
                "elements_total": 0,
                "parameter": "sketch",
                "cause": null,
                "suggestion": "name a sketch of the document, by its ID",
                "candidates": ["s0000001", "s0000002", "s0000003"],
                "advice": null,
            }],
            "total": 1,
        })
    };
    assert_eq!(
        f.read("current", "p0000001"),
        refused("p0000001 is a pad, not a sketch")
    );
    assert_eq!(
        f.read("current", "h0000001"),
        refused("h0000001 is a pocket, not a sketch")
    );
    assert_eq!(
        f.read("current", "s0000009"),
        refused("the document has no feature s0000009")
    );
}

#[test]
fn a_malformed_sketch_request_is_refused_by_the_wires_reader() {
    let f = fixture();
    let path = f.open("bracket.silico", &bracket());
    // A request that names the open session s1 is refused with its state (the contract
    // lows' item 1); one that names none, or a session that isn't open, with null.
    let s1 = json!({"id": "s1", "path": path.to_str().unwrap(), "revision": 1, "transaction": false, "reloaded": false});
    let refused = |arguments: &str| -> Value {
        let answer = f.call("sketch", arguments);
        assert!(!answer.ok, "{arguments}");
        serde_json::from_str(&answer.text).unwrap()
    };
    let error = |parameter: &str, message: &str, suggestion: Value| {
        json!({
            "code": "invalid_input",
            "message": message,
            "feature": null,
            "elements": [],
            "elements_total": 0,
            "parameter": parameter,
            "cause": null,
            "suggestion": suggestion,
            "candidates": [],
            "advice": null,
        })
    };
    let unread = |session: &Value, errors: Vec<Value>| {
        let total = errors.len();
        json!({"ok": false, "session": session, "model": null, "errors": errors, "total": total})
    };
    // A field it hasn't; a sketch's ID that isn't a string, or isn't an ID; a model that
    // isn't one; none of its fields; and two faults at once, each said, in field order.
    let leave_out = json!("leave it out: a request has only the fields its shape gives");
    assert_eq!(
        refused(
            r#"{"session": "s1", "model": "current", "sketch": "s0000002", "feature": "s0000002"}"#
        ),
        unread(
            &s1,
            vec![error(
                "feature",
                "feature isn't a field of this request",
                leave_out.clone()
            )]
        )
    );
    assert_eq!(
        refused(r#"{"session": "s1", "model": "current", "sketch": 2}"#),
        unread(
            &s1,
            vec![error(
                "sketch",
                "sketch must be a feature's ID, got 2",
                Value::Null
            )]
        )
    );
    assert_eq!(
        refused(r#"{"session": "s1", "model": "current", "sketch": "S0000002"}"#),
        unread(
            &s1,
            vec![error(
                "sketch",
                r#"sketch must be a feature's ID, got "S0000002""#,
                Value::Null
            )]
        )
    );
    assert_eq!(
        refused(r#"{"session": "s1", "model": "current", "sketch": null}"#),
        unread(
            &s1,
            vec![error(
                "sketch",
                "sketch must be a feature's ID, got null",
                Value::Null
            )]
        )
    );
    let model = refused(r#"{"session": "s1", "model": "latest", "sketch": "s0000002"}"#);
    assert_eq!(model["errors"][0]["parameter"], "model");
    assert_eq!(model["errors"][0]["code"], "invalid_input");
    assert_eq!((&model["total"], &model["session"]), (&json!(1), &s1));
    let missing = |field: &str| {
        error(
            field,
            &format!("{field} is missing"),
            json!("give every field the request's shape has; null where it allows none"),
        )
    };
    assert_eq!(
        refused("{}"),
        unread(
            &Value::Null,
            vec![missing("session"), missing("model"), missing("sketch")]
        )
    );
    assert_eq!(
        refused(r#"{"session": "s1", "sketch": 2, "model": "current", "x": 1}"#),
        unread(
            &s1,
            vec![
                error(
                    "sketch",
                    "sketch must be a feature's ID, got 2",
                    Value::Null
                ),
                error("x", "x isn't a field of this request", leave_out),
            ]
        )
    );
    // A session that isn't open: refused as every command is, naming no model.
    let closed = refused(r#"{"session": "s7", "model": "current", "sketch": "s0000002"}"#);
    assert_eq!(
        closed,
        unread(
            &Value::Null,
            vec![error(
                "session",
                "no session s7 is open",
                json!(
                    "open the document again: a session ends when it's closed or its client goes away"
                )
            )]
        )
    );
}

#[test]
fn a_sketch_that_cant_be_held_reads_invalid_from_its_diagnosis_as_check_refuses_it() {
    // An arc about p0000001 from p0000002 (5, 0) to p0000003 (8, 0), both ends at angle 0:
    // it sweeps nothing, so the translation refuses it (`invalid_input`, naming the arc) and
    // its geometry alone is diagnosed. By hand: 3 points, 6 coordinates, and the arc's radius
    // and two angles, 3; its 4 rules (each end on its circle at its angle, in x and y): 5
    // left (FreeCAD 1.1.3's lone arc), every member under defined, the sketch invalid, the
    // members as stored, no profile looked for.
    let f = fixture();
    let points = r#"{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 5, "y": 0, "construction": false}, {"id": "p0000003", "x": 8, "y": 0, "construction": false}"#;
    let text = sketch(points, "", "", "").replace(
        r#""arcs": []"#,
        r#""arcs": [{"id": "a0000001", "center": "p0000001", "start": "p0000002", "end": "p0000003", "construction": false}]"#,
    );
    f.open("unswept.silico", &document(&[&text]));
    let read = f.result("k7f2qx3m");
    assert_eq!(
        (&read["state"], &read["dofs"]),
        (&json!("invalid"), &json!(5))
    );
    assert_eq!(
        states(&read, "points"),
        pairs(&[
            ("p0000001", "under_defined"),
            ("p0000002", "under_defined"),
            ("p0000003", "under_defined"),
        ])
    );
    assert_eq!(
        states(&read, "arcs"),
        pairs(&[("a0000001", "under_defined")])
    );
    assert_eq!(read["points"][2]["x"], 8);
    let problem = &read["solve_problem"];
    assert_eq!(
        (&problem["code"], &problem["feature"], &problem["elements"]),
        (
            &json!("invalid_input"),
            &json!("k7f2qx3m"),
            &json!(["a0000001"])
        )
    );
    assert_eq!(
        (&read["regions"], &read["profile_problems"]),
        (&json!([]), &json!([]))
    );
    let checked: Value = serde_json::from_str(
        &f.call(
            "check",
            r#"{"session": "s1", "model": "current", "fresh": false}"#,
        )
        .text,
    )
    .unwrap();
    assert_eq!(checked["result"]["problems"], json!([problem]));

    // A line made horizontal and vertical: both hold only with its ends at one place, a line
    // that can't be built, so `invalid_result`. Diagnosed where it's drawn, (0, 0) to
    // (10, 3): rows (0, −1, 0, 1) and (−1, 0, 1, 0), rank 2 of 4: 2 left, every value free,
    // every member under defined, both constraints unsolved.
    let f = fixture();
    let points = r#"{"id": "p0000001", "x": 0, "y": 0, "construction": false}, {"id": "p0000002", "x": 10, "y": 3, "construction": false}"#;
    let lines =
        r#"{"id": "n0000001", "from": "p0000001", "to": "p0000002", "construction": false}"#;
    let constraints = r#"{"id": "k0000001", "type": "horizontal", "line": "n0000001"}, {"id": "k0000002", "type": "vertical", "line": "n0000001"}"#;
    f.open(
        "collapsed.silico",
        &document(&[&sketch(points, lines, "", constraints)]),
    );
    let read = f.result("k7f2qx3m");
    assert_eq!(
        (&read["state"], &read["dofs"]),
        (&json!("invalid"), &json!(2))
    );
    assert_eq!(
        states(&read, "constraints"),
        pairs(&[("k0000001", "unsolved"), ("k0000002", "unsolved")])
    );
    assert_eq!(
        states(&read, "lines"),
        pairs(&[("n0000001", "under_defined")])
    );
    assert_eq!(read["solve_problem"]["code"], "invalid_result");
    assert_eq!(read["points"][1]["y"], 3);
}

#[test]
fn a_sketch_kept_from_an_earlier_rebuild_or_carried_by_a_save_reads_as_its_first_solve() {
    // The rectangle (fully defined, one region), its corner 8j9k0m1n stored half a
    // millimetre off its solve at (40, 20), beside a box. Each later model takes the
    // sketch's solve from the first rebuild's, its step unchanged: an edit of the box alone,
    // outside a transaction (a save, its published model rebound to the saved document) and
    // inside one; and the commit's save, rebound too. `format` then writes the sketch as
    // solved, a new step solved again from the solve's own coordinates, to the same
    // solution. Each read, current and published, gives the first read's result, byte for
    // byte, at its own revision: what the solve gave (the rectangle's states by hand,
    // docs/ports/sketcher.md's rules: 8 unknowns, 8 independent equations, every member
    // fully defined, the corner at (40, 20)).
    let block = |length: u32| {
        format!(
            r#"{{"id": "b0000001", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": {length}, "width": 20, "height": 10}}"#
        )
    };
    let f = fixture();
    let off = rectangle(&[], "").replace(
        r#"{"id": "8j9k0m1n", "x": 40, "y": 20,"#,
        r#"{"id": "8j9k0m1n", "x": 40, "y": 20.5,"#,
    );
    let path = f.open("kept.silico", &document(&[&off, &block(10)]));
    let first = f.result("k7f2qx3m");
    assert_eq!(
        first["points"][3],
        json!({"id": "8j9k0m1n", "x": 40, "y": 20, "construction": false, "state": "fully_defined"})
    );
    assert_eq!(
        (&first["state"], &first["dofs"]),
        (&json!("fully_defined"), &json!(0))
    );
    assert_eq!(states(&first, "constraints"), rectangle_constraints(&[]));
    assert_eq!(
        first["regions"],
        json!([{"outer": ["6t7v8w9x", "a0b1c2d3", "j8k9m0n1", "p2q3r4s5"], "holes": []}])
    );
    let reads_alike = |revision: u64, what: &str| {
        for model in ["current", "published"] {
            let read = f.read(model, "k7f2qx3m");
            assert_eq!(read["ok"], true, "{what}, {model}: {read}");
            assert_eq!(
                read["model"],
                json!({"revision": revision, "of": model, "fault": null}),
                "{what}, {model}"
            );
            assert_eq!(read["result"], first, "{what}, {model}");
        }
    };
    let call = |name: &str, arguments: Value| {
        let answer = f.call(name, &arguments.to_string());
        assert!(answer.ok, "{name}: {}", answer.text);
    };
    call(
        "set_field",
        json!({"session": "s1", "revision": 1, "feature": "b0000001", "field": "length", "value": 12}),
    );
    reads_alike(2, "the box edited");
    call("begin", json!({"session": "s1"}));
    call(
        "set_field",
        json!({"session": "s1", "revision": 2, "feature": "b0000001", "field": "length", "value": 14}),
    );
    reads_alike(3, "the box edited in a transaction");
    call("commit", json!({"session": "s1"}));
    reads_alike(4, "committed");
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains(r#"{"id": "8j9k0m1n", "x": 40, "y": 20.5,"#),
        "no save before format writes the sketch's solve"
    );
    call(
        "format",
        json!({"session": "s1", "features": null, "write": true}),
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains(r#"{"id": "8j9k0m1n", "x": 40, "y": 20,"#),
        "format writes the sketch's solve"
    );
    reads_alike(5, "formatted");
}

#[test]
fn two_square_holes_in_the_rectangle_are_walked_clockwise_from_their_smallest_curves_in_id_order() {
    // M3.5's rule (src/profile.rs, `walk` and the nesting): each loop from its curve with
    // the smallest ID, walked as that curve is drawn; an outer loop counter-clockwise, a
    // hole clockwise, a loop walked the wrong way turned round from that first curve; the
    // holes by their smallest curve IDs. By hand:
    // - Hole q, x 5 to 10, y 5 to 10, drawn first: from q0000001, (10, 5) up to (10, 10),
    //   then q0000004 left, q0000002 down, q0000003 right: counter-clockwise, so turned
    //   round from q0000001: q0000001, q0000003, q0000002, q0000004.
    // - Hole m, x 25 to 30, y 5 to 10, drawn after: from m0000001, (25, 5) up to (25, 10),
    //   then m0000002 right, m0000003 down, m0000004 left: clockwise already, as drawn.
    // - m0000001 < q0000001, so hole m first, though drawn after and further along x.
    let f = fixture();
    let points = r#""points": [{"id": "ha000001", "x": 5, "y": 5, "construction": false}, {"id": "ha000002", "x": 10, "y": 5, "construction": false}, {"id": "ha000003", "x": 10, "y": 10, "construction": false}, {"id": "ha000004", "x": 5, "y": 10, "construction": false}, {"id": "hb000001", "x": 25, "y": 5, "construction": false}, {"id": "hb000002", "x": 30, "y": 5, "construction": false}, {"id": "hb000003", "x": 30, "y": 10, "construction": false}, {"id": "hb000004", "x": 25, "y": 10, "construction": false}, "#;
    let lines = r#""lines": [{"id": "q0000003", "from": "ha000001", "to": "ha000002", "construction": false}, {"id": "q0000001", "from": "ha000002", "to": "ha000003", "construction": false}, {"id": "q0000004", "from": "ha000004", "to": "ha000003", "construction": false}, {"id": "q0000002", "from": "ha000001", "to": "ha000004", "construction": false}, {"id": "m0000001", "from": "hb000001", "to": "hb000004", "construction": false}, {"id": "m0000002", "from": "hb000004", "to": "hb000003", "construction": false}, {"id": "m0000003", "from": "hb000003", "to": "hb000002", "construction": false}, {"id": "m0000004", "from": "hb000002", "to": "hb000001", "construction": false}, "#;
    let text = rectangle(&[], "")
        .replace(r#""points": ["#, points)
        .replace(r#""lines": ["#, lines);
    f.open("holes.silico", &document(&[&text]));
    let holed = f.result("k7f2qx3m");
    assert_eq!(
        holed["regions"],
        json!([{
            "outer": ["6t7v8w9x", "a0b1c2d3", "j8k9m0n1", "p2q3r4s5"],
            "holes": [
                ["m0000001", "m0000002", "m0000003", "m0000004"],
                ["q0000001", "q0000003", "q0000002", "q0000004"]
            ]
        }])
    );
    assert_eq!(
        (&holed["profile_problems"], &holed["profile_problems_total"]),
        (&json!([]), &json!(0))
    );
}
