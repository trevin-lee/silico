// The C++ side of silico-sketch's bridge: builds a PlaneGCS system from a sketch as the
// protocol describes it, diagnoses and solves it, and reads the result back. Nothing here
// decides anything: the core chooses what to send and what the result means.
#pragma once

#include <cstdint>

#include "rust/cxx.h"

namespace silico_sketch {

struct SolveInput;
struct SolveOutput;

// Diagnoses and solves `input`. C++ exceptions reach Rust as errors (cxx's Result).
SolveOutput solve(const SolveInput &input);

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
#endif

}  // namespace silico_sketch
