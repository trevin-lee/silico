//! /break-it on M6.3's pattern rebuild, on the real kernel: references by name that aren't
//! the origin's (a cube's vertical edge as a polar axis, a slanted edge as a direction, a
//! slanted face as a mirror plane off the origin, a disc's rim circle as an axis, a circle
//! as a direction, refused), a face of a mirrored copy as a direction (its outward normal
//! through a reflection), and an original sketched on a face, placed in a later round.
//! Volumes and boxes worked by hand.

use super::close::assert_close;
use super::sweeps::{circles, id, pad, pocket, polygon, rectangle};

use std::f64::consts::{PI, SQRT_2};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use silico_core::document::{Checked, read};
use silico_core::fake::FakeSketches;
use silico_core::kernel::ElementKind;
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};
use silico_protocol::{EdgeCurve, EdgeDescription, FaceDescription, ShapeSummary, Surface};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

const HANG: Duration = Duration::from_secs(300);

fn built(features: &[String]) -> Arc<Model> {
    let pool = KernelPool::start_workers(Path::new(WORKER), 2).unwrap();
    let engine = Recompute::new(pool).with_sketches(FakeSketches::new());
    let text = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"naming\": 6, \"features\": [{}]}}",
        features.join(", ")
    );
    let checked = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    match engine.start(checked).wait_timeout(HANG) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the rebuild didn't finish"),
    }
}

fn summary(model: &Model, feature: &str) -> ShapeSummary {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { built, .. }) => built.summary().unwrap().clone(),
        other => panic!("{feature}: {other:?}"),
    }
}

fn failure(model: &Model, feature: &str) -> Error {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Failed(error)) => error.clone(),
        other => panic!("{feature}: {other:?}"),
    }
}

fn up(n: &str) -> String {
    format!(r#"{{"type": "one_side", "length": {n}, "reversed": false}}"#)
}

const THROUGH: &str = r#"{"type": "through_all", "reversed": true}"#;
const SOLID: &str = r#"{"type": "solid"}"#;

fn direction(along: &str, kind: &str, length: &str, count: u32) -> String {
    format!(
        r#"{{"along": "{along}", "reversed": false, "distribution": {{"type": "{kind}", "length": {length}}}, "count": {count}}}"#
    )
}

fn linear(id: &str, base: &str, copies: &str, directions: &[String]) -> String {
    format!(
        r#"{{"id": "{id}", "type": "linear_pattern", "v": 1, "name": "Pattern", "base": "{base}", "copies": {copies}, "directions": [{}], "skip": []}}"#,
        directions.join(", ")
    )
}

fn polar(id: &str, base: &str, copies: &str, axis: &str, angle: &str, count: u32) -> String {
    format!(
        r#"{{"id": "{id}", "type": "polar_pattern", "v": 1, "name": "Pattern", "base": "{base}", "copies": {copies}, "axis": "{axis}", "reversed": false, "distribution": {{"type": "extent", "angle": {angle}}}, "count": {count}, "skip": []}}"#
    )
}

fn mirror(id: &str, base: &str, copies: &str, plane: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "mirror", "v": 1, "name": "Mirror", "base": "{base}", "copies": {copies}, "plane": "{plane}"}}"#
    )
}

fn of(feature: &str) -> String {
    format!(r#"{{"type": "features", "features": ["{feature}"]}}"#)
}

fn near(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9)
}

/// The first stable name of an edge of `feature`'s solid whose description `test` takes.
fn edge_named(model: &Model, feature: &str, test: impl Fn(&EdgeDescription) -> bool) -> String {
    let map = model.element_map(&id(feature)).unwrap();
    let description = model.description(&id(feature)).unwrap();
    map.iter()
        .filter(|(element, _)| element.kind == ElementKind::Edge)
        .filter(|(element, _)| test(&description.edges[element.index as usize - 1]))
        .flat_map(|(_, names)| names.iter().map(ToString::to_string))
        .find(|name| !name.contains('~'))
        .unwrap_or_else(|| panic!("no such edge of {feature}"))
}

/// The first stable name of a face of `feature`'s solid whose description `test` takes.
fn face_named(model: &Model, feature: &str, test: impl Fn(&FaceDescription) -> bool) -> String {
    let map = model.element_map(&id(feature)).unwrap();
    let description = model.description(&id(feature)).unwrap();
    map.iter()
        .filter(|(element, _)| element.kind == ElementKind::Face)
        .filter(|(element, _)| test(&description.faces[element.index as usize - 1]))
        .flat_map(|(_, names)| names.iter().map(ToString::to_string))
        .find(|name| !name.contains('~'))
        .unwrap_or_else(|| panic!("no such face of {feature}"))
}

