//! break-it on sketches on faces, on the real kernel (M5.9, docs/ports/attacher.md): the
//! frame worked out from a face OCCT describes (`face_plane::frame`), the rebuild's rounds
//! (`Engine::run`, `placed_faces`), and the reach check (`beyond_reach`). Faces that aren't
//! a box's axis-aligned ones (a pad's slanted side, a chamfer's bevel), a frame whose origin
//! is past a kilometre though every point drawn is within one, a revolve whose axis is
//! placed past a kilometre, a chain of six faces deep, a pocket's floor, and an edit two
//! rounds up rebuilt on a warm engine.
//!
//! Every expected value is worked by hand from the geometry: the frame by F1 and F2 (the
//! origin the model's origin projected onto the face's plane, x the token's axis projected,
//! y = n × x), each volume as a prism's area times its length, each bound from the corners.
//! Sketches are solved as stored (no constraints), by the fake sketch solver.

use super::sweeps::*;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use silico_core::ErrorCode;
use silico_core::fake::FakeSketches;
use silico_core::kernel::ElementKind;
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_protocol::FaceFrame;

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

const HANG: Duration = Duration::from_secs(180);

fn engine() -> Recompute<KernelPool> {
    let pool = KernelPool::start_workers(Path::new(WORKER), 2).unwrap();
    Recompute::new(pool).with_sketches(FakeSketches::new())
}

fn publish(engine: &Recompute<KernelPool>, features: &[String]) -> Arc<Model> {
    let features: Vec<&str> = features.iter().map(String::as_str).collect();
    match engine.start(checked_document(&features)).wait_timeout(HANG) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the rebuild didn't finish in {HANG:?}"),
    }
}

fn face(name: &str, x: &str, reversed: bool) -> String {
    format!(
        r#"{{"face": {}, "x": "{x}", "reversed": {reversed}}}"#,
        serde_json::to_string(name).unwrap()
    )
}

/// A `w` × `h` rectangle from `at`, sketch `id`, on the face plane `plane` (its text).
fn on_face(id: &str, plane: &str, at: [f64; 2], w: f64, h: f64) -> String {
    rectangle(id, "xy", at, w, h).replace(r#""plane": "xy""#, &format!(r#""plane": {plane}"#))
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let l = dot(v, v).sqrt();
    v.map(|c| c / l)
}

/// The stable name of the planar face of `feature`'s solid whose outward normal is within
/// 1e-9 of `normal` (and, if given, whose centroid's z is `z`): the one such face.
fn planar_face(model: &Model, feature: &str, normal: [f64; 3], z: Option<f64>) -> String {
    let entries = match model.entries(&id(feature)) {
        Some(Ok(entries)) => entries,
        other => panic!("{feature}: {other:?}"),
    };
    let found: Vec<String> = entries
        .iter()
        .filter(|e| e.kind == ElementKind::Face && e.stable)
        .filter(|e| {
            e.normal
                .is_some_and(|n| (0..3).all(|k| (n[k] - normal[k]).abs() <= 1e-9))
        })
        .filter(|e| z.is_none_or(|z| e.centroid.is_some_and(|c| (c[2].mm() - z).abs() <= 1e-6)))
        .map(|e| e.name.to_string())
        .collect();
    assert_eq!(found.len(), 1, "{feature}, normal {normal:?}: {found:?}");
    found[0].clone()
}

/// The stable name of the straight edge of `feature`'s solid from `a` to `b` (either way).
fn edge_between(model: &Model, feature: &str, a: [f64; 3], b: [f64; 3]) -> String {
    let entries = match model.entries(&id(feature)) {
        Some(Ok(entries)) => entries,
        other => panic!("{feature}: {other:?}"),
    };
    let near = |p: [silico_core::document::Length; 3], q: [f64; 3]| {
        (0..3).all(|k| (p[k].mm() - q[k]).abs() <= 1e-6)
    };
    let found: Vec<String> = entries
        .iter()
        .filter(|e| e.kind == ElementKind::Edge && e.stable)
        .filter(|e| match (e.start, e.end) {
            (Some(s), Some(t)) => (near(s, a) && near(t, b)) || (near(s, b) && near(t, a)),
            _ => false,
        })
        .map(|e| e.name.to_string())
        .collect();
    assert_eq!(found.len(), 1, "{feature} {a:?}–{b:?}: {found:?}");
    found[0].clone()
}

fn built_summary(model: &Model, feature: &str) -> silico_protocol::ShapeSummary {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { built, .. }) => built.summary().unwrap().clone(),
        other => panic!("{feature}: {other:?}"),
    }
}

#[track_caller]
fn near(got: f64, want: f64, tolerance: f64, what: &str) {
    assert!(
        (got - want).abs() <= tolerance,
        "{what}: {got} against {want}"
    );
}

fn frame_of(model: &Model, sketch: &str) -> FaceFrame {
    *model
        .frame(&id(sketch))
        .unwrap_or_else(|| panic!("{sketch}: {:?}", model.outcome(&id(sketch))))
}

/// A frame's normal, x × y.
fn normal_of(f: &FaceFrame) -> [f64; 3] {
    silico_protocol::SketchPlane::Face(*f).normal()
}

/// A triangle on xy through `corners`, sketch `id`, padded `height` on its own: `p` + id's
/// tail.
fn triangle_prism(sketch: &str, prism: &str, corners: [[f64; 2]; 3], height: &str) -> Vec<String> {
    vec![
        polygon(sketch, "xy", &corners),
        pad(prism, None, sketch, &one_side(height)),
    ]
}

