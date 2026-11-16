//! `command::render` (docs/design/m4.6-render.md, In the core; Tests) on the fake kernel and
//! the fake renderer: the camera a named view fits, the scene the renderer is sent, the
//! meshes it asks the kernel for (by the fake's record), and every refusal, each naming its
//! field with nothing sent. Expected values are by hand, from the example's body: the box
//! (0, 0, −1) to (40, 20, 11), 9 faces and 15 edges on the fake.

use std::path::PathBuf;
use std::sync::Arc;

use super::example::EXAMPLE;
use silico_core::command::render::{Render, RenderOn, Rendering, render};
use silico_core::command::{self, Problems};
use silico_core::document::Length;
use silico_core::fake::{FakeKernel, FakeRenderer};
use silico_core::kernel::{ElementKind, ElementRef, Op};
use silico_core::meshes::{MESH_CACHE_BYTES, MeshCache};
use silico_core::recompute::{Key, Model, Recompute};
use silico_core::views::NamedView;
use silico_core::{Error, ErrorCode};
use silico_protocol::{Camera, Projection};

/// A model rebuilt on its own fake kernel, with a mesh cache and a fake renderer.
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

    fn render(&self, request: &Render) -> Result<Rendering, Problems> {
        self.render_in(request, None)
    }

    fn render_in(
        &self,
        request: &Render,
        workspace: Option<&std::path::Path>,
    ) -> Result<Rendering, Problems> {
        render(
            &self.model,
            request,
            &RenderOn {
                kernel: &self.kernel,
                meshes: &self.meshes,
                renderer: &self.renderer,
                workspace: workspace
                    .map(|root| silico_core::file::Workspace::new(root).unwrap())
                    .as_ref(),
                held: &[],
                settle: &|| Ok(()),
            },
        )
    }

    /// The meshes asked of the kernel so far: each one's deflection.
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

    /// How many points the render asked the kernel to classify: asked, not started, as a
    /// render drops the classifies it no longer needs, which a thread may take later.
    fn classified(&self) -> usize {
        self.kernel
            .asked()
            .iter()
            .filter(|c| matches!(c.op, Op::Classify { .. }))
            .count()
    }

    /// A name of the body's element `element`, as its element map names it.
    fn name_of(&self, element: ElementRef) -> String {
        let hole = silico_core::document::FeatureId::new("m3tq8v0z").unwrap();
        self.model
            .element_map(&hole)
            .unwrap()
            .name(element)
            .unwrap()
            .to_string()
    }
}

fn view(name: &str, width: u32, height: u32) -> Render {
    Render {
        view: NamedView::named(name),
        camera: None,
        width,
        height,
        highlight: vec![],
        to: None,
        replace: false,
    }
}