/// A 10 mm cube from the origin.
fn cube() -> Vec<String> {
    vec![
        rectangle("s0000001", "xy", [0.0, 0.0], 10.0, 10.0),
        pad("p0000001", None, "s0000001", &up("10")),
    ]
}

/// A right prism 5 high on the triangle (0, 0), (10, 0), (0, 10).
fn wedge() -> Vec<String> {
    vec![
        polygon("s0000001", "xy", &[[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]]),
        pad("p0000001", None, "s0000001", &up("5")),
    ]
}

fn with(mut features: Vec<String>, more: &[String]) -> Vec<String> {
    features.extend(more.iter().cloned());
    features
}

#[test]
fn a_cube_turned_four_times_about_its_own_vertical_edge_makes_one_solid_of_4000() {
    // About the edge at (10, 0), either way up: the quarter turns put cubes on
    // [0, 10]·[0, 10], [10, 20]·[0, 10]... each sharing a face with the next: x in [0, 20],
    // y in [−10, 10], 4000.
    let base = built(&cube());
    let edge = edge_named(&base, "p0000001", |e| {
        e.curve == EdgeCurve::Line
            && [e.start, e.end]
                .iter()
                .all(|p| (p[0] - 10.0).abs() < 1e-9 && p[1].abs() < 1e-9)
    });
    let model = built(&with(
        cube(),
        &[polar("t0000001", "p0000001", SOLID, &edge, "360", 4)],
    ));
    let s = summary(&model, "t0000001");
    assert_close("volume", s.volume, 4000.0, 1e-9);
    let [low, high] = s.bounds.unwrap();
    assert!(near(low, [0.0, -10.0, 0.0]), "{low:?} {edge}");
    assert!(near(high, [20.0, 10.0, 10.0]), "{high:?} {edge}");
}

#[test]
fn a_wedge_mirrored_in_its_own_slanted_face_off_the_origin_makes_the_square_prism() {
    // The hypotenuse face, x + y = 10, through (5, 5) and not the origin: the wedge and its
    // image make the 10 × 10 square, 5 high: 500, in [0, 10]².
    let base = built(&wedge());
    let face = face_named(&base, "p0000001", |f| match f.surface {
        Surface::Plane { normal, .. } => (normal[0] - normal[1]).abs() < 1e-9 && normal[0] > 0.5,
        _ => false,
    });
    let model = built(&with(
        wedge(),
        &[mirror("m0000001", "p0000001", SOLID, &face)],
    ));
    let s = summary(&model, "m0000001");
    assert_close("volume", s.volume, 500.0, 1e-9);
    let [low, high] = s.bounds.unwrap();
    assert!(near(low, [0.0, 0.0, 0.0]), "{low:?}");
    assert!(near(high, [10.0, 10.0, 5.0]), "{high:?}");
}

#[test]
fn a_wedge_copied_10_along_its_slanted_edge_overlaps_by_the_corner_triangle() {
    // Moved 10 along x + y = 10, a step of 10/√2 = a on each axis: the overlap is the right
    // triangle of legs 10 − a, so the union is (100 − (10 − a)² / 2) · 5, either way along.
    let base = built(&wedge());
    let edge = edge_named(&base, "p0000001", |e| {
        e.curve == EdgeCurve::Line
            && e.start[2].abs() < 1e-9
            && e.end[2].abs() < 1e-9
            && [e.start, e.end]
                .iter()
                .all(|p| (p[0] + p[1] - 10.0).abs() < 1e-9)
    });
    let model = built(&with(
        wedge(),
        &[linear(
            "t0000001",
            "p0000001",
            SOLID,
            &[direction(&edge, "spacing", "10", 2)],
        )],
    ));
    let a = 10.0 / SQRT_2;
    let want = (100.0 - (10.0 - a) * (10.0 - a) / 2.0) * 5.0;
    assert_close("volume", summary(&model, "t0000001").volume, want, 1e-9);
}

#[test]
fn a_circle_as_a_direction_is_refused_for_its_kind_pointing_to_a_polar_axis() {
    let features = vec![
        circles("s0000001", "xy", &[([0.0, 0.0], 20.0)]),
        pad("p0000001", None, "s0000001", &up("5")),
    ];
    let base = built(&features);
    let rim = edge_named(&base, "p0000001", |e| {
        matches!(e.curve, EdgeCurve::Circle { .. }) && e.start[2] > 4.0
    });
    let model = built(&with(
        features,
        &[linear(
            "t0000001",
            "p0000001",
            SOLID,
            &[direction(&rim, "spacing", "10", 2)],
        )],
    ));
    let error = failure(&model, "t0000001");
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
    assert_eq!(error.parameter(), Some("directions.0.along"));
    assert!(
        error
            .message()
            .contains("is a circle: a direction is a straight edge or a planar face"),
        "{error}"
    );
    assert!(
        error
            .suggestion()
            .unwrap()
            .contains("a circle is a polar pattern's axis"),
        "{error}"
    );
}

