# Roadmap

The plan is to take the biggest risks first and build headless before the UI. Agents can only test what runs headless, and the UI is where the maintainer's taste and iteration time matter most, so it's built on a core that already works. Until the app exists, tests and agents log geometry to [Rerun](https://rerun.io) as a debug viewer.

Every milestone ends with `/adversarial-review --milestone`, and the council must approve before the next milestone starts.

## Detailed plans

| Milestone | Plan | Status |
|---|---|---|
| M0 – Foundations | [docs/roadmap/m0-foundations.md](docs/roadmap/m0-foundations.md) | Detailed |
| M1 – The kernel layer | [docs/roadmap/m1-kernel.md](docs/roadmap/m1-kernel.md) | Detailed |
| M2 – Document and naming | [docs/roadmap/m2-document-naming.md](docs/roadmap/m2-document-naming.md) | Detailed |
| M3 – Sketcher | [docs/roadmap/m3-sketcher.md](docs/roadmap/m3-sketcher.md) | Detailed |
| M4 – MCP, before any UI | [docs/roadmap/m4-mcp.md](docs/roadmap/m4-mcp.md) | Detailed, with each step as built |
| M5 | This file | Detailed when M4 ends |
| FreeCAD file import | [docs/roadmap/freecad-import.md](docs/roadmap/freecad-import.md) | Outline, detailed when its prerequisites exist |

**How to work through a plan:**
- **One step is one branch and one PR**, sized so the council can review it in one round.
- **Tests come first.** A step's listed tests are its specification. Write them, watch them fail, then implement. The listed tests are the minimum; `/break-it` and the council add more.
- **Expected values come from somewhere independent:** an analytic formula, a hand calculation, or the FreeCAD oracle. Never from the code under test.
- **Plans are refined one milestone at a time.** Near milestones are detailed; later ones stay as outlines until what we learn earlier can shape them. When reality differs from a plan, update the plan in the same PR and say why.

## Stage 1: the vertical slice

The goal: sketch → pad/pocket → fillet/chamfer → recompute with stable names → STEP and 3MF export, all usable from both the UI and MCP.

### M0 – Foundations
Cargo workspace. A pixi toolchain and a deterministic OCCT build from source (7.9.3, its FreeCAD reference's). A local gate (`pixi run gate`) that runs the workspace-lint check, cargo-deny, fmt, clippy, the tests, and a stress run on the committed tree. `tracing` set up, and the structured error type.

**Done when** one end-to-end test passes: Rust sends a request to a worker process, the worker has OCCT build a box and fillet an edge, and the volume that comes back matches FreeCAD's. This proves the process boundary, the `cxx` bridge, and the OCCT build all work together.

### M1 – The kernel layer
Worker pool. Handling for crashes, timeouts, and restarts, and discarding of stale results. Core operations that return history: primitives, extrude, revolve, booleans, fillet, chamfer. The FreeCAD oracle harness (`silico-oracle`).

**Done when** tests that `kill -9` workers mid-operation pass, and oracle tests match FreeCAD for every operation.

### M2 – Document and naming
The JSON timeline, the canonical writer, and the single parse → validate → recompute gate. DAG recompute, the per-feature cache, and fingerprints. The port of topological naming (`ElementMap` and `TopoShapeExpansion`). The CLI commands `silico check`, `silico fmt`, and `silico show`.

**Done when** golden models save and rebuild byte-identically on every platform, and references survive upstream edits or fail loudly.

### M3 – Sketcher
Can run in parallel with M2, except M3.6, which needs M2.7–M2.8. PlaneGCS in a worker, sketch entities and constraints, profile-to-face conversion, and pad, pocket, and revolve driven by sketches.

**Done when** the ported FreeCAD sketcher tests pass, and the latency of interactive drags through the worker has been measured. That measurement settles the IPC transport question.

### M4 – MCP, before any UI
The command API exposed over MCP: transactions, geometry queries that return stable names, measurement, offscreen PNG rendering, and `read_logs`. With it, a check's warnings, which M3 deferred here: M3.2's D2 warning for two curve ends at one place that aren't one point, and M3.3's agreeing redundant constraints, now `check`'s warnings with a third, a feature saved without a fingerprint (docs/design/m4.1-command-api.md, decision 17); `silico fmt --merge-touching`, to join such ends on request (docs/design/m3.2-sketch-format.md, D2), waits for M5's sketch editing.

**Done when** an agent designs a bracket headlessly and exports a printable 3MF.

### M5 – The app
A Fusion-style layout with browser tree, timeline bar, and property panel. A viewport built on `re_renderer` with crisp edges and GPU picking. The core's drag client, with the pool's rules for drags (ARCHITECTURE.md, *Sketches solve in the sketch worker*). The sketch-mode UI. Behavior follows SolidWorks; visuals follow Fusion 360. Sketch points are shared, merged as SolidWorks merges them (docs/design/m3.2-sketch-format.md, D2): drawing from a point shares it; trim, split, delete, and pulling a corner apart create or split shared points explicitly; unmerging is a deliberate act; deleting a curve keeps its neighbours' shared point.

**Done when** the vertical slice works end to end in both the UI and MCP.

The plan, its decisions taken: [docs/roadmap/m5-app.md](docs/roadmap/m5-app.md).

### FreeCAD file import
Once M2, M3, refine, and the `import` feature (with a deterministic STEP writer) exist; it doesn't block the slice. Silico opens a FreeCAD `.FCStd` file and saves it as a Silico document, one way: features Silico has become Silico features, checked against the shapes FreeCAD stored, and the rest come in as shapes with no history or are left out, reported. Silico never writes `.FCStd`.

**Done when** each file in a corpus imports as its hand-written table says (what each object becomes: a Silico feature, a stored shape, or left out, and why), and every imported shape matches FreeCAD's own, within the import's tolerance, by FreeCAD's measures and cut against it both ways.

### After the slice: the rest of stage 1
The full feature set (holes, loft, sweep, helix, draft, shell, patterns, mirror). Multi-body booleans. Parameters and their UX. Measure. Assembly with OndselSolver. STEP import.

Deferred, not a milestone: **sketch solver quality, maybe later.** Decomposition into rigid clusters in the core, ahead of PlaneGCS; constructive solving with stable root selection; structural degree-of-freedom analysis and minimal-motion drags. The gap to commercial solvers (D-Cubed DCM) is algorithmic, not transport: M3.7 measured the worker's round trip at about 0.05 ms and dense SQP on a 120-link chain at 166 ms per frame.

## Stage 2
Surface modeling, mesh editing, text-to-geometry, and DXF/SVG import into sketches.

## Stage 3
CAM, FEA, and TechDraw.