fn camera(camera: Camera, width: u32, height: u32) -> Render {
    Render {
        view: None,
        camera: Some(camera),
        ..view("front", width, height)
    }
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

/// Perspective, 40° high, from `eye` onto the body's centre.
fn looking_from(eye: [f64; 3]) -> Camera {
    Camera {
        projection: Projection::Perspective {
            field_of_view: 40.0,
        },
        eye,
        target: [20.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    }
}

/// The one error `result` was refused with: its code and field.
fn refused(result: Result<Rendering, Problems>) -> Error {
    let problems = result.unwrap_err();
    assert_eq!(problems.errors().len(), 1, "{problems:?}");
    problems.errors()[0].clone()
}

#[test]
fn a_named_view_fits_the_bodies_and_the_renderer_is_sent_their_scene() {
    // top_front_right at 800 x 600, on the box (0, 0, −1) to (40, 20, 11): by hand, its up
    // is (−1, 1, 2)/√6, along which the corners span 84/√6 (from (40, 0, −1) to (0, 20,
    // 11)), and its right (1, 1, 0)/√2, along which they span 60/√2; × 600/800 that's
    // 31.82, under 34.29, so the view is 1.1 × 84/√6 high, about the box's centre.
    let bench = Bench::of(EXAMPLE);
    let done = bench.render(&view("top_front_right", 800, 600)).unwrap();
    let c = &done.result.camera.camera();
    assert_eq!(c.target, [20.0, 10.0, 5.0]);
    let s6 = 6f64.sqrt();
    // Answered as a result writes it: the eye to the nanometre, which tilts the view by
    // under 1e-10, and up made perpendicular to that view; the height to the nanometre.
    for (got, want) in c.up.iter().zip([-1.0 / s6, 1.0 / s6, 2.0 / s6]) {
        assert!((got - want).abs() < 1e-10, "{:?}", c.up);
    }
    let Projection::Orthographic { view_height } = c.projection else {
        panic!("{:?}", c.projection)
    };
    assert!(
        (view_height - 1.1 * 84.0 / s6).abs() <= 5e-7,
        "{view_height}"
    );
    assert_eq!(view_height, 37.722_142);
    for k in 0..3 {
        assert_eq!(
            c.eye[k],
            Length::rounded(c.eye[k]).unwrap().mm(),
            "{:?}",
            c.eye
        );
    }
    // The result: the PNG's size, no path, the size asked; the PNG beside it.
    assert_eq!(done.png, b"PNG 800x600");
    assert_eq!(done.result.png_bytes, 11);
    assert_eq!(done.result.path, None);
    assert_eq!((done.result.width, done.result.height), (800, 600));
    // The scene: the one body, numbered 1, with no highlight, at the result's camera.
    let sent = bench.renderer.sent();
    assert_eq!(sent.len(), 1);
    let scene = &sent[0];
    assert_eq!((scene.width, scene.height, scene.ids), (800, 600, false));
    assert_eq!(&scene.camera, c);
    assert_eq!(scene.bodies.len(), 1);
    assert_eq!(scene.bodies[0].body, 1);
    assert_eq!(scene.bodies[0].mesh.face_triangles.len(), 9);
    assert_eq!(scene.bodies[0].mesh.edge_point_counts.len(), 15);
    assert!(scene.bodies[0].highlight_faces.is_empty());
    // Meshed at half a pixel's power of two: 37.72 / 600 / 2 = 0.0314, so 2⁻⁵.
    assert_eq!(bench.meshed(), [0.03125]);
    // An orthographic view asks nothing of the classifier.
    assert_eq!(bench.classified(), 0);
}

#[test]
fn an_edge_view_fits_the_bodies_as_worked_by_hand() {
    // On the box (0, 0, −1) to (40, 20, 11), at 800 x 600, by hand. top_front: up (0, 1,
    // 1)/√2, along which the corners span (20 + 11)/√2 + 1/√2 = 32/√2 = 22.63; its right
    // (1, 0, 0), along which they span 40, × 600/800 = 30: the width binds, 1.1 × 30 = 33.
    // front_right: up z, spanning 12; its right (1, 1, 0)/√2, spanning 60/√2 = 42.43, ×
    // 0.75 = 31.82: the width, 1.1 × 0.75 × 60/√2 = 35.0018.
    let bench = Bench::of(EXAMPLE);
    let r = std::f64::consts::FRAC_1_SQRT_2;
    let r2 = 2f64.sqrt();
    for (name, up, height) in [
        ("top_front", [0.0, r, r], 33.0),
        ("front_right", [0.0, 0.0, 1.0], 1.1 * 0.75 * 60.0 / r2),
    ] {
        let done = bench.render(&view(name, 800, 600)).unwrap();
        let c = &done.result.camera.camera();
        assert_eq!(c.target, [20.0, 10.0, 5.0], "{name}");
        for (got, want) in c.up.iter().zip(up) {
            assert!((got - want).abs() < 1e-10, "{name}: {:?}", c.up);
        }
        let Projection::Orthographic { view_height } = c.projection else {
            panic!("{name}: {:?}", c.projection)
        };
        // To the nanometre, as a result writes it.
        assert!(
            (view_height - height).abs() <= 5e-7,
            "{name}: {view_height}"
        );
        // The eye along the view's direction from the centre: its two faces' axes.
        let d = [c.eye[0] - 20.0, c.eye[1] - 10.0, c.eye[2] - 5.0];
        let along: [f64; 3] = match name {
            "top_front" => [0.0, -r, r],
            _ => [r, -r, 0.0],
        };
        let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        for k in 0..3 {
            assert!(
                (d[k] / length - along[k]).abs() < 1e-9,
                "{name}: {:?}",
                c.eye
            );
        }
    }
    assert_eq!(bench.renderer.sent().len(), 2);
    // top_front's half pixel, 33 / 600 / 2 = 0.0275, meshed at 2⁻⁶; front_right's,
    // 35.0018 / 1200 = 0.0292, at 2⁻⁶ too, held.
    assert_eq!(bench.meshed(), [0.015625]);
}

#[test]
fn a_second_render_at_one_level_meshes_nothing_and_two_zooms_past_the_floor_mesh_once() {
    let bench = Bench::of(EXAMPLE);
    bench.render(&view("top_front_right", 800, 600)).unwrap();
    bench.render(&view("top_front_right", 800, 600)).unwrap();
    // 790 pixels wide: the same height, so the same level.
    bench.render(&view("top_front_right", 790, 600)).unwrap();
    assert_eq!(bench.meshed(), [0.03125]);
    // The fake's floor is 1e-4 of the box's diagonal, √2144: 0.00463. A view 1 mm high at
    // 1,000 pixels asks for 2⁻¹¹ = 0.000488 and is meshed at the floor; one 0.5 mm high asks
    // for 2⁻¹², under the floor known now, so it's the floor's mesh, already kept.
    bench.render(&camera(front(1.0), 1000, 1000)).unwrap();
    bench.render(&camera(front(0.5), 1000, 1000)).unwrap();
    assert_eq!(bench.meshed(), [0.03125, 1.0 / 2048.0]);
    assert_eq!(bench.renderer.sent().len(), 5);
}

#[test]
fn a_camera_given_is_answered_with_its_up_made_perpendicular_and_drawn_as_given() {
    let bench = Bench::of(EXAMPLE);
    let mut given = front(60.0);
    // Up tilted toward the view, and long: made (0, 0, 1).
    given.up = [0.0, 3.0, 4.0];
    let done = bench.render(&camera(given.clone(), 300, 200)).unwrap();
    assert_eq!(done.result.camera.camera().up, [0.0, 0.0, 1.0]);
    assert_eq!(done.result.camera.camera().eye, given.eye);
    assert_eq!(done.result.camera.camera().target, given.target);
    // Passed back, it's the same render.
    let again = bench
        .render(&camera(done.result.camera.camera().clone(), 300, 200))
        .unwrap();
    assert_eq!(again.result.camera.camera(), done.result.camera.camera());
    let sent = bench.renderer.sent();
    assert_eq!(sent[1].camera, done.result.camera.camera());
}

#[test]
fn a_highlighted_face_or_edge_reaches_the_scene_by_its_index_from_zero() {
    let bench = Bench::of(EXAMPLE);
    let face3 = bench.name_of(ElementRef {
        kind: ElementKind::Face,
        index: 3,
    });
    let edge7 = bench.name_of(ElementRef::edge(7));
    let edge2 = bench.name_of(ElementRef::edge(2));
    let mut request = view("front", 100, 100);
    // Twice the same face, and the edges out of order: each once, ascending.
    request.highlight = vec![edge7.clone(), face3.clone(), edge2, face3];
    bench.render(&request).unwrap();
    let body = &bench.renderer.sent()[0].bodies[0];
    assert_eq!(body.highlight_faces, [2]);
    assert_eq!(body.highlight_edges, [1, 6]);
}

#[test]
fn three_bodies_and_a_camera_on_the_third_send_the_third_alone_numbered_three() {
    let three = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {"id": "aaaaaaaa", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10},
    {"id": "bbbbbbbb", "type": "box", "v": 1, "name": "B", "position": [100, 0, 0], "length": 10, "width": 10, "height": 10},
    {"id": "cccccccc", "type": "box", "v": 1, "name": "C", "position": [200, 0, 0], "length": 10, "width": 10, "height": 10}
  ]
}
"#;
    let bench = Bench::of(three);
    // From −y onto C's centre, 20 mm high and wide: x from 195 to 215.
    let on_c = Camera {
        projection: Projection::Orthographic { view_height: 20.0 },
        eye: [205.0, -100.0, 5.0],
        target: [205.0, 5.0, 5.0],
        up: [0.0, 0.0, 1.0],
    };
    // C's Face2 lit: the one body drawn holds it, as C's own face (index 1), not as the
    // third body's place among all three.
    let mut on_c_lit = camera(on_c, 100, 100);
    on_c_lit.highlight = vec!["cccccccc.box.Face2".to_owned()];
    bench.render(&on_c_lit).unwrap();
    let sent = bench.renderer.sent();
    let numbers: Vec<u32> = sent[0].bodies.iter().map(|b| b.body).collect();
    assert_eq!(numbers, [3]);
    assert_eq!(sent[0].bodies[0].highlight_faces, [1]);
    // Only C was meshed.
    assert_eq!(bench.meshed().len(), 1);
    // A named view takes all three, numbered in the model's order.
    bench.render(&view("front", 300, 100)).unwrap();
    let numbers: Vec<u32> = bench.renderer.sent()[1]
        .bodies
        .iter()
        .map(|b| b.body)
        .collect();
    assert_eq!(numbers, [1, 2, 3]);
}

