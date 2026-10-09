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
        fn is_valid_where_changed(input: &Shape, result: &Shape) -> Result<bool>;
        fn is_valid(shape: &Shape) -> Result<bool>;

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

impl From<ShapeSummary> for silico_protocol::ShapeSummary {
    fn from(s: ShapeSummary) -> Self {
        Self {
            volume: s.volume,
            area: s.area,
            center: [s.center.x, s.center.y, s.center.z],
            faces: s.faces,
            edges: s.edges,
            vertices: s.vertices,
            valid: s.valid,
        }
    }
}
pub use silico_protocol::Mesh;

pub mod worker;

/// An operation refused or failed, with OCCT's own message where OCCT reported it (for
/// example `Standard_DomainError: ...`). It stays inside the worker: the worker reports it
/// as a protocol `Failure`, and the core turns that into its structured `Error`, with a
/// code for each [`KernelErrorKind`] (`invalid_input`, `kernel_failed`, `invalid_result`)
/// and this message as the cause, adding the feature it knows about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelError {
    kind: KernelErrorKind,
    message: String,
    element: Option<String>,
    suggestion: Option<String>,
}

/// What kind of failure a [`KernelError`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelErrorKind {
    /// The request's values were refused before reaching OCCT.
    InvalidInput,
    /// OCCT refused or couldn't perform the operation.
    Failed,
    /// OCCT performed the operation, but its result isn't valid, so it was refused.
    InvalidResult,
}

