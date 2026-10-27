// The C++ side of silico-kernel's cxx bridge: thin calls into OCCT that return shapes
// and plain values (ARCHITECTURE.md, Workers are Rust binaries).
//
// Every function here may throw. The trycatch below is how cxx turns a C++ exception
// into a Rust Err. cxx's default keeps only what(); this one also names OCCT's exception
// type (Standard_DomainError, StdFail_NotDone, ...), which is what tells failures apart,
// and catches anything else too. With it, no exception ever crosses the bridge.
#pragma once

#include <Standard_Failure.hxx>
#include <TopoDS_Shape.hxx>

#include <exception>
#include <memory>
#include <string>
#include <vector>

#include "rust/cxx.h"

namespace rust {
namespace behavior {
template <typename Try, typename Fail>
static void trycatch(Try &&func, Fail &&fail) noexcept try {
  func();
} catch (const Standard_Failure &e) {
  // "Standard_DomainError: <OCCT's message>", or just the type when there's no message.
  // (In OCCT 7.9, Standard_Failure isn't a std::exception: its type comes from its
  // runtime type, and its message from GetMessageString.)
  std::string message = e.DynamicType()->Name();
  const char *detail = e.GetMessageString();
  if (detail != nullptr && *detail != '\0') {
    message += ": ";
    message += detail;
  }
  fail(message.c_str());
} catch (const std::exception &e) {
  fail(e.what());
} catch (...) {
  fail("unknown C++ exception");
}
} // namespace behavior
} // namespace rust

namespace silico {

struct Measures;
struct Bounds;
struct Flat;
struct Point3;
struct Amounts;
struct FreecadMeasures;
struct DescribeOut;
struct DistanceOut;
struct HistoryEntry;
struct SkippedElement;
struct Counts;
enum class Kind : uint8_t;
enum class PointState : uint8_t;
enum class Relation : uint8_t;
enum class BooleanOp : uint8_t;
enum class BrepDefect : uint8_t;
struct EdgeFact;
struct EdgeFacts;
struct TopologyFacts;
struct FaceCurve;
enum class CurveKind : uint8_t;

// An OCCT shape, owned from Rust through a UniquePtr.
struct Shape {
  TopoDS_Shape shape;
};

std::unique_ptr<Shape> make_box(double dx, double dy, double dz);
std::unique_ptr<Shape> make_cylinder(double radius, double height);
std::unique_ptr<Shape> make_sphere(double radius);
// OCCT's own cylinder, not extruded (FreeCAD builds a cone with equal radii this way).
std::unique_ptr<Shape> make_cylinder_directly(double radius, double height);
std::unique_ptr<Shape> make_cone(double radius1, double radius2, double height);
std::unique_ptr<Shape> make_torus(double major_radius, double minor_radius);

// Volume, area (each integrated to the relative `precision`), center of mass, the
// numbers of distinct faces, edges, and vertices, and the tight bounding box.
Measures measure(const Shape &shape, double precision);

// The unit normal of the planes the shape's faces lie on (each face's surface's, in place),
// if every face lies on a plane, all of them parallel (within Precision::Angular()); not
// flat otherwise, or if it has no faces.
Flat flat_normal(const Shape &shape);

// The tight box of the shape's geometry, as measure's (Shape.optimalBoundingBox(False,
// False)), if it has any.
Bounds tight_bounds(const Shape &shape);

// Where `point` is against the shape (inside, on its boundary, or outside), by OCCT's solid
// classifier at the shape's largest tolerance (FreeCAD's `Shape.isInside(point, tolerance,
// True)`). Throws if the classifier can't tell.
PointState classify_point(const Shape &shape, const Point3 &point);

// The volume and area by OCCT's quick fixed-order integration: up to 1% off on faces
// trimmed by curves, and further on broken shapes (a "solid" of volume -2e-4 reads 1.9).
Amounts rough_amounts(const Shape &shape);

// The volume, area, and center by OCCT's default integration, and the tight bounding box,
// as FreeCAD measures them, for comparing with FreeCAD like for like.
FreecadMeasures measure_as_freecad(const Shape &shape);

// A batch of a shape's elements of `kind`, numbered from `first` (from 1, in
// TopExp::MapShapes order), `count` of them or as many as there are: their geometry and
// measures (areas and lengths to the relative `precision`), and how many of `kind` it has.
DescribeOut describe(const Shape &shape, Kind kind, uint32_t first, uint32_t count,
                     double precision);

// The least distance between the element of `first_kind` numbered `first` (from 1, in
// TopExp::MapShapes order) of `first_shape` and the one of `second_kind` numbered `second`
// of `second_shape`, as bounded: BRepExtrema_DistShapeShape on one thread, at OCCT's default
// precision, with every pair of points where it found it met (on the first, then on the
// second), in OCCT's order. OCCT is given the two in one order, by their own B-reps, so the
// answer doesn't depend on which came first. If either number isn't an element's, `found`
// is false and only the totals are set.
DistanceOut distance(const Shape &first_shape, Kind first_kind, uint32_t first,
                     const Shape &second_shape, Kind second_kind, uint32_t second);

// The least distance from `point` to the element of `kind` numbered `index` (from 1, in
// TopExp::MapShapes order) of `shape`, as bounded, as FreeCAD's QuickMeasure measures a
// circle's centre against a face or an edge: a vertex made at the point, and
// BRepExtrema_DistShapeShape from it to the element, on one thread, at OCCT's default
// precision, with every pair of points where it found it met (on the vertex, then on the
// element), in OCCT's order. The vertex is the first element (`first_total` 1), the element
// the second; if `index` isn't an element's, `found` is false and only the totals are set.
DistanceOut point_distance(const Shape &shape, Kind kind, uint32_t index, const Point3 &point);

// A triangle mesh of a shape's faces and polylines along its edges, for display. Faces
// and edges are in TopExp::MapShapes order (edges numbered as edge_facts's). Built whole in
// C++ and read from Rust as slices, so each array crosses the bridge in one copy.
class Tessellation {
public:
  std::vector<double> positions_;        // x, y, z per vertex
  std::vector<float> normals_;           // x, y, z per vertex: unit, out of the solid (but
                                         // see docs/ports/tessellation.md)
  std::vector<uint32_t> triangles_;      // three vertex indices per triangle, outward
  std::vector<uint32_t> face_triangles_; // triangles per face
  std::vector<double> edge_points_;      // x, y, z per polyline point, edge after edge
  std::vector<uint32_t> edge_points_per_edge_;

