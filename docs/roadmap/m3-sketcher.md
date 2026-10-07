# M3 – Sketcher

**Status: outline.** Detail these steps once the study note below is written. M3 can run in parallel with M2.

**Goal:** constrained 2D sketches, solved by PlaneGCS in a worker, that turn into faces and drive pad, pocket, and revolve.

**Exit tests:** the ported FreeCAD sketcher and face-maker tests pass, interactive drag latency through the worker is measured, and the council approves with `/adversarial-review --milestone`.

## Study first

**`docs/ports/sketcher.md`**, from:
- `FreeCAD/src/Mod/Sketcher/App/SketchObject.cpp`, `Sketch.cpp`, `Constraint.cpp`, `SketchAnalysis.cpp`;
- the PlaneGCS API in `FreeCAD/src/Mod/Sketcher/App/planegcs/`;
- the C++ tests in `FreeCAD/tests/src/Mod/Sketcher/App/` (including `planegcs/`);
- `FreeCAD/src/Mod/Part/App/FaceMakerBullseye.cpp` and `FaceMakerCheese.cpp`, with tests in `FreeCAD/tests/src/Mod/Part/App/FaceMakerBullseye.cpp`;
- `FreeCAD/src/Mod/PartDesign/PartDesignTests/TestSketch.py`, `TestPad.py`, `TestPocket.py`, `TestRevolve.py`.

## Steps

| Step | Builds | Key tests |
|---|---|---|
| M3.1 PlaneGCS worker | PlaneGCS and Eigen built with the deterministic recipe (`-ffp-contract=off`, `EIGEN_DONT_VECTORIZE`, the `libm` shim), a `silico-sketch` worker with a `cxx` bridge | Solves a constrained rectangle; a solver exception becomes `Err`; the worker crash and timeout suite from M0.7, reused; the math-import allowlist check passes; a randomized sketch stress corpus is bit-identical across runs and between the Mac and the workstation |
| M3.2 Sketch model | Points, lines, arcs, circles, ellipses, B-splines, and constraints, in the document schema | Schema round trip; constraints that reference missing geometry are rejected by the validation gate |
| M3.3 Solving and diagnostics | Solving, degrees of freedom, and conflicting and redundant constraint reports | Ported FreeCAD solver tests; over-constrained sketches report the exact conflicting constraint IDs |
| M3.4 Stored coordinates | Solved coordinates written to the file, re-solved on load when constraints changed | A hand edit to a constraint triggers a re-solve, and `fmt` writes back the corrected coordinates; re-solving an unchanged sketch leaves the file's bytes unchanged on every checked platform |
| M3.5 Profile to face | Port of `FaceMakerBullseye` covering nesting, holes, and islands | Ported `FaceMakerBullseye` tests; `/break-it` catalog cases for open, self-intersecting, and touching profiles |
| M3.6 Sketch-driven features | Pad, pocket, and revolve from sketches, as timeline features | Ported `TestPad`, `TestPocket`, and `TestRevolve` cases, as oracle cases with analytic volumes |
| M3.7 Drag latency | A benchmark of the solve round trip during a simulated drag | p50 and p99 latency recorded. If p99 exceeds the frame budget, decide between shared-memory IPC and an in-process solver, and record the decision in ARCHITECTURE.md |
