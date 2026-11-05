//! M5.9 on the real kernel: sketches on a box's and a pad's faces, placed by the rebuild in
//! rounds (docs/ports/attacher.md, *How a rebuild places it (M5.9)*, and its key tests).
//! Every expected frame is the note's table for the face (F1, F2), and every volume and
//! bound worked by hand from the boxes and rectangles: a 10 mm cube at the origin, a 4 × 2
//! rectangle from (1, 1) on its top, padded 3, adds 24 and reaches z = 13. Sketches are
//! solved as stored (no constraints), by the fake sketch solver.

use super::sweeps;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::{Answered, Call, Sessions};
use silico_core::fake::FakeSketches;
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::sketch::SketchSolver;
use silico_protocol::{SketchPlane, Surface};
use sweeps::*;
use tempfile::TempDir;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

/// Long enough for any rebuild or command here, so one that hangs fails instead.
const HANG: Duration = Duration::from_secs(120);

fn engine(cache: Option<&Path>) -> Recompute<KernelPool> {
    let pool = KernelPool::start_workers(Path::new(WORKER), 2).unwrap();
    let engine = match cache {
        Some(dir) => Recompute::with_cache(
            pool,
            Cache::with_secret(dir.join("cache"), &dir.join("secret")),
        ),
        None => Recompute::new(pool),
    };
    engine.with_sketches(FakeSketches::new())
}

fn publish(engine: &Recompute<KernelPool>, features: &[String]) -> Arc<Model> {
    let features: Vec<&str> = features.iter().map(String::as_str).collect();
    match engine.start(checked_document(&features)).wait_timeout(HANG) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the rebuild didn't finish"),
    }
}

/// A face plane's object.
fn face(name: &str, x: &str, reversed: bool) -> String {
    format!(r#"{{"face": "{name}", "x": "{x}", "reversed": {reversed}}}"#)
}

/// A `w` × `h` rectangle from `at`, sketch `id`, on the face plane `plane` (its text).
fn on_face(id: &str, plane: &str, at: [f64; 2], w: f64, h: f64) -> String {
    rectangle(id, "xy", at, w, h).replace(r#""plane": "xy""#, &format!(r#""plane": {plane}"#))
}

/// The box `b0000001` from the origin, 10 × 10 × `height`; a 4 × 2 rectangle from (1, 1) on
/// its top face, `+x`; that padded 3 onto the box.
fn padded(height: f64) -> Vec<String> {
    vec![
        box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, height),
        on_face(
            "s0000001",
            &face("b0000001.box.Face6", "+x", false),
            [1.0, 1.0],
            4.0,
            2.0,
        ),
        pad("p0000001", Some("b0000001"), "s0000001", &one_side("3")),
    ]
}

/// A built feature's solid's summary.
fn summary(model: &Model, feature: &str) -> silico_protocol::ShapeSummary {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { built, .. }) => built.summary().unwrap().clone(),
        other => panic!("{feature}: {other:?}"),
    }
}

#[track_caller]
fn assert_near(got: f64, want: f64, tolerance: f64, what: &str) {
    assert!(
        (got - want).abs() <= tolerance,
        "{what}: {got} against {want}"
    );
}

/// That `feature`'s solid measures `volume` and spans `low` to `high` (each within OCCT's
/// own tolerance of 1e-7, the bounds being of its geometry).
#[track_caller]
fn assert_solid(model: &Model, feature: &str, volume: f64, low: [f64; 3], high: [f64; 3]) {
    let s = summary(model, feature);
    assert_near(
        s.volume,
        volume,
        1e-9 * volume,
        &format!("{feature}'s volume"),
    );
    let [l, h] = s.bounds.unwrap();
    for k in 0..3 {
        assert_near(l[k], low[k], 1e-7, &format!("{feature}'s low {k}"));
        assert_near(h[k], high[k], 1e-7, &format!("{feature}'s high {k}"));
    }
    assert!(s.valid, "{feature} isn't valid");
}

/// The sketch `sketch`'s resolved frame, as origin, x, y, and normal.
fn frame(model: &Model, sketch: &str) -> [[f64; 3]; 4] {
    let f = *model
        .frame(&id(sketch))
        .unwrap_or_else(|| panic!("{sketch}: {:?}", model.outcome(&id(sketch))));
    let plane = SketchPlane::Face(f);
    [
        plane.origin(),
        plane.x_axis(),
        plane.y_axis(),
        plane.normal(),
    ]
}

/// The planar face of `feature`'s solid on the plane z = `z` facing +Z: its area.
fn top_area(model: &Model, feature: &str, z: f64) -> f64 {
    let description = model.description(&id(feature)).unwrap();
    let tops: Vec<f64> = description
        .faces
        .iter()
        .filter_map(|f| match f.surface {
            Surface::Plane { normal, .. }
                if normal == [0.0, 0.0, 1.0] && (f.centroid[2] - z).abs() <= 1e-9 =>
            {
                Some(f.area)
            }
            _ => None,
        })
        .collect();
    assert_eq!(tops.len(), 1, "{feature} at z = {z}: {tops:?}");
    tops[0]
}

