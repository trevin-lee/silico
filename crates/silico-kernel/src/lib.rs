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
    /// dense), its area, how many distinct faces, edges, and vertices it has, and its tight
    /// bounding box, from `low` to `high` if it's `bounded`. [`summarize`](super::summarize)
    /// adds its validity.
    struct Measures {
        volume: f64,
        area: f64,
        center: Point3,
        faces: u32,
        edges: u32,
        vertices: u32,
        bounded: bool,
        low: Point3,
        high: Point3,
    }

    /// A shape's tight bounding box, from `low` to `high`, if it's `bounded` (has geometry).
    struct Bounds {
        bounded: bool,
        low: Point3,
        high: Point3,
    }

    /// Whether a shape's faces all lie on parallel planes (`flat`), and their unit normal.
    struct Flat {
        flat: bool,
        normal: Point3,
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

    /// Where a point is against a solid, as OCCT's classifier says.
    #[repr(u8)]
    enum PointState {
        Inside,
        On,
        Outside,
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

    /// One of a pair of elements a boolean's OCCT skipped intersecting
    /// (`BOPAlgo_AlertIntersectionOfPairOfShapesFailed`): its kind, the input it's of (0
    /// the base, 1 the tool, 2 neither: an element OCCT made of one), and its number there
    /// (0 for neither).
    #[derive(Clone, Copy)]
    struct SkippedElement {
        kind: Kind,
        input: u8,
        index: u32,
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

    /// How a shape's elements bound each other (`silico_protocol::Topology`), flattened: for
    /// each face in order, its edges (`face_edge_counts` of them) and its outer wire's;
    /// for each edge, its vertices. Each element numbered from 1 as the shape numbers it
    /// (0 if it somehow isn't one of the shape's).
    struct TopologyFacts {
        face_edges: Vec<u32>,
        face_edge_counts: Vec<u32>,
        outer_edges: Vec<u32>,
        outer_edge_counts: Vec<u32>,
        edge_vertices: Vec<u32>,
        edge_vertex_counts: Vec<u32>,
    }

    /// What kind of curve a [`FaceCurve`] is.
    #[repr(u8)]
    enum CurveKind {
        Line,
        Arc,
        Circle,
    }

    /// One curve of a profile's loop, in model space, for `make_faces`: a line from point
    /// `start` to point `end` (numbered from 0 in the points given); an arc of the circle of
    /// `radius` about `center`, turning counter-clockwise about `axis` from the plane's x
    /// axis, from point `start` at parameter `first` to point `end` at `last` (`first` <
    /// `last`, any representative of the arc's range: the bridge moves it by whole turns to
    /// the one OCCT's trimmed curves give it); or a whole circle about `axis` (its points and
    /// parameters unused).
    struct FaceCurve {
        kind: CurveKind,
        start: u32,
        end: u32,
        center: Point3,
        axis: Point3,
        radius: f64,
        first: f64,
        last: f64,
    }

    /// A face's surface type, as OCCT's `GeomAbs_SurfaceType` names it.
    #[repr(u8)]
    enum SurfaceType {
        Plane,
        Cylinder,
        Cone,
        Sphere,
        Torus,
        Bezier,
        Bspline,
        Revolution,
        Extrusion,
        Offset,
        Other,
    }

    /// An edge's curve type, as OCCT's `GeomAbs_CurveType` names it.
    #[repr(u8)]
    enum CurveType {
        Line,
        Circle,
        Ellipse,
        Hyperbola,
        Parabola,
        Bezier,
        Bspline,
        Offset,
        Other,
    }

    /// A face as `describe` finds it (`silico_protocol::FaceDescription`): its surface, area,
    /// and centre of area; a plane's outward `normal`; a cylinder's, cone's, sphere's, or
    /// torus's axis (`origin`, `direction`); a cylinder's, cone's, or sphere's `radius`, or
    /// a torus's major `radius` and minor `radius2`; and a cone's half-angle, `angle`.
    /// Unused fields are 0.
    struct FaceOut {
        surface: SurfaceType,
        area: f64,
        centroid: Point3,
        normal: Point3,
        origin: Point3,
        direction: Point3,
        radius: f64,
        radius2: f64,
        angle: f64,
    }

    /// An edge as `describe` finds it (`silico_protocol::EdgeDescription`): its curve,
    /// length, ends, and middle, whether it's degenerated; a circle's or ellipse's axis
    /// (`origin`, `direction`), its `radius` (an ellipse's major) and an ellipse's minor
    /// `radius2`; and a Bézier's or B-spline's `degree` and `poles`. Unused fields are 0.
    struct EdgeOut {
        curve: CurveType,
        length: f64,
        start: Point3,
        end: Point3,
        middle: Point3,
        degenerated: bool,
        origin: Point3,
        direction: Point3,
        radius: f64,
        radius2: f64,
        degree: u32,
        poles: u32,
    }

    /// A batch of a shape's elements of one kind, in `TopExp::MapShapes` order (the others
    /// empty), and how many of that kind the shape has.
    struct DescribeOut {
        total: u32,
        faces: Vec<FaceOut>,
        edges: Vec<EdgeOut>,
        vertices: Vec<Point3>,
    }

    /// The least distance between two elements (`distance`): whether both were found (if
    /// not, how many of each one's kind its shape has), whether OCCT found the distance, its
    /// value, and each pair of points where OCCT found it met, two points a pair (on the
    /// first element, then on the second), in OCCT's order.
    struct DistanceOut {
        found: bool,
        first_total: u32,
        second_total: u32,
        done: bool,
        value: f64,
        pairs: Vec<Point3>,
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
        fn measure(shape: &Shape, precision: f64) -> Result<Measures>;
        fn tight_bounds(shape: &Shape) -> Result<Bounds>;
        fn classify_point(shape: &Shape, point: &Point3) -> Result<PointState>;
        fn flat_normal(shape: &Shape) -> Result<Flat>;
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
        fn faulty_contours(self: &Dressing) -> Result<Vec<u32>>;
        fn faulty_vertices(self: &Dressing) -> Result<Vec<u32>>;
        fn contour_count(self: &Dressing) -> Result<u32>;
        fn contour_failed(self: &Dressing, contour: u32) -> Result<bool>;
        fn translate(
            shape: &Shape,
            dx: f64,
            dy: f64,
            dz: f64,
            history: &mut Vec<HistoryEntry>,
        ) -> Result<UniquePtr<Shape>>;
        fn make_polygon_face(points: &[Point3]) -> Result<UniquePtr<Shape>>;
        fn make_faces(
            normal: Point3,
            x_axis: Point3,
            points: &[Point3],
            curves: &[FaceCurve],
            loop_curves: &[u32],
            region_loops: &[u32],
            history: &mut Vec<HistoryEntry>,
        ) -> Result<UniquePtr<Shape>>;
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
        fn sweep_prism(
            faces: &Shape,
            start: Point3,
            vector: Point3,
            history: &mut Vec<HistoryEntry>,
        ) -> Result<UniquePtr<Shape>>;
        fn sweep_revolution(
            faces: &Shape,
            origin: Point3,
            direction: Point3,
            start: f64,
            angle: f64,
            history: &mut Vec<HistoryEntry>,
        ) -> Result<UniquePtr<Shape>>;
        fn boolean(
            op: BooleanOp,
            base: &Shape,
            tool: &Shape,
            history: &mut Vec<HistoryEntry>,
            warnings: &mut Vec<String>,
            skipped: &mut Vec<SkippedElement>,
        ) -> Result<UniquePtr<Shape>>;
        fn count_elements(shape: &Shape) -> Result<Counts>;
        fn topology(shape: &Shape) -> Result<TopologyFacts>;
        fn rough_amounts(shape: &Shape) -> Result<Amounts>;
        fn measure_as_freecad(shape: &Shape) -> Result<FreecadMeasures>;
        fn describe(
            shape: &Shape,
            kind: Kind,
            first: u32,
            count: u32,
            precision: f64,
        ) -> Result<DescribeOut>;
        fn distance(
            first_shape: &Shape,
            first_kind: Kind,
            first: u32,
            second_shape: &Shape,
            second_kind: Kind,
            second: u32,
        ) -> Result<DistanceOut>;
        fn point_distance(
            shape: &Shape,
            kind: Kind,
            index: u32,
            point: &Point3,
        ) -> Result<DistanceOut>;
        fn is_valid_where_changed(input: &Shape, result: &Shape) -> Result<bool>;
        fn is_valid_where_changed_from_two(
            first: &Shape,
            second: &Shape,
            result: &Shape,
        ) -> Result<bool>;
        fn is_valid(shape: &Shape) -> Result<bool>;

        type Tessellation;
        fn bounding_diagonal(shape: &Shape) -> Result<f64>;
        fn bounding_reach(shape: &Shape) -> Result<f64>;
        fn tessellate(shape: &Shape, deflection: f64) -> Result<UniquePtr<Tessellation>>;
        fn positions(self: &Tessellation) -> &[f64];
        fn normals(self: &Tessellation) -> &[f32];
        fn triangles(self: &Tessellation) -> &[u32];
        fn face_triangles(self: &Tessellation) -> &[u32];
        fn edge_points(self: &Tessellation) -> &[f64];
        fn edge_points_per_edge(self: &Tessellation) -> &[u32];

        type ExportTriangulation;
        fn export_mesh(
            shape: &Shape,
            tolerance: f64,
            angle: f64,
            force_face_deflection: bool,
        ) -> Result<UniquePtr<ExportTriangulation>>;
        fn nodes(self: &ExportTriangulation) -> &[f64];
        fn face_nodes(self: &ExportTriangulation) -> &[u32];
        fn triangles(self: &ExportTriangulation) -> &[u32];
        fn face_triangles(self: &ExportTriangulation) -> &[u32];
        fn face_numbers(self: &ExportTriangulation) -> &[u32];
        fn face_deflections(self: &ExportTriangulation) -> &[f64];
        fn face_shells(self: &ExportTriangulation) -> &[u32];
        fn face_tolerances(self: &ExportTriangulation) -> &[f64];
        fn unoriented_faces(self: &ExportTriangulation) -> &[u32];
        fn unoriented_shells(self: &ExportTriangulation) -> &[u32];
        fn shell_solids(self: &ExportTriangulation) -> &[u32];
        fn shell_outer(self: &ExportTriangulation) -> &[u8];
        fn solids(self: &ExportTriangulation) -> u32;
        fn loose_faces(self: &ExportTriangulation) -> &[u32];
        fn edge_faces(self: &ExportTriangulation) -> &[u32];
        fn edge_numbers(self: &ExportTriangulation) -> &[u32];
        fn edge_degenerate(self: &ExportTriangulation) -> &[u8];
        fn edge_vertices(self: &ExportTriangulation) -> &[u32];
        fn edge_deflections(self: &ExportTriangulation) -> &[f64];
        fn edge_node_counts(self: &ExportTriangulation) -> &[u32];
        fn edge_nodes(self: &ExportTriangulation) -> &[u32];

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
        #[cfg(feature = "test_ops")]
        fn debug_box_with_wireless_face() -> Result<UniquePtr<Shape>>;
        #[cfg(feature = "test_ops")]
        fn debug_box_with_empty_wire_face() -> Result<UniquePtr<Shape>>;
        #[cfg(feature = "test_ops")]
        fn debug_box_with_internal_wire_face() -> Result<UniquePtr<Shape>>;
    }
}

pub use silico_protocol::{
    Axis, BooleanOp, ChamferSize, Curve, ElementCounts, ElementFate, ElementKind, ElementRef, Ends,
    ExportMesh, FaceRegions, History, InputHistory, LENGTH_TOLERANCE, MAX_LENGTH,
    MIN_RELATIVE_DEFLECTION, Mesh, MeshAt, PointPlace, Primitive, Profile, Refusal, Region,
    Reported, ShapeSummary, SketchPlane, Topology, mesh_floor,
};
use silico_protocol::{
    BrepDefect, FailureKind, HistoryDefect, MAX_EXPORT_TRIANGLES, Operation, Returned, Role,
    check_export_tolerance, check_export_triangles, export_angle, export_deflection,
};

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
    /// OCCT ran out of memory: a shortage, not the values' fault.
    ResourceExhausted,
}

impl KernelError {
    fn new(refusal: Refusal) -> Self {
        Self::caused(refusal, None)
    }

    fn caused(refusal: Refusal, cause: Option<String>) -> Self {
        let described = refusal.describe(cause.as_deref(), &|e| e.to_string());
        // OCCT's own words, where it threw; otherwise the refusal's.
        let message = match (&refusal, &cause) {
            (Refusal::Occt { .. } | Refusal::EdgesFailed { .. }, Some(cause)) => cause.clone(),
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
            FailureKind::UnknownOp | FailureKind::Internal | FailureKind::Unavailable => {
                KernelErrorKind::Internal
            }
            FailureKind::ResourceExhausted => KernelErrorKind::ResourceExhausted,
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
        Self::caused(thrown(e.what()), Some(e.what().to_owned()))
    }
}

/// What OCCT's exception, in the bridge's words (its type first, as `bridge.h` writes it),
/// says went wrong: running out of memory is a shortage; anything else, OCCT's failure.
fn thrown(what: &str) -> Refusal {
    let out_of_memory = what == "std::bad_alloc"
        || what == "Standard_OutOfMemory"
        || what.starts_with("Standard_OutOfMemory:");
    if out_of_memory {
        Refusal::OutOfMemory
    } else {
        Refusal::Occt {
            operation: Operation::Other,
            edges: Vec::new(),
        }
    }
}

impl From<silico_protocol::InvalidParameter> for KernelError {
    fn from(invalid: silico_protocol::InvalidParameter) -> Self {
        refused(None, invalid)
    }
}

/// The OCCT build this crate links: its build cache entry, `<cache>/<hash of the recipe's
/// inputs, patches included>/install`. Executables load OCCT from its `lib` (their run
/// path), so the path is in a worker's bytes, and a worker built against another OCCT has
/// another identity to the core (`Kernel::identity`).
pub const OCCT_BUILD: &str = env!("SILICO_OCCT_BUILD");

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
    let m = ffi::measure(shape.get()?, silico_protocol::MEASURE_PRECISION)?;
    let point = |p: ffi::Point3| [p.x, p.y, p.z];
    Ok(ShapeSummary {
        volume: m.volume,
        area: m.area,
        center: point(m.center),
        faces: m.faces,
        edges: m.edges,
        vertices: m.vertices,
        bounds: m.bounded.then(|| [point(m.low), point(m.high)]),
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

/// The geometry and measures of `shape`'s elements of `kind` numbered from `first` (from 1,
/// in `TopExp::MapShapes` order), `count` of them or as many as there are, its areas and
/// lengths to the relative [`MEASURE_PRECISION`](silico_protocol::MEASURE_PRECISION), and
/// how many of `kind` it has (docs/design/m4.4-describe.md): one batch of
/// [`Op::Describe`](silico_protocol::Op::Describe).
pub fn describe(
    shape: &Shape,
    kind: ElementKind,
    first: u32,
    count: u32,
) -> Result<(u32, silico_protocol::Description), KernelError> {
    use silico_protocol::{Axis, EdgeCurve, EdgeDescription, FaceDescription, Surface};
    let kind = match kind {
        ElementKind::Face => ffi::Kind::Face,
        ElementKind::Edge => ffi::Kind::Edge,
        ElementKind::Vertex => ffi::Kind::Vertex,
        ElementKind::Solid => ffi::Kind::Solid,
    };
    let d = ffi::describe(
        shape.get()?,
        kind,
        first,
        count,
        silico_protocol::MEASURE_PRECISION,
    )?;
    let point = |p: &ffi::Point3| [p.x, p.y, p.z];
    let axis = |origin: &ffi::Point3, direction: &ffi::Point3| Axis {
        origin: point(origin),
        direction: point(direction),
    };
    let faces = d
        .faces
        .iter()
        .map(|f| {
            let axis = axis(&f.origin, &f.direction);
            let surface = match f.surface {
                ffi::SurfaceType::Plane => Surface::Plane {
                    normal: point(&f.normal),
                },
                ffi::SurfaceType::Cylinder => Surface::Cylinder {
                    axis,
                    radius: f.radius,
                },
                ffi::SurfaceType::Cone => Surface::Cone {
                    axis,
                    radius: f.radius,
                    half_angle: f.angle,
                },
                ffi::SurfaceType::Sphere => Surface::Sphere {
                    axis,
                    radius: f.radius,
                },
                ffi::SurfaceType::Torus => Surface::Torus {
                    axis,
                    major_radius: f.radius,
                    minor_radius: f.radius2,
                },
                ffi::SurfaceType::Bezier => Surface::Bezier,
                ffi::SurfaceType::Bspline => Surface::Bspline,
                ffi::SurfaceType::Revolution => Surface::Revolution,
                ffi::SurfaceType::Extrusion => Surface::Extrusion,
                ffi::SurfaceType::Offset => Surface::Offset,
                _ => Surface::Other,
            };
            FaceDescription {
                surface,
                area: f.area,
                centroid: point(&f.centroid),
            }
        })
        .collect();
    let edges = d
        .edges
        .iter()
        .map(|e| {
            let axis = axis(&e.origin, &e.direction);
            let curve = if e.degenerated {
                EdgeCurve::Degenerated
            } else {
                match e.curve {
                    ffi::CurveType::Line => EdgeCurve::Line,
                    ffi::CurveType::Circle => EdgeCurve::Circle {
                        axis,
                        radius: e.radius,
                    },
                    ffi::CurveType::Ellipse => EdgeCurve::Ellipse {
                        axis,
                        major_radius: e.radius,
                        minor_radius: e.radius2,
                    },
                    ffi::CurveType::Hyperbola => EdgeCurve::Hyperbola,
                    ffi::CurveType::Parabola => EdgeCurve::Parabola,
                    ffi::CurveType::Bezier => EdgeCurve::Bezier {
                        degree: e.degree,
                        poles: e.poles,
                    },
                    ffi::CurveType::Bspline => EdgeCurve::Bspline {
                        degree: e.degree,
                        poles: e.poles,
                    },
                    ffi::CurveType::Offset => EdgeCurve::Offset,
                    _ => EdgeCurve::Other,
                }
            };
            EdgeDescription {
                curve,
                length: e.length,
                start: point(&e.start),
                end: point(&e.end),
                middle: point(&e.middle),
            }
        })
        .collect();
    Ok((
        d.total,
        silico_protocol::Description {
            faces,
            edges,
            vertices: d.vertices.iter().map(point).collect(),
        },
    ))
}

/// Where `point` is against `shape`, by OCCT's solid classifier at the shape's largest
/// tolerance, as FreeCAD's `Shape.isInside(point, tolerance, True)` asks (M4.6: the core asks
/// it of each body whose tight box holds a camera's eye, and counts on the boundary as
/// inside). "On" is to about the tolerance, the classifier's band varying by face (on a box
/// with tolerance 1e-7, from 5e-8 to 9e-8). A point is checked by
/// [`check_classify_point`](silico_protocol::check_classify_point), as the core checks it; a
/// classifier that can't tell is OCCT's failure. A shape with no solid has no inside to
/// tell, and OCCT says every point is inside an empty compound, as FreeCAD's `isInside`
/// does: the core never asks of one, as it has no box to hold the eye.
pub fn classify(shape: &Shape, point: [f64; 3]) -> Result<PointPlace, KernelError> {
    silico_protocol::check_classify_point(point).map_err(|e| refused(None, e))?;
    match ffi::classify_point(shape.get()?, &point3(point))? {
        ffi::PointState::Inside => Ok(PointPlace::Inside),
        ffi::PointState::On => Ok(PointPlace::OnBoundary),
        ffi::PointState::Outside => Ok(PointPlace::Outside),
        // The bridge returns only the three; any other value is the bridge's bug.
        other => Err(KernelError::new(Refusal::InvalidParameter {
            parameter: "point".to_owned(),
            message: format!(
                "OCCT's classifier gave no state Silico knows ({})",
                other.repr
            ),
        })),
    }
}

/// The least distance between `first`, an element of `shape`, and `second`, one of
/// `other` (or of `shape` too, if there's no other), as bounded (docs/ports/measure.md):
/// OCCT's `BRepExtrema_DistShapeShape` on one thread, its distance, one pair of points where
/// it's met, chosen by their coordinates among those within OCCT's precision of the
/// distance ([`silico_protocol::witness_within`]), and how many pairs OCCT found. Refused,
/// naming `first` or `second`, for a solid, an element numbered 0, or one its shape hasn't;
/// and if OCCT finds no distance.
///
/// OCCT's search isn't symmetric: given the two elements the other way round, it can find
/// other pairs, and so another witness, and a distance an ulp off. So the bridge gives them
/// to it in one order, by the elements' own B-reps, and puts its pairs back in the
/// request's: swapping the elements swaps the witness's points and nothing else, and
/// nothing but the two elements (not the rest of their shapes, nor their numbers) decides
/// the answer.
pub fn distance(
    shape: &Shape,
    first: silico_protocol::ElementRef,
    other: Option<&Shape>,
    second: silico_protocol::ElementRef,
) -> Result<silico_protocol::Distance, KernelError> {
    silico_protocol::check_distance(first, second).map_err(KernelError::new)?;
    let out = ffi::distance(
        shape.get()?,
        bridge_kind(first),
        first.index,
        other.unwrap_or(shape).get()?,
        bridge_kind(second),
        second.index,
    )?;
    if !out.found {
        let missing = |element: silico_protocol::ElementRef, count: u32, parameter: &str| {
            (element.index > count).then(|| {
                KernelError::new(Refusal::ElementMissing {
                    parameter: parameter.to_owned(),
                    element,
                    count,
                })
            })
        };
        return Err(missing(first, out.first_total, "first")
            .or_else(|| missing(second, out.second_total, "second"))
            .unwrap_or_else(|| {
                KernelError::new(Refusal::Internal {
                    what: "the bridge found no element, though both are in range".to_owned(),
                })
            }));
    }
    measured(out)
}

/// The least distance from `point` to `element`, of `shape`, as bounded, as FreeCAD's
/// QuickMeasure measures a circle's centre against a face or an edge
/// (`Measurement::circleCenterDistance`, 0c4e125e44): a vertex made at the point, and OCCT's
/// `BRepExtrema_DistShapeShape` from it to the element on one thread; its distance, one pair
/// of points where it's met, `from` the point, chosen as [`distance`] chooses it, and how
/// many pairs OCCT found. Refused, naming `element` or `point`, by
/// [`check_point_distance`](silico_protocol::check_point_distance); naming `element`, for
/// one its shape hasn't; and if OCCT finds no distance.
pub fn point_distance(
    shape: &Shape,
    element: silico_protocol::ElementRef,
    point: [f64; 3],
) -> Result<silico_protocol::Distance, KernelError> {
    silico_protocol::check_point_distance(element, point).map_err(KernelError::new)?;
    let out = ffi::point_distance(
        shape.get()?,
        bridge_kind(element),
        element.index,
        &point3(point),
    )?;
    if !out.found {
        return Err(KernelError::new(Refusal::ElementMissing {
            parameter: "element".to_owned(),
            element,
            count: out.second_total,
        }));
    }
    measured(out)
}

/// The bridge's kind for an element: a solid is refused before the bridge is asked, and the
/// bridge refuses one too.
fn bridge_kind(element: silico_protocol::ElementRef) -> ffi::Kind {
    match element.kind {
        ElementKind::Face => ffi::Kind::Face,
        ElementKind::Edge => ffi::Kind::Edge,
        ElementKind::Vertex => ffi::Kind::Vertex,
        ElementKind::Solid => ffi::Kind::Solid,
    }
}

/// A distance the bridge found, both elements found: refused if OCCT isn't done or found no
/// pair; otherwise its value, its witness among the pairs within OCCT's precision of it
/// ([`silico_protocol::witness_within`]), and how many pairs there were.
fn measured(out: ffi::DistanceOut) -> Result<silico_protocol::Distance, KernelError> {
    if !out.done {
        return Err(KernelError::caused(
            Refusal::DistanceNotDone,
            Some("BRepExtrema_DistShapeShape: not done".to_owned()),
        ));
    }
    let point = |p: &ffi::Point3| [p.x, p.y, p.z];
    let pairs: Vec<[[f64; 3]; 2]> = out
        .pairs
        .chunks_exact(2)
        .map(|pair| [point(&pair[0]), point(&pair[1])])
        .collect();
    let Some([from, to]) =
        silico_protocol::witness_within(&pairs, out.value, silico_protocol::LENGTH_TOLERANCE)
    else {
        return Err(KernelError::caused(
            Refusal::DistanceNotDone,
            Some("BRepExtrema_DistShapeShape: done, with no solution".to_owned()),
        ));
    };
    Ok(silico_protocol::Distance {
        distance: out.value,
        from,
        to,
        solutions: u32::try_from(pairs.len()).unwrap_or(u32::MAX),
    })
}

/// Every one of `shape`'s faces, edges, and vertices described, a batch of at most
/// [`DESCRIBE_BATCH`](silico_protocol::DESCRIBE_BATCH) after another, joined
/// ([`silico_protocol::describe_in_batches`]: in the kernel's own process, with no summary to
/// plan from, where the core plans its batches from one, `silico_protocol::batches`).
pub fn describe_whole(shape: &Shape) -> Result<silico_protocol::Description, KernelError> {
    silico_protocol::describe_in_batches(
        |kind, first, count| describe(shape, kind, first, count),
        |kind, was, now| {
            KernelError::new(silico_protocol::Refusal::Internal {
                what: format!(
                    "the shape's count of {} changed between batches, {was} then {now}",
                    kind.plural()
                ),
            })
        },
    )
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

/// The unit normal of the plane `faces` lie on, if they can be swept: faces only (a
/// profile's, as [`make_faces`] makes them), at least one, no solid, all on parallel planes.
fn sweepable(faces: &Shape) -> Result<[f64; 3], KernelError> {
    let inner = faces.get()?;
    let c = ffi::count_elements(inner)?;
    let flat = ffi::flat_normal(inner)?;
    if c.faces == 0 || c.solids != 0 || !flat.flat {
        return Err(KernelError::new(Refusal::InvalidParameter {
            parameter: "faces".to_owned(),
            message: format!(
                "a sweep takes a profile's faces, on one plane, got a shape of {} faces and {} solids{}",
                c.faces,
                c.solids,
                if flat.flat { "" } else { ", not on one plane" }
            ),
        }));
    }
    Ok([flat.normal.x, flat.normal.y, flat.normal.z])
}

/// A sweep the plane checks refuse, as the refusal that says what to do instead.
fn flat(invalid: silico_protocol::InvalidParameter) -> KernelError {
    KernelError::new(Refusal::SweepFlat {
        parameter: invalid.parameter.to_owned(),
        message: invalid.message,
    })
}

fn point3([x, y, z]: [f64; 3]) -> ffi::Point3 {
    ffi::Point3 { x, y, z }
}

/// A sweep's result, checked as any operation's ([`checked_result`]), and refused as an
/// invalid result if it has no volume to speak of or is turned inside out ([`amount_of`], as a
/// boolean's is): a flat "solid" OCCT calls valid, which the plane checks keep out, should
/// one get past.
fn swept(
    faces: &Shape,
    solid: cxx::UniquePtr<ffi::Shape>,
    operation: Operation,
    entries: &[ffi::HistoryEntry],
) -> Result<Built, KernelError> {
    let result = checked_result(&[faces], solid, operation.clone())?;
    if !matches!(amount_of(result.get()?, &[])?, Amount::Some) {
        return Err(KernelError::new(Refusal::ResultInvalid {
            operation,
            edges: Vec::new(),
        }));
    }
    let history = history_of(&[faces.get()?], result.get()?, entries)?;
    Ok(Built {
        shape: result,
        history,
    })
}

/// `faces` (a profile's, [`make_faces`]) moved by `start`, then swept along `vector` into a
/// solid for each face: one solid, or a compound of them (docs/design/m3.6-sketch-features.md,
/// S3). The core works out the start and the vector; this builds exactly them, out of the
/// faces' plane ([`silico_protocol::check_leaves_plane`]). The faces are left as they were.
/// The history has the faces as its one input: each face's, edge's, and vertex's start
/// counterpart (the element itself, or its moved copy) its one `modified` element, never
/// kept; its end counterpart `generated`, of its own kind; and its sides `generated` (a face
/// its solid, an edge its side face, a vertex its side edge). `History::ends` is left empty.
pub fn prism(faces: &Shape, start: [f64; 3], vector: [f64; 3]) -> Result<Built, KernelError> {
    silico_protocol::check_prism(start, vector).map_err(|invalid| refused(None, invalid))?;
    let normal = sweepable(faces)?;
    silico_protocol::check_leaves_plane("vector", normal, vector).map_err(flat)?;
    build_prism(faces, start, vector)
}

/// The prism of [`prism`], its values checked.
fn build_prism(faces: &Shape, start: [f64; 3], vector: [f64; 3]) -> Result<Built, KernelError> {
    let mut entries = Vec::new();
    // OCCT has built every prism of valid faces tried; should one fail, the refusal says
    // what to change.
    let solid = ffi::sweep_prism(faces.get()?, point3(start), point3(vector), &mut entries)
        .map_err(|e| KernelError::from(e).during(Operation::Extrusion))?;
    swept(faces, solid, Operation::Extrusion, &entries)
}

/// `faces` swept from where they are along `direction` (a unit vector, out of their plane)
/// through all of `base`: [`silico_protocol::THROUGH_ALL_FACTOR`] times the diagonal of the
/// tight box around both ([`silico_protocol::through_all_length`], FreeCAD's rule, design
/// S3), refused naming `base` past a kilometre. `base` only sizes the prism; the history is
/// the faces' alone, as [`prism`]'s.
///
/// Sized here, in the worker, not by the core: the core holds `base` only as its B-rep's
/// bytes, so measuring its box there would be another request and the shape sent twice,
/// where this is one request, whose step's key already covers both shapes, its box taken
/// by the OCCT that builds the prism. The rule itself, the length from the box, is the
/// protocol's ([`silico_protocol::through_all_length`]), which the core's fake kernel
/// shares, so both size a prism alike.
pub fn prism_through_all(
    faces: &Shape,
    base: &Shape,
    direction: [f64; 3],
) -> Result<Built, KernelError> {
    silico_protocol::check_through_all(direction).map_err(|invalid| refused(None, invalid))?;
    let normal = sweepable(faces)?;
    silico_protocol::check_leaves_plane("direction", normal, direction).map_err(flat)?;
    let base_inner = base.get()?;
    if is_empty(base_inner)? {
        return Err(KernelError::new(Refusal::InvalidParameter {
            parameter: "base".to_owned(),
            message: "the base has nothing in it: through all goes through a solid".to_owned(),
        }));
    }
    let (a, b) = (
        ffi::tight_bounds(base_inner)?,
        ffi::tight_bounds(faces.get()?)?,
    );
    let corners = |p: &ffi::Point3| [p.x, p.y, p.z];
    let boxes: Vec<[[f64; 3]; 2]> = [a, b]
        .iter()
        .filter(|b| b.bounded)
        .map(|b| [corners(&b.low), corners(&b.high)])
        .collect();
    let mut around = boxes.first().copied().unwrap_or_default();
    for [low, high] in &boxes {
        for k in 0..3 {
            around[0][k] = around[0][k].min(low[k]);
            around[1][k] = around[1][k].max(high[k]);
        }
    }
    let length =
        silico_protocol::through_all_length(around).map_err(|invalid| refused(None, invalid))?;
    let vector = direction.map(|c| c * length);
    // The rules compose (check_prism allows a unit direction's rounding past the
    // kilometre); should they not, the length is the base's doing, so the base is named.
    silico_protocol::check_prism([0.0; 3], vector).map_err(|invalid| {
        refused(
            None,
            silico_protocol::InvalidParameter {
                parameter: "base",
                message: invalid.message,
            },
        )
    })?;
    build_prism(faces, [0.0; 3], vector)
}

/// `faces` (a profile's, [`make_faces`]) turned `start` radians about `axis` (its direction a
/// unit vector, in their plane: [`silico_protocol::check_axis_in_plane`]), then swept `angle`
/// radians about it, right-handed: a solid for each face (design S3). The faces are left as
/// they were. The history is as [`prism`]'s, but for an element on the axis, whose end is
/// its start (reported once, modified), and a whole turn, which has no ends
/// (`History::sweep_fault`).
pub fn revolution(
    faces: &Shape,
    axis: &Axis,
    start: f64,
    angle: f64,
) -> Result<Built, KernelError> {
    silico_protocol::check_revolution(axis.origin, axis.direction, start, angle)
        .map_err(|invalid| refused(None, invalid))?;
    let normal = sweepable(faces)?;
    silico_protocol::check_axis_in_plane(normal, axis.direction).map_err(flat)?;
    let mut entries = Vec::new();
    // OCCT fails on some profiles the core accepts, a side exactly 1e-7 from the axis among
    // them (FreeCAD's OCCT too): the refusal says what to change.
    let solid = ffi::sweep_revolution(
        faces.get()?,
        point3(axis.origin),
        point3(axis.direction),
        start,
        angle,
        &mut entries,
    )
    .map_err(|e| KernelError::from(e).during(Operation::Revolve))?;
    swept(faces, solid, Operation::Revolve, &entries)
}

/// A face for each of `regions`' regions, on its sketch's plane, as the core sends them
/// (docs/design/m3.5-profile.md): each curve one edge, each point one vertex the curves that
/// meet there share, each loop a wire of its curves in the order and direction given, and
/// each region a face of its outer loop and its holes, as FreeCAD's FaceMakerBullseye builds
/// one (its plane, its outer wire, its holes added, no fix-up). One region gives its face;
/// several, a compound of their faces in order. Nothing is classified or reoriented: the
/// regions are checked first ([`FaceRegions::check`]), and the result must be valid, or
/// it's refused (an invalid result).
///
/// The history has the regions as its one input, with no faces: its edges are the curves
/// and its vertices the points, in the order sent ([`FaceRegions`]), each kept as one of the
/// result's.
pub fn make_faces(regions: &FaceRegions) -> Result<Built, KernelError> {
    regions.check().map_err(|invalid| refused(None, invalid))?;
    let plane = regions.plane;
    let point3 = |[x, y, z]: [f64; 3]| ffi::Point3 { x, y, z };
    let normal = plane.normal();
    let reversed = normal.map(|c| -c);
    let mut points = Vec::new();
    let mut curves = Vec::new();
    let mut loop_curves = Vec::new();
    let mut region_loops = Vec::new();
    let number = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    for region in &regions.regions {
        region_loops.push(number(1 + region.holes.len()));
        for (contour, loop_) in region.loops().enumerate() {
            loop_curves.push(number(loop_.len()));
            // A loop of lines and arcs has a point at each curve's start; the last curve
            // ends at the first's.
            let first = points.len();
            for (i, curve) in loop_.iter().enumerate() {
                let start = number(first + i);
                let end = number(first + (i + 1) % loop_.len());
                let made = match *curve {
                    Curve::Line { start: from, .. } => {
                        points.push(point3(plane.place(from)));
                        ffi::FaceCurve {
                            kind: ffi::CurveKind::Line,
                            start,
                            end,
                            center: point3([0.0; 3]),
                            axis: point3(normal),
                            radius: 0.0,
                            first: 0.0,
                            last: 0.0,
                        }
                    }
                    Curve::Arc {
                        center,
                        start: from,
                        radius,
                        start_angle,
                        end_angle,
                        ..
                    } => {
                        points.push(point3(plane.place(from)));
                        // Counter-clockwise about the normal, or clockwise: about the
                        // reversed normal, whose angles from the x axis run the other way.
                        let (axis, first, last) = if end_angle > start_angle {
                            (normal, start_angle, end_angle)
                        } else {
                            (reversed, -start_angle, -end_angle)
                        };
                        ffi::FaceCurve {
                            kind: ffi::CurveKind::Arc,
                            start,
                            end,
                            center: point3(plane.place(center)),
                            axis: point3(axis),
                            radius,
                            first,
                            last,
                        }
                    }
                    Curve::Circle { center, radius } => ffi::FaceCurve {
                        kind: ffi::CurveKind::Circle,
                        start: 0,
                        end: 0,
                        center: point3(plane.place(center)),
                        // Counter-clockwise as an outer loop, clockwise as a hole.
                        axis: point3(if contour == 0 { normal } else { reversed }),
                        radius,
                        first: 0.0,
                        last: 0.0,
                    },
                };
                curves.push(made);
            }
        }
    }
    let mut entries = Vec::new();
    let made = ffi::make_faces(
        point3(normal),
        point3(plane.x_axis()),
        &points,
        &curves,
        &loop_curves,
        &region_loops,
        &mut entries,
    )
    .map_err(|e| KernelError::from(e).during(Operation::MakeFaces))?;
    let result = made.as_ref().ok_or_else(|| {
        KernelError::new(Refusal::NothingReturned {
            what: Returned::Shape,
        })
    })?;
    if !ffi::is_valid(result)? {
        return Err(KernelError::new(Refusal::ResultInvalid {
            operation: Operation::MakeFaces,
            edges: Vec::new(),
        }));
    }
    let c = ffi::count_elements(result)?;
    let counts = ElementCounts {
        faces: c.faces,
        edges: c.edges,
        vertices: c.vertices,
        solids: c.solids,
    };
    let sent = ElementCounts {
        faces: 0,
        edges: number(curves.len()),
        vertices: number(points.len()),
        solids: 0,
    };
    let mut history = assemble_history(&[sent], counts, &entries)?;
    history.topology = topology(&ffi::topology(result)?, counts)?;
    Ok(Built {
        shape: Shape::new(made).with_validity(true),
        history,
    })
}

/// Fuses `tool` with `base`, cuts it from `base`, or keeps what they share
/// (docs/ports/booleans.md). The history has `base` as input 0 and `tool` as input 1. A
/// shape with nothing in it is refused, as either input (OCCT would fuse it, or cut it
/// away, as if nothing were wrong) or as the result (a common of shapes that don't
/// overlap, a cut that removes everything) or as a result of solids with no volume to
/// speak of; and a result of solids turned inside out (a volume well below none) is
/// refused as invalid, whatever OCCT's own check says of it. A valid result is otherwise
/// accepted as OCCT gives it, as FreeCAD accepts it, though OCCT gets some wrong (the port
/// note lists them).
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
    let mut skipped = Vec::new();
    let built = ffi::boolean(
        bridge_op,
        base_inner,
        tool_inner,
        &mut entries,
        &mut warnings,
        &mut skipped,
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
    // One warning says which elements: OCCT skipped intersecting a pair, where the
    // intersection threw, and took them as not meeting. That's logged by name, and not
    // refused: a rod resting on a pointed cone's tip raises it with right results, as in
    // FreeCAD, while a cut of a barrel whose turned face lay outside its own surface raised
    // it and came back whole (docs/ports/booleans.md).
    if !skipped.is_empty() {
        tracing::warn!(
            "OCCT skipped intersecting {} in the {name}, taking them as not meeting",
            skipped_words(&skipped)
        );
    }
    if let Some(result) = built.as_ref() {
        let amount = if is_empty(result)? {
            Amount::None
        } else {
            amount_of(result, &[base_inner, tool_inner])?
        };
        match amount {
            // OCCT found nothing (or a result with no volume), which it can do for shapes
            // that do overlap (where faces or edges nearly meet, or across a torus's seam):
            // the refusal's suggestion doesn't claim more than that.
            Amount::None => return Err(KernelError::new(Refusal::BooleanResultEmpty { op })),
            // A solid turned inside out, its volume well below none: not empty, a result
            // that isn't a solid, whatever OCCT's check says of it.
            Amount::InsideOut => {
                return Err(KernelError::new(Refusal::ResultInvalid {
                    operation,
                    edges: Vec::new(),
                }));
            }
            Amount::Some => {}
        }
    }
    let result = checked_result(&[base, tool], built, operation)?;
    let history = history_of(&[base_inner, tool_inner], result.get()?, &entries)?;
    Ok(Built {
        shape: result,
        history,
    })
}

/// How much a boolean's result holds.
enum Amount {
    /// A volume to speak of (or its inputs aren't solids, so it isn't judged).
    Some,
    /// No volume to speak of: empty.
    None,
    /// A volume well below none: a solid turned inside out.
    InsideOut,
}

/// How much a boolean of solids gave: no volume to speak of if it has no solid at all,
/// or a mean thickness (volume over area) within OCCT's length tolerance of none, either
/// side; or a solid turned inside out, if its volume is further below none than that. OCCT
/// returns empty-as-good results for some booleans it gets wrong (a torus cut by a copy of
/// itself moved 0.5 across its seam: two whole torus surfaces of volume -8e-11, a center
/// 2.5e11 away), and they're as empty as an empty compound; their mean thickness is under
/// 1e-11. A real result is far thicker, however little of its bounding box it fills: a
/// 1 × 2 × 3 box's common with a copy 1e-4 into it is 5e-5 thick on average, a 0.01 mm
/// wire 0.005. A solid inside out isn't empty (a cylinder fused with a box it touches
/// along a line came back one shell of volume -5968): it's invalid. Not judged when an
/// input has no solid in it.
fn amount_of(result: &ffi::Shape, inputs: &[&ffi::Shape]) -> Result<Amount, KernelError> {
    for input in inputs {
        if ffi::count_elements(input)?.solids == 0 {
            return Ok(Amount::Some);
        }
    }
    if ffi::count_elements(result)?.solids == 0 {
        return Ok(Amount::None);
    }
    // The quick measure clears most results, against their size: on these broken shapes
    // its error grows with the model (it read the cut above as 4e-6, and the cut of a torus
    // of radii 200 and 100 by a copy moved 30 as 1.9, where its volume is -1.7e-4: both
    // really negative), so it's compared with the cube of the
    // bounding diagonal, not the area; and only if it's thicker than the tolerance on
    // average too, since a part a few micrometres across clears its size however thin it is
    // (a 1e-3 square swept 1.5e-7). What it doesn't clear (results that fill little of
    // their box, too) is measured precisely, and judged by its mean thickness.
    let rough = ffi::rough_amounts(result)?;
    let diagonal = ffi::bounding_diagonal(result)?;
    if rough.volume > 1e-5 * diagonal * diagonal * diagonal
        && rough.volume > LENGTH_TOLERANCE * rough.area
    {
        return Ok(Amount::Some);
    }
    let precise = ffi::measure(result, silico_protocol::MEASURE_PRECISION)?;
    let thin = LENGTH_TOLERANCE * precise.area;
    Ok(if precise.volume.is_nan() || precise.volume.abs() <= thin {
        Amount::None
    } else if precise.volume < -thin {
        Amount::InsideOut
    } else {
        Amount::Some
    })
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
    let result_counts = counts(result)?;
    let mut history = assemble_history(&input_counts, result_counts, entries)?;
    history.topology = topology(&ffi::topology(result)?, result_counts)?;
    Ok(history)
}

/// The result's topology, from the bridge's flattened facts, checked against its counts.
fn topology(facts: &ffi::TopologyFacts, counts: ElementCounts) -> Result<Topology, KernelError> {
    let bad = || {
        KernelError::new(Refusal::MalformedHistory {
            defect: HistoryDefect::BadTopology,
        })
    };
    // Each element's list, `sizes` long, from `all`, every element used.
    let split = |all: &[u32], sizes: &[u32]| -> Result<Vec<Vec<u32>>, KernelError> {
        let mut rest = all;
        let mut lists = Vec::with_capacity(sizes.len());
        for &n in sizes {
            let (list, after) = rest.split_at_checked(n as usize).ok_or_else(bad)?;
            lists.push(list.to_vec());
            rest = after;
        }
        if rest.is_empty() {
            Ok(lists)
        } else {
            Err(bad())
        }
    };
    let mut outer_edges = split(&facts.outer_edges, &facts.outer_edge_counts)?;
    for list in &mut outer_edges {
        list.sort_unstable();
    }
    let topology = Topology {
        face_edges: split(&facts.face_edges, &facts.face_edge_counts)?,
        outer_edges,
        edge_vertices: split(&facts.edge_vertices, &facts.edge_vertex_counts)?,
    };
    topology.validate(counts).map_err(|why| {
        // The refusal says the topology is bad; the log says how, for the bug report.
        tracing::warn!(%why, "OCCT's topology doesn't fit the result's counts");
        bad()
    })?;
    Ok(topology)
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
        topology: Topology::default(),
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

/// How many skipped pairs a boolean's log names; the rest are counted.
const PAIRS_NAMED: usize = 3;

/// The pairs a boolean's OCCT skipped intersecting, from the bridge's elements two by two,
/// each once in the order OCCT first gave it (a cut of the barrel raised its pair twice), in
/// words: "the base's Face2 with the tool's Face3", by each input's own numbering, or "an
/// edge OCCT made" for an element of neither input. The first few are named, the rest
/// counted.
fn skipped_words(elements: &[ffi::SkippedElement]) -> String {
    let one = |element: &ffi::SkippedElement| {
        let kind = element_kind(element.kind);
        match (kind, element.input) {
            (Some(kind), 0 | 1) => format!(
                "the {}'s {}",
                if element.input == 0 { "base" } else { "tool" },
                ElementRef {
                    kind,
                    index: element.index
                }
            ),
            (Some(ElementKind::Edge), _) => "an edge OCCT made".to_owned(),
            (Some(ElementKind::Vertex), _) => "a vertex OCCT made".to_owned(),
            (Some(ElementKind::Face), _) => "a face OCCT made".to_owned(),
            _ => "an element OCCT made".to_owned(),
        }
    };
    // The bridge gives them two by two.
    let key = |e: &ffi::SkippedElement| (e.kind.repr, e.input, e.index);
    let mut seen = std::collections::BTreeSet::new();
    let pairs: Vec<&[ffi::SkippedElement]> = elements
        .chunks_exact(2)
        .filter(|pair| seen.insert(pair.iter().map(key).collect::<Vec<_>>()))
        .collect();
    let mut named: Vec<String> = pairs
        .iter()
        .take(PAIRS_NAMED)
        .filter_map(|pair| match pair {
            [first, second] => Some(format!("{} with {}", one(first), one(second))),
            _ => None,
        })
        .collect();
    match pairs.len().saturating_sub(PAIRS_NAMED) {
        0 => {}
        1 => named.push("1 more pair".to_owned()),
        more => named.push(format!("{more} more pairs")),
    }
    match &named[..] {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
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

/// Meshes `shape` for display, with no point of the mesh further than `deflection` from
/// the surface. Single-threaded, so the same shape always gives the same mesh, and it
/// leaves the shape as it was (OCCT would otherwise keep the mesh on it). Faces and edges
/// are in OCCT's order; the first edge is Edge1, as [`fillet`] numbers them. Each vertex
/// carries its face's surface normal there, pointing out of the solid
/// (docs/ports/tessellation.md).
pub fn tessellate(shape: &Shape, deflection: f64) -> Result<Mesh, KernelError> {
    Ok(tessellated(shape, deflection, false)?.0)
}

/// [`tessellate`], at the larger of `deflection` and the shape's floor (the finest it
/// meshes: [`mesh_floor`] of its bounding box's diagonal), never refused for being too fine;
/// and the deflection it used. M4.6's renders ask for half a pixel, which a close view takes
/// under the floor.
pub fn tessellate_at_least_the_floor(
    shape: &Shape,
    deflection: f64,
) -> Result<MeshAt, KernelError> {
    let (mesh, deflection) = tessellated(shape, deflection, true)?;
    Ok(MeshAt { mesh, deflection })
}

/// The mesh, and the deflection it was meshed at: `deflection`, or the floor if it's finer
/// and `at_least_the_floor`; refused if it's finer and not.
fn tessellated(
    shape: &Shape,
    deflection: f64,
    at_least_the_floor: bool,
) -> Result<(Mesh, f64), KernelError> {
    if !(deflection.is_finite() && deflection > 0.0) {
        return Err(KernelError::new(Refusal::DeflectionInvalid { deflection }));
    }
    let inner = shape.get()?;
    let diagonal = ffi::bounding_diagonal(inner)?;
    if !diagonal.is_finite() {
        return Err(KernelError::new(Refusal::ExtentNotFinite));
    }
    let floor = mesh_floor(diagonal);
    let deflection = match deflection < floor {
        false => deflection,
        true if at_least_the_floor => floor,
        true => {
            return Err(KernelError::new(Refusal::DeflectionTooFine {
                deflection,
                floor,
            }));
        }
    };
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
    // nearly through has, at any deflection) has no triangles, as FreeCAD gives such a face
    // an empty domain (`TopoShape::getDomains`). Whether the shape is valid goes with the
    // mesh, checked only then, and the core decides: on a valid shape such faces are left
    // out; on one that isn't, the mesh is refused, naming the first hundred and counting them
    // all (there a face that can't be meshed says something is broken: one whose edges
    // cross, its area, its halves cancelling, saying nothing).
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
        valid: None,
    };
    // From the mesh's own counts, so what the core checks and what the worker sends agree.
    let mesh = match mesh.face_triangles.contains(&0) {
        false => mesh,
        true => {
            let faces = mesh.unmeshed().count();
            tracing::debug!(faces, "faces OCCT couldn't mesh, left without triangles");
            Mesh {
                valid: Some(shape.is_valid()?),
                ..mesh
            }
        }
    };
    // Geometry that isn't finite is refused when it's read, and a shape far enough out to
    // overflow its coordinates overflows its extent first: a mesh's coordinates are finite.
    // Anything else wrong with the mesh would be the bridge's bug, which the core's own
    // check catches (and stops the worker for).
    Ok((mesh, deflection))
}

/// Meshes `shape`'s solids for export within `tolerance` mm, as FreeCAD meshes for export
/// (`TopoShape::getFaces`): absolute, with angular deflection [`export_angle`], but
/// single-threaded, so the same shape always gives the same mesh, and with the deflection
/// the tolerance less what rounding to 32 bits can move the shape's vertices, as far out as
/// its bounding box reaches ([`export_deflection`]): a tolerance whose rounding there would
/// take more than half of it is refused ([`Refusal::RoundingPastTolerance`]), as is a mesh
/// of more than [`MAX_EXPORT_TRIANGLES`] triangles ([`Refusal::TooManyTriangles`], said as
/// the unraised mesh's with `force_face_deflection`, which isn't the export's). It leaves
/// the shape as it was. Reports each face's triangles and where its nodes lie on the shape's
/// edges and vertices, for the core to join and check (docs/ports/mesh-export.md).
pub fn export_mesh(
    shape: &Shape,
    tolerance: f64,
    force_face_deflection: bool,
) -> Result<ExportMesh, KernelError> {
    export_mesh_within(
        shape,
        tolerance,
        force_face_deflection,
        MAX_EXPORT_TRIANGLES,
    )
}

/// [`export_mesh`], refused past `limit` triangles.
fn export_mesh_within(
    shape: &Shape,
    tolerance: f64,
    force_face_deflection: bool,
    limit: u64,
) -> Result<ExportMesh, KernelError> {
    check_export_tolerance(tolerance)?;
    let inner = shape.get()?;
    let reach = ffi::bounding_reach(inner)?;
    if !reach.is_finite() {
        return Err(KernelError::new(Refusal::ExtentNotFinite));
    }
    let deflection = export_deflection(tolerance, reach).map_err(KernelError::new)?;
    let raw = ffi::export_mesh(
        inner,
        deflection,
        export_angle(tolerance),
        force_face_deflection,
    )
    .map_err(|e| KernelError::from(e).during(Operation::Tessellate))?;
    let raw = raw.as_ref().ok_or_else(|| {
        KernelError::new(Refusal::NothingReturned {
            what: Returned::Mesh,
        })
    })?;
    check_export_triangles(
        raw.face_triangles().iter().map(|&n| u64::from(n)).sum(),
        limit,
        force_face_deflection,
    )
    .map_err(KernelError::new)?;
    let triples = |values: &[f64]| -> Vec<[f64; 3]> {
        values.chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect()
    };
    Ok(ExportMesh {
        tolerance,
        deflection,
        force_face_deflection,
        nodes: triples(raw.nodes()),
        face_nodes: raw.face_nodes().to_vec(),
        triangles: raw
            .triangles()
            .chunks_exact(3)
            .map(|t| [t[0], t[1], t[2]])
            .collect(),
        face_triangles: raw.face_triangles().to_vec(),
        face_numbers: raw.face_numbers().to_vec(),
        face_deflections: raw.face_deflections().to_vec(),
        face_shells: raw.face_shells().to_vec(),
        face_tolerances: raw.face_tolerances().to_vec(),
        unoriented_faces: raw.unoriented_faces().to_vec(),
        unoriented_shells: raw.unoriented_shells().to_vec(),
        shell_solids: raw.shell_solids().to_vec(),
        shell_outer: raw.shell_outer().iter().map(|&b| b != 0).collect(),
        solids: raw.solids(),
        loose_faces: raw.loose_faces().to_vec(),
        edge_faces: raw.edge_faces().to_vec(),
        edge_numbers: raw.edge_numbers().to_vec(),
        edge_degenerate: raw.edge_degenerate().iter().map(|&b| b != 0).collect(),
        edge_vertices: raw
            .edge_vertices()
            .chunks_exact(2)
            .map(|v| [v[0], v[1]])
            .collect(),
        edge_deflections: raw.edge_deflections().to_vec(),
        edge_node_counts: raw.edge_node_counts().to_vec(),
        edge_nodes: raw.edge_nodes().to_vec(),
    })
}

/// Operations that exist only to test the bridge, built with the `test_ops` feature
/// (which this crate's own tests turn on, and nothing else should).
#[cfg(feature = "test_ops")]
pub mod test_ops {
    use super::{ExportMesh, KernelError, Shape, ffi};

    /// [`super::export_mesh`] refused past `limit` triangles, not the export's ten million,
    /// so the worker's limit is tested on a mesh small enough to make.
    pub fn export_mesh_within(
        shape: &Shape,
        tolerance: f64,
        force_face_deflection: bool,
        limit: u64,
    ) -> Result<ExportMesh, KernelError> {
        super::export_mesh_within(shape, tolerance, force_face_deflection, limit)
    }

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

    /// A compound of a 10 × 20 × 30 box and a face with a surface and no wires (Face7),
    /// which OCCT's STEP writer drops with only a warning.
    pub fn box_with_wireless_face() -> Result<super::Shape, KernelError> {
        Ok(super::Shape::new(ffi::debug_box_with_wireless_face()?))
    }

    /// A compound of a 10 × 20 × 30 box and a square face with a second, empty wire, which
    /// OCCT's STEP writer drops with only a warning.
    pub fn box_with_empty_wire_face() -> Result<super::Shape, KernelError> {
        Ok(super::Shape::new(ffi::debug_box_with_empty_wire_face()?))
    }

    /// A compound of a 10 × 20 × 30 box and a square face with an INTERNAL wire, which OCCT's
    /// STEP writer skips on purpose.
    pub fn box_with_internal_wire_face() -> Result<super::Shape, KernelError> {
        Ok(super::Shape::new(ffi::debug_box_with_internal_wire_face()?))
    }
}

#[cfg(test)]
#[path = "../tests/common/imports.rs"]
mod imports;

#[cfg(test)]
mod tests {
    use super::{ffi, skipped_words};

    /// An element of the bridge's: its kind, its input (0 the base, 1 the tool, 2 neither),
    /// its number there.
    fn element(kind: ffi::Kind, input: u8, index: u32) -> ffi::SkippedElement {
        ffi::SkippedElement { kind, input, index }
    }

    fn face(input: u8, index: u32) -> ffi::SkippedElement {
        element(ffi::Kind::Face, input, index)
    }

    #[test]
    fn skipped_pairs_are_named_by_kind_and_input_or_as_occts_own() {
        // Each kind by its input's numbering, and an element of neither input by its kind.
        let words = skipped_words(&[
            element(ffi::Kind::Vertex, 0, 4),
            element(ffi::Kind::Edge, 1, 7),
        ]);
        assert_eq!(words, "the base's Vertex4 with the tool's Edge7");
        let words = skipped_words(&[
            element(ffi::Kind::Edge, 2, 0),
            element(ffi::Kind::Vertex, 2, 0),
        ]);
        assert_eq!(words, "an edge OCCT made with a vertex OCCT made");
        let words = skipped_words(&[face(2, 0), element(ffi::Kind::Other, 2, 0)]);
        assert_eq!(words, "a face OCCT made with an element OCCT made");
    }

    #[test]
    fn skipped_pairs_are_listed_once_each_the_first_three_named() {
        let pair = |n| [face(0, 2), face(1, n)];
        let named = |n: u32| format!("the base's Face2 with the tool's Face{n}");
        // One, its repeats counted once.
        let words = skipped_words(&[pair(3), pair(3), pair(3)].concat());
        assert_eq!(words, named(3));
        // Two, and three.
        assert_eq!(
            skipped_words(&[pair(1), pair(3)].concat()),
            format!("{} and {}", named(1), named(3))
        );
        assert_eq!(
            skipped_words(&[pair(1), pair(3), pair(4)].concat()),
            format!("{}, {}, and {}", named(1), named(3), named(4))
        );
        // Four, a repeat among them: three named, one more pair.
        assert_eq!(
            skipped_words(&[pair(1), pair(3), pair(1), pair(4), pair(5)].concat()),
            format!("{}, {}, {}, and 1 more pair", named(1), named(3), named(4))
        );
        // Five: two more pairs.
        assert_eq!(
            skipped_words(&[pair(1), pair(3), pair(4), pair(5), pair(6)].concat()),
            format!("{}, {}, {}, and 2 more pairs", named(1), named(3), named(4))
        );
        // Pairs that differ only in an element's input, or only in its kind, are two.
        assert_eq!(
            skipped_words(&[[face(0, 2), face(1, 3)], [face(0, 2), face(0, 3)]].concat()),
            format!("{} and the base's Face2 with the base's Face3", named(3))
        );
        assert_eq!(
            skipped_words(
                &[
                    [face(0, 2), face(1, 3)],
                    [face(0, 2), element(ffi::Kind::Edge, 1, 3)]
                ]
                .concat()
            ),
            format!("{} and the base's Face2 with the tool's Edge3", named(3))
        );
        // A pair isn't its own reverse: the same two faces, the other way round, is another.
        assert_eq!(
            skipped_words(&[[face(0, 2), face(1, 3)], [face(1, 3), face(0, 2)]].concat()),
            format!("{} and the tool's Face3 with the base's Face2", named(3))
        );
    }

    /// This crate's unit-test binary links OCCT and the bridge, so it must import no
    /// inexact platform math either (ARCHITECTURE.md, Deterministic kernel).
    #[test]
    fn unit_test_binary_imports_no_platform_math() {
        super::imports::assert_this_binary_imports_no_platform_math();
    }

    #[test]
    fn occt_running_out_of_memory_is_a_shortage_and_anything_else_its_failure() {
        use silico_protocol::Refusal;
        for what in [
            "std::bad_alloc",
            "Standard_OutOfMemory",
            "Standard_OutOfMemory: Standard_MMgrRaw::Allocate(): malloc failed",
        ] {
            assert_eq!(super::thrown(what), Refusal::OutOfMemory, "{what}");
        }
        for what in [
            "StdFail_NotDone",
            "Standard_DomainError: gp_Dir() - input vector has zero norm",
            "Standard_OutOfMemoryish",
            "unknown C++ exception",
        ] {
            assert!(
                matches!(super::thrown(what), Refusal::Occt { .. }),
                "{what}"
            );
        }
    }

    #[test]
    fn a_prism_too_thin_for_a_volume_is_valid_to_occt_and_refused_as_empty() {
        // The M3 review's low (M3.6's fallout): the thin-sweep refusals were tested, but not
        // their premise, that OCCT builds such a shape and calls it valid, so it's
        // `amount_of`, not the validity check, that refuses it. A 10 × 10 square swept
        // 1.5e-7 (above OCCT's confusion, 1e-7, so the prism isn't degenerate) has volume
        // 1.5e-5 and area about 200: a mean thickness of 7.5e-8, within LENGTH_TOLERANCE
        // (1e-7) of none. Its rough volume is far below 1e-5 × the cube of its diagonal
        // (14.14³ × 1e-5 = 0.028), so it's measured precisely.
        let (faces, result, thin) = thin_prism(10.0);
        assert!(result.is_valid().unwrap());
        // But no volume to speak of.
        assert!(matches!(
            super::amount_of(result.get().unwrap(), &[]).unwrap(),
            super::Amount::None
        ));
        // So the sweep is refused as an invalid result, for that, not built.
        let refused = super::build_prism(&faces, [0.0; 3], thin).err().unwrap();
        assert!(
            matches!(
                refused.refusal(),
                silico_protocol::Refusal::ResultInvalid { .. }
            ),
            "{refused}"
        );
    }

    #[test]
    fn a_prism_too_thin_for_a_volume_is_refused_however_small_it_is() {
        // The prism above at 1e-3 × 1e-3, swept the same 1.5e-7: volume 1.5e-13, area
        // 2.0006e-6 (FreeCAD 1.1.3, which calls it valid), a mean thickness of 7.5e-8, as
        // thin as the 10 × 10 one. Its bounding diagonal is about 1.4145e-3 (1.41421e-3 and
        // OCCT's 1e-7 gap each side), so 1e-5 × its cube is 2.83e-14, which its rough volume
        // exceeds 5.3 times: its size alone would clear it. Its mean thickness doesn't, so
        // it's measured precisely, and refused (1.5e-13 against 1e-7 × 2.0006e-6 = 2.0e-13).
        let (faces, result, thin) = thin_prism(1e-3);
        assert!(result.is_valid().unwrap());
        assert!(matches!(
            super::amount_of(result.get().unwrap(), &[]).unwrap(),
            super::Amount::None
        ));
        let refused = super::build_prism(&faces, [0.0; 3], thin).err().unwrap();
        assert!(
            matches!(
                refused.refusal(),
                silico_protocol::Refusal::ResultInvalid { .. }
            ),
            "{refused}"
        );
    }

    /// A `side` × `side` square on the xy plane, its faces, swept 1.5e-7 up z, checked as
    /// every operation's result is, and that sweep.
    fn thin_prism(side: f64) -> (super::Shape, super::Shape, [f64; 3]) {
        use silico_protocol::{Curve, FaceRegions, Region, SketchPlane};
        let square = [[0.0, 0.0], [side, 0.0], [side, side], [0.0, side]];
        let outer = (0..4)
            .map(|i| Curve::Line {
                start: square[i],
                end: square[(i + 1) % 4],
            })
            .collect();
        let faces = super::make_faces(&FaceRegions {
            plane: SketchPlane::Xy,
            regions: vec![Region {
                outer,
                holes: Vec::new(),
            }],
        })
        .unwrap()
        .shape;
        let thin = [0.0, 0.0, 1.5e-7];
        let mut entries = Vec::new();
        let solid = super::ffi::sweep_prism(
            faces.get().unwrap(),
            super::point3([0.0; 3]),
            super::point3(thin),
            &mut entries,
        )
        .unwrap();
        // Valid to OCCT: the check every operation's result passes lets it through.
        let result = super::checked_result(&[&faces], solid, super::Operation::Extrusion)
            .unwrap_or_else(|e| panic!("OCCT called the thin prism invalid: {e}"));
        (faces, result, thin)
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

    #[test]
    fn a_sweep_turned_inside_out_is_refused_as_an_invalid_result() {
        use silico_protocol::{Operation, Refusal};
        // A 10 × 20 × 30 box, reversed: written as OCCT's binary B-rep, whose last 9 bytes
        // are the shape's own reference (its orientation, a byte, 0 for FORWARD, then its
        // index and its location's, 4 bytes each: BinTools_ShapeSet::Write), with that
        // byte set to 1, REVERSED. Each face is then reversed in it, so its volume is
        // -6000 (-(10 × 20 × 30)): a solid turned inside out, as a sweep should never give.
        let made = super::make_box(10.0, 20.0, 30.0).unwrap();
        let written = super::ffi::shape_to_brep(made.get().unwrap()).unwrap();
        let mut brep = written.as_bytes().to_vec();
        let at = brep.len() - 9;
        assert_eq!(brep[at], 0, "the box isn't written FORWARD where expected");
        brep[at] = 1;
        let reversed = super::ffi::shape_from_brep(&brep).unwrap();
        let measured = super::ffi::measure(&reversed, silico_protocol::MEASURE_PRECISION).unwrap();
        assert!(
            (measured.volume + 6000.0).abs() <= 1e-9 * 6000.0,
            "{}",
            measured.volume
        );
        let refused = super::swept(&made, reversed, Operation::Extrusion, &[])
            .err()
            .unwrap();
        assert_eq!(
            refused.refusal(),
            &Refusal::ResultInvalid {
                operation: Operation::Extrusion,
                edges: Vec::new(),
            }
        );
    }
}
