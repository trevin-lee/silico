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
        near(f.normal[k], n[k], 1e-12, "the frame's normal");
    }
    // Built, 8 mm³; or, if Silico decides a frame's origin must be within reach too, a
    // refusal of the sketch that says so as the document has it, never a faces request
    // refused on `plane` as a bug in the profile.
    match model.outcome(&id("p0000002")) {
        Some(FeatureOutcome::Built { built, .. }) => {
            let s = built.summary().unwrap();
            near(s.volume, 8.0, 1e-6, "the pad's volume");
            assert!(s.valid);
        }
        other => {
            let errors: Vec<_> = model.errors().collect();
            assert!(
                errors.iter().any(|e| e.feature() == Some("s0000002")
                    && !e.suggestion().unwrap_or("").contains("bug")),
                "p0000002: {other:?}"
            );
        }
    }
}

#[test]
fn two_alike_sketches_on_one_slanted_face_refused_alike_are_each_refused_in_their_own_words() {
    // Two sketches drawn alike on the slanted side of the previous test share one faces
    // step (its key has no ID): a refusal of it, kept by its key, must name each pad as
    // its own, never the first pad for both.
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
    for padded in ["p0000002", "p0000003"] {
        match model.outcome(&id(padded)) {
            Some(FeatureOutcome::Built { .. }) => {}
            Some(FeatureOutcome::Failed(e)) => {
                assert_eq!(e.feature(), Some(padded), "{e:?}");
            }
            Some(FeatureOutcome::Blocked { on }) => {
                assert_ne!(on.as_str(), "p0000002", "{padded} blocked on the other pad");
            }
            other => panic!("{padded}: {other:?}"),
        }
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
    // The model holds no frame for a sketch it refused: the read refuses it as check does.
    assert!(model.frame(&id("s0000001")).is_none());
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
        near(f.normal[k], n[k], 1e-15, "normal");
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
fn a_revolve_about_an_axis_placed_past_a_kilometre_is_refused_naming_the_revolve_and_its_axis() {
    // A cube from z = 999990 to the kilometre, its top front edge chamfered 1: the bevel's
    // o = (0, −499999.5, 499999.5), x = +X, y = (0, 1, 1)/√2. The bevel's points have v =
    // 1e6/√2 ≈ 707106.78. A square there, revolved 1° about a construction line at v = 1e6
    // (within a kilometre, as drawn), whose placed point is o + 1e6·y = (0, 207106.78,
    // 1207106.78): past a kilometre in z. Either the revolve builds, or it's refused as
    // `invalid_input` on the revolve naming its axis: never a bug, a crash, or the sketch.
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
    let f = frame_of(&model, "s0000001");
    near(f.origin[2], 499999.5, 1e-6, "the bevel's origin");
    // BREAKS: `beyond_reach` checks the profile's points placed on the frame, so that "no
    // later request is refused naming a value the document never wrote", but not the
    // revolve's axis: the revolution request is refused by the protocol's check, on
    // `axis`, naming the placed point [0, 207107.28, 1207106.28], which the document
    // doesn't hold, with the advice "give the revolution a valid axis". The refusal should
    // say which of the sketch's lines it is (nb000001) and where it lands, as the profile's
    // refusal does.
    match model.outcome(&id("r0000001")) {
        Some(FeatureOutcome::Built { built, .. }) => {
            assert!(built.summary().unwrap().valid);
        }
        Some(FeatureOutcome::Failed(e)) => {
            assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
            assert_eq!(e.feature(), Some("r0000001"), "{e:?}");
            assert_eq!(e.parameter(), Some("axis"), "{e:?}");
            assert!(
                e.message().contains("nb000001") || e.elements().iter().any(|n| n == "nb000001"),
                "the refusal doesn't name the axis line: {e:?}"
            );
        }
        other => panic!("{other:?}"),
    }
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
            (f.origin, f.x, f.y, f.normal),
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
    near(
        built_summary(&short, "p0000003").volume,
        1024.0,
        1e-9,
        "short",
    );
    let long = publish(&engine, &chain(3, "5"));
    assert_eq!(
        frames(&long),
        [[0.0, 0.0, 10.0], [0.0, 0.0, 15.0], [0.0, 0.0, 16.0]]
    );
    let s = built_summary(&long, "p0000003");
    near(s.volume, 1056.0, 1e-9, "long");
    near(s.bounds.unwrap()[1][2], 17.0, 1e-7, "long's top");
    let back = publish(&engine, &chain(3, "1"));
    assert_eq!(frames(&back), frames(&short));
    near(
        built_summary(&back, "p0000003").volume,
        1024.0,
        1e-9,
        "back",
    );
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
    near(
        built_summary(&first, "q0000001").volume,
        976.0,
        1e-9,
        "pocket",
    );
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
    near(built_summary(&model, "p0000002").volume, 977.0, 1e-9, "pad");
}

#[test]
fn a_sketch_on_a_boxs_top_after_a_fillet_of_a_vertical_edge_is_found_in_the_fillet_or_broken_with_candidates()
 {
    // The fillet rounds a vertical edge, which trims the top face's outline but not its
    // plane. The top is either found in the fillet's solid (z = 10) or reference_broken on
    // plane.face with candidates: never blocked, a bug, or re-pointed to another plane.
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
    match model.outcome(&id("s0000001")) {
        Some(FeatureOutcome::Sketch(_)) => {
            assert_eq!(frame_of(&model, "s0000001").origin, [0.0, 0.0, 10.0]);
            let filleted = built_summary(&model, "f0000001").volume;
            near(
                built_summary(&model, "p0000001").volume,
                filleted + 24.0,
                1e-6,
                "pad",
            );
        }
        Some(FeatureOutcome::Broken { errors, .. }) => {
            assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
            assert_eq!(errors[0].parameter(), Some("plane.face"));
            assert!(!errors[0].candidates().is_empty(), "{errors:?}");
        }
        other => panic!("{other:?}"),
    }
}
