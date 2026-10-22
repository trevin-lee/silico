//! M3.6, step iv on the fake kernel (docs/design/m3.6-sketch-features.md, S3, S5 to S7): a
//! pad's, pocket's, revolve's, and groove's steps, the values each sends the kernel (worked
//! out by hand from the sketch's plane and the extent), when they're asked again and when
//! they aren't, what a sketch that doesn't solve or makes no profile does to them, the
//! refusals before any kernel step, the checks of the result, the cache, and a fillet of a
//! pad's edge by name. Sketches are solved as stored, by the fake sketch solver.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/sweeps.rs"]
mod sweeps;

#[path = "common/sketch.rs"]
mod fixture;

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
fn a_sketch_that_makes_no_profile_fails_each_sweep_of_it_once_naming_it() {
    // Design S3 (Fable's decision on AR1): the sketch is solved, not failed (a sketch nothing
    // sweeps may be open), and no error names it; each feature sweeping it fails with one
    // whole error of its own, naming it, its elements the sketch's at fault, the profile's
    // refusals and their advice its cause, and what uses it is blocked. The open polyline
    // (0, 0), (4, 0), (4, 2) has two open ends, pa000001 (of na000001) and pa000003 (of
    // na000002): two refusals, four elements. Two pads: two errors, one per pad.
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
        let error = failed(&model, feature);
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{feature}");
        assert_eq!(error.feature(), Some(feature));
        assert_eq!(error.parameter(), Some("sketch"), "{feature}");
        assert_eq!(
            error.message(),
            format!("pad {feature} sweeps sketch ssssssss, which makes no profile (2 refusals)")
        );
        let advice = "end another curve at the point to close the outline, or make the curve construction geometry";
        assert_eq!(
            error.cause(),
            Some(
                format!(
                    "point pa000001 ends line na000001 and no other profile curve, so the profile is open there ({advice}); point pa000003 ends line na000002 and no other profile curve, so the profile is open there"
                )
                .as_str()
            )
        );
        assert_eq!(
            error.elements(),
            ["na000001", "na000002", "pa000001", "pa000003"]
        );
        assert_eq!(error.elements_total(), 4);
        assert_eq!(
            error.suggestion(),
            Some("fix sketch ssssssss where the cause says, or sweep another sketch")
        );
    }
    assert_eq!(
        model.outcome(&id("f0000001")),
        Some(&FeatureOutcome::Blocked { on: id("pppppppp") })
    );
    assert!(!names(&engine).contains(&"MakeFaces"));
    // One error per pad, each naming it, and nothing else: none names the sketch.
    let features: Vec<Option<&str>> = model.errors().map(Error::feature).collect();
    assert_eq!(features, [Some("pppppppp"), Some("p0000002")]);
    assert_eq!(model.error_count(), 2);
}

#[test]
fn two_pads_of_a_two_region_sketch_are_each_refused_naming_the_pad() {
    // Design S5 in AR1's shape: two circles, two regions; each pad's error names it, with
    // parameter `sketch` and the sketch's two circles; no error names the sketch. (A pocket
    // of them builds: a_pad_or_revolve_of_two_regions_is_refused_naming_them_and_a_pocket_takes_both.)
    let two = circles("ssssssss", "xy", &[([2.0, 2.0], 1.0), ([6.0, 2.0], 1.0)]);
    let model = published(
        &engine(),
        &[
            &two,
            &pad("pppppppp", None, "ssssssss", TEN),
            &pad("p0000002", None, "ssssssss", TEN),
        ],
    );
    let listed: Vec<(Option<&str>, Option<&str>, &[String])> = model
        .errors()
        .map(|e| (e.feature(), e.parameter(), e.elements()))
        .collect();
    let circles_at_fault = ["ca000001".to_owned(), "ca000002".to_owned()];
    assert_eq!(
        listed,
        [
            (Some("pppppppp"), Some("sketch"), &circles_at_fault[..]),
            (Some("p0000002"), Some("sketch"), &circles_at_fault[..]),
        ]
    );
}

