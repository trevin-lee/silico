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

    /// What OCCT measures of a shape: its volume and center of mass (taken as uniformly
    /// dense), its area, and how many distinct faces, edges, and vertices it has.
    /// [`summarize`](super::summarize) adds its validity.
    struct Measures {
        volume: f64,
        area: f64,
        center: Point3,
        faces: u32,
        edges: u32,
        vertices: u32,
    }

    /// A shape's volume and area.
    struct Amounts {
        volume: f64,
        area: f64,
    }

    /// A shape measured as FreeCAD measures it: volume, area, and center by OCCT's default
    /// integration, and its tight bounding box, from `low` to `high` if it's `bounded`.
    struct FreecadMeasures {
        volume: f64,
        area: f64,
        center: Point3,
        bounded: bool,
        low: Point3,
        high: Point3,
    }

    /// A kind of sub-shape, as the bridge reports history. `Other` is a kind history
    /// doesn't use (a wire, a shell), which Rust leaves out.
    #[repr(u8)]
    enum Kind {
        Vertex = 0,
        Edge = 1,
        Face = 2,
        Solid = 3,
        Other = 4,
    }

    /// How an input element relates to a result element.
    #[repr(u8)]
    enum Relation {
        /// The same sub-shape, unchanged.
        Kept,
        /// OCCT's `Modified`.
        Modified,
        /// OCCT's `Generated`.
        Generated,
    }

    /// One fact of an operation's history, as OCCT reports it: input `input`'s element
    /// (`input_kind`, `input_index`, numbered from 1) relates to the result's element
    /// (`output_kind`, `output_index`), or to something the result doesn't have
    /// (`output_index` 0). `position` is where OCCT's `Modified` or `Generated` list had
    /// it, from 1 (0 for `Kept`). For each input element, `Kept` comes first.
    struct HistoryEntry {
        input: u32,
        input_kind: Kind,
        input_index: u32,
        relation: Relation,
        output_kind: Kind,
        output_index: u32,
        position: u32,
    }

    /// What makes a shape read from bytes unusable, as `brep_defect` finds it.
    #[repr(u8)]
    enum BrepDefect {
        None,
        /// A location that mirrors or scales.
        MirrorOrScale,
        /// A location whose values aren't finite.
        NotFinite,
        /// A face with no surface.
        FaceWithoutSurface,
        /// A vertex whose point or tolerance isn't finite.
        PointNotFinite,
        /// An edge whose curve (in space or on a face), parameter range, stored polygon,
        /// or tolerance isn't finite.
        CurveNotFinite,
        /// A face whose surface, stored mesh, or tolerance isn't finite.
        SurfaceNotFinite,
    }

    /// Which boolean the bridge performs.
    #[repr(u8)]
    enum BooleanOp {
        Fuse,
        Cut,
        Common,
    }

    /// One edge, as the bridge reports it for a fillet or chamfer to decide on.
    struct EdgeFact {
        /// A point where a surface closes (a sphere's pole, a cone's tip).
        degenerate: bool,
        /// Bounding one of its faces twice: a seam.
        seam: bool,
        /// Its end vertices, numbered from 1 in `TopExp::MapShapes` order (0 when it has
        /// none); the same twice for a closed edge.
        first_vertex: u32,
        last_vertex: u32,
        /// How many of [`EdgeFacts::faces`] are its.
        face_count: u32,
    }

    /// A shape's edges in order, and the faces each lies on (numbered from 1, as OCCT lists
    /// them: a seam's face twice), edge after edge.
    struct EdgeFacts {
        edges: Vec<EdgeFact>,
        faces: Vec<u32>,
    }

    /// How many distinct faces, edges, vertices, and solids a shape has.
    struct Counts {
        faces: u32,
        edges: u32,
        vertices: u32,
        solids: u32,
    }

    unsafe extern "C++" {
        include!("silico-kernel/cpp/bridge.h");

        type Shape;

        fn make_box(dx: f64, dy: f64, dz: f64) -> Result<UniquePtr<Shape>>;
        fn make_cylinder(radius: f64, height: f64) -> Result<UniquePtr<Shape>>;
        fn make_sphere(radius: f64) -> Result<UniquePtr<Shape>>;
        fn make_cylinder_directly(radius: f64, height: f64) -> Result<UniquePtr<Shape>>;
        fn make_cone(radius1: f64, radius2: f64, height: f64) -> Result<UniquePtr<Shape>>;
        fn make_torus(major_radius: f64, minor_radius: f64) -> Result<UniquePtr<Shape>>;
        fn measure(shape: &Shape) -> Result<Measures>;
        fn edge_facts(shape: &Shape) -> Result<EdgeFacts>;

        type Dressing;
        fn new_fillet(shape: &Shape) -> Result<UniquePtr<Dressing>>;
        fn new_chamfer(shape: &Shape) -> Result<UniquePtr<Dressing>>;
        fn add_fillet(self: Pin<&mut Dressing>, edge: u32, radius: f64) -> Result<()>;
        fn add_distances(
            self: Pin<&mut Dressing>,
            edge: u32,
            face: u32,
            distance: f64,
            other: f64,
        ) -> Result<()>;
        fn add_distance_angle(
            self: Pin<&mut Dressing>,
            edge: u32,
            face: u32,
            distance: f64,
            radians: f64,
        ) -> Result<()>;
        fn contour_of(self: &Dressing, edge: u32) -> Result<u32>;
        fn contour_edges(self: &Dressing, contour: u32) -> Result<Vec<u32>>;
        fn build(
            self: Pin<&mut Dressing>,
            history: &mut Vec<HistoryEntry>,
        ) -> Result<UniquePtr<Shape>>;
        fn translate(
            shape: &Shape,
            dx: f64,
            dy: f64,
            dz: f64,
            history: &mut Vec<HistoryEntry>,
        ) -> Result<UniquePtr<Shape>>;
        fn make_polygon_face(points: &[Point3]) -> Result<UniquePtr<Shape>>;
        fn extrude(
            face: &Shape,
            dx: f64,
            dy: f64,
            dz: f64,
            history: &mut Vec<HistoryEntry>,
            start: &mut Vec<u32>,
            end: &mut Vec<u32>,
        ) -> Result<UniquePtr<Shape>>;
        fn revolve(
            face: &Shape,
            origin: Point3,
            direction: Point3,
            radians: f64,
            history: &mut Vec<HistoryEntry>,
            start: &mut Vec<u32>,
            end: &mut Vec<u32>,
        ) -> Result<UniquePtr<Shape>>;
        fn boolean(
            op: BooleanOp,
            base: &Shape,
            tool: &Shape,
            history: &mut Vec<HistoryEntry>,
            warnings: &mut Vec<String>,
        ) -> Result<UniquePtr<Shape>>;
        fn count_elements(shape: &Shape) -> Result<Counts>;
        fn rough_amounts(shape: &Shape) -> Result<Amounts>;
        fn measure_as_freecad(shape: &Shape) -> Result<FreecadMeasures>;
        fn is_valid_where_changed(input: &Shape, result: &Shape) -> Result<bool>;
        fn is_valid_where_changed_from_two(
            first: &Shape,
            second: &Shape,
            result: &Shape,
        ) -> Result<bool>;
        fn is_valid(shape: &Shape) -> Result<bool>;

        type Tessellation;
        fn bounding_diagonal(shape: &Shape) -> Result<f64>;
        fn tessellate(shape: &Shape, deflection: f64) -> Result<UniquePtr<Tessellation>>;
        fn positions(self: &Tessellation) -> &[f64];
        fn normals(self: &Tessellation) -> &[f32];
        fn triangles(self: &Tessellation) -> &[u32];
        fn face_triangles(self: &Tessellation) -> &[u32];
        fn edge_points(self: &Tessellation) -> &[f64];
        fn edge_points_per_edge(self: &Tessellation) -> &[u32];
        fn unmeshed_faces(self: &Tessellation) -> &[u32];
        fn shape_to_brep(shape: &Shape) -> Result<UniquePtr<CxxString>>;
        fn shape_from_brep(brep: &[u8]) -> Result<UniquePtr<Shape>>;
        fn shape_from_text_brep(brep: &[u8]) -> Result<UniquePtr<Shape>>;
        fn brep_defect(shape: &Shape) -> Result<BrepDefect>;
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
        #[cfg(feature = "test_ops")]
        fn debug_box_with_loose_edge() -> Result<UniquePtr<Shape>>;
        #[cfg(feature = "test_ops")]
        fn debug_empty_shape() -> Result<UniquePtr<Shape>>;
    }
}

