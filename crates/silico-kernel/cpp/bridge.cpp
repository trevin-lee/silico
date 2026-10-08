#include "silico-kernel/cpp/bridge.h"
#include "silico-kernel/src/lib.rs.h"

#include <BRepCheck_Analyzer.hxx>
#include <BRep_Builder.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>
#include <BRepGProp.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BinTools.hxx>
#include <GProp_GProps.hxx>
#include <OSD.hxx>
#include <Precision.hxx>
#include <ShapeFix_ShapeTolerance.hxx>
#include <TopExp.hxx>
#include <NCollection_IndexedMap.hxx>
#include <TopTools_ShapeMapHasher.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Shell.hxx>
#include <TopoDS_Solid.hxx>
#include <gp_Pnt.hxx>

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
                      BRepCheck_Analyzer(shape.shape).IsValid()};
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
  // OCCT can report a fillet done that isn't a valid solid (all of a 10 x 20 x 30 box's
  // edges at r = 5.5 gives volume 7792). FreeCAD refused those from 87293fac9b (2016),
  // first trying to repair tolerances as in 9ff76fc813 (2017), and later lost the refusal;
  // Silico keeps it. See docs/ports/fillet.md.
  TopoDS_Shape result = make.Shape();
  if (!BRepCheck_Analyzer(result).IsValid()) {
    ShapeFix_ShapeTolerance tolerance;
    tolerance.LimitTolerance(result, Precision::Confusion(), Precision::Confusion(),
                             TopAbs_SHAPE);
    if (!BRepCheck_Analyzer(result).IsValid()) {
      throw Standard_Failure("the fillet's result isn't a valid solid");
    }
  }
  return std::make_unique<Shape>(Shape{result});
}

// Returned whole, so Rust copies it once (copying byte by byte across the bridge costs
// about 16 times more).
std::unique_ptr<std::string> shape_to_brep(const Shape &shape) {
  std::ostringstream out(std::ios::binary);
  BinTools::Write(shape.shape, out);
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
