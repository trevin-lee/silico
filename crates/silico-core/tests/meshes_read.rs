//! `command::meshes` (docs/design/m5.0-app.md, `meshes`, M5.5) on the fake kernel: the
//! levels a view asks, the meshes it shares with other views and with `render` (by the
//! fake's record of what it was asked to mesh), each face and edge named by the element
//! map, and the refusals. Expected values are by hand, from the example: the cut m3tq8v0z,
//! the box (0, 0, −1) to (40, 20, 11), 9 faces and 15 edges on the fake (a triangle a face,
//! two points an edge), its floor 1e-4 of its diagonal, √2144: 0.0046303.

use std::sync::Arc;

use super::example::EXAMPLE;
use silico_core::command::meshes::{Held, Meshes, MeshesOn, MeshesRead, meshes};
use silico_core::command::render::{Render, RenderOn, render};
use silico_core::command::{self, Problems};
use silico_core::document::FeatureId;
use silico_core::fake::{FakeKernel, FakeRenderer};
use silico_core::kernel::{ElementKind, ElementRef, Op};
use silico_core::meshes::{MESH_CACHE_BYTES, MeshCache};
use silico_core::recompute::{FeatureOutcome, Key, Model, Recompute};
use silico_core::views::NamedView;
use silico_protocol::{Camera, Projection};

struct Bench {
    kernel: Arc<FakeKernel>,
    model: Arc<Model>,
    meshes: MeshCache<Key>,
    renderer: FakeRenderer,
}

impl Bench {
    fn of(text: &str) -> Bench {
        let kernel = Arc::new(FakeKernel::new(2).unwrap());
        let engine = Recompute::new(Arc::clone(&kernel));
        let model = command::rebuild(&engine, command::gate(text.as_bytes()).unwrap())
            .unwrap()
            .unwrap();
        Bench {
            kernel,
            model,
            meshes: MeshCache::new(MESH_CACHE_BYTES),
            renderer: FakeRenderer::new(),
        }
    }

    fn read(&self, request: &Meshes) -> Result<MeshesRead, Problems> {
        meshes(
            &self.model,
            request,
            &MeshesOn {
                kernel: &self.kernel,
                meshes: &self.meshes,
            },
        )
    }

    fn render(&self, camera: Camera, width: u32, height: u32) {
        render(
            &self.model,
            &Render {
                view: None,
                camera: Some(camera),
                width,
                height,
                highlight: vec![],
                to: None,
                replace: false,
            },
            &RenderOn {
                kernel: &self.kernel,
                meshes: &self.meshes,
                renderer: &self.renderer,
                workspace: None,
                held: &[],
                settle: &|| Ok(()),
            },
        )
        .unwrap();
    }

    /// The deflections the kernel has been asked to mesh at, in turn.
    fn meshed(&self) -> Vec<f64> {
        self.kernel
            .calls()
            .iter()
            .filter_map(|c| match &c.op {
                Op::Tessellate {
                    deflection,
                    at_least_the_floor: true,
                    ..
                } => Some(*deflection),
                _ => None,
            })
            .collect()
    }

    fn key(&self, feature: &str) -> Key {
        match self.model.outcome(&id(feature)) {
            Some(FeatureOutcome::Built { key, .. }) => *key,
            other => panic!("{other:?}"),
        }
    }
}

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