pub use silico_protocol::{
    Axis, BooleanOp, ChamferSize, ElementCounts, ElementFate, ElementKind, ElementRef, Ends,
    History, InputHistory, LENGTH_TOLERANCE, MAX_LENGTH, Mesh, Primitive, Profile, Refusal,
    Reported, ShapeSummary,
};
use silico_protocol::{BrepDefect, FailureKind, HistoryDefect, Operation, Returned, Role};

mod dressing;
pub mod worker;

pub use dressing::{chamfer, fillet};

/// An operation refused or failed: why, as data (a protocol [`Refusal`]), and OCCT's own
/// words where OCCT threw (for example `Standard_DomainError: ...`). It stays inside the
/// worker: the worker sends the refusal and those words as a protocol `Failure`, and the
/// core names the elements and writes its structured `Error` from them. In-process, it reads
/// as the core would write it, naming elements as FreeCAD does (`Edge3`).
#[derive(Debug, Clone, PartialEq)]
pub struct KernelError(Box<Parts>);

/// A [`KernelError`]'s parts: boxed, so a `Result` carrying one stays small.
#[derive(Debug, Clone, PartialEq)]
struct Parts {
    refusal: Refusal,
    cause: Option<String>,
    /// The refusal described, with FreeCAD's names.
    message: String,
    suggestion: Option<String>,
    elements: Vec<String>,
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
    /// Silico's bridge did something it shouldn't: a bug.
    Internal,
}

impl KernelError {
    fn new(refusal: Refusal) -> Self {
        Self::caused(refusal, None)
    }

    fn caused(refusal: Refusal, cause: Option<String>) -> Self {
        let described = refusal.describe(cause.as_deref(), &|e| e.to_string());
        // OCCT's own words, where it threw; otherwise the refusal's.
        let message = match (&refusal, &cause) {
            (Refusal::Occt { .. }, Some(cause)) => cause.clone(),
            _ => described.message,
        };
        let elements = refusal.elements().iter().map(ToString::to_string).collect();
        Self(Box::new(Parts {
            refusal,
            cause,
            message,
            suggestion: described.suggestion,
            elements,
        }))
    }

    /// The same failure, as OCCT's doing `operation` (for what to suggest): an exception
    /// or an invalid result not yet attributed to an operation.
    fn during(self, operation: Operation) -> Self {
        let Parts { refusal, cause, .. } = *self.0;
        let refusal = match refusal {
            Refusal::Occt {
                operation: Operation::Other,
                edges,
            } => Refusal::Occt { operation, edges },
            Refusal::ResultInvalid {
                operation: Operation::Other,
                edges,
            } => Refusal::ResultInvalid { operation, edges },
            other => other,
        };
        Self::caused(refusal, cause)
    }

    /// The same failure of a fillet or chamfer, given `edges`.
    fn on_edges(self, given: &[u32]) -> Self {
        let Parts { refusal, cause, .. } = *self.0;
        let refusal = match refusal {
            Refusal::Occt { operation, .. } => Refusal::Occt {
                operation,
                edges: given.to_vec(),
            },
            Refusal::ResultInvalid { operation, .. } => Refusal::ResultInvalid {
                operation,
                edges: given.to_vec(),
            },
            other => other,
        };
        Self::caused(refusal, cause)
    }

    /// Whether the values were refused, OCCT failed, or OCCT's result was refused.
    pub fn kind(&self) -> KernelErrorKind {
        match self.0.refusal.kind() {
            FailureKind::InvalidInput => KernelErrorKind::InvalidInput,
            FailureKind::KernelFailed => KernelErrorKind::Failed,
            FailureKind::InvalidResult => KernelErrorKind::InvalidResult,
            FailureKind::UnknownOp | FailureKind::Internal => KernelErrorKind::Internal,
        }
    }

    /// Why, as data: what the worker sends.
    pub fn refusal(&self) -> &Refusal {
        &self.0.refusal
    }