#[test]
fn a_pad_on_a_boxs_top_stands_on_it_and_moves_with_it_when_the_box_grows_with_the_cache_warm() {
    // The note's table, Face6 of a box at the origin: n = +Z, `+x`, x = +X, y = +Y, o =
    // (0, 0, 10). The rectangle lands at (1..5, 1..3, 10); padded 3 onto the box, 1000 + 4
    // × 2 × 3 = 1024, its top at z = 13, an 8 mm² face there.
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(Some(dir.path()));
    let model = publish(&engine, &padded(10.0));
    assert_eq!(
        frame(&model, "s0000001"),
        [
            [0.0, 0.0, 10.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0]
        ]
    );
    assert_solid(&model, "p0000001", 1024.0, [0.0; 3], [10.0, 10.0, 13.0]);
    assert_near(
        top_area(&model, "p0000001", 13.0),
        8.0,
        1e-9,
        "the pad's top",
    );
    // The box 20 high, rebuilt on the same engine, its steps and the cache warm: the face's
    // frame moves to z = 20 and the pad with it, 2000 + 24, its top at z = 23. A faces step
    // keyed by the face's name would have found the pad of the first build.
    let model = publish(&engine, &padded(20.0));
    assert_eq!(frame(&model, "s0000001")[0], [0.0, 0.0, 20.0]);
    assert_solid(&model, "p0000001", 2024.0, [0.0; 3], [10.0, 10.0, 23.0]);
    assert_near(
        top_area(&model, "p0000001", 23.0),
        8.0,
        1e-9,
        "the pad's top",
    );
    // And back: the first build, from what's kept.
    let model = publish(&engine, &padded(10.0));
    assert_solid(&model, "p0000001", 1024.0, [0.0; 3], [10.0, 10.0, 13.0]);
    // A fresh engine on the same cache directory, the 20 mm box: read from disk, the same.
    let again = engine_on(dir.path());
    let model = publish(&again, &padded(20.0));
    assert_solid(&model, "p0000001", 2024.0, [0.0; 3], [10.0, 10.0, 23.0]);
}

fn engine_on(dir: &Path) -> Recompute<KernelPool> {
    engine(Some(dir))
}

/// [`padded`], then a 1 × 1 square from (2, 1.5) on the pad's top (its name, design S4:
/// `p0000001.pad.Face(G s0000001.sketch.Face1)`, kept by the pad's fuse with the box),
/// padded 2 onto the first pad: a sketch on a face of a feature that's itself of a sketch on
/// a face, so two rounds.
fn stacked() -> Vec<String> {
    let mut features = padded(10.0);
    features.push(on_face(
        "s0000002",
        &face("p0000001.pad.Face(G s0000001.sketch.Face1)", "+x", false),
        [2.0, 1.5],
        1.0,
        1.0,
    ));
    features.push(pad(
        "p0000002",
        Some("p0000001"),
        "s0000002",
        &one_side("2"),
    ));
    features
}

/// What a model of [`stacked`] holds that a rebuild must give the same: each sketch's
/// frame's bits, and each pad's solid's bytes and summary.
fn stacked_bits(model: &Model) -> (Vec<u64>, Vec<Vec<u8>>, Vec<String>) {
    let mut bits = Vec::new();
    for sketch in ["s0000001", "s0000002"] {
        bits.extend(frame(model, sketch).iter().flatten().map(|c| c.to_bits()));
    }
    let mut shapes = Vec::new();
    let mut summaries = Vec::new();
    for feature in ["p0000001", "p0000002"] {
        match model.outcome(&id(feature)) {
            Some(FeatureOutcome::Built { built, .. }) => {
                shapes.push(built.shape.brep().as_bytes().to_vec());
                summaries.push(format!("{:?}", built.summary()));
            }
            other => panic!("{feature}: {other:?}"),
        }
    }
    (bits, shapes, summaries)
}

#[test]
fn a_sketch_on_a_pad_of_a_sketch_on_a_face_takes_two_rounds_and_builds_alike_cold_warm_and_from_disk()
 {
    // s2 on p1's top: o = (0, 0, 13), x = +X, y = +Y. Its square lands at (2..3, 1.5..2.5,
    // 13), inside p1's top (1..5, 1..3); padded 2, 1024 + 2 = 1026, reaching z = 15.
    let dir = tempfile::tempdir().unwrap();
    let cold = engine(Some(dir.path()));
    let first = publish(&cold, &stacked());
    assert_eq!(
        frame(&first, "s0000002"),
        [
            [0.0, 0.0, 13.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0]
        ]
    );
    assert_solid(&first, "p0000002", 1026.0, [0.0; 3], [10.0, 10.0, 15.0]);
    assert_near(
        top_area(&first, "p0000002", 15.0),
        1.0,
        1e-9,
        "the second pad's top",
    );
    let want = stacked_bits(&first);
    // Warm: the same engine, everything kept in memory.
    assert_eq!(stacked_bits(&publish(&cold, &stacked())), want);
    // From the disk cache: a new engine on the same directory.
    assert_eq!(
        stacked_bits(&publish(&engine(Some(dir.path())), &stacked())),
        want
    );
    // Cold again, on an engine of its own and no cache, and fresh on the first: the same
    // bits each time (ARCHITECTURE.md, invariant 5).
    for _ in 0..2 {
        assert_eq!(stacked_bits(&publish(&engine(None), &stacked())), want);
    }
    let fresh = match cold
        .start_fresh(checked_document(
            &stacked().iter().map(String::as_str).collect::<Vec<_>>(),
        ))
        .wait_timeout(HANG)
    {
        Ok(Ok(RunOutcome::Published(model))) => model,
        other => panic!("{:?}", other.map(|r| r.map(|_| ())).ok()),
    };
    assert_eq!(stacked_bits(&fresh), want);
}

