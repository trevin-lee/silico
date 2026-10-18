//! M3.6, step iv on the fake kernel (docs/design/m3.6-sketch-features.md, S3, S5 to S7): a
//! pad's, pocket's, revolve's, and groove's steps, the values each sends the kernel (worked
//! out by hand from the sketch's plane and the extent), when they're asked again and when
//! they aren't, what a sketch that doesn't solve or makes no profile does to them, the
//! refusals before any kernel step, the checks of the result, the cache, and a fillet of a
//! pad's edge by name. Sketches are solved as stored, by the fake sketch solver.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/sweeps.rs"]
mod sweeps;

use std::f64::consts::PI;
use std::sync::Arc;
use std::time::Duration;

use silico_core::cache::Cache;
use silico_core::document::{Checked, read};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{Axis, Op};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};
use sweeps::*;

fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new())
}

fn published(engine: &Recompute<FakeKernel>, features: &[&str]) -> Arc<Model> {
    let document = checked_document(features);
    match engine.start(document).wait_timeout(Duration::from_secs(60)) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish"),
    }
}

/// The ops the kernel was asked for that made shapes (every one but the summaries).
fn shape_ops(engine: &Recompute<FakeKernel>) -> Vec<Op> {
    engine
        .kernel()
        .calls()
        .into_iter()
        .map(|call| call.op)
        .filter(|op| op.name() != "Summarize")
        .collect()
}

fn names(engine: &Recompute<FakeKernel>) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = shape_ops(engine).iter().map(Op::name).collect();
    names.sort_unstable();
    names
}

fn built(model: &Model, feature: &str) -> bool {
    matches!(
        model.outcome(&id(feature)),
        Some(FeatureOutcome::Built { .. })
    )
}

fn failed(model: &Model, feature: &str) -> Error {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Failed(error)) => error.clone(),
        other => panic!("{feature}: {other:?}"),
    }
}

/// The 10 mm cube from (−1, −1, −10): its top is the XY plane, and a sketch on it from the
/// origin lies on its top.
fn cube_below() -> String {
    box_at("bbbbbbbb", [-1.0, -1.0, -10.0], 10.0, 10.0, 10.0)
}

/// The rectangle 4 by 2 from the origin on `plane`.
fn sketch(plane: &str) -> String {
    rectangle("ssssssss", plane, [0.0, 0.0], 4.0, 2.0)
}

/// The one sweep `engine` asked for: a prism's start and vector, a through-all prism's
/// direction (as its vector, with no start), or a revolution's axis, start, and angle.
#[derive(Debug, PartialEq)]
enum Sweep {
    Prism([f64; 3], [f64; 3]),
    ThroughAll([f64; 3]),
    Revolution(Axis, f64, f64),
}

fn the_sweep(engine: &Recompute<FakeKernel>) -> Sweep {
    let sweeps: Vec<Sweep> = shape_ops(engine)
        .into_iter()
        .filter_map(|op| match op {
            Op::Prism { start, vector, .. } => Some(Sweep::Prism(start, vector)),
            Op::PrismThroughAll { direction, .. } => Some(Sweep::ThroughAll(direction)),
            Op::Revolution {
                axis, start, angle, ..
            } => Some(Sweep::Revolution(axis, start, angle)),
            _ => None,
        })
        .collect();
    assert_eq!(sweeps.len(), 1, "{sweeps:?}");
    sweeps.into_iter().next().unwrap()
}

/// That every zero in `values` is +0, as every request's are (design S3).
#[track_caller]
fn assert_plus_zeros(values: &[f64]) {
    for v in values {
        assert!(*v != 0.0 || v.is_sign_positive(), "a −0 in {values:?}");
    }
}

/// A pad's or pocket's case: its type, its sketch's plane, its extent, and the start and
/// vector its prism should have.
type Form = (&'static str, &'static str, &'static str, [f64; 3], [f64; 3]);

#[test]
fn each_pad_and_pocket_form_sweeps_from_where_its_extent_says_along_its_planes_normal() {
    // Design S3, worked by hand: a pad goes along the sketch's normal (+Z on XY, −Y on XZ,
    // +X on YZ), a pocket against it, either the other way reversed; one side starts on the
    // plane, symmetric half its length back, two sides length2 back and sweeps their sum.
    let cases: [Form; 12] = [
        (
            "pad",
            "xy",
            r#"{"type": "one_side", "length": 10, "reversed": false}"#,
            [0.0; 3],
            [0.0, 0.0, 10.0],
        ),
        (
            "pad",
            "xy",
            r#"{"type": "one_side", "length": 10, "reversed": true}"#,
            [0.0; 3],
            [0.0, 0.0, -10.0],
        ),
        (
            "pad",
            "xz",
            r#"{"type": "one_side", "length": 10, "reversed": false}"#,
            [0.0; 3],
            [0.0, -10.0, 0.0],
        ),
        (
            "pad",
            "yz",
            r#"{"type": "one_side", "length": 10, "reversed": false}"#,
            [0.0; 3],
            [10.0, 0.0, 0.0],
        ),
        (
            "pad",
            "xy",
            r#"{"type": "symmetric", "length": 10}"#,
            [0.0, 0.0, -5.0],
            [0.0, 0.0, 10.0],
        ),
        (
            "pad",
            "xz",
            r#"{"type": "symmetric", "length": 3}"#,
            [0.0, 1.5, 0.0],
            [0.0, -3.0, 0.0],
        ),
        (
            "pad",
            "xy",
            r#"{"type": "two_sides", "length": 10, "length2": 5}"#,
            [0.0, 0.0, -5.0],
            [0.0, 0.0, 15.0],
        ),
        (
            "pad",
            "yz",
            r#"{"type": "two_sides", "length": 0.000001, "length2": 0.000002}"#,
            [-0.000002, 0.0, 0.0],
            [0.000003, 0.0, 0.0],
        ),
        (
            "pocket",
            "xy",
            r#"{"type": "one_side", "length": 5, "reversed": false}"#,
            [0.0; 3],
            [0.0, 0.0, -5.0],
        ),
        (
            "pocket",
            "xy",
            r#"{"type": "one_side", "length": 5, "reversed": true}"#,
            [0.0; 3],
            [0.0, 0.0, 5.0],
        ),
        (
            "pocket",
            "xy",
            r#"{"type": "symmetric", "length": 4}"#,
            [0.0, 0.0, 2.0],
            [0.0, 0.0, -4.0],
        ),
        (
            "pocket",
            "xy",
            r#"{"type": "two_sides", "length": 6, "length2": 1}"#,
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -7.0],
        ),
    ];
    for (kind, plane, extent, start, vector) in cases {
        let engine = engine();
        let feature = match kind {
            "pad" => pad("pppppppp", None, "ssssssss", extent),
            _ => pocket("pppppppp", "bbbbbbbb", "ssssssss", extent),
        };
        let model = published(&engine, &[&cube_below(), &sketch(plane), &feature]);
        // (A pocket of the sketch on XY cuts the cube under it, whichever way; the cube
        // isn't there to be cut on the other planes, and isn't needed: a pad has no base.)
        let _ = built(&model, "pppppppp");
        let Sweep::Prism(got_start, got_vector) = the_sweep(&engine) else {
            panic!("{kind} {extent}: not a prism");
        };
        assert_eq!(
            (got_start, got_vector),
            (start, vector),
            "{kind} on {plane}, {extent}"
        );
        assert_plus_zeros(&got_start);
        assert_plus_zeros(&got_vector);
    }
}