/// Orthographic, from −y onto the body's centre, `view_height` high.
fn front(view_height: f64) -> Camera {
    Camera {
        projection: Projection::Orthographic { view_height },
        eye: [20.0, -100.0, 5.0],
        target: [20.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    }
}

fn of(camera: Camera, width: u32, height: u32) -> Meshes {
    Meshes {
        view: None,
        camera: Some(camera),
        width,
        height,
        features: None,
        held: vec![],
    }
}

#[test]
fn every_body_is_meshed_at_its_views_level_each_face_and_edge_named_by_the_element_map() {
    // 80 mm over 1,000 pixels: a half pixel of 0.04, so 2⁻⁵.
    let bench = Bench::of(EXAMPLE);
    let read = bench.read(&of(front(80.0), 1600, 1000)).unwrap();
    assert_eq!(read.camera.camera(), front(80.0));
    assert_eq!(read.bodies.len(), 1);
    let body = &read.bodies[0];
    assert_eq!(body.feature, id("m3tq8v0z"));
    assert_eq!(body.body, Some(1));
    assert_eq!(body.key, bench.key("m3tq8v0z"));
    assert_eq!((body.level, body.deflection), (0.03125, 0.03125));
    assert_eq!(body.serves, [0.03125, 0.0625]);
    assert_eq!(body.bounds, Some([[0.0, 0.0, -1.0], [40.0, 20.0, 11.0]]));
    assert!(!body.held);
    let mesh = body.mesh.as_ref().unwrap();
    assert_eq!(mesh.face_triangles.len(), 9);
    assert_eq!(mesh.edge_point_counts.len(), 15);
    // Each face and edge in the mesh's order, by the map's own name and stability.
    let map = bench.model.element_map(&id("m3tq8v0z")).unwrap();
    for (kind, named, count) in [
        (ElementKind::Face, &body.faces, 1),
        (ElementKind::Edge, &body.edges, 2),
    ] {
        assert_eq!(named.len(), if kind == ElementKind::Face { 9 } else { 15 });
        // The map numbers them from 1.
        for (index, element) in (1u32..).zip(named.iter()) {
            let name = map.name(ElementRef { kind, index }).unwrap();
            assert_eq!(element.name, name);
            assert_eq!(element.stable, !map.is_unstable(&name));
            assert_eq!(element.count, Some(count));
        }
    }
    assert_eq!(bench.meshed(), [0.03125]);
}

#[test]
fn views_of_one_level_share_a_mesh_and_views_a_power_of_two_apart_dont() {
    // Half pixels of 0.04 and 0.05 are both 2⁻⁵: one mesh. 0.08 is 2⁻⁴: a second.
    let bench = Bench::of(EXAMPLE);
    let first = bench.read(&of(front(80.0), 1000, 1000)).unwrap();
    let second = bench.read(&of(front(100.0), 1000, 1000)).unwrap();
    assert_eq!(bench.meshed(), [0.03125]);
    assert!(Arc::ptr_eq(
        first.bodies[0].mesh.as_ref().unwrap(),
        second.bodies[0].mesh.as_ref().unwrap()
    ));
    let coarser = bench.read(&of(front(160.0), 1000, 1000)).unwrap();
    assert_eq!(bench.meshed(), [0.03125, 0.0625]);
    assert_eq!(coarser.bodies[0].serves, [0.0625, 0.125]);
}

#[test]
fn a_body_rendered_and_read_at_one_view_is_meshed_once_whichever_comes_first() {
    let bench = Bench::of(EXAMPLE);
    bench.render(front(80.0), 1000, 1000);
    bench.read(&of(front(80.0), 1000, 1000)).unwrap();
    assert_eq!(bench.meshed(), [0.03125]);
    let bench = Bench::of(EXAMPLE);
    bench.read(&of(front(80.0), 1000, 1000)).unwrap();
    bench.render(front(80.0), 1000, 1000);
    assert_eq!(bench.meshed(), [0.03125]);
}

#[test]
fn a_body_meshed_at_its_floor_serves_every_half_pixel_whose_level_is_under_it() {
    // 1 mm over 1,000 pixels asks 2⁻¹¹ = 0.000488, under the floor, 0.0046303: meshed at
    // the floor, which serves every half pixel up to 2 · 2⁻⁸ (the floor's power of two,
    // 0.00390625). A view 0.5 mm high asks 2⁻¹², and the floor's mesh is already kept.
    let bench = Bench::of(EXAMPLE);
    let read = bench.read(&of(front(1.0), 1000, 1000)).unwrap();
    let body = &read.bodies[0];
    assert_eq!(body.level, 1.0 / 2048.0);
    assert!(
        (body.deflection - 0.0046303).abs() < 1e-7,
        "{}",
        body.deflection
    );
    assert_eq!(body.serves, [0.0, 0.0078125]);
    let again = bench.read(&of(front(0.5), 1000, 1000)).unwrap();
    assert_eq!(again.bodies[0].level, 1.0 / 4096.0);
    assert_eq!(again.bodies[0].deflection, body.deflection);
    assert_eq!(again.bodies[0].serves, [0.0, 0.0078125]);
    assert_eq!(bench.meshed(), [1.0 / 2048.0]);
}

#[test]
fn features_named_are_read_in_timeline_order_a_consumed_solid_numbered_none() {
    // The box k7f2qx3m is the cut's base, consumed, so no body.
    let bench = Bench::of(EXAMPLE);
    let read = bench
        .read(&Meshes {
            features: Some(vec![id("m3tq8v0z"), id("k7f2qx3m")]),
            ..of(front(80.0), 1000, 1000)
        })
        .unwrap();
    let got: Vec<(String, Option<u32>)> = read
        .bodies
        .iter()
        .map(|b| (b.feature.to_string(), b.body))
        .collect();
    assert_eq!(
        got,
        [
            ("k7f2qx3m".to_owned(), None),
            ("m3tq8v0z".to_owned(), Some(1))
        ]
    );
    // Its own box, the block's: (0, 0, 0) to (40, 20, 10).
    assert_eq!(
        read.bodies[0].bounds,
        Some([[0.0, 0.0, 0.0], [40.0, 20.0, 10.0]])
    );
    // A box's faces on the fake: 6, named as the box's map names them.
    assert_eq!(read.bodies[0].faces.len(), 6);
    assert_eq!(
        read.bodies[0].faces[0].name.to_string(),
        "k7f2qx3m.box.Face1"
    );
    assert!(read.bodies[0].faces.iter().all(|f| f.stable));
}

#[test]
fn a_consumed_solid_in_a_perspective_view_is_meshed_for_its_own_nearest_depth() {
    // From (20, −100, 5) along +y, 40° high over 1,000 pixels: the block's near face at
    // y = 0 is 100 deep, a half pixel of 100 · tan 20° / 1000 = 0.036397, so 2⁻⁵.
    let bench = Bench::of(EXAMPLE);
    let camera = Camera {
        projection: Projection::Perspective {
            field_of_view: 40.0,
        },
        ..front(80.0)
    };
    let read = bench
        .read(&Meshes {
            features: Some(vec![id("k7f2qx3m")]),
            ..of(camera, 1000, 1000)
        })
        .unwrap();
    assert_eq!(read.bodies[0].body, None);
    assert_eq!(read.bodies[0].level, 0.03125);
}

#[test]
fn a_feature_unknown_or_named_twice_is_refused_naming_its_place_and_nothing_is_meshed() {
    let bench = Bench::of(EXAMPLE);
    let problems = bench
        .read(&Meshes {
            features: Some(vec![id("zzzzzzzz"), id("m3tq8v0z"), id("m3tq8v0z")]),
            ..of(front(80.0), 1000, 1000)
        })
        .unwrap_err();
    let said: Vec<(&str, Option<&str>)> = problems
        .errors()
        .iter()
        .map(|e| (e.message(), e.parameter()))
        .collect();
    assert_eq!(
        said,
        [
            ("the document has no feature zzzzzzzz", Some("features.0")),
            ("feature m3tq8v0z is named twice", Some("features.2")),
        ]
    );
    assert!(bench.meshed().is_empty());
}

#[test]
fn a_view_refused_as_render_refuses_it_with_a_feature_refused_too() {
    let bench = Bench::of(EXAMPLE);
    let problems = bench
        .read(&Meshes {
            view: None,
            camera: None,
            width: 0,
            height: 1000,
            features: Some(vec![id("zzzzzzzz")]),
            held: vec![],
        })
        .unwrap_err();
    let parameters: Vec<Option<&str>> = problems.errors().iter().map(|e| e.parameter()).collect();
    assert_eq!(
        parameters,
        [Some("view"), Some("width"), Some("features.0")]
    );
}

#[test]
fn a_mesh_the_client_holds_is_named_but_not_given() {
    // The names come either way: a key covers the geometry, not the names.
    let bench = Bench::of(EXAMPLE);
    let read = bench
        .read(&Meshes {
            held: vec![Held {
                key: bench.key("m3tq8v0z"),
                deflection: 0.03125,
            }],
            ..of(front(80.0), 1000, 1000)
        })
        .unwrap();
    let body = &read.bodies[0];
    assert!(body.held);
    assert!(body.mesh.is_none());
    // Named, without their counts, which came with the geometry.
    assert_eq!((body.faces.len(), body.edges.len()), (9, 15));
    assert!(
        body.faces
            .iter()
            .chain(&body.edges)
            .all(|e| e.count.is_none())
    );
    // A fresh cache can't know the level isn't under the floor, so it meshes once to learn
    // it (#331's format review): the answer is the same, held. Read again, it knows, and the
    // kernel isn't asked.
    assert_eq!(bench.meshed(), [0.03125]);
    let again = bench
        .read(&Meshes {
            held: vec![Held {
                key: bench.key("m3tq8v0z"),
                deflection: 0.03125,
            }],
            ..of(front(80.0), 1000, 1000)
        })
        .unwrap();
    assert!(again.bodies[0].held && again.bodies[0].mesh.is_none());
    assert_eq!(bench.meshed(), [0.03125]);
    // Held at another deflection, it's given.
    let read = bench
        .read(&Meshes {
            held: vec![Held {
                key: bench.key("m3tq8v0z"),
                deflection: 0.0625,
            }],
            ..of(front(80.0), 1000, 1000)
        })
        .unwrap();
    assert!(!read.bodies[0].held && read.bodies[0].mesh.is_some());
}

#[test]
fn a_named_view_fits_every_body_as_renders_does() {
    // front on the box: render's fit, so the same camera answered.
    let bench = Bench::of(EXAMPLE);
    let read = bench
        .read(&Meshes {
            view: NamedView::named("front"),
            camera: None,
            ..of(front(80.0), 800, 600)
        })
        .unwrap();
    let rendered = render(
        &bench.model,
        &Render {
            view: NamedView::named("front"),
            camera: None,
            width: 800,
            height: 600,
            highlight: vec![],
            to: None,
            replace: false,
        },
        &RenderOn {
            kernel: &bench.kernel,
            meshes: &bench.meshes,
            renderer: &bench.renderer,
            workspace: None,
            held: &[],
            settle: &|| Ok(()),
        },
    )
    .unwrap();
    assert_eq!(read.camera, rendered.result.camera);
    assert_eq!(bench.meshed().len(), 1);
}

#[test]
fn a_body_wholly_behind_the_eye_is_meshed_for_its_distance_as_one_in_front_at_that_depth() {
    // The eye at the origin looking along +y, 40° high, over 1,000 pixels: at depth d a half
    // pixel is d · tan 20° / 1000. A 2 mm cube 100 mm in front, its near face at y = 100
    // (depth 100), and one behind, its near face at y = −100 (100 mm from the eye, the eye
    // within its x and z): each 100 · 0.36397 / 1000 = 0.036397, so 2⁻⁵. One 1,000 mm
    // behind: 0.36397, so 2⁻² (0.25), coarser.
    let cube = |id: &str, y: f64| {
        format!(
            r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Cube", "position": [-1, {y}, -1], "length": 2, "width": 2, "height": 2}}"#
        )
    };
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}, {}, {}]}}"#,
        cube("f0000001", 100.0),
        cube("b0000001", -102.0),
        cube("b0000002", -1002.0)
    );
    let bench = Bench::of(&text);
    let camera = Camera {
        projection: Projection::Perspective {
            field_of_view: 40.0,
        },
        eye: [0.0, 0.0, 0.0],
        target: [0.0, 10.0, 0.0],
        up: [0.0, 0.0, 1.0],
    };
    let read = bench.read(&of(camera, 1000, 1000)).unwrap();
    let levels: Vec<(String, f64)> = read
        .bodies
        .iter()
        .map(|b| (b.feature.to_string(), b.level))
        .collect();
    assert_eq!(
        levels,
        [
            ("f0000001".to_owned(), 0.03125),
            ("b0000001".to_owned(), 0.03125),
            ("b0000002".to_owned(), 0.25),
        ]
    );
    // Each meshed at its level: above its floor (2√3 · 1e-4).
    assert!(read.bodies.iter().all(|b| b.deflection == b.level));
}

