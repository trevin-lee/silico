//! /break-it on `command::render` (docs/design/m4.6-render.md, In the core, Named views, A
//! given camera): hostile cameras, culling at the view's edges, hostile highlights, bodies
//! that failed or weren't measured, and `to` paths that change while a render runs, on the
//! fake kernel and a renderer that records what it's sent. Every expected value is by hand,
//! from the documents' boxes; a test marked `BREAKS (fixed):` failed before its fix, and says why.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::example::EXAMPLE;
use super::sweeps;
use proptest::prelude::*;
use silico_core::command::render::{AnsweredCamera, Render, RenderOn, Renderer, Rendering, render};
use silico_core::command::{self, Problems};
use silico_core::fake::{FakeKernel, FakeRenderer, FakeSketches};
use silico_core::kernel::Op;
use silico_core::meshes::{MESH_CACHE_BYTES, MeshCache};
use silico_core::recompute::{Key, Model, Recompute};
use silico_core::render::RenderTicket;
use silico_core::views::NamedView;
use silico_core::{Error, ErrorCode};
use silico_protocol::{
    BooleanOp, Camera, MAX_RENDER_TRIANGLES, Mesh, MeshAt, Projection, RenderScene, Reply,
};

/// A model rebuilt on its own fake kernel (configured first), with a mesh cache and a
/// recording renderer.
struct Bench {
    kernel: Arc<FakeKernel>,
    model: Arc<Model>,
    meshes: MeshCache<Key>,
    renderer: FakeRenderer,
}

impl Bench {
    fn of(text: &str) -> Bench {
        Bench::with(text, |_| {})
    }

    fn with(text: &str, configure: impl FnOnce(&FakeKernel)) -> Bench {
        let kernel = Arc::new(FakeKernel::new(2).unwrap());
        configure(&kernel);
        let engine = Recompute::new(Arc::clone(&kernel)).with_sketches(FakeSketches::new());
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
        self.render_on(request, None, &[], &self.renderer)
    }

    fn render_on(
        &self,
        request: &Render,
        workspace: Option<&Path>,
        held: &[PathBuf],
        renderer: &dyn Renderer,
    ) -> Result<Rendering, Problems> {
        render(
            &self.model,
            request,
            &RenderOn {
                kernel: &self.kernel,
                meshes: &self.meshes,
                renderer,
                workspace,
                held,
                settle: &|| Ok(()),
            },
        )
    }

    /// Each mesh asked of the kernel so far: its deflection.
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

    fn classified(&self) -> usize {
        self.kernel
            .calls()
            .iter()
            .filter(|c| matches!(c.op, Op::Classify { .. }))
            .count()
    }

    /// The body numbers of the `n`th scene sent.
    fn numbers(&self, n: usize) -> Vec<u32> {
        self.renderer.sent()[n]
            .bodies
            .iter()
            .map(|b| b.body)
            .collect()
    }
}

/// A renderer that runs `meanwhile` as the scene is drawn, then answers as the fake does: a
/// file system changed while a render runs.
struct Meanwhile<F: Fn() + Send + Sync> {
    inner: FakeRenderer,
    meanwhile: F,
}

impl<F: Fn() + Send + Sync> Renderer for Meanwhile<F> {
    fn render(&self, scene: RenderScene) -> RenderTicket {
        (self.meanwhile)();
        self.inner.render(scene)
    }
}

/// A scratch directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "silico-adversarial-render-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(std::fs::canonicalize(&dir).unwrap())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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

/// Orthographic, from −y onto the example's centre, (20, 10, 5), `view_height` high.
fn front(view_height: f64) -> Camera {
    Camera {
        projection: Projection::Orthographic { view_height },
        eye: [20.0, -100.0, 5.0],
        target: [20.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    }
}

fn perspective(field_of_view: f64, eye: [f64; 3], target: [f64; 3]) -> Camera {
    Camera {
        projection: Projection::Perspective { field_of_view },
        eye,
        target,
        up: [0.0, 0.0, 1.0],
    }
}

/// A document of boxes, each `(id, position, [length, width, height])`.
fn boxes(boxes: &[(&str, [f64; 3], [f64; 3])]) -> String {
    let features: Vec<String> = boxes
        .iter()
        .map(|(id, [x, y, z], [l, w, h])| {
            format!(
                r#"{{"id": "{id}", "type": "box", "v": 1, "name": "B", "position": [{x}, {y}, {z}], "length": {l}, "width": {w}, "height": {h}}}"#
            )
        })
        .collect();
    format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"naming\": 5, \"features\": [{}]}}",
        features.join(", ")
    )
}

/// The one error `result` was refused with.
#[track_caller]
fn refused(result: Result<Rendering, Problems>) -> Error {
    let problems = result.unwrap_err();
    assert_eq!(problems.errors().len(), 1, "{problems:?}");
    problems.errors()[0].clone()
}

// --- Named views -------------------------------------------------------------------------

