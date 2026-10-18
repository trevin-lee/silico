//! M3.6: the thin wall OCCT gets wrong (docs/ports/sketch-features.md, *Known OCCT wrong
//! results*), measured on Silico's own kernel: a profile with a wall g wide flush with a
//! 10 mm cube's side, made by the faces request, swept by the prism request, and fused with
//! the cube (a pad) or cut from it (a pocket) by the boolean request, as FreeCAD's Part
//! booleans were measured without the fuzzy value; and the pad's refusal as adding nothing
//! (design S7, `silico_core::sweep::check_result`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/close.rs"]
mod close;
#[path = "common/numbers.rs"]
mod numbers;

use close::assert_close;
use silico_core::ErrorCode;
use silico_core::document::FeatureId;
use silico_core::document::SweepFeature;
use silico_core::sweep::{Measured, check_result};
use silico_kernel::{
    BooleanOp, Curve, FaceRegions, Region, ShapeSummary, SketchPlane, boolean, make, make_faces,
    prism, summarize,
};
use silico_protocol::Primitive;

fn line(start: [f64; 2], end: [f64; 2]) -> Curve {
    Curve::Line { start, end }
}

/// The `w` × `h` rectangle from `(x, y)`, counter-clockwise from its lowest corner.
fn rectangle(x: f64, y: f64, w: f64, h: f64) -> Vec<Curve> {
    let p = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
    (0..4).map(|i| line(p[i], p[(i + 1) % 4])).collect()
}

fn one(plane: SketchPlane, outer: Vec<Curve>, holes: Vec<Vec<Curve>>) -> FaceRegions {
    FaceRegions {
        plane,
        regions: vec![Region { outer, holes }],
    }
}

/// The profile docs/ports/sketch-features.md measured on FreeCAD: a 10 × 10 square flush
/// with the cube's top, with a hole from (g, 2) to (5, 8), so a wall g wide lies flush with
/// the cube's side; its prism from the top up 5, fused with the cube through the boolean.
fn thin_wall_pad(g: f64) -> (ShapeSummary, ShapeSummary) {
    thin_wall(g, BooleanOp::Fuse)
}

/// The same profile, its prism from `z` = 10 (a pad, fused) or 5 (a pocket, cut) up 5.
fn thin_wall(g: f64, op: BooleanOp) -> (ShapeSummary, ShapeSummary) {
    let cube = make(&Primitive::Box {
        dx: 10.0,
        dy: 10.0,
        dz: 10.0,
    })
    .unwrap();
    let hole: Vec<Curve> = rectangle(g, 2.0, 5.0 - g, 6.0)
        .iter()
        .rev()
        .map(|c| match *c {
            Curve::Line { start, end } => line(end, start),
            other => other,
        })
        .collect();
    let made = make_faces(&one(
        SketchPlane::Xy,
        rectangle(0.0, 0.0, 10.0, 10.0),
        vec![hole],
    ))
    .unwrap();
    let z = if op == BooleanOp::Fuse { 10.0 } else { 5.0 };
    let pad = prism(&made.shape, [0.0, 0.0, z], [0.0, 0.0, 5.0]).unwrap();
    let fused = boolean(op, &cube, &pad.shape).unwrap();
    (summarize(&cube).unwrap(), summarize(&fused.shape).unwrap())
}

fn measured(s: &ShapeSummary) -> Measured {
    Measured {
        volume: s.volume,
        solids: 1,
        faces: s.faces,
        edges: s.edges,
        vertices: s.vertices,
    }
}

#[test]
fn a_thin_wall_flush_with_the_base_is_dropped_by_occt_and_the_pad_refused_as_adding_nothing() {
    let pad = FeatureId::new("p0000001").unwrap();
    let base = FeatureId::new("b0000001").unwrap();
    // g = 2e-7: OCCT fuses it to the cube alone, valid, as on FreeCAD's OCCT without the
    // fuzzy value; the no-op rule refuses it.
    let (cube, fused) = thin_wall_pad(2e-7);
    assert!(fused.valid);
    assert_eq!(fused.volume, cube.volume);
    assert_eq!((fused.faces, fused.edges, fused.vertices), (6, 12, 8));
    let refused = check_result(
        &pad,
        SweepFeature::Pad,
        &base,
        &measured(&cube),
        &measured(&fused),
    )
    .unwrap_err();
    assert_eq!(refused.code(), ErrorCode::KernelFailed);
    assert!(
        refused
            .message()
            .starts_with("pad p0000001 adds nothing to its base b0000001")
    );
    // g = 4e-7: right, 1000 + 5 × (100 − (5 − g) × 6), and accepted.
    let g = 4e-7;
    let (cube, fused) = thin_wall_pad(g);
    assert!(fused.valid);
    assert_close(
        "4e-7",
        fused.volume,
        1000.0 + 5.0 * (100.0 - (5.0 - g) * 6.0),
        1e-12,
    );
    assert_eq!(fused.faces, 15);
    assert_eq!(
        check_result(
            &pad,
            SweepFeature::Pad,
            &base,
            &measured(&cube),
            &measured(&fused)
        ),
        Ok(())
    );
}

#[test]
fn the_thin_wall_band_is_freecads_without_the_fuzzy_value() {
    // docs/ports/sketch-features.md's first row, on Silico's kernel: for 1e-7 < g ≤ 3e-7 the
    // pad is dropped (the cube alone) and the pocket valid with its volume 20 × g too large;
    // from 4e-7 both are right.
    let area = |g: f64| 100.0 - (5.0 - g) * 6.0;
    for g in [1.5e-7, 2e-7, 3e-7] {
        let (cube, pad) = thin_wall(g, BooleanOp::Fuse);
        assert!(pad.valid);
        assert_eq!((pad.volume, pad.faces), (cube.volume, 6), "pad, g = {g:e}");
        let (_, pocket) = thin_wall(g, BooleanOp::Cut);
        assert!(pocket.valid);
        assert_eq!(pocket.faces, 11);
        let error = pocket.volume - (1000.0 - 5.0 * area(g));
        assert!(
            (error / g - 20.0).abs() < 1e-3,
            "pocket, g = {g:e}: off by {error:e}"
        );
    }
    for g in [4e-7, 1e-6, 1e-5] {
        let (_, pad) = thin_wall(g, BooleanOp::Fuse);
        assert_close(
            &format!("pad, g = {g:e}"),
            pad.volume,
            1000.0 + 5.0 * area(g),
            1e-12,
        );
        assert_eq!(pad.faces, 15);
        let (_, pocket) = thin_wall(g, BooleanOp::Cut);
        assert_close(
            &format!("pocket, g = {g:e}"),
            pocket.volume,
            1000.0 - 5.0 * area(g),
            1e-12,
        );
    }
}