impl KernelError {
    fn of_kind(kind: KernelErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            element: None,
            suggestion: None,
        }
    }

    fn new(message: impl Into<String>) -> Self {
        Self::of_kind(KernelErrorKind::Failed, message)
    }

    /// The bridge refused the request's values before calling OCCT.
    fn invalid_input(message: impl Into<String>) -> Self {
        Self::of_kind(KernelErrorKind::InvalidInput, message)
    }

    fn with_element(mut self, element: impl Into<String>) -> Self {
        self.element = Some(element.into());
        self
    }

    fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Whether the values were refused, OCCT failed, or OCCT's result was refused.
    pub fn kind(&self) -> KernelErrorKind {
        self.kind
    }

    /// What to try instead, when there's something to suggest.
    pub fn suggestion(&self) -> Option<&str> {
        self.suggestion.as_deref()
    }

    /// The sub-element at fault (such as `Edge3`), when known.
    pub fn element(&self) -> Option<&str> {
        self.element.as_deref()
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
pub struct Shape {
    inner: cxx::UniquePtr<ffi::Shape>,
    /// Whether `BRepCheck_Analyzer` finds the whole shape valid: checked at most once,
    /// since it costs as much as the shape is large.
    valid: std::cell::OnceCell<bool>,
}

impl Shape {
    fn new(inner: cxx::UniquePtr<ffi::Shape>) -> Self {
        Self {
            inner,
            valid: std::cell::OnceCell::new(),
        }
    }

    /// Whether OCCT's `BRepCheck_Analyzer` finds the whole shape valid. Checked the first
    /// time it's asked, unless [`Shape::with_validity`] already said.
    pub fn is_valid(&self) -> Result<bool, KernelError> {
        if let Some(&valid) = self.valid.get() {
            return Ok(valid);
        }
        let valid = ffi::is_valid(self.get()?)?;
        Ok(*self.valid.get_or_init(|| valid))
    }

    /// Records the shape's validity, already known (from the worker that made it, which
    /// checked), so it isn't checked again. A shape that crosses the worker boundary
    /// carries its validity this way.
    pub fn with_validity(mut self, valid: bool) -> Self {
        self.valid = std::cell::OnceCell::from(valid);
        self
    }

    fn get(&self) -> Result<&ffi::Shape, KernelError> {
        self.inner
            .as_ref()
            .ok_or_else(|| KernelError::new("shape is empty"))
    }
}

/// OCCT's length tolerance (`Precision::Confusion()`): lengths at or below it are zero.
pub const LENGTH_TOLERANCE: f64 = 1e-7;

/// A box with one corner at the origin and the opposite corner at `(dx, dy, dz)`.
/// Every size must be a finite length above [`LENGTH_TOLERANCE`]: that's this operation's
/// precondition.
pub fn make_box(dx: f64, dy: f64, dz: f64) -> Result<Shape, KernelError> {
    // OCCT would build a negative size as a box on the other side of the origin, lets NaN
    // past its checks, and rejects sizes at or below its tolerance without saying which;
    // all break the precondition, so they're refused here, naming the size. A feature that
    // wants a reversed extent (core's decision) asks for a positive box and places it.
    for (name, value) in [("dx", dx), ("dy", dy), ("dz", dz)] {
        if !(value.is_finite() && value > LENGTH_TOLERANCE) {
            return Err(KernelError::invalid_input(format!(
                "box size {name} must be a finite length above {LENGTH_TOLERANCE}, got {value}"
            )));
        }
    }
    let shape = ffi::make_box(dx, dy, dz)?;
    if shape.is_null() {
        return Err(KernelError::new("OCCT returned no box"));
    }
    Ok(Shape::new(shape))
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
    Ok(Shape::new(shape))
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
    let mut summary = ffi::summarize(shape.get()?)?;
    summary.valid = shape.is_valid()?;
    Ok(summary)
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
    // When a single-edge fillet fails, that edge is the one at fault; OCCT doesn't say
    // which of several it was.
    let blame = |error: KernelError| match edges {
        [edge] => error
            .with_element(format!("Edge{edge}"))
            .with_suggestion("try a smaller radius"),
        _ => error.with_suggestion(
            "try a smaller radius, or fillet the edges one at a time to find the one that fails",
        ),
    };
    let rounded = ffi::fillet(inner, edges, radius).map_err(|e| blame(e.into()))?;
    checked_result(shape, rounded, "fillet").map_err(blame)
}

/// The check every operation's result passes before it's returned. OCCT can report an
/// operation done whose result isn't valid (all of a 10 × 20 × 30 box's edges filleted at
/// r = 5.5 gives a "solid" of volume 7792); Silico refuses those rather than build on
/// them, as FreeCAD's features did from 87293fac9b.
///
/// The faces the operation made or changed must be valid (FreeCAD's check). That alone
/// misses some broken results (a second fillet on a filleted box can give a "solid" larger
/// than the box, valid where it changed), so when the input is valid the whole result must
/// be too: an invalid result from a valid input is the operation's fault. When the input
/// was already invalid, only the changed faces count, so its problem isn't blamed on the
/// operation. See docs/ports/fillet.md.
///
/// Each shape is checked as a whole at most once: the input's validity is usually already
/// known (the worker that made it checked it, and it came with the shape), and the
/// result's is kept with it for the reply and its summary.
fn checked_result(
    input: &Shape,
    result: cxx::UniquePtr<ffi::Shape>,
    operation: &str,
) -> Result<Shape, KernelError> {
    let refused = || {
        KernelError::of_kind(
            KernelErrorKind::InvalidResult,
            format!("the {operation}'s result isn't a valid solid"),
        )
    };
    let checked = result
        .as_ref()
        .ok_or_else(|| KernelError::new("OCCT returned no shape"))?;
    if !ffi::is_valid_where_changed(input.get()?, checked)? {
        return Err(refused());
    }
    // After the changed-faces check, which may repair SameParameter flags on the result in
    // place, so this judges the shape that's returned. (No input tested so far needs that
    // repair, so the order hasn't yet changed an answer; see docs/ports/fillet.md.)
    let valid = ffi::is_valid(checked)?;
    if input.is_valid()? && !valid {
        return Err(refused());
    }
    Ok(Shape::new(result).with_validity(valid))
}

/// The smallest mesh deflection allowed, as a fraction of the shape's size. Finer meshes
/// cost far more than they're worth for display: every tenth of the deflection is about
/// ten times the triangles and seventy times the time.
pub const MIN_RELATIVE_DEFLECTION: f64 = 1e-4;

/// Meshes `shape` for display, with no point of the mesh further than `deflection` from
/// the surface. Single-threaded, so the same shape always gives the same mesh, and it
/// leaves the shape as it was (OCCT would otherwise keep the mesh on it). Faces and edges
/// are in OCCT's order; the first edge is Edge1, as [`fillet`] numbers them.
pub fn tessellate(shape: &Shape, deflection: f64) -> Result<Mesh, KernelError> {
    if !(deflection.is_finite() && deflection > 0.0) {
        return Err(KernelError::invalid_input(format!(
            "mesh deflection must be a positive, finite length, got {deflection}"
        )));
    }
    let inner = shape.get()?;
    let floor = (ffi::bounding_diagonal(inner)? * MIN_RELATIVE_DEFLECTION).max(LENGTH_TOLERANCE);
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
    Ok(Mesh {
        positions: triples(raw.positions()),
        triangles: raw
            .triangles()
            .chunks_exact(3)
            .map(|t| [t[0], t[1], t[2]])
            .collect(),
        face_triangles: raw.face_triangles().to_vec(),
        edge_points: triples(raw.edge_points()),
        edge_point_counts: raw.edge_points_per_edge().to_vec(),
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
        Ok(super::Shape::new(ffi::debug_open_box()?))
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