// BREAKS (fixed): a named view of a body far deeper along the view than it is across is refused as
// `internal`, "the core built a scene the render worker can't draw", with Silico's-bug
// advice, though nothing is wrong with Silico and the request is valid. `fit` makes the view
// 1.1 times the body's extent across (1.1 mm) while the scene's reach from the target is half
// the rod's length (250,000 mm), and `RenderScene::check` wants a view at least 1e-5 of the
// reach (2.5 mm): render.rs maps any `camera.*` refusal of a named view's scene to Internal.
#[test]
fn a_named_view_of_a_long_rod_seen_end_on_is_never_called_silicos_bug() {
    // A 1 x 500,000 x 1 rod along y. From the front (−y): along up (z) it spans 1, along
    // the right (x) 1 × 100/100, so the view is 1.1 high, about (0.5, 250000, 0.5). The
    // fake's mesh has a corner at the origin, 250,000 from there.
    let bench = Bench::of(&boxes(&[("aaaaaaaa", [0.0; 3], [1.0, 500_000.0, 1.0])]));
    // Drawn, the view held to the scene's reach: 1.01 × 1e-5 of half the rod's diagonal,
    // √(500,000² + 2)/2 = 250,000.000002, so 2.52500000002, written to the nanometre 2.525,
    // over the 1.1 its extent across gives.
    let done = bench.render(&view("front", 100, 100)).unwrap();
    let Projection::Orthographic { view_height } = done.result.camera.camera().projection else {
        panic!("{:?}", done.result.camera.camera());
    };
    assert_eq!(view_height, 2.525);
    // From the right (+x), the same rod spans 500,000 across: 550,000 high, drawn.
    bench.render(&view("right", 100, 100)).unwrap();
}

#[test]
fn every_named_view_of_a_nanometre_cube_at_either_extreme_aspect_is_drawn() {
    // A 1 nm cube (1e-6 mm, the document's resolution). Its smallest view is 1.1e-6 high,
    // above the least, 1e-6; its eye a diagonal, √3e-6, from its centre, above the least
    // distance, 1e-6.
    let bench = Bench::of(&boxes(&[("aaaaaaaa", [0.0; 3], [1e-6; 3])]));
    for name in [
        "front",
        "back",
        "left",
        "right",
        "top",
        "bottom",
        "top_front_left",
        "top_front_right",
        "top_back_left",
        "top_back_right",
        "bottom_front_left",
        "bottom_front_right",
        "bottom_back_left",
        "bottom_back_right",
    ] {
        for (w, h) in [(4096, 1), (1, 4096), (1, 1)] {
            let done = bench.render(&view(name, w, h)).unwrap_or_else(|p| {
                panic!("{name} {w}x{h}: {p:?}");
            });
            // The cube's centre, half a nanometre, as a result writes it: to the nanometre,
            // half to even, so 0; its view at least the nanometre a view may be.
            assert_eq!(done.result.camera.camera().target, [0.0; 3], "{name}");
            let Projection::Orthographic { view_height } = done.result.camera.camera().projection
            else {
                panic!();
            };
            assert!(view_height >= 1e-6, "{name} {w}x{h}");
        }
    }
    assert_eq!(bench.renderer.sent().len(), 14 * 3);
}

// --- Bodies that failed or weren't measured ----------------------------------------------

/// The example with every Summarize refused: its one body built, but not measured.
fn unmeasured() -> Bench {
    Bench::with(EXAMPLE, |k| {
        k.refuse(|op| {
            matches!(op, Op::Summarize { .. }).then(|| {
                Error::new(ErrorCode::KernelFailed, "the measure failed")
                    .with_suggestion("measure again")
            })
        });
    })
}

// BREAKS (fixed): a body built but not measured (its Summarize failed) has no tight box, so the
// render leaves it out of every view (`kept` keeps only bodies with bounds): a camera
// straight at it renders an empty background and answers Ok, as if the model were empty.
#[test]
fn a_body_whose_measure_failed_is_still_drawn_by_a_camera_on_it() {
    let bench = unmeasured();
    assert_eq!(bench.model.bodies().len(), 1, "the body is the model's");
    match bench.render(&camera(front(60.0), 100, 100)) {
        Ok(_) => assert_eq!(bench.numbers(0), [1], "the body was left out"),
        Err(problems) => {
            // Or refused, saying which body can't be drawn.
            let e = &problems.errors()[0];
            assert_eq!(e.feature(), Some("m3tq8v0z"), "{e:?}");
            assert!(bench.renderer.sent().is_empty());
        }
    }
}

#[test]
fn a_highlight_on_a_body_that_couldnt_be_described_is_lit() {
    // The cut built but its description was refused: it's drawn, and a face of it is lit
    // by name, which needs the element map, not the geometry a description holds.
    let bench = Bench::with(EXAMPLE, |k| {
        k.refuse(|op| {
            matches!(op, Op::Describe { .. }).then(|| {
                Error::new(ErrorCode::KernelFailed, "no describing today")
                    .with_suggestion("describe again")
            })
        });
    });
    let mut request = view("front", 100, 100);
    request.highlight = vec!["m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face1)".to_owned()];
    bench
        .render(&request)
        .unwrap_or_else(|p| panic!("{:?}", p.errors()));
    let sent = &bench.renderer.sent()[0];
    assert_eq!(sent.bodies.len(), 1);
    assert_eq!(
        sent.bodies[0].highlight_faces.len(),
        1,
        "{:?}",
        sent.bodies[0]
    );
}

#[test]
fn a_highlight_of_a_solid_is_refused_naming_it() {
    // A solid has no pixels of its own to light: `invalid_input` at `highlight.0`, never
    // looked for as a face (a solid taken would be `reference_broken`, or drawn).
    let bench = Bench::of(EXAMPLE);
    let mut request = view("front", 100, 100);
    request.highlight = vec!["m3tq8v0z.cut.Solid1".to_owned()];
    let e = refused(bench.render(&request));
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::InvalidInput, Some("highlight.0")),
        "{e:?}"
    );
    assert!(e.message().contains("names a solid"), "{e:?}");
    assert!(bench.renderer.sent().is_empty());
}