#[test]
fn a_held_view_reads_again_with_no_mesh_made_though_the_cache_keeps_none() {
    // The architecture review's case: a cache that keeps nothing (bound 0), as one past its
    // bound by other sessions' meshes is. The first read meshes; three more, the mesh held,
    // ask the kernel nothing.
    let bench = Bench {
        meshes: MeshCache::new(0),
        ..Bench::of(EXAMPLE)
    };
    let first = bench.read(&of(front(80.0), 1000, 1000)).unwrap();
    let body = &first.bodies[0];
    let held = Meshes {
        held: vec![Held {
            key: body.key,
            deflection: body.deflection,
        }],
        ..of(front(80.0), 1000, 1000)
    };
    for _ in 0..3 {
        let again = bench.read(&held).unwrap();
        assert!(again.bodies[0].held && again.bodies[0].mesh.is_none());
        assert_eq!(again.bodies[0].serves, body.serves);
    }
    assert_eq!(bench.meshed(), [0.03125]);
}

#[test]
fn a_view_held_at_the_floor_reads_again_with_no_mesh_made_though_the_cache_keeps_none() {
    // The architecture review's floor case: 0.001 mm over 1,000 pixels asks 2⁻²¹, under
    // the floor (0.0046303), with a cache that keeps nothing, so no floor is known to it.
    // The client holds the floor's mesh, a deflection no level is: three re-reads ask the
    // kernel nothing, and answer as the first.
    let bench = Bench {
        meshes: MeshCache::new(0),
        ..Bench::of(EXAMPLE)
    };
    let first = bench.read(&of(front(0.001), 1000, 1000)).unwrap();
    let body = &first.bodies[0];
    assert!(body.deflection > body.level);
    let held = Meshes {
        held: vec![Held {
            key: body.key,
            deflection: body.deflection,
        }],
        ..of(front(0.001), 1000, 1000)
    };
    for _ in 0..3 {
        let again = bench.read(&held).unwrap();
        let b = &again.bodies[0];
        assert!(b.held && b.mesh.is_none());
        assert_eq!(
            (b.level, b.deflection, b.serves),
            (body.level, body.deflection, body.serves)
        );
    }
    assert_eq!(bench.meshed().len(), 1);
    // A held deflection coarser than the level that's a power of two isn't a floor: a
    // zoomed-out mesh, which the view needs finer.
    let coarse = bench
        .read(&Meshes {
            held: vec![Held {
                key: body.key,
                deflection: 0.25,
            }],
            ..of(front(80.0), 1000, 1000)
        })
        .unwrap();
    assert!(!coarse.bodies[0].held && coarse.bodies[0].mesh.is_some());
}

