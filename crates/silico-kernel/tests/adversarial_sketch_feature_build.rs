//! Break-it on M3.6's steps iv and v on the real kernel (docs/design/m3.6-sketch-features.md,
//! S3 to S7, S12): pads, pockets, revolves, and grooves at the edges of what the gate admits
//! (a nanometre, a kilometre, a turn short of a whole one by the tolerance, a whole turn
//! started half a turn back), onto bases of several solids, into bases they empty, through bases too large to go
//! through, onto coincident faces, and the advice a refused name gives, followed. Each
//! expected value is worked by hand from the sketch and the extent. Sketches are solved as
//! stored, by the fake sketch solver.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../silico-core/tests/common/sweeps.rs"]
mod sweeps;

use std::collections::BTreeSet;
use std::f64::consts::PI;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use silico_core::document::{Checked, read};
use silico_core::fake::FakeSketches;
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};
use sweeps::*;

fn engine() -> Recompute<KernelPool> {
    Recompute::new(
        KernelPool::start_workers(Path::new(env!("CARGO_BIN_EXE_silico-kernel-worker")), 2)
            .unwrap(),
    )
    .with_sketches(FakeSketches::new())
}

fn publish(engine: &Recompute<KernelPool>, features: &[&str]) -> Arc<Model> {
    let document = Checked::new(read(document(features).as_bytes()).unwrap()).unwrap();
    match engine
        .start(document)
        .wait_timeout(Duration::from_secs(120))
    {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("{other:?}"),
        Err(_) => panic!("the recompute took more than two minutes"),
    }
}

fn volume(model: &Model, feature: &str) -> f64 {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { built, .. }) => built.summary().unwrap().volume,
        other => panic!("{feature}: {other:?}"),
    }
}

fn failed(model: &Model, feature: &str) -> Error {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Failed(error)) => error.clone(),
        other => panic!("{feature}: {other:?}"),
    }
}

#[track_caller]
fn assert_close(got: f64, want: f64, relative: f64, what: &str) {
    assert!(
        (got - want).abs() <= relative * want.abs().max(1.0),
        "{what}: {got} against {want}"
    );
}

/// That every element of `feature`'s solid has names no other element has, its first one
/// stable, and none a sketch's own.
#[track_caller]
fn assert_named_apart(model: &Model, feature: &str) {
    let map = model.element_map(&id(feature)).unwrap();
    let mut seen = BTreeSet::new();
    for (element, names) in map.iter() {
        let first = names.first().unwrap();
        assert!(!map.is_unstable(first), "{feature}: {first} is unstable");
        for name in names {
            let text = name.to_string();
            assert!(
                !text.starts_with("ssssssss."),
                "{feature}: {text} is a sketch's"
            );
            assert!(
                seen.insert(text.clone()),
                "{feature}: {text} names two elements, one {element:?}"
            );
        }
    }
}

/// The 10 mm cube from (−1, −1, −10): its top is the XY plane.
fn cube_below() -> String {
    box_at("bbbbbbbb", [-1.0, -1.0, -10.0], 10.0, 10.0, 10.0)
}

/// The rectangle 4 by 2 from the origin on XY.
fn rect() -> String {
    rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0)
}

fn fillet(base: &str, edge: &str) -> String {
    format!(
        r#"{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "{base}", "edges": ["{edge}"], "radius": 0.2}}"#
    )
}

