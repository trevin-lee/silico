//! M0.4: the cxx bridge builds a box, measures it, and turns every C++ exception into an
//! error instead of aborting.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/imports.rs"]
mod imports;

use silico_kernel::test_ops::{
    throw_non_exception, throw_standard_failure, throw_std_runtime_error,
};
use silico_kernel::{make_box, mass_properties, volume};

/// Relative error between `actual` and a nonzero `expected`.
fn relative_error(actual: f64, expected: f64) -> f64 {
    ((actual - expected) / expected).abs()
}

#[test]
fn box_volume_is_exact() {
    let shape = make_box(10.0, 20.0, 30.0).unwrap();
    let v = volume(&shape).unwrap();
    // Analytic: 10 × 20 × 30.
    assert!(relative_error(v, 6000.0) <= 1e-9, "volume {v}");
}

#[test]
fn box_spans_origin_to_its_size() {
    // A box from the origin to (10, 20, 30) has its center at (5, 10, 15) (analytic), which
    // a box with its sizes swapped between axes, or centered on the origin, doesn't.
    let c = mass_properties(&make_box(10.0, 20.0, 30.0).unwrap())
        .unwrap()
        .center;
    for (axis, got, want) in [("x", c.x, 5.0), ("y", c.y, 10.0), ("z", c.z, 15.0)] {
        assert!(
            relative_error(got, want) <= 1e-9,
            "center {axis} = {got}, want {want}"
        );
    }
}

#[test]
fn box_sizes_near_the_limits_are_accepted() {
    // Just above OCCT's 1e-7 tolerance, fractional, and large. Volumes are analytic.
    for (dx, dy, dz) in [
        (2e-7, 1.0, 1.0),
        (0.5, 0.25, 0.125),
        (1e6, 1e6, 1e6),
        (1e-6, 1e3, 1e5),
    ] {
        let v = volume(&make_box(dx, dy, dz).unwrap()).unwrap();
        let want = dx * dy * dz;
        assert!(
            relative_error(v, want) <= 1e-9,
            "{dx} × {dy} × {dz}: volume {v}"
        );
    }
}

#[test]
fn box_zero_dimension_is_err() {
    // Zero, negative zero, and a size below OCCT's 1e-7 tolerance.
    for (dx, dy, dz) in [
        (0.0, 20.0, 30.0),
        (10.0, 0.0, 30.0),
        (10.0, 20.0, 0.0),
        (-0.0, 20.0, 30.0),
        (1e-8, 20.0, 30.0),
    ] {
        let err = make_box(dx, dy, dz)
            .err()
            .unwrap_or_else(|| panic!("box {dx} × {dy} × {dz} was accepted"));
        // OCCT's own check, so this is the exception conversion at work.
        assert!(err.message().contains("Standard_DomainError"), "{err}");
    }
}

#[test]
fn box_negative_and_nan_are_err() {
    // The bridge's own check, which names the size at fault.
    let bad = [
        ("dx", -10.0, 20.0, 30.0),
        ("dy", 10.0, -20.0, 30.0),
        ("dz", 10.0, 20.0, -30.0),
        ("dx", f64::NAN, 20.0, 30.0),
        ("dy", 10.0, f64::NAN, 30.0),
        ("dz", 10.0, 20.0, f64::NAN),
        ("dx", f64::INFINITY, 20.0, 30.0),
        ("dy", 10.0, f64::NEG_INFINITY, 30.0),
    ];
    for (name, dx, dy, dz) in bad {
        let err = make_box(dx, dy, dz)
            .err()
            .unwrap_or_else(|| panic!("box {dx} × {dy} × {dz} was accepted"));
        assert!(
            err.message()
                .starts_with(&format!("box size {name} must be")),
            "{dx} × {dy} × {dz}: {err}"
        );
    }
}

#[test]
fn box_huge_or_subnormal_sizes_are_err() {
    // OCCT treats sizes past about 1e100 as infinite, and subnormals as zero.
    for (dx, dy, dz) in [
        (f64::MAX, 1.0, 1.0),
        (1e300, 1e300, 1e300),
        (f64::MIN_POSITIVE, 1.0, 1.0),
        (5e-324, 1.0, 1.0),
    ] {
        let err = make_box(dx, dy, dz)
            .err()
            .unwrap_or_else(|| panic!("box {dx:e} × {dy:e} × {dz:e} was accepted"));
        assert!(err.message().contains("Standard_DomainError"), "{err}");
    }
}

#[test]
fn every_exception_becomes_err() {
    // OCCT's Standard_Failure: its type, then its message.
    let err = throw_standard_failure("bridge test 42").unwrap_err();
    assert_eq!(err.message(), "Standard_Failure: bridge test 42");
    assert_eq!(
        err.to_string(),
        "kernel error: Standard_Failure: bridge test 42"
    );
    // With no message, just the type.
    assert_eq!(
        throw_standard_failure("").unwrap_err().message(),
        "Standard_Failure"
    );
    // Any other std::exception: its what().
    assert_eq!(
        throw_std_runtime_error("not OCCT").unwrap_err().message(),
        "not OCCT"
    );
    // Something that isn't an exception class at all.
    assert_eq!(
        throw_non_exception().unwrap_err().message(),
        "unknown C++ exception"
    );
}

#[test]
fn errors_leave_the_bridge_usable() {
    // A thrown exception mustn't leave OCCT or the bridge in a bad state.
    for _ in 0..100 {
        assert!(make_box(0.0, 1.0, 1.0).is_err());
        assert!(throw_standard_failure("again").is_err());
        assert!(throw_non_exception().is_err());
    }
    let v = volume(&make_box(1.0, 2.0, 3.0).unwrap()).unwrap();
    assert!(relative_error(v, 6.0) <= 1e-9, "volume {v}");
}

#[test]
fn test_binary_imports_no_platform_math() {
    imports::assert_this_binary_imports_no_platform_math();
}