#[test]
fn a_through_all_pocket_goes_against_the_normal_or_with_it_reversed() {
    for (reversed, direction) in [(false, [0.0, 0.0, -1.0]), (true, [0.0, 0.0, 1.0])] {
        let engine = engine();
        let extent = format!(r#"{{"type": "through_all", "reversed": {reversed}}}"#);
        let model = published(
            &engine,
            &[
                &cube_below(),
                &sketch("xy"),
                &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", &extent),
            ],
        );
        assert_eq!(the_sweep(&engine), Sweep::ThroughAll(direction));
        assert_eq!(
            built(&model, "qqqqqqqq"),
            !reversed,
            "{:?}",
            model.outcome(&id("qqqqqqqq"))
        );
    }
}

#[test]
fn a_revolve_turns_right_handed_about_its_axis_in_the_model() {
    // Design S3: x_axis along the plane's x, y_axis along its y, a line from its `from` to
    // its `to`, placed in the model; reversed turns the other way, by the axis reversed; the
    // angle by FreeCAD's conversion (degrees × π/180), symmetric from minus half of it.
    let radians = |degrees: f64| degrees * (PI / 180.0);
    // The rectangle 4 by 2 from (1, 0) on each plane, and on XY a line from (0, 0) to (0, 5)
    // beside it (construction), whose ID is nb000000.
    let line_sketch = r#"{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000001", "x": 1, "y": 0, "construction": false}, {"id": "pa000002", "x": 5, "y": 0, "construction": false}, {"id": "pa000003", "x": 5, "y": 2, "construction": false}, {"id": "pa000004", "x": 1, "y": 2, "construction": false}, {"id": "pb000001", "x": 0, "y": 0, "construction": false}, {"id": "pb000002", "x": 0, "y": 5, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}, {"id": "nb000000", "from": "pb000001", "to": "pb000002", "construction": true}], "circles": [], "arcs": [], "constraints": []}"#;
    let cases: Vec<(String, &str, &str, Axis, f64, f64)> = vec![
        (
            rectangle("ssssssss", "xz", [1.0, 0.0], 4.0, 2.0),
            "y_axis",
            r#"{"type": "one_side", "angle": 90, "reversed": false}"#,
            Axis {
                origin: [0.0; 3],
                direction: [0.0, 0.0, 1.0],
            },
            0.0,
            radians(90.0),
        ),
        (
            rectangle("ssssssss", "xz", [1.0, 0.0], 4.0, 2.0),
            "y_axis",
            r#"{"type": "one_side", "angle": 90, "reversed": true}"#,
            Axis {
                origin: [0.0; 3],
                direction: [0.0, 0.0, -1.0],
            },
            0.0,
            radians(90.0),
        ),
        (
            rectangle("ssssssss", "yz", [1.0, 0.0], 4.0, 2.0),
            "y_axis",
            r#"{"type": "symmetric", "angle": 45}"#,
            Axis {
                origin: [0.0; 3],
                direction: [0.0, 0.0, 1.0],
            },
            -radians(45.0) / 2.0,
            radians(45.0),
        ),
        (
            rectangle("ssssssss", "xy", [0.0, 1.0], 4.0, 2.0),
            "x_axis",
            r#"{"type": "one_side", "angle": 360, "reversed": false}"#,
            Axis {
                origin: [0.0; 3],
                direction: [1.0, 0.0, 0.0],
            },
            0.0,
            radians(360.0),
        ),
        (
            line_sketch.to_owned(),
            "nb000000",
            r#"{"type": "one_side", "angle": 30, "reversed": false}"#,
            Axis {
                origin: [0.0; 3],
                direction: [0.0, 1.0, 0.0],
            },
            0.0,
            radians(30.0),
        ),
    ];
    for (sketch, axis, extent, want, start, angle) in cases {
        let engine = engine();
        let model = published(
            &engine,
            &[
                &sketch,
                &turned("rrrrrrrr", "revolve", None, "ssssssss", axis, extent),
            ],
        );
        assert!(
            built(&model, "rrrrrrrr"),
            "{axis} {extent}: {:?}",
            model.outcome(&id("rrrrrrrr"))
        );
        let Sweep::Revolution(got, got_start, got_angle) = the_sweep(&engine) else {
            panic!("not a revolution");
        };
        assert_eq!(
            (got, got_start, got_angle),
            (want, start, angle),
            "{axis} {extent}"
        );
        assert_plus_zeros(&got.origin);
        assert_plus_zeros(&got.direction);
    }
}

#[test]
fn an_unchanged_profile_asks_nothing_again_whatever_its_construction_lines_do() {
    // Design S3's keys: the faces key is the profile as sent, not the sketch's IDs, so a
    // construction line toggled out of the profile's way, added, or moved, makes no kernel
    // call; and the IDs renamed with the geometry unchanged make none either, but new names.
    let engine = engine();
    let pad = pad("pppppppp", None, "ssssssss", TEN);
    let plain = rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0);
    let model = published(&engine, &[&plain, &pad]);
    assert!(built(&model, "pppppppp"));
    let before = engine.kernel().calls().len();
    // A construction diagonal added, then moved (its own points), then a construction
    // point dragged: the profile is the rectangle each time.
    let with_line = |x: f64| {
        plain
            .replace(
                r#"], "lines": ["#,
                &format!(r#", {{"id": "pk000001", "x": {x}, "y": 7, "construction": true}}, {{"id": "pk000002", "x": 9, "y": 9, "construction": true}}], "lines": [{{"id": "nk000001", "from": "pk000001", "to": "pk000002", "construction": true}}, "#),
            )
    };
    for edited in [with_line(3.0), with_line(-2.5)] {
        let model = published(&engine, &[&edited, &pad]);
        assert!(built(&model, "pppppppp"));
        assert_eq!(
            engine.kernel().calls().len(),
            before,
            "{edited}: {:#?}",
            &engine.kernel().calls()[before..]
        );
    }
    // The rectangle's IDs renamed, in the same order (a loop is walked from its smallest
    // curve ID, M3.5, so a rename that reorders them sends the same outline from another
    // curve, another request): no kernel call, and the names say the new IDs.
    let renamed = plain
        .replace("na000001", "na000000")
        .replace("pa000001", "pa000000");
    let model = published(&engine, &[&renamed, &pad]);
    assert_eq!(engine.kernel().calls().len(), before);
    let map = model.element_map(&id("pppppppp")).unwrap();
    let all: Vec<String> = map
        .iter()
        .flat_map(|(_, names)| names.iter().map(ToString::to_string))
        .collect();
    assert!(
        all.contains(&"pppppppp.pad.Face(G ssssssss.sketch.Edge.na000000)".to_owned()),
        "{all:#?}"
    );
    assert!(
        all.contains(&"pppppppp.pad.Edge(G ssssssss.sketch.Vertex.pa000000)".to_owned()),
        "{all:#?}"
    );
    assert!(
        !all.iter()
            .any(|n| n.contains("na000001") || n.contains("pa000001")),
        "{all:#?}"
    );
    // A profile moved is another request.
    let moved = rectangle("ssssssss", "xy", [0.5, 0.0], 4.0, 2.0);
    published(&engine, &[&moved, &pad]);
    assert!(engine.kernel().calls().len() > before);
}

#[test]
fn a_sketch_two_pads_sweep_is_one_faces_step() {
    let engine = engine();
    let model = published(
        &engine,
        &[
            &sketch("xy"),
            &pad("pppppppp", None, "ssssssss", TEN),
            &pad("p0000002", None, "ssssssss", &one_side("3")),
        ],
    );
    assert!(built(&model, "pppppppp") && built(&model, "p0000002"));
    assert_eq!(names(&engine), ["MakeFaces", "Prism", "Prism"]);
}

#[test]
fn a_sketch_that_doesnt_solve_blocks_what_sweeps_it() {
    // Design S3: a sketch that doesn't solve fails as M3.4 has it, and each feature using
    // it is blocked on it; nothing is asked of the kernel for them.
    let sketches = FakeSketches::new();
    sketches.answer(|_| {
        Err(Error::new(
            ErrorCode::SolveFailed,
            "the fake couldn't solve it",
        ))
    });
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(sketches);
    let model = published(
        &engine,
        &[
            &cube_below(),
            &sketch("xy"),
            &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN),
            r#"{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "qqqqqqqq", "edges": ["qqqqqqqq.pocket.Edge(M ssssssss.sketch.Edge.na000001)"], "radius": 0.1}"#,
        ],
    );
    assert_eq!(failed(&model, "ssssssss").code(), ErrorCode::SolveFailed);
    for feature in ["qqqqqqqq", "ffffffff"] {
        assert_eq!(
            model.outcome(&id(feature)),
            Some(&FeatureOutcome::Blocked { on: id("ssssssss") }),
            "{feature}"
        );
    }
    assert!(built(&model, "bbbbbbbb"));
    assert_eq!(model.bodies(), [id("bbbbbbbb")]);
    assert!(!names(&engine).contains(&"MakeFaces"));
}