  rust::Slice<const double> positions() const { return {positions_.data(), positions_.size()}; }
  rust::Slice<const float> normals() const { return {normals_.data(), normals_.size()}; }
  rust::Slice<const uint32_t> triangles() const { return {triangles_.data(), triangles_.size()}; }
  rust::Slice<const uint32_t> face_triangles() const {
    return {face_triangles_.data(), face_triangles_.size()};
  }
  rust::Slice<const double> edge_points() const {
    return {edge_points_.data(), edge_points_.size()};
  }
  rust::Slice<const uint32_t> edge_points_per_edge() const {
    return {edge_points_per_edge_.data(), edge_points_per_edge_.size()};
  }
};

// A shape's solids meshed for export (docs/ports/mesh-export.md), as flat lists read from
// Rust as slices: each face's nodes and triangles, solid by solid and shell by shell, and
// each face's polygon on each of its edges, which the core joins the faces by. Facts only:
// silico-protocol's ExportMesh (src/export.rs) documents each list.
class ExportTriangulation {
public:
  std::vector<double> nodes_; // x, y, z per node
  std::vector<uint32_t> face_nodes_;
  std::vector<uint32_t> triangles_; // three node indices per triangle, outward
  std::vector<uint32_t> face_triangles_;
  std::vector<uint32_t> face_numbers_;
  std::vector<double> face_deflections_;
  std::vector<uint32_t> face_shells_;
  std::vector<double> face_tolerances_;
  std::vector<uint32_t> unoriented_faces_;
  std::vector<uint32_t> unoriented_shells_;
  std::vector<uint32_t> shell_solids_;
  std::vector<uint8_t> shell_outer_;
  uint32_t solids_ = 0;
  std::vector<uint32_t> loose_faces_;
  std::vector<uint32_t> edge_faces_;
  std::vector<uint32_t> edge_numbers_;
  std::vector<uint8_t> edge_degenerate_;
  std::vector<uint32_t> edge_vertices_; // first and last per use
  std::vector<double> edge_deflections_;
  std::vector<uint32_t> edge_node_counts_;
  std::vector<uint32_t> edge_nodes_;