#[test]
fn a_sketch_on_a_slanted_face_whose_frame_origin_is_past_a_kilometre_but_whose_points_are_within_one_pads()
 {
    // BREAKS: the frame's origin, the model's origin projected onto the face's plane, is
    // the plane's point nearest the origin, but not within a kilometre in each component
    // when the face is slanted: the face's points can be, and the frame's check still
    // refuses its origin. Every value the document writes, every point placed, and the
    // pad's solid are within reach; the faces request is refused on `plane` naming an
    // origin the document never wrote.
    //
    // The triangle (1e6, 5e5), (950000, 1e6), (950000, 5e5) on xy, padded 10: its slanted
    // side through the first two faces n = (1, 0.1, 0)/√1.01 (the third corner is behind
    // it: n·(C − A) = −5e4/√1.01 < 0). n·A = 1.05e6/√1.01, so o = n(n·A) = (1.05e6/1.01)(1,
    // 0.1, 0) = (1039603.96, 103960.40, 0): past 1e6 in x. The token `+y` (n is nearest +X)
    // projects to x = (−0.1, 1, 0)/√1.01; y = n × x = +Z.
    let corners = [[1e6, 5e5], [950000.0, 1e6], [950000.0, 5e5]];
    let mut features = triangle_prism("s0000001", "p0000001", corners, "10");
    let first = publish(&engine(), &features);
    let n = unit([1.0, 0.1, 0.0]);
    let side = planar_face(&first, "p0000001", n, None);
    // The side's middle, (975000, 750000, 5), in the sketch's coordinates: u = (M − o)·x,
    // v = 5. A 4 × 2 rectangle about it, padded 1 on its own: 8 mm³.
    let o = [1.05e6 / 1.01, 1.05e5 / 1.01, 0.0];
    let x = unit([-0.1, 1.0, 0.0]);
    let u = dot([975000.0 - o[0], 750000.0 - o[1], 0.0], x).round();
    features.push(on_face(
        "s0000002",
        &face(&side, "+y", false),
        [u - 2.0, 4.0],
        4.0,
        2.0,
    ));
    features.push(pad("p0000002", None, "s0000002", &one_side("1")));
    let model = publish(&engine(), &features);
    let f = frame_of(&model, "s0000002");
    for k in 0..3 {
        near(f.origin[k], o[k], 1e-6, "the frame's origin");
        near(f.x[k], x[k], 1e-12, "the frame's x");
        near(normal_of(&f)[k], n[k], 1e-12, "the frame's normal");
    }
    // Built, 8 mm³: the frame's origin is within the frame's own reach (√3 · 1e6, its
    // norm 1.045e6), and every point placed within a kilometre on each axis.
    let s = built_summary(&model, "p0000002");
    near(s.volume, 8.0, 1e-6, "the pad's volume");
    assert!(s.valid);
}

#[test]
fn two_alike_sketches_on_one_slanted_face_share_its_faces_step_and_each_pads_a_body_of_its_own() {
    // Two sketches drawn alike on the slanted side of the previous test share one faces
    // step (its key has no ID): each pads a body of its own, 8 mm³, valid, neither
    // refused, the one step serving both.
    let corners = [[1e6, 5e5], [950000.0, 1e6], [950000.0, 5e5]];
    let mut features = triangle_prism("s0000001", "p0000001", corners, "10");
    let first = publish(&engine(), &features);
    let side = planar_face(&first, "p0000001", unit([1.0, 0.1, 0.0]), None);
    let o = [1.05e6 / 1.01, 1.05e5 / 1.01, 0.0];
    let u = dot(
        [975000.0 - o[0], 750000.0 - o[1], 0.0],
        unit([-0.1, 1.0, 0.0]),
    )
    .round();
    for (sketch, padded) in [("s0000002", "p0000002"), ("s0000003", "p0000003")] {
        features.push(on_face(
            sketch,
            &face(&side, "+y", false),
            [u - 2.0, 4.0],
            4.0,
            2.0,
        ));
        features.push(pad(padded, None, sketch, &one_side("1")));
    }
    let model = publish(&engine(), &features);
    // Each built, its own body, 8 mm³: one faces step between them, neither refused.
    for padded in ["p0000002", "p0000003"] {
        let s = built_summary(&model, padded);
        near(s.volume, 8.0, 1e-6, padded);
        assert!(s.valid, "{padded}");
    }
}

#[test]
fn a_profile_placed_past_a_kilometre_on_a_bevel_is_refused_on_the_sketch_and_blocks_its_pad() {
    // The bevel of a cube from z = 999990, chamfered 1 at its top front edge: o = (0,
    // −499999.5, 499999.5), x = +X, y = (0, 1, 1)/√2. A square at v = 999998 (as drawn,
    // within a kilometre) lands at z = 499999.5 + 999998/√2 ≈ 1207105: past it. The core
    // refuses the sketch (`beyond_reach`), naming it and `plane`, and blocks the pad.
    let cube = box_at("b0000001", [0.0, 0.0, 999990.0], 10.0, 10.0, 10.0);
    let first = publish(&engine(), std::slice::from_ref(&cube));
    let edge = edge_between(&first, "b0000001", [0.0, 0.0, 1e6], [10.0, 0.0, 1e6]);
    let chamfer = format!(
        r#"{{"id": "c0000001", "type": "chamfer", "v": 1, "name": "Bevel", "base": "b0000001", "edges": [{edge:?}], "size": {{"distance": 1}}}}"#
    );
    let second = publish(&engine(), &[cube.clone(), chamfer.clone()]);
    let bevel = planar_face(&second, "c0000001", unit([0.0, -1.0, 1.0]), None);
    let features = vec![
        cube,
        chamfer,
        on_face(
            "s0000001",
            &face(&bevel, "+x", false),
            [2.0, 999998.0],
            1.0,
            1.0,
        ),
        pad("p0000001", Some("c0000001"), "s0000001", &one_side("1")),
    ];
    let model = publish(&engine(), &features);
    let Some(FeatureOutcome::Failed(e)) = model.outcome(&id("s0000001")) else {
        panic!("{:?}", model.outcome(&id("s0000001")));
    };
    assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
    assert_eq!(e.feature(), Some("s0000001"));
    assert_eq!(e.parameter(), Some("plane"));
    assert!(e.message().contains("past 1e6 mm"), "{}", e.message());
    assert_eq!(
        model.outcome(&id("p0000001")),
        Some(&FeatureOutcome::Blocked { on: id("s0000001") })
    );
    // The model holds no frame for a sketch it refused; the read answers, its plane null
    // and the refusal in plane_problem, the error check gives (below).
    assert!(model.frame(&id("s0000001")).is_none());
    let dir = tempfile::tempdir().unwrap();
    let sessions = silico_core::command::Sessions::with_stand_ins(
        KernelPool::start_on_demand(Path::new(WORKER), 2).unwrap(),
        |cache: &Path| silico_core::cache::Cache::unsealed(cache),
        || Ok(Box::new(FakeSketches::new()) as Box<dyn silico_core::sketch::SketchSolver>),
    );
    let path = dir.path().join("bevel.silico");
    let refs: Vec<&str> = features.iter().map(String::as_str).collect();
    std::fs::write(&path, document(&refs)).unwrap();
    let call = |name: &str, arguments: String| -> serde_json::Value {
        match sessions.call(name, &arguments) {
            silico_core::command::Call::Running(calling) => {
                let answered = calling
                    .wait_timeout(HANG)
                    .unwrap_or_else(|_| panic!("{name} didn't answer"));
                serde_json::from_str(&answered.text).unwrap()
            }
            silico_core::command::Call::Unknown => panic!("no command {name}"),
        }
    };
    let opened = call(
        "open",
        serde_json::json!({"path": path.to_str().unwrap(), "create": false}).to_string(),
    );
    assert_eq!(opened["ok"], true, "{opened}");
    let read = call(
        "sketch",
        serde_json::json!({"session": "s1", "model": "current", "sketch": "s0000001"}).to_string(),
    );
    assert_eq!(read["ok"], true, "{read}");
    let problem = &read["result"]["plane_problem"];
    assert_eq!(read["result"]["plane"], serde_json::Value::Null, "{read}");
    assert_eq!(problem["code"], "invalid_input", "{read}");
    assert_eq!(problem["parameter"], "plane", "{read}");
    assert_eq!(problem["feature"], "s0000001", "{read}");
    assert_eq!(problem["message"], e.message(), "{read}");
    assert_eq!(
        read["result"]["solve_problem"],
        serde_json::Value::Null,
        "{read}"
    );
}