    /// OCCT's own words, where it threw.
    pub fn cause(&self) -> Option<&str> {
        self.0.cause.as_deref()
    }

    /// What to try instead, when there's something to suggest.
    pub fn suggestion(&self) -> Option<&str> {
        self.0.suggestion.as_deref()
    }

    /// The first sub-element at fault (such as `Edge3`), when known.
    pub fn element(&self) -> Option<&str> {
        self.0.elements.first().map(String::as_str)
    }

    /// Every sub-element at fault, in order.
    pub fn elements(&self) -> &[String] {
        &self.0.elements
    }

    /// What went wrong, in OCCT's words where OCCT threw.
    pub fn message(&self) -> &str {
        &self.0.message
    }
}

impl fmt::Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "kernel error: {}", self.0.message)
    }
}

impl std::error::Error for KernelError {}

impl From<cxx::Exception> for KernelError {
    fn from(e: cxx::Exception) -> Self {
        Self::caused(
            Refusal::Occt {
                operation: Operation::Other,
                edges: Vec::new(),
            },
            Some(e.what().to_owned()),
        )
    }
}

impl From<silico_protocol::InvalidParameter> for KernelError {
    fn from(invalid: silico_protocol::InvalidParameter) -> Self {
        refused(None, invalid)
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
            .ok_or_else(|| KernelError::new(Refusal::NoShape))
    }
}

/// A solid primitive, placed as [`Primitive`] says. Values that can't make one are refused
/// before OCCT sees them, naming the parameter (the core refuses them first; this is the
/// worker's own guard). OCCT would build a box with a negative size on the other side of
/// the origin, lets NaN past its checks, and rejects lengths at or below its tolerance
/// without saying which.
pub fn make(primitive: &Primitive) -> Result<Shape, KernelError> {
    primitive.check()?;
    let made = |e: KernelError| {
        e.during(Operation::Primitive {
            name: primitive.name().to_owned(),
        })
    };
    let shape = match *primitive {
        Primitive::Box { dx, dy, dz } => ffi::make_box(dx, dy, dz).map_err(|e| made(e.into()))?,
        Primitive::Cylinder { radius, height } => {
            ffi::make_cylinder(radius, height).map_err(|e| made(e.into()))?
        }
        Primitive::Sphere { radius } => ffi::make_sphere(radius).map_err(|e| made(e.into()))?,
        Primitive::Cone {
            radius1,
            radius2,
            height,
        } => {
            // Radii within OCCT's tolerance of each other make a cylinder, built directly
            // (so numbered like OCCT's cylinder, not like the cylinder primitive), as
            // FreeCAD's Part::Cone does. OCCT's cone would refuse them.
            if (radius1 - radius2).abs() < LENGTH_TOLERANCE {
                ffi::make_cylinder_directly(radius1, height).map_err(|e| made(e.into()))?
            } else {
                // A radius of -0.0 is zero; written as 0.0, so the B-rep's bytes don't
                // depend on the sign of a zero.
                let unsigned_zero = |r: f64| if r == 0.0 { 0.0 } else { r };
                ffi::make_cone(unsigned_zero(radius1), unsigned_zero(radius2), height)
                    .map_err(|e| made(e.into()))?
            }
        }
        Primitive::Torus {
            major_radius,
            minor_radius,
        } => ffi::make_torus(major_radius, minor_radius).map_err(|e| made(e.into()))?,
    };
    if shape.is_null() {
        return Err(KernelError::new(Refusal::NothingReturned {
            what: Returned::Shape,
        }));
    }
    // A result OCCT builds isn't always valid (a sphere of radius 1e300 comes back with
    // three faces), refused as an operation's invalid result is. Within the allowed sizes
    // no input is known to reach this; it guards against one that does.
    let shape = Shape::new(shape);
    if !shape.is_valid()? {
        return Err(made(KernelError::new(Refusal::ResultInvalid {
            operation: Operation::Other,
            edges: Vec::new(),
        })));
    }
    Ok(shape)
}

/// A box with one corner at the origin and the opposite corner at `(dx, dy, dz)`.
pub fn make_box(dx: f64, dy: f64, dz: f64) -> Result<Shape, KernelError> {
    make(&Primitive::Box { dx, dy, dz })
}

/// The shape as OCCT's binary B-rep followed by its CRC-32 (4 bytes, little-endian):
/// how shapes cross the worker boundary, and how they'll be cached. OCCT reads many
/// corrupted B-reps without complaint, as a different shape, so [`from_brep`] checks the
/// CRC first.
pub fn to_brep(shape: &Shape) -> Result<Vec<u8>, KernelError> {
    let bytes = ffi::shape_to_brep(shape.get()?)?;
    let bytes = bytes
        .as_ref()
        .ok_or_else(|| {
            KernelError::new(Refusal::NothingReturned {
                what: Returned::Brep,
            })
        })?
        .as_bytes();
    let mut brep = Vec::with_capacity(bytes.len() + 4);
    brep.extend_from_slice(bytes);
    brep.extend_from_slice(&crc32fast::hash(bytes).to_le_bytes());
    Ok(brep)
}

/// A shape from [`to_brep`]'s output. Bytes whose CRC doesn't match are refused before
/// OCCT sees them. Then what OCCT reads but can't work on is refused: a location, on a
/// shape or on its geometry, that mirrors or scales (OCCT's operations disagree about which
/// side of the shape is solid); a face with no surface (OCCT's validity check crashes on
/// one with edges, and its booleans on one of only a triangulation, as mesh import makes);
/// and a value that isn't finite in a location, a vertex's point or tolerance, an edge's
/// tolerance, curve in space or on a face, range, or stored polygon, or a face's
/// tolerance, surface, or stored mesh (docs/roadmap/m1-kernel.md, M1.9). Not checked: a
/// vertex's parameters on its edges, and an edge's polygons on a mesh.
pub fn from_brep(brep: &[u8]) -> Result<Shape, KernelError> {
    let (body, crc) = brep
        .split_last_chunk::<4>()
        .ok_or_else(|| KernelError::new(Refusal::BrepTooShort))?;
    if crc32fast::hash(body) != u32::from_le_bytes(*crc) {
        return Err(KernelError::new(Refusal::BrepCorrupted));
    }
    checked(ffi::shape_from_brep(body)?)
}