#[test]
fn a_perspective_eye_inside_the_body_or_on_its_face_is_refused_and_one_outside_drawn() {
    let bench = Bench::of(EXAMPLE);
    // Inside (beside the target), on a face, at a corner.
    for eye in [[25.0, 10.0, 5.0], [20.0, 0.0, 5.0], [40.0, 20.0, 11.0]] {
        let e = refused(bench.render(&camera(looking_from(eye), 100, 100)));
        assert_eq!(e.code(), ErrorCode::InvalidInput, "{eye:?}: {e:?}");
        assert_eq!(e.parameter(), Some("camera.eye"), "{eye:?}");
        assert_eq!(e.feature(), Some("m3tq8v0z"), "{eye:?}");
        assert!(e.message().contains("inside m3tq8v0z's solid"), "{e:?}");
    }
    assert!(bench.renderer.sent().is_empty());
    assert!(bench.meshed().is_empty());
    assert_eq!(bench.classified(), 3);
    // Outside its box: drawn, the classifier never asked.
    bench
        .render(&camera(looking_from([20.0, -100.0, 5.0]), 100, 100))
        .unwrap();
    assert_eq!(bench.classified(), 3);
    assert_eq!(bench.renderer.sent().len(), 1);
    // An orthographic eye inside it: where the eye is doesn't matter to one.
    let mut inside = front(60.0);
    inside.eye = [20.0, 5.0, 5.0];
    bench.render(&camera(inside, 100, 100)).unwrap();
    assert_eq!(bench.classified(), 3);
}