// BREAKS (fixed): a named view of a model whose one body wasn't measured says the model has no
// bodies to fit ("view front fits the model's bodies, and it has none to fit"), and advises
// a camera, which (above) renders nothing either. It has a body; the error should say that
// body couldn't be measured.
#[test]
fn a_named_view_of_an_unmeasured_body_doesnt_say_the_model_has_none() {
    let bench = unmeasured();
    let e = refused(bench.render(&view("front", 100, 100)));
    assert!(!e.message().contains("has none"), "{e:?}");
    assert_eq!(
        (e.code(), e.parameter(), e.feature()),
        (ErrorCode::InvalidInput, Some("view"), Some("m3tq8v0z")),
        "{e:?}"
    );
    assert!(bench.renderer.sent().is_empty());
}

// BREAKS (fixed): a perspective eye inside a body that wasn't measured isn't refused: the classifier
// is asked only of bodies whose tight box holds the eye, and this body has none, so the eye
// at (25, 10, 5), inside the block, is taken and an empty scene drawn.
#[test]
fn a_perspective_eye_inside_an_unmeasured_body_is_refused() {
    let bench = unmeasured();
    let inside = perspective(40.0, [25.0, 10.0, 5.0], [20.0, 10.0, 5.0]);
    let e = refused(bench.render(&camera(inside, 100, 100)));
    assert_eq!(
        (e.code(), e.parameter(), e.feature()),
        (
            ErrorCode::InvalidInput,
            Some("camera.eye"),
            Some("m3tq8v0z")
        ),
        "{e:?}"
    );
    assert!(bench.renderer.sent().is_empty());
}

#[test]
fn a_failed_cut_leaves_its_inputs_as_bodies_numbered_in_order_and_its_names_say_its_failure() {
    // The cut refused: the block and the pin stay bodies (a failed feature consumes
    // nothing), 1 and 2; a name of the cut's own is answered with the cut's failure, as
    // `elements` and the measures answer one (M4.1 section 7, M4.5); each input's first face
    // is its own, index 0, by body.
    let bench = Bench::with(EXAMPLE, |k| {
        k.refuse(|op| {
            matches!(
                op,
                Op::Boolean {
                    op: BooleanOp::Cut,
                    ..
                }
            )
            .then(|| Error::new(ErrorCode::KernelFailed, "no cut").with_suggestion("x"))
        });
    });
    let mut request = view("front", 100, 100);
    request.highlight = vec!["m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face1)".to_owned()];
    let e = refused(bench.render(&request));
    assert_eq!(
        (e.code(), e.feature()),
        (ErrorCode::KernelFailed, Some("m3tq8v0z")),
        "{e:?}"
    );
    assert!(bench.renderer.sent().is_empty());
    request.highlight = vec![
        "a91cw0de.cylinder.Face1".to_owned(),
        "k7f2qx3m.box.Face1".to_owned(),
    ];
    let done = bench.render(&request).unwrap();
    assert_eq!(bench.numbers(0), [1, 2]);
    let sent = &bench.renderer.sent()[0];
    assert_eq!(sent.bodies[0].highlight_faces, [0]);
    assert_eq!(sent.bodies[1].highlight_faces, [0]);
    // The view fits both: the block (0, 0, 0)-(40, 20, 10) and the pin's box (17.5, 7.5,
    // −1)-(22.5, 12.5, 11), so (0, 0, −1)-(40, 20, 11), centred at (20, 10, 5).
    assert_eq!(done.result.camera.camera().target, [20.0, 10.0, 5.0]);
}

// --- Given cameras -----------------------------------------------------------------------

#[test]
fn an_up_just_across_the_view_is_taken_and_one_just_under_refused() {
    let bench = Bench::of(EXAMPLE);
    // From −y: the up (0, 1, 2e-9) is 2e-9 across the view, over the least, 1e-9; made
    // perpendicular it's (0, 0, 2e-9), so (0, 0, 1).
    let mut c = front(60.0);
    c.up = [0.0, 1.0, 2e-9];
    let done = bench.render(&camera(c.clone(), 100, 100)).unwrap();
    assert_eq!(done.result.camera.camera().up, [0.0, 0.0, 1.0]);
    // 5e-10 across: refused, nothing sent.
    c.up = [0.0, 1.0, 5e-10];
    let e = refused(bench.render(&camera(c, 100, 100)));
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::InvalidInput, Some("camera.up"))
    );
    assert_eq!(bench.renderer.sent().len(), 1);
}

#[test]
fn cameras_at_the_coordinate_limit_mesh_at_the_ladders_level_by_hand() {
    let bench = Bench::of(EXAMPLE);
    // The eye at (1e12, 1e12, 1e12), the target at its negation, 179.999° high. Half a
    // pixel is taken at the body's nearest depth along the view, its box's corner
    // (40, 20, 11)'s, (3e12 − 71)/√3 = √3e12 to 1e-10 of it: at 100 pixels √3e12 × tan(89.9995°) / 100 = 1.7321e12 ×
    // 114591.56 / 100 = 1.98e15, whose power of two below is 2⁵⁰ (at the target's 2√3e12
    // it would be 2⁵¹).
    let far = perspective(179.999, [1e12; 3], [-1e12; 3]);
    bench.render(&camera(far.clone(), 100, 100)).unwrap();
    assert_eq!(bench.meshed(), [1_125_899_906_842_624.0]);
    // An ulp past the limit: refused.
    let mut past = far;
    past.eye[0] = f64::from_bits(1e12f64.to_bits() + 1);
    let e = refused(bench.render(&camera(past, 100, 100)));
    assert_eq!(e.parameter(), Some("camera.eye"));
    // The tallest orthographic view, 1e12, at 1 pixel: half a pixel 5e11, so 2³⁸.
    bench.render(&camera(front(1e12), 1, 1)).unwrap();
    assert_eq!(bench.meshed(), [1_125_899_906_842_624.0, 274_877_906_944.0]);
    assert_eq!(bench.renderer.sent().len(), 2);
}