#[test]
fn a_sketch_that_makes_no_profile_fails_each_sweep_of_it_with_every_refusal() {
    // Design S3: the sketch is solved, not failed (a sketch nothing sweeps may be open);
    // each feature sweeping it fails with every refusal M3.5 words, naming the sketch and
    // its elements, and what uses it is blocked. Two open polylines: two open ends each.
    let open = r#"{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 4, "y": 0, "construction": false}, {"id": "pa000003", "x": 4, "y": 2, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#;
    let engine = engine();
    let model = published(
        &engine,
        &[
            open,
            &pad("pppppppp", None, "ssssssss", TEN),
            &pad("p0000002", None, "ssssssss", TEN),
            &box_at("b0000002", [0.0; 3], 1.0, 1.0, 1.0),
            r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "F", "base": "pppppppp", "tool": "b0000002"}"#,
        ],
    );
    assert!(matches!(
        model.outcome(&id("ssssssss")),
        Some(FeatureOutcome::Sketch(_))
    ));
    for feature in ["pppppppp", "p0000002"] {
        let Some(FeatureOutcome::Broken { errors, total }) = model.outcome(&id(feature)) else {
            panic!("{feature}: {:?}", model.outcome(&id(feature)));
        };
        assert_eq!(*total, errors.len() as u64);
        assert!(!errors.is_empty());
        for error in errors {
            assert_eq!(error.code(), ErrorCode::InvalidInput);
            assert_eq!(error.feature(), Some("ssssssss"));
            assert!(!error.elements().is_empty(), "{error:?}");
        }
    }
    assert_eq!(
        model.outcome(&id("f0000001")),
        Some(&FeatureOutcome::Blocked { on: id("pppppppp") })
    );
    assert!(!names(&engine).contains(&"MakeFaces"));
    // Every refusal counted among the model's errors, for each feature.
    assert!(model.error_count() >= 2);
}

