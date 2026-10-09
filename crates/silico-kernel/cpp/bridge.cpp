#include "silico-kernel/cpp/bridge.h"
#include "silico-kernel/src/lib.rs.h"

#include <BRepAdaptor_Curve.hxx>
#include <BRepAlgo.hxx>
#include <BRepCheck_Analyzer.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRepBndLib.hxx>
#include <BRepBuilderAPI_Copy.hxx>
#include <BRep_Tool.hxx>
#include <Bnd_Box.hxx>
#include <GCPnts_TangentialDeflection.hxx>
#include <Poly_Triangulation.hxx>
#include <BRep_Builder.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>
#include <BRepGProp.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BinTools.hxx>
#include <GProp_GProps.hxx>
#include <OSD.hxx>
#include <Precision.hxx>
#include <TopExp.hxx>
#include <NCollection_IndexedMap.hxx>
#include <NCollection_List.hxx>
#include <TopTools_ShapeMapHasher.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Shell.hxx>
#include <TopoDS_Solid.hxx>
#include <gp_Pnt.hxx>

#include <cmath>
#include <csignal>
#include <sstream>
#include <stdexcept>
#include <string>

#if defined(__linux__)
#include <fenv.h>
#endif

namespace silico {

std::unique_ptr<Shape> make_box(double dx, double dy, double dz) {
  // Throws Standard_DomainError for a size at or below Precision::Confusion().
  BRepPrimAPI_MakeBox box(dx, dy, dz);
  return std::make_unique<Shape>(Shape{box.Shape()});
}

MassProperties mass_properties(const Shape &shape) {
  GProp_GProps props;
  BRepGProp::VolumeProperties(shape.shape, props);
  gp_Pnt c = props.CentreOfMass();
  return MassProperties{props.Mass(), Point3{c.X(), c.Y(), c.Z()}};
}

namespace {
// Distinct sub-shapes, numbered from 1 in the order TopExp::MapShapes finds them.
using ShapeMap = NCollection_IndexedMap<TopoDS_Shape, TopTools_ShapeMapHasher>;

uint32_t count_distinct(const TopoDS_Shape &shape, TopAbs_ShapeEnum kind) {
  ShapeMap map;
  TopExp::MapShapes(shape, kind, map);
  return static_cast<uint32_t>(map.Extent());
}
} // namespace

ShapeSummary summarize(const Shape &shape) {
  GProp_GProps volume;
  BRepGProp::VolumeProperties(shape.shape, volume);
  GProp_GProps surface;
  BRepGProp::SurfaceProperties(shape.shape, surface);
  const gp_Pnt c = volume.CentreOfMass();
  return ShapeSummary{volume.Mass(),
                      surface.Mass(),
                      Point3{c.X(), c.Y(), c.Z()},
                      count_distinct(shape.shape, TopAbs_FACE),
                      count_distinct(shape.shape, TopAbs_EDGE),
                      count_distinct(shape.shape, TopAbs_VERTEX),
                      // Filled in by Rust, which checks each shape at most once.
                      false};
}

double bounding_diagonal(const Shape &shape) {
  Bnd_Box box;
  BRepBndLib::Add(shape.shape, box);
  return box.IsVoid() ? 0.0 : std::sqrt(box.SquareExtent());
}

std::unique_ptr<Tessellation> tessellate(const Shape &shape, double deflection) {
  // Mesh a copy. Meshing changes the shape it meshes: OCCT stores the mesh on it (where
  // later meshing would reuse it) and updates edge flags, which changes the shape's B-rep
  // bytes. The copy shares the geometry but not the topology those live on.
  const TopoDS_Shape copy = BRepBuilderAPI_Copy(shape.shape, false, false).Shape();
  // isRelative = false, angular deflection 0.5 rad, isInParallel = false (deterministic).
  BRepMesh_IncrementalMesh mesher(copy, deflection, false, 0.5, false);
  if (!mesher.IsDone()) {
    throw Standard_Failure("the shape couldn't be meshed");
  }
  auto out = std::make_unique<Tessellation>();
  int unmeshed = 0;

  ShapeMap faces;
  TopExp::MapShapes(copy, TopAbs_FACE, faces);
  for (int i = 1; i <= faces.Extent(); ++i) {
    const TopoDS_Face face = TopoDS::Face(faces(i));
    TopLoc_Location location;
    const Handle(Poly_Triangulation) mesh = BRep_Tool::Triangulation(face, location);
    if (mesh.IsNull()) {
      ++unmeshed;
      continue;
    }
    const gp_Trsf transform = location.Transformation();
    const uint32_t base = static_cast<uint32_t>(out->positions_.size() / 3);
    for (int n = 1; n <= mesh->NbNodes(); ++n) {
      const gp_Pnt p = mesh->Node(n).Transformed(transform);
      out->positions_.insert(out->positions_.end(), {p.X(), p.Y(), p.Z()});
    }
    // A reversed face's triangles wind the other way, so they still face outward.
    const bool reversed = face.Orientation() == TopAbs_REVERSED;
    for (int t = 1; t <= mesh->NbTriangles(); ++t) {
      int a, b, c;
      mesh->Triangle(t).Get(a, b, c);
      if (reversed) {
        std::swap(b, c);
      }
      out->triangles_.insert(out->triangles_.end(),
                             {base + static_cast<uint32_t>(a - 1),
                              base + static_cast<uint32_t>(b - 1),
                              base + static_cast<uint32_t>(c - 1)});
    }
    out->face_triangles_.push_back(static_cast<uint32_t>(mesh->NbTriangles()));
  }

  ShapeMap edges;
  TopExp::MapShapes(copy, TopAbs_EDGE, edges);
  for (int i = 1; i <= edges.Extent(); ++i) {
    const TopoDS_Edge edge = TopoDS::Edge(edges(i));
    if (BRep_Tool::Degenerated(edge)) {
      out->edge_points_per_edge_.push_back(0);
      continue;
    }
    const BRepAdaptor_Curve curve(edge);
    const GCPnts_TangentialDeflection points(curve, 0.5, deflection);
    for (int k = 1; k <= points.NbPoints(); ++k) {
      const gp_Pnt p = points.Value(k);
      out->edge_points_.insert(out->edge_points_.end(), {p.X(), p.Y(), p.Z()});
    }
    out->edge_points_per_edge_.push_back(static_cast<uint32_t>(points.NbPoints()));
  }
  if (unmeshed > 0) {
    // A display with holes in it would be worse than an error.
    throw Standard_Failure((std::to_string(unmeshed) + " faces couldn't be meshed").c_str());
  }
  return out;
}

uint32_t edge_count(const Shape &shape) { return count_distinct(shape.shape, TopAbs_EDGE); }

std::unique_ptr<Shape> fillet(const Shape &shape, rust::Slice<const uint32_t> edges,
                              double radius) {
  ShapeMap map;
  TopExp::MapShapes(shape.shape, TopAbs_EDGE, map);
  BRepFilletAPI_MakeFillet make(shape.shape);
  for (uint32_t n : edges) {
    // The Rust side checks the numbers; this guards the map itself.
    if (n < 1 || static_cast<int>(n) > map.Extent()) {
      throw Standard_Failure("edge number out of range");
    }
    make.Add(radius, TopoDS::Edge(map(static_cast<int>(n))));
  }
  make.Build();
  if (!make.IsDone()) {
    throw Standard_Failure("the fillet couldn't be built");
  }
  return std::make_unique<Shape>(Shape{make.Shape()});
}

bool is_valid_where_changed(const Shape &input, const Shape &result) {
  NCollection_List<TopoDS_Shape> inputs;
  inputs.Append(input.shape);
  // closedSolid = false, GeomCtrl = false: FreeCAD's arguments since 87293fac9b.
  return BRepAlgo::IsValid(inputs, result.shape, false, false);
}

bool is_valid(const Shape &shape) { return BRepCheck_Analyzer(shape.shape).IsValid(); }

// Returned whole, so Rust copies it once (copying byte by byte across the bridge costs
// about 16 times more).
std::unique_ptr<std::string> shape_to_brep(const Shape &shape) {
  std::ostringstream out(std::ios::binary);
  // Without triangulations: a mesh made for display must never change a shape's bytes.
  BinTools::Write(shape.shape, out, false, false, BinTools_FormatVersion_CURRENT);
  if (!out) {
    throw Standard_Failure("writing the B-rep failed");
  }
  return std::make_unique<std::string>(std::move(out).str());
}

namespace {
// Reads straight from the caller's bytes, without copying them into a string first.
// Seekable, because BinTools moves around in the stream as it reads.
class ByteStreamBuf : public std::streambuf {
public:
  ByteStreamBuf(const char *data, size_t size) {
    char *begin = const_cast<char *>(data);
    setg(begin, begin, begin + size);
  }

protected:
  pos_type seekoff(off_type off, std::ios_base::seekdir dir,
                   std::ios_base::openmode which) override {
    if (!(which & std::ios_base::in)) {
      return pos_type(off_type(-1));
    }
    char *base = dir == std::ios_base::beg   ? eback()
                 : dir == std::ios_base::cur ? gptr()
                                             : egptr();
    const off_type target = (base - eback()) + off;
    if (target < 0 || target > egptr() - eback()) {
      return pos_type(off_type(-1));
    }
    setg(eback(), eback() + target, egptr());
    return pos_type(target);
  }

