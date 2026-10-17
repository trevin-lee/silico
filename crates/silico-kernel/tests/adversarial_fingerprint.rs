//! /break-it on M2.6 fingerprints, on OCCT: every primitive the document makes, placed off
//! the origin, fingerprinted by the kernel and checked against its formulas (the box
//! exactly, to the nanometre; volume and area within the kernel's integration) and against
//! the fake kernel the core's tests trust; edge sizes the gate admits; booleans whose
//! results are empty or curved at the box's edge; and the same document rebuilt in another
//! pool. Expected values are hand calculations.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::f64::consts::PI;
use std::path::Path;
use std::sync::Arc;

use silico_core::document::{Checked, FeatureId, Fingerprint, Length, read};
use silico_core::fake::FakeKernel;
use silico_core::kernel::Kernel;
use silico_core::pool::KernelPool;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};

const WORKER: &str = env!("CARGO_BIN_EXE_silico-kernel-worker");

const HEAD: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": ["#;

fn doc(features: &[&str]) -> String {
    format!("{HEAD}{}]}}", features.join(", "))
}

fn built<K: Kernel + 'static>(kernel: K, text: &str) -> Arc<Model> {
    let engine = Recompute::new(kernel);
    let document = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    match engine.start(document).wait().unwrap() {
        RunOutcome::Published(model) => model,
        RunOutcome::Superseded => panic!("superseded"),
    }
}

fn occt(text: &str) -> Arc<Model> {
    built(
        KernelPool::start_workers(Path::new(WORKER), 2).unwrap(),
        text,
    )
}

fn fake(text: &str) -> Arc<Model> {
    built(FakeKernel::new(2).unwrap(), text)
}

fn print(model: &Model, id: &str) -> Fingerprint {
    let fid = FeatureId::new(id).unwrap();
    model
        .fingerprint(&fid)
        .unwrap_or_else(|| panic!("{id}: {:?}", model.outcome(&fid)))
}

fn nm(mm: [f64; 6]) -> Option<[Length; 6]> {
    Some(mm.map(|v| Length::from_nm((v * 1e6).round() as i64)))
}

/// Whether a measure, in millionths, is within `relative` of `want`, plus the half a
/// millionth its rounding may move it.
fn near(millionths: i128, want: f64, relative: f64) -> bool {
    (millionths as f64 / 1e6 - want).abs() <= relative * want.abs() + 5e-7
}

/// Every primitive the document makes, at (1, -2, 3): (id, feature's text, volume, area,
/// box, faces, edges, vertices), the counts as OCCT builds each (a sphere: one face, its
/// seam and two degenerate pole edges, its two poles; and so on).
fn primitives() -> Vec<(&'static str, String, f64, f64, [f64; 6])> {
    let at = r#""position": [1, -2, 3]"#;
    vec![
        (
            "bx000000",
            format!(
                r#"{{"id": "bx000000", "type": "box", "v": 1, "name": "B", {at}, "length": 3, "width": 4, "height": 5}}"#
            ),
            60.0,
            94.0,
            [1.0, -2.0, 3.0, 4.0, 2.0, 8.0],
        ),
        (
            "cy000000",
            format!(
                r#"{{"id": "cy000000", "type": "cylinder", "v": 1, "name": "C", {at}, "radius": 2, "height": 5}}"#
            ),
            PI * 4.0 * 5.0,
            2.0 * PI * 2.0 * 5.0 + 2.0 * PI * 4.0,
            [-1.0, -4.0, 3.0, 3.0, 0.0, 8.0],
        ),
        (
            "sph00000",
            format!(
                r#"{{"id": "sph00000", "type": "sphere", "v": 1, "name": "S", {at}, "radius": 3}}"#
            ),
            36.0 * PI,
            36.0 * PI,
            [-2.0, -5.0, 0.0, 4.0, 1.0, 6.0],
        ),
        (
            "cn000000",
            format!(
                r#"{{"id": "cn000000", "type": "cone", "v": 1, "name": "K", {at}, "radius1": 3, "radius2": 0, "height": 4}}"#
            ),
            12.0 * PI,
            24.0 * PI,
            [-2.0, -5.0, 3.0, 4.0, 1.0, 7.0],
        ),
        (
            "fr000000",
            format!(
                r#"{{"id": "fr000000", "type": "cone", "v": 1, "name": "F", {at}, "radius1": 1, "radius2": 2, "height": 3}}"#
            ),
            7.0 * PI,
            PI * (3.0 * 10f64.sqrt() + 5.0),
            [-1.0, -4.0, 3.0, 3.0, 0.0, 6.0],
        ),
        (
            "tr000000",
            format!(
                r#"{{"id": "tr000000", "type": "torus", "v": 1, "name": "T", {at}, "radius1": 5, "radius2": 1}}"#
            ),
            10.0 * PI * PI,
            20.0 * PI * PI,
            [-5.0, -8.0, 2.0, 7.0, 4.0, 4.0],
        ),
    ]
}