#[test]
fn a_reversed_sketch_on_a_top_face_faces_down_its_x_back_and_pads_into_where_it_faces() {
    // Face6 reversed: n = −Z, x = −X, y = +Y, o = (0, 0, 10). The rectangle from (−5, 1),
    // 4 × 2, lands at x = 1..5 (x = −u), y = 1..3; padded 3 alone, along −Z: (1..5, 1..3,
    // 7..10), 24 mm³.
    let features = vec![
        box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0),
        on_face(
            "s0000001",
            &face("b0000001.box.Face6", "+x", true),
            [-5.0, 1.0],
            4.0,
            2.0,
        ),
        pad("p0000001", None, "s0000001", &one_side("3")),
    ];
    let model = publish(&engine(None), &features);
    assert_eq!(
        frame(&model, "s0000001"),
        [
            [0.0, 0.0, 10.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, -1.0]
        ]
    );
    assert_solid(&model, "p0000001", 24.0, [1.0, 1.0, 7.0], [5.0, 3.0, 10.0]);
}

#[test]
fn every_face_of_a_box_off_the_origin_takes_the_notes_frame() {
    // The box from p = (2, 3, 5), 10 × 20 × 30, each face by its number with its default
    // token: the note's table, n, x, y, and o.
    let cases = [
        (
            "Face1",
            "-y",
            [-1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [2.0, 0.0, 0.0],
        ),
        (
            "Face2",
            "+y",
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [12.0, 0.0, 0.0],
        ),
        (
            "Face3",
            "+x",
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 3.0, 0.0],
        ),
        (
            "Face4",
            "-x",
            [0.0, 1.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 23.0, 0.0],
        ),
        (
            "Face5",
            "+x",
            [0.0, 0.0, -1.0],
            [1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 5.0],
        ),
        (
            "Face6",
            "+x",
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 35.0],
        ),
    ];
    let mut features = vec![box_at("b0000001", [2.0, 3.0, 5.0], 10.0, 20.0, 30.0)];
    for (i, (number, token, ..)) in cases.iter().enumerate() {
        features.push(on_face(
            &format!("s000000{}", i + 1),
            &face(&format!("b0000001.box.{number}"), token, false),
            [0.0, 0.0],
            1.0,
            1.0,
        ));
    }
    let model = publish(&engine(None), &features);
    for (i, (number, _, n, x, y, o)) in cases.iter().enumerate() {
        let got = frame(&model, &format!("s000000{}", i + 1));
        assert_eq!(got, [*o, *x, *y, *n], "{number}");
    }
}

#[test]
fn a_top_face_cut_away_breaks_the_sketch_with_candidates_and_blocks_its_pad() {
    // The box's top cut away by a box from z = 8 up, over the whole of it: b0000001.box.Face6
    // is in no live solid (the cut's), so the sketch's face is reference_broken, never
    // re-pointed to the cut's new top. The candidates are the cut's faces nearest it by
    // number that the box made: Face5, the bottom (no face is made from Face6).
    let features = vec![
        box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0),
        box_at("t0000001", [-1.0, -1.0, 8.0], 12.0, 12.0, 12.0),
        r#"{"id": "c0000001", "type": "cut", "v": 1, "name": "Cut", "base": "b0000001", "tool": "t0000001"}"#
            .to_owned(),
        on_face(
            "s0000001",
            &face("b0000001.box.Face6", "+x", false),
            [1.0, 1.0],
            4.0,
            2.0,
        ),
        pad("p0000001", Some("c0000001"), "s0000001", &one_side("3")),
    ];
    let model = publish(&engine(None), &features);
    assert_solid(&model, "c0000001", 800.0, [0.0; 3], [10.0, 10.0, 8.0]);
    let Some(FeatureOutcome::Broken { errors, total }) = model.outcome(&id("s0000001")) else {
        panic!("{:?}", model.outcome(&id("s0000001")));
    };
    assert_eq!(*total, 1);
    let e = &errors[0];
    assert_eq!(e.code(), ErrorCode::ReferenceBroken);
    assert_eq!(e.feature(), Some("s0000001"));
    assert_eq!(e.parameter(), Some("plane.face"));
    assert_eq!(e.candidates(), ["b0000001.box.Face5"]);
    assert_eq!(
        model.outcome(&id("p0000001")),
        Some(&FeatureOutcome::Blocked { on: id("s0000001") })
    );
}