/// A shape from OCCT's text B-rep (FreeCAD's `Shape.exportBrep`, the `.brp` files it
/// keeps in documents), refused as [`from_brep`] refuses one. Only for text OCCT wrote:
/// OCCT's text reader crashes or never returns on some corrupt text (a file cut short, a
/// negative radius, a trim that isn't finite), so only tools use it, in a process of their
/// own (`silico-brep-convert`), to bring shapes FreeCAD made to Silico for the oracle's
/// corpus of real parts.
pub fn from_text_brep(brep: &[u8]) -> Result<Shape, KernelError> {
    checked(ffi::shape_from_text_brep(brep)?)
}

/// A shape just read, unless OCCT can't work on it correctly.
fn checked(shape: cxx::UniquePtr<ffi::Shape>) -> Result<Shape, KernelError> {
    let Some(read) = shape.as_ref() else {
        return Err(KernelError::new(Refusal::NothingReturned {
            what: Returned::Shape,
        }));
    };
    let defect = match ffi::brep_defect(read)? {
        ffi::BrepDefect::None => return Ok(Shape::new(shape)),
        ffi::BrepDefect::MirrorOrScale => BrepDefect::MirrorOrScale,
        ffi::BrepDefect::NotFinite => BrepDefect::LocationNotFinite,
        ffi::BrepDefect::FaceWithoutSurface => BrepDefect::FaceWithoutSurface,
        ffi::BrepDefect::PointNotFinite => BrepDefect::PointNotFinite,
        ffi::BrepDefect::CurveNotFinite => BrepDefect::CurveNotFinite,
        ffi::BrepDefect::SurfaceNotFinite => BrepDefect::SurfaceNotFinite,
        other => {
            return Err(KernelError::new(Refusal::UnknownBrepDefect {
                code: u32::from(other.repr),
            }));
        }
    };
    Err(KernelError::new(Refusal::BrepDefect { defect }))
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
    let m = ffi::measure(shape.get()?)?;
    Ok(ShapeSummary {
        volume: m.volume,
        area: m.area,
        center: [m.center.x, m.center.y, m.center.z],
        faces: m.faces,
        edges: m.edges,
        vertices: m.vertices,
        valid: shape.is_valid()?,
    })
}

/// `shape` measured as FreeCAD measures it ([`silico_protocol::FreecadMeasures`]).
pub fn measure_as_freecad(shape: &Shape) -> Result<silico_protocol::FreecadMeasures, KernelError> {
    let m = ffi::measure_as_freecad(shape.get()?)?;
    let point = |p: ffi::Point3| [p.x, p.y, p.z];
    Ok(silico_protocol::FreecadMeasures {
        volume: m.volume,
        area: m.area,
        center: point(m.center),
        bounds: m.bounded.then(|| [point(m.low), point(m.high)]),
    })
}

/// `shape` moved by `offset`. Each face, edge, and vertex is modified into its moved
/// self, numbered the same (or kept, for an offset of zero). An offset must be finite and,
/// in each direction, at most [`MAX_LENGTH`].
pub fn translate(shape: &Shape, offset: [f64; 3]) -> Result<Built, KernelError> {
    silico_protocol::check_offset(offset)
        .map_err(|invalid| refused(Some(silico_protocol::OFFSET_FIELD), invalid))?;
    let inner = shape.get()?;
    let [dx, dy, dz] = offset;
    let mut entries = Vec::new();
    let moved = ffi::translate(inner, dx, dy, dz, &mut entries)
        .map_err(|e| KernelError::from(e).during(Operation::Translate))?;
    if moved.is_null() {
        return Err(KernelError::new(Refusal::NothingReturned {
            what: Returned::Shape,
        }));
    }
    // Moving a shape doesn't change whether it's valid: it has the input's validity, which
    // is checked now if no one had.
    let moved = Shape::new(moved).with_validity(shape.is_valid()?);
    let history = history_of(&[inner], moved.get()?, &entries)?;
    Ok(Built {
        shape: moved,
        history,
    })
}

/// A value refused by the protocol's check, named as the core names it
/// (`InvalidParameter::name`): `under`, the field the value is a part of, if any.
fn refused(under: Option<&str>, invalid: silico_protocol::InvalidParameter) -> KernelError {
    KernelError::new(Refusal::InvalidParameter {
        parameter: invalid.name(under),
        message: invalid.message,
    })
}

/// The face a profile bounds, its edges and vertices in its points' order. The profile is
/// checked first ([`Profile::check`]), and its plane returned with it.
fn profile_face(profile: &Profile) -> Result<(Shape, silico_protocol::ProfilePlane), KernelError> {
    let plane = profile.check().map_err(|invalid| refused(None, invalid))?;
    let points: Vec<ffi::Point3> = profile
        .points
        .iter()
        .map(|&[x, y, z]| ffi::Point3 { x, y, z })
        .collect();
    let face = ffi::make_polygon_face(&points)?;
    let Some(made) = face.as_ref() else {
        return Err(KernelError::new(Refusal::NothingReturned {
            what: Returned::Face,
        }));
    };
    // OCCT drops a corner it finds within its tolerance of the one before, measuring the
    // distance its own way: a side the rules measure as a hair over 1e-7 can be exactly
    // 1e-7 to OCCT. Every edge after it would then be misnumbered, so a face without one
    // edge and one vertex per corner is refused.
    let c = ffi::count_elements(made)?;
    let n = profile.points.len();
    if c.edges as usize != n || c.vertices as usize != n {
        // The pair OCCT merged is the closest consecutive pair.
        let i = profile.shortest_side();
        let corner = |i: usize| u32::try_from(i + 1).unwrap_or(u32::MAX);
        return Err(KernelError::new(Refusal::CornersMerged {
            corners: corner(n - 1),
            sides: c.edges,
            first: corner(i),
            second: corner((i + 1) % n),
        }));
    }
    Ok((Shape::new(face), plane))
}

/// A sweep's ends as the bridge reported them: faces the result doesn't have (0) left
/// out, sorted.
fn ends(start: Vec<u32>, end: Vec<u32>) -> Ends {
    let tidy = |mut faces: Vec<u32>| {
        faces.retain(|&f| f != 0);
        faces.sort_unstable();
        faces.dedup();
        faces
    };
    Ends {
        start: tidy(start),
        end: tidy(end),
    }
}