#[test]
fn a_body_behind_and_aside_the_eye_is_meshed_for_its_distance_not_its_depth() {
    // The tests review's: the eye at the origin along +y, 40°, 1,000 px; a 2 mm cube at
    // x 200..202, y −102..−100. Its nearest point, (200, −100, 0), is √50000 = 223.607 from
    // the eye: a half pixel of 223.607 · tan 20° / 1000 = 0.081386, so 2⁻⁴. Its depth
    // behind, 100, would give 2⁻⁵.
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "b0000001", "type": "box", "v": 1, "name": "Cube", "position": [200, -102, -1], "length": 2, "width": 2, "height": 2}]}"#;
    let bench = Bench::of(text);
    let camera = Camera {
        projection: Projection::Perspective {
            field_of_view: 40.0,
        },
        eye: [0.0, 0.0, 0.0],
        target: [0.0, 10.0, 0.0],
        up: [0.0, 0.0, 1.0],
    };
    let read = bench.read(&of(camera, 1000, 1000)).unwrap();
    assert_eq!(read.bodies[0].level, 0.0625);
}

#[test]
fn a_floor_dropped_from_the_caches_floors_is_meshed_again_and_answered_alike() {
    // A 225 by 300 by 500 box: its diagonal is 625, its floor 0.0625. On a cache that keeps
    // no meshes and no floors, a view asking 2⁻⁵ (a half pixel of 0.05) is meshed at the
    // floor; held at it, the next read can't know the floor, so it meshes again, and
    // answers held, with no geometry and no counts, as a cache that knew the floor would.
    let text = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "b0000001", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 225, "width": 300, "height": 500}]}"#;
    let bench = Bench {
        meshes: MeshCache::with_floors(0, 0),
        ..Bench::of(text)
    };
    let camera = Camera {
        projection: Projection::Orthographic { view_height: 100.0 },
        eye: [112.5, -1000.0, 250.0],
        target: [112.5, 150.0, 250.0],
        up: [0.0, 0.0, 1.0],
    };
    let first = bench.read(&of(camera.clone(), 1000, 1000)).unwrap();
    let body = &first.bodies[0];
    assert_eq!((body.level, body.deflection), (0.03125, 0.0625));
    assert_eq!(body.serves, [0.0, 0.125]);
    let again = bench
        .read(&Meshes {
            held: vec![Held {
                key: body.key,
                deflection: 0.0625,
            }],
            ..of(camera.clone(), 1000, 1000)
        })
        .unwrap();
    let held = &again.bodies[0];
    assert!(held.held && held.mesh.is_none());
    assert!(
        held.faces
            .iter()
            .chain(&held.edges)
            .all(|e| e.count.is_none())
    );
    assert_eq!(
        (held.level, held.deflection, held.serves),
        (0.03125, 0.0625, [0.0, 0.125])
    );
    assert_eq!(bench.meshed(), [0.03125, 0.03125]);
    // A cache that keeps its floors (though no meshes) answers alike, meshing once.
    let kept = Bench {
        meshes: MeshCache::new(0),
        ..Bench::of(text)
    };
    kept.read(&of(camera.clone(), 1000, 1000)).unwrap();
    let again_kept = kept
        .read(&Meshes {
            held: vec![Held {
                key: body.key,
                deflection: 0.0625,
            }],
            ..of(camera, 1000, 1000)
        })
        .unwrap();
    let b = &again_kept.bodies[0];
    assert!(b.held && b.mesh.is_none());
    assert_eq!(
        (b.level, b.deflection, b.serves),
        (held.level, held.deflection, held.serves)
    );
    assert_eq!(kept.meshed(), [0.03125]);
}