#[test]
fn an_eye_in_the_second_body_names_it_and_one_on_a_shared_face_names_the_first() {
    // A at (0, 0, 0) and B at (10, 0, 0), 10 mm cubes, touching at x = 10.
    let bench = Bench::of(&boxes(&[
        ("aaaaaaaa", [0.0; 3], [10.0; 3]),
        ("bbbbbbbb", [10.0, 0.0, 0.0], [10.0; 3]),
    ]));
    let at = |eye: [f64; 3]| camera(perspective(40.0, eye, [10.0, 100.0, 5.0]), 100, 100);
    let e = refused(bench.render(&at([15.0, 5.0, 5.0])));
    assert_eq!(
        (e.parameter(), e.feature()),
        (Some("camera.eye"), Some("bbbbbbbb"))
    );
    assert_eq!(bench.classified(), 1, "only B's box holds (15, 5, 5)");
    let e = refused(bench.render(&at([10.0, 5.0, 5.0])));
    assert_eq!(
        (e.parameter(), e.feature()),
        (Some("camera.eye"), Some("aaaaaaaa"))
    );
    assert_eq!(bench.classified(), 3, "both boxes hold (10, 5, 5)");
    // At B's far corner, (20, 10, 10): on it, refused.
    let e = refused(bench.render(&at([20.0, 10.0, 10.0])));
    assert_eq!(e.feature(), Some("bbbbbbbb"));
    // A nanometre beyond it, the least a request can say: outside every box, never
    // classified, drawn.
    let beyond = 20.000_001;
    bench.render(&at([beyond, 10.0, 10.0])).unwrap();
    assert_eq!(bench.classified(), 4);
    assert_eq!(bench.renderer.sent().len(), 1);
}

#[test]
fn a_perspective_view_leaves_out_what_is_behind_or_beside_it_and_keeps_what_straddles_its_eye() {
    // The eye at the origin looking along +y, 90° high, square: a point at depth y is in
    // view if |x| and |z| are at most y (tan 45°, a hair under 1 in f64).
    // 1: wholly behind the eye (y from −20 to −10), however wide: out.
    // 2: beside, from depth −10 to 10, x from 50: every corner's x past its depth: out.
    // 3: the same depths, x from 5: its corner (5, 10) is in view at depth 10: kept.
    let bench = Bench::of(&boxes(&[
        (
            "aaaaaaaa",
            [-1000.0, -20.0, -1000.0],
            [2000.0, 10.0, 2000.0],
        ),
        ("bbbbbbbb", [50.0, -10.0, -1.0], [10.0, 20.0, 2.0]),
        ("cccccccc", [5.0, -10.0, -1.0], [55.0, 20.0, 2.0]),
    ]));
    let c = perspective(90.0, [0.0; 3], [0.0, 100.0, 0.0]);
    bench.render(&camera(c, 100, 100)).unwrap();
    assert_eq!(bench.numbers(0), [3]);
    assert_eq!(bench.meshed().len(), 1);
    assert_eq!(bench.classified(), 0, "no box holds the eye");
    // The same orthographic, 30 high: no depth clips it, so 1, all across x and z, is kept;
    // 2, x from 50, is past the side at 15; 3, x from 5 to 60, overlaps it.
    let mut o = perspective(90.0, [0.0; 3], [0.0, 100.0, 0.0]);
    o.projection = Projection::Orthographic { view_height: 30.0 };
    bench.render(&camera(o, 100, 100)).unwrap();
    assert_eq!(bench.numbers(1), [1, 3]);
}

/// A camera's numbers, biased to the hostile: near the coordinate limit and tiny, ups nearly
/// along the view.
fn a_camera() -> impl Strategy<Value = Camera> {
    let coordinate = || {
        prop_oneof![
            -1e3..1e3f64,
            Just(0.0),
            Just(-0.0),
            Just(1e12),
            Just(-1e12),
            -1e-3..1e-3f64,
            -1e11..1e11f64,
        ]
    };
    let point = || [coordinate(), coordinate(), coordinate()];
    let across = prop_oneof![Just(0.0), 1e-8..1e-6f64, 1e-6..1.0f64, 1.0..1e6f64];
    let projection = prop_oneof![
        (1e-6..1e12f64).prop_map(|view_height| Projection::Orthographic { view_height }),
        (1e-3..179.999f64).prop_map(|field_of_view| Projection::Perspective { field_of_view }),
    ];
    (
        point(),
        point(),
        [-1.0..1.0f64, -1.0..1.0f64, -1.0..1.0f64],
        across,
        prop_oneof![Just(1.0), 1e-200..1e-100f64, 1e100..1e200f64],
        projection,
    )
        .prop_map(|(eye, target, random, across, scale, projection)| {
            // The up: the view's direction, plus `across` of a random direction, scaled.
            let d = [eye[0] - target[0], eye[1] - target[1], eye[2] - target[2]];
            let up = [0, 1, 2].map(|k| (d[k] + across * random[k] * 1e3) * scale);
            Camera {
                projection,
                eye,
                target,
                up,
            }
        })
}

