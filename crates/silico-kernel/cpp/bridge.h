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

// An OCCT shape, owned from Rust through a UniquePtr.
struct Shape {
  TopoDS_Shape shape;
};

std::unique_ptr<Shape> make_box(double dx, double dy, double dz);
MassProperties mass_properties(const Shape &shape);

#ifdef SILICO_TEST_OPS
// Each throws something different, to prove every branch of the trycatch above.
void debug_throw_standard_failure(rust::Str message);
void debug_throw_std_runtime_error(rust::Str message);
void debug_throw_non_exception();
#endif

} // namespace silico
