//! Hostile scenes for `RenderScene::check` and `Camera::check` (`/break-it`,
//! docs/design/m4.6-render.md, The protocol): the check against the design's rules restated
//! here independently, over random scenes biased to their edges, and the numbers whose
//! squares overflow or underflow f64.

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use silico_protocol::{
    Camera, MAX_RENDER_SIDE, Mesh, Projection, RenderBody, RenderScene, field_of_view_range,
};

fn camera() -> Camera {
    Camera {
        projection: Projection::Orthographic { view_height: 10.0 },
        eye: [0.0, -10.0, 0.0],
        target: [0.0; 3],
        up: [0.0, 0.0, 1.0],
    }
}

#[test]
fn an_eye_and_target_whose_distance_overflows_arent_called_the_same_point() {
    // 2e308 apart, the view's squared length would overflow to infinity: refused for being
    // past the farthest a scene's point may be, not as "the same point".
    let mut c = camera();
    c.eye = [1e308, 0.0, 0.0];
    c.target = [-1e308, 0.0, 0.0];
    c.up = [0.0, 0.0, 1.0];
    let e = c.check(64, 48).unwrap_err();
    assert!(
        !e.message.contains("must be apart"),
        "{}: {}",
        e.parameter,
        e.message
    );
}

#[test]
fn an_up_across_the_view_passes_whatever_its_length() {
    // Only an up's direction counts: one of (0, 0, 1e200), whose squared length overflows,
    // or (0, 0, 1e-170), whose squared length underflows, square to the view, passes, with
    // the frame's up the unit z (its length found scaled, so neither is lost).
    for length in [1e200, 1e-170, f64::MAX, f64::MIN_POSITIVE * 4.0] {
        let mut c = camera();
        c.up = [0.0, 0.0, length];
        let frame = c.frame().unwrap();
        assert_eq!(frame.up, [0.0, 0.0, 1.0], "{length}");
        assert_eq!(frame.back, [0.0, -1.0, 0.0], "{length}");
    }
}

#[test]
fn an_up_at_the_least_part_across_passes_and_just_under_it_is_refused() {
    // The rule's edge, by hand: an up of (x, 1, 0) along a view down +y has a part across
    // of |x| / sqrt(1 + x²) of its length.
    for (x, passes) in [
        (2e-9, true),
        (1.000_001e-9, true),
        (0.999_999e-9, false),
        (0.0, false),
    ] {
        let mut c = camera();
        c.up = [x, 1.0, 0.0];
        assert_eq!(c.check(64, 48).is_ok(), passes, "up ({x}, 1, 0)");
    }
}

#[test]
fn heights_at_their_ends_and_angles_past_any_size() {
    // Each bound and the next number past it: from 1e-6 to 1e12 mm (what f32 can draw);
    // an angle of no view at any size, whatever the image.
    let past =
        |x: f64, up: bool| f64::from_bits(if up { x.to_bits() + 1 } else { x.to_bits() - 1 });
    let perspective = |field_of_view| Projection::Perspective { field_of_view };
    let orthographic = |view_height| Projection::Orthographic { view_height };
    for (projection, passes) in [
        (perspective(5e-324), false),
        (perspective(180.0), false),
        (perspective(-0.0), false),
        (perspective(f64::INFINITY), false),
        (perspective(f64::NAN), false),
        (orthographic(1e-6), true),
        (orthographic(past(1e-6, false)), false),
        (orthographic(1e12), true),
        (orthographic(past(1e12, true)), false),
        (orthographic(5e-324), false),
        (orthographic(f64::MAX), false),
        (orthographic(-0.0), false),
        (orthographic(f64::INFINITY), false),
    ] {
        let mut c = camera();
        c.projection = projection;
        for (width, height) in [(1, 1), (64, 48), (4096, 4096)] {
            assert_eq!(
                c.check(width, height).is_ok(),
                passes,
                "{projection:?} at {width} by {height}"
            );
        }
    }
}

