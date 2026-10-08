// Determinism probe: runs a fixed corpus of OCCT operations and writes every
// numeric result as exact bits, so two builds can be compared bit for bit.
// Usage: probe <output.json>

#include <BRepAlgoAPI_Common.hxx>
#include <BRepAlgoAPI_Cut.hxx>
#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepBuilderAPI_MakeEdge.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_MakeWire.hxx>
#include <BRepCheck_Analyzer.hxx>
#include <BRepFilletAPI_MakeChamfer.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>
#include <BRepGProp.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRepOffsetAPI_MakeThickSolid.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRepPrimAPI_MakeCone.hxx>
#include <BRepPrimAPI_MakeCylinder.hxx>
#include <BRepPrimAPI_MakePrism.hxx>
#include <BRepPrimAPI_MakeRevol.hxx>
#include <BRepPrimAPI_MakeSphere.hxx>
#include <BRepPrimAPI_MakeTorus.hxx>
#include <BRepBndLib.hxx>
#include <BRep_Tool.hxx>
#include <BinTools.hxx>
#include <Bnd_Box.hxx>
#include <GProp_GProps.hxx>
#include <GeomAPI_PointsToBSpline.hxx>
#include <Geom_BSplineCurve.hxx>
#include <Poly_Triangulation.hxx>
#include <Standard_Failure.hxx>
#include <TColgp_Array1OfPnt.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <gp_Ax1.hxx>
#include <gp_Ax2.hxx>
#include <gp_Pnt.hxx>
#include <gp_Vec.hxx>

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <functional>
#include <sstream>
#include <string>
#include <vector>