#[test]
fn every_request_refused_names_its_field_and_nothing_is_meshed_or_sent() {
    let bench = Bench::of(EXAMPLE);
    let with = |f: fn(&mut Render)| {
        let mut r = view("front", 100, 100);
        f(&mut r);
        r
    };
    let mut nan = front(60.0);
    nan.eye[1] = f64::NAN;
    let mut at_target = front(60.0);
    at_target.eye = at_target.target;
    let mut up_along = front(60.0);
    up_along.up = [0.0, 1.0, 0.0];
    let mut wide = looking_from([20.0, -100.0, 5.0]);
    wide.projection = Projection::Perspective {
        field_of_view: 180.0,
    };
    let mut narrow = wide.clone();
    narrow.projection = Projection::Perspective { field_of_view: 0.0 };
    let cases: Vec<(Render, ErrorCode, &str)> = vec![
        (with(|r| r.width = 0), ErrorCode::InvalidInput, "width"),
        (with(|r| r.height = 4097), ErrorCode::InvalidInput, "height"),
        (
            with(|r| r.camera = Some(front(60.0))),
            ErrorCode::InvalidInput,
            "camera",
        ),
        (with(|r| r.view = None), ErrorCode::InvalidInput, "view"),
        (camera(nan, 100, 100), ErrorCode::InvalidInput, "camera.eye"),
        (
            camera(at_target, 100, 100),
            ErrorCode::InvalidInput,
            "camera.eye",
        ),
        (
            camera(up_along, 100, 100),
            ErrorCode::InvalidInput,
            "camera.up",
        ),
        (
            camera(front(0.0), 100, 100),
            ErrorCode::InvalidInput,
            "camera.view_height",
        ),
        (
            camera(wide, 100, 100),
            ErrorCode::InvalidInput,
            "camera.field_of_view",
        ),
        (
            camera(narrow, 100, 100),
            ErrorCode::InvalidInput,
            "camera.field_of_view",
        ),
        (
            with(|r| r.highlight = vec!["m3tq8v0z.cut.Face1".to_owned(); 1001]),
            ErrorCode::InvalidInput,
            "highlight",
        ),
        (
            with(|r| r.highlight = vec!["k7f2qx3m.box.Face99".to_owned()]),
            ErrorCode::ReferenceBroken,
            "highlight.0",
        ),
        (
            with(|r| r.highlight = vec!["not a name".to_owned()]),
            ErrorCode::ReferenceBroken,
            "highlight.0",
        ),
        (
            with(|r| r.to = Some(PathBuf::from("view.jpg"))),
            ErrorCode::InvalidInput,
            "to",
        ),
        (
            with(|r| r.to = Some(PathBuf::from("part.silico"))),
            ErrorCode::InvalidInput,
            "to",
        ),
    ];
    for (request, code, field) in cases {
        let e = refused(bench.render(&request));
        assert_eq!((e.code(), e.parameter()), (code, Some(field)), "{e:?}");
        assert!(e.suggestion().is_some(), "{e:?}");
    }
    // A view too small for the bodies' reach: 1e-5 of the 23.6 mm they reach, at the least.
    let e = refused(bench.render(&camera(front(1e-4), 100, 100)));
    assert_eq!(e.parameter(), Some("camera.view_height"), "{e:?}");
    assert!(
        bench.renderer.sent().is_empty(),
        "a refused render was sent"
    );
    assert!(bench.meshed().len() <= 1, "{:?}", bench.meshed());
}