/// The ends of a perspective view's angles, by hand: the roots of `height (1 + a²) t² − 2^20
/// t + height = 0` (`a` the aspect, `t` the tangent of half the angle), each `2 atan(t)` in
/// degrees, worked to 20 digits in mpmath (40-digit arithmetic) from the quadratic formula;
/// and those ends as a refusal gives them, rounded inward to six decimals.
const ANGLE_ENDS: [(u32, u32, f64, f64, &str, &str); 5] = [
    (
        1,
        1,
        0.000_109_283_026_720_369_67,
        179.999_781_433_946_57,
        "0.00011",
        "179.999781",
    ),
    (
        64,
        48,
        0.005_245_585_309_439_119,
        179.985_428_929_764_37,
        "0.005246",
        "179.985428",
    ),
    (
        1920,
        1080,
        0.118_026_148_044_260_88,
        179.508_955_771_627_65,
        "0.118027",
        "179.508955",
    ),
    (
        4096,
        4096,
        0.447_634_661_740_786_37,
        179.104_744_337_522_13,
        "0.447635",
        "179.104744",
    ),
    (
        384,
        4096,
        0.447_627_891_064_195_1,
        179.548_437_919_975_4,
        "0.447628",
        "179.548437",
    ),
];

#[test]
fn a_perspective_views_angles_end_where_f32_draws_to_an_eighth_of_a_pixel_at_its_size() {
    for (width, height, low, high, shown_low, shown_high) in ANGLE_ENDS {
        let (narrowest, widest) = field_of_view_range(width, height);
        assert!(
            (narrowest - low).abs() <= 1e-12 * low,
            "{width} by {height}: {narrowest} against {low}"
        );
        assert!(
            (widest - high).abs() <= 1e-12,
            "{width} by {height}: {widest} against {high}"
        );
        // Each end the last angle taken: it passes, and the next double past it is refused.
        let at = |field_of_view: f64| {
            let mut c = camera();
            c.projection = Projection::Perspective { field_of_view };
            c.check(width, height)
        };
        at(narrowest).unwrap();
        at(widest).unwrap();
        for past in [narrowest.next_down(), widest.next_up(), 1e-3, 179.999_999] {
            if (narrowest..=widest).contains(&past) {
                continue;
            }
            let e = at(past).unwrap_err();
            assert_eq!(e.parameter, "camera.field_of_view");
            assert_eq!(
                e.message,
                format!(
                    "camera.field_of_view {past:?} must be from {shown_low} to {shown_high} degrees for an image {width} by {height} pixels, where f32 draws the view to an eighth of a pixel"
                ),
                "{width} by {height}"
            );
        }
        // The ends a refusal gives are taken, as a request sends them back.
        for shown in [shown_low, shown_high] {
            at(shown.parse().unwrap()).unwrap();
        }
    }
}

#[test]
fn the_old_ends_of_a_perspective_views_angle_hold_only_where_the_image_is_small_enough() {
    // 0.001°, M4.6's narrowest: under the narrowest at 64 by 48 (0.0052°) and at 4096 rows
    // (0.4476°), but past the narrowest at 1 by 1 (1.09e-4°). 179.999°, M4.6's widest: its
    // tangent of half is 114591.56, so a square image takes it while h × (1 + 2t²) ≤ 2^20 t,
    // h ≤ 4.575 (by hand): at 4 by 4, not 5 by 5.
    let at = |field_of_view: f64, width, height| {
        let mut c = camera();
        c.projection = Projection::Perspective { field_of_view };
        c.check(width, height).is_ok()
    };
    assert!(at(1e-3, 1, 1));
    assert!(!at(1e-3, 64, 48));
    assert!(!at(1e-3, 384, 4096));
    assert!(at(179.999, 4, 4));
    assert!(!at(179.999, 5, 5));
    assert!(!at(179.999, 4096, 4096));
}

// ---------------------------------------------------------------------------------------
// The check against the design's rules, restated: the first rule broken, in the order the
// design gives (sizes, the camera, then each body), by the parameter it names.