#[test]
fn a_sketch_on_a_chamfers_bevel_pads_a_prism_square_to_it() {
    // A 10 mm cube's top front edge (y = 0, z = 10) chamfered 2: its bevel's plane runs from
    // (·, 0, 8) to (·, 2, 10), n = (0, −1, 1)/√2; o = n(n·p) = (0, −4, 4) for p = (0, 0, 8).
    // n is as near Y as Z: Y wins the tie, n_y < 0, so the default token is `+x`, x = +X,
    // and y = n × x = (0, 1, 1)/√2. The bevel spans v from 8/√2 to 12/√2 (5.66 to 8.49):
    // the rectangle u in 2..6, v in 6..8 lies on it, and padded 1 onto the chamfer adds
    // 4 × 2 × 1 = 8 to the chamfered cube's 1000 − ½ · 2 · 2 · 10 = 980: 988. Its bounds
    // are its corners o + u·x + v·y + t·n, t in 0..1.
    let cube = box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0);
    let first = publish(&engine(), std::slice::from_ref(&cube));
    let edge = edge_between(&first, "b0000001", [0.0, 0.0, 10.0], [10.0, 0.0, 10.0]);
    let chamfer = format!(
        r#"{{"id": "c0000001", "type": "chamfer", "v": 1, "name": "Bevel", "base": "b0000001", "edges": [{edge:?}], "size": {{"distance": 2}}}}"#
    );
    let n = unit([0.0, -1.0, 1.0]);
    let second = publish(&engine(), &[cube.clone(), chamfer.clone()]);
    let bevel = planar_face(&second, "c0000001", n, None);
    let features = vec![
        cube,
        chamfer,
        on_face("s0000001", &face(&bevel, "+x", false), [2.0, 6.0], 4.0, 2.0),
        pad("p0000001", Some("c0000001"), "s0000001", &one_side("1")),
    ];
    let model = publish(&engine(), &features);
    let f = frame_of(&model, "s0000001");
    f.check().unwrap();
    let y = unit([0.0, 1.0, 1.0]);
    for k in 0..3 {
        near(f.origin[k], [0.0, -4.0, 4.0][k], 1e-12, "origin");
        near(f.x[k], [1.0, 0.0, 0.0][k], 1e-15, "x");
        near(f.y[k], y[k], 1e-15, "y");
        near(normal_of(&f)[k], n[k], 1e-15, "normal");
    }
    near(
        built_summary(&model, "c0000001").volume,
        980.0,
        1e-6,
        "chamfer",
    );
    let s = built_summary(&model, "p0000001");
    near(s.volume, 988.0, 1e-6, "the pad onto the chamfer");
    assert!(s.valid);
    // The pad's own corners, the highest z among them: v = 8, t = 1: 4 + 9/√2.
    let [_, high] = s.bounds.unwrap();
    near(
        high[2],
        10.0f64.max(4.0 + 9.0 / 2f64.sqrt()),
        1e-7,
        "the pad's top",
    );
    // And the pad's corner lowest in y, below the cube's front (y = 0): v = 6, t = 1:
    // −4 + 5/√2.
    let [low, _] = s.bounds.unwrap();
    near(low[1], -4.0 + 5.0 / 2f64.sqrt(), 1e-7, "the pad's front");
}

#[test]
fn a_revolve_about_an_axis_placed_past_a_kilometre_is_refused_naming_the_sketch_and_its_plane() {
    // A cube from z = 999990 to the kilometre, its top front edge chamfered 1: the bevel's
    // o = (0, −499999.5, 499999.5), x = +X, y = (0, 1, 1)/√2. The bevel's points have v =
    // 1e6/√2 ≈ 707106.78. A square there, revolved 1° about a construction line at v = 1e6
    // (within a kilometre, as drawn), whose placed point is o + 1e6·y = (0, 207106.78,
    // 1207106.78): past a kilometre in z. Refused, as docs/ports/attacher.md has it, on the
    // sketch's `plane`, naming the axis line nb000001, as `invalid_input`, and the revolve
    // blocked by it: never a bug, a crash, or a refusal of a point the document never wrote.
    let cube = box_at("b0000001", [0.0, 0.0, 999990.0], 10.0, 10.0, 10.0);
    let first = publish(&engine(), std::slice::from_ref(&cube));
    let edge = edge_between(&first, "b0000001", [0.0, 0.0, 1e6], [10.0, 0.0, 1e6]);
    let chamfer = format!(
        r#"{{"id": "c0000001", "type": "chamfer", "v": 1, "name": "Bevel", "base": "b0000001", "edges": [{edge:?}], "size": {{"distance": 1}}}}"#
    );
    let second = publish(&engine(), &[cube.clone(), chamfer.clone()]);
    let bevel = planar_face(&second, "c0000001", unit([0.0, -1.0, 1.0]), None);
    let sketch = format!(
        r#"{{"id": "s0000001", "type": "sketch", "v": 1, "name": "S", "plane": {}, "points": [{{"id": "pa000001", "x": 2, "y": 707106.5, "construction": false}}, {{"id": "pa000002", "x": 3, "y": 707106.5, "construction": false}}, {{"id": "pa000003", "x": 3, "y": 707107, "construction": false}}, {{"id": "pa000004", "x": 2, "y": 707107, "construction": false}}, {{"id": "pb000001", "x": 0, "y": 1000000, "construction": true}}, {{"id": "pb000002", "x": 1, "y": 1000000, "construction": true}}], "lines": [{{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}}, {{"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}}, {{"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}}, {{"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}}, {{"id": "nb000001", "from": "pb000001", "to": "pb000002", "construction": true}}], "circles": [], "arcs": [], "constraints": []}}"#,
        face(&bevel, "+x", false)
    );
    let features = vec![
        cube,
        chamfer,
        sketch,
        turned(
            "r0000001",
            "revolve",
            None,
            "s0000001",
            "nb000001",
            &angle("1"),
        ),
    ];
    let model = publish(&engine(), &features);
    // Found by break-it, now fixed: the reach was checked of the profile's points placed on
    // the frame, not of the revolve's axis, so the revolution request was refused by the
    // protocol's check, on `axis`, naming the placed point, which the document doesn't
    // hold, with the advice "give the revolution a valid axis".
    let Some(FeatureOutcome::Failed(e)) = model.outcome(&id("s0000001")) else {
        panic!("{:?}", model.outcome(&id("s0000001")));
    };
    assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
    assert_eq!(e.feature(), Some("s0000001"), "{e:?}");
    assert_eq!(e.parameter(), Some("plane"), "{e:?}");
    assert_eq!(e.elements(), ["nb000001"], "{e:?}");
    assert!(e.message().contains("r0000001's axis nb000001"), "{e:?}");
    assert_eq!(
        model.outcome(&id("r0000001")),
        Some(&FeatureOutcome::Blocked { on: id("s0000001") })
    );
}

