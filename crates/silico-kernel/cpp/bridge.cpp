#include "silico-kernel/cpp/bridge.h"
#include "silico-kernel/src/lib.rs.h"

#include <BRepAdaptor_Curve.hxx>
#include <BRepAlgo.hxx>
#include <BRepAlgoAPI_Common.hxx>
#include <BRepAlgoAPI_Cut.hxx>
#include <BRepAlgoAPI_Fuse.hxx>
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
#include <BRepPrimAPI_MakeCone.hxx>
#include <BRepPrimAPI_MakeCylinder.hxx>
#include <BRepPrimAPI_MakeSphere.hxx>
#include <BRepBuilderAPI_MakeEdge.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_MakeWire.hxx>
#include <BRepPrimAPI_MakePrism.hxx>
#include <BRepPrimAPI_MakeRevol.hxx>
#include <BRepPrim_Cylinder.hxx>
#include <gp_Ax1.hxx>
#include <gp_Circ.hxx>
#include <gp_Dir.hxx>
#include <gp_Vec.hxx>
#include <BinTools.hxx>
#include <GProp_GProps.hxx>
#include <Message_Alert.hxx>
#include <Message_Report.hxx>
#include <OSD.hxx>
#include <Precision.hxx>
#include <TopExp.hxx>
#include <NCollection_IndexedMap.hxx>
#include <NCollection_List.hxx>
#include <TopTools_ShapeMapHasher.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Compound.hxx>
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

namespace {
// Distinct sub-shapes, numbered from 1 in the order TopExp::MapShapes finds them.
using ShapeMap = NCollection_IndexedMap<TopoDS_Shape, TopTools_ShapeMapHasher>;

uint32_t count_distinct(const TopoDS_Shape &shape, TopAbs_ShapeEnum kind) {
  ShapeMap map;
  TopExp::MapShapes(shape, kind, map);
  return static_cast<uint32_t>(map.Extent());
}

// Degrees to radians as FreeCAD converts them (Base::toRadians), so OCCT gets the same
// angles, bit for bit.
double radians(double degrees) { return degrees * (M_PI / 180.0); }
} // namespace

// Each primitive is built as FreeCAD's Part and PartDesign features build it, so faces
// and edges come out in the same order (FreeCAD's Face1, Edge1, ...). See
// docs/ports/primitives.md.

std::unique_ptr<Shape> make_cylinder(double radius, double height) {
  // FreeCAD extrudes the cylinder's bottom disc (Part::Cylinder, through PrismExtension),
  // which numbers faces and edges differently from the cylinder itself.
  BRepPrimAPI_MakeCylinder cylinder(radius, height, radians(360.0));
  BRepPrim_Cylinder &primitive = cylinder.Cylinder();
  BRepPrimAPI_MakePrism prism(primitive.BottomFace(), gp_Vec(0.0, 0.0, height));
  return std::make_unique<Shape>(Shape{prism.Shape()});
}

std::unique_ptr<Shape> make_sphere(double radius) {
  BRepPrimAPI_MakeSphere sphere(radius, radians(-90.0), radians(90.0), radians(360.0));
  return std::make_unique<Shape>(Shape{sphere.Shape()});
}

std::unique_ptr<Shape> make_cylinder_directly(double radius, double height) {
  BRepPrimAPI_MakeCylinder cylinder(radius, height, radians(360.0));
  return std::make_unique<Shape>(Shape{cylinder.Shape()});
}

std::unique_ptr<Shape> make_cone(double radius1, double radius2, double height) {
  BRepPrimAPI_MakeCone cone(radius1, radius2, height, radians(360.0));
  return std::make_unique<Shape>(Shape{cone.Shape()});
}

std::unique_ptr<Shape> make_torus(double major_radius, double minor_radius) {
  // FreeCAD revolves a circular face about Z (TopoShape::makeTorus) rather than building
  // OCCT's torus: the circle centered at (major, 0, 0), in the XZ plane, from -180 to 180
  // degrees, revolved a full turn.
  gp_Circ circle;
  circle.SetRadius(minor_radius);
  circle.SetAxis(gp_Ax1(gp_Pnt(major_radius, 0.0, 0.0), gp_Dir(0.0, 1.0, 0.0)));
  BRepBuilderAPI_MakeEdge edge(circle, radians(-180.0), radians(180.0));
  BRepBuilderAPI_MakeWire wire;
  wire.Add(edge.Edge());
  BRepBuilderAPI_MakeFace face(wire.Wire());
  BRepPrimAPI_MakeRevol revolve(face.Face(), gp_Ax1(gp_Pnt(0.0, 0.0, 0.0), gp_Dir(0.0, 0.0, 1.0)),
                                radians(360.0), true);
  return std::make_unique<Shape>(Shape{revolve.Shape()});
}