/// The parameter the design's rules refuse `scene` for, if any. Its numbers are kept where
/// no square overflows (the strategies below), so lengths are plain here.
fn rules(scene: &RenderScene) -> Option<&'static str> {
    for (name, side) in [("width", scene.width), ("height", scene.height)] {
        if side == 0 || side > 4096 {
            return Some(name);
        }
    }
    let c = &scene.camera;
    // Within 1e12 mm of the origin (so finite too).
    let within = |p: &[f64; 3]| p.iter().all(|x| x.abs() <= 1e12);
    for (name, v) in [("camera.eye", c.eye), ("camera.target", c.target)] {
        if !within(&v) {
            return Some(name);
        }
    }
    if c.up.iter().any(|x| x.is_nan() || x.is_infinite()) {
        return Some("camera.up");
    }
    // At least a nanometre apart, the distance found scaled so it neither overflows nor
    // underflows (the eye and target are within 1e12, so their difference is finite).
    let d = [
        c.eye[0] - c.target[0],
        c.eye[1] - c.target[1],
        c.eye[2] - c.target[2],
    ];
    let m = d.iter().fold(0.0f64, |m, x| m.max(x.abs()));
    if m == 0.0 || m * d.iter().map(|x| (x / m) * (x / m)).sum::<f64>().sqrt() < 1e-6 {
        return Some("camera.eye");
    }
    // The up's part across the view, against its length: the sine of their angle, from
    // each scaled by its largest component first, so no square overflows or underflows.
    let view = [
        c.target[0] - c.eye[0],
        c.target[1] - c.eye[1],
        c.target[2] - c.eye[2],
    ];
    let scaled = |v: [f64; 3]| {
        let m = v.iter().fold(0.0f64, |m, x| m.max(x.abs()));
        [v[0] / m, v[1] / m, v[2] / m]
    };
    if c.up.iter().all(|x| *x == 0.0) {
        return Some("camera.up");
    }
    let (u, v) = (scaled(c.up), scaled(view));
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    if dot(cross, cross) < 1e-18 * dot(u, u) * dot(v, v) {
        return Some("camera.up");
    }
    match c.projection {
        Projection::Orthographic { view_height } => {
            if !(1e-6..=1e12).contains(&view_height) {
                return Some("camera.view_height");
            }
        }
        Projection::Perspective { field_of_view } => {
            // f32 to 2^-22 of a point's distance from the eye, grown to the image's corner by
            // its ray's sec², c = 1 + t²(1 + a²), at most an eighth of a pixel there, 2t/h of
            // the distance: h c ≤ 2^20 t.
            let t = libm::tan(field_of_view.to_radians() / 2.0);
            let a = f64::from(scene.width) / f64::from(scene.height);
            let c = 1.0 + t * t * (1.0 + a * a);
            let drawn = f64::from(scene.height) * c <= 1_048_576.0 * t;
            if !(field_of_view > 0.0 && field_of_view < 180.0 && drawn) {
                return Some("camera.field_of_view");
            }
        }
    }
    let mut before = 0u32;
    for b in &scene.bodies {
        if b.body == 0 || b.body <= before {
            return Some("bodies");
        }
        before = b.body;
        let m = &b.mesh;
        if m.normals.len() != m.positions.len()
            || !m.positions.iter().all(within)
            || !m
                .normals
                .iter()
                .all(|n| n.iter().all(|x| !x.is_nan() && !x.is_infinite()))
            || m.triangles
                .iter()
                .flatten()
                .any(|&i| i as usize >= m.positions.len())
            || m.face_triangles.iter().map(|&c| c as usize).sum::<usize>() != m.triangles.len()
            || !m.edge_points.iter().all(within)
            || m.edge_point_counts
                .iter()
                .map(|&c| c as usize)
                .sum::<usize>()
                != m.edge_points.len()
        {
            return Some("bodies");
        }
        for (highlights, count) in [
            (&b.highlight_faces, m.face_triangles.len()),
            (&b.highlight_edges, m.edge_point_counts.len()),
        ] {
            if highlights.windows(2).any(|w| w[0] >= w[1])
                || highlights.iter().any(|&h| h as usize >= count)
            {
                return Some("bodies");
            }
        }
    }
    // At most 20,000 faces, 200,000 edges, and 2,000,000 triangles across the bodies.
    let faces: usize = scene
        .bodies
        .iter()
        .map(|b| b.mesh.face_triangles.len())
        .sum();
    let edges: usize = scene
        .bodies
        .iter()
        .map(|b| b.mesh.edge_point_counts.len())
        .sum();
    let triangles: usize = scene.bodies.iter().map(|b| b.mesh.triangles.len()).sum();
    if faces > 20_000 || edges > 200_000 || triangles > 2_000_000 {
        return Some("bodies");
    }
    let points = || {
        scene
            .bodies
            .iter()
            .flat_map(|b| b.mesh.positions.iter().chain(&b.mesh.edge_points))
    };
    let h = f64::from(scene.height);
    match c.projection {
        // An orthographic view at least h × reach / 2^19 high: f32 to 2^-22 of the reach
        // from the target, an eighth of a pixel.
        Projection::Orthographic { view_height } => {
            let reach = points()
                .map(|p| {
                    let d = [p[0] - c.target[0], p[1] - c.target[1], p[2] - c.target[2]];
                    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
                })
                .fold(0.0f64, f64::max);
            if view_height < h * reach / 524_288.0 {
                return Some("camera.view_height");
            }
        }
        // Perspective: any bodies; what lies outside the view's pyramid is cut as it's
        // drawn.
        Projection::Perspective { .. } => {}
    }
    None
}