/// A chain `depth` deep: the box, then at each level a 4 × 2 rectangle from (1, 1) on the
/// top of what's below (the box's Face6, then each pad's top, kept by its fuse as the pad
/// names it), padded 1 onto it.
fn chain(depth: usize, first_length: &str) -> Vec<String> {
    let mut features = vec![box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0)];
    for k in 1..=depth {
        let (name, base) = if k == 1 {
            ("b0000001.box.Face6".to_owned(), "b0000001".to_owned())
        } else {
            (
                format!("p{:07}.pad.Face(G s{:07}.sketch.Face1)", k - 1, k - 1),
                format!("p{:07}", k - 1),
            )
        };
        features.push(on_face(
            &format!("s{k:07}"),
            &face(&name, "+x", false),
            [1.0, 1.0],
            4.0,
            2.0,
        ));
        let length = if k == 1 { first_length } else { "1" };
        features.push(pad(
            &format!("p{k:07}"),
            Some(&base),
            &format!("s{k:07}"),
            &one_side(length),
        ));
    }
    features
}

#[test]
fn a_chain_six_faces_deep_places_each_sketch_on_the_last_pads_top_in_its_own_round() {
    // Level k's frame is at z = 10 + (k − 1) (each pad 1 high), x +X, y +Y; the last pad's
    // solid 1000 + 6 · 8 = 1048, its top at z = 16.
    let model = publish(&engine(), &chain(6, "1"));
    for k in 1..=6 {
        let f = frame_of(&model, &format!("s{k:07}"));
        assert_eq!(
            (f.origin, f.x, f.y, normal_of(&f)),
            (
                [0.0, 0.0, 9.0 + k as f64],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0]
            ),
            "level {k}"
        );
    }
    let s = built_summary(&model, "p0000006");
    near(s.volume, 1048.0, 1e-9, "the last pad");
    assert!(s.valid, "the last pad isn't valid");
    near(s.bounds.unwrap()[1][2], 16.0, 1e-7, "the last pad's top");
}

#[test]
fn an_edit_of_the_first_pad_moves_every_face_above_it_on_a_warm_engine_and_back() {
    // The chain three deep, the first pad 1 then 5 long, on one engine (every step kept):
    // levels 2 and 3 move up by 4, the last pad 1000 + 8·5 + 8 + 8 = 1056, its top at 17.
    // And back to 1: 1024, top 13, the same frames as the first build.
    let engine = engine();
    let short = publish(&engine, &chain(3, "1"));
    let frames = |model: &Model| {
        (1..=3)
            .map(|k| frame_of(model, &format!("s{k:07}")).origin)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        frames(&short),
        [[0.0, 0.0, 10.0], [0.0, 0.0, 11.0], [0.0, 0.0, 12.0]]
    );
    let s = built_summary(&short, "p0000003");
    near(s.volume, 1024.0, 1e-9, "short");
    assert!(s.valid, "short isn't valid");
    let long = publish(&engine, &chain(3, "5"));
    assert_eq!(
        frames(&long),
        [[0.0, 0.0, 10.0], [0.0, 0.0, 15.0], [0.0, 0.0, 16.0]]
    );
    let s = built_summary(&long, "p0000003");
    near(s.volume, 1056.0, 1e-9, "long");
    assert!(s.valid, "long isn't valid");
    near(s.bounds.unwrap()[1][2], 17.0, 1e-7, "long's top");
    let back = publish(&engine, &chain(3, "1"));
    assert_eq!(frames(&back), frames(&short));
    let s = built_summary(&back, "p0000003");
    near(s.volume, 1024.0, 1e-9, "back");
    assert!(s.valid, "back isn't valid");
}

#[test]
fn a_sketch_on_a_pockets_floor_is_placed_on_the_floor_and_pads_up_from_it() {
    // A 4 × 2 pocket 3 deep from (1, 1) on the cube's top: its floor at z = 7, n = +Z. A 1 ×
    // 1 square from (2, 1.5) on the floor, padded 1 onto the pocket: 1000 − 24 + 1 = 977.
    let cube = box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0);
    let top = on_face(
        "s0000001",
        &face("b0000001.box.Face6", "+x", false),
        [1.0, 1.0],
        4.0,
        2.0,
    );
    let pocketed = pocket("q0000001", "b0000001", "s0000001", &one_side("3"));
    let first = publish(&engine(), &[cube.clone(), top.clone(), pocketed.clone()]);
    let pocketed_summary = built_summary(&first, "q0000001");
    near(pocketed_summary.volume, 976.0, 1e-9, "pocket");
    assert!(pocketed_summary.valid, "the pocket isn't valid");
    let floor = planar_face(&first, "q0000001", [0.0, 0.0, 1.0], Some(7.0));
    let features = vec![
        cube,
        top,
        pocketed,
        on_face("s0000002", &face(&floor, "+x", false), [2.0, 1.5], 1.0, 1.0),
        pad("p0000002", Some("q0000001"), "s0000002", &one_side("1")),
    ];
    let model = publish(&engine(), &features);
    assert_eq!(frame_of(&model, "s0000002").origin, [0.0, 0.0, 7.0]);
    let s = built_summary(&model, "p0000002");
    near(s.volume, 977.0, 1e-9, "pad");
    assert!(s.valid, "the pad on the floor isn't valid");
}