#[test]
fn a_pad_or_revolve_of_two_regions_is_refused_naming_them_and_a_pocket_takes_both() {
    // Decision 3, strict (design S5): with or without a base that would join them, refused
    // before any kernel step, naming the feature, the elements every region's outer-loop
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
        assert_eq!(error.feature(), Some("pppppppp"));
        assert_eq!(error.parameter(), Some("sketch"));
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
        assert_eq!(model.errors().count(), 0);
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
            assert_eq!(error.feature(), Some("rrrrrrrr"));
            assert_eq!(error.parameter(), Some("axis"));
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
    // The pocket alone first: its refusal is then known when the fillet is planned, so the
    // fillet's step is never started. (In one recompute, a step's users start as soon as
    // it's built, beside its measure, and the verdict stops only what hasn't started:
    // round 3's council. Fresh, the outcomes are the same.)
    published(
        &engine,
        &[
            &cube_above,
            &sketch("xy"),
            &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN),
        ],
    );
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
    assert!(!names(&engine).contains(&"Fillet"));
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
    // Removed when dropped, even if the test fails.
    let scratch = tempfile::Builder::new()
        .prefix("silico-recompute-sweeps-")
        .tempdir()
        .unwrap();
    let dir = scratch.path().join("cache");
    let secret = scratch.path().join("secret");
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
    assert_eq!(error.feature(), Some("rrrrrrrr"));
    assert_eq!(error.parameter(), Some("axis"));
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

/// A sketch `ssssssss` on XY of the closed polygon through `corners` (pa000001, ... and
/// its sides na000001 from the first corner, ...), and, if given, the construction line
/// nb000000 between two points of its own.
fn polygon_and_line(corners: &[[f64; 2]], line: Option<([f64; 2], [f64; 2])>) -> String {
    let point = |id: String, [x, y]: [f64; 2]| fixture::point(&id, x, y);
    let segment = |id: String, from: String, to: String, construction: bool| {
        fixture::line_as(&id, &from, &to, construction)
    };
    let n = corners.len();
    let mut points: Vec<String> = (0..n)
        .map(|i| point(format!("pa{:06}", i + 1), corners[i]))
        .collect();
    let mut lines: Vec<String> = (0..n)
        .map(|i| {
            let (from, to) = (
                format!("pa{:06}", i + 1),
                format!("pa{:06}", (i + 1) % n + 1),
            );
            segment(format!("na{:06}", i + 1), from, to, false)
        })
        .collect();
    if let Some((from, to)) = line {
        points.push(point("pb000001".to_owned(), from));
        points.push(point("pb000002".to_owned(), to));
        lines.push(segment(
            "nb000000".to_owned(),
            "pb000001".to_owned(),
            "pb000002".to_owned(),
            true,
        ));
    }
    format!(
        r#"{{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{}], "lines": [{}], "circles": [], "arcs": [], "constraints": []}}"#,
        points.join(", "),
        lines.join(", ")
    )
}

#[test]
fn a_crossing_circle_is_refused_with_its_reach_either_side_of_the_axis() {
    // A circle reaches its centre's distance from the axis, plus and minus its radius:
    // centre (0, 1), radius 3, about the x axis, whose left is +y, reaches 1 − 3 = −2 and
    // 1 + 3 = 4, so 2 mm on one side and 4 on the other.
    let model = published(
        &engine(),
        &[
            &circles("ssssssss", "xy", &[([0.0, 1.0], 3.0)]),
            &turned("rrrrrrrr", "revolve", None, "ssssssss", "x_axis", TURN),
        ],
    );
    let error = failed(&model, "rrrrrrrr");
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(error.feature(), Some("rrrrrrrr"));
    assert_eq!(error.parameter(), Some("axis"));
    assert_eq!(
        error.message(),
        "revolve rrrrrrrr turns sketch ssssssss about a line that crosses its profile: its axis, the sketch's x_axis, has the profile's curves on both sides, 2 mm on one and 4 mm on the other"
    );
    assert_eq!(error.elements(), ["ca000001"]);
    assert_eq!(error.elements_total(), 1);
}

#[test]
fn a_region_reaching_exactly_the_tolerance_past_the_axis_touches_it() {
    // The rectangle from y = −1e-7 to y = 1 about the x axis: its lowest point is exactly
    // OCCT's 1e-7 from the axis, so on it, and the region doesn't cross it. Built.
    let model = published(
        &engine(),
        &[
            &polygon(
                "ssssssss",
                "xy",
                &[[0.0, -1e-7], [4.0, -1e-7], [4.0, 1.0], [0.0, 1.0]],
            ),
            &turned("rrrrrrrr", "revolve", None, "ssssssss", "x_axis", TURN),
        ],
    );
    assert!(built(&model, "rrrrrrrr"), "{:?}", model.features());
}

