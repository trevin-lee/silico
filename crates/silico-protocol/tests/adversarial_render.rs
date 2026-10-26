//! Hostile scenes for `RenderScene::check` and `Camera::check` (`/break-it`,
//! docs/design/m4.6-render.md, The protocol): the check against the design's rules restated
//! here independently, over random scenes biased to their edges, and the numbers whose
//! squares overflow or underflow f64.

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use silico_protocol::{Camera, MAX_RENDER_SIDE, Mesh, Projection, RenderBody, RenderScene};

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
    let e = c.check().unwrap_err();
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
        assert_eq!(c.check().is_ok(), passes, "up ({x}, 1, 0)");
    }
}

#[test]
fn fields_of_view_and_heights_at_their_ends() {
    // Each bound and the next number past it: from 0.001 to 179.999 degrees, and from 1e-6
    // to 1e12 mm (what f32 can draw).
    let past =
        |x: f64, up: bool| f64::from_bits(if up { x.to_bits() + 1 } else { x.to_bits() - 1 });
    let perspective = |field_of_view| Projection::Perspective { field_of_view };
    let orthographic = |view_height| Projection::Orthographic { view_height };
    for (projection, passes) in [
        (perspective(1e-3), true),
        (perspective(past(1e-3, false)), false),
        (perspective(179.999), true),
        (perspective(past(179.999, true)), false),
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
        assert_eq!(c.check().is_ok(), passes, "{projection:?}");
    }
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
            if !(1e-3..=179.999).contains(&field_of_view) {
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
    // An orthographic view no smaller than 1e-5 of how far the scene reaches from the
    // target.
    if let Projection::Orthographic { view_height } = c.projection {
        let reach = scene
            .bodies
            .iter()
            .flat_map(|b| b.mesh.positions.iter().chain(&b.mesh.edge_points))
            .map(|p| {
                let d = [p[0] - c.target[0], p[1] - c.target[1], p[2] - c.target[2]];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            })
            .fold(0.0f64, f64::max);
        if view_height < reach * 1e-5 {
            return Some("camera.view_height");
        }
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
