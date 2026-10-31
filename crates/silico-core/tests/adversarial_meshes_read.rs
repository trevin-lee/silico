//! /break-it on `command::meshes` (docs/design/m5.0-app.md, `meshes`, M5.5) and the view
//! functions it shares with `render` (`asked_view`, `camera_for`, `eye_outside`, `meshed`),
//! on the fake kernel: hostile `features` lists (sketches, failures, blocked features,
//! duplicates, and a list as long as a request may carry), hostile `held` entries, bodies
//! behind and beside a perspective eye, `serves` checked against what a second read
//! actually meshes at, answers that mustn't depend on the cache's history, a worker that
//! sends a mesh of another shape's faces, and reads cut off by their session.
//!
//! Every expected value is by hand: the fake meshes a triangle a face and two points an
//! edge, its floor 1e-4 of a body's box's diagonal (`mesh_floor`); a mesh's level is the
//! largest power of two no coarser than half a pixel (view height / pixels / 2 in an
//! orthographic view, 2 · depth · tan(fov / 2) / pixels / 2 in a perspective one).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::example::EXAMPLE;
use super::model::WAIT;
use super::sweeps;
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use silico_core::command::meshes::{Held, Meshes, MeshesOn, MeshesRead, meshes};
use silico_core::command::{self, Problems};
use silico_core::document::FeatureId;
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{Generation, Kernel, Op, Primitive};
use silico_core::meshes::{MESH_CACHE_BYTES, MeshCache, mesh_level};
use silico_core::recompute::{FeatureOutcome, Key, Model, Recompute};
use silico_core::views::{NamedView, half_pixel_at, meshing_depth};
use silico_core::{Error, ErrorCode};
use silico_protocol::{BooleanOp, Camera, Mesh, MeshAt, Projection, Reply};

/// A model rebuilt on its own fake kernel (configured first), with a mesh cache.
struct Bench {
    kernel: Arc<FakeKernel>,
    model: Arc<Model>,
    meshes: MeshCache<Key>,
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

