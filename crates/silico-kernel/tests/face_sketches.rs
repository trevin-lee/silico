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

#[test]
fn the_sketch_read_of_a_face_sketch_whose_face_broke_is_refused_with_its_own_failure() {
    let f = fixture();
    let mut features = padded(10.0);
    features[1] = features[1].replace("b0000001.box.Face6", "b0000001.box.Face7");
    let session = f.open("broken.silico", &features);
    let read = f.read_sketch(&session, "s0000001");
    assert_eq!(read["ok"], false, "{read}");
    let error = &read["errors"][0];
    assert_eq!(error["code"], "reference_broken", "{read}");
    assert_eq!(error["feature"], "s0000001");
    assert_eq!(error["parameter"], "plane.face");
    assert_eq!(error["candidates"], json!(["b0000001.box.Face6"]));
}