  rust::Slice<const double> nodes() const { return {nodes_.data(), nodes_.size()}; }
  rust::Slice<const uint32_t> face_nodes() const { return {face_nodes_.data(), face_nodes_.size()}; }
  rust::Slice<const uint32_t> triangles() const { return {triangles_.data(), triangles_.size()}; }
  rust::Slice<const uint32_t> face_triangles() const {
    return {face_triangles_.data(), face_triangles_.size()};
  }
  rust::Slice<const uint32_t> face_numbers() const {
    return {face_numbers_.data(), face_numbers_.size()};
  }
  rust::Slice<const double> face_deflections() const {
    return {face_deflections_.data(), face_deflections_.size()};
  }
  rust::Slice<const uint32_t> face_shells() const {
    return {face_shells_.data(), face_shells_.size()};
  }
  rust::Slice<const double> face_tolerances() const {
    return {face_tolerances_.data(), face_tolerances_.size()};
  }
  rust::Slice<const uint32_t> unoriented_faces() const {
    return {unoriented_faces_.data(), unoriented_faces_.size()};
  }
  rust::Slice<const uint32_t> unoriented_shells() const {
    return {unoriented_shells_.data(), unoriented_shells_.size()};
  }
  rust::Slice<const uint32_t> shell_solids() const {
    return {shell_solids_.data(), shell_solids_.size()};
  }
  rust::Slice<const uint8_t> shell_outer() const {
    return {shell_outer_.data(), shell_outer_.size()};
  }
  uint32_t solids() const { return solids_; }
  rust::Slice<const uint32_t> loose_faces() const {
    return {loose_faces_.data(), loose_faces_.size()};
  }
  rust::Slice<const uint32_t> edge_faces() const { return {edge_faces_.data(), edge_faces_.size()}; }
  rust::Slice<const uint32_t> edge_numbers() const {
    return {edge_numbers_.data(), edge_numbers_.size()};
  }
  rust::Slice<const uint8_t> edge_degenerate() const {
    return {edge_degenerate_.data(), edge_degenerate_.size()};
  }
  rust::Slice<const uint32_t> edge_vertices() const {
    return {edge_vertices_.data(), edge_vertices_.size()};
  }
  rust::Slice<const double> edge_deflections() const {
    return {edge_deflections_.data(), edge_deflections_.size()};
  }
  rust::Slice<const uint32_t> edge_node_counts() const {
    return {edge_node_counts_.data(), edge_node_counts_.size()};
  }
  rust::Slice<const uint32_t> edge_nodes() const { return {edge_nodes_.data(), edge_nodes_.size()}; }
};

// Meshes a copy of the shape for export as FreeCAD does (`TopoShape::getFaces`): absolute
// deflection `tolerance`, angular deflection `angle`, but single-threaded, so the mesh is
// deterministic. The shape itself is left exactly as it was. With `force_face_deflection`,
// OCCT meshes every face at `tolerance` too, not raising a face's deflection to twice its
// own tolerance (IMeshTools_Parameters::ForceFaceDeflection).
std::unique_ptr<ExportTriangulation> export_mesh(const Shape &shape, double tolerance,
                                                 double angle, bool force_face_deflection);

// The length of the shape's bounding box diagonal (0 for an empty shape).
double bounding_diagonal(const Shape &shape);

// How far the shape's bounding box reaches from the origin on any axis: the largest
// coordinate of its corners, either way (0 for an empty shape). The box is OCCT's, enlarged
// by the shape's tolerances, so it bounds every point of the shape and its meshes.
double bounding_reach(const Shape &shape);

// Meshes a copy of the shape with OCCT's mesher, single-threaded so the mesh is
// deterministic. The shape itself is left exactly as it was.
// `deflection` is the largest distance allowed between the mesh and the surface.
std::unique_ptr<Tessellation> tessellate(const Shape &shape, double deflection);

// For each edge (numbered from 1 in TopExp::MapShapes order), in order: whether it's
// degenerate or a seam, its end vertices, and the faces it lies on, as
// TopExp::MapShapesAndAncestors lists them. Facts only: Rust decides which edges a fillet
// or chamfer may take.
EdgeFacts edge_facts(const Shape &shape);

// A fillet or chamfer of a shape, given its edges one at a time (numbered as edge_facts
// numbers them, faces in TopExp::MapShapes order too), which Rust then asks OCCT about:
// which contour took each edge (0 for none), and each contour's edges, before it's built.
// Built, it appends what it did to the shape's faces, edges, and vertices to `history`, and
// throws if OCCT can't build it; whether the result is valid is the caller's question.
class Dressing {
public:
  enum class Of { Fillet, Chamfer };
  Dressing(const TopoDS_Shape &shape, Of of);
  ~Dressing();
  void add_fillet(uint32_t edge, double radius);
  // `distance` along `face`, and along the edge's other face `other`, or as far as a
  // chamfer face at `radians` to `face` reaches.
  void add_distances(uint32_t edge, uint32_t face, double distance, double other);
  void add_distance_angle(uint32_t edge, uint32_t face, double distance, double radians);
  uint32_t contour_of(uint32_t edge) const;
  rust::Vec<uint32_t> contour_edges(uint32_t contour) const;
  std::unique_ptr<Shape> build(rust::Vec<HistoryEntry> &history);
  // After a build that failed, OCCT's report of where: the contours it couldn't build (which
  // may include a corner stripe building added, past the given contours), and the corners it
  // couldn't blend (vertices, numbered as `edge_facts` numbers them). A chamfer's through
  // Silico's OCCT patch 0002.
  rust::Vec<uint32_t> faulty_contours() const;
  rust::Vec<uint32_t> faulty_vertices() const;
  // How many contours were given (OCCT's `NbContours`, which stops before corner stripes).
  uint32_t contour_count() const;
  // Whether OCCT set an error status on given `contour` (`StripeStatus`): its own surfaces
  // failed. Throws for a contour that wasn't given, whose status can't be read.
  bool contour_failed(uint32_t contour) const;

private:
  struct Impl;
  std::unique_ptr<Impl> impl_;
};

std::unique_ptr<Dressing> new_fillet(const Shape &shape);
std::unique_ptr<Dressing> new_chamfer(const Shape &shape);

// The shape moved by (dx, dy, dz), appending what became of its faces, edges, and vertices
// to `history`. The geometry isn't copied: the result is the shape with a new location,
// the move composed with its own.
std::unique_ptr<Shape> translate(const Shape &shape, double dx, double dy, double dz,
                                 rust::Vec<HistoryEntry> &history);

// A flat face bounded by the closed polygon through `points` (checked by Rust first: flat,
// distinct, not crossing itself). Its edges and vertices are in the points' order.
std::unique_ptr<Shape> make_polygon_face(rust::Slice<const Point3> points);

// A face for each region on the plane through the origin with `normal`, its u axis along
// `x_axis`, as FreeCAD's FaceMakerBullseye builds one (BRep_Builder: a Geom_Plane, the outer
// wire, then each hole), with nothing reoriented or fixed: a vertex for each of `points`,
// an edge for each of `curves` between the vertices it names, a wire of each run of
// `loop_curves[i]` curves in order, and a face of each run of `region_loops[j]` wires (its
// outer wire first). One face, or a compound of them in order. Appends each curve's edge
// (input 0's edge n, kept) and each point's vertex (input 0's vertex n, kept) to `history`,
// numbered in the result. Rust checks the regions first; whether the faces are valid is the
// caller's question.
std::unique_ptr<Shape> make_faces(Point3 normal, Point3 x_axis, rust::Slice<const Point3> points,
                                  rust::Slice<const FaceCurve> curves,
                                  rust::Slice<const uint32_t> loop_curves,
                                  rust::Slice<const uint32_t> region_loops,
                                  rust::Vec<HistoryEntry> &history);

// `face` swept along (dx, dy, dz) into a solid (BRepPrimAPI_MakePrism, as FreeCAD's
// makeElementPrism). Appends the face's history to `history`, and the result's faces the
// sweep starts and ends with (OCCT's FirstShape and LastShape) to `start` and `end`, 0
// where the result doesn't have one.
std::unique_ptr<Shape> extrude(const Shape &face, double dx, double dy, double dz,
                               rust::Vec<HistoryEntry> &history, rust::Vec<uint32_t> &start,
                               rust::Vec<uint32_t> &end);

// `face` swept around the axis through `origin` along `direction` by `radians`
// (BRepPrimAPI_MakeRevol, as FreeCAD's makeElementRevolve). History and ends as extrude's.
std::unique_ptr<Shape> revolve(const Shape &face, Point3 origin, Point3 direction,
                               double radians, rust::Vec<HistoryEntry> &history,
                               rust::Vec<uint32_t> &start, rust::Vec<uint32_t> &end);

// `faces` (a profile's faces, or a compound of them) moved by `start`, then swept along
// `vector` (BRepPrimAPI_MakePrism, as FreeCAD's makeElementPrism, on the moved faces, as
// FreeCAD's symmetric and two-sided pads move theirs). Appends each of the faces' faces,
// edges, and vertices' start counterpart (FirstShape) as Modified, end counterpart
// (LastShape) as Generated, and sides (Generated) to `history`, 0 where the result doesn't
// have one.
std::unique_ptr<Shape> sweep_prism(const Shape &faces, Point3 start, Point3 vector,
                                   rust::Vec<HistoryEntry> &history);

// `faces` turned `start` radians about the axis through `origin` along `direction`, then
// swept `angle` radians about it (BRepPrimAPI_MakeRevol, as FreeCAD's makeElementRevolve).
// History as sweep_prism's; an element whose end is its start (on the axis) reports only
// the start.
std::unique_ptr<Shape> sweep_revolution(const Shape &faces, Point3 origin, Point3 direction,
                                        double start, double angle,
                                        rust::Vec<HistoryEntry> &history);

// Fuses, cuts, or intersects `base` and `tool` with OCCT's Boolean Operations, as FreeCAD
// does (non-destructive, OCCT's default fuzzy value), but single-threaded. Appends the
// history of `base` as input 0 and of `tool` as input 1, the names of the warnings OCCT
// reported (BOPAlgo_AlertTooSmallEdge, ...) to `warnings`, and, two by two, each pair of
// elements OCCT skipped intersecting (BOPAlgo_AlertIntersectionOfPairOfShapesFailed) to
// `skipped`. Throws with OCCT's report if it fails.
std::unique_ptr<Shape> boolean(BooleanOp op, const Shape &base, const Shape &tool,
                               rust::Vec<HistoryEntry> &history,
                               rust::Vec<rust::String> &warnings,
                               rust::Vec<SkippedElement> &skipped);

// How many distinct faces, edges, vertices, and solids the shape has.
Counts count_elements(const Shape &shape);
// Each face's edges and outer wire's edges, and each edge's vertices, each in
// TopExp::MapShapes order within the element, numbered as the shape numbers them.
TopologyFacts topology(const Shape &shape);

// Whether the faces of `result` that aren't in `input` (those an operation made or
// changed) are valid, and its shells oriented: OCCT's BRepAlgo::IsValid, as FreeCAD's
// features call it. Repairs SameParameter flags on those faces' edges in place first.
bool is_valid_where_changed(const Shape &input, const Shape &result);
// The same, for an operation on two shapes: the faces in neither input are checked.
bool is_valid_where_changed_from_two(const Shape &first, const Shape &second,
                                     const Shape &result);

// Whether OCCT's BRepCheck_Analyzer finds the whole shape valid.
bool is_valid(const Shape &shape);

// OCCT's binary B-rep format, which is how shapes cross the worker boundary.
std::unique_ptr<std::string> shape_to_brep(const Shape &shape);
std::unique_ptr<Shape> shape_from_brep(rust::Slice<const uint8_t> brep);
// OCCT's text B-rep, as FreeCAD's Shape.exportBrep writes it.
std::unique_ptr<Shape> shape_from_text_brep(rust::Slice<const uint8_t> brep);
// What makes a shape read from bytes unusable, if anything.
BrepDefect brep_defect(const Shape &shape);

// Floating-point traps (ARCHITECTURE.md, Deterministic kernel): the trap-enable bits that
// are set, 0 if none, and turning them all off through OCCT.
uint32_t fp_trap_mask();
void disable_fp_traps();

#ifdef SILICO_TEST_OPS
// Each throws something different, to prove every branch of the trycatch above.
void debug_throw_standard_failure(rust::Str message);
void debug_throw_std_runtime_error(rust::Str message);
void debug_throw_non_exception();
// Turns floating-point traps on through OCCT. Linux keeps them on; some Apple silicon
// ignores the trap-enable bits (they read back as 0), and some keeps them.
void debug_enable_fp_traps();
// Crashes the process with SIGSEGV, as a fault inside OCCT would.
void debug_raise_sigsegv();
// A box with one face left out: a solid whose shell doesn't close, which
// BRepCheck_Analyzer must find invalid.
std::unique_ptr<Shape> debug_open_box();
// A compound of a 10 x 20 x 30 box and an edge on no face, from (20, 0, 0) to (30, 0, 0):
// the box's 12 edges, then the loose one (Edge13).
std::unique_ptr<Shape> debug_box_with_loose_edge();
// A compound with nothing in it: a shape that isn't null, with no faces, edges, or vertices.
std::unique_ptr<Shape> debug_empty_shape();
// A compound of a 10 x 20 x 30 box and a face with a surface and no wires (a plane at x = 50,
// Face7), which OCCT's STEP writer drops with only a warning.
std::unique_ptr<Shape> debug_box_with_wireless_face();
// The box and a 10 x 10 square face at x = 50 with a second, empty wire, which OCCT's STEP
// writer drops (" a Wire not mapped") with only a warning.
std::unique_ptr<Shape> debug_box_with_empty_wire_face();
// The box and the square face with an INTERNAL wire (a slit), which OCCT's STEP writer skips
// on purpose.
std::unique_ptr<Shape> debug_box_with_internal_wire_face();
#endif

} // namespace silico
