//! Chamfer sizes, briefly.

use silico_kernel::ChamferSize;

pub fn equal(distance: f64) -> ChamferSize {
    ChamferSize::Equal { distance }
}

pub fn two(distance: f64, other: f64) -> ChamferSize {
    ChamferSize::TwoDistances { distance, other }
}

pub fn angled(distance: f64, angle: f64) -> ChamferSize {
    ChamferSize::DistanceAngle { distance, angle }
}