#[test]
fn a_sketch_on_a_boxs_top_after_a_fillet_of_a_vertical_edge_is_found_in_the_fillet() {
    // The fillet rounds a vertical edge, which trims the top face's outline but not its
    // plane: the top, modified one from one, keeps its name, and is found in the fillet's
    // solid (z = 10), the pad onto it the filleted box and 24.
    let cube = box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0);
    let first = publish(&engine(), std::slice::from_ref(&cube));
    let edge = edge_between(&first, "b0000001", [10.0, 10.0, 0.0], [10.0, 10.0, 10.0]);
    let fillet = format!(
        r#"{{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "b0000001", "edges": [{edge:?}], "radius": 1}}"#
    );
    let features = vec![
        cube,
        fillet,
        on_face(
            "s0000001",
            &face("b0000001.box.Face6", "+x", false),
            [1.0, 1.0],
            4.0,
            2.0,
        ),
        pad("p0000001", Some("f0000001"), "s0000001", &one_side("3")),
    ];
    let model = publish(&engine(), &features);
    let outcome = model.outcome(&id("s0000001"));
    assert!(
        matches!(outcome, Some(FeatureOutcome::Sketch(_))),
        "{outcome:?}"
    );
    assert_eq!(frame_of(&model, "s0000001").origin, [0.0, 0.0, 10.0]);
    let fillet = built_summary(&model, "f0000001");
    assert!(fillet.valid, "the fillet isn't valid");
    let s = built_summary(&model, "p0000001");
    near(s.volume, fillet.volume + 24.0, 1e-6, "pad");
    assert!(s.valid, "the pad onto the fillet isn't valid");
}

#[test]
fn a_sketch_on_a_boxs_top_after_a_fuse_with_another_box_is_found_in_the_fuse_and_pads_onto_it() {
    // Found by the tests review: on the fake, every face found is refused as not planar, so
    // nothing showed the face found through the solid that consumed its maker. A 10 mm box
    // b1, a 1 mm box b2 at (10, 0, 0) against its +X side (a fuse of boxes apart makes two
    // solids, which Silico refuses until bodies exist), and their fuse; then s1 on b1's top,
    // `+x`, a 4 × 2 rectangle from (1, 1), padded 3 onto the fuse. The top, untouched by
    // the fuse, is found in its solid (b1 is consumed): the frame ((0, 0, 10), +X, +Y, +Z),
    // by hand, and the pad 1000 + 1 + 24 = 1025, from (0, 0, 0) to (11, 10, 13).
    let fuse = r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "Both", "base": "b0000001", "tool": "b0000002"}"#;
    let features = vec![
        box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0),
        box_at("b0000002", [10.0, 0.0, 0.0], 1.0, 1.0, 1.0),
        fuse.to_owned(),
        on_face(
            "s0000001",
            &face("b0000001.box.Face6", "+x", false),
            [1.0, 1.0],
            4.0,
            2.0,
        ),
        pad("p0000001", Some("f0000001"), "s0000001", &one_side("3")),
    ];
    let model = publish(&engine(), &features);
    let f = frame_of(&model, "s0000001");
    assert_eq!(
        (f.origin, f.x, f.y, normal_of(&f)),
        (
            [0.0, 0.0, 10.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0]
        )
    );
    let s = built_summary(&model, "p0000001");
    near(s.volume, 1025.0, 1e-9, "the pad onto the fuse");
    assert!(s.valid, "the pad onto the fuse isn't valid");
    let [low, high] = s.bounds.unwrap();
    for k in 0..3 {
        near(low[k], [0.0, 0.0, 0.0][k], 1e-7, "the low corner");
        near(high[k], [11.0, 10.0, 13.0][k], 1e-7, "the high corner");
    }
}

#[test]
fn a_sketch_on_a_boxs_top_after_a_fillet_of_a_bottom_edge_is_found_in_the_fillet() {
    // The fillet rounds the box's bottom front edge (y = 0, z = 0), which no edge of the
    // top touches: the top is kept, its name with it, and found in the fillet's solid. The
    // frame ((0, 0, 10), +X, +Y); the pad of a 4 × 2 rectangle, 3 high, onto the fillet:
    // the filleted box, 1000 − (1 − π/4) · 10 = 997.85, and 24, its top at 13.
    let cube = box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0);
    let first = publish(&engine(), std::slice::from_ref(&cube));
    let edge = edge_between(&first, "b0000001", [0.0, 0.0, 0.0], [10.0, 0.0, 0.0]);
    let fillet = format!(
        r#"{{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "b0000001", "edges": [{edge:?}], "radius": 1}}"#
    );
    let features = vec![
        cube,
        fillet,
        on_face(
            "s0000001",
            &face("b0000001.box.Face6", "+x", false),
            [1.0, 1.0],
            4.0,
            2.0,
        ),
        pad("p0000001", Some("f0000001"), "s0000001", &one_side("3")),
    ];
    let model = publish(&engine(), &features);
    let f = frame_of(&model, "s0000001");
    assert_eq!(
        (f.origin, f.x, f.y),
        ([0.0, 0.0, 10.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0])
    );
    let filleted = 1000.0 - (1.0 - std::f64::consts::FRAC_PI_4) * 10.0;
    let fillet = built_summary(&model, "f0000001");
    near(fillet.volume, filleted, 1e-6, "the fillet");
    assert!(fillet.valid, "the fillet isn't valid");
    let s = built_summary(&model, "p0000001");
    near(s.volume, filleted + 24.0, 1e-6, "the pad onto the fillet");
    assert!(s.valid, "the pad onto the fillet isn't valid");
    near(s.bounds.unwrap()[1][2], 13.0, 1e-7, "the pad's top");
}