/// `profile` swept into a solid along its normal by `length`, or along `direction` far
/// enough to rise `length` along the normal (docs/ports/extrude-revolve.md). The history is
/// the profile's: its face (`Face1`), its edges (`Edge1` from its first point to its
/// second, ...), and its vertices (`Vertex1` its first point, ...), with the bottom and top
/// faces as its ends.
pub fn extrude(
    profile: &Profile,
    length: f64,
    direction: Option<[f64; 3]>,
) -> Result<Built, KernelError> {
    let (face, plane) = profile_face(profile)?;
    let [dx, dy, dz] = silico_protocol::extrusion_vector(&plane, length, direction)
        .map_err(|invalid| refused(None, invalid))?;
    let inner = face.get()?;
    let (mut entries, mut start, mut end) = (Vec::new(), Vec::new(), Vec::new());
    // OCCT has built every extrusion of a valid profile tried; should one fail, the
    // refusal says what to change.
    let solid = ffi::extrude(inner, dx, dy, dz, &mut entries, &mut start, &mut end)
        .map_err(|e| KernelError::from(e).during(Operation::Extrusion))?;
    let result = checked_result(&[&face], solid, Operation::Extrusion)?;
    let mut history = history_of(&[inner], result.get()?, &entries)?;
    history.ends = ends(start, end);
    Ok(Built {
        shape: result,
        history,
    })
}

/// `profile` swept around `axis`, which lies in its plane with the profile on one side, by
/// `angle` degrees, more than 0 and at most 360 (docs/ports/extrude-revolve.md). The
/// history is the profile's, as [`extrude`]'s is; a full turn has no ends.
pub fn revolve(profile: &Profile, axis: &Axis, angle: f64) -> Result<Built, KernelError> {
    let (face, plane) = profile_face(profile)?;
    let [dx, dy, dz] = silico_protocol::check_revolve(&plane, &profile.points, axis, angle)
        .map_err(|invalid| refused(None, invalid))?;
    let [ox, oy, oz] = axis.origin;
    let inner = face.get()?;
    let (mut entries, mut start, mut end) = (Vec::new(), Vec::new(), Vec::new());
    let radians = silico_protocol::radians(angle);
    // OCCT fails on some profiles the rules accept, a side exactly 1e-7 from the axis
    // among them (FreeCAD's OCCT too): the refusal says what to change.
    let solid = ffi::revolve(
        inner,
        ffi::Point3 {
            x: ox,
            y: oy,
            z: oz,
        },
        ffi::Point3 {
            x: dx,
            y: dy,
            z: dz,
        },
        radians,
        &mut entries,
        &mut start,
        &mut end,
    )
    .map_err(|e| KernelError::from(e).during(Operation::Revolve))?;
    let result = checked_result(&[&face], solid, Operation::Revolve)?;
    let mut history = history_of(&[inner], result.get()?, &entries)?;
    history.ends = ends(start, end);
    Ok(Built {
        shape: result,
        history,
    })
}