#[test]
fn a_cylinders_side_is_refused_as_not_planar_with_its_planar_faces_as_candidates() {
    // OCCT's cylinder: Face1 its side, Face2 its top, Face3 its bottom.
    let features = vec![
        r#"{"id": "c0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [0, 0, 0], "radius": 5, "height": 10}"#
            .to_owned(),
        on_face(
            "s0000001",
            &face("c0000001.cylinder.Face1", "+x", false),
            [1.0, 1.0],
            1.0,
            1.0,
        ),
        pad("p0000001", Some("c0000001"), "s0000001", &one_side("1")),
    ];
    let model = publish(&engine(None), &features);
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("s0000001")) else {
        panic!("{:?}", model.outcome(&id("s0000001")));
    };
    let e = &errors[0];
    assert_eq!(e.code(), ErrorCode::ReferenceBroken);
    assert_eq!(e.parameter(), Some("plane.face"));
    assert_eq!(
        e.message(),
        "plane.face, \"c0000001.cylinder.Face1\", names a face that isn't planar: OCCT types its surface a cylinder, and a sketch sits only on a face typed a plane"
    );
    assert_eq!(
        e.candidates(),
        ["c0000001.cylinder.Face2", "c0000001.cylinder.Face3"]
    );
    assert_eq!(
        model.outcome(&id("p0000001")),
        Some(&FeatureOutcome::Blocked { on: id("s0000001") })
    );
}

#[test]
fn an_axis_along_the_top_faces_normal_is_refused_on_plane_x_with_the_axes_that_work() {
    let mut features = padded(10.0);
    features[1] = features[1].replace(r#""x": "+x""#, r#""x": "-z""#);
    let model = publish(&engine(None), &features);
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("s0000001")) else {
        panic!("{:?}", model.outcome(&id("s0000001")));
    };
    let e = &errors[0];
    assert_eq!(e.code(), ErrorCode::ReferenceBroken);
    assert_eq!(e.parameter(), Some("plane.x"));
    assert_eq!(e.elements(), ["-z"]);
    assert_eq!(e.candidates(), ["+x", "-x", "+y", "-y"]);
    assert_eq!(
        model.outcome(&id("p0000001")),
        Some(&FeatureOutcome::Blocked { on: id("s0000001") })
    );
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

    /// `features` written as a document in `file` and opened: the session's ID.
    fn open(&self, file: &str, features: &[String]) -> String {
        let features: Vec<&str> = features.iter().map(String::as_str).collect();
        std::fs::write(self.path(file), document(&features)).unwrap();
        let opened = self.call(
            "open",
            &json!({"path": self.path(file).to_str().unwrap(), "create": false}).to_string(),
        );
        assert_eq!(opened["ok"], true, "{opened}");
        opened["session"]["id"].as_str().unwrap().to_owned()
    }

    fn call(&self, name: &str, arguments: &str) -> Value {
        let answer: Answered = match self.sessions.call(name, arguments) {
            Call::Running(calling) => calling
                .wait_timeout(HANG)
                .unwrap_or_else(|_| panic!("{name} didn't answer in {HANG:?}")),
            Call::Unknown => panic!("no command {name}"),
        };
        serde_json::from_str(&answer.text).unwrap()
    }

    fn read_sketch(&self, session: &str, sketch: &str) -> Value {
        self.call(
            "sketch",
            &json!({"session": session, "model": "current", "sketch": sketch}).to_string(),
        )
    }
}