// BREAKS (fixed): the answer passed back isn't answered unchanged, by an ulp of its up
// (`an_answered_camera_passed_back_is_answered_to_the_bit` is its shrunk case, by hand).
proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    /// The result's camera, passed back, is answered unchanged, to the bit (M4.6: its up
    /// "made perpendicular to the view and normalized, so the next request can pass it back
    /// unchanged"), and its up is of unit length. A model with no bodies, so nothing is
    /// culled, classified, or meshed: only the camera is at stake.
    #[test]
    fn a_cameras_answer_passed_back_is_answered_unchanged(c in a_camera()) {
        let bench = Bench::of(&boxes(&[]));
        let Ok(first) = bench.render(&camera(c.clone(), 64, 48)) else {
            // Refused: it must be for its camera (failing its check, or failing it once
            // rounded as a result writes it: an eye and a target half a nanometre apart),
            // and nothing sent.
            let rounded = AnsweredCamera::rounded(&c).map(|a| a.camera());
            prop_assert!(
                c.check().is_err() || rounded.is_none_or(|r| r.check().is_err()),
                "{c:?}"
            );
            prop_assert!(bench.renderer.sent().is_empty());
            return Ok(());
        };
        let answered = first.result.camera.camera();
        let again = bench.render(&camera(answered.clone(), 64, 48)).unwrap().result.camera.camera();
        prop_assert_eq!(&again, &answered, "from {:?}", c);
        let up = answered.up;
        let length = (up[0] * up[0] + up[1] * up[1] + up[2] * up[2]).sqrt();
        prop_assert!((length - 1.0).abs() <= 1e-15, "{length} {c:?}");
    }
}

// --- Highlights --------------------------------------------------------------------------

#[test]
fn a_thousand_highlights_exactly_are_taken_and_a_thousand_broken_each_found() {
    let bench = Bench::of(EXAMPLE);
    // 1,000, one face over and over: taken, once.
    let mut request = view("front", 100, 100);
    request.highlight = vec!["k7f2qx3m.box.Face1".to_owned(); 1000];
    bench.render(&request).unwrap();
    assert_eq!(bench.renderer.sent()[0].bodies[0].highlight_faces, [0]);
    // 1,000 that find nothing: the first 100 listed, by their places, all 1,000 counted,
    // and promptly (each one's candidates looked for).
    request.highlight = (1..=1000)
        .map(|n| format!("k7f2qx3m.box.Face{}", 100 + n))
        .collect();
    let start = std::time::Instant::now();
    let problems = bench.render(&request).unwrap_err();
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(problems.errors().len(), 100);
    assert_eq!(problems.total(), 1000);
    for (i, e) in problems.errors().iter().enumerate() {
        assert_eq!(e.code(), ErrorCode::ReferenceBroken, "{e:?}");
        assert_eq!(e.parameter(), Some(format!("highlight.{i}").as_str()));
    }
    assert_eq!(bench.renderer.sent().len(), 1);
}

#[test]
fn hostile_highlight_text_is_a_broken_reference_named_by_its_place_and_shown_short() {
    let bench = Bench::of(EXAMPLE);
    let huge = "k7f2qx3m.box.Face1".repeat(60_000);
    let hostile = [
        "",
        "\0",
        "Face1",
        " k7f2qx3m.box.Face1",
        "k7f2qx3m.box.Face1 ",
        "k7f2qx3m.box.Face0",
        "k7f2qx3m.box.Face01",
        "k7f2qx3m.box.Face-1",
        "k7f2qx3m.box.Face4294967296",
        "K7F2QX3M.box.Face1",
        "k7f2qx3m.cylinder.Face1",
        "zzzzzzzz.box.Face1",
        "k7f2qx3m.box.Face1\u{202e}",
        "k7f2qx3m.box.Face1;k7f2qx3m.box.Face1",
        "m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face1",
        &huge,
    ];
    for (i, text) in hostile.iter().enumerate() {
        let mut request = view("front", 100, 100);
        request.highlight = vec!["k7f2qx3m.box.Face1".to_owned(), (*text).to_owned()];
        let e = refused(bench.render(&request));
        assert_eq!(
            (e.code(), e.parameter()),
            (ErrorCode::ReferenceBroken, Some("highlight.1")),
            "{i}: {e:?}"
        );
        assert!(e.message().len() < 2000, "{i}: {} bytes", e.message().len());
        assert!(e.suggestion().is_some());
    }
    assert!(bench.renderer.sent().is_empty());
}

#[test]
fn a_sketchs_own_edge_finds_nothing_and_a_pads_face_named_from_it_is_lit() {
    let pad = sweeps::document(&[
        &sweeps::rectangle("s0000001", "xy", [0.0, 0.0], 10.0, 5.0),
        &sweeps::pad("p0000001", None, "s0000001", sweeps::TEN),
    ]);
    let bench = Bench::of(&pad);
    let mut request = view("top", 100, 100);
    request.highlight = vec!["s0000001.sketch.Edge.na000001".to_owned()];
    let e = refused(bench.render(&request));
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::ReferenceBroken, Some("highlight.0"))
    );
    assert!(bench.renderer.sent().is_empty());
    // The pad's top, Face6, made from the sketch's face: index 5; its side from na000002,
    // Face2: 1.
    request.highlight = vec![
        "p0000001.pad.Face(G s0000001.sketch.Face1)".to_owned(),
        "p0000001.pad.Face(G s0000001.sketch.Edge.na000002)".to_owned(),
    ];
    bench.render(&request).unwrap();
    assert_eq!(bench.renderer.sent()[0].bodies[0].highlight_faces, [1, 5]);
}

// --- `to` --------------------------------------------------------------------------------