#[test]
fn a_pad_or_revolve_of_two_regions_is_refused_naming_them_and_a_pocket_takes_both() {
    // Decision 3, strict (design S5): with or without a base that would join them, refused
    // before any kernel step, the feature the sketch, the elements every region's outer-loop
    // curves in region order, and their total; a pocket or groove takes every region.
    let two = circles("ssssssss", "xy", &[([2.0, 2.0], 1.0), ([6.0, 2.0], 1.0)]);
    for (feature, word) in [
        (pad("pppppppp", None, "ssssssss", TEN), "pad"),
        (pad("pppppppp", Some("bbbbbbbb"), "ssssssss", TEN), "pad"),
        (
            turned("pppppppp", "revolve", None, "ssssssss", "x_axis", TURN),
            "revolve",
        ),
    ] {
        let engine = engine();
        let model = published(&engine, &[&cube_below(), &two, &feature]);
        let error = failed(&model, "pppppppp");
        assert_eq!(error.code(), ErrorCode::InvalidInput);
        assert_eq!(error.feature(), Some("ssssssss"));
        assert_eq!(error.elements(), ["ca000001", "ca000002"]);
        assert_eq!(error.elements_total(), 2);
        assert_eq!(
            error.message(),
            format!(
                "sketch ssssssss makes 2 separate regions (inside loops ca000001 and ca000002), and {word} pppppppp makes one solid of one region"
            )
        );
        assert!(!names(&engine).contains(&"MakeFaces"));
    }
    for feature in [
        pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN),
        turned(
            "qqqqqqqq",
            "groove",
            Some("bbbbbbbb"),
            "ssssssss",
            "x_axis",
            TURN,
        ),
    ] {
        let engine = engine();
        let model = published(&engine, &[&cube_below(), &two, &feature]);
        assert!(
            built(&model, "qqqqqqqq"),
            "{:?}",
            model.outcome(&id("qqqqqqqq"))
        );
        let faces: Vec<Op> = shape_ops(&engine)
            .into_iter()
            .filter(|op| op.name() == "MakeFaces")
            .collect();
        let [Op::MakeFaces(regions)] = faces.as_slice() else {
            panic!("{faces:?}");
        };
        assert_eq!(regions.regions.len(), 2);
    }
}