#[test]
fn the_sketch_read_gives_a_face_sketchs_frame_as_resolved_and_the_timeline_its_faces_feature() {
    let f = fixture();
    let session = f.open("top.silico", &padded(10.0));
    let read = f.read_sketch(&session, "s0000001");
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(
        read["result"]["plane"],
        json!({"origin": [0, 0, 10], "x": [1, 0, 0], "y": [0, 1, 0], "normal": [0, 0, 1]})
    );
    // Resolved, so no plane problem; and likewise a sketch on an origin plane, its frame
    // xy's (D6).
    assert_eq!(read["result"]["plane_problem"], Value::Null, "{read}");
    let mut flat = padded(10.0);
    flat[1] = rectangle("s0000001", "xy", [1.0, 1.0], 4.0, 2.0);
    let on_xy = f.open("xy.silico", &flat);
    let xy = f.read_sketch(&on_xy, "s0000001");
    assert_eq!(xy["ok"], true, "{xy}");
    assert_eq!(
        xy["result"]["plane"],
        json!({"origin": [0, 0, 0], "x": [1, 0, 0], "y": [0, 1, 0], "normal": [0, 0, 1]})
    );
    assert_eq!(xy["result"]["plane_problem"], Value::Null, "{xy}");
    let timeline = f.call(
        "timeline",
        &json!({"session": session, "model": "current"}).to_string(),
    );
    assert_eq!(timeline["ok"], true, "{timeline}");
    let inputs: Vec<&Value> = timeline["result"]["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| &f["inputs"])
        .collect();
    assert_eq!(
        inputs,
        [
            &json!([]),
            &json!(["b0000001"]),
            &json!(["b0000001", "s0000001"])
        ]
    );
}

/// The read's result but its plane and its plane problem: the 2D, which a plane doesn't change.
fn two_d(read: &Value) -> Value {
    let mut result = read["result"].clone();
    let object = result.as_object_mut().unwrap();
    assert!(object.remove("plane").is_some(), "{read}");
    assert!(object.remove("plane_problem").is_some(), "{read}");
    result
}

#[test]
fn the_sketch_read_of_a_face_sketch_whose_face_broke_answers_its_2d_and_mends_by_replace_text() {
    // Face7 names nothing of a box: the read answers, plane null, the broken reference in
    // plane_problem with Face6, its nearest by number, as the candidate, and the 2D whole.
    let f = fixture();
    let mut features = padded(10.0);
    features[1] = features[1].replace("b0000001.box.Face6", "b0000001.box.Face7");
    let session = f.open("broken.silico", &features);
    let read = f.read_sketch(&session, "s0000001");
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["result"]["plane"], Value::Null, "{read}");
    let error = &read["result"]["plane_problem"];
    assert_eq!(error["code"], "reference_broken", "{read}");
    assert_eq!(error["feature"], "s0000001");
    assert_eq!(error["parameter"], "plane.face");
    assert_eq!(error["candidates"], json!(["b0000001.box.Face6"]));
    assert_eq!(read["result"]["solve_problem"], Value::Null, "{read}");
    // Its pad is blocked on it, as before.
    let measured = f.call(
        "mass_properties",
        &json!({"session": session, "model": "current", "feature": "p0000001"}).to_string(),
    );
    assert_eq!(measured["ok"], false, "{measured}");
    assert_eq!(
        measured["errors"][0]["message"],
        "p0000001 didn't build, since it uses s0000001, which failed"
    );
    // The repair: the plane's face set to the candidate, by replace_text (and by set_field,
    // below). The read's plane is the box's top's frame (F1: origin the
    // face's centroid's projection, (0, 0, 10) for +x on a top through it), no plane
    // problem, the 2D unchanged; and the pad builds, 1000 + 4 × 2 × 3 = 1024, to z = 13.
    let mended = document(&features.iter().map(String::as_str).collect::<Vec<_>>())
        .replace(
            r#""resolution": "1nm"}, "#,
            r#""resolution": "1nm"}, "naming": 5, "#,
        )
        .replace("b0000001.box.Face7", "b0000001.box.Face6");
    assert!(mended.contains(r#""naming": 5"#), "{mended}");
    let replaced = f.call(
        "replace_text",
        &json!({"session": session, "revision": 1, "text": mended}).to_string(),
    );
    assert_eq!(replaced["ok"], true, "{replaced}");
    let again = f.read_sketch(&session, "s0000001");
    assert_eq!(again["ok"], true, "{again}");
    assert_eq!(
        again["result"]["plane"],
        json!({"origin": [0, 0, 10], "x": [1, 0, 0], "y": [0, 1, 0], "normal": [0, 0, 1]})
    );
    assert_eq!(again["result"]["plane_problem"], Value::Null, "{again}");
    assert_eq!(two_d(&again), two_d(&read));
    let measured = f.call(
        "mass_properties",
        &json!({"session": session, "model": "current", "feature": "p0000001"}).to_string(),
    );
    assert_eq!(measured["ok"], true, "{measured}");
    let volume = measured["result"]["bodies"][0]["volume"].as_f64().unwrap();
    assert_near(volume, 1024.0, 1e-9 * 1024.0, "the mended pad's volume");
}

#[test]
fn the_sketch_read_of_a_sketch_on_a_cylinders_side_answers_with_the_planar_faces_as_candidates() {
    // OCCT's cylinder: Face1 its side, Face2 its top, Face3 its bottom. The side isn't
    // planar: the read answers, plane null, the refusal with the planar faces.
    let f = fixture();
    let features = vec![
        r#"{"id": "c0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [0, 0, 0], "radius": 5, "height": 10}"#
            .to_owned(),
        on_face(
            "s0000001",
            &face("c0000001.cylinder.Face1", "+x", false),
            [1.0, 1.0],
            1.0,
            1.0,
        ),
        pad("p0000001", Some("c0000001"), "s0000001", &one_side("1")),
    ];
    let session = f.open("pin.silico", &features);
    let read = f.read_sketch(&session, "s0000001");
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["result"]["plane"], Value::Null, "{read}");
    let error = &read["result"]["plane_problem"];
    assert_eq!(error["code"], "reference_broken", "{read}");
    assert_eq!(error["feature"], "s0000001");
    assert_eq!(error["parameter"], "plane.face");
    assert_eq!(
        error["message"],
        "plane.face, \"c0000001.cylinder.Face1\", names a face that isn't planar: OCCT types its surface a cylinder, and a sketch sits only on a face typed a plane"
    );
    assert_eq!(
        error["candidates"],
        json!(["c0000001.cylinder.Face2", "c0000001.cylinder.Face3"])
    );
    assert_eq!(read["result"]["solve_problem"], Value::Null, "{read}");
    // The 2D whole: a 1 × 1 square from (1, 1), as stored; one region of its four sides.
    assert_eq!(read["result"]["state"], "fully_defined", "{read}");
    assert_eq!(
        read["result"]["regions"],
        json!([{"outer": ["na000001", "na000002", "na000003", "na000004"], "holes": []}])
    );
    // The same object check reports for the sketch.
    let checked = f.call(
        "check",
        &json!({"session": session, "model": "current", "fresh": false}).to_string(),
    );
    assert_eq!(checked["result"]["problems"], json!([error]), "{checked}");
    // The whole 2D is the same sketch's read on xy, padded onto the cylinder alike.
    let mut flat = features.clone();
    flat[1] = rectangle("s0000001", "xy", [1.0, 1.0], 1.0, 1.0);
    let on_xy = f.open("pin-xy.silico", &flat);
    let xy = f.read_sketch(&on_xy, "s0000001");
    assert_eq!(xy["ok"], true, "{xy}");
    assert_eq!(xy["result"]["plane_problem"], Value::Null, "{xy}");
    assert_eq!(two_d(&read), two_d(&xy));
}

// M5.9's part 3: `add_feature` of a sketch on a face that leaves out its plane's `x`, which
// the core writes as the face's `default_x` (M4.1 section 5), and `set_field` of `plane`.

impl Fixture {
    fn revision(&self, session: &str) -> u64 {
        let status = self.call("status", &json!({"session": session}).to_string());
        status["session"]["revision"].as_u64().unwrap()
    }

    fn text(&self, session: &str) -> String {
        let text = self.call(
            "text",
            &json!({"session": session, "feature": null, "revision": null}).to_string(),
        );
        text["result"]["text"].as_str().unwrap().to_owned()
    }

    /// `add_feature` of `feature`, before `before` or at the end, against the session's
    /// revision.
    fn add(&self, session: &str, before: Option<&str>, feature: &Value) -> Value {
        let revision = self.revision(session);
        self.call(
            "add_feature",
            &json!({"session": session, "revision": revision, "before": before, "feature": feature})
                .to_string(),
        )
    }

    /// `set_field` of `feature`'s `field` to `value`, against the session's revision.
    fn set(&self, session: &str, feature: &str, field: &str, value: Value) -> Value {
        let revision = self.revision(session);
        self.call(
            "set_field",
            &json!({"session": session, "revision": revision, "feature": feature, "field": field, "value": value})
                .to_string(),
        )
    }

    /// The face `name`'s `default_x`, as `elements` reports it, of `feature`'s solid.
    fn default_x(&self, session: &str, feature: &str, name: &str) -> Value {
        let listed = self.call(
            "elements",
            &json!({"session": session, "model": "current", "feature": feature, "after": null, "limit": 1000})
                .to_string(),
        );
        assert_eq!(listed["ok"], true, "{listed}");
        listed["result"]["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == name)
            .unwrap_or_else(|| panic!("{feature} has no {name}: {listed}"))["default_x"]
            .clone()
    }
}

/// A 1 × 1 square from (1, 1), sketch `id`, on the face `face`, its `x` left out.
fn without_x(id: &str, face: &str) -> Value {
    let mut sketch: Value =
        serde_json::from_str(&rectangle(id, "xy", [1.0, 1.0], 1.0, 1.0)).unwrap();
    sketch["plane"] = json!({"face": face, "reversed": false});
    sketch
}

#[test]
fn a_sketch_added_on_each_face_of_a_cube_without_its_x_takes_the_notes_token_and_elements_default_x()
 {
    // The note's table for a box's six faces (docs/ports/attacher.md, *Worked frames*), for
    // a 10 mm cube at the origin as OCCT numbers its faces.
    let table = [
        ("Face1", "-y"),
        ("Face2", "+y"),
        ("Face3", "+x"),
        ("Face4", "-x"),
        ("Face5", "+x"),
        ("Face6", "+x"),
    ];
    let f = fixture();
    let session = f.open(
        "cube.silico",
        &[box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0)],
    );
    for (i, (number, token)) in table.iter().enumerate() {
        let face = format!("b0000001.box.{number}");
        let sketch = format!("s000000{}", i + 1);
        let revision = f.revision(&session);
        let added = f.add(&session, None, &without_x(&sketch, &face));
        assert_eq!(added["ok"], true, "{number}: {added}");
        assert_eq!(
            added["result"]["feature"]["plane"],
            json!({"face": face, "x": token, "reversed": false}),
            "{number}"
        );
        assert_eq!(f.revision(&session), revision + 1);
        // The token is the face's default_x as `elements` reports it.
        assert_eq!(f.default_x(&session, "b0000001", &face), json!(token));
        // And the rebuild places the sketch on the note's frame for it: its x is the token's
        // axis (each face's normal is an axis, so the projection is the axis itself).
        let read = f.read_sketch(&session, &sketch);
        assert_eq!(read["result"]["plane_problem"], Value::Null, "{read}");
        let axis = match *token {
            "+x" => json!([1, 0, 0]),
            "-x" => json!([-1, 0, 0]),
            "+y" => json!([0, 1, 0]),
            "-y" => json!([0, -1, 0]),
            other => panic!("{other}"),
        };
        assert_eq!(read["result"]["plane"]["x"], axis, "{number}: {read}");
    }
}

#[test]
fn a_cylinders_side_added_without_x_is_refused_as_not_planar_with_the_planar_faces_and_not_made() {
    let f = fixture();
    let session = f.open(
        "pin.silico",
        &[r#"{"id": "c0000001", "type": "cylinder", "v": 1, "name": "Pin", "position": [0, 0, 0], "radius": 5, "height": 10}"#
            .to_owned()],
    );
    let (revision, text) = (f.revision(&session), f.text(&session));
    let answer = f.add(
        &session,
        None,
        &without_x("s0000001", "c0000001.cylinder.Face1"),
    );
    assert_eq!(answer["ok"], false, "{answer}");
    assert_eq!(
        answer["errors"][0],
        json!({
            "code": "reference_broken",
            "message": "plane.face, \"c0000001.cylinder.Face1\", names a face that isn't planar: OCCT types its surface a cylinder, and a sketch sits only on a face typed a plane",
            "feature": "s0000001",
            "elements": ["c0000001.cylinder.Face1"],
            "elements_total": 1,
            "parameter": "plane.face",
            "cause": null,
            "suggestion": "name the face as its feature now names it: the candidates are the likeliest, none taken for it",
            "candidates": ["c0000001.cylinder.Face2", "c0000001.cylinder.Face3"],
            "advice": null,
        })
    );
    assert_eq!((f.revision(&session), f.text(&session)), (revision, text));
    // Its top, Face2, faces +Z: +x.
    let added = f.add(
        &session,
        None,
        &without_x("s0000001", "c0000001.cylinder.Face2"),
    );
    assert_eq!(added["result"]["feature"]["plane"]["x"], "+x", "{added}");
}

#[test]
fn a_sketch_added_before_a_fuse_finds_its_face_in_the_boxs_own_solid_and_after_it_through_the_fuse()
{
    // b1, a 10 mm cube; b2, 10 × 10 × 5 on its top; fused. b1's top, Face6, lies inside the
    // fuse: live in b1's solid before the fuse, and gone from the fuse after it. Its side,
    // Face2 (+X), is the fuse's too.
    let f = fixture();
    let session = f.open(
        "fused.silico",
        &[
            box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0),
            box_at("b0000002", [0.0, 0.0, 10.0], 10.0, 10.0, 5.0),
            r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "Both", "base": "b0000001", "tool": "b0000002"}"#
                .to_owned(),
        ],
    );
    assert_eq!(
        f.call("begin", &json!({"session": session}).to_string())["ok"],
        true
    );
    let before = f.add(
        &session,
        Some("f0000001"),
        &without_x("s0000001", "b0000001.box.Face6"),
    );
    assert_eq!(before["ok"], true, "{before}");
    assert_eq!(before["result"]["index"], 2);
    assert_eq!(before["result"]["feature"]["plane"]["x"], "+x");
    assert_eq!(
        f.call("rollback", &json!({"session": session}).to_string())["ok"],
        true
    );
    let (revision, text) = (f.revision(&session), f.text(&session));
    let after = f.add(&session, None, &without_x("s0000001", "b0000001.box.Face6"));
    assert_eq!(after["ok"], false, "{after}");
    assert_eq!(after["errors"][0]["code"], "reference_broken", "{after}");
    assert_eq!(after["errors"][0]["parameter"], "plane.face");
    assert_eq!((f.revision(&session), f.text(&session)), (revision, text));
    let side = f.add(&session, None, &without_x("s0000001", "b0000001.box.Face2"));
    assert_eq!(side["ok"], true, "{side}");
    assert_eq!(side["result"]["feature"]["plane"]["x"], "+y");
    assert_eq!(side["result"]["index"], 3);
}

