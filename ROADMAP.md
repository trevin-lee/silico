# Roadmap

The plan is to take the biggest risks first and build headless before the UI. Agents can only test what runs headless, and the UI is where the maintainer's taste and iteration time matter most, so it's built on a core that already works. Until the app exists, tests and agents log geometry to [Rerun](https://rerun.io) as a debug viewer.

Every milestone ends with `/adversarial-review --milestone`, and the council must approve before the next milestone starts.

## Stage 1: the vertical slice

The goal: sketch → pad/pocket → fillet/chamfer → recompute with stable names → STEP and 3MF export, all usable from both the UI and MCP.

### M0 – Foundations
Cargo workspace. A pixi environment with OCCT 8.0.x. CI on macOS and Linux running fmt, clippy, nextest, a check that `unsafe` stays in FFI crates, and mutation testing on PRs. `tracing` set up, and the structured error type.

**Done when** one end-to-end test passes: Rust sends a request to a worker process, the worker has OCCT build a box and fillet an edge, and the volume that comes back matches FreeCAD's. This proves the process boundary, the `cxx` bridge, and the OCCT build all work together.

### M1 – The kernel layer
Worker pool. Handling for crashes, timeouts, and restarts, and discarding of stale results. Core operations that return history: primitives, extrude, revolve, booleans, fillet, chamfer. The FreeCAD oracle harness (`silico-oracle`).

**Done when** tests that `kill -9` workers mid-operation pass, and oracle tests match FreeCAD for every operation.

### M2 – Document and naming
The JSON timeline, the canonical writer, and the single parse → validate → recompute gate. DAG recompute, the per-feature cache, and fingerprints. The port of topological naming (`ElementMap` and `TopoShapeExpansion`). The CLI commands `silico check`, `silico fmt`, and `silico show`.

**Done when** golden models rebuild byte-for-byte deterministically, and references survive upstream edits or fail loudly.

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