fn number() -> impl Strategy<Value = f64> {
    prop_oneof![
        6 => -100.0..100.0f64,
        1 => prop::sample::select(vec![
            0.0, -0.0, 1.0, -1.0, 1e-100, -1e-100, 1e100, -1e100, f64::NAN, f64::INFINITY,
            f64::NEG_INFINITY, 1e-7, -1e-7, 1e-6, 1e12, -1e12, 1.000_000_000_001e12,
        ]),
    ]
}

fn point() -> impl Strategy<Value = [f64; 3]> {
    prop_oneof![
        6 => [-50.0..50.0f64, -50.0..50.0f64, -50.0..50.0f64],
        1 => [number(), number(), number()],
    ]
}

/// A camera, its up often along the view (exactly, by a scale of it) or far from it.
fn camera_strategy() -> impl Strategy<Value = Camera> {
    let projection = prop_oneof![
        2 => number().prop_map(|view_height| Projection::Orthographic { view_height }),
        2 => prop_oneof![
            number(),
            prop::sample::select(vec![180.0, 179.999, 1e-300, 5e-324, 360.0])
        ]
        .prop_map(|field_of_view| Projection::Perspective { field_of_view }),
    ];
    let along = prop_oneof![Just(None), (-10.0..10.0f64).prop_map(Some)];
    (
        projection,
        point(),
        point(),
        point(),
        prop::bool::weighted(0.15),
        along,
    )
        .prop_map(|(projection, eye, target, up, same, along)| {
            let target = if same { eye } else { target };
            let up = match along {
                // A whole multiple of the view: exactly along it.
                Some(k) if k.round() != 0.0 => {
                    let k = k.round();
                    [
                        (target[0] - eye[0]) * k,
                        (target[1] - eye[1]) * k,
                        (target[2] - eye[2]) * k,
                    ]
                }
                _ => up,
            };
            Camera {
                projection,
                eye,
                target,
                up,
            }
        })
}

fn mesh_strategy() -> impl Strategy<Value = Mesh> {
    (0usize..6).prop_flat_map(|n| {
        let index = prop_oneof![8 => 0..(n as u32 + 1), 1 => Just(u32::MAX)];
        (
            prop::collection::vec(point(), n),
            prop::collection::vec(
                prop_oneof![
                    8 => Just([0.0f32, 0.0, 1.0]),
                    1 => Just([f32::NAN, 0.0, 0.0]),
                    1 => Just([0.0, f32::NEG_INFINITY, 0.0]),
                ],
                n..=n + 1,
            ),
            prop::collection::vec([index.clone(), index.clone(), index], 0..5),
            prop::collection::vec(prop_oneof![8 => 0u32..4, 1 => Just(u32::MAX)], 0..4),
            prop::collection::vec(point(), 0..6),
            prop::collection::vec(prop_oneof![8 => 0u32..4, 1 => Just(u32::MAX)], 0..4),
        )
            .prop_map(
                |(
                    positions,
                    normals,
                    triangles,
                    face_triangles,
                    edge_points,
                    edge_point_counts,
                )| {
                    Mesh {
                        positions,
                        normals,
                        triangles,
                        face_triangles,
                        edge_points,
                        edge_point_counts,
                        valid: None,
                    }
                },
            )
    })
}