#[test]
fn a_sketch_added_on_a_side_an_edit_just_turned_takes_the_turned_faces_default() {
    // A pad of the quadrilateral (0, 0), (10, 0), (5, 10), (0, 10), counter-clockwise: its
    // side from (10, 0) to (5, 10), na000002, runs along (−5, 10), so its outward normal is
    // (10, 5)/√125, nearest +X: `+y`. With pa000003 at (25, 10) it runs along (15, 10), its
    // normal (10, −15)/√325, nearest −Y: `+x`.
    let side = "p0000001.pad.Face(G s0000001.sketch.Edge.na000002)";
    let f = fixture();
    let session = f.open(
        "slant.silico",
        &[
            polygon(
                "s0000001",
                "xy",
                &[[0.0, 0.0], [10.0, 0.0], [5.0, 10.0], [0.0, 10.0]],
            ),
            pad("p0000001", None, "s0000001", &one_side("5")),
        ],
    );
    let begin = || {
        assert_eq!(
            f.call("begin", &json!({"session": session}).to_string())["ok"],
            true
        );
    };
    // As opened: `+y`, its model published.
    begin();
    let added = f.add(&session, None, &without_x("s0000002", side));
    assert_eq!(added["result"]["feature"]["plane"]["x"], "+y", "{added}");
    assert_eq!(
        f.call("rollback", &json!({"session": session}).to_string())["ok"],
        true
    );
    // Inside a transaction, an edit turns the side and starts no rebuild: the published
    // model is still the side's before. The add waits for the current document's.
    begin();
    let moved = f.set(&session, "s0000001", "points.pa000003.x", json!(25));
    assert_eq!(moved["ok"], true, "{moved}");
    let added = f.add(&session, None, &without_x("s0000002", side));
    assert_eq!(added["ok"], true, "{added}");
    assert_eq!(added["result"]["feature"]["plane"]["x"], "+x", "{added}");
    assert_eq!(f.default_x(&session, "p0000001", side), json!("+x"));
    assert_eq!(
        f.call("rollback", &json!({"session": session}).to_string())["ok"],
        true
    );
    // Outside one, the edit is committed, and the add right after it takes the turned side's.
    let moved = f.set(&session, "s0000001", "points.pa000003.x", json!(25));
    assert_eq!(moved["ok"], true, "{moved}");
    let added = f.add(&session, None, &without_x("s0000002", side));
    assert_eq!(added["result"]["feature"]["plane"]["x"], "+x", "{added}");
    assert!(
        f.text(&session).contains(&format!(
            r#""plane": {{"face": "{side}", "x": "+x", "reversed": false}}"#
        )),
        "{}",
        f.text(&session)
    );
}