#[test]
fn a_revolves_axis_is_checked_against_the_solved_profile_before_any_kernel_step() {
    // Design S6: the axis mustn't cross the profile, a point within 1e-7 of it counting as
    // on it: a side along the axis passes, a corner across by 1.01e-7 is refused and by
    // 0.99e-7 passes; an arc whose bulge crosses, its ends not, is refused; and an angle
    // that moves the profile's farthest point by at most 1e-7 is refused. Each refusal with
    // no kernel call. The profile is on XY, turned about the sketch's x axis (y = 0).
    let quad = |corners: &[[f64; 2]]| polygon("ssssssss", "xy", corners);
    let turn = |angle_text: &str| {
        turned(
            "rrrrrrrr",
            "revolve",
            None,
            "ssssssss",
            "x_axis",
            &angle(angle_text),
        )
    };
    let along = quad(&[[0.0, 0.0], [4.0, 0.0], [4.0, 2.0], [0.0, 2.0]]);
    let across = |by: f64| quad(&[[0.0, -by], [4.0, 0.0], [4.0, 2.0], [0.0, 2.0]]);
    for (sketch, refused) in [
        (along.clone(), false),
        (across(0.99e-7), false),
        (across(1.01e-7), true),
    ] {
        let engine = engine();
        let model = published(&engine, &[&sketch, &turn("90")]);
        if refused {
            let error = failed(&model, "rrrrrrrr");
            assert_eq!(error.code(), ErrorCode::InvalidInput);
            assert_eq!(error.feature(), Some("ssssssss"));
            assert!(
                error.message().contains("crosses its profile"),
                "{}",
                error.message()
            );
            assert!(
                error.elements().contains(&"na000001".to_owned()),
                "{error:?}"
            );
            assert_eq!(
                error.suggestion(),
                Some(
                    "move the profile to one side of the axis, or turn it about another line; a profile may touch its axis"
                )
            );
            assert!(!names(&engine).contains(&"MakeFaces"));
        } else {
            assert!(
                built(&model, "rrrrrrrr"),
                "{:?}",
                model.outcome(&id("rrrrrrrr"))
            );
        }
    }
    // The arc counter-clockwise from (0, 0.5) to (4, 0.5) about (2, 0.5), radius 2: its ends
    // are above the axis, its bottom at y = −1.5 below; closed by a line along y = 0.5.
    let arc = r#"{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000001", "x": 4, "y": 0.5, "construction": false}, {"id": "pa000002", "x": 0, "y": 0.5, "construction": false}, {"id": "pc000001", "x": 2, "y": 0.5, "construction": false}], "lines": [{"id": "na000001", "from": "pa000002", "to": "pa000001", "construction": false}], "circles": [], "arcs": [{"id": "aa000001", "center": "pc000001", "start": "pa000002", "end": "pa000001", "construction": false}], "constraints": []}"#;
    let engine = engine();
    let model = published(&engine, &[arc, &turn("90")]);
    let error = failed(&model, "rrrrrrrr");
    assert!(
        error.message().contains("crosses its profile"),
        "{}",
        error.message()
    );
    assert!(
        error.elements().contains(&"aa000001".to_owned()),
        "{error:?}"
    );
    // The same arc the other way up (counter-clockwise from (4, 0.5) to (0, 0.5), y from
    // 0.5 to 2.5): on one side, so it turns.
    let up = arc.replace(
        r#""start": "pa000002", "end": "pa000001""#,
        r#""start": "pa000001", "end": "pa000002""#,
    );
    let model = published(&self::engine(), &[&up, &turn("90")]);
    assert!(
        built(&model, "rrrrrrrr"),
        "{:?}",
        model.outcome(&id("rrrrrrrr"))
    );
    // The angle: the farthest point is 2 from the axis, so 1e-7 mm is 5e-8 radians, about
    // 2.865e-6 degrees: 2e-6 degrees moves it 6.98e-8 mm, refused; 4e-6 moves it 1.4e-7.
    let small = self::engine();
    let model = published(&small, &[&along, &turn("0.000002")]);
    let error = failed(&model, "rrrrrrrr");
    assert_eq!(error.feature(), Some("rrrrrrrr"));
    assert_eq!(error.parameter(), Some("extent.angle"));
    assert!(!names(&small).contains(&"MakeFaces"));
    assert!(built(
        &published(&self::engine(), &[&along, &turn("0.000004")]),
        "rrrrrrrr"
    ));
}

#[test]
fn a_revolves_axis_line_whose_ends_solve_to_one_point_is_refused_with_no_kernel_call() {
    // Design S6: the solve can bring an axis line's ends together, which the gate can't
    // see. Stored apart, (0, 5) and (0, 6); the fake solves the second where the first is
    // (the only point at y = 6). M3.4's solve refuses geometry of no size first, naming the
    // line, so the revolve is blocked on its sketch, and nothing reaches the kernel;
    // `sweep::check_axis_length`, which recompute asks next, is held to the same rule by
    // its own tests (tests/sweep_axis.rs).
    let sketch = r#"{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000001", "x": 1, "y": 0, "construction": false}, {"id": "pa000002", "x": 3, "y": 0, "construction": false}, {"id": "pa000003", "x": 3, "y": 2, "construction": false}, {"id": "pb000001", "x": 0, "y": 5, "construction": false}, {"id": "pb000002", "x": 0, "y": 6, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000001", "construction": false}, {"id": "nb000001", "from": "pb000001", "to": "pb000002", "construction": true}], "circles": [], "arcs": [], "constraints": []}"#;
    let sketches = FakeSketches::new();
    sketches.answer(|system| {
        let mut solved = FakeSketches::as_sent(system);
        let sixes: Vec<usize> = (0..solved.values.len())
            .filter(|&i| solved.values[i] == 6.0)
            .collect();
        assert_eq!(sixes.len(), 1, "{:?}", solved.values);
        solved.values[sixes[0]] = 5.0;
        Ok(solved)
    });
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(sketches);
    let model = published(
        &engine,
        &[
            sketch,
            &turned("rrrrrrrr", "revolve", None, "ssssssss", "nb000001", TURN),
        ],
    );
    let error = failed(&model, "ssssssss");
    assert_eq!(error.elements(), ["nb000001"]);
    assert_eq!(
        model.outcome(&id("rrrrrrrr")),
        Some(&FeatureOutcome::Blocked { on: id("ssssssss") })
    );
    assert_eq!(names(&engine), Vec::<&str>::new());
}

#[test]
fn a_sweep_feature_that_changes_nothing_is_refused_and_blocks_what_uses_it() {
    // Design S7 through recompute: a pocket that misses its base (the sketch on XY, the
    // cube above it, the pocket going down) removes nothing; refused as `kernel_failed`,
    // naming it, and a fillet of it blocked on it. Reversed, it cuts the cube and builds.
    let cube_above = box_at("bbbbbbbb", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0);
    let fillet = r#"{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "qqqqqqqq", "edges": ["bbbbbbbb.box.Edge1"], "radius": 0.1}"#;
    let engine = engine();
    let model = published(
        &engine,
        &[
            &cube_above,
            &sketch("xy"),
            &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN),
            fillet,
        ],
    );
    let error = failed(&model, "qqqqqqqq");
    assert_eq!(error.code(), ErrorCode::KernelFailed);
    assert!(
        error
            .message()
            .starts_with("pocket qqqqqqqq removes nothing from its base bbbbbbbb"),
        "{}",
        error.message()
    );
    assert_eq!(
        model.outcome(&id("ffffffff")),
        Some(&FeatureOutcome::Blocked { on: id("qqqqqqqq") })
    );
    // (The check is the pocket's, decided with its outcome, so the fillet's step may have
    // run; what's reported is blocked: the council's round 1.)
    let reversed = r#"{"type": "one_side", "length": 10, "reversed": true}"#;
    let model = published(
        &engine,
        &[
            &cube_above,
            &sketch("xy"),
            &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", reversed),
            fillet,
        ],
    );
    assert!(built(&model, "qqqqqqqq") && built(&model, "ffffffff"));
}