#[test]
fn a_curve_exactly_the_tolerance_from_the_axis_isnt_named_off_it() {
    // A(0, 1), B(0, −1e-7), C(5, −1e-7), D(5, −1), E(6, −1), F(6, 1), about the x axis: the
    // region crosses it (1 mm below at D and E, 1 above at F and A), and every side reaches
    // off it but BC (na000002), which lies exactly 1e-7 below it, on it by the tolerance.
    let model = published(
        &engine(),
        &[
            &polygon(
                "ssssssss",
                "xy",
                &[
                    [0.0, 1.0],
                    [0.0, -1e-7],
                    [5.0, -1e-7],
                    [5.0, -1.0],
                    [6.0, -1.0],
                    [6.0, 1.0],
                ],
            ),
            &turned("rrrrrrrr", "revolve", None, "ssssssss", "x_axis", TURN),
        ],
    );
    let error = failed(&model, "rrrrrrrr");
    assert_eq!(
        error.message(),
        "revolve rrrrrrrr turns sketch ssssssss about a line that crosses its profile: its axis, the sketch's x_axis, has the profile's curves on both sides, 1 mm on one and 1 mm on the other"
    );
    assert_eq!(
        error.elements(),
        ["na000001", "na000003", "na000004", "na000005", "na000006"]
    );
    assert_eq!(error.elements_total(), 5);
}

#[test]
fn a_crossing_with_more_curves_than_are_listed_counts_them_all_and_the_axis() {
    // A region of 101 sides, every one reaching off the x axis: the bottom (0, −1) to
    // (98, −1), the right side up to (98, 1), a zigzag back along y = 1 and y = 2 to (1, 2),
    // and the left side from (0, 1) down. The first 100 curves by ID are listed
    // (PROBLEMS_LISTED), and all 101 counted; about a construction line along the x axis
    // the line is counted too, 102, but not listed, the list being full.
    let mut corners = vec![[0.0, -1.0], [98.0, -1.0], [98.0, 1.0]];
    for k in (1..98).rev() {
        let up = (98 - k) % 2 == 1;
        corners.push([f64::from(k), if up { 2.0 } else { 1.0 }]);
    }
    corners.push([0.0, 1.0]);
    assert_eq!(corners.len(), 101);
    let listed: Vec<String> = (1..=100).map(|i| format!("na{i:06}")).collect();
    for (axis, line, total) in [
        ("x_axis", None, 101),
        ("nb000000", Some(([-5.0, 0.0], [105.0, 0.0])), 102),
    ] {
        let model = published(
            &engine(),
            &[
                &polygon_and_line(&corners, line),
                &turned("rrrrrrrr", "revolve", None, "ssssssss", axis, TURN),
            ],
        );
        let error = failed(&model, "rrrrrrrr");
        assert!(
            error.message().ends_with(
                "has the profile's curves on both sides, 1 mm on one and 2 mm on the other"
            ),
            "{axis}: {error:?}"
        );
        assert_eq!(error.elements(), listed, "{axis}");
        assert_eq!(error.elements_total(), total, "{axis}");
    }
}

#[test]
fn nothing_is_built_on_a_refused_pocket() {
    // Council rounds 2 and 3: three pockets chained on the cube above the XY plane, each of
    // the last, all going down and missing it. Fresh, the first's cut is made and refused,
    // the others blocked, reported alike whatever ran: a step's users start as soon as it's
    // built, beside its measure (round 3: the measure on the critical path doubled a cold
    // chain), so a later cut may start before the first's verdict lands, and none starts
    // after it. Once the verdict is known, at a recompute's start (kept), nothing is
    // started for a refused feature's dependants: here two new pockets on it ask for no
    // cut at all.
    let cube_above = box_at("bbbbbbbb", [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0);
    let chain = |second: &str, third: &str| {
        vec![
            cube_above.clone(),
            sketch("xy"),
            pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN),
            pocket("rrrrrrrr", "qqqqqqqq", "ssssssss", second),
            pocket("tttttttt", "rrrrrrrr", "ssssssss", third),
        ]
    };
    let check = |model: &Model| {
        assert_eq!(failed(model, "qqqqqqqq").code(), ErrorCode::KernelFailed);
        for feature in ["rrrrrrrr", "tttttttt"] {
            assert_eq!(
                model.outcome(&id(feature)),
                Some(&FeatureOutcome::Blocked { on: id("qqqqqqqq") })
            );
        }
    };
    let cuts =
        |engine: &Recompute<FakeKernel>| names(engine).iter().filter(|n| **n == "Cut").count();
    let fresh = engine();
    let all = chain(TEN, TEN);
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    // How many cuts ran fresh depends on when the verdict lands, so it isn't pinned here;
    // shared_step_verdicts.rs's a_verdict_that_lands_mid_recompute_stops_what_hasnt_started
    // pins it with the order held.
    check(&published(&fresh, &refs));
    // The verdict known before anything is planned: the pockets on it, new steps (other
    // lengths), are never started.
    let engine = engine();
    published(&engine, &refs[..3]);
    assert_eq!(cuts(&engine), 1);
    let other = chain(&one_side("3"), &one_side("4"));
    let refs: Vec<&str> = other.iter().map(String::as_str).collect();
    check(&published(&engine, &refs));
    assert_eq!(cuts(&engine), 1);
}