fn highlights() -> impl Strategy<Value = Vec<u32>> {
    prop::collection::vec(prop_oneof![8 => 0u32..5, 1 => Just(u32::MAX)], 0..4)
}

fn scene_strategy() -> impl Strategy<Value = RenderScene> {
    let side = prop_oneof![
        8 => 1u32..=MAX_RENDER_SIDE,
        1 => prop::sample::select(vec![0, 1, MAX_RENDER_SIDE, MAX_RENDER_SIDE + 1, u32::MAX]),
    ];
    let body = (
        prop_oneof![8 => 1u32..6, 1 => prop::sample::select(vec![0, u32::MAX])],
        mesh_strategy(),
        highlights(),
        highlights(),
    )
        .prop_map(
            |(body, mesh, highlight_faces, highlight_edges)| RenderBody {
                body,
                mesh,
                highlight_faces,
                highlight_edges,
            },
        );
    (
        prop::collection::vec(body, 0..4),
        camera_strategy(),
        side.clone(),
        side,
        any::<bool>(),
    )
        .prop_map(|(bodies, camera, width, height, ids)| RenderScene {
            bodies,
            camera,
            width,
            height,
            ids,
        })
}

#[test]
fn the_check_refuses_exactly_what_the_designs_rules_refuse_naming_the_first_broken() {
    let mut runner = TestRunner::new_with_rng(
        Config {
            cases: 20_000,
            failure_persistence: None,
            ..Config::default()
        },
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    );
    let tally = std::cell::RefCell::new((0u32, std::collections::BTreeMap::<&str, u32>::new()));
    runner
        .run(&scene_strategy(), |scene| {
            let got = scene.check().err().map(|e| e.parameter);
            prop_assert_eq!(got, rules(&scene), "{:?}", scene.check());
            match scene.check() {
                Ok(()) => tally.borrow_mut().0 += 1,
                Err(e) => {
                    *tally.borrow_mut().1.entry(e.parameter).or_default() += 1;
                    // Every refusal names its parameter, and a body's says which body.
                    if e.parameter == "bodies" {
                        prop_assert!(
                            e.message.starts_with("bodies[")
                                || e.message.starts_with("the bodies have"),
                            "{}",
                            e.message
                        );
                    } else {
                        prop_assert!(e.message.contains(e.parameter), "{}", e.message);
                    }
                }
            }
            Ok(())
        })
        .unwrap();
    let (passed, by_parameter) = tally.into_inner();
    // Every rule was reached, and some scenes passed them all.
    for parameter in [
        "width",
        "height",
        "camera.eye",
        "camera.target",
        "camera.up",
        "camera.view_height",
        "camera.field_of_view",
        "bodies",
    ] {
        assert!(
            by_parameter.get(parameter).copied().unwrap_or(0) > 20,
            "{parameter}: {by_parameter:?}"
        );
    }
    assert!(passed > 100, "passed {passed}, {by_parameter:?}");
}

// ---------------------------------------------------------------------------------------
// The angles' range at every size (`/break-it` on `field_of_view_range` and the angle's
// check): random sizes up to past any texture, and angles about the ends.

