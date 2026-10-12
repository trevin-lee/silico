#include "silico-kernel/cpp/bridge.h"
#include "silico-kernel/src/lib.rs.h"

#include <BRepAdaptor_Curve.hxx>
#include <BRepAlgo.hxx>
#include <BRepAlgoAPI_Common.hxx>
#include <BRepAlgoAPI_Cut.hxx>
#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepCheck_Analyzer.hxx>
#include <BRepLib_ToolTriangulatedShape.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRepBndLib.hxx>
#include <BRepBuilderAPI_Copy.hxx>
#include <BRep_Tool.hxx>
#include <Bnd_Box.hxx>
#include <GCPnts_TangentialDeflection.hxx>
#include <Poly_Triangulation.hxx>
#include <BRepTools.hxx>
#include <BRep_Builder.hxx>
#include <BRepFilletAPI_MakeChamfer.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>
#include <BRepGProp.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRepPrimAPI_MakeCone.hxx>
#include <BRepPrimAPI_MakeCylinder.hxx>
#include <BRepPrimAPI_MakeSphere.hxx>
#include <BRepBuilderAPI_MakeEdge.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_MakePolygon.hxx>
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
#include <Geom2d_BSplineCurve.hxx>
#include <Geom2d_BezierCurve.hxx>
#include <Geom2d_Circle.hxx>
#include <Geom2d_Ellipse.hxx>
#include <Geom2d_Hyperbola.hxx>
#include <Geom2d_Line.hxx>
#include <Geom2d_OffsetCurve.hxx>
#include <Geom2d_Parabola.hxx>
#include <Geom2d_TrimmedCurve.hxx>
#include <Geom_BSplineCurve.hxx>
#include <Geom_BSplineSurface.hxx>
#include <Geom_BezierCurve.hxx>
#include <Geom_BezierSurface.hxx>
#include <Geom_Circle.hxx>
#include <Geom_ConicalSurface.hxx>
#include <Geom_CylindricalSurface.hxx>
#include <Geom_Ellipse.hxx>
#include <Geom_Hyperbola.hxx>
#include <Geom_Line.hxx>
#include <Geom_OffsetCurve.hxx>
#include <Geom_OffsetSurface.hxx>
#include <Geom_Parabola.hxx>
#include <Geom_RectangularTrimmedSurface.hxx>
#include <Geom_SphericalSurface.hxx>
#include <Geom_SurfaceOfRevolution.hxx>
#include <Geom_SweptSurface.hxx>
#include <Geom_ToroidalSurface.hxx>
#include <Geom_TrimmedCurve.hxx>
#include <BRep_TEdge.hxx>
#include <BRep_TFace.hxx>
#include <BRep_TVertex.hxx>
#include <Poly_Polygon3D.hxx>
#include <gp_Ax22d.hxx>
#include <Precision.hxx>
#include <TopExp.hxx>
#include <NCollection_IndexedMap.hxx>
#include <TopLoc_Datum3D.hxx>
#include <TopLoc_Location.hxx>
#include <TopTools_MapOfShape.hxx>
#include <TopoDS_Iterator.hxx>
#include <NCollection_List.hxx>
#include <TopTools_IndexedDataMapOfShapeListOfShape.hxx>
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
#include <unordered_set>

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

