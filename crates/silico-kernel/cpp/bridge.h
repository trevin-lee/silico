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

struct MassProperties;
struct ShapeSummary;

// An OCCT shape, owned from Rust through a UniquePtr.
struct Shape {
  TopoDS_Shape shape;
};

std::unique_ptr<Shape> make_box(double dx, double dy, double dz);
MassProperties mass_properties(const Shape &shape);

// Volume, area, center of mass, the numbers of distinct faces, edges, and vertices, and
// whether BRepCheck_Analyzer finds the shape valid.
ShapeSummary summarize(const Shape &shape);

// The number of distinct edges, which TopExp::MapShapes numbers from 1.
uint32_t edge_count(const Shape &shape);

// Rounds the given edges (numbered as edge_count's) with a constant radius.
std::unique_ptr<Shape> fillet(const Shape &shape, rust::Slice<const uint32_t> edges,
                              double radius);

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
#endif

} // namespace silico