#[test]
fn a_pad_onto_one_of_a_bases_two_separate_solids_isnt_said_to_miss_its_base() {
    // M2's fuse of two boxes apart was two solids, and accepted; a pad from the first box's
    // top, which reaches it, was refused as "pad pppppppp doesn't reach its base f0000001".
    // Found by break-it, fixed by Fable's decision (design S5): D2 holds at M2's booleans
    // too, so the fuse is refused naming itself, and the pad is blocked on it.
    let engine = engine();
    let model = publish(
        &engine,
        &[
            &cube_below(),
            &box_at("b0000002", [100.0, 0.0, -10.0], 10.0, 10.0, 10.0),
            r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "Two", "base": "bbbbbbbb", "tool": "b0000002"}"#,
            &rect(),
            &pad("pppppppp", Some("f0000001"), "ssssssss", TEN),
        ],
    );
    let error = failed(&model, "f0000001");
    assert_eq!(error.code(), silico_core::ErrorCode::KernelFailed);
    assert_eq!(
        error.message(),
        "fuse f0000001 makes 2 solids: its base bbbbbbbb and tool b0000002 don't touch, or meet only along an edge or at a point"
    );
    assert_eq!(
        model.outcome(&id("pppppppp")),
        Some(&FeatureOutcome::Blocked { on: id("f0000001") })
    );
}

#[test]
fn a_pocket_that_removes_its_whole_base_fails_naming_itself() {
    // A through-all pocket whose profile, 20 by 20, covers the whole 10 mm cube's top:
    // nothing is left. Refused, `kernel_failed`, naming the pocket, saying it leaves
    // nothing; and the engine goes on to build the same document mended.
    let engine = engine();
    let all = r#"{"type": "through_all", "reversed": false}"#;
    let wide = rectangle("ssssssss", "xy", [-5.0, -5.0], 20.0, 20.0);
    let model = publish(
        &engine,
        &[
            &cube_below(),
            &wide,
            &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", all),
        ],
    );
    let error = failed(&model, "qqqqqqqq");
    assert_eq!(error.code(), ErrorCode::KernelFailed, "{error:?}");
    assert_eq!(error.feature(), Some("qqqqqqqq"));
    assert!(error.suggestion().is_some(), "{error:?}");
    let model = publish(
        &engine,
        &[
            &cube_below(),
            &rect(),
            &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", all),
        ],
    );
    // 1000 less the rectangle's 8 through the cube's 10.
    assert_close(
        volume(&model, "qqqqqqqq"),
        1000.0 - 80.0,
        1e-9,
        "the pocket",
    );
}

#[test]
fn the_advice_for_a_sketchs_edge_named_whole_resolves_on_a_symmetric_pocket() {
    // S12: a fillet naming `ssssssss.sketch.Edge.na000003` whole is refused. A symmetric
    // pocket 4 deep starts 2 above the cube's top, outside it, so its start edges are cut
    // away with the rest of the prism outside the cube: the rim at z = 0 is a section edge,
    // and `qqqqqqqq.pocket.Edge(M ssssssss.sketch.Edge.na000003)` names nothing.
    // Found by break-it, fixed: the gate advised that `M` name whatever the extent; it now
    // offers it only where the sweep starts at the sketch's plane (one side, through all),
    // and here only the form the pocket's own edges take, naming no edge that may not be.
    let symmetric = r#"{"type": "symmetric", "length": 4}"#;
    let features = |edge: &str| {
        vec![
            cube_below(),
            rect(),
            pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", symmetric),
            fillet("qqqqqqqq", edge),
        ]
    };
    let whole = features("ssssssss.sketch.Edge.na000003");
    let refs: Vec<&str> = whole.iter().map(String::as_str).collect();
    let invalid = checked(&refs).unwrap_err();
    let message = invalid.errors()[0].message().to_owned();
    assert!(!message.contains("Edge(M "), "{message}");
    assert!(message.contains("qqqqqqqq.pocket.Edge(...)"), "{message}");
    // The name it would have offered does name nothing: a broken reference, never another
    // edge.
    let engine = engine();
    let followed = features("qqqqqqqq.pocket.Edge(M ssssssss.sketch.Edge.na000003)");
    let refs: Vec<&str> = followed.iter().map(String::as_str).collect();
    let model = publish(&engine, &refs);
    assert!(
        matches!(
            model.outcome(&id("ffffffff")),
            Some(FeatureOutcome::Broken { .. })
        ),
        "{:?}",
        model.outcome(&id("ffffffff"))
    );
    // One side, the sweep starts at the sketch's plane, the cube's top: the rim is the
    // start edge, the `M` name is offered, and it resolves.
    let one_sided = |edge: &str| {
        vec![
            cube_below(),
            rect(),
            pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", &one_side("3")),
            fillet("qqqqqqqq", edge),
        ]
    };
    let whole = one_sided("ssssssss.sketch.Edge.na000003");
    let refs: Vec<&str> = whole.iter().map(String::as_str).collect();
    let message = checked(&refs).unwrap_err().errors()[0].message().to_owned();
    let advice = "qqqqqqqq.pocket.Edge(M ssssssss.sketch.Edge.na000003)";
    assert!(message.contains(advice), "{message}");
    let followed = one_sided(advice);
    let refs: Vec<&str> = followed.iter().map(String::as_str).collect();
    let model = publish(&engine, &refs);
    assert!(
        matches!(
            model.outcome(&id("ffffffff")),
            Some(FeatureOutcome::Built { .. })
        ),
        "{:?}",
        model.outcome(&id("ffffffff"))
    );
}