#[test]
fn four_holes_turned_about_a_discs_rim_circle_remove_four_circles() {
    // A disc r 20, 5 thick, a hole r 2 at (10, 0) through it, turned about the rim's axis
    // (z through the origin) four times over a turn: 5 · (400π − 4 · 4π) = 1920π.
    let features = vec![
        circles("s0000001", "xy", &[([0.0, 0.0], 20.0)]),
        pad("p0000001", None, "s0000001", &up("5")),
        circles("s0000002", "xy", &[([10.0, 0.0], 2.0)]),
        pocket("q0000002", "p0000001", "s0000002", THROUGH),
    ];
    let base = built(&features);
    let rim = edge_named(&base, "q0000002", |e| match &e.curve {
        EdgeCurve::Circle { radius, .. } => (radius - 20.0).abs() < 1e-9 && e.start[2] > 4.0,
        _ => false,
    });
    let model = built(&with(
        features,
        &[polar(
            "t0000001",
            "q0000002",
            &of("q0000002"),
            &rim,
            "360",
            4,
        )],
    ));
    assert_close(
        "volume",
        summary(&model, "t0000001").volume,
        1920.0 * PI,
        1e-9,
    );
}

#[test]
fn a_face_of_a_mirrored_copy_points_out_of_the_solid_through_the_reflection() {
    // The cube mirrored about yz: a block x in [−10, 10]. Its x = −10 face, the copy's
    // image of x = 10, points −x: the block copied 20 along it lies on x in [−30, −10],
    // touching, so the result spans x from −30 (an inward normal would put it at +10).
    let doubled = with(cube(), &[mirror("m0000001", "p0000001", SOLID, "yz")]);
    let base = built(&doubled);
    assert_close("doubled", summary(&base, "m0000001").volume, 2000.0, 1e-9);
    let face = face_named(&base, "m0000001", |f| match f.surface {
        Surface::Plane { normal, location } => {
            normal[0].abs() > 0.5 && (f.centroid[0] + 10.0).abs() < 1e-9 && location[0] < 0.0
        }
        _ => false,
    });
    let model = built(&with(
        doubled,
        &[linear(
            "t0000001",
            "m0000001",
            SOLID,
            &[direction(&face, "spacing", "20", 2)],
        )],
    ));
    let s = summary(&model, "t0000001");
    assert_close("volume", s.volume, 4000.0, 1e-9);
    let [low, high] = s.bounds.unwrap();
    assert!(near(low, [-30.0, 0.0, 0.0]), "{low:?} {face}");
    assert!(near(high, [10.0, 10.0, 10.0]), "{high:?} {face}");
}

