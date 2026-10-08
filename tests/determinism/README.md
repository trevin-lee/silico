# Determinism tests

Can OCCT produce bit-identical results on every machine? Silico stores models as feature timelines and rebuilds them, so it needs to. An experiment in October 2026 answered yes, with the recipe now in ARCHITECTURE.md's *Deterministic kernel* and built by `third_party/occt/build.sh`. Its stress test is `pixi run stress`: run it on every OCCT upgrade, every patch change, and every toolchain change. `stress-reference.json` is the reference every run must match byte for byte; it's protected (CLAUDE.md), and changes only deliberately, when a kernel change moves results.

## What it measures

- `probe.cpp`: 12 fixed operations plus direct tests of 10 math library functions.
- `stress.cpp`: 605 randomized operations across 11 kinds: booleans of curved and rotated primitives, fillets, chamfers, shells, offsets, revolves, B-spline prisms, lofts, pipe sweeps, and meshes. Inputs come from an integer-only seeded generator, so they're identical everywhere without touching a math library.

Every result is written as exact bits: volume, area, centre of mass, bounding box, topology counts, validity, a hash of all vertex coordinates, and a hash of the `BinTools` dump. Failures are compared by exception type and message. `compare-stress.py` compares two outputs case by case.

## Findings

Each row adds to the one above it.

| OCCT build | Stress test: 10 runs on one Mac | macOS ARM vs x86 |
|---|---|---|
| Stock 8.0.1 (conda-forge) | not measured | probe: 6 of 22 |
| + built from source with `-ffp-contract=off` | not measured | probe: 8 of 22 |
| + Rust `libm` shim in place of the platform math library | 0 of 9 matched the first | probe: 22 of 22; stress: 573 of 605 |
| + upstream PR #1600 (the adopted recipe) | **9 of 9 matched the first** | **stress: 605 of 605** |

The adopted recipe was also built on Linux x86 (Ryzen 9950X3D, Debian 13, clang 21.1.8 from pixi): 3 of 3 repeat runs identical and **605 of 605 identical to macOS ARM**. The committed `stress-reference.json` is byte-identical to the Linux output. The same 47 cases fail on every build, with identical error messages, including genuinely stock conda-forge OCCT 8.0.1, so the recipe changes no outcomes.

What caused each difference:

1. **Fused multiply-add.** Compilers fuse `a*b+c` on ARM but not on generic x86. `-ffp-contract=off` stops it.
2. **Platform math libraries.** Apple's ARM and x86 `sin`, `exp`, `pow` and others disagree in the last bit, and so do macOS and glibc. The shim (now `third_party/occt/libm-shim/`) replaces them with the pure-Rust `libm` crate, which is the same code everywhere. `third_party/occt/scripts/check-math-imports.sh` fails if any library still imports an inexact platform math function. It's an allowlist, because compilers introduce calls nobody wrote (`pow(10, x)` becomes `__exp10` on macOS).
3. **Address-dependent iteration.** `BRepOffset_MakeOffset::BuildOffsetByArc` iterated a map hashed by `TShape` address, so arc-join offsets changed from run to run even on one machine. Upstream PR #1600 fixes it, and Silico carries it as `third_party/occt/patches/0001`.
4. **Build-time folding**, found in the M0.2 review. When a math call's argument is constant, clang computes the result while compiling, using the build machine's own math library: the shim is bypassed and no import is left for the check to see. A reviewer showed `pow(10, -20)` giving different bits folded and at run time. `-fmath-errno` with `-fno-builtin-<fn>` for each inexact function stops it (the exact recipe is in ARCHITECTURE.md), and `third_party/occt/scripts/fold-canary.sh` fails the build if folding ever returns. No OCCT call site was diverging yet.

The recipe's cost against stock OCCT on the stress test is recorded in ARCHITECTURE.md (about 6% after the M0.2 review; the first version of the recipe cost about 15%).

## Diagnostics

`diagnostics/occt-serial-hash.patch` hashes every handle and shape by a creation-order serial number instead of an address. It was the experiment's first fix for finding 3 and isn't used in builds. It's kept for a planned *scrambled hash* sweep: build OCCT with a deliberately different hash and rerun the stress test, so any other algorithm that depends on hash order shows up.

Developing it surfaced an undocumented OCCT behaviour. Upstream's `std::hash<TopoDS_Shape>` for located shapes hashes the `TShape`'s first bytes (its vtable pointer) rather than its address. `BRepTools_Quilt` (used by revolves, prisms and pipes) depends on that, and fails with `Standard_NoSuchObject` if located shapes are hashed by `TShape` identity. The diagnostic patch keeps upstream's behaviour for located shapes. The root cause is still open.

## Rerunning

```bash
pixi run stress    # builds OCCT if needed, then 10 runs, each byte-identical to the reference
```

This works on macOS ARM and Linux x86. For a run-by-run comparison, use `compare-stress.py <reference> <run>`. `probe.cpp` is the original 12-case probe (compare its outputs with `compare.py`), and `diagnostics/` holds the serial-hash patch.

The experiment's original one-off scripts (macOS x86 builds under Rosetta, a self-contained Linux runner, the first `detmath` shim) are in git history, under `experiments/deterministic-kernel/`. They were removed once `third_party/occt/` replaced them, so there's only one way to build OCCT.

## Not covered

- The corpus is 605 cases, not all of OCCT. The math-import check covers code paths it doesn't reach.
- PlaneGCS, OndselSolver and the `cxx` shim get the same treatment and their own stress tests when they're built (M3 and later).
- Windows is untested.