FreecadMeasures measure_as_freecad(const Shape &shape) {
  // OCCT's default integration, as FreeCAD's Shape.Volume, Area, and CenterOfMass.
  GProp_GProps volume;
  BRepGProp::VolumeProperties(shape.shape, volume);
  GProp_GProps surface;
  BRepGProp::SurfaceProperties(shape.shape, surface);
  const gp_Pnt c = volume.CentreOfMass();
  // The tight box of the shape's geometry, as Shape.optimalBoundingBox(False, False).
  // OCCT's quick box (Shape.BoundBox) bounds each surface's control points, so it moves
  // with how a surface is parametrized, which two OCCT builds can do differently.
  Bnd_Box box;
  BRepBndLib::AddOptimal(shape.shape, box, false, false);
  Point3 low{0.0, 0.0, 0.0};
  Point3 high{0.0, 0.0, 0.0};
  const bool bounded = !box.IsVoid();
  if (bounded) {
    box.Get(low.x, low.y, low.z, high.x, high.y, high.z);
  }
  return FreecadMeasures{volume.Mass(), surface.Mass(), Point3{c.X(), c.Y(), c.Z()},
                         bounded,       low,           high};
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

  ShapeMap faces;
  TopExp::MapShapes(copy, TopAbs_FACE, faces);
  for (int i = 1; i <= faces.Extent(); ++i) {
    const TopoDS_Face face = TopoDS::Face(faces(i));
    TopLoc_Location location;
    const Handle(Poly_Triangulation) mesh = BRep_Tool::Triangulation(face, location);
    if (mesh.IsNull()) {
      // Reported, with its area, for Rust to decide on.
      GProp_GProps props;
      BRepGProp::SurfaceProperties(face, props);
      out->unmeshed_faces_.push_back(static_cast<uint32_t>(i - 1));
      out->unmeshed_areas_.push_back(props.Mass());
      out->face_triangles_.push_back(0);
      continue;
    }
    const gp_Trsf transform = location.Transformation();
    const uint32_t base = static_cast<uint32_t>(out->positions_.size() / 3);
    // A reversed face's triangles wind the other way, and its normals point the other way,
    // so both still face out of the solid. (No location mirrors: `from_brep` refuses one.)
    const bool reversed = face.Orientation() == TopAbs_REVERSED;
    // Each node's normal is the face's surface normal at the node's (u, v). Where the surface
    // has none, it's the average of the node's triangles' normals. This is OCCT's own
    // ComputeNormals, which FreeCAD's display uses; it keeps them as floats. Each normal is
    // turned with the face, like its node.
    BRepLib_ToolTriangulatedShape::ComputeNormals(face, mesh);
    for (int n = 1; n <= mesh->NbNodes(); ++n) {
      const gp_Pnt p = mesh->Node(n).Transformed(transform);
      out->positions_.insert(out->positions_.end(), {p.X(), p.Y(), p.Z()});
      gp_Dir normal = mesh->Normal(n);
      if (reversed) {
        normal.Reverse();
      }
      normal.Transform(transform);
      out->normals_.insert(out->normals_.end(), {static_cast<float>(normal.X()),
                                                 static_cast<float>(normal.Y()),
                                                 static_cast<float>(normal.Z())});
    }
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
  return out;
}

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

EdgeFacts edge_facts(const Shape &shape) {
  ShapeMap edges, vertices, faces;
  TopExp::MapShapes(shape.shape, TopAbs_EDGE, edges);
  TopExp::MapShapes(shape.shape, TopAbs_VERTEX, vertices);
  TopExp::MapShapes(shape.shape, TopAbs_FACE, faces);
  TopTools_IndexedDataMapOfShapeListOfShape ancestors;
  TopExp::MapShapesAndAncestors(shape.shape, TopAbs_EDGE, TopAbs_FACE, ancestors);
  EdgeFacts facts;
  for (int i = 1; i <= edges.Extent(); ++i) {
    const TopoDS_Edge &edge = TopoDS::Edge(edges(i));
    TopoDS_Vertex first, last;
    TopExp::Vertices(edge, first, last);
    auto number = [&](const TopoDS_Vertex &v) {
      return v.IsNull() ? 0u : static_cast<uint32_t>(vertices.FindIndex(v));
    };
    bool seam = false;
    uint32_t count = 0;
    if (const TopTools_ListOfShape *on = ancestors.Seek(edge)) {
      for (const TopoDS_Shape &face : *on) {
        facts.faces.push_back(static_cast<uint32_t>(faces.FindIndex(face)));
        seam = seam || BRep_Tool::IsClosed(edge, TopoDS::Face(face));
        ++count;
      }
    }
    facts.edges.push_back(
        EdgeFact{BRep_Tool::Degenerated(edge), seam, number(first), number(last), count});
  }
  return facts;
}

struct Dressing::Impl {
  TopoDS_Shape shape;
  ShapeMap edges;
  ShapeMap faces;
  // The fillet or the chamfer: `maker` owns it, never null, and exactly one of `fillet` and
  // `chamfer` points to it too.
  std::unique_ptr<BRepFilletAPI_LocalOperation> maker;
  BRepFilletAPI_MakeFillet *fillet = nullptr;
  BRepFilletAPI_MakeChamfer *chamfer = nullptr;

  const TopoDS_Edge &edge(uint32_t n) const {
    // Rust checks the numbers; this guards the map itself.
    if (n < 1 || static_cast<int>(n) > edges.Extent()) {
      throw Standard_Failure("edge number out of range");
    }
    return TopoDS::Edge(edges(static_cast<int>(n)));
  }
  const TopoDS_Face &face(uint32_t n) const {
    if (n < 1 || static_cast<int>(n) > faces.Extent()) {
      throw Standard_Failure("face number out of range");
    }
    return TopoDS::Face(faces(static_cast<int>(n)));
  }
  BRepFilletAPI_MakeChamfer &chamfer_maker() const {
    if (chamfer == nullptr) {
      throw Standard_Failure("not a chamfer");
    }
    return *chamfer;
  }
};

Dressing::Dressing(const TopoDS_Shape &shape, Of of) : impl_(std::make_unique<Impl>()) {
  impl_->shape = shape;
  TopExp::MapShapes(shape, TopAbs_EDGE, impl_->edges);
  TopExp::MapShapes(shape, TopAbs_FACE, impl_->faces);
  if (of == Of::Chamfer) {
    auto chamfer = std::make_unique<BRepFilletAPI_MakeChamfer>(shape);
    impl_->chamfer = chamfer.get();
    impl_->maker = std::move(chamfer);
  } else {
    auto fillet = std::make_unique<BRepFilletAPI_MakeFillet>(shape);
    impl_->fillet = fillet.get();
    impl_->maker = std::move(fillet);
  }
}

Dressing::~Dressing() = default;

void Dressing::add_fillet(uint32_t edge, double radius) {
  if (impl_->fillet == nullptr) {
    throw Standard_Failure("not a fillet");
  }
  impl_->fillet->Add(radius, impl_->edge(edge));
}

void Dressing::add_distances(uint32_t edge, uint32_t face, double distance, double other) {
  impl_->chamfer_maker().Add(distance, other, impl_->edge(edge), impl_->face(face));
}

void Dressing::add_distance_angle(uint32_t edge, uint32_t face, double distance,
                                  double radians) {
  impl_->chamfer_maker().AddDA(distance, radians, impl_->edge(edge), impl_->face(face));
}

uint32_t Dressing::contour_of(uint32_t edge) const {
  return static_cast<uint32_t>(impl_->maker->Contour(impl_->edge(edge)));
}

rust::Vec<uint32_t> Dressing::contour_edges(uint32_t contour) const {
  const BRepFilletAPI_LocalOperation &maker = *impl_->maker;
  const int ic = static_cast<int>(contour);
  if (ic < 1 || ic > maker.NbContours()) {
    throw Standard_Failure("contour number out of range");
  }
  rust::Vec<uint32_t> out;
  for (int j = 1; j <= maker.NbEdges(ic); ++j) {
    out.push_back(static_cast<uint32_t>(impl_->edges.FindIndex(maker.Edge(ic, j))));
  }
  return out;
}

std::unique_ptr<Shape> Dressing::build(rust::Vec<HistoryEntry> &history) {
  BRepFilletAPI_LocalOperation &maker = *impl_->maker;
  maker.Build();
  if (!maker.IsDone()) {
    throw Standard_Failure(impl_->fillet != nullptr ? "the fillet couldn't be built"
                                                    : "the chamfer couldn't be built");
  }
  const TopoDS_Shape result = maker.Shape();
  capture_history(maker, 0, impl_->shape, result, history);
  return std::make_unique<Shape>(Shape{result});
}

std::unique_ptr<Dressing> new_fillet(const Shape &shape) {
  return std::make_unique<Dressing>(shape.shape, Dressing::Of::Fillet);
}

std::unique_ptr<Dressing> new_chamfer(const Shape &shape) {
  return std::make_unique<Dressing>(shape.shape, Dressing::Of::Chamfer);
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

std::unique_ptr<Shape> make_polygon_face(rust::Slice<const Point3> points) {
  BRepBuilderAPI_MakePolygon polygon;
  for (const Point3 &p : points) {
    polygon.Add(gp_Pnt(p.x, p.y, p.z));
  }
  polygon.Close();
  if (!polygon.IsDone()) {
    throw Standard_Failure("the profile's polygon couldn't be built");
  }
  // OnlyPlane = true: the face's surface is a plane, as the points are.
  BRepBuilderAPI_MakeFace face(polygon.Wire(), true);
  if (!face.IsDone()) {
    throw Standard_Failure("the profile's face couldn't be built");
  }
  return std::make_unique<Shape>(Shape{face.Face()});
}

namespace {
// The faces of `ends` (a sweep's FirstShape or LastShape) by their numbers in `result`,
// 0 for one the result doesn't have.
void record_ends(const TopoDS_Shape &ends, const TopoDS_Shape &result,
                 rust::Vec<uint32_t> &numbers) {
  if (ends.IsNull()) {
    return;
  }
  ShapeMap faces;
  TopExp::MapShapes(result, TopAbs_FACE, faces);
  ShapeMap seen;
  TopExp::MapShapes(ends, TopAbs_FACE, seen);
  for (int i = 1; i <= seen.Extent(); ++i) {
    numbers.push_back(static_cast<uint32_t>(faces.FindIndex(seen(i))));
  }
}
} // namespace

std::unique_ptr<Shape> extrude(const Shape &face, double dx, double dy, double dz,
                               rust::Vec<HistoryEntry> &history, rust::Vec<uint32_t> &start,
                               rust::Vec<uint32_t> &end) {
  BRepPrimAPI_MakePrism prism(face.shape, gp_Vec(dx, dy, dz));
  if (!prism.IsDone()) {
    throw Standard_Failure("the extrusion couldn't be built");
  }
  const TopoDS_Shape result = prism.Shape();
  capture_history(prism, 0, face.shape, result, history);
  record_ends(prism.FirstShape(), result, start);
  record_ends(prism.LastShape(), result, end);
  return std::make_unique<Shape>(Shape{result});
}

std::unique_ptr<Shape> revolve(const Shape &face, Point3 origin, Point3 direction,
                               double radians, rust::Vec<HistoryEntry> &history,
                               rust::Vec<uint32_t> &start, rust::Vec<uint32_t> &end) {
  const gp_Ax1 axis(gp_Pnt(origin.x, origin.y, origin.z),
                    gp_Dir(direction.x, direction.y, direction.z));
  BRepPrimAPI_MakeRevol revolution(face.shape, axis, radians);
  if (!revolution.IsDone()) {
    throw Standard_Failure("the revolve couldn't be built");
  }
  const TopoDS_Shape result = revolution.Shape();
  capture_history(revolution, 0, face.shape, result, history);
  record_ends(revolution.FirstShape(), result, start);
  record_ends(revolution.LastShape(), result, end);
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

std::unique_ptr<Shape> shape_from_text_brep(rust::Slice<const uint8_t> brep) {
  ByteStreamBuf buffer(reinterpret_cast<const char *>(brep.data()), brep.size());
  std::istream in(&buffer);
  TopoDS_Shape shape;
  BRep_Builder builder;
  BRepTools::Read(shape, in, builder);
  if (shape.IsNull()) {
    throw Standard_Failure("not a text B-rep");
  }
  return std::make_unique<Shape>(Shape{shape});
}

namespace {

// Whether every number a piece of geometry is defined by is finite: OCCT reads NaN and
// infinity from a B-rep without complaint, then measures, meshes, or operates on them
// wrongly, slowly, or with a crash.
bool finite(double v) { return std::isfinite(v); }
bool finite(const gp_XYZ &p) { return finite(p.X()) && finite(p.Y()) && finite(p.Z()); }
bool finite(const gp_Pnt &p) { return finite(p.XYZ()); }
bool finite(const gp_Dir &d) { return finite(d.XYZ()); }
bool finite(const gp_XY &p) { return finite(p.X()) && finite(p.Y()); }
bool finite(const gp_Pnt2d &p) { return finite(p.XY()); }
bool finite(const gp_Dir2d &d) { return finite(d.XY()); }
bool finite(const gp_Ax1 &a) { return finite(a.Location()) && finite(a.Direction()); }
bool finite(const gp_Ax2 &a) {
  return finite(a.Location()) && finite(a.Direction()) && finite(a.XDirection()) &&
         finite(a.YDirection());
}
bool finite(const gp_Ax3 &a) {
  return finite(a.Location()) && finite(a.Direction()) && finite(a.XDirection()) &&
         finite(a.YDirection());
}
bool finite(const gp_Ax2d &a) { return finite(a.Location()) && finite(a.Direction()); }
bool finite(const gp_Ax22d &a) {
  return finite(a.Location()) && finite(a.XDirection()) && finite(a.YDirection());
}

// A curve in space. Kinds no reader or operation makes (none Silico knows of) are taken
// as they are.
bool finite_curve(const Handle(Geom_Curve) &curve) {
  if (curve.IsNull()) {
    return true;
  }
  if (Handle(Geom_TrimmedCurve) c = Handle(Geom_TrimmedCurve)::DownCast(curve); !c.IsNull()) {
    return finite(c->FirstParameter()) && finite(c->LastParameter()) &&
           finite_curve(c->BasisCurve());
  }
  if (Handle(Geom_OffsetCurve) c = Handle(Geom_OffsetCurve)::DownCast(curve); !c.IsNull()) {
    return finite(c->Offset()) && finite(c->Direction()) && finite_curve(c->BasisCurve());
  }
  if (Handle(Geom_Line) c = Handle(Geom_Line)::DownCast(curve); !c.IsNull()) {
    return finite(c->Position());
  }
  if (Handle(Geom_Circle) c = Handle(Geom_Circle)::DownCast(curve); !c.IsNull()) {
    return finite(c->Position()) && finite(c->Radius());
  }
  if (Handle(Geom_Ellipse) c = Handle(Geom_Ellipse)::DownCast(curve); !c.IsNull()) {
    return finite(c->Position()) && finite(c->MajorRadius()) && finite(c->MinorRadius());
  }
  if (Handle(Geom_Hyperbola) c = Handle(Geom_Hyperbola)::DownCast(curve); !c.IsNull()) {
    return finite(c->Position()) && finite(c->MajorRadius()) && finite(c->MinorRadius());
  }
  if (Handle(Geom_Parabola) c = Handle(Geom_Parabola)::DownCast(curve); !c.IsNull()) {
    return finite(c->Position()) && finite(c->Focal());
  }
  if (Handle(Geom_BezierCurve) c = Handle(Geom_BezierCurve)::DownCast(curve); !c.IsNull()) {
    for (int i = 1; i <= c->NbPoles(); ++i) {
      if (!finite(c->Pole(i)) || !finite(c->Weight(i))) {
        return false;
      }
    }
    return true;
  }
  if (Handle(Geom_BSplineCurve) c = Handle(Geom_BSplineCurve)::DownCast(curve); !c.IsNull()) {
    for (int i = 1; i <= c->NbPoles(); ++i) {
      if (!finite(c->Pole(i)) || !finite(c->Weight(i))) {
        return false;
      }
    }
    for (int i = 1; i <= c->NbKnots(); ++i) {
      if (!finite(c->Knot(i))) {
        return false;
      }
    }
    return true;
  }
  return true;
}

// A curve in a surface's parameters (an edge's pcurve).
bool finite_curve(const Handle(Geom2d_Curve) &curve) {
  if (curve.IsNull()) {
    return true;
  }
  if (Handle(Geom2d_TrimmedCurve) c = Handle(Geom2d_TrimmedCurve)::DownCast(curve);
      !c.IsNull()) {
    return finite(c->FirstParameter()) && finite(c->LastParameter()) &&
           finite_curve(c->BasisCurve());
  }
  if (Handle(Geom2d_OffsetCurve) c = Handle(Geom2d_OffsetCurve)::DownCast(curve); !c.IsNull()) {
    return finite(c->Offset()) && finite_curve(c->BasisCurve());
  }
  if (Handle(Geom2d_Line) c = Handle(Geom2d_Line)::DownCast(curve); !c.IsNull()) {
    return finite(c->Position());
  }
  if (Handle(Geom2d_Circle) c = Handle(Geom2d_Circle)::DownCast(curve); !c.IsNull()) {
    return finite(c->Position()) && finite(c->Radius());
  }
  if (Handle(Geom2d_Ellipse) c = Handle(Geom2d_Ellipse)::DownCast(curve); !c.IsNull()) {
    return finite(c->Position()) && finite(c->MajorRadius()) && finite(c->MinorRadius());
  }
  if (Handle(Geom2d_Hyperbola) c = Handle(Geom2d_Hyperbola)::DownCast(curve); !c.IsNull()) {
    return finite(c->Position()) && finite(c->MajorRadius()) && finite(c->MinorRadius());
  }
  if (Handle(Geom2d_Parabola) c = Handle(Geom2d_Parabola)::DownCast(curve); !c.IsNull()) {
    return finite(c->Position()) && finite(c->Focal());
  }
  if (Handle(Geom2d_BezierCurve) c = Handle(Geom2d_BezierCurve)::DownCast(curve);
      !c.IsNull()) {
    for (int i = 1; i <= c->NbPoles(); ++i) {
      if (!finite(c->Pole(i)) || !finite(c->Weight(i))) {
        return false;
      }
    }
    return true;
  }
  if (Handle(Geom2d_BSplineCurve) c = Handle(Geom2d_BSplineCurve)::DownCast(curve);
      !c.IsNull()) {
    for (int i = 1; i <= c->NbPoles(); ++i) {
      if (!finite(c->Pole(i)) || !finite(c->Weight(i))) {
        return false;
      }
    }
    for (int i = 1; i <= c->NbKnots(); ++i) {
      if (!finite(c->Knot(i))) {
        return false;
      }
    }
    return true;
  }
  return true;
}

bool finite_surface(const Handle(Geom_Surface) &surface) {
  if (surface.IsNull()) {
    return true;
  }
  if (Handle(Geom_RectangularTrimmedSurface) s =
          Handle(Geom_RectangularTrimmedSurface)::DownCast(surface);
      !s.IsNull()) {
    double u1 = 0.0, u2 = 0.0, v1 = 0.0, v2 = 0.0;
    s->Bounds(u1, u2, v1, v2);
    return finite(u1) && finite(u2) && finite(v1) && finite(v2) &&
           finite_surface(s->BasisSurface());
  }
  if (Handle(Geom_OffsetSurface) s = Handle(Geom_OffsetSurface)::DownCast(surface);
      !s.IsNull()) {
    return finite(s->Offset()) && finite_surface(s->BasisSurface());
  }
  if (Handle(Geom_SweptSurface) s = Handle(Geom_SweptSurface)::DownCast(surface); !s.IsNull()) {
    bool ok = finite(s->Direction()) && finite_curve(s->BasisCurve());
    if (Handle(Geom_SurfaceOfRevolution) r = Handle(Geom_SurfaceOfRevolution)::DownCast(s);
        !r.IsNull()) {
      ok = ok && finite(r->Location());
    }
    return ok;
  }
  if (Handle(Geom_ElementarySurface) s = Handle(Geom_ElementarySurface)::DownCast(surface);
      !s.IsNull()) {
    bool ok = finite(s->Position());
    if (Handle(Geom_CylindricalSurface) c = Handle(Geom_CylindricalSurface)::DownCast(s);
        !c.IsNull()) {
      ok = ok && finite(c->Radius());
    } else if (Handle(Geom_ConicalSurface) c = Handle(Geom_ConicalSurface)::DownCast(s);
               !c.IsNull()) {
      ok = ok && finite(c->RefRadius()) && finite(c->SemiAngle());
    } else if (Handle(Geom_SphericalSurface) c = Handle(Geom_SphericalSurface)::DownCast(s);
               !c.IsNull()) {
      ok = ok && finite(c->Radius());
    } else if (Handle(Geom_ToroidalSurface) c = Handle(Geom_ToroidalSurface)::DownCast(s);
               !c.IsNull()) {
      ok = ok && finite(c->MajorRadius()) && finite(c->MinorRadius());
    }
    return ok;
  }
  if (Handle(Geom_BezierSurface) s = Handle(Geom_BezierSurface)::DownCast(surface);
      !s.IsNull()) {
    for (int i = 1; i <= s->NbUPoles(); ++i) {
      for (int j = 1; j <= s->NbVPoles(); ++j) {
        if (!finite(s->Pole(i, j)) || !finite(s->Weight(i, j))) {
          return false;
        }
      }
    }
    return true;
  }
  if (Handle(Geom_BSplineSurface) s = Handle(Geom_BSplineSurface)::DownCast(surface);
      !s.IsNull()) {
    for (int i = 1; i <= s->NbUPoles(); ++i) {
      for (int j = 1; j <= s->NbVPoles(); ++j) {
        if (!finite(s->Pole(i, j)) || !finite(s->Weight(i, j))) {
          return false;
        }
      }
    }
    for (int i = 1; i <= s->NbUKnots(); ++i) {
      if (!finite(s->UKnot(i))) {
        return false;
      }
    }
    for (int i = 1; i <= s->NbVKnots(); ++i) {
      if (!finite(s->VKnot(i))) {
        return false;
      }
    }
    return true;
  }
  return true;
}

// A location's links, each raised to its power: values that aren't finite, or a mirror or
// scale (OCCT's TopoDS_Shape::validateTransformation test, which OCCT makes only when
// asked).
BrepDefect location_defect(const TopLoc_Location &location) {
  for (TopLoc_Location l = location; !l.IsIdentity(); l = l.NextLocation()) {
    const gp_Trsf t = l.FirstDatum()->Transformation().Powered(l.FirstPower());
    bool ok = finite(t.ScaleFactor());
    for (int row = 1; row <= 3; ++row) {
      for (int column = 1; column <= 4; ++column) {
        ok = ok && finite(t.Value(row, column));
      }
    }
    if (!ok) {
      return BrepDefect::NotFinite;
    }
    if (Abs(Abs(t.ScaleFactor()) - 1.) > TopLoc_Location::ScalePrec() || t.IsNegative()) {
      return BrepDefect::MirrorOrScale;
    }
  }
  return BrepDefect::None;
}


bool finite_geometry(const Handle(Geom_Curve) &c) { return finite_curve(c); }
bool finite_geometry(const Handle(Geom2d_Curve) &c) { return finite_curve(c); }
bool finite_geometry(const Handle(Geom_Surface) &s) { return finite_surface(s); }

// A sub-shape's own tolerance: BRep_Tool::Tolerance gives at least Precision::Confusion,
// which hides a NaN (NaN compares false), though OCCT's algorithms read the raw value.
template <typename TShape> bool finite_tolerance(const TopoDS_Shape &s) {
  const Handle(TShape) t = Handle(TShape)::DownCast(s.TShape());
  return t.IsNull() || finite(t->Tolerance());
}

// An edge's tolerance, its curve in space (and range), and its stored polygon, each with
// the location it's placed by, which may no more mirror or scale than a shape's.
// The curves and surfaces already found finite in one walk: one may be shared by many
// edges or faces, and is checked once.
using Checked = std::unordered_set<const Standard_Transient *>;

template <typename Geometry>
bool finite_once(Checked &checked, const Handle(Geometry) &geometry) {
  if (checked.count(geometry.get()) != 0) {
    return true;
  }
  const bool ok = finite_geometry(geometry);
  if (ok) {
    checked.insert(geometry.get());
  }
  return ok;
}

BrepDefect edge_defect(const TopoDS_Edge &edge, Checked &checked) {
  if (!finite_tolerance<BRep_TEdge>(edge)) {
    return BrepDefect::CurveNotFinite;
  }
  TopLoc_Location location;
  double first = 0.0, last = 0.0;
  const Handle(Geom_Curve) curve = BRep_Tool::Curve(edge, location, first, last);
  if (!curve.IsNull()) {
    if (const BrepDefect d = location_defect(location); d != BrepDefect::None) {
      return d;
    }
    if (!(finite(first) && finite(last) && finite_once(checked, curve))) {
      return BrepDefect::CurveNotFinite;
    }
  }
  const Handle(Poly_Polygon3D) polygon = BRep_Tool::Polygon3D(edge, location);
  if (!polygon.IsNull()) {
    if (const BrepDefect d = location_defect(location); d != BrepDefect::None) {
      return d;
    }
    for (int i = 1; i <= polygon->NbNodes(); ++i) {
      if (!finite(polygon->Nodes().Value(i))) {
        return BrepDefect::CurveNotFinite;
      }
    }
  }
  return BrepDefect::None;
}

// A face's surface, tolerance, stored mesh, and its edges' curves on it.
BrepDefect face_defect(const TopoDS_Face &face, Checked &checked) {
  TopLoc_Location location;
  const Handle(Geom_Surface) surface = BRep_Tool::Surface(face, location);
  if (surface.IsNull()) {
    return BrepDefect::FaceWithoutSurface;
  }
  if (const BrepDefect d = location_defect(location); d != BrepDefect::None) {
    return d;
  }
  if (!finite_tolerance<BRep_TFace>(face) || !finite_once(checked, surface)) {
    return BrepDefect::SurfaceNotFinite;
  }
  const Handle(Poly_Triangulation) mesh = BRep_Tool::Triangulation(face, location);
  if (!mesh.IsNull()) {
    if (const BrepDefect d = location_defect(location); d != BrepDefect::None) {
      return d;
    }
    for (int i = 1; i <= mesh->NbNodes(); ++i) {
      if (!finite(mesh->Node(i)) || (mesh->HasUVNodes() && !finite(mesh->UVNode(i)))) {
        return BrepDefect::SurfaceNotFinite;
      }
    }
  }
  // Each pcurve: an edge on a seam is there twice, once each way round.
  for (TopExp_Explorer it(face, TopAbs_EDGE); it.More(); it.Next()) {
    double first = 0.0, last = 0.0;
    const Handle(Geom2d_Curve) pcurve =
        BRep_Tool::CurveOnSurface(TopoDS::Edge(it.Current()), face, first, last);
    if (!pcurve.IsNull() && !(finite(first) && finite(last) && finite_once(checked, pcurve))) {
      return BrepDefect::CurveNotFinite;
    }
  }
  return BrepDefect::None;
}

} // namespace

BrepDefect brep_defect(const Shape &shape) {
  // Every location in the shape's tree, and on its geometry (none may mirror or scale, or
  // hold a value that isn't finite); every vertex's point and
  // tolerance; every edge's curve, range, polygon, and tolerance; every face's surface
  // (a face of only a triangulation, as OCCT's STEP and glTF readers make, has none: OCCT
  // measures and meshes it, but a boolean on it segfaults), mesh, tolerance, and curves
  // on it. Each shared sub-shape is visited once.
  TopTools_MapOfShape seen;
  Checked checked;
  std::vector<TopoDS_Shape> todo{shape.shape};
  while (!todo.empty()) {
    const TopoDS_Shape s = todo.back();
    todo.pop_back();
    const BrepDefect defect = location_defect(s.Location());
    if (defect != BrepDefect::None) {
      return defect;
    }
    switch (s.ShapeType()) {
    case TopAbs_VERTEX: {
      const TopoDS_Vertex &v = TopoDS::Vertex(s);
      if (!finite(BRep_Tool::Pnt(v)) || !finite_tolerance<BRep_TVertex>(v)) {
        return BrepDefect::PointNotFinite;
      }
      break;
    }
    case TopAbs_EDGE:
      if (const BrepDefect d = edge_defect(TopoDS::Edge(s), checked); d != BrepDefect::None) {
        return d;
      }
      break;
    case TopAbs_FACE:
      if (const BrepDefect d = face_defect(TopoDS::Face(s), checked); d != BrepDefect::None) {
        return d;
      }
      break;
    default:
      break;
    }
    for (TopoDS_Iterator it(s, false, false); it.More(); it.Next()) {
      if (seen.Add(it.Value())) {
        todo.push_back(it.Value());
      }
    }
  }
  return BrepDefect::None;
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

std::unique_ptr<Shape> debug_box_with_loose_edge() {
  BRep_Builder builder;
  TopoDS_Compound compound;
  builder.MakeCompound(compound);
  builder.Add(compound, BRepPrimAPI_MakeBox(10, 20, 30).Shape());
  builder.Add(compound, BRepBuilderAPI_MakeEdge(gp_Pnt(20, 0, 0), gp_Pnt(30, 0, 0)).Edge());
  return std::make_unique<Shape>(Shape{compound});
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