Measures measure(const Shape &shape) {
  // Integrated adaptively to a relative precision of 1e-9. OCCT's default, a fixed order
  // of Gauss points per face (FreeCAD's Shape.Volume), misjudges faces trimmed by curves:
  // the common of two tori (R 2, r 1) 2.2 apart measures 12.238 for 12.3586, 1% low.
  constexpr double precision = 1e-9;
  GProp_GProps volume;
  BRepGProp::VolumeProperties(shape.shape, volume, precision);
  GProp_GProps surface;
  BRepGProp::SurfaceProperties(shape.shape, surface, precision);
  const gp_Pnt c = volume.CentreOfMass();
  return Measures{volume.Mass(),
                  surface.Mass(),
                  Point3{c.X(), c.Y(), c.Z()},
                  count_distinct(shape.shape, TopAbs_FACE),
                  count_distinct(shape.shape, TopAbs_EDGE),
                  count_distinct(shape.shape, TopAbs_VERTEX)};
}

Amounts rough_amounts(const Shape &shape) {
  GProp_GProps volume;
  BRepGProp::VolumeProperties(shape.shape, volume);
  GProp_GProps surface;
  BRepGProp::SurfaceProperties(shape.shape, surface);
  return Amounts{volume.Mass(), surface.Mass()};
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

namespace {
Kind kind_of(TopAbs_ShapeEnum type) {
  switch (type) {
  case TopAbs_VERTEX:
    return Kind::Vertex;
  case TopAbs_EDGE:
    return Kind::Edge;
  case TopAbs_FACE:
    return Kind::Face;
  case TopAbs_SOLID:
    return Kind::Solid;
  default:
    return Kind::Other;
  }
}

// Reports what `maker` did to input `input` (`source`): for each of its faces, edges, and
// vertices, whether the result has it unchanged (the same sub-shape: TShape and location),
// and what OCCT says it was modified into and generated, each numbered in the result
// (0 when the result doesn't have it) with its position in OCCT's list. Facts only; Rust
// decides what to keep. For each element the Kept entry comes first: Rust relies on it.
// OCCT's IsDeleted isn't asked: it's wrong for fillets (docs/ports/history.md).
void capture_history(BRepBuilderAPI_MakeShape &maker, uint32_t input,
                     const TopoDS_Shape &source, const TopoDS_Shape &result,
                     rust::Vec<HistoryEntry> &history) {
  // Indexed by Kind's values, which lib.rs fixes at Vertex 0, Edge 1, Face 2, Solid 3.
  static_assert(static_cast<uint8_t>(Kind::Vertex) == 0 && static_cast<uint8_t>(Kind::Edge) == 1 &&
                    static_cast<uint8_t>(Kind::Face) == 2 && static_cast<uint8_t>(Kind::Solid) == 3,
                "results[] is indexed by Kind");
  ShapeMap results[4];
  TopExp::MapShapes(result, TopAbs_VERTEX, results[0]);
  TopExp::MapShapes(result, TopAbs_EDGE, results[1]);
  TopExp::MapShapes(result, TopAbs_FACE, results[2]);
  TopExp::MapShapes(result, TopAbs_SOLID, results[3]);
  for (TopAbs_ShapeEnum type : {TopAbs_FACE, TopAbs_EDGE, TopAbs_VERTEX}) {
    const Kind input_kind = kind_of(type);
    ShapeMap inputs;
    TopExp::MapShapes(source, type, inputs);
    for (int i = 1; i <= inputs.Extent(); ++i) {
      const TopoDS_Shape &element = inputs(i);
      auto record = [&](Relation relation, const TopoDS_Shape &output, uint32_t position) {
        const Kind kind = kind_of(output.ShapeType());
        const int index =
            kind == Kind::Other ? 0 : results[static_cast<uint8_t>(kind)].FindIndex(output);
        history.push_back(HistoryEntry{input, input_kind, static_cast<uint32_t>(i), relation,
                                       kind, static_cast<uint32_t>(index), position});
      };
      if (results[static_cast<uint8_t>(input_kind)].Contains(element)) {
        record(Relation::Kept, element, 0);
      }
      uint32_t position = 0;
      for (const TopoDS_Shape &output : maker.Modified(element)) {
        record(Relation::Modified, output, ++position);
      }
      position = 0;
      for (const TopoDS_Shape &output : maker.Generated(element)) {
        record(Relation::Generated, output, ++position);
      }
    }
  }
}
} // namespace

Counts count_elements(const Shape &shape) {
  return Counts{count_distinct(shape.shape, TopAbs_FACE), count_distinct(shape.shape, TopAbs_EDGE),
                count_distinct(shape.shape, TopAbs_VERTEX),
                count_distinct(shape.shape, TopAbs_SOLID)};
}

std::unique_ptr<Shape> fillet(const Shape &shape, rust::Slice<const uint32_t> edges,
                              double radius, rust::Vec<HistoryEntry> &history) {
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
  const TopoDS_Shape result = make.Shape();
  capture_history(make, 0, shape.shape, result, history);
  return std::make_unique<Shape>(Shape{result});
}

namespace {
bool is_identity(const gp_Trsf &trsf) {
  for (int row = 1; row <= 3; ++row) {
    for (int column = 1; column <= 4; ++column) {
      if (trsf.Value(row, column) != (row == column ? 1.0 : 0.0)) {
        return false;
      }
    }
  }
  return true;
}
} // namespace

std::unique_ptr<Shape> translate(const Shape &shape, double dx, double dy, double dz,
                                 rust::Vec<HistoryEntry> &history) {
  gp_Trsf move;
  move.SetTranslation(gp_Vec(dx, dy, dz));
  // One location, the move composed with the shape's own, as FreeCAD places a shape:
  // moving it again replaces the location rather than adding to it, and a shape moved
  // back to where it was has no location at all, as it started.
  const gp_Trsf placed = move * shape.shape.Location().Transformation();
  // A move by nothing is the shape itself, with its own location, so every element is
  // kept (a new location of the same value isn't the same sub-shape to OCCT).
  const TopoDS_Shape result =
      is_identity(move)     ? shape.shape
      : is_identity(placed) ? shape.shape.Located(TopLoc_Location())
                            : shape.shape.Located(TopLoc_Location(placed));
  // The result has the same sub-shapes in the same order, each with a new location: its
  // element n is the input's element n (the same TShape, the same orientation), moved.
  // Checked rather than assumed; kept where it's the same sub-shape (a move by nothing).
  for (TopAbs_ShapeEnum type : {TopAbs_FACE, TopAbs_EDGE, TopAbs_VERTEX}) {
    const Kind kind = kind_of(type);
    ShapeMap inputs, outputs;
    TopExp::MapShapes(shape.shape, type, inputs);
    TopExp::MapShapes(result, type, outputs);
    if (inputs.Extent() != outputs.Extent()) {
      throw Standard_Failure("the moved shape doesn't have the shape's elements");
    }
    for (int i = 1; i <= inputs.Extent(); ++i) {
      const TopoDS_Shape &in = inputs(i);
      const TopoDS_Shape &out = outputs(i);
      if (!in.IsPartner(out) || in.Orientation() != out.Orientation()) {
        throw Standard_Failure("the moved shape's elements aren't the shape's, in order");
      }
      const Relation relation = in.IsSame(out) ? Relation::Kept : Relation::Modified;
      history.push_back(HistoryEntry{0, kind, static_cast<uint32_t>(i), relation, kind,
                                     static_cast<uint32_t>(i),
                                     relation == Relation::Kept ? 0u : 1u});
    }
  }
  return std::make_unique<Shape>(Shape{result});
}

std::unique_ptr<Shape> boolean(BooleanOp op, const Shape &base, const Shape &tool,
                               rust::Vec<HistoryEntry> &history,
                               rust::Vec<rust::String> &warnings) {
  std::unique_ptr<BRepAlgoAPI_BooleanOperation> maker;
  switch (op) {
  case BooleanOp::Fuse:
    maker = std::make_unique<BRepAlgoAPI_Fuse>();
    break;
  case BooleanOp::Cut:
    maker = std::make_unique<BRepAlgoAPI_Cut>();
    break;
  case BooleanOp::Common:
    maker = std::make_unique<BRepAlgoAPI_Common>();
    break;
  default:
    throw Standard_Failure("unknown boolean operation");
  }
  NCollection_List<TopoDS_Shape> arguments;
  arguments.Append(base.shape);
  NCollection_List<TopoDS_Shape> tools;
  tools.Append(tool.shape);
  maker->SetArguments(arguments);
  maker->SetTools(tools);
  // Leave the inputs' sub-shapes as they are, so history reports changed elements as
  // modified (docs/ports/booleans.md). Single-threaded, so the result is deterministic.
  maker->SetNonDestructive(true);
  maker->SetRunParallel(false);
  maker->Build();
  if (maker->HasErrors() || !maker->IsDone()) {
    std::ostringstream report;
    maker->DumpErrors(report);
    std::string message = report.str();
    while (!message.empty() && (message.back() == '\n' || message.back() == ' ')) {
      message.pop_back();
    }
    throw Standard_Failure(
        (message.empty() ? std::string("the boolean couldn't be built") : message).c_str());
  }
  for (const Handle(Message_Alert) &alert : maker->GetReport()->GetAlerts(Message_Warning)) {
    warnings.push_back(rust::String(alert->GetMessageKey()));
  }
  const TopoDS_Shape result = maker->Shape();
  capture_history(*maker, 0, base.shape, result, history);
  capture_history(*maker, 1, tool.shape, result, history);
  return std::make_unique<Shape>(Shape{result});
}

bool is_valid_where_changed(const Shape &input, const Shape &result) {
  NCollection_List<TopoDS_Shape> inputs;
  inputs.Append(input.shape);
  // closedSolid = false, GeomCtrl = false: FreeCAD's arguments since 87293fac9b.
  return BRepAlgo::IsValid(inputs, result.shape, false, false);
}

bool is_valid_where_changed_from_two(const Shape &first, const Shape &second,
                                     const Shape &result) {
  NCollection_List<TopoDS_Shape> inputs;
  inputs.Append(first.shape);
  inputs.Append(second.shape);
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

std::unique_ptr<Shape> debug_empty_shape() {
  TopoDS_Compound compound;
  BRep_Builder().MakeCompound(compound);
  return std::make_unique<Shape>(Shape{compound});
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