namespace {

uint64_t fnv1a(const void* data, size_t len, uint64_t h = 1469598103934665603ULL) {
  const auto* p = static_cast<const unsigned char*>(data);
  for (size_t i = 0; i < len; ++i) {
    h ^= p[i];
    h *= 1099511628211ULL;
  }
  return h;
}

uint64_t bits(double d) {
  uint64_t u;
  std::memcpy(&u, &d, sizeof u);
  return u;
}

std::string hexd(double d) {
  char buf[64];
  std::snprintf(buf, sizeof buf, "\"%016llx\"", static_cast<unsigned long long>(bits(d)));
  return buf;
}

std::string hexu(uint64_t u) {
  char buf[64];
  std::snprintf(buf, sizeof buf, "\"%016llx\"", static_cast<unsigned long long>(u));
  return buf;
}

// Every result is a flat list of named doubles plus named hashes.
struct Record {
  std::vector<std::pair<std::string, double>> values;
  std::vector<std::pair<std::string, uint64_t>> hashes;
  std::string error;
};

Record summarize(const TopoDS_Shape& s) {
  Record r;
  GProp_GProps vp, sp;
  BRepGProp::VolumeProperties(s, vp);
  BRepGProp::SurfaceProperties(s, sp);
  r.values.push_back({"volume", vp.Mass()});
  r.values.push_back({"area", sp.Mass()});
  gp_Pnt c = vp.CentreOfMass();
  r.values.push_back({"cx", c.X()});
  r.values.push_back({"cy", c.Y()});
  r.values.push_back({"cz", c.Z()});
  Bnd_Box b;
  BRepBndLib::AddOptimal(s, b, false, false);
  double x0, y0, z0, x1, y1, z1;
  b.Get(x0, y0, z0, x1, y1, z1);
  for (auto [n, v] : {std::pair{"bx0", x0}, {"by0", y0}, {"bz0", z0}, {"bx1", x1}, {"by1", y1}, {"bz1", z1}})
    r.values.push_back({n, v});

  TopTools_IndexedMapOfShape faces, edges, verts;
  TopExp::MapShapes(s, TopAbs_FACE, faces);
  TopExp::MapShapes(s, TopAbs_EDGE, edges);
  TopExp::MapShapes(s, TopAbs_VERTEX, verts);
  r.values.push_back({"faces", double(faces.Extent())});
  r.values.push_back({"edges", double(edges.Extent())});
  r.values.push_back({"vertices", double(verts.Extent())});
  r.values.push_back({"valid", BRepCheck_Analyzer(s).IsValid() ? 1.0 : 0.0});

  uint64_t vh = 1469598103934665603ULL;
  for (int i = 1; i <= verts.Extent(); ++i) {
    gp_Pnt p = BRep_Tool::Pnt(TopoDS::Vertex(verts(i)));
    double xyz[3] = {p.X(), p.Y(), p.Z()};
    vh = fnv1a(xyz, sizeof xyz, vh);
  }
  r.hashes.push_back({"vertex_coords", vh});

  std::ostringstream os;
  BinTools::Write(s, os);
  std::string blob = os.str();
  r.hashes.push_back({"brep_bytes", fnv1a(blob.data(), blob.size())});
  return r;
}

TopoDS_Edge edgeAt(const TopoDS_Shape& s, int index) {
  TopTools_IndexedMapOfShape edges;
  TopExp::MapShapes(s, TopAbs_EDGE, edges);
  return TopoDS::Edge(edges(index));
}

TopoDS_Face rectFace(double x0, double y0, double x1, double y1) {
  BRepBuilderAPI_MakeWire w;
  gp_Pnt p[4] = {{x0, 0, y0}, {x1, 0, y0}, {x1, 0, y1}, {x0, 0, y1}};
  for (int i = 0; i < 4; ++i) w.Add(BRepBuilderAPI_MakeEdge(p[i], p[(i + 1) % 4]));
  return BRepBuilderAPI_MakeFace(w.Wire());
}

std::vector<std::pair<std::string, std::function<TopoDS_Shape()>>> corpus() {
  return {
      {"box_fillet_one_edge", [] {
         TopoDS_Shape box = BRepPrimAPI_MakeBox(10, 20, 30);
         BRepFilletAPI_MakeFillet f(box);
         f.Add(2.0, edgeAt(box, 1));
         return f.Shape();
       }},
      {"box_fillet_all_edges", [] {
         TopoDS_Shape box = BRepPrimAPI_MakeBox(10, 20, 30);
         BRepFilletAPI_MakeFillet f(box);
         for (TopExp_Explorer e(box, TopAbs_EDGE); e.More(); e.Next()) f.Add(1.5, TopoDS::Edge(e.Current()));
         return f.Shape();
       }},
      {"box_chamfer_edge", [] {
         TopoDS_Shape box = BRepPrimAPI_MakeBox(10, 20, 30);
         BRepFilletAPI_MakeChamfer c(box);
         c.Add(2.0, edgeAt(box, 1));
         return c.Shape();
       }},
      {"box_minus_cylinder", [] {
         TopoDS_Shape box = BRepPrimAPI_MakeBox(10, 10, 10);
         TopoDS_Shape cyl = BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(5, 5, -1), gp::DZ()), 2, 12);
         return BRepAlgoAPI_Cut(box, cyl).Shape();
       }},
      {"cylinder_fuse_sphere", [] {
         TopoDS_Shape cyl = BRepPrimAPI_MakeCylinder(5, 10);
         TopoDS_Shape sph = BRepPrimAPI_MakeSphere(gp_Pnt(1.3, 0.7, 10), 4.2);
         return BRepAlgoAPI_Fuse(cyl, sph).Shape();
       }},
      {"torus_cut_box", [] {
         TopoDS_Shape tor = BRepPrimAPI_MakeTorus(10, 2);
         TopoDS_Shape box = BRepPrimAPI_MakeBox(gp_Pnt(0.3, -15, -5), 20, 30, 10);
         return BRepAlgoAPI_Cut(tor, box).Shape();
       }},
      {"sphere_common_cone", [] {
         TopoDS_Shape sph = BRepPrimAPI_MakeSphere(5);
         TopoDS_Shape cone = BRepPrimAPI_MakeCone(gp_Ax2(gp_Pnt(0.4, 0.2, -6), gp::DZ()), 4, 0.5, 12);
         return BRepAlgoAPI_Common(sph, cone).Shape();
       }},
      {"fillet_curved_seam", [] {
         TopoDS_Shape cyl = BRepPrimAPI_MakeCylinder(5, 10);
         TopoDS_Shape box = BRepPrimAPI_MakeBox(gp_Pnt(-2, -2, 4), 9, 4, 8);
         TopoDS_Shape fused = BRepAlgoAPI_Fuse(cyl, box).Shape();
         BRepFilletAPI_MakeFillet f(fused);
         TopTools_IndexedMapOfShape edges;
         TopExp::MapShapes(fused, TopAbs_EDGE, edges);
         for (int i = 1; i <= edges.Extent(); ++i) {
           TopoDS_Edge e = TopoDS::Edge(edges(i));
           if (BRep_Tool::IsClosed(e)) continue;
           f.Add(0.4, e);
         }
         return f.Shape();
       }},
      {"revolve_270", [] {
         return BRepPrimAPI_MakeRevol(rectFace(3, 0, 5, 10), gp_Ax1(gp::Origin(), gp::DZ()), 1.5 * M_PI).Shape();
       }},
      {"bspline_prism", [] {
         // Fixed inputs: computing them with libm would mix libm differences into this case.
         const double ys[7] = {2.43, 3.03, 1.32, -1.37, -3.03, -2.39, 0.18};
         TColgp_Array1OfPnt pts(1, 7);
         for (int i = 1; i <= 7; ++i) pts(i) = gp_Pnt(i * 1.7, ys[i - 1], 0);
         Handle(Geom_BSplineCurve) crv = GeomAPI_PointsToBSpline(pts).Curve();
         TopoDS_Edge e1 = BRepBuilderAPI_MakeEdge(crv);
         TopoDS_Edge e2 = BRepBuilderAPI_MakeEdge(pts(7), pts(1));
         TopoDS_Face f = BRepBuilderAPI_MakeFace(BRepBuilderAPI_MakeWire(e1, e2));
         return BRepPrimAPI_MakePrism(f, gp_Vec(0, 0, 4.3)).Shape();
       }},
      {"box_shell", [] {
         TopoDS_Shape box = BRepPrimAPI_MakeBox(10, 20, 30);
         TopTools_ListOfShape remove;
         TopExp_Explorer ex(box, TopAbs_FACE);
         remove.Append(ex.Current());
         BRepOffsetAPI_MakeThickSolid s;
         s.MakeThickSolidByJoin(box, remove, -1.0, 1e-3);
         return s.Shape();
       }},
  };
}