#[test]
fn each_primitives_fingerprint_on_occt_is_its_formulas_and_the_box_is_exact() {
    let cases = primitives();
    let texts: Vec<&str> = cases.iter().map(|c| c.1.as_str()).collect();
    let model = occt(&doc(&texts));
    let mut wrong = Vec::new();
    for (id, _, volume, area, bbox) in &cases {
        let f = print(&model, id);
        if !near(f.volume.millionths(), *volume, 1e-9) {
            wrong.push(format!("{id} volume {} not {volume}", f.volume));
        }
        if !near(f.area.millionths(), *area, 1e-9) {
            wrong.push(format!("{id} area {} not {area}", f.area));
        }
        if f.bbox != nm(*bbox) {
            wrong.push(format!(
                "{id} bbox {:?} not {bbox:?}",
                f.bbox.map(|b| b.map(|l| l.to_string()))
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn the_fake_fingerprints_each_primitive_as_occt_does() {
    // The core's fingerprint tests trust the fake: its counts and box must be OCCT's, its
    // volume and area OCCT's to within the kernel's integration.
    let cases = primitives();
    let texts: Vec<&str> = cases.iter().map(|c| c.1.as_str()).collect();
    let text = doc(&texts);
    let (real, fake) = (occt(&text), fake(&text));
    let mut wrong = Vec::new();
    for (id, ..) in &cases {
        let (r, f) = (print(&real, id), print(&fake, id));
        if (r.faces, r.edges, r.vertices) != (f.faces, f.edges, f.vertices) {
            wrong.push(format!(
                "{id} counts: occt {:?}, fake {:?}",
                (r.faces, r.edges, r.vertices),
                (f.faces, f.edges, f.vertices)
            ));
        }
        if r.bbox != f.bbox {
            wrong.push(format!("{id} bbox: occt {:?}, fake {:?}", r.bbox, f.bbox));
        }
        if !near(
            r.volume.millionths(),
            f.volume.millionths() as f64 / 1e6,
            1e-9,
        ) || !near(r.area.millionths(), f.area.millionths() as f64 / 1e6, 1e-9)
        {
            wrong.push(format!(
                "{id} measures: occt {} {}, fake {} {}",
                r.volume, r.area, f.volume, f.area
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn the_smallest_parts_the_gate_admits_fingerprint_as_their_formulas() {
    // Sizes of 1 nm (the document's resolution, above the gate's 1e-7 mm), and a cone whose
    // small radius is 1 nm under a radius of 1000 mm.
    let text = doc(&[
        r#"{"id": "bx000000", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 0.000001, "width": 1000, "height": 1000}"#,
        r#"{"id": "cn000000", "type": "cone", "v": 1, "name": "K", "position": [0, 0, 0], "radius1": 1000, "radius2": 0.000001, "height": 1}"#,
        r#"{"id": "sph00000", "type": "sphere", "v": 1, "name": "S", "position": [0, 0, 0], "radius": 0.000001}"#,
    ]);
    let (real, fake) = (occt(&text), fake(&text));
    // The 1 nm slab: 1e-6 × 1e6 = 1 mm³; area 2 × 1e6 + 4 × 1e-3 = 2000000.004 mm².
    let slab = print(&real, "bx000000");
    assert_eq!(slab.volume.to_string(), "1");
    assert_eq!(slab.area.to_string(), "2000000.004");
    assert_eq!(slab.bbox, nm([0.0, 0.0, 0.0, 1e-6, 1000.0, 1000.0]));
    // The sphere of 1 nm: 4/3 π 1e-18 mm³ and 4π 1e-12 mm² both round to 0; its box ±1 nm.
    let ball = print(&real, "sph00000");
    assert_eq!((ball.volume.millionths(), ball.area.millionths()), (0, 0));
    assert_eq!(ball.bbox, nm([-1e-6, -1e-6, -1e-6, 1e-6, 1e-6, 1e-6]));
    // The cone: OCCT and the fake must agree on its faces (a cap of 1 nm or an apex).
    let (r, f) = (print(&real, "cn000000"), print(&fake, "cn000000"));
    assert_eq!(
        (r.faces, r.edges, r.vertices),
        (f.faces, f.edges, f.vertices)
    );
    assert_eq!(r.bbox, nm([-1000.0, -1000.0, 0.0, 1000.0, 1000.0, 1.0]));
}

#[test]
fn a_common_of_parts_that_dont_meet_fails_cleanly() {
    let text = doc(&[
        r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#,
        r#"{"id": "bbbbbbbb", "type": "box", "v": 1, "name": "B", "position": [5, 5, 5], "length": 1, "width": 1, "height": 1}"#,
        r#"{"id": "cccccccc", "type": "common", "v": 1, "name": "C", "base": "aaaaaaaa", "tool": "bbbbbbbb"}"#,
    ]);
    let model = occt(&text);
    let id = FeatureId::new("cccccccc").unwrap();
    // The kernel refuses an empty boolean result, so the feature fails, naming itself.
    match model.outcome(&id) {
        Some(FeatureOutcome::Failed(e)) => {
            assert_eq!(e.feature(), Some("cccccccc"), "{e}");
            assert!(e.suggestion().is_some(), "{e}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(model.fingerprint(&id), None);
}

#[test]
fn a_solid_with_nothing_in_it_is_measured_with_no_box() {
    // No operation in a document makes one (an empty boolean is refused), so it's measured
    // directly: nothing in it, no box, and a fingerprint without one.
    let empty = silico_kernel::test_ops::empty_shape().unwrap();
    let summary = silico_kernel::summarize(&empty).unwrap();
    assert_eq!(summary.bounds, None);
    assert_eq!((summary.faces, summary.edges, summary.vertices), (0, 0, 0));
    assert_eq!((summary.volume, summary.area), (0.0, 0.0));
    let f = silico_core::document::Fingerprint::of(&summary).unwrap();
    assert_eq!(f.bbox, None);
    assert_eq!((f.volume.millionths(), f.area.millionths()), (0, 0));
}

#[test]
fn a_box_cut_by_a_ball_at_its_corner_keeps_the_boxs_box() {
    // A ball of radius 2 centred on the corner (0, 0, 0) of a 10 cube, cut away: an eighth
    // of the ball gone, 1000 - (4/3 π 8) / 8 = 1000 - 4π/3; the box unchanged.
    let text = doc(&[
        r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#,
        r#"{"id": "bbbbbbbb", "type": "sphere", "v": 1, "name": "B", "position": [0, 0, 0], "radius": 2}"#,
        r#"{"id": "cccccccc", "type": "cut", "v": 1, "name": "C", "base": "aaaaaaaa", "tool": "bbbbbbbb"}"#,
        r#"{"id": "dddddddd", "type": "box", "v": 1, "name": "D", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#,
        r#"{"id": "eeeeeeee", "type": "sphere", "v": 1, "name": "E", "position": [10, 10, 10], "radius": 2}"#,
        r#"{"id": "ffffffff", "type": "fuse", "v": 1, "name": "F", "base": "dddddddd", "tool": "eeeeeeee"}"#,
    ]);
    let model = occt(&text);
    let cut = print(&model, "cccccccc");
    assert!(
        near(cut.volume.millionths(), 1000.0 - 4.0 * PI / 3.0, 1e-9),
        "{cut:?}"
    );
    assert_eq!(cut.bbox, nm([0.0, 0.0, 0.0, 10.0, 10.0, 10.0]));
    // Fused at the far corner: 1000 + 7/8 of 32π/3; the box out to 12.
    let fused = print(&model, "ffffffff");
    assert!(
        near(
            fused.volume.millionths(),
            1000.0 + 7.0 / 8.0 * 32.0 * PI / 3.0,
            1e-9
        ),
        "{fused:?}"
    );
    assert_eq!(fused.bbox, nm([0.0, 0.0, 0.0, 12.0, 12.0, 12.0]));
}

#[test]
fn parts_a_kilometre_out_fingerprint_their_box_to_the_nanometre() {
    // At 1e6 mm an f64 resolves 1.2e-10 mm, so the box must be exact to the nanometre.
    let text = doc(&[
        r#"{"id": "aaaaaaaa", "type": "sphere", "v": 1, "name": "A", "position": [1000000, -1000000, 999999.999999], "radius": 0.123457}"#,
        r#"{"id": "bbbbbbbb", "type": "cylinder", "v": 1, "name": "B", "position": [-999999.5, 1000000, -1000000], "radius": 0.25, "height": 0.000003}"#,
    ]);
    let model = occt(&text);
    let a = print(&model, "aaaaaaaa");
    assert_eq!(
        a.bbox.map(|b| b.map(Length::nm)),
        Some([
            999_999_876_543,
            -1_000_000_123_457,
            999_999_876_542,
            1_000_000_123_457,
            -999_999_876_543,
            1_000_000_123_456
        ])
    );
    let b = print(&model, "bbbbbbbb");
    assert_eq!(
        b.bbox.map(|b| b.map(Length::nm)),
        Some([
            -999_999_750_000,
            999_999_750_000,
            -1_000_000_000_000,
            -999_999_250_000,
            1_000_000_250_000,
            -999_999_999_997
        ])
    );
}

#[test]
fn curved_booleans_fingerprint_alike_in_pools_of_any_size() {
    // Measures integrated adaptively and boxes found by optimisation: the same document
    // must give the same fingerprints whichever worker, and however many, ran each step.
    let text = doc(&[
        r#"{"id": "aaaaaaaa", "type": "torus", "v": 1, "name": "A", "position": [0, 0, 0], "radius1": 10, "radius2": 3}"#,
        r#"{"id": "bbbbbbbb", "type": "sphere", "v": 1, "name": "B", "position": [10, 0, 0], "radius": 4.123457}"#,
        r#"{"id": "cccccccc", "type": "fuse", "v": 1, "name": "C", "base": "aaaaaaaa", "tool": "bbbbbbbb"}"#,
        r#"{"id": "dddddddd", "type": "cone", "v": 1, "name": "D", "position": [7.3, 1.1, -9], "radius1": 2.5, "radius2": 0.7, "height": 20}"#,
        r#"{"id": "eeeeeeee", "type": "cut", "v": 1, "name": "E", "base": "cccccccc", "tool": "dddddddd"}"#,
    ]);
    let ids = ["aaaaaaaa", "bbbbbbbb", "cccccccc", "dddddddd", "eeeeeeee"];
    let runs: Vec<Vec<Fingerprint>> = [1, 4, 2]
        .into_iter()
        .map(|workers| {
            let model = built(
                KernelPool::start_workers(Path::new(WORKER), workers).unwrap(),
                &text,
            );
            ids.iter().map(|id| print(&model, id)).collect()
        })
        .collect();
    assert_eq!(runs[0], runs[1]);
    assert_eq!(runs[0], runs[2]);
    // The fused ball reaches past the torus's outer edge: x to 10 + 4.123457.
    let fused = runs[0][2];
    assert_eq!(fused.bbox.map(|b| b[3].nm()), Some(14_123_457));
}
