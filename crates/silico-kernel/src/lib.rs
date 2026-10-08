//! The OCCT bridge and kernel worker. The only crate that links OCCT: every other crate
//! reaches OCCT through the worker process (ARCHITECTURE.md, invariant 1), so no other
//! crate may depend on this one.
//!
//! The bridge is deliberately thin: C++ calls OCCT and returns shapes and plain values,
//! and every C++ exception comes back as a [`KernelError`], never as an abort. Lengths
//! are in the document's length unit (millimetres in v1), as plain `f64` until
//! `silico-core` has a length type.

// FFI crate: `unsafe` is permitted here and nowhere else (see ARCHITECTURE.md).
#![allow(unsafe_code)]

use std::fmt;

#[cxx::bridge(namespace = "silico")]
mod ffi {
    /// A point in model space.
    struct Point3 {
        x: f64,
        y: f64,
        z: f64,
    }

    /// A solid's mass properties, taken as uniformly dense, from one integration over
    /// its faces.
    struct MassProperties {
        volume: f64,
        center: Point3,
    }

    unsafe extern "C++" {
        include!("silico-kernel/cpp/bridge.h");

        type Shape;

        fn make_box(dx: f64, dy: f64, dz: f64) -> Result<UniquePtr<Shape>>;
        fn mass_properties(shape: &Shape) -> Result<MassProperties>;

        #[cfg(feature = "test_ops")]
        fn debug_throw_standard_failure(message: &str) -> Result<()>;
        #[cfg(feature = "test_ops")]
        fn debug_throw_std_runtime_error(message: &str) -> Result<()>;
        #[cfg(feature = "test_ops")]
        fn debug_throw_non_exception() -> Result<()>;
    }
}

pub use ffi::{MassProperties, Point3};

/// An operation OCCT refused or failed, with OCCT's own message (for example
/// `Standard_DomainError: ...`). M0.5 replaces this with the structured error type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelError {
    message: String,
}

impl KernelError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// What went wrong, in OCCT's words where OCCT reported it.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "kernel error: {}", self.message)
    }
}

impl std::error::Error for KernelError {}

impl From<cxx::Exception> for KernelError {
    fn from(e: cxx::Exception) -> Self {
        Self::new(e.what())
    }
}

/// A shape owned by OCCT.
pub struct Shape(cxx::UniquePtr<ffi::Shape>);

impl Shape {
    fn get(&self) -> Result<&ffi::Shape, KernelError> {
        self.0
            .as_ref()
            .ok_or_else(|| KernelError::new("shape is empty"))
    }
}

/// A box with one corner at the origin and the opposite corner at `(dx, dy, dz)`.
/// Every size must be a positive, finite length: that's this operation's precondition.
pub fn make_box(dx: f64, dy: f64, dz: f64) -> Result<Shape, KernelError> {
    // OCCT would build a negative size as a box on the other side of the origin, and lets
    // NaN past its checks; both break the precondition, so they're refused here. Zero and
    // sizes below OCCT's 1e-7 tolerance are left to OCCT, which rejects them with
    // Standard_DomainError. A feature that wants a reversed extent (core's decision) asks
    // for a positive box and places it.
    for (name, value) in [("dx", dx), ("dy", dy), ("dz", dz)] {
        if !(value.is_finite() && value >= 0.0) {
            return Err(KernelError::new(format!(
                "box size {name} must be a positive, finite length, got {value}"
            )));
        }
    }
    let shape = ffi::make_box(dx, dy, dz)?;
    if shape.is_null() {
        return Err(KernelError::new("OCCT returned no box"));
    }
    Ok(Shape(shape))
}

/// A solid's volume and center of mass, taken as uniformly dense.
pub fn mass_properties(shape: &Shape) -> Result<MassProperties, KernelError> {
    Ok(ffi::mass_properties(shape.get()?)?)
}

/// The volume of a solid (from [`mass_properties`]).
pub fn volume(shape: &Shape) -> Result<f64, KernelError> {
    Ok(mass_properties(shape)?.volume)
}

/// Operations that exist only to test the bridge, built with the `test_ops` feature
/// (which this crate's own tests turn on, and nothing else should).
#[cfg(feature = "test_ops")]
pub mod test_ops {
    use super::{KernelError, ffi};

    /// Throws OCCT's `Standard_Failure` with `message` on the C++ side.
    pub fn throw_standard_failure(message: &str) -> Result<(), KernelError> {
        Ok(ffi::debug_throw_standard_failure(message)?)
    }

    /// Throws `std::runtime_error` with `message`: a `std::exception` that isn't OCCT's.
    pub fn throw_std_runtime_error(message: &str) -> Result<(), KernelError> {
        Ok(ffi::debug_throw_std_runtime_error(message)?)
    }

    /// Throws an `int`, which isn't an exception class at all.
    pub fn throw_non_exception() -> Result<(), KernelError> {
        Ok(ffi::debug_throw_non_exception()?)
    }
}

#[cfg(test)]
#[path = "../tests/common/imports.rs"]
mod imports;

#[cfg(test)]
mod tests {
    /// This crate's unit-test binary links OCCT and the bridge, so it must import no
    /// inexact platform math either (ARCHITECTURE.md, Deterministic kernel).
    #[test]
    fn unit_test_binary_imports_no_platform_math() {
        super::imports::assert_this_binary_imports_no_platform_math();
    }
}