#[test]
fn a_valid_chains_users_dont_wait_for_its_measures() {
    // Council round 3: a feature's measure runs beside its users, not before them. Every
    // measure held, a pad onto a base and a fillet of the pad's top edge: the fillet's step
    // starts (and is held too) while the measures are, so three requests run at once (on
    // four workers: two held measures would fill the usual two).
    let engine = Recompute::new(FakeKernel::new(4).unwrap()).with_sketches(FakeSketches::new());
    let summaries = engine.kernel().hold(|op| op.name() == "Summarize");
    let fillets = engine.kernel().hold(|op| op.name() == "Fillet");
    let fillet = r#"{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "pppppppp", "edges": ["pppppppp.pad.Edge(G ssssssss.sketch.Edge.na000003)"], "radius": 0.5}"#;
    let run = engine.start(checked_document(&[
        &cube_below(),
        &sketch("xy"),
        &pad("pppppppp", Some("bbbbbbbb"), "ssssssss", TEN),
        fillet,
    ]));
    assert!(
        engine.kernel().wait_running(3, Duration::from_secs(30)),
        "the fillet didn't start while the measures were held: {:?}",
        names(&engine)
    );
    assert!(names(&engine).contains(&"Fillet"));
    drop(summaries);
    drop(fillets);
    let Ok(Ok(RunOutcome::Published(model))) = run.wait_timeout(Duration::from_secs(60)) else {
        panic!("not published");
    };
    assert!(built(&model, "pppppppp") && built(&model, "ffffffff"));
}