#[test]
fn a_revolve_short_of_a_turn_by_the_tolerance_is_refused_and_a_nanodegree_more_is_valid() {
    // The rectangle x 2..4, y 0..2 on XY about the y axis: a whole turn is π(4² − 2²)·2 =
    // 24π. 360° less k nanodegrees leaves a slit of r · k · 1e-9 · π/180 mm at a point r
    // from the axis, narrowest at the near side, 2: within 1e-7 mm for
    // k <= 1e-7 · 180 / (2π · 1e-9) = 9000/π = 2864.79. A nanodegree short (3.5e-11 mm,
    // which OCCT builds as an open ring whose ends are one place), 1433 short (past the
    // band at the far side, 4: 1432.39) and 2864 short are refused before the kernel is
    // asked; 2865 short is a partial turn, valid, its volume 24π(360 − 2.865e-6)/360 to
    // 1e-9, and every element named apart.
    let ring = rectangle("ssssssss", "xy", [2.0, 0.0], 2.0, 2.0);
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
    let engine = engine();
    for degrees in ["359.999999999", "359.999998567", "359.999997136"] {
        let model = publish(&engine, &[&ring, &turn(degrees)]);
        let error = failed(&model, "rrrrrrrr");
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{degrees}");
        assert_eq!(error.parameter(), Some("extent.angle"), "{degrees}");
        assert!(
            error
                .message()
                .ends_with("at its profile's point 2 mm from the axis: within 1e-7 mm, its ends would be one place"),
            "{degrees}: {error}"
        );
    }
    let model = publish(&engine, &[&ring, &turn("359.999997135")]);
    let want = 24.0 * PI * (360.0 - 2.865e-6) / 360.0;
    assert_close(volume(&model, "rrrrrrrr"), want, 1e-9, "the near turn");
    assert_named_apart(&model, "rrrrrrrr");
}

#[test]
fn a_torus_short_of_a_turn_is_measured_at_its_circles_nearest_point() {
    // A circle of radius 2.5 about (2, 7.5) on XY, about the x axis: nearest the axis at
    // its point (2, 5), 7.5 − 2.5 = 5 away, no curve end. Its band is 3600/π = 1145.92
    // nanodegrees short: 1145 short is refused before the kernel is asked; 1146 short
    // builds the open torus, its volume by Pappus the circle's area π · 2.5² times its
    // centre's path, 7.5 · θ, θ = (360 − 1.146e-6)° in radians, to 1e-9.
    let torus = circles("ssssssss", "xy", &[([2.0, 7.5], 2.5)]);
    let turn = |degrees: &str| {
        turned(
            "rrrrrrrr",
            "revolve",
            None,
            "ssssssss",
            "x_axis",
            &angle(degrees),
        )
    };
    let engine = engine();
    let model = publish(&engine, &[&torus, &turn("359.999998855")]);
    let error = failed(&model, "rrrrrrrr");
    assert_eq!(error.parameter(), Some("extent.angle"), "{error}");
    assert!(
        error.message().ends_with(
            "at its profile's point 5 mm from the axis: within 1e-7 mm, its ends would be one place"
        ),
        "{error}"
    );
    let model = publish(&engine, &[&torus, &turn("359.999998854")]);
    let theta = (360.0 - 1.146e-6) * PI / 180.0;
    let want = PI * 2.5 * 2.5 * 7.5 * theta;
    assert_close(volume(&model, "rrrrrrrr"), want, 1e-9, "the open torus");
}

