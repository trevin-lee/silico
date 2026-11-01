//! Every kind of plane a sketch can lie on, for the tests that loop over every plane: the
//! origin planes, and two face frames (docs/ports/attacher.md, M5.9), one tilted out of
//! model space's axes and one along them.
#![allow(dead_code)]

use silico_kernel::{FaceFrame, SketchPlane};

/// The XY frame tilted 30° about X, from (1, 2, 3): x +X, y (0, cos 30°, sin 30°), so its
/// normal is (0, −sin 30°, cos 30°).
pub fn tilted() -> SketchPlane {
    let (c, s) = (libm::cos(30f64.to_radians()), libm::sin(30f64.to_radians()));
    SketchPlane::Face(FaceFrame {
        origin: [1.0, 2.0, 3.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, c, s],
    })
}

/// A box's top face at z = 7, reversed: x −X, y +Y, its normal −Z. Its axes are model
/// space's, so a box that is the box of a parallelepiped's corners only on such a plane
/// (a circle's, a turn's) is still that on it.
pub fn reversed_top() -> SketchPlane {
    SketchPlane::Face(FaceFrame {
        origin: [0.0, 0.0, 7.0],
        x: [-1.0, 0.0, 0.0],
        y: [0.0, 1.0, 0.0],
    })
}

/// Every plane: the origin planes and both face frames.
pub fn every() -> [SketchPlane; 5] {
    let [xy, xz, yz] = SketchPlane::ORIGIN_PLANES;
    [xy, xz, yz, tilted(), reversed_top()]
}

/// The planes whose axes are model space's: the origin planes and the reversed top.
pub fn square() -> [SketchPlane; 4] {
    let [xy, xz, yz] = SketchPlane::ORIGIN_PLANES;
    [xy, xz, yz, reversed_top()]
}
