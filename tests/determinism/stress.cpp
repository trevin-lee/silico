// Determinism stress probe: ~600 randomized OCCT operations from a seeded
// integer generator, each written as exact bits (or its exact error), so
// builds on different machines can be compared bit for bit.
// Usage: stress <output.json> [cases]

#include <BRepAlgoAPI_Common.hxx>
#include <BRepAlgoAPI_Cut.hxx>
#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepBndLib.hxx>
#include <BRepBuilderAPI_MakeEdge.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_MakePolygon.hxx>
#include <BRepBuilderAPI_MakeWire.hxx>
#include <BRepBuilderAPI_Transform.hxx>
#include <BRepCheck_Analyzer.hxx>
#include <BRepFilletAPI_MakeChamfer.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>
#include <BRepGProp.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRepOffsetAPI_MakeOffsetShape.hxx>
#include <BRepOffsetAPI_MakePipe.hxx>
#include <BRepOffsetAPI_MakeThickSolid.hxx>
#include <BRepOffsetAPI_ThruSections.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRepPrimAPI_MakeCone.hxx>
#include <BRepPrimAPI_MakeCylinder.hxx>
#include <BRepPrimAPI_MakePrism.hxx>
#include <BRepPrimAPI_MakeRevol.hxx>
#include <BRepPrimAPI_MakeSphere.hxx>
#include <BRepPrimAPI_MakeTorus.hxx>
#include <BRep_Tool.hxx>
#include <BinTools.hxx>
#include <Bnd_Box.hxx>
#include <GProp_GProps.hxx>
#include <GeomAPI_PointsToBSpline.hxx>
#include <Geom_BSplineCurve.hxx>
#include <Poly_Triangulation.hxx>
#include <Standard_Failure.hxx>
#include <Standard_Transient.hxx>
#include <TColgp_Array1OfPnt.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS.hxx>
#include <gp_Ax2.hxx>
#include <gp_Circ.hxx>
#include <gp_Trsf.hxx>

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <sstream>
#include <string>
#include <vector>

