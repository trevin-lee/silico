#include "silico-kernel/cpp/bridge.h"
#include "silico-kernel/src/lib.rs.h"

#include <BRepGProp.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BinTools.hxx>
#include <GProp_GProps.hxx>
#include <OSD.hxx>
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

void debug_raise_sigsegv() {
  // Rust's runtime handles SIGSEGV (to report stack overflows): its handler restores the
  // default action and returns, so a real fault faults again and kills the process. A
  // raised signal doesn't repeat, so restore the default first, as that second fault would.
  std::signal(SIGSEGV, SIG_DFL);
  std::raise(SIGSEGV);
}
#endif

} // namespace silico