#[test]
fn a_vertex_highlighted_is_refused_and_every_bad_highlight_is_found() {
    let bench = Bench::of(EXAMPLE);
    let vertex = bench.name_of(ElementRef {
        kind: ElementKind::Vertex,
        index: 1,
    });
    let face = bench.name_of(ElementRef {
        kind: ElementKind::Face,
        index: 1,
    });
    let mut request = view("front", 100, 100);
    request.highlight = vec![face, vertex, "k7f2qx3m.box.Face99".to_owned()];
    let problems = bench.render(&request).unwrap_err();
    let found: Vec<(ErrorCode, Option<&str>)> = problems
        .errors()
        .iter()
        .map(|e| (e.code(), e.parameter()))
        .collect();
    assert_eq!(
        found,
        [
            (ErrorCode::InvalidInput, Some("highlight.1")),
            (ErrorCode::ReferenceBroken, Some("highlight.2")),
        ]
    );
    assert!(problems.errors()[0].message().contains("names a vertex"));
    assert!(bench.renderer.sent().is_empty());
}

#[test]
fn a_named_view_of_a_model_with_no_bodies_is_refused_and_a_camera_renders_the_background() {
    let empty = r#"{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": []
}
"#;
    let bench = Bench::of(empty);
    let e = refused(bench.render(&view("top", 100, 100)));
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::InvalidInput, Some("view"))
    );
    assert!(bench.renderer.sent().is_empty());
    bench.render(&camera(front(60.0), 100, 100)).unwrap();
    let sent = bench.renderer.sent();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].bodies.is_empty());
}