#[test]
fn a_symmetric_whole_turn_is_the_whole_turn_named_as_one_side_names_it() {
    // A symmetric 360° starts half a turn back (−π) and sweeps 2π: the same ring as one
    // side's, 24π, a whole turn by `SweepKind::of_turn`. Its names are the same set as the
    // one-sided turn's: a whole turn has no ends, so where it starts is no name's business.
    let ring = rectangle("ssssssss", "xy", [2.0, 0.0], 2.0, 2.0);
    let names = |extent: &str| {
        let engine = engine();
        let model = publish(
            &engine,
            &[
                &ring,
                &turned("rrrrrrrr", "revolve", None, "ssssssss", "y_axis", extent),
            ],
        );
        assert_close(volume(&model, "rrrrrrrr"), 24.0 * PI, 1e-9, extent);
        assert_named_apart(&model, "rrrrrrrr");
        let map = model.element_map(&id("rrrrrrrr")).unwrap();
        map.iter()
            .map(|(_, names)| names[0].to_string())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(names(r#"{"type": "symmetric", "angle": 360}"#), names(TURN));
}

#[test]
fn a_pad_one_nanometre_long_onto_a_cube_adds_its_sliver() {
    // The shortest pad the gate admits, 1 nm (ten times OCCT's tolerance), onto the cube's
    // top: it adds 4 × 2 × 1e-6 = 8e-6 mm³, above S7's 1e-9 × 1000 = 1e-6, so it isn't a
    // no-op; built, its volume 1000.000008 to the measure's precision.
    let engine = engine();
    let model = publish(
        &engine,
        &[
            &cube_below(),
            &rect(),
            &pad(
                "pppppppp",
                Some("bbbbbbbb"),
                "ssssssss",
                &one_side("0.000001"),
            ),
        ],
    );
    assert_close(
        volume(&model, "pppppppp"),
        1000.000008,
        1e-9,
        "the 1 nm pad",
    );
    assert_named_apart(&model, "pppppppp");
}

#[test]
fn a_kilometre_pad_and_a_kilometre_of_two_sides_are_the_prism_they_say() {
    // The longest the gate admits: one side 1 km, and two sides of 600 m and 400 m (their
    // sum the kilometre): the rectangle's 8 mm² times 1e6 mm.
    for extent in [
        one_side("1000000"),
        r#"{"type": "two_sides", "length": 600000, "length2": 400000}"#.to_owned(),
    ] {
        let engine = engine();
        let model = publish(
            &engine,
            &[&rect(), &pad("pppppppp", None, "ssssssss", &extent)],
        );
        assert_close(volume(&model, "pppppppp"), 8e6, 1e-9, &extent);
    }
}

#[test]
fn a_through_all_pocket_of_a_base_too_large_to_go_through_says_so_naming_the_pocket() {
    // Design S3: through all is 2.02 × the diagonal of the box around the base and the
    // profile, refused past a kilometre. A cube 300 m on a side has a diagonal of 519.6 m,
    // so the prism would be 1.05 km: refused, naming the pocket, with advice.
    let all = r#"{"type": "through_all", "reversed": false}"#;
    let engine = engine();
    let model = publish(
        &engine,
        &[
            &box_at(
                "bbbbbbbb",
                [-1.0, -1.0, -300000.0],
                300000.0,
                300000.0,
                300000.0,
            ),
            &rect(),
            &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", all),
        ],
    );
    let error = failed(&model, "qqqqqqqq");
    assert_eq!(error.feature(), Some("qqqqqqqq"), "{error:?}");
    assert!(error.suggestion().is_some(), "{error:?}");
}

#[test]
fn a_hole_through_a_large_block_is_built_or_refused_in_words_that_are_true() {
    // A 4 by 2 hole through all of a cube 200 m on a side (the prism 700 m, inside the
    // kilometre) removes 8 × 200000 = 1.6e6 mm³: a litre and a half, real material. S7
    // refuses a change of at most 1e-9 × the base's volume (8e15 → 8e6 mm³), so this
    // hole is refused by design (S11 accepts "a pad under 1e-9 of its base", loudly, "the
    // message saying why"). Whatever the rule, the refusal's words must be true: the
    // result's volume isn't the base's (it's 1.6e6 less), and the advice (reverse it, move
    // it, or thicken a thin wall) fits none of it.
    let all = r#"{"type": "through_all", "reversed": false}"#;
    let side = 200000.0f64;
    let engine = engine();
    let model = publish(
        &engine,
        &[
            &box_at("bbbbbbbb", [-1.0, -1.0, -side], side, side, side),
            &rect(),
            &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", all),
        ],
    );
    match model.outcome(&id("qqqqqqqq")) {
        Some(FeatureOutcome::Built { built, .. }) => assert_close(
            built.summary().unwrap().volume,
            side * side * side - 8.0 * side,
            1e-9,
            "the 200 m cube pocketed",
        ),
        Some(FeatureOutcome::Failed(error)) => {
            // Found by break-it, fixed: "pocket qqqqqqqq removes nothing from its base
            // bbbbbbbb: the result's volume is the base's, 7.999999998400001e15 mm³", 1.6e6
            // under the base's, and advice (reverse, move, thicken) that can't help. Now it
            // says the change and the limit it's under, and to make the feature larger or
            // the part smaller.
            assert!(
                !error
                    .message()
                    .contains("the result's volume is the base's"),
                "{} ({:?})",
                error.message(),
                error.suggestion()
            );
            assert!(
                error.message().contains("is no more than the"),
                "{}",
                error.message()
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_revolve_onto_a_pad_of_its_own_sketch_meeting_it_face_to_face_names_every_element_apart() {
    // S2: the pad (z 0..10) and a quarter turn of the same rectangle about its own left
    // side, na000004 (from (0, 2) down to (0, 0): about −y, right-handed, x goes to +z,
    // into the pad, which adds nothing and is refused, S7). Reversed, about +y, x goes to
    // −z: the turn starts in the pad's bottom face and sweeps below it, meeting the pad
    // face to face. The fuse is the pad's 80 and the quarter cylinder's π·4²·2/4 = 8π,
    // and every element is named apart, though both inputs bite the face they share.
    let engine = engine();
    let features = |reversed: bool| {
        vec![
            rect(),
            pad("pppppppp", None, "ssssssss", TEN),
            turned(
                "rrrrrrrr",
                "revolve",
                Some("pppppppp"),
                "ssssssss",
                "na000004",
                &format!(r#"{{"type": "one_side", "angle": 90, "reversed": {reversed}}}"#),
            ),
        ]
    };
    let into = features(false);
    let model = publish(
        &engine,
        &into.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let error = failed(&model, "rrrrrrrr");
    assert!(
        error
            .message()
            .starts_with("revolve rrrrrrrr adds nothing to its base pppppppp"),
        "{}",
        error.message()
    );
    let below = features(true);
    let model = publish(
        &engine,
        &below.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    assert_close(
        volume(&model, "rrrrrrrr"),
        80.0 + 8.0 * PI,
        1e-9,
        "pad and turn",
    );
    assert_named_apart(&model, "rrrrrrrr");
}

#[test]
fn a_revolve_whose_side_is_a_nanometre_from_its_axis_builds_or_fails_naming_itself() {
    // S6: a side exactly 1e-7 from the axis makes OCCT fail, but a document holds whole
    // nanometres, so the nearest a stored point comes is 1 nm (1e-6), ten times the
    // tolerance. The rectangle x 1e-6..4, y 0..2 about the y axis: built, never a panic,
    // and a refusal names the revolve with advice. The engine then builds the same profile
    // moved off the axis.
    let engine = engine();
    let near = polygon(
        "ssssssss",
        "xy",
        &[[1e-6, 0.0], [4.0, 0.0], [4.0, 2.0], [1e-6, 2.0]],
    );
    let model = publish(
        &engine,
        &[
            &near,
            &turned("rrrrrrrr", "revolve", None, "ssssssss", "y_axis", TURN),
        ],
    );
    match model.outcome(&id("rrrrrrrr")) {
        Some(FeatureOutcome::Built { built, .. }) => {
            // π(4² − 1e-12)·2.
            assert_close(built.summary().unwrap().volume, 32.0 * PI, 1e-9, "built");
        }
        Some(FeatureOutcome::Failed(error)) => {
            assert_eq!(error.feature(), Some("rrrrrrrr"), "{error:?}");
            assert!(error.suggestion().is_some(), "{error:?}");
        }
        other => panic!("{other:?}"),
    }
    let off = rectangle("ssssssss", "xy", [1.0, 0.0], 3.0, 2.0);
    let model = publish(
        &engine,
        &[
            &off,
            &turned("rrrrrrrr", "revolve", None, "ssssssss", "y_axis", TURN),
        ],
    );
    assert_close(volume(&model, "rrrrrrrr"), 30.0 * PI, 1e-9, "the ring");
}

#[test]
fn a_revolve_by_one_nanodegree_from_the_axis_is_a_valid_wedge_or_refused() {
    // The smallest angle the gate admits, 1e-9° (1.745e-11 rad, above OCCT's angular
    // precision of 1e-12), on a rectangle from the axis out to 10 m, 1 mm tall: its far
    // side moves 1.745e-7 mm, past S6's 1e-7, so recompute sends it. A wedge of volume
    // θ·h·R²/2 = 1.745e-11 × 1 × 1e8 / 2 = 8.73e-4 mm³, or a refusal naming the revolve;
    // never a solid OCCT calls valid with another volume.
    let wide = rectangle("ssssssss", "xy", [0.0, 0.0], 10000.0, 1.0);
    let engine = engine();
    let model = publish(
        &engine,
        &[
            &wide,
            &turned(
                "rrrrrrrr",
                "revolve",
                None,
                "ssssssss",
                "y_axis",
                &angle("0.000000001"),
            ),
        ],
    );
    let theta = 1e-9 * PI / 180.0;
    match model.outcome(&id("rrrrrrrr")) {
        Some(FeatureOutcome::Built { built, .. }) => assert_close(
            built.summary().unwrap().volume,
            theta * 1e8 / 2.0,
            1e-6,
            "the wedge",
        ),
        Some(FeatureOutcome::Failed(error)) => {
            assert_eq!(error.feature(), Some("rrrrrrrr"), "{error:?}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_pocket_of_three_hundred_and_forty_one_holes_is_cut_in_reasonable_time() {
    // The profile's bound: at most 341 loops (design S9's break-it note). 341 circles of
    // radius 0.3 on a 1 mm grid (19 by 18, less one), pocketed through a 20 by 20 by 2
    // plate from its top: 800 − 341 × π × 0.09 × 2 mm³, within two minutes.
    let mut points = Vec::new();
    let mut curves = Vec::new();
    for k in 0..341 {
        let (i, j) = (k % 19, k / 19);
        let (x, y) = (1.0 + i as f64, 1.0 + j as f64);
        points.push(format!(
            r#"{{"id": "pc{k:06}", "x": {x}, "y": {y}, "construction": false}}"#
        ));
        curves.push(format!(
            r#"{{"id": "ca{k:06}", "center": "pc{k:06}", "radius": 0.3, "construction": false}}"#
        ));
    }
    let sketch = format!(
        r#"{{"id": "ssssssss", "type": "sketch", "v": 1, "name": "Holes", "plane": "xy", "points": [{}], "lines": [], "circles": [{}], "arcs": [], "constraints": []}}"#,
        points.join(", "),
        curves.join(", ")
    );
    let plate = box_at("bbbbbbbb", [0.0, 0.0, -2.0], 20.0, 20.0, 2.0);
    let all = r#"{"type": "through_all", "reversed": false}"#;
    let engine = engine();
    let started = std::time::Instant::now();
    let model = publish(
        &engine,
        &[
            &plate,
            &sketch,
            &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", all),
        ],
    );
    let took = started.elapsed();
    assert_close(
        volume(&model, "qqqqqqqq"),
        800.0 - 341.0 * PI * 0.09 * 2.0,
        1e-9,
        "the perforated plate",
    );
    assert!(took < Duration::from_secs(120), "{took:?}");
    assert_named_apart(&model, "qqqqqqqq");
}

#[test]
fn the_gates_advice_for_a_sketchs_edge_named_whole_resolves_on_a_through_pocket_and_a_revolve() {
    // Council round 1: the advice round trip for the other extents that start at the
    // sketch's plane. Each fillet names the sketch's curve whole, is refused advising the
    // `M` name, and, that name followed, builds. A through-all pocket of the rectangle from
    // the cube's top; a quarter turn, reversed (away from the cube, toward −y), of the
    // rectangle x 2..4, z −4..−2 on XZ, in the cube's front face (y = 0), about the sketch's
    // y axis: its start edges are the rims where it meets that face.
    let all = r#"{"type": "through_all", "reversed": false}"#;
    let front = box_at("bbbbbbbb", [-1.0, 0.0, -10.0], 10.0, 10.0, 10.0);
    let quarter = r#"{"type": "one_side", "angle": 90, "reversed": true}"#;
    let cases: [(Vec<String>, &str, &str); 2] = [
        (
            vec![
                cube_below(),
                rect(),
                pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", all),
            ],
            "qqqqqqqq",
            "pocket",
        ),
        (
            vec![
                front,
                rectangle("ssssssss", "xz", [2.0, -4.0], 2.0, 2.0),
                turned(
                    "qqqqqqqq",
                    "revolve",
                    Some("bbbbbbbb"),
                    "ssssssss",
                    "y_axis",
                    quarter,
                ),
            ],
            "qqqqqqqq",
            "revolve",
        ),
    ];
    let engine = engine();
    for (features, feature, word) in cases {
        let with = |edge: &str| {
            let mut all = features.clone();
            all.push(fillet(feature, edge));
            all
        };
        let whole = with("ssssssss.sketch.Edge.na000003");
        let refs: Vec<&str> = whole.iter().map(String::as_str).collect();
        let message = checked(&refs).unwrap_err().errors()[0].message().to_owned();
        let advice = format!("{feature}.{word}.Edge(M ssssssss.sketch.Edge.na000003)");
        assert!(message.contains(&advice), "{word}: {message}");
        let followed = with(&advice);
        let refs: Vec<&str> = followed.iter().map(String::as_str).collect();
        let model = publish(&engine, &refs);
        assert!(
            matches!(
                model.outcome(&id("ffffffff")),
                Some(FeatureOutcome::Built { .. })
            ),
            "{word}: {:?}",
            model.outcome(&id("ffffffff"))
        );
    }
}