#[test]
fn every_size_has_angles_whose_ends_are_taken_and_whose_outside_is_refused() {
    // For each size, the ends are taken and the doubles past them refused; angles between
    // them, at random, are taken (the check is one quadratic, so none between is refused);
    // and the ends a refusal gives (rounded inward to six decimals) are taken. Sizes up to
    // 32768 a side, past any texture a device has; and sizes of 0 or past u32's 2^32 - 1
    // are refused without a panic.
    let mut runner = TestRunner::new_with_rng(
        Config {
            cases: 3000,
            failure_persistence: None,
            ..Config::default()
        },
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    );
    let side = prop_oneof![
        4 => 1u32..=4096,
        1 => 4097u32..=32768,
        1 => prop::sample::select(vec![1, 2, 3, 4, 5, 48, 1080, 4096, 32768]),
    ];
    runner
        .run(
            &(side.clone(), side, 0.0..1.0f64),
            |(width, height, between)| {
                let (low, high) = field_of_view_range(width, height);
                prop_assert!(0.0 < low && low < high && high < 180.0, "{low} {high}");
                let at = |field_of_view: f64| {
                    let mut c = camera();
                    c.projection = Projection::Perspective { field_of_view };
                    c.check(width, height)
                };
                prop_assert!(at(low).is_ok() && at(high).is_ok(), "{width} x {height}");
                let below = at(low.next_down()).unwrap_err();
                prop_assert_eq!(below.parameter, "camera.field_of_view");
                prop_assert!(at(high.next_up()).is_err());
                let inside = low + (high - low) * between;
                prop_assert!(at(inside).is_ok(), "{inside} at {width} x {height}");
                // The message's ends, sent back as written, are taken.
                let words: Vec<&str> = below.message.split(' ').collect();
                let from = words.iter().position(|w| *w == "from").unwrap();
                for shown in [words[from + 1], words[from + 3]] {
                    let shown: f64 = shown.parse().unwrap();
                    prop_assert!(at(shown).is_ok(), "{shown} at {width} x {height}");
                }
                Ok(())
            },
        )
        .unwrap();
    for (width, height) in [(0, 0), (0, 48), (64, 0), (u32::MAX, 1), (1, u32::MAX)] {
        let mut c = camera();
        c.projection = Projection::Perspective {
            field_of_view: 60.0,
        };
        let _ = c.check(width, height);
        let _ = field_of_view_range(width, height);
    }
}

#[test]
fn a_boxs_farthest_corner_bounds_its_points_reach_from_anywhere() {
    // `/break-it` on the corner bound: random points and targets; the box's farthest
    // corner is at least as far as every point, and one point of the box's corners' own
    // set (the box of a box's corners) is exactly as far.
    use silico_protocol::{points_box, reach_of_box, reach_of_points};
    let mut runner = TestRunner::new_with_rng(
        Config {
            cases: 5000,
            failure_persistence: None,
            ..Config::default()
        },
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    );
    let points = prop::collection::vec([-50.0..50.0f64, -50.0..50.0f64, -50.0..50.0f64], 1..12);
    let target = [-200.0..200.0f64, -200.0..200.0f64, -200.0..200.0f64];
    runner
        .run(&(points, target), |(positions, target)| {
            let mesh = Mesh {
                normals: vec![[0.0, 0.0, 1.0]; positions.len()],
                positions,
                triangles: vec![],
                face_triangles: vec![],
                edge_points: vec![],
                edge_point_counts: vec![],
                valid: None,
            };
            let bounds = points_box(&mesh).unwrap();
            let (by_box, by_points) =
                (reach_of_box(bounds, target), reach_of_points(&mesh, target));
            prop_assert!(by_box >= by_points, "{by_box} < {by_points}");
            let (low, high) = bounds;
            let corners = Mesh {
                positions: (0..8)
                    .map(|i: usize| {
                        std::array::from_fn(|k| if i >> k & 1 == 0 { low[k] } else { high[k] })
                    })
                    .collect(),
                normals: vec![[0.0, 0.0, 1.0]; 8],
                ..mesh.clone()
            };
            prop_assert_eq!(reach_of_points(&corners, target), by_box);
            Ok(())
        })
        .unwrap();
}