// BREAKS (fixed): a relative `to` is taken against the process's working directory, not the
// workspace (M4.1, section 2: "Paths are relative to the workspace when the process has
// one"): `view.png`, with a workspace that isn't the working directory, is refused as
// outside the workspace.
#[test]
fn a_relative_to_is_written_in_the_workspace() {
    let dir = Scratch::new("relative");
    let bench = Bench::of(EXAMPLE);
    let mut request = view("front", 40, 30);
    request.to = Some(PathBuf::from("view.png"));
    let done = bench
        .render_on(&request, Some(&dir.0), &[], &bench.renderer)
        .unwrap();
    assert_eq!(done.result.path, Some(dir.0.join("view.png")));
    assert_eq!(std::fs::read(dir.0.join("view.png")).unwrap(), b"PNG 40x30");
}

// BREAKS (fixed): a file name as long as a name may be (255 bytes) passes the check, the scene is
// drawn, and then the write fails, `io_failed`: "couldn't create a file in ...: File name
// too long". The file beside it, `.NAME.part-PID`, is longer than the name, past the limit.
// Its advice ("check that this user may write the file...") doesn't fit either.
#[test]
fn a_png_whose_name_is_as_long_as_a_name_may_be_is_written() {
    let dir = Scratch::new("long-name");
    let bench = Bench::of(EXAMPLE);
    let name = format!("{}.png", "a".repeat(251));
    assert_eq!(name.len(), 255);
    let mut request = view("front", 40, 30);
    request.to = Some(dir.0.join(&name));
    let result = bench.render_on(&request, Some(&dir.0), &[], &bench.renderer);
    match result {
        Ok(done) => {
            assert_eq!(done.result.path, Some(dir.0.join(&name)));
            assert_eq!(std::fs::read(dir.0.join(&name)).unwrap(), b"PNG 40x30");
        }
        Err(problems) => panic!(
            "a valid name, drawn ({} sent), wasn't written: {problems:?}",
            bench.renderer.sent().len()
        ),
    }
}

#[test]
fn a_file_put_at_to_while_rendering_is_refused_and_left_as_it_is() {
    let dir = Scratch::new("raced-file");
    let bench = Bench::of(EXAMPLE);
    let to = dir.0.join("view.png");
    let theirs = to.clone();
    let renderer = Meanwhile {
        inner: FakeRenderer::new(),
        meanwhile: move || std::fs::write(&theirs, b"theirs").unwrap(),
    };
    let mut request = view("front", 40, 30);
    request.to = Some(to.clone());
    let e = refused(bench.render_on(&request, Some(&dir.0), &[], &renderer));
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::InvalidInput, Some("to"))
    );
    assert!(e.message().contains("is there already"), "{e:?}");
    assert_eq!(std::fs::read(&to).unwrap(), b"theirs");
    assert_eq!(renderer.inner.sent().len(), 1);
}

