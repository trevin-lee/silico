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
  std::string message = e.ExceptionType();
  const char *detail = e.what();
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
struct Point3;
struct Amounts;
struct HistoryEntry;
struct Counts;
enum class Kind : uint8_t;
enum class Relation : uint8_t;
enum class BooleanOp : uint8_t;

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

// Volume, area (each integrated to a relative precision of 1e-9), center of mass, and
// the numbers of distinct faces, edges, and vertices.
Measures measure(const Shape &shape);

// The volume and area by OCCT's quick fixed-order integration: up to 1% off on faces
// trimmed by curves, and further on broken shapes (a "solid" of volume -2e-4 reads 1.9).
Amounts rough_amounts(const Shape &shape);

// The number of distinct edges, which TopExp::MapShapes numbers from 1.
uint32_t edge_count(const Shape &shape);

// A triangle mesh of a shape's faces and polylines along its edges, for display. Faces
// and edges are in TopExp::MapShapes order (edges numbered as fillet's). Built whole in
// C++ and read from Rust as slices, so each array crosses the bridge in one copy.
class Tessellation {
public:
  std::vector<double> positions_;        // x, y, z per vertex
  std::vector<uint32_t> triangles_;      // three vertex indices per triangle, outward
  std::vector<uint32_t> face_triangles_; // triangles per face
  std::vector<double> edge_points_;      // x, y, z per polyline point, edge after edge
  std::vector<uint32_t> edge_points_per_edge_;

  rust::Slice<const double> positions() const { return {positions_.data(), positions_.size()}; }
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

// The length of the shape's bounding box diagonal (0 for an empty shape).
double bounding_diagonal(const Shape &shape);

// Meshes a copy of the shape with OCCT's mesher, single-threaded so the mesh is
// deterministic. The shape itself is left exactly as it was.
// `deflection` is the largest distance allowed between the mesh and the surface.
std::unique_ptr<Tessellation> tessellate(const Shape &shape, double deflection);

// Rounds the given edges (numbered as edge_count's) with a constant radius, appending what
// it did to the shape's faces, edges, and vertices to `history`. Throws if OCCT can't build
// it; whether the result is valid is the caller's question.
std::unique_ptr<Shape> fillet(const Shape &shape, rust::Slice<const uint32_t> edges,
                              double radius, rust::Vec<HistoryEntry> &history);

// The shape moved by (dx, dy, dz), appending what became of its faces, edges, and vertices
// to `history`. The geometry isn't copied: the result is the shape with a new location,
// the move composed with its own.
std::unique_ptr<Shape> translate(const Shape &shape, double dx, double dy, double dz,
                                 rust::Vec<HistoryEntry> &history);

// A flat face bounded by the closed polygon through `points` (checked by Rust first: flat,
// distinct, not crossing itself). Its edges and vertices are in the points' order.
std::unique_ptr<Shape> make_polygon_face(rust::Slice<const Point3> points);

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

// Fuses, cuts, or intersects `base` and `tool` with OCCT's Boolean Operations, as FreeCAD
// does (non-destructive, OCCT's default fuzzy value), but single-threaded. Appends the
// history of `base` as input 0 and of `tool` as input 1, and the names of the warnings
// OCCT reported (BOPAlgo_AlertTooSmallEdge, ...) to `warnings`. Throws with OCCT's report
// if it fails.
std::unique_ptr<Shape> boolean(BooleanOp op, const Shape &base, const Shape &tool,
                               rust::Vec<HistoryEntry> &history,
                               rust::Vec<rust::String> &warnings);

// How many distinct faces, edges, vertices, and solids the shape has.
Counts count_elements(const Shape &shape);

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

// Floating-point traps (ARCHITECTURE.md, Deterministic kernel): the trap-enable bits that
// are set, 0 if none, and turning them all off through OCCT.
uint32_t fp_trap_mask();
void disable_fp_traps();

#ifdef SILICO_TEST_OPS
// Each throws something different, to prove every branch of the trycatch above.
void debug_throw_standard_failure(rust::Str message);
void debug_throw_std_runtime_error(rust::Str message);
void debug_throw_non_exception();
// Turns floating-point traps on through OCCT. Only Linux can: Apple silicon ignores the
// trap-enable bits.
void debug_enable_fp_traps();
// Crashes the process with SIGSEGV, as a fault inside OCCT would.
void debug_raise_sigsegv();
// A box with one face left out: a solid whose shell doesn't close, which
// BRepCheck_Analyzer must find invalid.
std::unique_ptr<Shape> debug_open_box();
// A compound with nothing in it: a shape that isn't null, with no faces, edges, or vertices.
std::unique_ptr<Shape> debug_empty_shape();
#endif

} // namespace silico