#[test]
fn the_frustums_side_holds_its_border_and_not_an_ulp_past_it() {
    // From the origin along +y, up +z (right +x, so a point's x across the view is its x and
    // its depth its y, exactly), 60° over 48 rows, 64 wide: the right side is x = a t × depth,
    // t = tan 30°, a = 4/3. A flat box at depth 1 from x = a t to a t + 1: its corner on the
    // side, the rest past it: crosses; from an ulp farther: out. A box wholly behind the eye:
    // out; one at depth 0, on the eye's plane: out too (the frustum's floor is open).
    use silico_protocol::{Class, ViewRegion};
    let camera = Camera {
        projection: Projection::Perspective {
            field_of_view: 60.0,
        },
        eye: [0.0; 3],
        target: [0.0, 10.0, 0.0],
        up: [0.0, 0.0, 1.0],
    };
    let frame = camera.check(64, 48).unwrap();
    let f = ViewRegion::frustum(&camera, &frame, 64, 48);
    let tx = libm::tan(30f64.to_radians()) * (64.0 / 48.0);
    let flat = |x: f64| ([x, 1.0, -0.1], [x + 1.0, 1.0, 0.1]);
    assert_eq!(f.class_of_box(flat(tx)), Class::Crosses);
    assert_eq!(f.class_of_box(flat(tx.next_up())), Class::Out);
    assert_eq!(
        f.class_of_box(([-1.0, 0.1, -0.01], [1.0, 0.2, 0.01])),
        Class::Crosses
    );
    assert_eq!(
        f.class_of_box(([-0.01, 1.0, -0.01], [0.01, 2.0, 0.01])),
        Class::In
    );
    assert_eq!(
        f.class_of_box(([-1.0, -2.0, -1.0], [1.0, -1.0, 1.0])),
        Class::Out
    );
    assert_eq!(
        f.class_of_box(([-1.0, 0.0, -1.0], [1.0, 0.0, 1.0])),
        Class::Out
    );
    // No points: out.
    assert_eq!(f.class(std::iter::empty()), Class::Out);
}

#[test]
fn a_box_in_or_out_of_a_region_never_contradicts_its_points() {
    // `/break-it` on the classes: random triangles, random cameras; against the frustum and
    // the pyramid, where a triangle's box is in, each of its points is in; where its box is
    // out, each of its points is outside one same side; and its points' own class is as a
    // point-by-point check says.
    use silico_protocol::{Class, ViewRegion, box_of, pyramid_slope};
    let mut runner = TestRunner::new_with_rng(
        Config {
            cases: 3000,
            failure_persistence: None,
            ..Config::default()
        },
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    );
    let point = || [-20.0..20.0f64, -20.0..20.0f64, -20.0..20.0f64];
    runner
        .run(
            &(point(), point(), point(), point(), point(), 1.0..170.0f64),
            |(a, b, c, eye, target, fov)| {
                let camera = Camera {
                    projection: Projection::Perspective { field_of_view: fov },
                    eye,
                    target,
                    up: [0.0, 0.0, 1.0],
                };
                let Ok(frame) = camera.check(64, 48) else {
                    return Ok(());
                };
                let tan = libm::tan(fov.to_radians() / 2.0);
                let regions = [
                    ViewRegion::frustum(&camera, &frame, 64, 48),
                    ViewRegion::pyramid(eye, &frame, pyramid_slope(tan, 48)),
                ];
                let points = [a, b, c];
                let bounds = box_of(points.iter()).unwrap();
                for region in regions {
                    let by_point: Vec<Vec<bool>> = points
                        .iter()
                        .map(|&p| {
                            let e = region.eye_frame(p);
                            region.sides.iter().map(|s| s.holds(e)).collect()
                        })
                        .collect();
                    let all_in = by_point.iter().all(|p| p.iter().all(|&h| h));
                    let out_by_one = (0..5).any(|k| by_point.iter().all(|p| !p[k]));
                    let want = if out_by_one {
                        Class::Out
                    } else if all_in {
                        Class::In
                    } else {
                        Class::Crosses
                    };
                    prop_assert_eq!(region.class(points), want);
                    match region.class_of_box(bounds) {
                        Class::In => prop_assert!(all_in, "box in, a point out"),
                        Class::Out => prop_assert!(out_by_one, "box out, the points not"),
                        Class::Crosses => {}
                    }
                }
                Ok(())
            },
        )
        .unwrap();
}