namespace {

// splitmix64: integer-only, so inputs are identical on every machine.
struct Rng {
  uint64_t s;
  explicit Rng(uint64_t seed) : s(seed * 0x9E3779B97F4A7C15ULL + 1) {}
  uint64_t next() {
    uint64_t z = (s += 0x9E3779B97F4A7C15ULL);
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ULL;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBULL;
    return z ^ (z >> 31);
  }
  // Uniform in [lo, hi). The integer-to-double conversion and scaling are exact IEEE ops.
  double uni(double lo, double hi) { return lo + (hi - lo) * (double(next() >> 11) * 0x1.0p-53); }
  int pick(int n) { return int(next() % uint64_t(n)); }
  gp_Pnt pnt(double r) { return gp_Pnt(uni(-r, r), uni(-r, r), uni(-r, r)); }
  gp_Dir dir() {
    for (;;) {
      double x = uni(-1, 1), y = uni(-1, 1), z = uni(-1, 1);
      if (x * x + y * y + z * z > 0.05) return gp_Dir(x, y, z);
    }
  }
  gp_Ax2 ax2(double r) {
    gp_Dir n = dir();
    gp_Dir t = dir();
    while (n.IsParallel(t, 0.1)) t = dir();
    return gp_Ax2(pnt(r), n, n.Crossed(t));
  }
};

uint64_t fnv1a(const void* data, size_t len, uint64_t h = 1469598103934665603ULL) {
  const auto* p = static_cast<const unsigned char*>(data);
  for (size_t i = 0; i < len; ++i) {
    h ^= p[i];
    h *= 1099511628211ULL;
  }
  return h;
}

std::string hex64(uint64_t u) {
  char buf[32];
  std::snprintf(buf, sizeof buf, "%016llx", static_cast<unsigned long long>(u));
  return buf;
}

std::string hexd(double d) {
  uint64_t u;
  std::memcpy(&u, &d, sizeof u);
  return hex64(u);
}

TopoDS_Shape rotated(const TopoDS_Shape& s, Rng& r) {
  gp_Trsf t;
  t.SetRotation(gp_Ax1(gp::Origin(), r.dir()), r.uni(-3.2, 3.2));  // sin/cos inside OCCT
  return BRepBuilderAPI_Transform(s, t, true).Shape();
}

TopoDS_Shape primitive(Rng& r) {
  switch (r.pick(5)) {
    case 0: return BRepPrimAPI_MakeBox(r.ax2(3), r.uni(1, 8), r.uni(1, 8), r.uni(1, 8)).Shape();
    case 1: return BRepPrimAPI_MakeCylinder(r.ax2(3), r.uni(0.5, 4), r.uni(1, 8)).Shape();
    case 2: return BRepPrimAPI_MakeSphere(r.pnt(3), r.uni(0.5, 5)).Shape();
    case 3: return BRepPrimAPI_MakeCone(r.ax2(3), r.uni(0.5, 4), r.uni(0, 2), r.uni(1, 8)).Shape();
    default: return BRepPrimAPI_MakeTorus(r.ax2(3), r.uni(3, 6), r.uni(0.3, 2)).Shape();
  }
}

TopoDS_Shape boolean(Rng& r) {
  TopoDS_Shape a = primitive(r), b = primitive(r);
  switch (r.pick(3)) {
    case 0: return BRepAlgoAPI_Fuse(a, b).Shape();
    case 1: return BRepAlgoAPI_Cut(a, b).Shape();
    default: return BRepAlgoAPI_Common(a, b).Shape();
  }
}

std::vector<TopoDS_Edge> someEdges(const TopoDS_Shape& s, Rng& r, bool skipClosed) {
  TopTools_IndexedMapOfShape edges;
  TopExp::MapShapes(s, TopAbs_EDGE, edges);
  std::vector<TopoDS_Edge> out;
  for (int i = 1; i <= edges.Extent(); ++i) {
    TopoDS_Edge e = TopoDS::Edge(edges(i));
    if (skipClosed && BRep_Tool::IsClosed(e)) continue;
    if (r.pick(3) == 0) out.push_back(e);
  }
  if (out.empty() && edges.Extent() > 0) out.push_back(TopoDS::Edge(edges(1)));
  return out;
}

TopoDS_Wire polygonWire(Rng& r, int n, double z, double rad) {
  BRepBuilderAPI_MakePolygon p;
  // Points on a jittered square/regular-ish ring without trig: use fixed unit directions.
  static const double ux[8] = {1, 0.7071067811865476, 0, -0.7071067811865476, -1, -0.7071067811865476, 0, 0.7071067811865476};
  static const double uy[8] = {0, 0.7071067811865476, 1, 0.7071067811865476, 0, -0.7071067811865476, -1, -0.7071067811865476};
  for (int i = 0; i < n; ++i) {
    int k = i * 8 / n;
    double rr = rad * r.uni(0.8, 1.2);
    p.Add(gp_Pnt(ux[k] * rr, uy[k] * rr, z));
  }
  p.Close();
  return p.Wire();
}

TopoDS_Shape makeCase(int kind, Rng& r) {
  switch (kind) {
    case 0: return boolean(r);
    case 1: return rotated(boolean(r), r);
    case 2: {
      TopoDS_Shape s = r.pick(2) ? BRepPrimAPI_MakeBox(r.ax2(2), r.uni(3, 9), r.uni(3, 9), r.uni(3, 9)).Shape() : boolean(r);
      BRepFilletAPI_MakeFillet f(s);
      for (auto& e : someEdges(s, r, true)) f.Add(r.uni(0.1, 1.2), e);
      return f.Shape();
    }
    case 3: {
      TopoDS_Shape s = r.pick(2) ? BRepPrimAPI_MakeBox(r.ax2(2), r.uni(3, 9), r.uni(3, 9), r.uni(3, 9)).Shape() : boolean(r);
      BRepFilletAPI_MakeChamfer c(s);
      for (auto& e : someEdges(s, r, true)) c.Add(r.uni(0.1, 1.0), e);
      return c.Shape();
    }
    case 4: {
      TopoDS_Shape s = r.pick(2) ? BRepPrimAPI_MakeBox(r.ax2(2), r.uni(4, 9), r.uni(4, 9), r.uni(4, 9)).Shape()
                                 : BRepPrimAPI_MakeCylinder(r.ax2(2), r.uni(2, 5), r.uni(3, 9)).Shape();
      TopTools_IndexedMapOfShape faces;
      TopExp::MapShapes(s, TopAbs_FACE, faces);
      TopTools_ListOfShape remove;
      remove.Append(faces(1 + r.pick(faces.Extent())));
      BRepOffsetAPI_MakeThickSolid t;
      t.MakeThickSolidByJoin(s, remove, -r.uni(0.2, 1.5), 1e-3);
      return t.Shape();
    }
    case 5: {
      TopoDS_Shape s = r.pick(2) ? primitive(r) : boolean(r);
      BRepOffsetAPI_MakeOffsetShape o;
      o.PerformByJoin(s, r.uni(-0.5, 1.0), 1e-3);
      return o.Shape();
    }
    case 6: {
      TopoDS_Face f = BRepBuilderAPI_MakeFace(polygonWire(r, 3 + r.pick(6), 0, r.uni(1, 3)));
      gp_Trsf t;
      t.SetTranslation(gp_Vec(r.uni(4, 6), 0, 0));
      TopoDS_Shape moved = BRepBuilderAPI_Transform(f, t, true).Shape();
      return BRepPrimAPI_MakeRevol(moved, gp_Ax1(gp::Origin(), gp::DY()), r.uni(0.3, 6.28)).Shape();
    }
    case 7: {
      int n = 5 + r.pick(5);
      TColgp_Array1OfPnt pts(1, n);
      for (int i = 1; i <= n; ++i) pts(i) = gp_Pnt(i * r.uni(1, 2), r.uni(-3, 3), 0);
      Handle(Geom_BSplineCurve) c = GeomAPI_PointsToBSpline(pts).Curve();
      TopoDS_Wire w = BRepBuilderAPI_MakeWire(BRepBuilderAPI_MakeEdge(c), BRepBuilderAPI_MakeEdge(pts(n), pts(1)));
      return BRepPrimAPI_MakePrism(BRepBuilderAPI_MakeFace(w).Face(), gp_Vec(r.uni(-1, 1), r.uni(-1, 1), r.uni(2, 6))).Shape();
    }
    case 8: {
      BRepOffsetAPI_ThruSections loft(true);
      int sections = 2 + r.pick(3);
      for (int i = 0; i < sections; ++i) {
        if (r.pick(2)) {
          gp_Circ c(gp_Ax2(gp_Pnt(r.uni(-1, 1), r.uni(-1, 1), i * r.uni(2, 4)), gp::DZ()), r.uni(1, 4));
          loft.AddWire(BRepBuilderAPI_MakeWire(BRepBuilderAPI_MakeEdge(c)));
        } else {
          loft.AddWire(polygonWire(r, 8, i * r.uni(2, 4), r.uni(1, 4)));
        }
      }
      return loft.Shape();
    }
    case 9: {
      TColgp_Array1OfPnt pts(1, 5);
      for (int i = 1; i <= 5; ++i) pts(i) = gp_Pnt(r.uni(-2, 2), r.uni(-2, 2), (i - 1) * r.uni(3, 5));
      pts(1) = gp_Pnt(0, 0, 0);
      pts(2) = gp_Pnt(0, 0, 3);  // fixed start tangent along +Z, so a profile in the XY plane is perpendicular
      Handle(Geom_BSplineCurve) c = GeomAPI_PointsToBSpline(pts).Curve();
      TopoDS_Wire spine = BRepBuilderAPI_MakeWire(BRepBuilderAPI_MakeEdge(c));
      gp_Circ prof(gp_Ax2(gp::Origin(), gp::DZ()), r.uni(0.3, 1.2));
      TopoDS_Face face = BRepBuilderAPI_MakeFace(BRepBuilderAPI_MakeWire(BRepBuilderAPI_MakeEdge(prof)));
      return BRepOffsetAPI_MakePipe(spine, face).Shape();
    }
    default: {
      TopoDS_Shape s = boolean(r);
      BRepMesh_IncrementalMesh(s, r.uni(0.01, 0.2), false, r.uni(0.1, 0.5), false);
      return s;
    }
  }
}

std::string summarize(const TopoDS_Shape& s, bool mesh) {
  std::ostringstream o;
  if (s.IsNull()) return "\"null\"";
  GProp_GProps vp, sp;
  BRepGProp::VolumeProperties(s, vp);
  BRepGProp::SurfaceProperties(s, sp);
  Bnd_Box b;
  BRepBndLib::AddOptimal(s, b, false, false);
  double v[11] = {vp.Mass(), sp.Mass(), vp.CentreOfMass().X(), vp.CentreOfMass().Y(), vp.CentreOfMass().Z()};
  if (!b.IsVoid()) b.Get(v[5], v[6], v[7], v[8], v[9], v[10]);
  TopTools_IndexedMapOfShape f, e, vt;
  TopExp::MapShapes(s, TopAbs_FACE, f);
  TopExp::MapShapes(s, TopAbs_EDGE, e);
  TopExp::MapShapes(s, TopAbs_VERTEX, vt);
  uint64_t vh = 1469598103934665603ULL;
  for (int i = 1; i <= vt.Extent(); ++i) {
    gp_Pnt p = BRep_Tool::Pnt(TopoDS::Vertex(vt(i)));
    double xyz[3] = {p.X(), p.Y(), p.Z()};
    vh = fnv1a(xyz, sizeof xyz, vh);
  }
  uint64_t mh = 1469598103934665603ULL;
  if (mesh) {
    for (TopExp_Explorer ex(s, TopAbs_FACE); ex.More(); ex.Next()) {
      TopLoc_Location loc;
      Handle(Poly_Triangulation) t = BRep_Tool::Triangulation(TopoDS::Face(ex.Current()), loc);
      if (t.IsNull()) continue;
      for (int i = 1; i <= t->NbNodes(); ++i) {
        gp_Pnt p = t->Node(i);
        double xyz[3] = {p.X(), p.Y(), p.Z()};
        mh = fnv1a(xyz, sizeof xyz, mh);
      }
    }
  }
  std::ostringstream blob;
  BinTools::Write(s, blob, mesh, false, BinTools_FormatVersion_CURRENT);
  std::string bytes = blob.str();
  o << "{";
  const char* names[11] = {"volume", "area", "cx", "cy", "cz", "bx0", "by0", "bz0", "bx1", "by1", "bz1"};
  for (int i = 0; i < 11; ++i) o << "\"" << names[i] << "\": [\"" << hexd(v[i]) << "\", " << v[i] << "], ";
  o << "\"counts\": \"" << f.Extent() << "/" << e.Extent() << "/" << vt.Extent() << "\", ";
  o << "\"valid\": " << (BRepCheck_Analyzer(s).IsValid() ? 1 : 0) << ", ";
  o << "\"vertex_coords\": \"" << hex64(vh) << "\", ";
  if (mesh) o << "\"mesh_nodes\": \"" << hex64(mh) << "\", ";
  o << "\"brep_bytes\": \"" << hex64(fnv1a(bytes.data(), bytes.size())) << "\"}";
  return o.str();
}

std::string escape(const std::string& s) {
  std::string out;
  for (char c : s) {
    if (c == '"' || c == '\\') out += '\\';
    if (c >= 32) out += c;
  }
  return out;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc < 2) {
    std::fprintf(stderr, "usage: stress <output.json> [cases]\n");
    return 2;
  }
  int cases = argc > 2 ? std::atoi(argv[2]) : 605;
  FILE* out = std::fopen(argv[1], "w");
  if (!out) return 2;
  std::fprintf(out, "{\n");
  for (int i = 0; i < cases; ++i) {
    int kind = i % 11;
    Rng r{static_cast<uint64_t>(i)};
    // A kernel worker resets serials per request, so results never depend on earlier work.
    std::string body;
    try {
      body = summarize(makeCase(kind, r), kind == 10);
    } catch (const Standard_Failure& e) {
      // The type and message as OCCT 7.9 gives them (its Standard_Failure isn't a
      // std::exception).
      const char* message = e.GetMessageString();
      body = std::string("{\"error\": \"") + escape(e.DynamicType()->Name()) + ": " +
             escape(message != nullptr ? message : "") + "\"}";
    } catch (const std::exception& e) {
      body = std::string("{\"error\": \"std: ") + escape(e.what()) + "\"}";
    }
    std::fprintf(out, "  \"case%03d_kind%02d\": %s%s\n", i, kind, body.c_str(), i + 1 == cases ? "" : ",");
  }
  std::fprintf(out, "}\n");
  std::fclose(out);
  return 0;
}
