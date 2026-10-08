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
        fn shape_to_brep(shape: &Shape) -> Result<UniquePtr<CxxString>>;
        fn shape_from_brep(brep: &[u8]) -> Result<UniquePtr<Shape>>;
        fn fp_trap_mask() -> u32;
        fn disable_fp_traps() -> Result<()>;

        #[cfg(feature = "test_ops")]
        fn debug_throw_standard_failure(message: &str) -> Result<()>;
        #[cfg(feature = "test_ops")]
        fn debug_throw_std_runtime_error(message: &str) -> Result<()>;
        #[cfg(feature = "test_ops")]
        fn debug_throw_non_exception() -> Result<()>;
        #[cfg(feature = "test_ops")]
        fn debug_enable_fp_traps() -> Result<()>;
        #[cfg(feature = "test_ops")]
        fn debug_raise_sigsegv();
    }
}

pub use ffi::{MassProperties, Point3};

pub mod worker;

/// An operation OCCT refused or failed, with OCCT's own message (for example
/// `Standard_DomainError: ...`). It stays inside the worker: the worker reports it as a
/// protocol `Failure`, and the core turns that into its structured `Error`
/// (`kernel_failed`, with this message as the cause), adding the feature it knows about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelError {
    message: String,
    invalid_input: bool,
}

impl KernelError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            invalid_input: false,
        }
    }

    /// The bridge refused the request's values before calling OCCT.
    fn invalid_input(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            invalid_input: true,
        }
    }

    /// What went wrong, in OCCT's words where OCCT reported it.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Whether the request's values were refused before reaching OCCT (rather than OCCT
    /// refusing or failing the operation).
    pub fn is_invalid_input(&self) -> bool {
        self.invalid_input
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
            return Err(KernelError::invalid_input(format!(
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

/// The shape as OCCT's binary B-rep followed by its CRC-32 (4 bytes, little-endian):
/// how shapes cross the worker boundary, and how they'll be cached. OCCT reads many
/// corrupted B-reps without complaint, as a different shape, so [`from_brep`] checks the
/// CRC first.
pub fn to_brep(shape: &Shape) -> Result<Vec<u8>, KernelError> {
    let bytes = ffi::shape_to_brep(shape.get()?)?;
    let bytes = bytes
        .as_ref()
        .ok_or_else(|| KernelError::new("OCCT returned no B-rep"))?
        .as_bytes();
    let mut brep = Vec::with_capacity(bytes.len() + 4);
    brep.extend_from_slice(bytes);
    brep.extend_from_slice(&crc32fast::hash(bytes).to_le_bytes());
    Ok(brep)
}

/// A shape from [`to_brep`]'s output. Bytes whose CRC doesn't match are refused before
/// OCCT sees them.
pub fn from_brep(brep: &[u8]) -> Result<Shape, KernelError> {
    let (body, crc) = brep
        .split_last_chunk::<4>()
        .ok_or_else(|| KernelError::invalid_input("B-rep is too short to be one"))?;
    if crc32fast::hash(body) != u32::from_le_bytes(*crc) {
        return Err(KernelError::invalid_input(
            "B-rep is corrupted: its checksum doesn't match",
        ));
    }
    let shape = ffi::shape_from_brep(body)?;
    if shape.is_null() {
        return Err(KernelError::new("OCCT returned no shape"));
    }
    Ok(Shape(shape))
}

/// The floating-point trap-enable bits that are set, 0 when none is. Workers must run
/// with every trap off (ARCHITECTURE.md, Deterministic kernel).
pub fn fp_trap_mask() -> u32 {
    ffi::fp_trap_mask()
}

/// Turns every floating-point trap off, through OCCT.
pub fn disable_fp_traps() -> Result<(), KernelError> {
    Ok(ffi::disable_fp_traps()?)
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

    /// Turns floating-point traps on through OCCT. Only Linux can; on Apple silicon the
    /// hardware ignores the request.
    pub fn enable_fp_traps() -> Result<(), KernelError> {
        Ok(ffi::debug_enable_fp_traps()?)
    }

    /// Crashes the process with SIGSEGV, as a fault inside OCCT would.
    pub fn raise_sigsegv() {
        ffi::debug_raise_sigsegv();
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
