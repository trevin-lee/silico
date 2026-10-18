// The C++ side of silico-sketch's bridge: builds a PlaneGCS system from a sketch as the
// protocol describes it, diagnoses and solves it, and reads the result back. Nothing here
// decides anything: the core chooses what to send and what the result means.
#pragma once

#include <cstdint>
#include <memory>

#include "rust/cxx.h"

namespace silico_sketch {

struct SolveInput;
struct Diagnosed;
struct Attempt;
struct Pin;
struct Diagnosis;
struct MoveOutput;
enum class AlgorithmIn : std::uint8_t;

// A sketch built and diagnosed once, to be solved by one algorithm after another, each from
// the values it was built with (FreeCAD's Sketch::internalSolve). Opaque to Rust, which
// holds it by a UniquePtr; it's never moved, since PlaneGCS points into it. C++ exceptions
// reach Rust as errors (cxx's Result).
class Solver {
 public:
  struct State;
  explicit Solver(const SolveInput &input);
  ~Solver();
  Solver(const Solver &) = delete;
  Solver &operator=(const Solver &) = delete;

  std::unique_ptr<State> state;
};

// Builds and diagnoses `input`.
std::unique_ptr<Solver> solver_new(const SolveInput &input);
// What the diagnosis found.
Diagnosed solver_diagnosis(const Solver &solver);
// Solves with `algorithm` from the values the sketch was built with, and gives PlaneGCS's
// status and, unless it failed, its solution; the values are then as they were built.
Attempt solver_try(Solver &solver, AlgorithmIn algorithm);
// Each of `tags`' residuals with the sketch's values set to `values`.
rust::Vec<double> solver_residuals(Solver &solver, rust::Slice<const double> values,
                                   rust::Slice<const std::uint32_t> tags);

// A drag under way: the PlaneGCS system of the part a drag moves, kept between moves. Opaque to Rust,
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
Diagnosis diagnose(const SolveInput &input);
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
rust::Vec<std::uint64_t> debug_eigen_cache_sizes();
#endif

}  // namespace silico_sketch