    /// The deflections the kernel has been asked to mesh at, in turn.
    fn meshed(&self) -> Vec<f64> {
        self.kernel
            .calls()
            .iter()
            .filter_map(|c| match &c.op {
                Op::Tessellate { deflection, .. } => Some(*deflection),
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

/// Orthographic, from −y onto (20, 10, 5), the example's centre, `view_height` high.
fn front(view_height: f64) -> Camera {
    Camera {
        projection: Projection::Orthographic { view_height },
        eye: [20.0, -100.0, 5.0],
        target: [20.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    }
}

/// A read of every body by `camera` (owned or borrowed), `width` by `height`.
fn of(camera: impl std::borrow::Borrow<Camera>, width: u32, height: u32) -> Meshes {
    Meshes {
        view: None,
        camera: Some(camera.borrow().clone()),
        width,
        height,
        features: None,
        held: vec![],
    }
}

/// 2^k, exactly, from its bits (a normal number's exponent, biased by 1023).
fn pow2(k: i32) -> f64 {
    f64::from_bits(u64::try_from(k + 1023).unwrap() << 52)
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
fn refused(result: Result<MeshesRead, Problems>) -> Error {
    let problems = result.unwrap_err();
    assert_eq!(problems.errors().len(), 1, "{problems:?}");
    problems.errors()[0].clone()
}

/// What a read says of each body, the mesh's arrays aside: what two reads of one view
/// must agree on.
fn said(read: &MeshesRead) -> Vec<String> {
    read.bodies
        .iter()
        .map(|b| {
            format!(
                "{} {:?} {:?} level {:e} deflection {:e} serves {:?} held {} faces {:?} edges {:?} mesh {:?}",
                b.feature,
                b.body,
                b.key,
                b.level,
                b.deflection,
                b.serves,
                b.held,
                b.faces,
                b.edges,
                b.mesh.as_deref()
            )
        })
        .collect()
}

// --- `features` -------------------------------------------------------------------------

#[test]
fn an_empty_features_list_meshes_nothing_and_a_named_view_still_fits_every_body() {
    let bench = Bench::of(EXAMPLE);
    let read = bench
        .read(&Meshes {
            features: Some(vec![]),
            ..of(front(80.0), 100, 100)
        })
        .unwrap();
    assert!(read.bodies.is_empty());
    assert_eq!(read.camera.camera(), front(80.0));
    // A named view fits the model's bodies, asked or not: front of (0, 0, −1)-(40, 20, 11)
    // centres on (20, 10, 5).
    let read = bench
        .read(&Meshes {
            view: NamedView::named("front"),
            camera: None,
            features: Some(vec![]),
            ..of(front(80.0), 100, 100)
        })
        .unwrap();
    assert!(read.bodies.is_empty());
    assert_eq!(read.camera.camera().target, [20.0, 10.0, 5.0]);
    assert!(bench.meshed().is_empty());
}

#[test]
fn a_sketch_named_in_features_is_refused_naming_its_place_and_itself_and_nothing_is_meshed() {
    let text = sweeps::document(&[
        &sweeps::block("b0000001"),
        &sweeps::empty_sketch("s0000001", "Sketch", "xy"),
    ]);
    let bench = Bench::of(&text);
    let e = refused(bench.read(&Meshes {
        features: Some(vec![id("b0000001"), id("s0000001")]),
        ..of(front(80.0), 100, 100)
    }));
    assert_eq!(
        (e.code(), e.parameter(), e.feature()),
        (
            ErrorCode::InvalidInput,
            Some("features.1"),
            Some("s0000001")
        ),
        "{e:?}"
    );
    assert!(bench.meshed().is_empty());
    // With the sketch left out, the block is read: body 1.
    let read = bench
        .read(&Meshes {
            features: Some(vec![id("b0000001")]),
            ..of(front(80.0), 100, 100)
        })
        .unwrap();
    assert_eq!(read.bodies.len(), 1);
    assert_eq!(read.bodies[0].body, Some(1));
}

/// The example with its cut refused by the kernel.
fn cut_refused() -> Bench {
    Bench::with(EXAMPLE, |k| {
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
    })
}

#[test]
fn a_failed_cut_named_is_its_failure_and_left_out_its_inputs_are_the_bodies() {
    let bench = cut_refused();
    let e = refused(bench.read(&Meshes {
        features: Some(vec![id("m3tq8v0z")]),
        ..of(front(80.0), 100, 100)
    }));
    assert_eq!(
        (e.code(), e.feature()),
        (ErrorCode::KernelFailed, Some("m3tq8v0z")),
        "{e:?}"
    );
    // Every body: the block and the pin, 1 and 2 in timeline order (a failed feature
    // consumes nothing), each with its own faces, 6 and 3 on the fake.
    let read = bench.read(&of(front(80.0), 100, 100)).unwrap();
    let got: Vec<(String, Option<u32>, usize)> = read
        .bodies
        .iter()
        .map(|b| (b.feature.to_string(), b.body, b.faces.len()))
        .collect();
    assert_eq!(
        got,
        [
            ("k7f2qx3m".to_owned(), Some(1), 6),
            ("a91cw0de".to_owned(), Some(2), 3)
        ]
    );
    assert_eq!(
        read.bodies[1].faces[0].name.to_string(),
        "a91cw0de.cylinder.Face1"
    );
}

#[test]
fn a_feature_blocked_on_a_failed_one_is_refused_naming_it_and_what_it_waits_on() {
    // The pin refused: the cut, which uses it, is blocked; the block is the one body.
    let bench = Bench::with(EXAMPLE, |k| {
        k.refuse(|op| {
            matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
                .then(|| Error::new(ErrorCode::KernelFailed, "no pin").with_suggestion("x"))
        });
    });
    let problems = bench
        .read(&Meshes {
            features: Some(vec![id("a91cw0de"), id("m3tq8v0z"), id("k7f2qx3m")]),
            ..of(front(80.0), 100, 100)
        })
        .unwrap_err();
    let got: Vec<(ErrorCode, Option<&str>)> = problems
        .errors()
        .iter()
        .map(|e| (e.code(), e.feature()))
        .collect();
    assert_eq!(
        got,
        [
            (ErrorCode::KernelFailed, Some("a91cw0de")),
            (ErrorCode::InvalidInput, Some("m3tq8v0z")),
        ],
        "{problems:?}"
    );
    assert!(
        problems.errors()[1].message().contains("a91cw0de"),
        "{problems:?}"
    );
    assert!(bench.meshed().is_empty());
}

#[test]
fn every_reason_a_read_is_refused_is_found_in_the_requests_order() {
    let text = sweeps::document(&[
        &sweeps::block("b0000001"),
        &sweeps::empty_sketch("s0000001", "Sketch", "xy"),
    ]);
    let bench = Bench::of(&text);
    let problems = bench
        .read(&Meshes {
            view: NamedView::named("front"),
            camera: Some(front(80.0)),
            width: 4097,
            height: 0,
            features: Some(vec![
                id("s0000001"),
                id("zzzzzzzz"),
                id("b0000001"),
                id("zzzzzzzz"),
                id("b0000001"),
            ]),
            held: vec![],
        })
        .unwrap_err();
    let parameters: Vec<Option<&str>> = problems.errors().iter().map(|e| e.parameter()).collect();
    assert_eq!(
        parameters,
        [
            Some("camera"),
            Some("width"),
            Some("height"),
            Some("features.0"),
            Some("features.1"),
            Some("features.3"),
            Some("features.4"),
        ],
        "{problems:?}"
    );
    assert_eq!(problems.total(), 7);
    assert!(bench.meshed().is_empty());
}

/// `n` distinct feature IDs no document of these tests has: `y` and seven of the alphabet.
fn unknown_ids(n: usize) -> Vec<FeatureId> {
    const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    (0..n)
        .map(|mut i| {
            let mut text = vec![b'y'];
            for _ in 0..7 {
                text.push(ALPHABET[i % 32]);
                i /= 32;
            }
            FeatureId::new(std::str::from_utf8(&text).unwrap()).unwrap()
        })
        .collect()
}

// Broke, before the fix: a `features` list of a thousand IDs the document hasn't lists a thousand errors.
// Every other command keeps M4.1's rule (`Problems::push_with`: "a request with a million
// reasons it's refused builds a hundred errors, not a million"): the first
// PROBLEMS_LISTED (100) listed, all counted. `chosen` collects a plain `Vec<Error>` and
// `Problems::of` lists them all, so a 64 MiB request (about six million IDs) builds and
// answers six million errors.
#[test]
fn a_thousand_unknown_features_list_the_first_hundred_and_count_them_all() {
    let bench = Bench::of(EXAMPLE);
    let problems = bench
        .read(&Meshes {
            features: Some(unknown_ids(1000)),
            ..of(front(80.0), 100, 100)
        })
        .unwrap_err();
    assert_eq!(problems.total(), 1000);
    assert_eq!(problems.errors().len(), 100, "every error listed");
    assert_eq!(problems.errors()[99].parameter(), Some("features.99"));
}

// Broke, before the fix: refusing a long `features` list takes time quadratic in its length: each entry
// is checked for a repeat by a scan of every entry before it (`features[..place]
// .contains(id)`), and looked for by scans of the model's features and bodies. 200,000 IDs
// (2.2 MB of a request's 64 MiB) are 2 · 10¹⁰ comparisons: in a test build, 100,000 took
// 30 s and 200,000 about four times as long, past the tests' one WAIT (60 s); a 64 MiB
// list (six million IDs) would take about a day. A set of the IDs seen makes it n · log n.
#[test]
fn two_hundred_thousand_unknown_features_are_refused_within_the_tests_wait() {
    let bench = Arc::new(Bench::of(EXAMPLE));
    let features = unknown_ids(200_000);
    let (send, receive) = std::sync::mpsc::channel();
    let reading = Arc::clone(&bench);
    std::thread::spawn(move || {
        let started = Instant::now();
        let refused = reading
            .read(&Meshes {
                features: Some(features),
                ..of(front(80.0), 100, 100)
            })
            .map(|_| ());
        let _ = send.send((refused, started.elapsed()));
    });
    let (refused, took) = receive
        .recv_timeout(WAIT)
        .expect("a list of 200,000 IDs still being checked after a minute");
    let problems = refused.unwrap_err();
    assert_eq!(problems.total(), 200_000, "{took:?}");
}

// --- `held` -----------------------------------------------------------------------------

#[test]
fn held_entries_of_nonsense_deflections_and_keys_never_match_and_never_refuse() {
    let bench = Bench::of(EXAMPLE);
    let key = bench.key("m3tq8v0z");
    let other = bench.key("k7f2qx3m");
    let mut held: Vec<Held> = [
        f64::NAN,
        -f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        0.0,
        -0.0,
        -0.03125,
        0.03125_f64.next_up(),
        0.03125_f64.next_down(),
        f64::MIN_POSITIVE,
        f64::MAX,
    ]
    .into_iter()
    .map(|deflection| Held { key, deflection })
    .collect();
    // The consumed box's key at the cut's level: the box isn't read, so it holds nothing.
    held.push(Held {
        key: other,
        deflection: 0.03125,
    });
    // Ten thousand repeats of one entry.
    held.extend(std::iter::repeat_n(
        Held {
            key: other,
            deflection: f64::NAN,
        },
        10_000,
    ));
    let read = bench
        .read(&Meshes {
            held,
            ..of(front(80.0), 1000, 1000)
        })
        .unwrap();
    let body = &read.bodies[0];
    assert!(!body.held);
    assert_eq!(body.mesh.as_ref().unwrap().face_triangles.len(), 9);
    assert_eq!(body.deflection, 0.03125);
}

#[test]
fn a_floor_mesh_held_at_its_floor_is_held_whatever_level_asks_it() {
    // 1 mm over 1,000 pixels asks 2⁻¹¹, under the cut's floor, √2144 · 1e-4: meshed at the
    // floor. Held at the floor's deflection, a read at 2⁻¹² (0.5 mm) is held too.
    let bench = Bench::of(EXAMPLE);
    let first = bench.read(&of(front(1.0), 1000, 1000)).unwrap();
    let floor = first.bodies[0].deflection;
    assert_eq!(floor, 2144f64.sqrt() * 1e-4);
    let read = bench
        .read(&Meshes {
            held: vec![Held {
                key: bench.key("m3tq8v0z"),
                deflection: floor,
            }],
            ..of(front(0.5), 1000, 1000)
        })
        .unwrap();
    let body = &read.bodies[0];
    assert!(body.held && body.mesh.is_none());
    assert_eq!((body.level, body.deflection), (1.0 / 4096.0, floor));
    assert_eq!(body.serves, [0.0, 0.0078125]);
    assert_eq!(bench.meshed(), [1.0 / 2048.0]);
}

#[test]
fn two_equal_boxes_share_a_key_and_a_mesh_and_each_has_its_own_names_held_or_not() {
    // Two boxes of one size at one place: one step's key, so holding it holds both; each
    // body's faces are its own feature's names, held or not (the design: a key covers the
    // geometry, not the names).
    let bench = Bench::of(&boxes(&[
        ("aaaaaaaa", [0.0; 3], [10.0; 3]),
        ("bbbbbbbb", [0.0; 3], [10.0; 3]),
    ]));
    let key = bench.key("aaaaaaaa");
    assert_eq!(key, bench.key("bbbbbbbb"));
    let camera = Camera {
        projection: Projection::Orthographic { view_height: 80.0 },
        eye: [5.0, -100.0, 5.0],
        target: [5.0, 5.0, 5.0],
        up: [0.0, 0.0, 1.0],
    };
    for held in [
        vec![],
        vec![Held {
            key,
            deflection: 0.03125,
        }],
    ] {
        let holding = !held.is_empty();
        let read = bench
            .read(&Meshes {
                held,
                ..of(&camera, 1000, 1000)
            })
            .unwrap();
        assert_eq!(read.bodies.len(), 2);
        for (body, (feature, number)) in read.bodies.iter().zip([("aaaaaaaa", 1), ("bbbbbbbb", 2)])
        {
            assert_eq!(body.feature, id(feature));
            assert_eq!(body.body, Some(number));
            assert_eq!(body.held, holding);
            assert_eq!(body.mesh.is_none(), holding);
            let names: Vec<String> = body.faces.iter().map(|f| f.name.to_string()).collect();
            let want: Vec<String> = (1..=6).map(|i| format!("{feature}.box.Face{i}")).collect();
            assert_eq!(names, want);
        }
    }
}

// --- `serves` and the levels -------------------------------------------------------------

#[test]
fn half_pixels_at_and_an_ulp_under_a_power_of_two_take_their_levels_by_hand() {
    // 2⁻⁵ = 0.03125 exactly: a view of 62.5 mm over 1,000 pixels. An ulp under, the level
    // halves. Both above the cut's floor, 0.00463.
    let bench = Bench::of(EXAMPLE);
    let read = bench.read(&of(front(62.5), 1000, 1000)).unwrap();
    assert_eq!(read.bodies[0].level, 0.03125);
    assert_eq!(read.bodies[0].serves, [0.03125, 0.0625]);
    // 62.499999 mm (a nanometre under, as the answer writes lengths): 0.0312499995.
    let read = bench.read(&of(front(62.499_999), 1000, 1000)).unwrap();
    assert_eq!(read.bodies[0].level, 0.015625);
    assert_eq!(read.bodies[0].serves, [0.015625, 0.03125]);
    assert_eq!(bench.meshed(), [0.03125, 0.015625]);
}

#[test]
fn a_perspective_view_gives_each_body_its_own_level_and_meshes_those_behind_and_beside_it() {
    // From (0, −100, 0) along +y, a 90° field of view, 1,000 pixels high: a half pixel of
    // depth · tan 45° / 1,000. Four 10 mm cubes:
    // - aaaaaaaa at y 0: depth 100, half pixel 0.1, level 2⁻⁴;
    // - bbbbbbbb at y 50: depth 150, half pixel 0.15, level 2⁻³;
    // - cccccccc at y −300 (wholly behind the eye): meshed for its distance from the eye,
    //   which is within its x and z, to its near face at y −290: 190, half pixel 0.19,
    //   level 2⁻³ (Main's ruling, where the depth along the view, 0, gave the floor);
    // - dddddddd at x 10,000 (far outside the view): depth 100, level 2⁻⁴, never culled.
    let bench = Bench::of(&boxes(&[
        ("aaaaaaaa", [-5.0, 0.0, -5.0], [10.0; 3]),
        ("bbbbbbbb", [-5.0, 50.0, -5.0], [10.0; 3]),
        ("cccccccc", [-5.0, -300.0, -5.0], [10.0; 3]),
        ("dddddddd", [10_000.0, 0.0, -5.0], [10.0; 3]),
    ]));
    let camera = Camera {
        projection: Projection::Perspective {
            field_of_view: 90.0,
        },
        eye: [0.0, -100.0, 0.0],
        target: [0.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
    };
    let read = bench.read(&of(camera, 1000, 1000)).unwrap();
    let got: Vec<(String, f64, f64, [f64; 2])> = read
        .bodies
        .iter()
        .map(|b| (b.feature.to_string(), b.level, b.deflection, b.serves))
        .collect();
    assert_eq!(
        got,
        [
            ("aaaaaaaa".to_owned(), 0.0625, 0.0625, [0.0625, 0.125]),
            ("bbbbbbbb".to_owned(), 0.125, 0.125, [0.125, 0.25]),
            ("cccccccc".to_owned(), 0.125, 0.125, [0.125, 0.25]),
            ("dddddddd".to_owned(), 0.0625, 0.0625, [0.0625, 0.125]),
        ]
    );
    // The client's own rule finds every body's half pixel within its `serves`.
    for b in &read.bodies {
        let h = half_pixel_at(
            &read.camera.camera(),
            1000,
            meshing_depth(&read.camera.camera(), b.bounds.unwrap()),
        )
        .unwrap();
        assert!(b.serves[0] <= h && h < b.serves[1], "{}: {h}", b.feature);
    }
    // No eye was inside a box, so nothing was classified.
    assert!(
        !bench
            .kernel
            .calls()
            .iter()
            .any(|c| matches!(c.op, Op::Classify { .. }))
    );
}

#[test]
fn a_perspective_eye_inside_a_body_asked_for_is_refused_naming_it() {
    let bench = Bench::of(&boxes(&[
        ("aaaaaaaa", [0.0; 3], [10.0; 3]),
        ("bbbbbbbb", [100.0, 0.0, 0.0], [10.0; 3]),
    ]));
    let inside_b = Camera {
        projection: Projection::Perspective {
            field_of_view: 40.0,
        },
        eye: [105.0, 5.0, 5.0],
        target: [0.0, 5.0, 5.0],
        up: [0.0, 0.0, 1.0],
    };
    let e = refused(bench.read(&of(&inside_b, 100, 100)));
    assert_eq!(
        (e.code(), e.parameter(), e.feature()),
        (
            ErrorCode::InvalidInput,
            Some("camera.eye"),
            Some("bbbbbbbb")
        ),
        "{e:?}"
    );
    assert!(bench.meshed().is_empty());
    // On its face counts as inside.
    let on_face = Camera {
        eye: [100.0, 5.0, 5.0],
        ..inside_b
    };
    let e = refused(bench.read(&of(on_face, 100, 100)));
    assert_eq!(e.feature(), Some("bbbbbbbb"), "{e:?}");
}

// Broke, before the fix: what a read answers of one view depends on what the shared cache has seen. A
// body whose floor is exactly a power of two (a 225 × 300 × 500 box: diagonal 625, floor
// 0.0625), read at a level equal to its floor (a half pixel of 0.07), answers `serves`
// [0.0625, 0.125] on a fresh cache, as the kernel met the level without raising it, but
// [0, 0.125] once any read (another session's, sharing the kernel's cache) has asked it
// under its floor and recorded the floor (`on.meshes.floor(&key) == Some(deflection)`).
// Both are true of the mesh, but one request's answer changes with other sessions'
// history (CLAUDE.md: nothing that reaches results may depend on scheduling).
#[test]
fn one_view_of_a_body_whose_floor_is_a_power_of_two_answers_alike_whatever_the_cache_has_seen() {
    let text = boxes(&[("aaaaaaaa", [0.0; 3], [225.0, 300.0, 500.0])]);
    let camera = |view_height: f64| Camera {
        projection: Projection::Orthographic { view_height },
        eye: [112.5, -1000.0, 250.0],
        target: [112.5, 150.0, 250.0],
        up: [0.0, 0.0, 1.0],
    };
    // 140 mm over 1,000 pixels: a half pixel of 0.07, level 0.0625, the floor itself.
    let bench = Bench::of(&text);
    let fresh = bench.read(&of(camera(140.0), 1000, 1000)).unwrap();
    assert_eq!(
        (fresh.bodies[0].level, fresh.bodies[0].deflection),
        (0.0625, 0.0625)
    );
    // 80 mm: a half pixel of 0.04, level 2⁻⁵, under the floor: meshed at the floor.
    let under = bench.read(&of(camera(80.0), 1000, 1000)).unwrap();
    assert_eq!(under.bodies[0].deflection, 0.0625);
    assert_eq!(under.bodies[0].serves, [0.0, 0.125]);
    let again = bench.read(&of(camera(140.0), 1000, 1000)).unwrap();
    assert_eq!(said(&fresh), said(&again));
}

// Broke, before the fix (#331's format review): a held entry at a level under the body's
// floor, its floor unknown to a fresh cache, was answered held, with no mesh and the level
// as its deflection, without the kernel; a cache that had met the floor answered it not
// held, meshed at the floor. One request, two answers, by what the shared cache had seen.
// Answered without the kernel only when the cache knows the deflection the kernel would use;
// otherwise meshed, and the entry compared with what the kernel used.
#[test]
fn a_held_entry_under_the_floor_is_answered_alike_whatever_the_cache_has_seen() {
    // 1 mm over 1,000 pixels: a half pixel of 0.0005, level 2⁻¹¹, under the cut's floor,
    // 1e-4 of its box's diagonal, √2144 · 1e-4 = 0.0046303.
    let level = pow2(-11);
    let fresh = Bench::of(EXAMPLE);
    let key = fresh.key("m3tq8v0z");
    let held = Meshes {
        held: vec![Held {
            key,
            deflection: level,
        }],
        ..of(front(1.0), 1000, 1000)
    };
    let cold = fresh.read(&held).unwrap();
    // The same request on a cache that has met the floor.
    let warm = Bench::of(EXAMPLE);
    warm.read(&of(front(1.0), 1000, 1000)).unwrap();
    let seen = warm.read(&held).unwrap();
    assert_eq!(said(&cold), said(&seen));
    // Not held: the kernel meshes at the floor, never at the level held.
    let body = &cold.bodies[0];
    assert!(!body.held);
    assert!(body.mesh.is_some());
    assert_eq!(body.level, level);
    assert!(
        (body.deflection - 0.004_630_3).abs() < 1e-7,
        "{}",
        body.deflection
    );
    assert_eq!(body.serves, [0.0, 2.0 * pow2(-8)]);
    assert_eq!(fresh.meshed(), [level]);
}

// The other side of the same rule: a level a mesh was made at as asked is known to be at or
// above the floor, so held there it's answered without the kernel, on a cache that keeps no
// meshes; and a level finer than any made so, on a floor not met, asks the kernel again.
#[test]
fn a_level_made_as_asked_is_known_and_a_finer_one_is_asked_again() {
    let bench = Bench {
        meshes: MeshCache::new(0),
        ..Bench::of(EXAMPLE)
    };
    let key = bench.key("m3tq8v0z");
    let held_at = |view_height: f64, deflection: f64| Meshes {
        held: vec![Held { key, deflection }],
        ..of(front(view_height), 1000, 1000)
    };
    // 80 mm: 2⁻⁵, made as asked.
    bench.read(&of(front(80.0), 1000, 1000)).unwrap();
    assert!(bench.read(&held_at(80.0, pow2(-5))).unwrap().bodies[0].held);
    // 160 mm: 2⁻⁴, coarser than one made as asked, so known too.
    assert!(bench.read(&held_at(160.0, pow2(-4))).unwrap().bodies[0].held);
    assert_eq!(bench.meshed(), [pow2(-5)]);
    // 40 mm: 2⁻⁶, finer than any made, the floor not met: meshed to learn it (as asked:
    // above the floor), and then held.
    let finer = bench.read(&held_at(40.0, pow2(-6))).unwrap();
    assert!(finer.bodies[0].held);
    assert_eq!(bench.meshed(), [pow2(-5), pow2(-6)]);
}

// #331, Main's condition on `box: null`: a body the kernel couldn't measure has no box, so
// no depth. In a perspective view it asks the finest level there is, so it's meshed at its
// floor, and its level is that floor, serving [0, 2 · the floor's power of two]; `render`
// asks by the same rule (`level_of`), so a render of the same view meshes nothing more. In
// an orthographic view, depth doesn't matter: its level is the view's.
#[test]
fn an_unmeasured_body_has_no_box_and_is_meshed_as_render_meshes_it() {
    use silico_core::command::render::{Render, RenderOn, render};
    use silico_core::fake::FakeRenderer;
    let bench = Bench::with(EXAMPLE, |k| {
        k.refuse(|op| {
            matches!(op, Op::Summarize { .. }).then(|| {
                Error::new(ErrorCode::KernelFailed, "the measure failed")
                    .with_suggestion("measure again")
            })
        });
    });
    let perspective = Camera {
        projection: Projection::Perspective {
            field_of_view: 30.0,
        },
        eye: [20.0, -100.0, 5.0],
        target: [20.0, 10.0, 5.0],
        up: [0.0, 0.0, 1.0],
    };
    let read = bench.read(&of(&perspective, 1000, 1000)).unwrap();
    let body = &read.bodies[0];
    assert_eq!(body.bounds, None);
    // The fake's floor: 1e-4 of the cut's diagonal, √2144 · 1e-4 = 0.0046303, its level as
    // well as its deflection; its power of two 2⁻⁸.
    assert!(
        (body.deflection - 0.004_630_3).abs() < 1e-7,
        "{}",
        body.deflection
    );
    assert_eq!(body.level.to_bits(), body.deflection.to_bits());
    assert_eq!(body.serves, [0.0, 2.0 * pow2(-8)]);
    // Asked of the kernel as the finest there is, which it meshes at the floor.
    assert_eq!(bench.meshed(), [f64::MIN_POSITIVE]);
    // A render of the same view asks the same level: the floor's mesh, already kept.
    let renderer = FakeRenderer::new();
    render(
        &bench.model,
        &Render {
            view: None,
            camera: Some(perspective),
            width: 1000,
            height: 1000,
            highlight: vec![],
            to: None,
            replace: false,
        },
        &RenderOn {
            kernel: &bench.kernel,
            meshes: &bench.meshes,
            renderer: &renderer,
            workspace: None,
            held: &[],
            settle: &|| Ok(()),
        },
    )
    .unwrap_or_else(|p| panic!("{:?}", p.errors()));
    assert_eq!(renderer.sent().len(), 1);
    assert_eq!(bench.meshed(), [f64::MIN_POSITIVE]);
    // Orthographic: the view's level, 80 mm over 1,000 pixels, 2⁻⁵.
    let read = bench.read(&of(front(80.0), 1000, 1000)).unwrap();
    assert_eq!(read.bodies[0].bounds, None);
    assert_eq!(
        (read.bodies[0].level, read.bodies[0].deflection),
        (pow2(-5), pow2(-5))
    );
}

/// An orthographic camera on the example of a half pixel near a power of two, its height,
/// or one near the cut's floor (0.00463), and a height of 1 to 4,096 pixels.
fn an_orthographic_view() -> impl Strategy<Value = (Camera, u32)> {
    let half_pixel = prop_oneof![
        // A power of two, an ulp each side, and between.
        (
            -12i32..6,
            prop_oneof![Just(1.0), Just(1.0 + 1e-12), Just(1.0 - 1e-12), 1.0..2.0f64]
        )
            .prop_map(|(k, f)| pow2(k) * f),
        // Around the floor.
        (0.5..2.0f64).prop_map(|f| 2144f64.sqrt() * 1e-4 * f),
    ];
    let height = prop_oneof![Just(1u32), Just(4096u32), 1u32..=4096];
    (half_pixel, height).prop_filter_map("a view within the limits", |(h, height)| {
        let view_height = h * 2.0 * f64::from(height);
        (1e-6..=1e12)
            .contains(&view_height)
            .then(|| (front(view_height), height))
    })
}

/// Checks a read of the example's one body at `camera`, `height` pixels high, on `bench`:
/// its level and `serves` as the ladder gives them for the half pixel the client's rule
/// computes, and its deflection the level or, under the floor, the floor.
fn holds_to_the_ladder(read: &MeshesRead, height: u32) -> Result<(), TestCaseError> {
    let floor = 2144f64.sqrt() * 1e-4;
    let camera = read.camera.camera();
    let body = &read.bodies[0];
    let h = half_pixel_at(
        &camera,
        height,
        meshing_depth(&camera, body.bounds.unwrap()),
    )
    .unwrap();
    let level = mesh_level(h).unwrap();
    prop_assert_eq!(body.level, level, "half pixel {}", h);
    prop_assert!(
        body.serves[0] <= h && h < body.serves[1],
        "{} {:?}",
        h,
        body.serves
    );
    if level < floor {
        prop_assert_eq!(body.deflection, floor);
        prop_assert_eq!(body.serves, [0.0, 2.0 * mesh_level(floor).unwrap()]);
    } else {
        prop_assert_eq!(body.deflection, level);
        prop_assert_eq!(body.serves, [level, 2.0 * level]);
    }
    prop_assert_eq!(body.faces.len(), 9);
    prop_assert_eq!(body.edges.len(), 15);
    Ok(())
}

#[test]
fn every_orthographic_views_level_and_serves_hold_to_the_ladder_and_a_view_inside_serves_reads_the_same_mesh()
 {
    // One bench through every case, so its cache has seen every earlier view; each read
    // checked against the ladder by hand, and against a read of a fresh bench, so the
    // answer is the view's alone. Then a second view: inside the first's `serves`, it's
    // the same mesh, asked of no kernel; outside, another deflection.
    let shared = Bench::of(EXAMPLE);
    let mut runner = TestRunner::new(Config {
        cases: 1000,
        failure_persistence: None,
        ..Config::default()
    });
    runner
        .run(
            &(an_orthographic_view(), an_orthographic_view()),
            |((first, first_height), (second, second_height))| {
                let read = shared
                    .read(&of(&first, first_height, first_height))
                    .map_err(|p| TestCaseError::fail(format!("{p:?}")))?;
                holds_to_the_ladder(&read, first_height)?;
                let fresh = Bench::of(EXAMPLE)
                    .read(&of(&first, first_height, first_height))
                    .unwrap();
                prop_assert_eq!(said(&read), said(&fresh));
                let asked = shared.meshed().len();
                let next = shared
                    .read(&of(second, second_height, second_height))
                    .map_err(|p| TestCaseError::fail(format!("{p:?}")))?;
                holds_to_the_ladder(&next, second_height)?;
                let camera = next.camera.camera();
                let h = half_pixel_at(&camera, second_height, 0.0).unwrap();
                let [low, high] = read.bodies[0].serves;
                let inside = low <= h && h < high;
                prop_assert_eq!(
                    next.bodies[0].deflection == read.bodies[0].deflection,
                    inside,
                    "first serves {:?}, second's half pixel {}",
                    read.bodies[0].serves,
                    h
                );
                if inside {
                    prop_assert_eq!(shared.meshed().len(), asked, "a mesh served asked again");
                }
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn reads_of_many_views_at_once_answer_as_each_alone_does() {
    // Eight threads, each reading a view of its own on one bench; each answer as a fresh
    // bench's of the same view.
    let views: Vec<f64> = vec![80.0, 1.0, 0.5, 9.26, 9.27, 62.5, 62.499_999, 1e6];
    let shared = Arc::new(Bench::of(EXAMPLE));
    for _ in 0..5 {
        let threads: Vec<_> = views
            .iter()
            .map(|&view_height| {
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || {
                    said(&shared.read(&of(front(view_height), 1000, 1000)).unwrap())
                })
            })
            .collect();
        for (thread, &view_height) in threads.into_iter().zip(&views) {
            let got = thread.join().unwrap();
            let alone = said(
                &Bench::of(EXAMPLE)
                    .read(&of(front(view_height), 1000, 1000))
                    .unwrap(),
            );
            assert_eq!(got, alone, "{view_height}");
        }
    }
}

#[test]
fn the_largest_and_smallest_views_answer_finite_levels_and_serves() {
    // 1e12 mm over one pixel: a half pixel of 5e11, level 2³⁸; the least view, 1e-6 mm over
    // 4,096: 1.2e-10, under the floor.
    let bench = Bench::of(EXAMPLE);
    let read = bench.read(&of(front(1e12), 1, 1)).unwrap();
    let body = &read.bodies[0];
    assert_eq!(body.level, pow2(38));
    assert_eq!(body.serves, [pow2(38), pow2(39)]);
    let read = bench.read(&of(front(1e-6), 4096, 4096)).unwrap();
    let body = &read.bodies[0];
    assert_eq!(body.level, pow2(-33));
    assert_eq!(body.deflection, 2144f64.sqrt() * 1e-4);
    assert_eq!(body.serves, [0.0, 0.0078125]);
}

// --- The kernel -------------------------------------------------------------------------

#[test]
fn a_mesh_the_kernel_fails_names_its_body_and_poisons_nothing() {
    let failing = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&failing);
    let bench = Bench::with(EXAMPLE, move |k| {
        k.refuse(move |op| {
            (matches!(op, Op::Tessellate { .. }) && flag.load(Ordering::SeqCst))
                .then(|| Error::new(ErrorCode::KernelFailed, "no mesh").with_suggestion("x"))
        });
    });
    let e = refused(bench.read(&of(front(80.0), 1000, 1000)));
    assert_eq!(
        (e.code(), e.feature()),
        (ErrorCode::KernelFailed, Some("m3tq8v0z")),
        "{e:?}"
    );
    failing.store(false, Ordering::SeqCst);
    let read = bench.read(&of(front(80.0), 1000, 1000)).unwrap();
    assert_eq!(read.bodies[0].faces.len(), 9);
}

/// A mesh of `faces` faces of a triangle each and `edges` edges of two points each, as a
/// worker could send it whatever the shape is: it holds together.
fn faces_and_edges(faces: usize, edges: usize) -> Mesh {
    Mesh {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        normals: vec![[0.0, -1.0, 0.0]; 3],
        triangles: vec![[0, 1, 2]; faces],
        face_triangles: vec![1; faces],
        edge_points: vec![[0.0, 0.0, 0.0]; 2 * edges],
        edge_point_counts: vec![2; edges],
        valid: None,
    }
}

/// The example, whose cut has 9 faces and 15 edges, on a kernel whose meshes are `mesh`
/// while `wrong` is set, at the deflection asked.
fn meshing_to(mesh: Mesh, wrong: Arc<AtomicBool>) -> Bench {
    Bench::with(EXAMPLE, move |k| {
        k.answer_instead(move |op| match op {
            Op::Tessellate { deflection, .. } if wrong.load(Ordering::SeqCst) => {
                Some(Reply::MeshAt(MeshAt {
                    mesh: mesh.clone(),
                    deflection: *deflection,
                }))
            }
            _ => None,
        });
    })
}

// Broke, before the fix: a worker reply of a mesh with more faces than the solid has (10 for the cut's 9)
// is refused as Silico's own bug (`internal`, "m3tq8v0z's element map names fewer Faces
// than its mesh holds", "this is a bug in Silico, not in the request: report it"), though
// it's the worker that's at fault: every other inconsistent reply is `protocol_error`,
// "start a new kernel worker" (`checked_mesh`).
#[test]
fn a_worker_mesh_of_more_faces_than_the_solid_is_the_workers_fault() {
    let bench = meshing_to(faces_and_edges(10, 15), Arc::new(AtomicBool::new(true)));
    let e = refused(bench.read(&of(front(80.0), 1000, 1000)));
    assert_eq!(
        (e.code(), e.feature()),
        (ErrorCode::ProtocolError, Some("m3tq8v0z")),
        "{e:?}"
    );
}

// Broke, before the fix: the same reply poisons the shared mesh cache: `meshed` puts the worker's mesh in
// the cache before `named` checks it against the element map, so every later read of that
// body at that level (and every `render` of it, which shares the cache) gets the bad mesh
// back without asking the kernel, now well again, and `meshes` is refused the same way
// until the cache happens to drop it. No advice the error gives (report a bug, or a new
// worker) helps, and the session can't recover on its own.
#[test]
fn after_a_worker_mesh_of_another_shapes_faces_the_next_read_on_a_well_kernel_recovers() {
    let wrong = Arc::new(AtomicBool::new(true));
    let bench = meshing_to(faces_and_edges(10, 15), Arc::clone(&wrong));
    bench.read(&of(front(80.0), 1000, 1000)).unwrap_err();
    wrong.store(false, Ordering::SeqCst);
    let again = bench
        .read(&of(front(80.0), 1000, 1000))
        .unwrap_or_else(|p| panic!("the kernel well, still refused: {p:?}"));
    assert_eq!(again.bodies[0].faces.len(), 9);
    assert_eq!(
        bench.meshed(),
        [0.03125, 0.03125],
        "the bad mesh served from the cache"
    );
}

// Broke, before the fix: a worker reply of a mesh with fewer faces and edges than the solid has (3 and 4
// for the cut's 9 and 15) is answered Ok, the body's faces and edges listed short (only
// Face1 to Face3 named): a client sees a 9-faced solid as three faces, with nothing to say
// the rest are missing. The model holds the solid's counts (its summary's 9 and 15, which
// `render` checks its caps by), so the core can tell the mesh isn't the solid's.
#[test]
fn a_worker_mesh_of_fewer_faces_than_the_solid_is_refused_as_the_workers_fault() {
    let bench = meshing_to(faces_and_edges(3, 4), Arc::new(AtomicBool::new(true)));
    let e = match bench.read(&of(front(80.0), 1000, 1000)) {
        Ok(read) => panic!(
            "answered with {} faces and {} edges of 9 and 15",
            read.bodies[0].faces.len(),
            read.bodies[0].edges.len()
        ),
        Err(problems) => problems.errors()[0].clone(),
    };
    assert_eq!(
        (e.code(), e.feature()),
        (ErrorCode::ProtocolError, Some("m3tq8v0z")),
        "{e:?}"
    );
}

#[test]
fn a_face_the_kernel_couldnt_mesh_on_a_valid_shape_is_named_with_no_triangles() {
    // The cut's nine faces, its fifth a sliver OCCT left out (0 triangles, the shape
    // checked valid): named, with a count of 0, and the rest in order.
    let mut mesh = faces_and_edges(9, 15);
    mesh.face_triangles[4] = 0;
    mesh.triangles.pop();
    mesh.valid = Some(true);
    let bench = meshing_to(mesh, Arc::new(AtomicBool::new(true)));
    let read = bench.read(&of(front(80.0), 1000, 1000)).unwrap();
    let counts: Vec<u32> = read.bodies[0]
        .faces
        .iter()
        .map(|f| f.count.unwrap())
        .collect();
    assert_eq!(counts, [1, 1, 1, 1, 0, 1, 1, 1, 1]);
    let map = bench.model.element_map(&id("m3tq8v0z")).unwrap();
    let fifth = map
        .name(silico_core::kernel::ElementRef {
            kind: silico_core::kernel::ElementKind::Face,
            index: 5,
        })
        .unwrap();
    assert_eq!(read.bodies[0].faces[4].name, fifth);
}

// Broke, before the fix: a meshes read whose session closes while its mesh is being made (its requests
// superseded) answers "the session closed before the render finished", advising "open the
// document again, and render it there": it names a command the client never ran. `meshed`
// and `eye_outside`, shared with `render`, word the close with render's own
// `session_closed`.
#[test]
fn a_meshes_read_whose_session_closes_while_meshing_doesnt_say_it_was_a_render() {
    let bench = Arc::new(Bench::of(EXAMPLE));
    let hold = bench.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    let reading = Arc::clone(&bench);
    let read = std::thread::spawn(move || reading.read(&of(front(80.0), 1000, 1000)));
    assert!(bench.kernel.wait_running(1, WAIT));
    let now = bench.kernel.generation();
    bench.kernel.advance(Generation(now.0 + 1));
    hold.release();
    let e = refused(read.join().unwrap());
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::InvalidInput, Some("session")),
        "{e:?}"
    );
    assert!(
        !e.message().contains("render") && !e.suggestion().unwrap_or("").contains("render"),
        "{e:?}"
    );
    // The cache holds nothing of it, and the next read, in the new generation, meshes.
    let read = bench.read(&of(front(80.0), 1000, 1000)).unwrap();
    assert_eq!(read.bodies[0].faces.len(), 9);
}

#[test]
fn reads_of_one_view_on_two_benches_answer_byte_for_byte_alike() {
    // Determinism: the example and a perspective view of four boxes, read on fresh benches
    // twice (the fake's two workers taking requests in whatever order).
    let four = boxes(&[
        ("aaaaaaaa", [-5.0, 0.0, -5.0], [10.0; 3]),
        ("bbbbbbbb", [-5.0, 50.0, -5.0], [10.0; 3]),
        ("cccccccc", [-5.0, -300.0, -5.0], [10.0; 3]),
        ("dddddddd", [10_000.0, 0.0, -5.0], [10.0; 3]),
    ]);
    let perspective = Camera {
        projection: Projection::Perspective {
            field_of_view: 90.0,
        },
        eye: [0.0, -100.0, 0.0],
        target: [0.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
    };
    for (text, camera) in [(EXAMPLE.to_owned(), front(80.0)), (four, perspective)] {
        let first = said(&Bench::of(&text).read(&of(&camera, 640, 480)).unwrap());
        for _ in 0..10 {
            assert_eq!(
                said(&Bench::of(&text).read(&of(&camera, 640, 480)).unwrap()),
                first
            );
        }
    }
}

#[test]
fn the_ladder_of_a_read_never_waits_on_a_kernel_for_a_level_it_holds() {
    // A level the cache holds asks the kernel nothing (the design's A8): with every mesh
    // request held forever, a read of a view already meshed answers within the wait.
    let bench = Arc::new(Bench::of(EXAMPLE));
    bench.read(&of(front(80.0), 1000, 1000)).unwrap();
    let _hold = bench.kernel.hold(|op| matches!(op, Op::Tessellate { .. }));
    let reading = Arc::clone(&bench);
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(
            reading
                .read(&of(front(100.0), 1000, 1000))
                .map(|r| said(&r)),
        );
    });
    let got = receive
        .recv_timeout(Duration::from_secs(60))
        .expect("a cached level waited on the kernel");
    assert_eq!(got.unwrap().len(), 1);
}