#[test]
fn a_render_with_to_writes_the_png_there_and_says_where() {
    let dir = std::env::temp_dir().join(format!("silico-render-command-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let bench = Bench::of(EXAMPLE);
    let mut request = view("front", 40, 30);
    request.to = Some(dir.join("front.png"));
    let done = bench.render_in(&request, Some(&dir)).unwrap();
    // Answered as the request gave it, as `export`, `open`, and `commit` answer theirs: not
    // through links (the temporary directory is reached through one on macOS).
    assert_eq!(done.result.path, request.to);
    let wrote = dir.join("front.png");
    assert_eq!(std::fs::read(&wrote).unwrap(), b"PNG 40x30");
    // Again, without replace: refused before anything is sent; with it, written.
    let e = refused(bench.render_in(&request, Some(&dir)));
    assert_eq!(e.parameter(), Some("to"));
    assert_eq!(bench.renderer.sent().len(), 1);
    request.replace = true;
    bench.render_in(&request, Some(&dir)).unwrap();
    assert_eq!(bench.renderer.sent().len(), 2);
    // Out of the workspace: refused.
    request.to = Some(std::env::temp_dir().join("silico-render-outside.png"));
    let e = refused(bench.render_in(&request, Some(&dir)));
    assert_eq!(e.parameter(), Some("to"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_render_its_renderer_cancels_answers_as_of_a_session_that_closed() {
    // A render's ticket is cancelled only by its read's stop: a cancel, whose answer the
    // session gives (`cancelled`), or its session closing, which the render answers itself,
    // as `distance` does, naming the session.
    let bench = Bench::of(EXAMPLE);
    bench.renderer.cancel_all();
    let e = refused(bench.render(&view("front", 100, 100)));
    assert_eq!(
        (e.code(), e.parameter(), e.feature()),
        (ErrorCode::InvalidInput, Some("session"), None),
        "{e:?}"
    );
    assert_eq!(e.message(), "the session closed before the render finished");
}

#[test]
fn a_render_the_renderer_fails_is_its_error() {
    let bench = Bench::of(EXAMPLE);
    bench.renderer.fail_with(
        Error::new(
            ErrorCode::WorkerTimeout,
            "the render worker didn't finish Render within 30s",
        )
        .with_suggestion("render again"),
    );
    let e = refused(bench.render(&view("front", 100, 100)));
    assert_eq!(e.code(), ErrorCode::WorkerTimeout);
}

#[test]
fn a_perspective_view_meshes_each_body_at_half_a_pixel_where_it_is_nearest_the_eye() {
    // Found by /break-it: half a pixel was taken at the target's distance, so a target far
    // past the part meshed it far coarser than its pixels. The eye 100 mm in front of the
    // box (its nearest face y = 0), 40° high at 100 pixels: half a pixel there is 100 ×
    // tan 20° / 100 = 0.364, so 2⁻². The target a kilometre past it would have given 2¹¹.
    let bench = Bench::of(EXAMPLE);
    let far_target = Camera {
        projection: Projection::Perspective {
            field_of_view: 40.0,
        },
        eye: [20.0, -100.0, 5.0],
        target: [20.0, 1e6, 5.0],
        up: [0.0, 0.0, 1.0],
    };
    bench.render(&camera(far_target, 100, 100)).unwrap();
    assert_eq!(bench.meshed(), [0.25]);
}