#[test]
fn a_sketch_on_a_boxs_top_after_a_pad_under_it_is_found_in_the_pad() {
    // A pad consumes the box: a 2 × 2 square from (2, 2) on xy, padded 1 down (reversed)
    // onto the box, which leaves its top as it was. s1 on the box's top is found in the
    // pad's solid: the frame ((0, 0, 10), +X, +Y); its pad, 3 high, onto the first: 1000 +
    // 4 + 24 = 1028, from (0, 0, −1) to (10, 10, 13).
    let under = r#"{"type": "one_side", "length": 1, "reversed": true}"#;
    let features = vec![
        box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0),
        rectangle("s0000009", "xy", [2.0, 2.0], 2.0, 2.0),
        pad("p0000009", Some("b0000001"), "s0000009", under),
        on_face(
            "s0000001",
            &face("b0000001.box.Face6", "+x", false),
            [1.0, 1.0],
            4.0,
            2.0,
        ),
        pad("p0000001", Some("p0000009"), "s0000001", &one_side("3")),
    ];
    let model = publish(&engine(), &features);
    let below = built_summary(&model, "p0000009");
    near(below.volume, 1004.0, 1e-9, "the pad under the box");
    assert!(below.valid, "the pad under the box isn't valid");
    let f = frame_of(&model, "s0000001");
    assert_eq!(
        (f.origin, f.x, f.y),
        ([0.0, 0.0, 10.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0])
    );
    let s = built_summary(&model, "p0000001");
    near(s.volume, 1028.0, 1e-9, "the pad on top");
    assert!(s.valid, "the pad on top isn't valid");
    let [low, high] = s.bounds.unwrap();
    for k in 0..3 {
        near(low[k], [0.0, 0.0, -1.0][k], 1e-7, "the low corner");
        near(high[k], [10.0, 10.0, 13.0][k], 1e-7, "the high corner");
    }
}

#[test]
fn a_sketch_on_a_top_a_slot_split_in_two_is_broken_with_both_pieces_as_candidates() {
    // A slot across the whole top (a box 2 wide, from y = −1 to 11, z = 8 to 13, cut from
    // the cube) splits Face6 in two, each a piece ranked by OCCT's order
    // (`c0000001.cut.Face(M1/2 b0000001.box.Face6)`, `M2/2`): the face as it was is gone
    // from the cut's map. s1 on it is reference_broken on plane.face, never re-pointed to a
    // piece, the two pieces its candidates (each shares its one source).
    let cut = r#"{"id": "c0000001", "type": "cut", "v": 1, "name": "Slot", "base": "b0000001", "tool": "t0000001"}"#;
    let features = vec![
        box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0),
        box_at("t0000001", [4.0, -1.0, 8.0], 2.0, 12.0, 5.0),
        cut.to_owned(),
        on_face(
            "s0000001",
            &face("b0000001.box.Face6", "+x", false),
            [1.0, 1.0],
            2.0,
            2.0,
        ),
    ];
    let model = publish(&engine(), &features);
    near(
        built_summary(&model, "c0000001").volume,
        1000.0 - 2.0 * 10.0 * 2.0,
        1e-9,
        "the slotted cube",
    );
    let Some(FeatureOutcome::Broken { errors, total }) = model.outcome(&id("s0000001")) else {
        panic!("{:?}", model.outcome(&id("s0000001")));
    };
    assert_eq!(*total, 1);
    let e = &errors[0];
    assert_eq!(e.code(), ErrorCode::ReferenceBroken);
    assert_eq!(e.parameter(), Some("plane.face"));
    assert_eq!(
        e.message(),
        "plane.face, \"b0000001.box.Face6\", names no face of the solids live at the sketch's place: it isn't one of the faces they're made from, or an edit before this sketch removed or remade it"
    );
    assert_eq!(
        e.candidates(),
        [
            "c0000001.cut.Face(M1/2 b0000001.box.Face6)",
            "c0000001.cut.Face(M2/2 b0000001.box.Face6)",
        ]
    );
    assert!(model.frame(&id("s0000001")).is_none());
}

#[test]
fn a_stored_token_that_isnt_the_default_turns_the_sketch_on_the_boxs_top() {
    // Face6 (n = +Z) stored `+y`, where its default is `+x`: x = +Y, y = n × x = (−1, 0,
    // 0), by hand. A 4 × 2 rectangle from (1, −3), padded 3 as a body of its own: u from 1
    // to 5 along +Y, v from −3 to −1 along −X, so the pad spans x 1..3, y 1..5, z 10..13
    // (a resolver that took the default, x = +X, would put it at x 1..5, y −3..−1).
    let features = vec![
        box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0),
        on_face(
            "s0000001",
            &face("b0000001.box.Face6", "+y", false),
            [1.0, -3.0],
            4.0,
            2.0,
        ),
        pad("p0000001", None, "s0000001", &one_side("3")),
    ];
    let model = publish(&engine(), &features);
    let f = frame_of(&model, "s0000001");
    assert_eq!(
        (f.origin, f.x, f.y, normal_of(&f)),
        (
            [0.0, 0.0, 10.0],
            [0.0, 1.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0]
        )
    );
    let s = built_summary(&model, "p0000001");
    near(s.volume, 24.0, 1e-9, "the pad");
    assert!(s.valid, "the pad isn't valid");
    let [low, high] = s.bounds.unwrap();
    for k in 0..3 {
        near(low[k], [1.0, 1.0, 10.0][k], 1e-7, "the pad's low corner");
        near(high[k], [3.0, 5.0, 13.0][k], 1e-7, "the pad's high corner");
    }
}

#[test]
fn a_face_tilted_past_45_degrees_keeps_its_stored_token_and_its_y_turns_with_it() {
    // docs/ports/attacher.md, *A tilted face's continuous turn*: n(θ) = (cos θ, 0, sin θ),
    // the token `+y`, chosen below 45° (X wins), kept past it (where Z would win and the
    // default be `+x`). The face here is a prism's slanted side, a triangle (0, 0), (10, 0),
    // (0, b) on xz padded along y (the document's chamfers take one distance, so its two
    // distances can't tilt a bevel): its hypotenuse's outward normal is (b, 0, 10)/√(100 +
    // b²), so tan θ = 10/b. b = 10.36 gives θ = 43.99°, b = 9.66 gives 46.01°. Each way x =
    // +Y, and y = n × x = (−sin θ, 0, cos θ) = (−10, 0, b)/√(100 + b²), by hand: Silico's y
    // turns smoothly with the face, where FreeCAD's turns 90° about it between them.
    for b in [10.36, 9.66] {
        let length = (100.0f64 + b * b).sqrt();
        let n = [b / length, 0.0, 10.0 / length];
        let prism = vec![
            polygon("s0000009", "xz", &[[0.0, 0.0], [10.0, 0.0], [0.0, b]]),
            pad("p0000009", None, "s0000009", &one_side("10")),
        ];
        let first = publish(&engine(), &prism);
        let side = planar_face(&first, "p0000009", n, None);
        let mut features = prism.clone();
        features.push(on_face(
            "s0000001",
            &face(&side, "+y", false),
            [1.0, 1.0],
            1.0,
            1.0,
        ));
        let model = publish(&engine(), &features);
        let f = frame_of(&model, "s0000001");
        let theta = libm::atan(10.0 / b).to_degrees();
        let y = [-10.0 / length, 0.0, b / length];
        for k in 0..3 {
            near(
                f.x[k],
                [0.0, 1.0, 0.0][k],
                1e-12,
                &format!("x at {theta:.2}°"),
            );
            near(f.y[k], y[k], 1e-12, &format!("y at {theta:.2}°"));
            near(normal_of(&f)[k], n[k], 1e-12, &format!("n at {theta:.2}°"));
        }
    }
}

