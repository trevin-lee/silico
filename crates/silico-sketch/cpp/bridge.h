// The C++ side of silico-sketch's bridge: builds a PlaneGCS system from a sketch as the
// protocol describes it, diagnoses and solves it, and reads the result back. Nothing here
// decides anything: the core chooses what to send and what the result means.
#pragma once

#include <cstdint>
#include <memory>

#include "rust/cxx.h"

namespace silico_sketch {

struct SolveInput;
struct SolveOutput;
struct Pin;
struct DragDiagnosis;
struct MoveOutput;

// Diagnoses and solves `input`. C++ exceptions reach Rust as errors (cxx's Result).
SolveOutput solve(const SolveInput &input);

// A drag under way: the sketch's PlaneGCS system, kept between moves. Opaque to Rust,
// which holds it by a UniquePtr; it's never moved, since PlaneGCS points into it.
class DragSession {
 public:
  struct State;
  DragSession(const SolveInput &input, size_t pins);
  ~DragSession();
  DragSession(const DragSession &) = delete;
  DragSession &operator=(const DragSession &) = delete;

  std::unique_ptr<State> state;
};

// Builds and diagnoses `input` (the part a drag moves), and pins each of `pins` where it
// is, at its weight: a drag's start.
std::unique_ptr<DragSession> drag_begin(const SolveInput &input, rust::Slice<const Pin> pins);
// Builds and diagnoses `input`: the degrees of freedom and the constraints in conflict.
DragDiagnosis diagnose(const SolveInput &input);
// Moves the first pin's target (the dragged point's) to (x, y) and solves from the drag's
// start.
MoveOutput drag_move(DragSession &session, double x, double y);

// The trap-enable bits that are set: 0 when every floating-point trap is off.
uint32_t fp_trap_mask();
// Switches every floating-point trap off.
void disable_fp_traps();
// Fixes the cache sizes Eigen blocks its matrix products by, so they sum in the same
// order on every CPU: Eigen asks an x86 CPU for its caches, and uses fixed sizes on ARM.
void fix_eigen_cache_sizes();

#ifdef SILICO_TEST_OPS
void debug_throw_runtime_error(rust::Str message);
void debug_raise_sigsegv();
void debug_enable_fp_traps();
void debug_throw_bad_alloc();
#endif

}  // namespace silico_sketch
