# Roadmap

The plan is to take the biggest risks first and build headless before the UI. Agents can only test what runs headless, and the UI is where the maintainer's taste and iteration time matter most, so it's built on a core that already works. Until the app exists, tests and agents log geometry to [Rerun](https://rerun.io) as a debug viewer.

Every milestone ends with `/adversarial-review --milestone`, and the council must approve before the next milestone starts.

## Detailed plans

| Milestone | Plan | Status |
|---|---|---|
| M0 – Foundations | [docs/roadmap/m0-foundations.md](docs/roadmap/m0-foundations.md) | Detailed |
| M1 – The kernel layer | [docs/roadmap/m1-kernel.md](docs/roadmap/m1-kernel.md) | Detailed |
| M2 – Document and naming | [docs/roadmap/m2-document-naming.md](docs/roadmap/m2-document-naming.md) | Outline, detailed after the FreeCAD study |
| M3 – Sketcher | [docs/roadmap/m3-sketcher.md](docs/roadmap/m3-sketcher.md) | Outline, detailed after the FreeCAD study |
| M4, M5 | This file | Detailed when M3 ends |

**How to work through a plan:**
- **One step is one branch and one PR**, sized so the council can review it in one round.
- **Tests come first.** A step's listed tests are its specification. Write them, watch them fail, then implement. The listed tests are the minimum; `/break-it` and the council add more.
- **Expected values come from somewhere independent:** an analytic formula, a hand calculation, or the FreeCAD oracle. Never from the code under test.
- **Plans are refined one milestone at a time.** Near milestones are detailed; later ones stay as outlines until what we learn earlier can shape them. When reality differs from a plan, update the plan in the same PR and say why.

## Stage 1: the vertical slice

The goal: sketch → pad/pocket → fillet/chamfer → recompute with stable names → STEP and 3MF export, all usable from both the UI and MCP.

### M0 – Foundations
Cargo workspace. A pixi toolchain and a deterministic OCCT 8.0.1 build from source. A local gate (`pixi run gate`) that runs the workspace-lint check, cargo-deny, fmt, clippy, the tests, and a stress run on the committed tree. `tracing` set up, and the structured error type.

**Done when** one end-to-end test passes: Rust sends a request to a worker process, the worker has OCCT build a box and fillet an edge, and the volume that comes back matches FreeCAD's. This proves the process boundary, the `cxx` bridge, and the OCCT build all work together.

### M1 – The kernel layer
Worker pool. Handling for crashes, timeouts, and restarts, and discarding of stale results. Core operations that return history: primitives, extrude, revolve, booleans, fillet, chamfer. The FreeCAD oracle harness (`silico-oracle`).

**Done when** tests that `kill -9` workers mid-operation pass, and oracle tests match FreeCAD for every operation.

### M2 – Document and naming
The JSON timeline, the canonical writer, and the single parse → validate → recompute gate. DAG recompute, the per-feature cache, and fingerprints. The port of topological naming (`ElementMap` and `TopoShapeExpansion`). The CLI commands `silico check`, `silico fmt`, and `silico show`.

**Done when** golden models save and rebuild byte-identically on every platform, and references survive upstream edits or fail loudly.

### M3 – Sketcher
Can run in parallel with M2. PlaneGCS in a worker, sketch entities and constraints, profile-to-face conversion, and pad, pocket, and revolve driven by sketches.

**Done when** the ported FreeCAD sketcher tests pass, and the latency of interactive drags through the worker has been measured. That measurement settles the IPC transport question.

### M4 – MCP, before any UI
The command API exposed over MCP: transactions, geometry queries that return stable names, measurement, offscreen PNG rendering, and `read_logs`.

**Done when** an agent designs a bracket headlessly and exports a printable 3MF.

### M5 – The app
A Fusion-style layout with browser tree, timeline bar, and property panel. A viewport built on `re_renderer` with crisp edges and GPU picking. The sketch-mode UI. Behavior follows SolidWorks; visuals follow Fusion 360.

**Done when** the vertical slice works end to end in both the UI and MCP.

### After the slice: the rest of stage 1
The full feature set (holes, loft, sweep, helix, draft, shell, patterns, mirror). Multi-body booleans. Parameters and their UX. Measure. Assembly with OndselSolver. STEP import.

## Stage 2
Surface modeling, mesh editing, text-to-geometry, and DXF/SVG import into sketches.

## Stage 3
CAM, FEA, and TechDraw.