#[test]
fn fifty_sketches_on_a_boxs_top_each_padded_onto_the_last_build_in_fifty_rounds() {
    // The robustness review's chain: a box B, then K = 50 times a sketch S_k on B's top and
    // a pad P_k of it onto P_{k−1} (P_1 onto B). Each pad's fuse leaves B's top one face,
    // its name kept, so S_k is found in P_{k−1}: each frame ((0, 0, 10), +X, +Y). Each S_k a
    // 0.5 × 0.5 square of its own on the top, padded 1: 1000 + 50 · 0.25 = 1012.5, from (0,
    // 0, 0) to (10, 10, 11). Fifty rounds, each placing one sketch, and a last that finds
    // none waiting: 51.
    let mut features = vec![box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0)];
    for k in 0..50 {
        let (column, row) = ((k % 10) as f64, (k / 10) as f64);
        let sketch = format!("s{:07}", k + 1);
        let base = if k == 0 {
            "b0000001".to_owned()
        } else {
            format!("p{k:07}")
        };
        features.push(on_face(
            &sketch,
            &face("b0000001.box.Face6", "+x", false),
            [0.5 + 0.9 * column, 0.5 + 1.8 * row],
            0.5,
            0.5,
        ));
        features.push(pad(
            &format!("p{:07}", k + 1),
            Some(&base),
            &sketch,
            &one_side("1"),
        ));
    }
    let engine = engine();
    let refs: Vec<&str> = features.iter().map(String::as_str).collect();
    let run = engine.start(checked_document(&refs));
    let progress = run.progress();
    let model = match run.wait_timeout(HANG) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the rebuild didn't finish in {HANG:?}"),
    };
    assert_eq!(progress.rounds(), 51);
    for k in 1..=50 {
        let f = frame_of(&model, &format!("s{k:07}"));
        assert_eq!(
            (f.origin, f.x, f.y),
            ([0.0, 0.0, 10.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            "s{k:07}"
        );
    }
    let s = built_summary(&model, "p0000050");
    near(s.volume, 1012.5, 1e-9, "the last pad");
    assert!(s.valid, "the last pad isn't valid");
    let [low, high] = s.bounds.unwrap();
    for k in 0..3 {
        near(low[k], 0.0, 1e-7, "the low corner");
        near(high[k], [10.0, 10.0, 11.0][k], 1e-7, "the high corner");
    }
}

/// Sketch `id` on `plane` (its text): the unit square from `at`, its corners pa000001 to
/// pa000004 and sides na000001 to na000004, and the construction line nb000001 from `from`
/// to `to`, an axis to turn about.
fn square_and_axis(id: &str, plane: &str, at: [f64; 2], from: [f64; 2], to: [f64; 2]) -> String {
    let [u, v] = at;
    let corners = [[u, v], [u + 1.0, v], [u + 1.0, v + 1.0], [u, v + 1.0]];
    let mut points: Vec<String> = corners
        .iter()
        .enumerate()
        .map(|(i, [x, y])| {
            format!(
                r#"{{"id": "pa00000{}", "x": {x}, "y": {y}, "construction": false}}"#,
                i + 1
            )
        })
        .collect();
    for (i, [x, y]) in [from, to].iter().enumerate() {
        points.push(format!(
            r#"{{"id": "pb00000{}", "x": {x}, "y": {y}, "construction": true}}"#,
            i + 1
        ));
    }
    let mut lines: Vec<String> = (0..4)
        .map(|i| {
            format!(
                r#"{{"id": "na00000{}", "from": "pa00000{}", "to": "pa00000{}", "construction": false}}"#,
                i + 1,
                i + 1,
                (i + 1) % 4 + 1
            )
        })
        .collect();
    lines.push(
        r#"{"id": "nb000001", "from": "pb000001", "to": "pb000002", "construction": true}"#
            .to_owned(),
    );
    format!(
        r#"{{"id": "{id}", "type": "sketch", "v": 1, "name": "S", "plane": {plane}, "points": [{}], "lines": [{}], "circles": [], "arcs": [], "constraints": []}}"#,
        points.join(", "),
        lines.join(", ")
    )
}

#[test]
fn a_revolve_of_a_face_sketch_about_an_axis_within_reach_builds_by_pappus() {
    // The cube's top, +x: the note's frame, o (0, 0, 10), x +X, y +Y. The unit square from
    // (1, 1), turned 90° about nb000001, the sketch's line from (0, 0) to (1, 0): placed, the
    // line through (0, 0, 10) along +X, well within reach. By Pappus, the square's area 1
    // times the path of its centroid, 1.5 from the axis, through π/2: 0.75π.
    let features = vec![
        box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0),
        square_and_axis(
            "s0000001",
            &face("b0000001.box.Face6", "+x", false),
            [1.0, 1.0],
            [0.0, 0.0],
            [1.0, 0.0],
        ),
        turned(
            "r0000001",
            "revolve",
            None,
            "s0000001",
            "nb000001",
            &angle("90"),
        ),
    ];
    let model = publish(&engine(), &features);
    let f = frame_of(&model, "s0000001");
    assert_eq!(
        (f.origin, f.x, f.y),
        ([0.0, 0.0, 10.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0])
    );
    let s = built_summary(&model, "r0000001");
    near(
        s.volume,
        0.75 * std::f64::consts::PI,
        1e-9,
        "the revolve, by Pappus",
    );
    assert!(s.valid);
}

