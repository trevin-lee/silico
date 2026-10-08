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

    /// What a solid is: its mass properties, area, how many distinct faces, edges, and
    /// vertices it has, and whether OCCT's BRepCheck_Analyzer finds it valid.
    struct ShapeSummary {
        volume: f64,
        area: f64,
        center: Point3,
        faces: u32,
        edges: u32,
        vertices: u32,
        valid: bool,
    }

    unsafe extern "C++" {
        include!("silico-kernel/cpp/bridge.h");

        type Shape;

        fn make_box(dx: f64, dy: f64, dz: f64) -> Result<UniquePtr<Shape>>;
        fn mass_properties(shape: &Shape) -> Result<MassProperties>;
        fn summarize(shape: &Shape) -> Result<ShapeSummary>;
        fn edge_count(shape: &Shape) -> Result<u32>;
        fn fillet(shape: &Shape, edges: &[u32], radius: f64) -> Result<UniquePtr<Shape>>;

        type Tessellation;
        fn bounding_diagonal(shape: &Shape) -> Result<f64>;
        fn tessellate(shape: &Shape, deflection: f64) -> Result<UniquePtr<Tessellation>>;
        fn positions(self: &Tessellation) -> &[f64];
        fn triangles(self: &Tessellation) -> &[u32];
        fn face_triangles(self: &Tessellation) -> &[u32];
        fn edge_points(self: &Tessellation) -> &[f64];
        fn edge_points_per_edge(self: &Tessellation) -> &[u32];
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
        #[cfg(feature = "test_ops")]
        fn debug_open_box() -> Result<UniquePtr<Shape>>;
    }
}

pub use ffi::{MassProperties, Point3, ShapeSummary};
pub use silico_protocol::Mesh;

pub mod worker;

/// An operation OCCT refused or failed, with OCCT's own message (for example
/// `Standard_DomainError: ...`). It stays inside the worker: the worker reports it as a
/// protocol `Failure`, and the core turns that into its structured `Error`
/// (`kernel_failed`, with this message as the cause), adding the feature it knows about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelError {
    message: String,
    invalid_input: bool,
    element: Option<String>,
}

impl KernelError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            invalid_input: false,
            element: None,
        }
    }

    /// The bridge refused the request's values before calling OCCT.
    fn invalid_input(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            invalid_input: true,
            element: None,
        }
    }

    fn with_element(mut self, element: impl Into<String>) -> Self {
        self.element = Some(element.into());
        self
    }

    /// The sub-element at fault (such as `Edge3`), when known.
    pub fn element(&self) -> Option<&str> {
        self.element.as_deref()
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

/// The shape's summary: volume, area, center, topology counts, and validity.
pub fn summarize(shape: &Shape) -> Result<ShapeSummary, KernelError> {
    Ok(ffi::summarize(shape.get()?)?)
}

/// Rounds `edges` of `shape` with a constant `radius`. Edges are numbered from 1 in OCCT's
/// `TopExp::MapShapes` order, which is also FreeCAD's `Edge1`, `Edge2`, and so on.
pub fn fillet(shape: &Shape, edges: &[u32], radius: f64) -> Result<Shape, KernelError> {
    if !(radius.is_finite() && radius > 0.0) {
        return Err(KernelError::invalid_input(format!(
            "fillet radius must be a positive, finite length, got {radius}"
        )));
    }
    if edges.is_empty() {
        return Err(KernelError::invalid_input(
            "a fillet needs at least one edge",
        ));
    }
    let inner = shape.get()?;
    let count = ffi::edge_count(inner)?;
    let mut seen = std::collections::BTreeSet::new();
    for &n in edges {
        if n == 0 || n > count {
            return Err(KernelError::invalid_input(format!(
                "Edge{n} doesn't exist: the shape has edges 1 to {count}"
            ))
            .with_element(format!("Edge{n}")));
        }
        if !seen.insert(n) {
            return Err(
                KernelError::invalid_input(format!("Edge{n} is listed twice"))
                    .with_element(format!("Edge{n}")),
            );
        }
    }
    // When OCCT fails a single-edge fillet, that edge is the one at fault; it doesn't say
    // which of several it was.
    let rounded = ffi::fillet(inner, edges, radius).map_err(|e| {
        let error = KernelError::from(e);
        match edges {
            [edge] => error.with_element(format!("Edge{edge}")),
            _ => error,
        }
    })?;
    if rounded.is_null() {
        return Err(KernelError::new("OCCT returned no shape"));
    }
    Ok(Shape(rounded))
}

/// The smallest mesh deflection allowed, as a fraction of the shape's size. Finer meshes
/// cost far more than they're worth for display: every tenth of the deflection is about
/// ten times the triangles and seventy times the time.
pub const MIN_RELATIVE_DEFLECTION: f64 = 1e-4;

/// Meshes `shape` for display, with no point of the mesh further than `deflection` from
/// the surface. Single-threaded, so the same shape always gives the same mesh, and it
/// leaves the shape as it was (OCCT would otherwise keep the mesh on it). Faces and edges
/// are in OCCT's order; `edges[0]` is Edge1, as [`fillet`] numbers them.
pub fn tessellate(shape: &Shape, deflection: f64) -> Result<Mesh, KernelError> {
    if !(deflection.is_finite() && deflection > 0.0) {
        return Err(KernelError::invalid_input(format!(
            "mesh deflection must be a positive, finite length, got {deflection}"
        )));
    }
    let inner = shape.get()?;
    let floor = (ffi::bounding_diagonal(inner)? * MIN_RELATIVE_DEFLECTION).max(1e-7);
    if deflection < floor {
        return Err(KernelError::invalid_input(format!(
            "mesh deflection {deflection} is too fine for this shape: use at least {floor}"
        )));
    }
    let raw = ffi::tessellate(inner, deflection)?;
    let raw = raw
        .as_ref()
        .ok_or_else(|| KernelError::new("OCCT returned no mesh"))?;
    let triples = |values: &[f64]| -> Vec<[f64; 3]> {
        values.chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect()
    };
    let mut edges = Vec::new();
    let mut points = raw.edge_points().chunks_exact(3);
    for &count in raw.edge_points_per_edge() {
        edges.push(
            points
                .by_ref()
                .take(count as usize)
                .map(|p| [p[0], p[1], p[2]])
                .collect(),
        );
    }
    Ok(Mesh {
        positions: triples(raw.positions()),
        triangles: raw
            .triangles()
            .chunks_exact(3)
            .map(|t| [t[0], t[1], t[2]])
            .collect(),
        face_triangles: raw.face_triangles().to_vec(),
        edges,
    })
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

    /// A box with one face left out: a solid whose shell doesn't close.
    pub fn open_box() -> Result<super::Shape, KernelError> {
        Ok(super::Shape(ffi::debug_open_box()?))
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