#[test]
fn a_face_sketch_whose_face_broke_is_mended_by_set_field_of_its_plane_and_its_pad_builds() {
    // #347's case: Face7 names nothing of a box. set_field of plane to the candidate, its x
    // given: the read's plane is the box's top's frame, and the pad builds, 1000 + 4 × 2 ×
    // 3 = 1024, to z = 13.
    let f = fixture();
    let mut features = padded(10.0);
    features[1] = features[1].replace("b0000001.box.Face6", "b0000001.box.Face7");
    let session = f.open("broken.silico", &features);
    let read = f.read_sketch(&session, "s0000001");
    assert_eq!(read["result"]["plane"], Value::Null, "{read}");
    let candidate = read["result"]["plane_problem"]["candidates"][0].clone();
    assert_eq!(candidate, "b0000001.box.Face6");
    let set = f.set(
        &session,
        "s0000001",
        "plane",
        json!({"face": candidate, "x": "+x", "reversed": false}),
    );
    assert_eq!(set["ok"], true, "{set}");
    assert_eq!(
        set["result"]["before"],
        json!({"face": "b0000001.box.Face7", "x": "+x", "reversed": false})
    );
    assert_eq!(
        set["result"]["after"],
        json!({"face": "b0000001.box.Face6", "x": "+x", "reversed": false})
    );
    let again = f.read_sketch(&session, "s0000001");
    assert_eq!(
        again["result"]["plane"],
        json!({"origin": [0, 0, 10], "x": [1, 0, 0], "y": [0, 1, 0], "normal": [0, 0, 1]})
    );
    assert_eq!(again["result"]["plane_problem"], Value::Null, "{again}");
    assert_eq!(two_d(&again), two_d(&read));
    let measured = f.call(
        "mass_properties",
        &json!({"session": session, "model": "current", "feature": "p0000001"}).to_string(),
    );
    assert_eq!(measured["ok"], true, "{measured}");
    let volume = measured["result"]["bodies"][0]["volume"].as_f64().unwrap();
    assert_near(volume, 1024.0, 1e-9 * 1024.0, "the mended pad's volume");
    // Its face set alone, plane.face, is the same repair.
    let mut features = padded(10.0);
    features[1] = features[1].replace("b0000001.box.Face6", "b0000001.box.Face7");
    let other = f.open("broken-2.silico", &features);
    let set = f.set(
        &other,
        "s0000001",
        "plane.face",
        json!("b0000001.box.Face6"),
    );
    assert_eq!(set["ok"], true, "{set}");
    let measured = f.call(
        "mass_properties",
        &json!({"session": other, "model": "current", "feature": "p0000001"}).to_string(),
    );
    let volume = measured["result"]["bodies"][0]["volume"].as_f64().unwrap();
    assert_near(
        volume,
        1024.0,
        1e-9 * 1024.0,
        "the pad mended by plane.face",
    );
}