/// A band test's case: the sketch, an angle refused, where its slit is measured and how
/// wide, an angle that turns, and the curves the refusal names.
type BandCase<'a> = (String, &'a str, f64, f64, &'a str, &'a [&'a str]);

/// The slit a refusal of a turn short of a whole one reports, in mm.
fn slit(message: &str) -> f64 {
    message
        .split("leaves a slit of ")
        .nth(1)
        .and_then(|m| m.split(' ').next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(f64::NAN)
}

#[test]
fn a_turn_short_of_a_whole_one_by_the_tolerance_is_refused_and_a_nanodegree_more_turns() {
    // A turn of 360° less k nanodegrees leaves a slit of r · k · 1e-9 · π/180 mm at a point
    // r from the axis, narrowest at the profile's nearest point off the axis: within 1e-7
    // mm for k ≤ 1e-7 · 180 / (r π · 1e-9). About the x axis, on XY:
    // - the rectangle 4 by 2 from the origin runs out from the axis: its corners on it are
    //   left out, and the nearest off it are 2 away: k ≤ 9000/π = 2864.79, so 2864 short
    //   (359.999997136°, a slit of 9.9973e-8) is refused and 2865 short turns;
    // - the rectangle 4 by 5 from (0, 5) lies 5 to 10 from it: k ≤ 3600/π = 1145.92, so
    //   1145 short (359.999998855°, 9.9920e-8 at 5) is refused and 1146 short turns; 573
    //   short, past the band at its far side (572.96), is refused too (5.0003e-8 at 5);
    // - a circle of radius 2.5 about (2, 7.5) is nearest the axis at (2, 5), 7.5 − 2.5 = 5
    //   away, its point toward it, no curve end: the same band as the rectangle at 5;
    // - an arc of radius 5 about (0, 10), counter-clockwise from (−4, 7) to (4, 7) (from
    //   −143.13° past −90° to −36.87°), closed by its chord: its ends are 7 from the axis,
    //   its bottom, (0, 5), inside it, 5: the band at 5 (at 7 it would be 818.51, and 1145
    //   would turn).
    // For a revolve, and a groove into the cube below.
    let off = rectangle("ssssssss", "xy", [0.0, 5.0], 4.0, 5.0);
    let torus = circles("ssssssss", "xy", &[([2.0, 7.5], 2.5)]);
    let bulge = r#"{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 10, "construction": false}, {"id": "pa000002", "x": -4, "y": 7, "construction": false}, {"id": "pa000003", "x": 4, "y": 7, "construction": false}], "lines": [{"id": "na000001", "from": "pa000003", "to": "pa000002", "construction": false}], "circles": [], "arcs": [{"id": "aa000001", "center": "pa000001", "start": "pa000002", "end": "pa000003", "construction": false}], "constraints": []}"#.to_owned();
    // Arcs about the x axis, each closed by its chord, the arc from its start
    // counter-clockwise to its end:
    // - below the axis, about (0, −10) from (4, −7) to (−4, −7), through 90° to (0, −5): its
    //   nearest point 5 away, toward the axis from below (the band at 5);
    // - wrapping past π, about (0, 10) from (−5, 10) (π) to (4, 7) (5.64 rad, −36.87° a
    //   turn on), through 3π/2 to (0, 5): its nearest point 5 away (the band at 5; its
    //   ends at 10 and 7);
    // - a cap, about (0, 10) from (4, 13) over the top to (−4, 13): its circle's point
    //   toward the axis, (0, 5), isn't on it, so it's nearest at its ends, 13 away: the
    //   band is 1e-7 · 180 / (13π · 1e-9) = 440.76 nanodegrees, so 440 short
    //   (359.99999956°, 13 · 440e-9 · π/180 = 9.9833e-8) is refused, and 1000 short
    //   (359.999999°, 2.2689e-7) turns (at 5 it would be 8.73e-8, and refused).
    let arc = |center: [i32; 2], start: [i32; 2], end: [i32; 2]| {
        format!(
            r#"{{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{{"id": "pa000001", "x": {}, "y": {}, "construction": false}}, {{"id": "pa000002", "x": {}, "y": {}, "construction": false}}, {{"id": "pa000003", "x": {}, "y": {}, "construction": false}}], "lines": [{{"id": "na000001", "from": "pa000003", "to": "pa000002", "construction": false}}], "circles": [], "arcs": [{{"id": "aa000001", "center": "pa000001", "start": "pa000002", "end": "pa000003", "construction": false}}], "constraints": []}}"#,
            center[0], center[1], start[0], start[1], end[0], end[1]
        )
    };
    let below = arc([0, -10], [4, -7], [-4, -7]);
    let wrapping = arc([0, 10], [-5, 10], [4, 7]);
    let cap = arc([0, 10], [4, 13], [-4, 13]);
    // Profiles with no point off the axis, measured at their farthest point:
    // - a circle of radius 5 about (0, 5), tangent to the axis at the origin: no end, and
    //   its point toward the axis on it; its farthest point is (0, 10), whose band is
    //   1800/π = 572.96 nanodegrees, so 572 short (359.999999428°, 10 · 572e-9 · π/180 =
    //   9.9833e-8) is refused and 573 short turns;
    // - a semicircle of radius 5 about the origin on its diameter, the chord on the axis:
    //   its ends on the axis, its circle's centre too; farthest at its top, (0, 5): the band
    //   at 5.
    let tangent = circles("ssssssss", "xy", &[([0.0, 5.0], 5.0)]);
    let semicircle = arc([0, 0], [5, 0], [-5, 0]);
    // The same two mirrored below the axis, their farthest points on its other side: the
    // circle about (0, −5), farthest at (0, −10); the semicircle from (−5, 0) through
    // (0, −5) to (5, 0), farthest at (0, −5).
    let tangent_below = circles("ssssssss", "xy", &[([0.0, -5.0], 5.0)]);
    let semicircle_below = arc([0, 0], [-5, 0], [5, 0]);
    // Each refusal names the sketch's curves where its slit is measured: the circle, the
    // arc at its interior point, the cap's arc and chord at their shared ends, a
    // rectangle's side there and the two sides ending on it; and where nothing is off the
    // axis, the curve reaching the farthest point, the tangent circle, and the semicircle's
    // arc and not its chord.
    let short = "359.999998855";
    let turns = "359.999998854";
    let cases: [BandCase; 12] = [
        (
            tangent_below,
            "359.999999428",
            10.0,
            9.9833e-8,
            "359.999999427",
            &["ca000001"],
        ),
        (
            semicircle_below,
            short,
            5.0,
            9.9920e-8,
            turns,
            &["aa000001"],
        ),
        (
            tangent,
            "359.999999428",
            10.0,
            9.9833e-8,
            "359.999999427",
            &["ca000001"],
        ),
        (semicircle, short, 5.0, 9.9920e-8, turns, &["aa000001"]),
        (torus, short, 5.0, 9.9920e-8, turns, &["ca000001"]),
        (bulge, short, 5.0, 9.9920e-8, turns, &["aa000001"]),
        (below, short, 5.0, 9.9920e-8, turns, &["aa000001"]),
        (wrapping, short, 5.0, 9.9920e-8, turns, &["aa000001"]),
        (
            cap,
            "359.99999956",
            13.0,
            9.9833e-8,
            "359.999999",
            &["aa000001", "na000001"],
        ),
        (
            off.clone(),
            short,
            5.0,
            9.9920e-8,
            turns,
            &["na000001", "na000002", "na000004"],
        ),
        (
            off,
            "359.999999427",
            5.0,
            5.0003e-8,
            turns,
            &["na000001", "na000002", "na000004"],
        ),
        (
            sketch("xy"),
            "359.999997136",
            2.0,
            9.9973e-8,
            "359.999997135",
            &["na000002", "na000003", "na000004"],
        ),
    ];
    for (word, base) in [("revolve", None), ("groove", Some("bbbbbbbb"))] {
        let feature = |degrees: &str| {
            turned(
                "rrrrrrrr",
                word,
                base,
                "ssssssss",
                "x_axis",
                &angle(degrees),
            )
        };
        for (drawn, short, at, width, turns, there) in &cases {
            let engine = engine();
            let model = published(&engine, &[&cube_below(), drawn, &feature(short)]);
            let error = failed(&model, "rrrrrrrr");
            assert_eq!(error.code(), ErrorCode::InvalidInput, "{word} {short}");
            assert_eq!(error.feature(), Some("rrrrrrrr"), "{word}");
            assert_eq!(error.parameter(), Some("extent.angle"), "{word}");
            let message = error.message();
            assert!(
                message.starts_with(&format!(
                    "{word} rrrrrrrr's angle, {short} degrees, short of a whole turn, leaves a slit of "
                )) && message.ends_with(&format!(
                    " mm at its profile's point {at} mm from the axis: within 1e-7 mm, its ends would be one place"
                )),
                "{message}"
            );
            assert!((slit(message) - width).abs() < 1e-12, "{message}");
            assert_eq!(
                error.suggestion(),
                Some(
                    "turn it by 360 degrees for a whole turn, or by less, so its ends are more than 1e-7 mm apart"
                )
            );
            assert_eq!(error.elements(), *there, "{word} {short}");
            assert_eq!(error.elements_total(), there.len() as u64, "{word}");
            assert!(!names(&engine).contains(&"MakeFaces"), "{word}");

            let engine = self::engine();
            let model = published(&engine, &[&cube_below(), drawn, &feature(turns)]);
            assert!(
                built(&model, "rrrrrrrr"),
                "{word} {turns}: {:?}",
                model.outcome(&id("rrrrrrrr"))
            );
            let Sweep::Revolution(_, _, sent) = the_sweep(&engine) else {
                panic!("{word}: not a revolution");
            };
            let turns: f64 = turns.parse().unwrap();
            assert_eq!(sent, turns * (PI / 180.0), "{word}");
        }
    }
}

#[test]
fn a_slit_is_measured_at_the_nearest_point_off_the_axis_over_every_region_and_hole() {
    // About the y axis, on XY. A profile running out from the axis has its corners there
    // left out (their copies meet on the axis); the nearest point off it is where the slit
    // is narrowest, though another region or the outline touches the axis. 360° less k
    // nanodegrees leaves r · k · 1e-9 · π/180 mm at r:
    // - the rectangle 0..10 with a hole 1..2: nearest off the axis 1 (the hole); 5000 short
    //   leaves 8.7266e-8 there, refused (measured at 10, 8.7e-7, it passed);
    // - a groove of the rectangles 0..1 and 5..10: nearest off it 1; 5000 short, refused;
    // - a groove of the rectangles 0..10 and −2..−1 (the robustness review's case): nearest
    //   off it 1, the second's near side; 600 short leaves 1.0472e-8, refused;
    // - the control, the rectangle 0..10 alone: nearest off the axis 10; 5000 short leaves
    //   8.7266e-7, and it turns.
    let cases = [
        (
            "revolve",
            [[0.0, 0.0, 10.0, 5.0], [1.0, 1.0, 2.0, 2.0]],
            "359.999995",
            8.7266e-8,
            ["nb000001", "nb000003", "nb000004"],
        ),
        (
            "groove",
            [[0.0, 0.0, 1.0, 5.0], [5.0, 0.0, 10.0, 5.0]],
            "359.999995",
            8.7266e-8,
            ["na000001", "na000002", "na000003"],
        ),
        (
            "groove",
            [[0.0, 0.0, 10.0, 5.0], [-2.0, 0.0, -1.0, 5.0]],
            "359.9999994",
            1.0472e-8,
            ["nb000001", "nb000002", "nb000003"],
        ),
    ];
    // Each names the curves 1 mm off the axis: the near side, and the two sides ending on
    // it.
    for (word, rects, short, width, there) in cases {
        let base = (word == "groove").then_some("bbbbbbbb");
        let model = published(
            &engine(),
            &[
                &cube_below(),
                &rectangles("ssssssss", "xy", &rects),
                &turned("rrrrrrrr", word, base, "ssssssss", "y_axis", &angle(short)),
            ],
        );
        let error = failed(&model, "rrrrrrrr");
        assert_eq!(error.parameter(), Some("extent.angle"), "{rects:?}");
        let message = error.message();
        assert!(
            message.ends_with(
                " mm at its profile's point 1 mm from the axis: within 1e-7 mm, its ends would be one place"
            ),
            "{message}"
        );
        assert!((slit(message) - width).abs() < 1e-12, "{message}");
        assert_eq!(error.elements(), there, "{rects:?}");
        assert_eq!(error.elements_total(), 3, "{rects:?}");
    }
    let model = published(
        &engine(),
        &[
            &rectangles("ssssssss", "xy", &[[0.0, 0.0, 10.0, 5.0]]),
            &turned(
                "rrrrrrrr",
                "revolve",
                None,
                "ssssssss",
                "y_axis",
                &angle("359.999995"),
            ),
        ],
    );
    assert!(
        built(&model, "rrrrrrrr"),
        "{:?}",
        model.outcome(&id("rrrrrrrr"))
    );
}

#[test]
fn a_slit_refusal_lists_a_hundred_of_its_curves_and_counts_them_all() {
    // A comb: a zigzag along the bottom from (0, 5) to (240, 5), its corners at x = 0, 1,
    // ..., 240 alternately at y = 5 and 6, closed by (240, 10) and (0, 10). About the x
    // axis its nearest points are the 121 corners at y = 5, ends of every zigzag side
    // (240) and of the two sides up to y = 10: 242 curves at 5, all but the top, n0000242.
    // 1145 nanodegrees short is refused there; the refusal lists the first hundred by ID,
    // n0000001 to n0000100, and counts 242.
    let mut points: Vec<String> = (0..=240)
        .map(|i| {
            let y = if i % 2 == 0 { 5 } else { 6 };
            format!(
                r#"{{"id": "p{:07}", "x": {i}, "y": {y}, "construction": false}}"#,
                i + 1
            )
        })
        .collect();
    points.push(r#"{"id": "p0000242", "x": 240, "y": 10, "construction": false}"#.to_owned());
    points.push(r#"{"id": "p0000243", "x": 0, "y": 10, "construction": false}"#.to_owned());
    let lines: Vec<String> = (1..=243)
        .map(|k| {
            format!(
                r#"{{"id": "n{k:07}", "from": "p{k:07}", "to": "p{:07}", "construction": false}}"#,
                k % 243 + 1
            )
        })
        .collect();
    let comb = format!(
        r#"{{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{}], "lines": [{}], "circles": [], "arcs": [], "constraints": []}}"#,
        points.join(", "),
        lines.join(", ")
    );
    let model = published(
        &engine(),
        &[
            &comb,
            &turned(
                "rrrrrrrr",
                "revolve",
                None,
                "ssssssss",
                "x_axis",
                &angle("359.999998855"),
            ),
        ],
    );
    let error = failed(&model, "rrrrrrrr");
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert_eq!(error.parameter(), Some("extent.angle"), "{error}");
    assert!(
        error.message().ends_with(
            " mm at its profile's point 5 mm from the axis: within 1e-7 mm, its ends would be one place"
        ),
        "{error}"
    );
    let listed: Vec<String> = (1..=100).map(|k| format!("n{k:07}")).collect();
    assert_eq!(error.elements(), listed);
    assert_eq!(error.elements_total(), 242);
}

#[test]
fn a_half_disc_right_of_the_y_axis_names_its_arc_where_its_farthest_point_is() {
    // An arc of radius 5 about the origin from (0, −5) counter-clockwise through (5, 0) to
    // (0, 5), its chord on the y axis, turned about y: its ends and its circle's centre
    // are on the axis, so its slit is measured at its farthest point, (5, 0), on the
    // axis's negative side (right of y is negative to it). The band at 5 is 3600/π =
    // 1145.92 nanodegrees: 1145 short leaves 9.9920e-8 and is refused, naming the arc
    // alone, and 1146 short turns.
    let half = r#"{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 0, "y": -5, "construction": false}, {"id": "pa000003", "x": 0, "y": 5, "construction": false}], "lines": [{"id": "na000001", "from": "pa000003", "to": "pa000002", "construction": false}], "circles": [], "arcs": [{"id": "aa000001", "center": "pa000001", "start": "pa000002", "end": "pa000003", "construction": false}], "constraints": []}"#;
    let turn = |degrees: &str| {
        turned(
            "rrrrrrrr",
            "revolve",
            None,
            "ssssssss",
            "y_axis",
            &angle(degrees),
        )
    };
    let model = published(&engine(), &[half, &turn("359.999998855")]);
    let error = failed(&model, "rrrrrrrr");
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert!(
        error.message().ends_with(
            " mm at its profile's point 5 mm from the axis: within 1e-7 mm, its ends would be one place"
        ),
        "{error}"
    );
    assert!((slit(error.message()) - 9.9920e-8).abs() < 1e-12, "{error}");
    assert_eq!(error.elements(), ["aa000001"]);
    assert_eq!(error.elements_total(), 1);
    let model = published(&engine(), &[half, &turn("359.999998854")]);
    assert!(
        built(&model, "rrrrrrrr"),
        "{:?}",
        model.outcome(&id("rrrrrrrr"))
    );
}

#[test]
fn a_slit_refusal_names_every_curve_whose_farthest_point_is_where_it_is_measured() {
    // A groove of two regions about the x axis: a semicircle of radius 5 on its diameter
    // on the axis (no point off the axis but its arc, farthest at its top, (0, 5)), and a
    // rectangle 10..14 × 5..10. The slit is measured at the rectangle's nearest corners,
    // 5 away: 1145 nanodegrees short is refused there. Named: the curves with a point
    // nearest 5 (the rectangle's bottom, nb000001, and its two sides ending on it,
    // nb000002 and nb000004), and the curves whose farthest point is at 5, the
    // semicircle's arc, aa000001: its top lies at the same distance, its slit as narrow.
    let both = r#"{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 5, "y": 0, "construction": false}, {"id": "pa000003", "x": -5, "y": 0, "construction": false}, {"id": "pb000001", "x": 10, "y": 5, "construction": false}, {"id": "pb000002", "x": 14, "y": 5, "construction": false}, {"id": "pb000003", "x": 14, "y": 10, "construction": false}, {"id": "pb000004", "x": 10, "y": 10, "construction": false}], "lines": [{"id": "na000001", "from": "pa000003", "to": "pa000002", "construction": false}, {"id": "nb000001", "from": "pb000001", "to": "pb000002", "construction": false}, {"id": "nb000002", "from": "pb000002", "to": "pb000003", "construction": false}, {"id": "nb000003", "from": "pb000003", "to": "pb000004", "construction": false}, {"id": "nb000004", "from": "pb000004", "to": "pb000001", "construction": false}], "circles": [], "arcs": [{"id": "aa000001", "center": "pa000001", "start": "pa000002", "end": "pa000003", "construction": false}], "constraints": []}"#;
    let model = published(
        &engine(),
        &[
            &cube_below(),
            both,
            &turned(
                "rrrrrrrr",
                "groove",
                Some("bbbbbbbb"),
                "ssssssss",
                "x_axis",
                &angle("359.999998855"),
            ),
        ],
    );
    let error = failed(&model, "rrrrrrrr");
    assert!(
        error.message().ends_with(
            " mm at its profile's point 5 mm from the axis: within 1e-7 mm, its ends would be one place"
        ),
        "{error}"
    );
    assert_eq!(
        error.elements(),
        ["aa000001", "nb000001", "nb000002", "nb000004"]
    );
    assert_eq!(error.elements_total(), 4);
}