#[test]
fn a_sweep_features_fingerprint_is_written_and_its_drift_found() {
    // M2.6 for the new features: a pad's solid measured, its fingerprint written by a save,
    // and a stored one that doesn't match is drift. The fake measures a prism of the 4 by 2
    // rectangle 10 long exactly: 80 mm³, its box from the origin to (4, 2, 10).
    let engine = engine();
    let model = published(
        &engine,
        &[&sketch("xy"), &pad("pppppppp", None, "ssssssss", TEN)],
    );
    let fingerprint = model.fingerprint(&id("pppppppp")).unwrap();
    let saved = silico_core::document::write(&model.saved()).unwrap();
    assert!(
        saved.contains(r#""fingerprint": {"volume": 80, "#),
        "{saved}"
    );
    assert!(saved.contains(r#""bbox": [0, 0, 0, 4, 2, 10]"#), "{saved}");
    let _ = fingerprint;
    let drifted = saved.replace(r#""volume": 80"#, r#""volume": 81"#);
    let document = Checked::new(read(drifted.as_bytes()).unwrap()).unwrap();
    let Ok(RunOutcome::Published(model)) = engine.start(document).wait() else {
        panic!("not published");
    };
    let drift = model.drift();
    assert_eq!(drift.len(), 1, "{drift:?}");
    assert_eq!(drift[0].code(), ErrorCode::Drift);
    assert_eq!(drift[0].feature(), Some("pppppppp"));
}

#[test]
fn a_fillet_of_a_pads_edge_by_name_is_resolved_from_the_pads_steps() {
    // MA1 with a sweep feature (design S3): the fillet's step names its base's map from the
    // pad's three steps' results (faces, prism, fuse) and rounds the edge the name finds.
    let engine = engine();
    let fillet = r#"{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "pppppppp", "edges": ["pppppppp.pad.Edge(G ssssssss.sketch.Edge.na000003)"], "radius": 0.5}"#;
    let model = published(
        &engine,
        &[
            &cube_below(),
            &sketch("xy"),
            &pad("pppppppp", Some("bbbbbbbb"), "ssssssss", TEN),
            fillet,
        ],
    );
    assert!(
        built(&model, "ffffffff"),
        "{:?}",
        model.outcome(&id("ffffffff"))
    );
    let fillets: Vec<Op> = shape_ops(&engine)
        .into_iter()
        .filter(|op| op.name() == "Fillet")
        .collect();
    let [Op::Fillet { edges, .. }] = fillets.as_slice() else {
        panic!("{fillets:?}");
    };
    // The fake's fuse keeps the cube's 12 edges first, then the pad's: the prism's start
    // edges 1 to 4, its end edges 5 to 8, so na000003's end edge is the pad's 7th, the
    // fuse's 19th.
    assert_eq!(edges, &[19]);
    // And a name the pad doesn't have breaks, with candidates sharing its sketch's curve.
    let wrong = fillet.replace(
        "Edge(G ssssssss.sketch.Edge.na000003)",
        "Edge(G ssssssss.sketch.Edge.nz000003)",
    );
    let model = published(
        &engine,
        &[
            &cube_below(),
            &sketch("xy"),
            &pad("pppppppp", Some("bbbbbbbb"), "ssssssss", TEN),
            &wrong,
        ],
    );
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("ffffffff")) else {
        panic!("{:?}", model.outcome(&id("ffffffff")));
    };
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
}

#[test]
fn a_cached_sweep_is_taken_from_disk_by_a_new_session_with_no_kernel_call() {
    // M2.5's cache with the new steps: faces (no input keys, a history of one), a through-
    // all prism (two input keys, a history of one, the sweep's rule), and the checked cut,
    // measured as it's made. A second session over the same cache asks the kernel nothing.
    let dir = std::env::temp_dir().join(format!(
        "silico-recompute-sweeps-cache-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let secret = std::env::temp_dir().join(format!(
        "silico-recompute-sweeps-cache-{}.secret",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&secret);
    let session = || {
        Recompute::with_cache(
            FakeKernel::new(2).unwrap(),
            Cache::with_secret(&dir, &secret),
        )
        .with_sketches(FakeSketches::new())
    };
    let features = [
        cube_below(),
        sketch("xy"),
        pocket(
            "qqqqqqqq",
            "bbbbbbbb",
            "ssssssss",
            r#"{"type": "through_all", "reversed": false}"#,
        ),
        turned(
            "rrrrrrrr",
            "groove",
            Some("qqqqqqqq"),
            "ssssssss",
            "x_axis",
            &angle("90"),
        ),
    ];
    let refs: Vec<&str> = features.iter().map(String::as_str).collect();
    let first = session();
    let model = published(&first, &refs);
    assert!(
        built(&model, "qqqqqqqq") && built(&model, "rrrrrrrr"),
        "{:?}",
        model.features()
    );
    let fingerprints = (
        model.fingerprint(&id("qqqqqqqq")),
        model.fingerprint(&id("rrrrrrrr")),
    );
    drop(first);
    let second = session();
    let model = published(&second, &refs);
    assert!(built(&model, "qqqqqqqq") && built(&model, "rrrrrrrr"));
    assert_eq!(
        second.kernel().calls(),
        [],
        "the second session asked the kernel"
    );
    assert_eq!(
        (
            model.fingerprint(&id("qqqqqqqq")),
            model.fingerprint(&id("rrrrrrrr"))
        ),
        fingerprints
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&secret);
}

#[test]
fn a_swept_sketch_is_solved_once_per_recompute() {
    // Council round 1 (proven): the sketch a pad sweeps was solved before the plan, then its
    // solve dropped (kept before the plan's keys were, so trimmed) and solved again beside
    // the steps; its profile never kept. Once a recompute, and an unchanged sketch not again.
    let sketches = FakeSketches::new();
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(sketches.clone());
    let features = [sketch("xy"), pad("pppppppp", None, "ssssssss", TEN)];
    let refs: Vec<&str> = features.iter().map(String::as_str).collect();
    assert!(built(&published(&engine, &refs), "pppppppp"));
    assert_eq!(sketches.requests().len(), 1);
    assert!(built(&published(&engine, &refs), "pppppppp"));
    assert_eq!(
        sketches.requests().len(),
        1,
        "an unchanged sketch was solved again"
    );
}

/// Two rectangles on XZ, x 15..25 and x −25..−15, both z 5..10: either side of the sketch's
/// y axis, each on one side of it.
const TWO_SIDES: &str = r#"{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xz", "points": [{"id": "pa000001", "x": 15, "y": 5, "construction": false}, {"id": "pa000002", "x": 25, "y": 5, "construction": false}, {"id": "pa000003", "x": 25, "y": 10, "construction": false}, {"id": "pa000004", "x": 15, "y": 10, "construction": false}, {"id": "pb000001", "x": -25, "y": 5, "construction": false}, {"id": "pb000002", "x": -15, "y": 5, "construction": false}, {"id": "pb000003", "x": -15, "y": 10, "construction": false}, {"id": "pb000004", "x": -25, "y": 10, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}, {"id": "nb000001", "from": "pb000001", "to": "pb000002", "construction": false}, {"id": "nb000002", "from": "pb000002", "to": "pb000003", "construction": false}, {"id": "nb000003", "from": "pb000003", "to": "pb000004", "construction": false}, {"id": "nb000004", "from": "pb000004", "to": "pb000001", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#;

#[test]
fn a_groove_whose_regions_lie_either_side_of_its_axis_is_built() {
    // Council round 1 (proven against FreeCAD 1.1.3, which checks each face's outer wire
    // alone, FeatureSketchBased.cpp's checkLineCrossesFace per face): no region crosses the
    // axis, so the groove builds; the axis was refused as crossing the profile, the regions
    // taken together. On the fake, the axis passes and the faces are turned (the fake
    // measures a turn by its faces' box, centred here on the axis, so it can't tell the
    // volume removed); the real kernel's volume is in silico-kernel
    // tests/sketch_feature_names.rs.
    let engine = engine();
    let model = published(
        &engine,
        &[
            &circles("cccccccc", "xy", &[([0.0, 0.0], 20.0)]),
            &pad(
                "pppppppp",
                None,
                "cccccccc",
                r#"{"type": "one_side", "length": 20, "reversed": false}"#,
            ),
            TWO_SIDES,
            &turned(
                "gggggggg",
                "groove",
                Some("pppppppp"),
                "ssssssss",
                "y_axis",
                &angle("90"),
            ),
        ],
    );
    if let Some(FeatureOutcome::Failed(error)) = model.outcome(&id("gggggggg")) {
        assert!(!error.message().contains("crosses"), "{error:?}");
    }
    assert!(names(&engine).contains(&"Revolution"));
}

#[test]
fn an_axis_crossing_its_profile_and_an_angle_too_small_are_refused_in_full() {
    // Council round 1: every field of both refusals, worked by hand. The L-shaped region
    // A(−1, 3), B(−1, 2), C(0, 2), D(0, 0), E(2, 0), F(2, 3), its sides na000001 (AB) to
    // na000006 (FA), about a construction line nb000000 up the y axis, x = 0: CD lies along
    // the axis (not listed), the rest reach off it, 1 mm to the left (x = −1) and 2 to the
    // right (x = 2); the line's left of +y is −x, so 2 mm on one side and 1 on the other.
    let sketch = r#"{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000001", "x": -1, "y": 3, "construction": false}, {"id": "pa000002", "x": -1, "y": 2, "construction": false}, {"id": "pa000003", "x": 0, "y": 2, "construction": false}, {"id": "pa000004", "x": 0, "y": 0, "construction": false}, {"id": "pa000005", "x": 2, "y": 0, "construction": false}, {"id": "pa000006", "x": 2, "y": 3, "construction": false}, {"id": "pb000001", "x": 0, "y": -5, "construction": false}, {"id": "pb000002", "x": 0, "y": 5, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000005", "construction": false}, {"id": "na000005", "from": "pa000005", "to": "pa000006", "construction": false}, {"id": "na000006", "from": "pa000006", "to": "pa000001", "construction": false}, {"id": "nb000000", "from": "pb000001", "to": "pb000002", "construction": true}], "circles": [], "arcs": [], "constraints": []}"#;
    let engine = engine();
    let model = published(
        &engine,
        &[
            sketch,
            &turned("rrrrrrrr", "revolve", None, "ssssssss", "nb000000", TURN),
        ],
    );
    let error = failed(&model, "rrrrrrrr");
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(error.feature(), Some("ssssssss"));
    assert_eq!(error.parameter(), None);
    assert_eq!(
        error.message(),
        "revolve rrrrrrrr turns sketch ssssssss about a line that crosses its profile: its axis, line nb000000, has the profile's curves on both sides, 2 mm on one and 1 mm on the other"
    );
    assert_eq!(
        error.elements(),
        [
            "na000001", "na000002", "na000004", "na000005", "na000006", "nb000000"
        ]
    );
    assert_eq!(error.elements_total(), 6);
    assert_eq!(
        error.suggestion(),
        Some(
            "move the profile to one side of the axis, or turn it about another line; a profile may touch its axis"
        )
    );
    // The angle: the rectangle 4 by 2 from the origin about the x axis, its farthest point
    // 2 mm off it, turned 2e-6 degrees: 2 × 2e-6 × π/180 = 7e-8 mm, no more than 1e-7.
    let model = published(
        &engine,
        &[
            &rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0),
            &turned(
                "rrrrrrrr",
                "revolve",
                None,
                "ssssssss",
                "x_axis",
                &angle("0.000002"),
            ),
        ],
    );
    let error = failed(&model, "rrrrrrrr");
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(error.feature(), Some("rrrrrrrr"));
    assert_eq!(error.parameter(), Some("extent.angle"));
    assert_eq!(
        error.message(),
        "revolve rrrrrrrr's angle, 2e-6 degrees, moves its profile's farthest point, 2 mm from the axis, by at most 1e-7 mm: it would make no solid"
    );
    assert!(error.elements().is_empty());
    assert_eq!(error.suggestion(), Some("turn it by a larger angle"));
}

#[test]
fn regions_on_both_sides_of_the_axis_turn_at_most_half_a_turn_in_all() {
    // Council round 2 (proven, critical): two rectangles either side of the axis turned
    // past 180° turn into each other, and OCCT's cut of the overlapping tools was wrong. The
    // rule (the coordinator's, strict): regions wholly on each side may turn at most 180° in
    // all, exactly, in the file's nanodegrees; one side's angle, symmetric's whole angle.
    // Rectangle A at x 15..25, z 5..10; B at x −25..−15, z 7..12; the cylinder r 20 below.
    let base = |engine: &Recompute<FakeKernel>, sketch: &str, extent: &str| {
        published(
            engine,
            &[
                &circles("cccccccc", "xy", &[([0.0, 0.0], 20.0)]),
                &pad("pppppppp", None, "cccccccc", &one_side("20")),
                sketch,
                &turned(
                    "gggggggg",
                    "groove",
                    Some("pppppppp"),
                    "ssssssss",
                    "y_axis",
                    extent,
                ),
            ],
        )
    };
    let both = rectangles(
        "ssssssss",
        "xz",
        &[[15.0, 5.0, 25.0, 10.0], [-25.0, 7.0, -15.0, 12.0]],
    );
    let one_side =
        |text: &str| format!(r#"{{"type": "one_side", "angle": {text}, "reversed": false}}"#);
    let symmetric = |text: &str| format!(r#"{{"type": "symmetric", "angle": {text}}}"#);
    for extent in [one_side("180"), symmetric("180")] {
        let engine = engine();
        let model = base(&engine, &both, &extent);
        if let Some(FeatureOutcome::Failed(e)) = model.outcome(&id("gggggggg")) {
            assert_ne!(e.parameter(), Some("extent.angle"), "{extent}: {e:?}");
        }
        assert!(names(&engine).contains(&"Revolution"), "{extent}");
    }
    for extent in [
        one_side("180.000000001"),
        symmetric("180.000000001"),
        one_side("360"),
    ] {
        let engine = engine();
        let model = base(&engine, &both, &extent);
        let error = failed(&model, "gggggggg");
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{extent}");
        assert_eq!(error.feature(), Some("gggggggg"));
        assert_eq!(error.parameter(), Some("extent.angle"));
        // B is left of the axis (+y's left is −x), found first: its loop, then A's.
        assert_eq!(
            error.elements(),
            [
                "nb000001", "nb000002", "nb000003", "nb000004", "na000001", "na000002", "na000003",
                "na000004"
            ]
        );
        assert_eq!(error.elements_total(), 8);
        assert!(
            error.message().ends_with(
                "about a line with regions on both sides of it (inside loops nb000001, nb000002, nb000003, nb000004 and na000001, na000002, na000003, na000004): past 180° they turn into each other"
            ),
            "{}",
            error.message()
        );
        assert_eq!(
            error.suggestion(),
            Some("turn it at most 180°, or make one groove per side")
        );
        assert!(!names(&engine).contains(&"Revolution"), "{extent}");
    }
    // The rule doesn't ask where the base is: B beyond the cylinder, x −40..−30, still.
    let beyond = rectangles(
        "ssssssss",
        "xz",
        &[[15.0, 5.0, 25.0, 10.0], [-40.0, 7.0, -30.0, 12.0]],
    );
    let model = base(&engine(), &beyond, &one_side("360"));
    assert_eq!(failed(&model, "gggggggg").parameter(), Some("extent.angle"));
    // Regions all on one side turn a whole turn.
    let one = rectangles(
        "ssssssss",
        "xz",
        &[[15.0, 5.0, 25.0, 10.0], [15.0, 12.0, 25.0, 15.0]],
    );
    let engine = engine();
    base(&engine, &one, &one_side("360"));
    assert!(names(&engine).contains(&"Revolution"));
}