/// Fuses `tool` with `base`, cuts it from `base`, or keeps what they share
/// (docs/ports/booleans.md). The history has `base` as input 0 and `tool` as input 1. A
/// shape with nothing in it is refused, as either input (OCCT would fuse it, or cut it
/// away, as if nothing were wrong) or as the result (a common of shapes that don't
/// overlap, a cut that removes everything) or as a result of solids with no volume to
/// speak of. A valid result is otherwise accepted as OCCT gives it, as FreeCAD accepts it,
/// though OCCT gets some wrong (the port note lists them).
pub fn boolean(op: BooleanOp, base: &Shape, tool: &Shape) -> Result<Built, KernelError> {
    let name = op.name().to_lowercase();
    let operation = Operation::Boolean { op };
    let bridge_op = match op {
        BooleanOp::Fuse => ffi::BooleanOp::Fuse,
        BooleanOp::Cut => ffi::BooleanOp::Cut,
        BooleanOp::Common => ffi::BooleanOp::Common,
    };
    let (base_inner, tool_inner) = (base.get()?, tool.get()?);
    for (role, input) in [(Role::Base, base_inner), (Role::Tool, tool_inner)] {
        if is_empty(input)? {
            return Err(KernelError::new(Refusal::BooleanInputEmpty { op, role }));
        }
    }
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let built = ffi::boolean(
        bridge_op,
        base_inner,
        tool_inner,
        &mut entries,
        &mut warnings,
    )
    .map_err(|e| KernelError::from(e).during(operation.clone()))?;
    // FreeCAD accepts whatever valid result OCCT gives, warnings or not, and so does
    // Silico (docs/ports/booleans.md, Known wrong results). The warnings are logged.
    if !warnings.is_empty() {
        let alerts: std::collections::BTreeSet<&str> =
            warnings.iter().map(String::as_str).collect();
        tracing::warn!(
            "OCCT warned about the {name}: {}",
            alerts.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    if let Some(result) = built.as_ref()
        && (is_empty(result)? || has_no_volume(result, &[base_inner, tool_inner])?)
    {
        // OCCT found nothing (or a result with no volume), which it can do for shapes that
        // do overlap (where faces or edges nearly meet, or across a torus's seam): the
        // refusal's suggestion doesn't claim more than that.
        return Err(KernelError::new(Refusal::BooleanResultEmpty { op }));
    }
    let result = checked_result(&[base, tool], built, operation)?;
    let history = history_of(&[base_inner, tool_inner], result.get()?, &entries)?;
    Ok(Built {
        shape: result,
        history,
    })
}

/// Whether a boolean of solids gave a result with no volume to speak of: no solid at all,
/// or a mean thickness (volume over area) of at most OCCT's length tolerance, or less
/// than none. OCCT returns such results for some booleans it gets wrong (a torus cut by
/// a copy of itself moved 0.5 across its seam: two whole torus surfaces of volume
/// -8e-11, a center 2.5e11 away), and they're as empty as an empty compound; their mean
/// thickness is under 1e-11. A real result is far thicker, however little of its
/// bounding box it fills: a 1 × 2 × 3 box's common with a copy 1e-4 into it is 5e-5
/// thick on average, a 0.01 mm wire 0.005. Not judged when an input has no solid in it.
fn has_no_volume(result: &ffi::Shape, inputs: &[&ffi::Shape]) -> Result<bool, KernelError> {
    for input in inputs {
        if ffi::count_elements(input)?.solids == 0 {
            return Ok(false);
        }
    }
    if ffi::count_elements(result)?.solids == 0 {
        return Ok(true);
    }
    // The quick measure clears most results, against their size: on these broken shapes
    // its error grows with the model (it read the cut above as 4e-6, and the same cut 100
    // times larger as 1.9, both really negative), so it's compared with the cube of the
    // bounding diagonal, not the area. What it doesn't clear (results that fill little of
    // their box, too) is measured precisely, and judged by its mean thickness.
    let rough = ffi::rough_amounts(result)?;
    let diagonal = ffi::bounding_diagonal(result)?;
    if rough.volume > 1e-5 * diagonal * diagonal * diagonal {
        return Ok(false);
    }
    let precise = ffi::measure(result)?;
    Ok(precise.volume.is_nan() || precise.volume <= LENGTH_TOLERANCE * precise.area)
}

/// Whether the shape has nothing in it: no faces, edges, or vertices.
fn is_empty(shape: &ffi::Shape) -> Result<bool, KernelError> {
    let c = ffi::count_elements(shape)?;
    Ok(c.faces == 0 && c.edges == 0 && c.vertices == 0)
}

/// A shape an operation built, and what the operation did to its inputs' faces, edges,
/// and vertices (docs/ports/history.md).
pub struct Built {
    pub shape: Shape,
    pub history: History,
}

/// The history the bridge reported for an operation on `inputs` giving `result`.
fn history_of(
    inputs: &[&ffi::Shape],
    result: &ffi::Shape,
    entries: &[ffi::HistoryEntry],
) -> Result<History, KernelError> {
    let counts = |shape: &ffi::Shape| -> Result<ElementCounts, KernelError> {
        let c = ffi::count_elements(shape)?;
        Ok(ElementCounts {
            faces: c.faces,
            edges: c.edges,
            vertices: c.vertices,
            solids: c.solids,
        })
    };
    let input_counts = inputs
        .iter()
        .map(|input| counts(input))
        .collect::<Result<Vec<_>, _>>()?;
    assemble_history(&input_counts, counts(result)?, entries)
}

/// The history as Silico keeps it, from the bridge's facts: each input element's fate,
/// with what OCCT named that the result doesn't have left out (its position still counts),
/// an element "modified" into itself counted as kept, a "modified" of another kind left
/// out (FreeCAD's rule: OCCT's `Modified` means the same kind), and each list sorted by
/// element so its order never depends on OCCT's.
fn assemble_history(
    inputs: &[ElementCounts],
    result: ElementCounts,
    entries: &[ffi::HistoryEntry],
) -> Result<History, KernelError> {
    let mut history = History {
        inputs: inputs
            .iter()
            .map(|c| InputHistory {
                faces: vec![ElementFate::default(); c.faces as usize],
                edges: vec![ElementFate::default(); c.edges as usize],
                vertices: vec![ElementFate::default(); c.vertices as usize],
            })
            .collect(),
        result,
        ends: Ends::default(),
    };
    let malformed = |defect| KernelError::new(Refusal::MalformedHistory { defect });
    for e in entries {
        let input = history
            .inputs
            .get_mut(e.input as usize)
            .ok_or_else(|| malformed(HistoryDefect::UnknownInput))?;
        let input_kind = element_kind(e.input_kind);
        let fates = match input_kind {
            Some(ElementKind::Face) => &mut input.faces,
            Some(ElementKind::Edge) => &mut input.edges,
            Some(ElementKind::Vertex) => &mut input.vertices,
            _ => return Err(malformed(HistoryDefect::WrongKind)),
        };
        let fate = (e.input_index as usize)
            .checked_sub(1)
            .and_then(|i| fates.get_mut(i))
            .ok_or_else(|| malformed(HistoryDefect::UnknownElement))?;
        // A wire or a shell, or something the result doesn't have (OCCT names those, as
        // for a revolve's vertices): not part of the history.
        let Some(kind) = element_kind(e.output_kind) else {
            continue;
        };
        if e.output_index == 0 {
            continue;
        }
        let output = Reported {
            element: ElementRef {
                kind,
                index: e.output_index,
            },
            position: e.position,
        };
        let listed = |list: &[Reported]| list.iter().any(|r| r.element == output.element);
        match e.relation {
            ffi::Relation::Kept => fate.kept = Some(e.output_index),
            ffi::Relation::Modified => {
                let same_kind = Some(kind) == input_kind;
                let itself = same_kind && fate.kept == Some(e.output_index);
                if same_kind && !itself && !listed(&fate.modified) {
                    fate.modified.push(output);
                }
            }
            ffi::Relation::Generated => {
                if !listed(&fate.generated) {
                    fate.generated.push(output);
                }
            }
            _ => return Err(malformed(HistoryDefect::UnknownRelation)),
        }
    }
    for input in &mut history.inputs {
        for fate in input
            .faces
            .iter_mut()
            .chain(&mut input.edges)
            .chain(&mut input.vertices)
        {
            fate.modified.sort();
            fate.generated.sort();
        }
    }
    Ok(history)
}

fn element_kind(kind: ffi::Kind) -> Option<ElementKind> {
    match kind {
        ffi::Kind::Vertex => Some(ElementKind::Vertex),
        ffi::Kind::Edge => Some(ElementKind::Edge),
        ffi::Kind::Face => Some(ElementKind::Face),
        ffi::Kind::Solid => Some(ElementKind::Solid),
        _ => None,
    }
}

/// The check every operation's result passes before it's returned. OCCT can report an
/// operation done whose result isn't valid (all of a 10 × 20 × 30 box's edges filleted at
/// r = 5.5 gives a "solid" of volume 7792); Silico refuses those rather than build on
/// them, as FreeCAD's features did from 87293fac9b.
///
/// The faces the operation made or changed must be valid (FreeCAD's check). That alone
/// misses some broken results (a second fillet on a filleted box can give a "solid" larger
/// than the box, valid where it changed), so when the input is valid the whole result must
/// be too: an invalid result from valid inputs is the operation's fault. When an input
/// was already invalid, only the changed faces count, so its problem isn't blamed on the
/// operation. See docs/ports/fillet.md.
///
/// Each shape is checked as a whole at most once: the input's validity is usually already
/// known (the worker that made it checked it, and it came with the shape), and the
/// result's is kept with it for the reply and its summary.
fn checked_result(
    inputs: &[&Shape],
    result: cxx::UniquePtr<ffi::Shape>,
    operation: Operation,
) -> Result<Shape, KernelError> {
    let refused = || {
        KernelError::new(Refusal::ResultInvalid {
            operation: operation.clone(),
            edges: Vec::new(),
        })
    };
    let checked = result.as_ref().ok_or_else(|| {
        KernelError::new(Refusal::NothingReturned {
            what: Returned::Shape,
        })
    })?;
    let valid_where_changed = match inputs {
        [input] => ffi::is_valid_where_changed(input.get()?, checked)?,
        [first, second] => {
            ffi::is_valid_where_changed_from_two(first.get()?, second.get()?, checked)?
        }
        _ => {
            return Err(KernelError::new(Refusal::ResultCheckMissing {
                inputs: u32::try_from(inputs.len()).unwrap_or(u32::MAX),
            }));
        }
    };
    if !valid_where_changed {
        return Err(refused());
    }
    // After the changed-faces check, which may repair SameParameter flags on the result in
    // place, so this judges the shape that's returned. (No input tested so far needs that
    // repair, so the order hasn't yet changed an answer; see docs/ports/fillet.md.)
    let valid = ffi::is_valid(checked)?;
    if !valid && every_input_valid(inputs)? {
        return Err(refused());
    }
    Ok(Shape::new(result).with_validity(valid))
}

fn every_input_valid(inputs: &[&Shape]) -> Result<bool, KernelError> {
    for input in inputs {
        if !input.is_valid()? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The smallest mesh deflection allowed, as a fraction of the shape's size. Finer meshes
/// cost far more than they're worth for display: every tenth of the deflection is about
/// ten times the triangles and seventy times the time.
pub const MIN_RELATIVE_DEFLECTION: f64 = 1e-4;

/// Meshes `shape` for display, with no point of the mesh further than `deflection` from
/// the surface. Single-threaded, so the same shape always gives the same mesh, and it
/// leaves the shape as it was (OCCT would otherwise keep the mesh on it). Faces and edges
/// are in OCCT's order; the first edge is Edge1, as [`fillet`] numbers them. Each vertex
/// carries its face's surface normal there, pointing out of the solid
/// (docs/ports/tessellation.md).
pub fn tessellate(shape: &Shape, deflection: f64) -> Result<Mesh, KernelError> {
    if !(deflection.is_finite() && deflection > 0.0) {
        return Err(KernelError::new(Refusal::DeflectionInvalid { deflection }));
    }
    let inner = shape.get()?;
    let diagonal = ffi::bounding_diagonal(inner)?;
    if !diagonal.is_finite() {
        return Err(KernelError::new(Refusal::ExtentNotFinite));
    }
    let floor = (diagonal * MIN_RELATIVE_DEFLECTION).max(LENGTH_TOLERANCE);
    if deflection < floor {
        return Err(KernelError::new(Refusal::DeflectionTooFine {
            deflection,
            floor,
        }));
    }
    // A mesher that fails outright is almost always broken geometry, not the deflection:
    // the refusal says so.
    let raw = ffi::tessellate(inner, deflection)
        .map_err(|e| KernelError::from(e).during(Operation::Tessellate))?;
    let raw = raw.as_ref().ok_or_else(|| {
        KernelError::new(Refusal::NothingReturned {
            what: Returned::Mesh,
        })
    })?;
    // A face OCCT leaves unmeshed (a sliver too narrow for its tolerance, as a box chamfered
    // nearly through has, at any deflection) has no triangles, if the shape is valid: as
    // FreeCAD gives such a face an empty domain (`TopoShape::getDomains`). On a shape that
    // isn't valid it's refused, naming the face: there it says something is broken (a face
    // whose edges cross can't be meshed, and its area, its halves cancelling, says nothing).
    let unmeshed = raw.unmeshed_faces();
    if let Some(&face) = unmeshed.first() {
        if !shape.is_valid()? {
            return Err(KernelError::new(Refusal::FaceUnmeshed { face: face + 1 }));
        }
        tracing::debug!(faces = ?unmeshed, "faces OCCT couldn't mesh, left without triangles");
    }
    let triples = |values: &[f64]| -> Vec<[f64; 3]> {
        values.chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect()
    };
    let mesh = Mesh {
        positions: triples(raw.positions()),
        normals: raw
            .normals()
            .chunks_exact(3)
            .map(|n| [n[0], n[1], n[2]])
            .collect(),
        triangles: raw
            .triangles()
            .chunks_exact(3)
            .map(|t| [t[0], t[1], t[2]])
            .collect(),
        face_triangles: raw.face_triangles().to_vec(),
        edge_points: triples(raw.edge_points()),
        edge_point_counts: raw.edge_points_per_edge().to_vec(),
    };
    // Geometry that isn't finite is refused when it's read, and a shape far enough out to
    // overflow its coordinates overflows its extent first: a mesh's coordinates are finite.
    // Anything else wrong with the mesh would be the bridge's bug, which the core's own
    // check catches (and stops the worker for).
    Ok(mesh)
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

    /// A compound of a 10 × 20 × 30 box and an edge on no face (Edge13).
    pub fn box_with_loose_edge() -> Result<super::Shape, KernelError> {
        Ok(super::Shape::new(ffi::debug_box_with_loose_edge()?))
    }

    /// A compound with nothing in it.
    pub fn empty_shape() -> Result<super::Shape, KernelError> {
        Ok(super::Shape::new(ffi::debug_empty_shape()?))
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

    use super::ffi::{HistoryEntry, Kind, Relation};
    use super::{
        ElementCounts, ElementKind, ElementRef, KernelErrorKind, Reported, assemble_history,
    };

    fn at(position: u32, e: HistoryEntry) -> HistoryEntry {
        HistoryEntry { position, ..e }
    }

    fn reported(kind: ElementKind, index: u32, position: u32) -> Reported {
        Reported {
            element: ElementRef { kind, index },
            position,
        }
    }

    fn entry(
        input_index: u32,
        relation: Relation,
        output_kind: Kind,
        output_index: u32,
    ) -> HistoryEntry {
        HistoryEntry {
            input: 0,
            input_kind: Kind::Face,
            input_index,
            relation,
            output_kind,
            output_index,
            position: 1,
        }
    }

    const ONE_FACE: ElementCounts = ElementCounts {
        faces: 1,
        edges: 0,
        vertices: 0,
        solids: 0,
    };
    const RESULT: ElementCounts = ElementCounts {
        faces: 3,
        edges: 2,
        vertices: 0,
        solids: 1,
    };

    #[test]
    fn history_keeps_what_the_result_has_and_drops_the_rest() {
        let h = assemble_history(
            &[ONE_FACE],
            RESULT,
            &[
                at(0, entry(1, Relation::Kept, Kind::Face, 2)),
                // Modified into itself (as transforms report): kept, not modified.
                at(1, entry(1, Relation::Modified, Kind::Face, 2)),
                // Something the result doesn't have still counts for the next position.
                at(2, entry(1, Relation::Modified, Kind::Face, 0)),
                at(3, entry(1, Relation::Modified, Kind::Face, 3)),
                at(4, entry(1, Relation::Modified, Kind::Face, 3)),
                // Modified into another kind: not what Modified means (FreeCAD skips it).
                at(5, entry(1, Relation::Modified, Kind::Edge, 1)),
                // Reported out of order, twice, and with a kind history doesn't use.
                at(1, entry(1, Relation::Generated, Kind::Solid, 1)),
                at(2, entry(1, Relation::Generated, Kind::Other, 1)),
                at(3, entry(1, Relation::Generated, Kind::Edge, 2)),
                at(4, entry(1, Relation::Generated, Kind::Edge, 2)),
                at(5, entry(1, Relation::Generated, Kind::Edge, 1)),
            ],
        )
        .unwrap();
        let fate = &h.inputs[0].faces[0];
        assert_eq!(fate.kept, Some(2));
        assert_eq!(fate.modified, [reported(ElementKind::Face, 3, 3)]);
        // Sorted by element (edges before solids), each with OCCT's position.
        assert_eq!(
            fate.generated,
            [
                reported(ElementKind::Edge, 1, 5),
                reported(ElementKind::Edge, 2, 3),
                reported(ElementKind::Solid, 1, 1),
            ]
        );
    }

    #[test]
    fn every_list_of_every_kind_is_sorted() {
        let one_each = ElementCounts {
            faces: 1,
            edges: 1,
            vertices: 1,
            solids: 0,
        };
        let result = ElementCounts {
            faces: 3,
            edges: 3,
            vertices: 3,
            solids: 0,
        };
        let of = |input_kind, relation, output_kind, output_index, position| HistoryEntry {
            input: 0,
            input_kind,
            input_index: 1,
            relation,
            output_kind,
            output_index,
            position,
        };
        let h = assemble_history(
            &[one_each],
            result,
            &[
                of(Kind::Face, Relation::Modified, Kind::Face, 3, 1),
                of(Kind::Face, Relation::Modified, Kind::Face, 1, 2),
                of(Kind::Edge, Relation::Modified, Kind::Edge, 3, 1),
                of(Kind::Edge, Relation::Modified, Kind::Edge, 1, 2),
                of(Kind::Vertex, Relation::Generated, Kind::Edge, 3, 1),
                of(Kind::Vertex, Relation::Generated, Kind::Edge, 1, 2),
            ],
        )
        .unwrap();
        let i = &h.inputs[0];
        assert_eq!(
            i.faces[0].modified,
            [
                reported(ElementKind::Face, 1, 2),
                reported(ElementKind::Face, 3, 1)
            ]
        );
        assert_eq!(
            i.edges[0].modified,
            [
                reported(ElementKind::Edge, 1, 2),
                reported(ElementKind::Edge, 3, 1)
            ]
        );
        assert_eq!(
            i.vertices[0].generated,
            [
                reported(ElementKind::Edge, 1, 2),
                reported(ElementKind::Edge, 3, 1)
            ]
        );
    }

    #[test]
    fn a_malformed_report_is_an_error() {
        for bad in [
            // An input element the input doesn't have, numbered 0 or past the end.
            entry(0, Relation::Kept, Kind::Face, 1),
            entry(2, Relation::Kept, Kind::Face, 1),
            HistoryEntry {
                input: 1,
                ..entry(1, Relation::Kept, Kind::Face, 1)
            },
            HistoryEntry {
                input_kind: Kind::Solid,
                ..entry(1, Relation::Kept, Kind::Face, 1)
            },
            HistoryEntry {
                relation: Relation { repr: 9 },
                ..entry(1, Relation::Kept, Kind::Face, 1)
            },
        ] {
            let err = assemble_history(&[ONE_FACE], RESULT, &[bad]).unwrap_err();
            assert!(err.message().contains("history"), "{err}");
            assert_eq!(err.kind(), KernelErrorKind::Internal);
            assert!(err.suggestion().unwrap().contains("bug"), "{err}");
        }
    }

    #[test]
    fn an_invalid_boolean_result_says_the_shapes_nearly_meet() {
        // No input is known to make OCCT return an invalid boolean result, so the advice is
        // tested on its own: for an invalid result, and something else for OCCT failing.
        use silico_protocol::{BooleanOp, Operation, Refusal};

        use super::KernelError;
        let cut = Operation::Boolean { op: BooleanOp::Cut };
        let invalid = KernelError::new(Refusal::ResultInvalid {
            operation: cut.clone(),
            edges: Vec::new(),
        });
        assert_eq!(
            invalid.suggestion(),
            Some(
                "faces or edges of the two shapes nearly meet: move one so they clearly overlap or stay clearly apart"
            )
        );
        let failed = KernelError::new(Refusal::Occt {
            operation: cut,
            edges: Vec::new(),
        });
        assert_eq!(
            failed.suggestion(),
            Some("check that the shapes are solids that overlap")
        );
    }
}