#[test]
fn bosses_sketched_on_a_blocks_top_face_are_patterned_once_their_face_is_placed() {
    // A 40 × 20 × 10 block; a boss r 3 at (10, 10) sketched on its top face (the face's
    // frame: origin (0, 0, 10), x along +x), padded 5 up; copied 10 along x three times:
    // 8000 + 3 · 9π · 5 = 8000 + 135π.
    let block = vec![
        rectangle("s0000001", "xy", [0.0, 0.0], 40.0, 20.0),
        pad("p0000001", None, "s0000001", &up("10")),
    ];
    let base = built(&block);
    let top = face_named(&base, "p0000001", |f| f.centroid[2] > 9.0);
    let on_top = circles("s0000002", "xy", &[([10.0, 10.0], 3.0)]).replace(
        r#""plane": "xy""#,
        &format!(r#""plane": {{"face": "{top}", "x": "+x", "reversed": false}}"#),
    );
    let features = with(
        block,
        &[
            on_top,
            pad("p0000002", Some("p0000001"), "s0000002", &up("5")),
            linear(
                "t0000001",
                "p0000002",
                &of("p0000002"),
                &[direction("x", "spacing", "10", 3)],
            ),
        ],
    );
    let model = built(&features);
    assert_close(
        "boss",
        summary(&model, "p0000002").volume,
        8000.0 + 45.0 * PI,
        1e-9,
    );
    let s = summary(&model, "t0000001");
    assert_close("volume", s.volume, 8000.0 + 135.0 * PI, 1e-9);
    let [_, high] = s.bounds.unwrap();
    assert_close("top", high[2], 15.0, 1e-12);
}

#[test]
fn a_disc_turned_about_its_own_axis_lands_on_itself_and_is_refused_as_changing_nothing() {
    // Every copy of a disc about its own axis is the disc: the result is the base's volume,
    // so S7 refuses it (kernel_failed, naming the pattern), never a solid passed as built.
    let features = vec![
        circles("s0000001", "xy", &[([0.0, 0.0], 20.0)]),
        pad("p0000001", None, "s0000001", &up("5")),
        polar("t0000001", "p0000001", SOLID, "z", "360", 4),
    ];
    let model = built(&features);
    let error = failure(&model, "t0000001");
    assert_eq!(error.code(), ErrorCode::KernelFailed, "{error}");
    assert_eq!(error.feature(), Some("t0000001"));
}

#[test]
fn a_direction_along_a_face_parallel_to_x_either_way_is_refused_naming_it() {
    // The cube's x = 10 face points +x, its x = 0 face −x: either beside "x" lands copies
    // on copies, refused (decision 11), the face's name its element.
    let base = built(&cube());
    for at in [10.0, 0.0] {
        let face = face_named(&base, "p0000001", |f| match f.surface {
            Surface::Plane { normal, .. } => {
                normal[0].abs() > 0.5 && (f.centroid[0] - at).abs() < 1e-9
            }
            _ => false,
        });
        let model = built(&with(
            cube(),
            &[linear(
                "t0000001",
                "p0000001",
                SOLID,
                &[
                    direction("x", "spacing", "10", 2),
                    direction(&face, "spacing", "10", 2),
                ],
            )],
        ));
        let error = failure(&model, "t0000001");
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{error}");
        assert_eq!(error.parameter(), Some("directions.1.along"));
        assert_eq!(error.elements(), std::slice::from_ref(&face));
    }
}

#[test]
fn nine_hundred_ninety_nine_cubes_off_the_base_are_refused_listing_a_hundred_counting_all() {
    // 1000 cubes 20 apart along x: none touches the next, so every copy is off the base.
    let model = built(&with(
        cube(),
        &[linear(
            "t0000001",
            "p0000001",
            SOLID,
            &[direction("x", "spacing", "20", 1000)],
        )],
    ));
    let error = failure(&model, "t0000001");
    assert_eq!(error.code(), ErrorCode::KernelFailed, "{error}");
    assert_eq!(error.elements().len(), 100);
    assert_eq!(error.elements_total(), 999);
    assert_eq!(error.elements()[0], "1");
    assert_eq!(error.elements()[99], "100");
    assert!(
        error
            .message()
            .contains("instances 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, and 989 more"),
        "{error}"
    );
    let verdicts = model.pattern_verdicts(&id("t0000001")).unwrap();
    assert_eq!(verdicts[0].off_base.len(), 999);
    assert_eq!(verdicts[0].solids, 1000);
}

#[test]
fn a_second_direction_a_tenth_of_a_microradian_off_the_first_makes_the_union_of_its_copies() {
    // A slab whose bottom edge runs to (10, 1e-6): 1e-7 radians off x, past OCCT's 1e-12,
    // so taken. Copies at 10 along x and 10 along that edge lie 1e-6 apart (past 1e-7 mm):
    // the union is three slabs' worth less what the near-twins share, all but slivers of
    // 1e-5 mm²·5, so 3 · V within a millionth, and one solid.
    let slab = vec![
        polygon(
            "s0000001",
            "xy",
            &[[0.0, 0.0], [10.0, 1e-6], [10.0, 10.0], [0.0, 10.0]],
        ),
        pad("p0000001", None, "s0000001", &up("5")),
    ];
    let base = built(&slab);
    let v = summary(&base, "p0000001").volume;
    assert_close("slab", v, (100.0 - 5e-6) * 5.0, 1e-12);
    let edge = edge_named(&base, "p0000001", |e| {
        e.curve == EdgeCurve::Line
            && e.start[2].abs() < 1e-9
            && e.end[2].abs() < 1e-9
            && (e.start[1] - e.end[1]).abs() > 5e-7
            && (e.start[1] - e.end[1]).abs() < 2e-6
    });
    let model = built(&with(
        slab,
        &[linear(
            "t0000001",
            "p0000001",
            SOLID,
            &[
                direction("x", "spacing", "10", 2),
                direction(&edge, "spacing", "10", 2),
            ],
        )],
    ));
    match model.outcome(&id("t0000001")) {
        Some(FeatureOutcome::Built { built, .. }) => {
            let s = built.summary().unwrap();
            assert!(s.valid);
            assert_close("volume", s.volume, 3.0 * v, 1e-6);
        }
        other => panic!("{other:?}"),
    }
}