/// The cube from z = 999990 to the kilometre, its top front edge chamfered 1, and the
/// chamfer's bevel's name (o = (0, −499999.5, 499999.5), x = +X, y = (0, 1, 1)/√2): as the
/// past-a-kilometre revolve test builds them.
fn far_bevel() -> (String, String, String) {
    let cube = box_at("b0000001", [0.0, 0.0, 999990.0], 10.0, 10.0, 10.0);
    let first = publish(&engine(), std::slice::from_ref(&cube));
    let edge = edge_between(&first, "b0000001", [0.0, 0.0, 1e6], [10.0, 0.0, 1e6]);
    let chamfer = format!(
        r#"{{"id": "c0000001", "type": "chamfer", "v": 1, "name": "Bevel", "base": "b0000001", "edges": [{edge:?}], "size": {{"distance": 1}}}}"#
    );
    let second = publish(&engine(), &[cube.clone(), chamfer.clone()]);
    let bevel = planar_face(&second, "c0000001", unit([0.0, -1.0, 1.0]), None);
    (cube, chamfer, bevel)
}

/// The sketch s0000001 on the far bevel: the unit square from (2, 707106), within reach
/// placed (its highest z 499999.5 + 707107/√2 ≈ 999999.65), and nb000001 at v = 1e6, which placed on the bevel passes through (0,
/// 207106.78, 1207106.78), past a kilometre in z.
fn far_square(bevel: &str) -> String {
    square_and_axis(
        "s0000001",
        &face(bevel, "+x", false),
        [2.0, 707106.0],
        [0.0, 1e6],
        [1.0, 1e6],
    )
}

/// A pad 1 long into the face it's sketched on, so a pad of a sketch on the far bevel,
/// whose square reaches z = 999999.65, stays within the kilometre.
const INTO: &str = r#"{"type": "one_side", "length": 1, "reversed": true}"#;

#[test]
fn a_suppressed_revolve_about_an_axis_past_a_kilometre_refuses_nothing_and_the_sketch_pads() {
    // The revolve whose axis would be past the reach is suppressed: it builds nothing, so
    // its axis is never placed, and the sketch is placed and padded, 1 × 1 × 1.
    let (cube, chamfer, bevel) = far_bevel();
    let revolve = turned(
        "r0000001",
        "revolve",
        None,
        "s0000001",
        "nb000001",
        &angle("1"),
    )
    .replace(
        r#""name": "Turned","#,
        r#""name": "Turned", "suppressed": true,"#,
    );
    assert!(revolve.contains(r#""suppressed": true"#), "{revolve}");
    let features = vec![
        cube,
        chamfer,
        far_square(&bevel),
        revolve,
        pad("p0000001", None, "s0000001", INTO),
    ];
    let model = publish(&engine(), &features);
    let f = frame_of(&model, "s0000001");
    assert_eq!(f.x, [1.0, 0.0, 0.0]);
    assert_eq!(
        model.outcome(&id("r0000001")),
        Some(&FeatureOutcome::Suppressed)
    );
    let s = built_summary(&model, "p0000001");
    near(s.volume, 1.0, 1e-6, "the pad");
    assert!(s.valid);
}

#[test]
fn a_revolve_of_another_sketch_never_refuses_this_one_though_its_axis_would_be_past_reach_here() {
    // s2, on xy, is turned about its own nb000001, from (0, 0) to (1, 0). s1 on the far bevel
    // has a line of the same ID, whose placing there passes a kilometre: but s1 is no
    // revolve's sketch, so it's placed and padded, and the revolve of s2 builds, by Pappus
    // 1 × 1.5 × π/2.
    let (cube, chamfer, bevel) = far_bevel();
    let features = vec![
        cube,
        chamfer,
        far_square(&bevel),
        pad("p0000001", None, "s0000001", INTO),
        square_and_axis("s0000002", r#""xy""#, [1.0, 1.0], [0.0, 0.0], [1.0, 0.0]),
        turned(
            "r0000002",
            "revolve",
            None,
            "s0000002",
            "nb000001",
            &angle("90"),
        ),
    ];
    let model = publish(&engine(), &features);
    assert_eq!(frame_of(&model, "s0000001").x, [1.0, 0.0, 0.0]);
    near(
        built_summary(&model, "p0000001").volume,
        1.0,
        1e-6,
        "the pad",
    );
    let s = built_summary(&model, "r0000002");
    near(s.volume, 0.75 * std::f64::consts::PI, 1e-9, "the revolve");
    assert!(s.valid);
}

#[test]
fn a_sketch_on_a_boxs_plus_x_face_follows_minus_z_and_plus_z_each_its_own_way() {
    // The cube's Face2, n = +X, through x = 10: o (10, 0, 0). `-z`: x = (0, 0, −1), y = n ×
    // x = (0, 1, 0); `+z`: x = (0, 0, 1), y = (0, −1, 0). The unit square from (1, 1) lands
    // at (10, v, −u) and (10, −v, u), u and v in 1..2; padded 1 along +X: the boxes (10..11,
    // 1..2, −2..−1) and (10..11, −2..−1, 1..2).
    let features = vec![
        box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0),
        on_face(
            "s0000001",
            &face("b0000001.box.Face2", "-z", false),
            [1.0, 1.0],
            1.0,
            1.0,
        ),
        pad("p0000001", None, "s0000001", &one_side("1")),
        on_face(
            "s0000002",
            &face("b0000001.box.Face2", "+z", false),
            [1.0, 1.0],
            1.0,
            1.0,
        ),
        pad("p0000002", None, "s0000002", &one_side("1")),
    ];
    let model = publish(&engine(), &features);
    for (sketch, pad, x, y, low, high) in [
        (
            "s0000001",
            "p0000001",
            [0.0, 0.0, -1.0],
            [0.0, 1.0, 0.0],
            [10.0, 1.0, -2.0],
            [11.0, 2.0, -1.0],
        ),
        (
            "s0000002",
            "p0000002",
            [0.0, 0.0, 1.0],
            [0.0, -1.0, 0.0],
            [10.0, -2.0, 1.0],
            [11.0, -1.0, 2.0],
        ),
    ] {
        let f = frame_of(&model, sketch);
        assert_eq!((f.origin, f.x, f.y), ([10.0, 0.0, 0.0], x, y), "{sketch}");
        let s = built_summary(&model, pad);
        near(s.volume, 1.0, 1e-9, pad);
        let [l, h] = s.bounds.unwrap();
        for k in 0..3 {
            near(l[k], low[k], 1e-7, &format!("{pad}'s low {k}"));
            near(h[k], high[k], 1e-7, &format!("{pad}'s high {k}"));
        }
    }
}