#[test]
fn a_padded_sketchs_plane_set_on_a_warm_session_moves_the_pad_to_each_plane_in_turn() {
    // A 4 × 2 rectangle from (1, 1), padded 3 as a body of its own beside a 10 mm cube, its
    // plane set by set_field on one session, each rebuild warm from the one before (its
    // cache and kept steps): on xy the pad spans (1..5, 1..3, 0..3), its centre of mass
    // (3, 2, 1.5); on xz (x +X, y +Z, normal −Y, D6) it spans (1..5, −3..0, 1..3), its
    // centre (3, −1.5, 2); on the cube's top, +x (the note's table: o (0, 0, 10), x +X,
    // y +Y), (1..5, 1..3, 10..13), its centre (3, 2, 11.5). 24 mm³ each time.
    let f = fixture();
    let session = f.open(
        "planes.silico",
        &[
            box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0),
            rectangle("s0000001", "xy", [1.0, 1.0], 4.0, 2.0),
            pad("p0000001", None, "s0000001", &one_side("3")),
        ],
    );
    let pad_at = |what: &str, centre: [f64; 3]| {
        let measured = f.call(
            "mass_properties",
            &json!({"session": session, "model": "current", "feature": "p0000001"}).to_string(),
        );
        assert_eq!(measured["ok"], true, "{what}: {measured}");
        let body = &measured["result"]["bodies"][0];
        assert_near(body["volume"].as_f64().unwrap(), 24.0, 1e-9 * 24.0, what);
        for (k, want) in centre.into_iter().enumerate() {
            let got = body["center_of_mass"][k].as_f64().unwrap();
            assert_near(got, want, 1e-9, &format!("{what}: centre {k}"));
        }
    };
    pad_at("on xy", [3.0, 2.0, 1.5]);
    let set = f.set(&session, "s0000001", "plane", json!("xz"));
    assert_eq!(set["ok"], true, "{set}");
    pad_at("on xz", [3.0, -1.5, 2.0]);
    let set = f.set(
        &session,
        "s0000001",
        "plane",
        json!({"face": "b0000001.box.Face6", "x": "+x", "reversed": false}),
    );
    assert_eq!(set["ok"], true, "{set}");
    pad_at("on the cube's top", [3.0, 2.0, 11.5]);
    // And back to xy: where it began.
    let set = f.set(&session, "s0000001", "plane", json!("xy"));
    assert_eq!(set["ok"], true, "{set}");
    pad_at("on xy again", [3.0, 2.0, 1.5]);
}