void writeRecord(FILE* out, const std::string& name, const Record& r, bool last) {
  std::fprintf(out, "  \"%s\": {", name.c_str());
  if (!r.error.empty()) {
    std::fprintf(out, "\"error\": \"%s\"", r.error.c_str());
  } else {
    bool first = true;
    for (auto& [k, v] : r.values) {
      std::fprintf(out, "%s\"%s\": [%s, %.17g]", first ? "" : ", ", k.c_str(), hexd(v).c_str(), v);
      first = false;
    }
    for (auto& [k, h] : r.hashes) {
      std::fprintf(out, "%s\"%s\": %s", first ? "" : ", ", k.c_str(), hexu(h).c_str());
      first = false;
    }
  }
  std::fprintf(out, "}%s\n", last ? "" : ",");
}

Record meshRecord() {
  Record r;
  TopoDS_Shape box = BRepPrimAPI_MakeBox(10, 20, 30);
  BRepFilletAPI_MakeFillet f(box);
  for (TopExp_Explorer e(box, TopAbs_EDGE); e.More(); e.Next()) f.Add(1.5, TopoDS::Edge(e.Current()));
  TopoDS_Shape s = f.Shape();
  BRepMesh_IncrementalMesh mesh(s, 0.01, false, 0.1, false);
  uint64_t h = 1469598103934665603ULL;
  int nodes = 0, tris = 0;
  for (TopExp_Explorer e(s, TopAbs_FACE); e.More(); e.Next()) {
    TopLoc_Location loc;
    Handle(Poly_Triangulation) t = BRep_Tool::Triangulation(TopoDS::Face(e.Current()), loc);
    if (t.IsNull()) continue;
    nodes += t->NbNodes();
    tris += t->NbTriangles();
    for (int i = 1; i <= t->NbNodes(); ++i) {
      gp_Pnt p = t->Node(i);
      double xyz[3] = {p.X(), p.Y(), p.Z()};
      h = fnv1a(xyz, sizeof xyz, h);
    }
  }
  r.values.push_back({"nodes", double(nodes)});
  r.values.push_back({"triangles", double(tris)});
  r.hashes.push_back({"node_coords", h});
  return r;
}

// Direct libm comparison: outputs of each function over a fixed input set.
std::vector<std::pair<std::string, Record>> libmRecords() {
  std::vector<std::pair<std::string, std::function<double(double)>>> fns = {
      {"sin", [](double x) { return std::sin(x); }},
      {"cos", [](double x) { return std::cos(x); }},
      {"tan", [](double x) { return std::tan(x); }},
      {"atan", [](double x) { return std::atan(x); }},
      {"exp", [](double x) { return std::exp(x * 0.01); }},
      {"log", [](double x) { return std::log(std::fabs(x) + 1e-3); }},
      {"pow", [](double x) { return std::pow(std::fabs(x) + 0.5, 1.37); }},
      {"atan2", [](double x) { return std::atan2(x, 1.0 - x * 0.3); }},
      {"acos", [](double x) { return std::acos(std::fmod(x, 1.0)); }},
      {"cbrt", [](double x) { return std::cbrt(x); }},
  };
  std::vector<std::pair<std::string, Record>> out;
  for (auto& [name, fn] : fns) {
    Record r;
    uint64_t h = 1469598103934665603ULL;
    for (int i = 0; i < 100000; ++i) {
      double x = -50.0 + i * 0.001000037;
      double y = fn(x);
      h = fnv1a(&y, sizeof y, h);
    }
    r.hashes.push_back({"outputs", h});
    out.push_back({"libm_" + name, r});
  }
  return out;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 2) {
    std::fprintf(stderr, "usage: probe <output.json>\n");
    return 2;
  }
  FILE* out = std::fopen(argv[1], "w");
  if (!out) return 2;
  std::vector<std::pair<std::string, Record>> all;
  for (auto& [name, build] : corpus()) {
    Record r;
    try {
      r = summarize(build());
    } catch (const Standard_Failure& e) {
      r.error = std::string("Standard_Failure: ") + (e.GetMessageString() ? e.GetMessageString() : "");
    } catch (const std::exception& e) {
      r.error = e.what();
    }
    all.push_back({name, r});
  }
  try {
    all.push_back({"mesh_filleted_box", meshRecord()});
  } catch (...) {
    Record r;
    r.error = "mesh failed";
    all.push_back({"mesh_filleted_box", r});
  }
  for (auto& lr : libmRecords()) all.push_back(lr);
  std::fprintf(out, "{\n");
  for (size_t i = 0; i < all.size(); ++i) writeRecord(out, all[i].first, all[i].second, i + 1 == all.size());
  std::fprintf(out, "}\n");
  std::fclose(out);
  return 0;
}
