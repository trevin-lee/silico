//! A boolean whose faces touch at one point, where OCCT built a section edge with no 3D curve
//! and dereferenced it (patch 0005, third_party/occt/patches): a pointed cone (radius 5 to 0,
//! height 5) and a level rod of radius 1, 20 long, touching the cone's side at its half
//! height, 300° round. The B-reps are the ones Silico's own primitives and revolve made
//! (tests/fixtures/brep/README.md), bit for bit: the crash depends on the exact numbers.
//!
//! Expected values are by hand: the cone πr²h/3 = 125π/3, the rod πr²l = 20π, and solids that
//! touch at a point share nothing. FreeCAD 1.1.3 (on conda-forge's OCCT 7.9.3, which doesn't
//! reach the crash from these B-reps) gives the same: the cut 130.8996939, the fuse
//! 193.731546971 in two solids, the common no solid. Run on a worker: before the patch, the
//! crash took the worker with it, and would take a test program run in-process.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::f64::consts::PI;

use super::brep_fixture::fixture;
use super::kernel::kernel;

use silico_core::ErrorCode;
use silico_kernel::BooleanOp;
use silico_protocol::{Blob, ShapeData};

const CONE: f64 = 125.0 * PI / 3.0;
const ROD: f64 = 20.0 * PI;

fn data(name: &str) -> ShapeData {
    ShapeData::unchecked(Blob::inline(fixture(name)))
}

fn close(got: f64, want: f64) -> bool {
    ((got - want) / want).abs() <= 1e-9
}

#[test]
fn a_rod_touching_a_pointed_cones_side_is_fused_cut_and_commoned_as_by_hand() {
    let (cone, rod) = (data("pointed_cone.bin"), data("rod_on_a_cones_side.bin"));
    let mut k = kernel();
    assert!(close(k.summarize(&cone).unwrap().volume, CONE));
    assert!(close(k.summarize(&rod).unwrap().volume, ROD));
    // The fuse: both solids, whole, apart but for the point.
    let fuse = k
        .boolean(BooleanOp::Fuse, &cone, &rod)
        .unwrap_or_else(|e| panic!("fuse: {e}"));
    let s = k.summarize(&fuse.shape).unwrap();
    assert!(s.valid, "fuse: {s:?}");
    assert!(
        close(s.volume, CONE + ROD),
        "fuse: {} for {}",
        s.volume,
        CONE + ROD
    );
    // The cut: the cone whole.
    let cut = k
        .boolean(BooleanOp::Cut, &cone, &rod)
        .unwrap_or_else(|e| panic!("cut: {e}"));
    let s = k.summarize(&cut.shape).unwrap();
    assert!(s.valid, "cut: {s:?}");
    assert!(close(s.volume, CONE), "cut: {} for {CONE}", s.volume);
    // The common: nothing, refused as an empty result, as a common of solids that touch at
    // a point is.
    let e = k
        .boolean(BooleanOp::Common, &cone, &rod)
        .map(|_| ())
        .unwrap_err();
    assert_eq!(e.code(), ErrorCode::KernelFailed, "common: {e}");
    assert!(
        e.message().contains("the common's result is empty"),
        "common: {e}"
    );
}