#[test]
fn a_directory_swapped_for_a_link_out_of_the_workspace_while_rendering_is_refused() {
    let dir = Scratch::new("raced-link");
    let workspace = dir.0.join("work");
    let outside = dir.0.join("outside");
    std::fs::create_dir_all(workspace.join("sub")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let bench = Bench::of(EXAMPLE);
    let (sub, out) = (workspace.join("sub"), outside.clone());
    let renderer = Meanwhile {
        inner: FakeRenderer::new(),
        meanwhile: move || {
            std::fs::remove_dir(&sub).unwrap();
            std::os::unix::fs::symlink(&out, &sub).unwrap();
        },
    };
    let mut request = view("front", 40, 30);
    request.to = Some(workspace.join("sub/view.png"));
    let e = refused(bench.render_on(&request, Some(&workspace), &[], &renderer));
    assert_eq!(e.parameter(), Some("to"));
    assert!(e.message().contains("outside the workspace"), "{e:?}");
    assert!(!outside.join("view.png").exists());
}

#[test]
fn a_hard_link_to_a_held_document_never_changes_the_document() {
    let dir = Scratch::new("hard-link");
    let document = dir.0.join("part.silico");
    std::fs::write(&document, EXAMPLE).unwrap();
    let alias = dir.0.join("alias.png");
    std::fs::hard_link(&document, &alias).unwrap();
    let bench = Bench::of(EXAMPLE);
    let mut request = view("front", 40, 30);
    request.to = Some(alias.clone());
    request.replace = true;
    let held = [document.clone()];
    // Written over the alias, or refused: either way the document is as it was.
    let _ = bench.render_on(&request, Some(&dir.0), &held, &bench.renderer);
    assert_eq!(std::fs::read_to_string(&document).unwrap(), EXAMPLE);
}

#[test]
fn a_png_in_capitals_and_a_unicode_name_are_written_and_a_png_directory_refused_unsent() {
    let dir = Scratch::new("names");
    let bench = Bench::of(EXAMPLE);
    for name in ["VIEW.PNG", "vüe ✓.Png"] {
        let mut request = view("front", 40, 30);
        request.to = Some(dir.0.join(name));
        let done = bench
            .render_on(&request, Some(&dir.0), &[], &bench.renderer)
            .unwrap();
        assert_eq!(done.result.path, Some(dir.0.join(name)));
    }
    std::fs::create_dir(dir.0.join("dir.png")).unwrap();
    let mut request = view("front", 40, 30);
    request.to = Some(dir.0.join("dir.png"));
    let e = refused(bench.render_on(&request, Some(&dir.0), &[], &bench.renderer));
    assert_eq!(e.parameter(), Some("to"));
    assert_eq!(bench.renderer.sent().len(), 2);
}

#[test]
fn a_sibling_whose_name_starts_with_the_workspaces_is_outside_it() {
    // `work2` starts with `work`'s text, but isn't inside it.
    let dir = Scratch::new("sibling");
    let (workspace, sibling) = (dir.0.join("work"), dir.0.join("work2"));
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    let bench = Bench::of(EXAMPLE);
    let mut request = view("front", 40, 30);
    request.to = Some(sibling.join("view.png"));
    let e = refused(bench.render_on(&request, Some(&workspace), &[], &bench.renderer));
    assert_eq!(e.parameter(), Some("to"));
    assert!(e.message().contains("outside the workspace"), "{e:?}");
    assert!(bench.renderer.sent().is_empty());
}

// --- What the kernel fails ---------------------------------------------------------------

/// The example with the kernel's `picks` requests refused while `failing` is set, as a
/// worker would fail them (no feature of its own).
fn failing_while(failing: Arc<std::sync::atomic::AtomicBool>, picks: fn(&Op) -> bool) -> Bench {
    Bench::with(EXAMPLE, move |k| {
        k.refuse(move |op| {
            (picks(op) && failing.load(std::sync::atomic::Ordering::SeqCst)).then(|| {
                Error::new(ErrorCode::KernelFailed, "the kernel failed").with_suggestion("x")
            })
        });
    })
}

// BREAKS (fixed): a body's mesh that the kernel fails is the kernel's error as it came, naming no
// feature: in a model of many bodies, nothing says which body couldn't be meshed (CLAUDE.md:
// errors carry the feature). `meshed` passes `done(pending.wait())?` through without
// `with_feature(body.id)`. The next render, the kernel well again, works.
#[test]
fn a_mesh_the_kernel_fails_names_its_body_and_the_next_render_works() {
    let failing = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let bench = failing_while(Arc::clone(&failing), |op| {
        matches!(op, Op::Tessellate { .. })
    });
    let e = refused(bench.render(&view("front", 100, 100)));
    assert!(bench.renderer.sent().is_empty());
    failing.store(false, std::sync::atomic::Ordering::SeqCst);
    bench.render(&view("front", 100, 100)).unwrap();
    assert_eq!(bench.numbers(0), [1]);
    assert_eq!(e.code(), ErrorCode::KernelFailed);
    assert_eq!(e.feature(), Some("m3tq8v0z"), "{e:?}");
}

// BREAKS (fixed): likewise a classification the kernel fails names no body: the eye's check fails
// with the kernel's error alone.
#[test]
fn a_classification_the_kernel_fails_names_its_body() {
    let failing = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let bench = failing_while(Arc::clone(&failing), |op| matches!(op, Op::Classify { .. }));
    let inside_its_box = perspective(40.0, [25.0, 10.0, 5.0], [20.0, 10.0, 5.0]);
    let e = refused(bench.render(&camera(inside_its_box, 100, 100)));
    assert!(bench.renderer.sent().is_empty());
    assert_eq!(e.feature(), Some("m3tq8v0z"), "{e:?}");
}

// BREAKS (fixed): a camera that is already an answer, its up across the view and of unit length, is
// answered with its up changed in its last bit (x 0.21383887929413314 becomes …312; the
// answer this camera came from, proptest's shrunk case, had y …916 where this has …917):
// `perpendicular` takes `Camera::frame`'s up, which scales, normalizes, projects, and
// normalizes again what's already normal, so it isn't a fixed point, and the camera passed
// back "unchanged" (M4.6, A given camera) is never answered unchanged.
#[test]
fn an_answered_camera_passed_back_is_answered_to_the_bit() {
    let bench = Bench::of(&boxes(&[]));
    let answered = Camera {
        projection: Projection::Orthographic { view_height: 1e-6 },
        eye: [0.0; 3],
        target: [0.0, 0.0, 1e12],
        up: [0.21383887929413314, 0.9768689439746917, 0.0],
    };
    let again = bench.render(&camera(answered.clone(), 64, 48)).unwrap();
    assert_eq!(again.result.camera.camera(), answered);
}

// --- Round 1 of the council ----------------------------------------------------------

#[test]
fn bodies_in_view_of_more_faces_than_a_render_draws_are_refused_before_meshing() {
    // 3,334 boxes of 6 faces: 20,004, past the 20,000 a render draws. Counted from the
    // B-reps, before any is meshed; named `camera`, as a camera on part of the model has
    // fewer faces in view.
    let many: Vec<(String, [f64; 3])> = (0..3334)
        .map(|i| (format!("b{i:07}"), [f64::from(i) * 2.0, 0.0, 0.0]))
        .collect();
    let refs: Vec<(&str, [f64; 3], [f64; 3])> = many
        .iter()
        .map(|(id, at)| (id.as_str(), *at, [1.0, 1.0, 1.0]))
        .collect();
    let bench = Bench::of(&boxes(&refs));
    let e = refused(bench.render(&view("front", 100, 100)));
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::InvalidInput, Some("camera")),
        "{e:?}"
    );
    assert!(e.message().contains("20004 faces"), "{e:?}");
    assert!(bench.meshed().is_empty());
    assert!(bench.renderer.sent().is_empty());
}

/// A mesh of one face of `triangles` triangles, at `deflection`.
fn one_face_of(triangles: usize, deflection: f64) -> Reply {
    Reply::MeshAt(MeshAt {
        mesh: Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            normals: vec![[0.0, -1.0, 0.0]; 3],
            triangles: vec![[0, 1, 2]; triangles],
            face_triangles: vec![u32::try_from(triangles).unwrap()],
            edge_points: vec![],
            edge_point_counts: vec![],
            valid: None,
        },
        deflection,
    })
}