#[test]
fn a_forged_floor_is_answered_alike_whatever_the_cache_keeps() {
    // The tests review's: a held 0.006 for the cut, no floor of it (its floor is 0.0046303),
    // at a view asking 2⁻²¹. A cache that knows the floor, one that keeps no meshes, and one
    // that keeps no floors either answer alike: not held, the mesh at the floor given.
    let answer = |meshes: MeshCache<Key>| {
        let bench = Bench {
            meshes,
            ..Bench::of(EXAMPLE)
        };
        bench.read(&of(front(0.001), 1000, 1000)).unwrap();
        let read = bench
            .read(&Meshes {
                held: vec![Held {
                    key: bench.key("m3tq8v0z"),
                    deflection: 0.006,
                }],
                ..of(front(0.001), 1000, 1000)
            })
            .unwrap();
        let b = &read.bodies[0];
        (b.held, b.deflection, b.mesh.is_some())
    };
    let known = answer(MeshCache::new(MESH_CACHE_BYTES));
    assert!(!known.0);
    assert!(known.2);
    assert!((known.1 - 0.0046303).abs() < 1e-7, "{}", known.1);
    assert_eq!(answer(MeshCache::new(0)), known);
    assert_eq!(answer(MeshCache::with_floors(0, 0)), known);
}