  pos_type seekpos(pos_type pos, std::ios_base::openmode which) override {
    return seekoff(off_type(pos), std::ios_base::beg, which);
  }
};
} // namespace

std::unique_ptr<Shape> shape_from_brep(rust::Slice<const uint8_t> brep) {
  ByteStreamBuf buffer(reinterpret_cast<const char *>(brep.data()), brep.size());
  std::istream in(&buffer);
  TopoDS_Shape shape;
  BinTools::Read(shape, in);
  if (shape.IsNull()) {
    throw Standard_Failure("not a B-rep");
  }
  return std::make_unique<Shape>(Shape{shape});
}

uint32_t fp_trap_mask() {
#if defined(__linux__)
  // glibc reports the enabled traps; fegetexcept computes nothing, so it's exact.
  return static_cast<uint32_t>(fegetexcept());
#elif defined(__aarch64__)
  // The trap-enable bits of FPCR: IOE, DZE, OFE, UFE, IXE (8-12) and IDE (15). Read
  // directly, since macOS has no fegetexcept.
  uint64_t fpcr;
  __asm__ volatile("mrs %0, fpcr" : "=r"(fpcr));
  return static_cast<uint32_t>(fpcr & 0x9f00);
#else
#error "fp_trap_mask: unsupported platform"
#endif
}

void disable_fp_traps() { OSD::SetFloatingSignal(false); }

#ifdef SILICO_TEST_OPS
void debug_throw_standard_failure(rust::Str message) {
  throw Standard_Failure(std::string(message).c_str());
}

void debug_throw_std_runtime_error(rust::Str message) {
  throw std::runtime_error(std::string(message));
}

void debug_throw_non_exception() { throw 42; }

void debug_enable_fp_traps() { OSD::SetFloatingSignal(true); }

std::unique_ptr<Shape> debug_open_box() {
  const TopoDS_Shape box = BRepPrimAPI_MakeBox(10, 20, 30).Shape();
  BRep_Builder builder;
  TopoDS_Shell shell;
  builder.MakeShell(shell);
  bool skipped = false;
  for (TopExp_Explorer faces(box, TopAbs_FACE); faces.More(); faces.Next()) {
    if (!skipped) {
      skipped = true;
      continue;
    }
    builder.Add(shell, faces.Current());
  }
  TopoDS_Solid solid;
  builder.MakeSolid(solid);
  builder.Add(solid, shell);
  return std::make_unique<Shape>(Shape{solid});
}

void debug_raise_sigsegv() {
  // Rust's runtime handles SIGSEGV (to report stack overflows): its handler restores the
  // default action and returns, so a real fault faults again and kills the process. A
  // raised signal doesn't repeat, so restore the default first, as that second fault would.
  std::signal(SIGSEGV, SIG_DFL);
  std::raise(SIGSEGV);
}
#endif

} // namespace silico