#[test]
fn meshes_of_more_triangles_than_a_render_draws_name_the_side_that_meshes_coarser() {
    // The kernel meshing the example's body to 2,000,001 triangles, one past the cap: the
    // request can mesh coarser, so it's `invalid_input`, never Silico's bug. From the front
    // at 100 x 100 the fit is bound by the width (40 across against 12 up): `width`.
    let bench = Bench::with(EXAMPLE, |k| {
        k.answer_instead(|op| match op {
            Op::Tessellate { deflection, .. } => {
                Some(one_face_of(MAX_RENDER_TRIANGLES + 1, *deflection))
            }
            _ => None,
        });
    });
    let e = refused(bench.render(&view("front", 100, 100)));
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::InvalidInput, Some("width")),
        "{e:?}"
    );
    assert!(e.message().contains("2000001 triangles"), "{e:?}");
    assert!(bench.renderer.sent().is_empty());
    // A camera's: `height`.
    let e = refused(bench.render(&camera(front(60.0), 100, 100)));
    assert_eq!(e.parameter(), Some("height"), "{e:?}");
    // Exactly at the cap: drawn.
    let bench = Bench::with(EXAMPLE, |k| {
        k.answer_instead(|op| match op {
            Op::Tessellate { deflection, .. } => {
                Some(one_face_of(MAX_RENDER_TRIANGLES, *deflection))
            }
            _ => None,
        });
    });
    bench.render(&view("front", 100, 100)).unwrap();
}

#[test]
fn the_triangle_cap_counts_every_bodys_triangles_together() {
    // Two bodies of 1,000,001 triangles each: neither is past the cap, but the scene is
    // (2,000,002), so it's the request's fault before anything is sent.
    let bench = Bench::with(
        &boxes(&[
            ("aaaaaaaa", [0.0; 3], [1.0; 3]),
            ("bbbbbbbb", [2.0, 0.0, 0.0], [1.0; 3]),
        ]),
        |k| {
            k.answer_instead(|op| match op {
                Op::Tessellate { deflection, .. } => {
                    Some(one_face_of(MAX_RENDER_TRIANGLES / 2 + 1, *deflection))
                }
                _ => None,
            });
        },
    );
    let e = refused(bench.render(&view("front", 100, 100)));
    assert_eq!(e.code(), ErrorCode::InvalidInput, "{e:?}");
    assert!(e.message().contains("2000002 triangles"), "{e:?}");
    assert!(bench.renderer.sent().is_empty());
}

#[test]
fn a_render_whose_read_wont_settle_writes_no_file() {
    // The read cancelled while the scene was drawn: the render answers its cancel, and the
    // PNG drawn is never written (decision 34: a cancelled command leaves no trace).
    let dir = Scratch::new("settle");
    let bench = Bench::of(EXAMPLE);
    let to = dir.0.join("view.png");
    let mut request = view("front", 40, 30);
    request.to = Some(to.clone());
    let cancelled = || Err(Error::new(ErrorCode::Cancelled, "the render was cancelled"));
    let e = refused(render(
        &bench.model,
        &request,
        &RenderOn {
            kernel: &bench.kernel,
            meshes: &bench.meshes,
            renderer: &bench.renderer,
            workspace: Some(&dir.0),
            held: &[],
            settle: &cancelled,
        },
    ));
    assert_eq!(e.code(), ErrorCode::Cancelled, "{e:?}");
    assert_eq!(bench.renderer.sent().len(), 1);
    assert!(!to.exists());
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
}

#[test]
fn a_camera_whose_eye_and_target_round_to_one_point_is_refused_naming_it() {
    // 1.7e-6 apart, so it passes its check; written to the nanometre, both are 0, and a
    // camera at its own target makes no view: refused, nothing sent.
    let bench = Bench::of(EXAMPLE);
    let c = Camera {
        projection: Projection::Orthographic { view_height: 1.0 },
        eye: [-4.9e-7; 3],
        target: [4.9e-7; 3],
        up: [0.0, 0.0, 1.0],
    };
    assert!(c.check().is_ok());
    let e = refused(bench.render(&camera(c, 100, 100)));
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::InvalidInput, Some("camera")),
        "{e:?}"
    );
    assert!(bench.renderer.sent().is_empty());
}

#[test]
fn every_reason_a_render_is_refused_for_its_values_is_found() {
    let bench = Bench::of(EXAMPLE);
    let mut request = view("front", 0, 0);
    request.highlight = vec!["k7f2qx3m.box.Face1".to_owned(); 1001];
    let problems = bench.render(&request).unwrap_err();
    let fields: Vec<Option<&str>> = problems.errors().iter().map(|e| e.parameter()).collect();
    assert_eq!(fields, [Some("width"), Some("height"), Some("highlight")]);
}

#[test]
fn a_broken_highlights_candidates_are_the_likeliest_by_m2_8s_rule() {
    // The cut's side face named with a box face it isn't made from: no element of the Hole
    // has that name. Its candidates, by M2.8's rule: those sharing the most of what it's
    // made from (each of these shares one), the name's own feature's first, then by name:
    // the cut's face of the cylinder alone, then the box's top.
    let bench = Bench::of(EXAMPLE);
    let mut request = view("front", 100, 100);
    request.highlight =
        vec!["m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face1, I k7f2qx3m.box.Face6)".to_owned()];
    let e = refused(bench.render(&request));
    assert_eq!(e.code(), ErrorCode::ReferenceBroken, "{e:?}");
    assert_eq!(
        e.candidates(),
        [
            "m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face1)",
            "k7f2qx3m.box.Face6"
        ],
        "{e:?}"
    );
    // An edge's name: only edges.
    request.highlight =
        vec!["m3tq8v0z.cut.Edge(I a91cw0de.cylinder.Edge1, I k7f2qx3m.box.Face6)".to_owned()];
    let e = refused(bench.render(&request));
    assert!(!e.candidates().is_empty(), "{e:?}");
    assert!(e.candidates().iter().all(|c| c.contains(".Edge")), "{e:?}");
}
