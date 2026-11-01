//! The `sketch` read of a sketch on a face whose face the rebuild couldn't resolve, on the
//! fake kernel (Fable's decision of 2026-10-31; docs/design/m5.0-app.md, section 7,
//! *`sketch` (M5.10)*, `plane_problem`): the read answers, its plane null and the rebuild's
//! one error for it in `plane_problem`, the rest of it whole, and still refuses what the
//! model holds no data for. Expected values: the 2D read of a sketch doesn't depend on its
//! plane (so a broken face sketch's 2D is, byte for byte, the same sketch's read on `xy`),
//! the errors as `check` reports the sketch, and the face plane's messages as
//! `face_plane.rs` words them, written out by hand. The real kernel's cases (a cylinder's
//! side, a repair that pads the box's top) are silico-kernel's tests/face_sketches.rs.

use super::model::WAIT;
use super::sweeps::{box_at, document, one_side, pad, rectangle};

use std::sync::Arc;

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Call, Sessions};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::Op;
use silico_core::sketch::SketchSolver;
use silico_core::{Error, ErrorCode};
use silico_protocol::{SolveStatus, Solved, Value as Unknown};

fn face(name: &str, x: &str, reversed: bool) -> String {
    format!(r#"{{"face": "{name}", "x": "{x}", "reversed": {reversed}}}"#)
}

/// A 4 × 2 rectangle from (1, 1) on the plane `plane` (its text: a face plane's object, or
/// an origin plane's word quoted), sketch `id`.
fn on(id: &str, plane: &str) -> String {
    rectangle(id, "xy", [1.0, 1.0], 4.0, 2.0)
        .replace(r#""plane": "xy""#, &format!(r#""plane": {plane}"#))
}

fn cube() -> String {
    box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0)
}

/// Sessions on `kernel` and `sketches`, with `features` opened as session s1.
struct Opened {
    _dir: tempfile::TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn opened_on(kernel: FakeKernel, sketches: FakeSketches, features: &[String]) -> Opened {
    let dir = tempfile::tempdir().unwrap();
    let sessions = Sessions::with_stand_ins(
        Arc::new(kernel),
        |cache: &std::path::Path| Cache::unsealed(cache),
        move || Ok(Box::new(sketches.clone()) as Box<dyn SketchSolver>),
    );
    let path = dir.path().join("part.silico");
    let features: Vec<&str> = features.iter().map(String::as_str).collect();
    std::fs::write(&path, document(&features)).unwrap();
    let opened = Opened {
        _dir: dir,
        sessions,
    };
    let answer = opened.call(
        "open",
        &json!({"path": path.to_str().unwrap(), "create": false}).to_string(),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    opened
}

fn opened(features: &[String]) -> Opened {
    opened_on(FakeKernel::new(2).unwrap(), FakeSketches::new(), features)
}

impl Opened {
    /// The command `name`'s answer's text, as the wire writes it.
    fn text(&self, name: &str, arguments: &str) -> String {
        match self.sessions.call(name, arguments) {
            Call::Running(calling) => {
                calling
                    .wait_timeout(WAIT)
                    .unwrap_or_else(|_| panic!("{name} didn't answer in {WAIT:?}"))
                    .text
            }
            Call::Unknown => panic!("no command {name}"),
        }
    }

    fn call(&self, name: &str, arguments: &str) -> Value {
        serde_json::from_str(&self.text(name, arguments)).unwrap()
    }

    /// The `sketch` read of `sketch` in the current model: its text.
    fn read_text(&self, sketch: &str) -> String {
        self.text(
            "sketch",
            &json!({"session": "s1", "model": "current", "sketch": sketch}).to_string(),
        )
    }

    fn read(&self, sketch: &str) -> Value {
        serde_json::from_str(&self.read_text(sketch)).unwrap()
    }

    /// `check`'s problems, as it lists them.
    fn problems(&self) -> Vec<Value> {
        let checked = self.call(
            "check",
            &json!({"session": "s1", "model": "current", "fresh": false}).to_string(),
        );
        assert_eq!(checked["ok"], true, "{checked}");
        checked["result"]["problems"].as_array().unwrap().clone()
    }

    /// Each feature's health in the timeline, by ID.
    fn health(&self) -> Vec<(String, String)> {
        let timeline = self.call(
            "timeline",
            &json!({"session": "s1", "model": "current"}).to_string(),
        );
        assert_eq!(timeline["ok"], true, "{timeline}");
        timeline["result"]["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["id"].as_str().unwrap().to_owned(),
                    f["health"].to_string(),
                )
            })
            .collect()
    }
}

/// The read's 2D, as the wire writes it: its text from `"state"` up to `"plane_problem"`
/// (state, dofs, every member with its state, regions, the profile's problems, and the
/// solve's), the fields that don't depend on the plane, byte for byte.
fn two_d(text: &str) -> &str {
    let from = text
        .find(r#""state": "#)
        .unwrap_or_else(|| panic!("{text}"));
    let to = text
        .find(r#""plane_problem": "#)
        .unwrap_or_else(|| panic!("{text}"));
    assert!(from < to, "{text}");
    &text[from..to]
}

/// The error contract's object, as `check` and the read write it.
fn error(
    code: &str,
    message: &str,
    feature: &str,
    elements: &[&str],
    parameter: Option<&str>,
    suggestion: &str,
    candidates: &[&str],
) -> Value {
    json!({
        "code": code,
        "message": message,
        "feature": feature,
        "elements": elements,
        "elements_total": elements.len(),
        "parameter": parameter,
        "cause": null,
        "suggestion": suggestion,
        "candidates": candidates,
        "advice": null,
    })
}

/// The refusal of s0000001's face, `b0000001.box.Face7`, which names nothing: its nearest by
/// number, Face6, the candidate (`face_plane.rs`'s wording).
fn face7_broken() -> Value {
    error(
        "reference_broken",
        "plane.face, \"b0000001.box.Face7\", names no face of the solids live at the sketch's place: it isn't one of the faces they're made from, or an edit before this sketch removed or remade it",
        "s0000001",
        &["b0000001.box.Face7"],
        Some("plane.face"),
        "name the face as its feature now names it: the candidates are the likeliest, none taken for it",
        &["b0000001.box.Face6"],
    )
}

const XY_FRAME: &str =
    r#"{"origin": [0, 0, 0], "x": [1, 0, 0], "y": [0, 1, 0], "normal": [0, 0, 1]}"#;

#[test]
fn a_face_sketch_whose_face_names_nothing_reads_its_2d_whole_with_its_plane_null_and_why_beside() {
    // #344's case: the box, and s1 on its Face7, which a box hasn't. With and without a pad
    // of it: the pad changes nothing of the read, and stays blocked on the sketch.
    for padded in [false, true] {
        let mut broken = vec![
            cube(),
            on("s0000001", &face("b0000001.box.Face7", "+x", false)),
        ];
        let mut flat = vec![cube(), on("s0000001", r#""xy""#)];
        if padded {
            for features in [&mut broken, &mut flat] {
                features.push(pad(
                    "p0000001",
                    Some("b0000001"),
                    "s0000001",
                    &one_side("3"),
                ));
            }
        }
        let broken = opened(&broken);
        let text = broken.read_text("s0000001");
        let read: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(read["ok"], true, "{text}");
        let result = &read["result"];
        assert_eq!(result["sketch"], "s0000001");
        assert_eq!(result["plane"], Value::Null, "{text}");
        assert_eq!(result["plane_problem"], face7_broken(), "{text}");
        assert_eq!(result["solve_problem"], Value::Null, "{text}");
        // The 2D by hand: the fake solves as stored, so each corner where the file has it,
        // nothing free; one region, its loop from the smallest ID, counter-clockwise.
        assert_eq!(result["state"], "fully_defined");
        assert_eq!(result["dofs"], 0);
        let corners: Vec<(f64, f64)> = result["points"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["x"].as_f64().unwrap(), p["y"].as_f64().unwrap()))
            .collect();
        assert_eq!(corners, [(1.0, 1.0), (5.0, 1.0), (5.0, 3.0), (1.0, 3.0)]);
        assert_eq!(
            result["regions"],
            json!([{"outer": ["na000001", "na000002", "na000003", "na000004"], "holes": []}])
        );
        assert_eq!(result["profile_problems"], json!([]));
        assert_eq!(result["profile_problems_total"], 0);
        // The 2D, byte for byte, is the same sketch's read on xy: it doesn't depend on the
        // plane. That read's plane is xy's frame, and it has no plane problem.
        let flat = opened(&flat);
        let xy = flat.read_text("s0000001");
        assert_eq!(two_d(&text), two_d(&xy), "padded {padded}");
        let xy: Value = serde_json::from_str(&xy).unwrap();
        assert_eq!(xy["ok"], true, "{xy}");
        assert_eq!(
            xy["result"]["plane"],
            serde_json::from_str::<Value>(XY_FRAME).unwrap()
        );
        assert_eq!(xy["result"]["plane_problem"], Value::Null);
        // The field comes straight after solve_problem.
        assert!(
            text.contains(r#""solve_problem": null, "plane_problem": {"code": "reference_broken""#),
            "{text}"
        );
        // Its dependents as before: check reports the sketch's one error, the same object
        // as plane_problem, and the timeline marks the sketch an error; a pad of it is
        // blocked on it, so a read of its solid is refused naming the sketch.
        let problems = broken.problems();
        assert_eq!(problems, [face7_broken()], "padded {padded}");
        let health = broken.health();
        assert_eq!(health[1], ("s0000001".to_owned(), r#""error""#.to_owned()));
        if padded {
            let elements = broken.call(
                "elements",
                &json!({"session": "s1", "model": "current", "feature": "p0000001", "after": null, "limit": 10})
                    .to_string(),
            );
            assert_eq!(elements["ok"], false, "{elements}");
            assert_eq!(
                elements["errors"][0]["message"],
                "p0000001 didn't build, since it uses s0000001, which failed",
                "{elements}"
            );
        }
    }
}

/// A chain of two sketches on faces: s1 on the box's top, padded; s2 on the pad's top,
/// padded. On the fake no face is typed a plane, so s1 is refused as not planar, and s2,
/// whose face only p1 could hold, is blocked on s1.
fn chain(s2_plane: &str) -> Vec<String> {
    vec![
        cube(),
        on("s0000001", &face("b0000001.box.Face6", "+x", false)),
        pad("p0000001", Some("b0000001"), "s0000001", &one_side("1")),
        on("s0000002", s2_plane),
        pad("p0000002", Some("p0000001"), "s0000002", &one_side("1")),
    ]
}

#[test]
fn a_face_sketch_blocked_upstream_reads_its_2d_whole_its_plane_problem_naming_the_root() {
    let s2_face = face("p0000001.pad.Face(G s0000001.sketch.Face1)", "+x", false);
    let chained = opened(&chain(&s2_face));
    // s1: its own face refused, as not planar, with no planar face to offer.
    let s1 = chained.read("s0000001");
    assert_eq!(s1["ok"], true, "{s1}");
    assert_eq!(s1["result"]["plane"], Value::Null);
    let not_planar = error(
        "reference_broken",
        "plane.face, \"b0000001.box.Face6\", names a face that isn't planar: OCCT types its surface a other, and a sketch sits only on a face typed a plane",
        "s0000001",
        &["b0000001.box.Face6"],
        Some("plane.face"),
        "name the face as its feature now names it: the candidates are the likeliest, none taken for it",
        &[],
    );
    assert_eq!(s1["result"]["plane_problem"], not_planar, "{s1}");
    // s2: blocked on s1, the root, as a read of a feature's solid words it, never as a bug.
    let text = chained.read_text("s0000002");
    let s2: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(s2["ok"], true, "{text}");
    assert_eq!(s2["result"]["plane"], Value::Null);
    assert_eq!(
        s2["result"]["plane_problem"],
        error(
            "invalid_input",
            "s0000002 didn't build, since it uses s0000001, which failed",
            "s0000002",
            &[],
            None,
            "mend s0000001 first: `check` says why it failed",
            &[],
        ),
        "{text}"
    );
    assert_eq!(s2["result"]["solve_problem"], Value::Null);
    // Its 2D whole: byte for byte s2's read on xy, in the same chain.
    let flat = opened(&chain(r#""xy""#));
    let xy = flat.read_text("s0000002");
    assert_eq!(two_d(&text), two_d(&xy));
    assert_eq!(
        serde_json::from_str::<Value>(&xy).unwrap()["result"]["plane_problem"],
        Value::Null
    );
    // check lists only the root, s1's, as before.
    let problems = chained.problems();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0], not_planar);
}

#[test]
fn a_face_sketch_whose_faces_description_crashed_the_worker_reads_with_the_fault_kept() {
    // The worker describing the box crashes: s1 fails with the fault, and s2 is blocked on
    // it. Each reads its 2D, the fault in plane_problem as every read words a kept kernel
    // fault (its suggestion saying to try again with check's fresh), never `internal`.
    let s2_face = face("p0000001.pad.Face(G s0000001.sketch.Face1)", "+x", false);
    let kernel = FakeKernel::new(2).unwrap();
    kernel.refuse(|op| {
        matches!(op, Op::DescribeAll { .. }).then(|| {
            Error::new(ErrorCode::WorkerCrashed, "the kernel worker crashed")
                .with_cause("it was killed by signal 9")
                .with_suggestion("restart the worker")
        })
    });
    let faulted = opened_on(kernel, FakeSketches::new(), &chain(&s2_face));
    let flat = opened(&chain(r#""xy""#));
    for sketch in ["s0000001", "s0000002"] {
        let text = faulted.read_text(sketch);
        let read: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(read["ok"], true, "{sketch}: {text}");
        let result = &read["result"];
        assert_eq!(result["plane"], Value::Null, "{sketch}");
        // The fault whole, as every read words a kept one: met describing s1's face (so
        // s1's, for s2 too, which is blocked on it), its cause kept, and its suggestion the
        // worker's own, then how a session's client tries it again (`fault_suggestion`).
        // With the worker's advice too: command/sketch.rs's own test (advice can't be given
        // from outside the crate).
        assert_eq!(
            result["plane_problem"],
            json!({
                "code": "worker_crashed",
                "message": "the kernel worker crashed",
                "feature": "s0000001",
                "elements": [],
                "elements_total": 0,
                "parameter": null,
                "cause": "it was killed by signal 9",
                "suggestion": "restart the worker; then check with fresh to try it again",
                "candidates": [],
                "advice": null,
            }),
            "{sketch}: {text}"
        );
        assert_eq!(result["solve_problem"], Value::Null, "{sketch}");
        // s2's own 2D, on xy in a healthy chain, is the same bytes; s1's is the box's
        // sketch's, whichever plane.
        if sketch == "s0000002" {
            assert_eq!(two_d(&text), two_d(&flat.read_text(sketch)));
        }
    }
    // Both reads give the one fault the same: s2 is blocked on the fault s1 met.
    let s1 = faulted.read("s0000001");
    let s2 = faulted.read("s0000002");
    assert_eq!(s1["result"]["plane_problem"], s2["result"]["plane_problem"]);
}

#[test]
fn a_broken_face_sketch_whose_solve_was_refused_reads_both_problems_each_in_its_own_field() {
    // No solver meets it, and its face names nothing: the solve's refusal in solve_problem,
    // the face's in plane_problem, neither in the other's; members as stored.
    let sketches = FakeSketches::new();
    sketches.answer(|system| {
        Ok(Solved {
            status: SolveStatus::Failed,
            algorithm: None,
            ..FakeSketches::as_sent(system)
        })
    });
    let features = vec![
        cube(),
        on("s0000001", &face("b0000001.box.Face7", "+x", false)),
    ];
    let both = opened_on(FakeKernel::new(2).unwrap(), sketches.clone(), &features);
    let read = both.read("s0000001");
    assert_eq!(read["ok"], true, "{read}");
    let result = &read["result"];
    assert_eq!(result["plane"], Value::Null);
    assert_eq!(result["plane_problem"], face7_broken(), "{read}");
    assert_eq!(result["solve_problem"]["code"], "solve_failed", "{read}");
    assert_eq!(result["solve_problem"]["feature"], "s0000001", "{read}");
    assert_eq!(result["state"], "no_solution");
    assert_eq!(result["regions"], json!([]));
    // check reports the face's refusal, the sketch's outcome: the read's plane_problem.
    assert!(
        both.problems().contains(&face7_broken()),
        "{:?}",
        both.problems()
    );
}

#[test]
fn a_broken_face_sketch_is_still_refused_where_the_model_holds_no_data() {
    // A solve the worker failed: no diagnosis, so the read is refused with that failure,
    // as for a sketch on xy, never answered with the plane problem alone.
    let sketches = FakeSketches::new();
    sketches.answer(|_| {
        Err(Error::new(
            ErrorCode::WorkerCrashed,
            "the sketch worker crashed",
        ))
    });
    let features = vec![
        cube(),
        on("s0000001", &face("b0000001.box.Face7", "+x", false)),
    ];
    let failed = opened_on(FakeKernel::new(2).unwrap(), sketches, &features);
    let read = failed.read("s0000001");
    assert_eq!(read["ok"], false, "{read}");
    assert_eq!(read["errors"][0]["code"], "worker_crashed", "{read}");
    assert_eq!(read["result"], Value::Null, "{read}");
    // A suppressed sketch on a broken face: never solved, so refused, naming it.
    let suppressed = on("s0000001", &face("b0000001.box.Face7", "+x", false)).replace(
        r#""name": "Sketch","#,
        r#""name": "Sketch", "suppressed": true,"#,
    );
    assert!(suppressed.contains(r#""suppressed": true"#), "{suppressed}");
    let off = opened(&[cube(), suppressed]);
    let read = off.read("s0000001");
    assert_eq!(read["ok"], false, "{read}");
    assert_eq!(
        read["errors"],
        json!([error(
            "invalid_input",
            "s0000001 is suppressed, so it isn't solved",
            "s0000001",
            &[],
            Some("sketch"),
            "unsuppress s0000001 first (suppress_feature with suppressed false), or name a sketch that isn't suppressed",
            &[],
        )]),
        "{read}"
    );
}

#[test]
fn a_face_sketch_whose_x_lies_along_its_faces_normal_reads_with_the_axes_that_work() {
    // The box's faces described as planes facing +Z through the origin: s1's face found,
    // its x, -z, along the normal. The read answers, plane null, the refusal on plane.x
    // with the four axes off the normal (AxisToken's order) as candidates; the 2D whole.
    let features = vec![
        cube(),
        on("s0000001", &face("b0000001.box.Face6", "-z", false)),
    ];
    let along = opened_on(planar_kernel(), FakeSketches::new(), &features);
    let text = along.read_text("s0000001");
    let read: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(read["ok"], true, "{text}");
    assert_eq!(read["result"]["plane"], Value::Null);
    let parallel = error(
        "reference_broken",
        "plane.x, \"-z\", is parallel to the face's normal [0.0, 0.0, 1.0]: projected onto the face it leaves 0, under the 1e-12 a sketch's x needs",
        "s0000001",
        &["-z"],
        Some("plane.x"),
        "give plane.x one of the candidates, an axis that isn't the face's normal: none is taken for it",
        &["+x", "-x", "+y", "-y"],
    );
    assert_eq!(read["result"]["plane_problem"], parallel, "{text}");
    assert_eq!(along.problems(), [parallel]);
    let flat = opened(&[cube(), on("s0000001", r#""xy""#)]);
    assert_eq!(two_d(&text), two_d(&flat.read_text("s0000001")));
}

/// The fake kernel describing every face as a plane facing +Z through the origin.
fn planar_kernel() -> FakeKernel {
    let kernel = FakeKernel::new(2).unwrap();
    kernel.answer_instead(|op| match op {
        Op::DescribeAll { shape } => {
            let mut description = FakeKernel::described(shape)?;
            for face in &mut description.faces {
                face.surface = silico_protocol::Surface::Plane {
                    normal: [0.0, 0.0, 1.0],
                    location: [0.0, 0.0, 0.0],
                };
            }
            Some(silico_protocol::Reply::DescribedAll(description))
        }
        _ => None,
    });
    kernel
}

/// A solver that solves as sent but for the first unknown (the values before it, the
/// origin's, are fixed), the first corner's x, put at 1.5 (0.5 from the file's 1, and where
/// a solve from there leaves it, so the solution settles when loaded again) and left free:
/// one degree of freedom.
fn drifting() -> FakeSketches {
    let sketches = FakeSketches::new();
    sketches.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        let first: Unknown = system.unknowns[0];
        solved.values[first.0 as usize] = 1.5;
        solved.dofs = 1;
        solved.free = vec![first];
        Ok(solved)
    });
    sketches
}

#[test]
fn a_broken_face_sketchs_2d_is_its_solve_not_its_file_a_moved_free_corner_as_on_xy() {
    // The solver moves pa000001's x, the first unknown, from 1 to 1.5 and leaves it free:
    // the read gives the solve, not the file. By hand: pa000001 at (1.5, 1), under-defined
    // with the lines through it (na000001 and na000004); the other corners and lines fully
    // defined; the sketch under-defined, 1 left; still one region. Byte for byte the same
    // sketch's read on xy, solved alike.
    let broken = opened_on(
        FakeKernel::new(2).unwrap(),
        drifting(),
        &[
            cube(),
            on("s0000001", &face("b0000001.box.Face7", "+x", false)),
        ],
    );
    let text = broken.read_text("s0000001");
    let read: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(read["ok"], true, "{text}");
    let result = &read["result"];
    assert_eq!(result["plane"], Value::Null);
    assert_eq!(result["plane_problem"], face7_broken(), "{text}");
    assert_eq!(result["state"], "under_defined", "{text}");
    assert_eq!(result["dofs"], 1);
    let points: Vec<(f64, f64, &str)> = result["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["x"].as_f64().unwrap(),
                p["y"].as_f64().unwrap(),
                p["state"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        points,
        [
            (1.5, 1.0, "under_defined"),
            (5.0, 1.0, "fully_defined"),
            (5.0, 3.0, "fully_defined"),
            (1.0, 3.0, "fully_defined"),
        ]
    );
    let lines: Vec<&str> = result["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["state"].as_str().unwrap())
        .collect();
    assert_eq!(
        lines,
        [
            "under_defined",
            "fully_defined",
            "fully_defined",
            "under_defined"
        ]
    );
    assert_eq!(
        result["regions"],
        json!([{"outer": ["na000001", "na000002", "na000003", "na000004"], "holes": []}])
    );
    let flat = opened_on(
        FakeKernel::new(2).unwrap(),
        drifting(),
        &[cube(), on("s0000001", r#""xy""#)],
    );
    assert_eq!(two_d(&text), two_d(&flat.read_text("s0000001")));
}

#[test]
fn a_face_sketch_placed_whose_solve_was_refused_reads_its_frame_and_the_solves_refusal() {
    // The face found and planar (+Z through the origin), x +x: placed, the frame by hand
    // ((0, 0, 0), +X, +Y, +Z: o = n(n·p) = 0). No solver meets it: solve_problem
    // solve_failed, naming the sketch; plane_problem null, the plane being resolved.
    let sketches = FakeSketches::new();
    sketches.answer(|system| {
        Ok(Solved {
            status: SolveStatus::Failed,
            algorithm: None,
            ..FakeSketches::as_sent(system)
        })
    });
    let placed = opened_on(
        planar_kernel(),
        sketches,
        &[
            cube(),
            on("s0000001", &face("b0000001.box.Face6", "+x", false)),
        ],
    );
    let read = placed.read("s0000001");
    assert_eq!(read["ok"], true, "{read}");
    let result = &read["result"];
    assert_eq!(
        result["plane"],
        serde_json::from_str::<Value>(XY_FRAME).unwrap(),
        "{read}"
    );
    assert_eq!(result["plane_problem"], Value::Null, "{read}");
    assert_eq!(result["solve_problem"]["code"], "solve_failed", "{read}");
    assert_eq!(result["solve_problem"]["feature"], "s0000001", "{read}");
    assert_eq!(result["state"], "no_solution");
}

#[test]
fn a_face_sketch_whose_faces_solid_couldnt_be_described_reads_the_fault_as_the_solids_read_words_it()
 {
    // The box builds, but describing it crashes the worker, with no advice of the worker's
    // own. The fault was met describing the box, which names no feature, so a read of the
    // box's solid says to try again alone; the sketch on its top crashed nothing, so its
    // plane_problem, naming it, says the same, never to report a feature that crashed the
    // worker. Written out by hand.
    let kernel = FakeKernel::new(2).unwrap();
    kernel.refuse(|op| {
        matches!(op, Op::DescribeAll { .. })
            .then(|| Error::new(ErrorCode::WorkerCrashed, "the kernel worker crashed"))
    });
    let features = vec![
        cube(),
        on("s0000001", &face("b0000001.box.Face6", "+x", false)),
    ];
    let crashed = opened_on(kernel, FakeSketches::new(), &features);
    let suggestion = "check with fresh to try it again";
    let read = crashed.read("s0000001");
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["result"]["plane"], Value::Null, "{read}");
    assert_eq!(
        read["result"]["plane_problem"],
        json!({
            "code": "worker_crashed",
            "message": "the kernel worker crashed",
            "feature": "s0000001",
            "elements": [],
            "elements_total": 0,
            "parameter": null,
            "cause": null,
            "suggestion": suggestion,
            "candidates": [],
            "advice": null,
        }),
        "{read}"
    );
    // The suggestion a read of the box's solid gives.
    let elements = crashed.call(
        "elements",
        &json!({"session": "s1", "model": "current", "feature": "b0000001", "after": null, "limit": 1})
            .to_string(),
    );
    assert_eq!(elements["ok"], false, "{elements}");
    assert_eq!(elements["errors"][0]["code"], "worker_crashed");
    assert_eq!(
        elements["errors"][0]["suggestion"], suggestion,
        "{elements}"
    );
    // check's model fault, beside its result, words it alike.
    let checked = crashed.call(
        "check",
        &json!({"session": "s1", "model": "current", "fresh": false}).to_string(),
    );
    assert_eq!(checked["ok"], true, "{checked}");
    assert_eq!(
        checked["model"]["fault"]["code"], "worker_crashed",
        "{checked}"
    );
    assert_eq!(
        checked["model"]["fault"]["suggestion"], suggestion,
        "{checked}"
    );
}

#[test]
fn a_face_sketch_whose_frame_lies_past_the_reach_is_the_documents_refusal_on_plane_never_a_bug() {
    // The box's top described as a plane of normal +X through (2e6, 0, 0): a unit normal,
    // so the face is described soundly, but the frame's origin, n(n·p) = (2e6, 0, 0), lies
    // 2e6 mm out, past √3 · 1e6: no sketch can be placed there. That's the document's
    // (the sketch sits on a face too far out), refused as invalid_input on plane, never an
    // internal error. Written out by hand.
    let kernel = FakeKernel::new(2).unwrap();
    kernel.answer_instead(|op| {
        let Op::DescribeAll { shape } = op else {
            return None;
        };
        let mut description = FakeKernel::described(shape)?;
        if let Some(top) = description.faces.get_mut(5) {
            top.surface = silico_protocol::Surface::Plane {
                normal: [1.0, 0.0, 0.0],
                location: [2e6, 0.0, 0.0],
            };
        }
        Some(silico_protocol::Reply::DescribedAll(description))
    });
    let features = vec![
        cube(),
        on("s0000001", &face("b0000001.box.Face6", "+y", false)),
    ];
    let far = opened_on(kernel, FakeSketches::new(), &features);
    let refused = error(
        "invalid_input",
        "the sketch's face's frame can't place a sketch: the face frame's origin must be within √3 · 1e6 mm (a kilometre on each axis) of the model's origin, got [2000000.0, 0.0, 0.0], 2000000 mm out",
        "s0000001",
        &[],
        Some("plane"),
        "put the sketch on a face nearer the model's origin: a face's frame is placed from the origin projected onto its plane",
        &[],
    );
    let read = far.read("s0000001");
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["result"]["plane"], Value::Null, "{read}");
    assert_eq!(read["result"]["plane_problem"], refused, "{read}");
    assert_eq!(far.problems(), [refused]);
}
