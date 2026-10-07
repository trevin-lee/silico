# Silico architecture

Silico is a parametric CAD system with a Rust core, an egui/wgpu UI, and OpenCascade (OCCT) as the geometry kernel. FreeCAD is the reference implementation: we study how it solves hard problems such as topological naming and the edge cases its history records, then re-implement that behavior in idiomatic Rust. Humans and agents are equal clients of the same core.

This document records decisions. The **Invariants** section is enforced by the review skills in `.claude/skills/`. Change an invariant only with the maintainer's explicit agreement, and record why.

## Invariants

1. **No C++ runs in the app process.** OCCT, PlaneGCS, and OndselSolver run only in worker processes. A worker crash becomes a structured error, never an app crash.
2. **Workers are dumb.** A geometry operation goes in; a shape plus raw history (input sub-shape → output sub-shapes) comes out. Topological naming, the feature graph, recompute, caching, and error reporting all live in the Rust core.
3. **One headless core, many clients.** The egui app, the MCP server, the CLI, and the tests all go through the same command API. Anything the UI can do, the API can do.
4. **The UI thread never blocks** on recompute, worker calls, or IO.
5. **Recompute is deterministic.** The same document with the same pinned kernel gives identical shapes, names, and serialized bytes.
6. **Documents are git-native text.** The file stores the feature timeline, never B-rep. B-rep and meshes are rebuildable caches and are never committed. No LFS.
7. **One modeling workflow.** There is no Part vs PartDesign split. Primitives, booleans, and sketch-based features are all features in one timeline, and a feature can target any body.
8. **`unsafe` appears only in FFI crates.** No panics on paths reachable from user, agent, file, or worker input. Those return structured errors.
9. **Errors are structured:** a stable code, the feature, the sub-element, the cause, and a suggestion. `tracing` spans follow work across the worker boundary.
10. **Port behavior, not lines.** FreeCAD is the reference for *what* the code must do and *why*. The Rust is idiomatic in *how*.
11. **Oracle and golden tests are protected.** Any change to `tests/oracle/` or `tests/golden/` needs the maintainer's review.

## Process model

```
┌─────────────────────── silico app process (Rust only) ───────────────────────┐
│                                                                              │
│   egui UI ──┐                                                                │
│   MCP server ├──► command API ──► silico-core                                │
│   CLI ──────┘                     document · feature DAG · naming ·          │
│                                   recompute scheduler · cache · errors       │
└──────────────────────────────────────┬───────────────────────────────────────┘
                                       │ IPC (silico-protocol)
            ┌──────────────┬───────────┴──┬──────────────────┐
     kernel worker   kernel worker   sketch worker     assembly worker
        (OCCT)          (OCCT)        (PlaneGCS)        (OndselSolver)
```

- **Worker pool.** Kernel workers are sized to the CPU count. Independent branches of the feature DAG recompute in parallel across them. OCCT isn't reliably thread-safe, so process-level parallelism is the safe route.
- **Failure handling.** Every request has a timeout. A crashed or hung worker is killed and respawned, and the request fails with a structured error naming the feature. Each request carries a document generation number, and results for superseded generations are discarded.
- **Worker output.** Serialized B-rep (OCCT binary format), raw history maps, and tessellation when requested.
- **Workers are Rust binaries.** They handle protocol, timeouts, and errors in Rust, and call OCCT through a thin `cxx` bridge. The C++ side only calls OCCT and returns shapes and history.
- **OCCT 8.0.x**, the same version FreeCAD's baseline pins, so oracle tests compare like with like. It's installed from conda-forge through pixi (`pixi.toml`), never built ad hoc.
- **Platforms.** macOS first; Linux and Windows come later.
- **Checks run locally.** `pixi run gate` runs every check on the committed tree before a merge. Hosted CI waits until the repository moves to GitHub.
- **Headless mode.** `silico serve` runs the core without the UI, for CI and remote agents.

## Crates (planned)

| Crate | Role |
|---|---|
| `silico-core` | Document model, feature DAG, recompute, topological naming, cache, serialization, command API |
| `silico-protocol` | IPC message types shared by the core and workers. Few dependencies. |
| `silico-kernel` | Thin C++ shim over OCCT plus the worker binary. The only crate that links OCCT. |
| `silico-sketch` | PlaneGCS binding and sketch worker |
| `silico-assembly` | OndselSolver binding and assembly worker (later) |
| `silico-render` | Viewport built on `re_renderer` |
| `silico-app` | egui UI client |
| `silico-mcp` | MCP server client, including the perception tools |
| `silico-cli` | `check`, `fmt`, `show`, `export`, `serve`, CI commands |
| `silico-oracle` | Differential test harness against FreeCAD |

`silico-core` must never depend on egui, wgpu, or any client crate.

## Document model

A `.silico` file is strict JSON describing the feature timeline. Silico rebuilds geometry from it.

```json
{
  "silico": 1,
  "features": [
    {
      "id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "base", "plane": "XY",
      "points": [
        {"id": "p1", "x": 0, "y": 0},
        {"id": "p2", "x": 40, "y": 0}
      ],
      "lines": [
        {"id": "l1", "from": "p1", "to": "p2"}
      ],
      "constraints": [
        {"id": "c1", "type": "horizontal", "on": "l1"},
        {"id": "c2", "type": "distance", "on": "l1", "value": 40}
      ]
    },
    {
      "id": "a91cw0de", "type": "pad", "v": 1, "name": "body", "profile": "k7f2qx3m", "length": 10,
      "note": "Wall thickness matches the motor mount interface",
      "fingerprint": {"volume": 8000, "faces": 6, "bbox": [0, 0, 0, 40, 20, 10]}
    }
  ]
}
```

- **The timeline is the array order** of `features`.
- **IDs** are random 8-character lowercase base32, unique within the document, so parallel branches don't collide. Display names are separate and don't need to be unique.
- **Units are fixed in the file:** millimetres and degrees. Display units are a UI setting. Expressions are strings starting with `=` (`"length": "=width * 2"`).
- **Parameters** are named, typed values with units, stored at the top of the document. Expressions reference them. This is the data model only. How parameters are presented and edited (a spreadsheet, a parameter panel, inline in dimension fields) is a separate UX decision.
- **Sketches store solved coordinates.** On load they are the solver's initial guess. If the constraints changed, for example after a hand edit, the sketch is re-solved and `silico fmt` writes the corrected coordinates back.
- **Fingerprints** record volume, area, bounding box, and face/edge/vertex counts for each feature, much as a lockfile does. A rebuild that doesn't match its fingerprint is flagged loudly, and CI fails on drift. Fingerprints also make PR diffs meaningful.
- **Features are versioned** with `v`. Changing a feature's behavior bumps its version. Old documents keep rebuilding with the old behavior until they're explicitly upgraded.
- **Notes** are a data field, not comments, so they survive UI saves and are visible in the UI and through MCP.
- **Canonical form.** Keys are written in schema order, floats as the shortest round-trip form with no `-0`, one leaf entity per line, LF line endings, and a trailing newline. `silico fmt` produces canonical form, and every save writes it.
- **Imported geometry** is an `import` feature that references a STEP file committed next to the document.
- **Cache.** `.silico-cache/` stores the B-rep for each feature, keyed by a hash of the feature, its inputs, and the kernel version. It's gitignored. A fresh clone rebuilds once.

**One gate for every write.** The UI, MCP, hand edits to the text, and git merges all go through the same steps: parse → validate → recompute → structured errors. Agents may edit `.silico` files directly, then run `silico check`. The app watches open files and reloads when they change on disk.

**Human-readable views.** `silico show` prints a readable outline of a model. A git `textconv` driver makes `git diff` show that outline. CAD CI posts a readable diff and before/after renders on each PR.

## Topological naming

- The kernel returns history. The core builds the element map from it, as a port of FreeCAD's `ElementMap` and `TopoShapeExpansion`.
- Names stored in files are human-readable. No opaque hashes.
- After an upstream edit, a reference either resolves to the geometrically equivalent element or fails loudly with a structured error that lists candidates. It must never silently resolve to a different element.

## Agent interface (MCP)

- The MCP server is a client of the command API, with exactly the UI's capabilities.
- **Transactions:** begin, commit, and rollback, so agents can try something and undo it cheaply.
- **Perception:**
  - render a PNG from a named view or any camera, with elements highlighted;
  - list faces and edges with stable names, types, normals, and areas;
  - geometric queries that return stable names ("largest planar face facing +Z");
  - measurement and mass properties;
  - `check`.
- **Errors** carry a code, the feature, the element, the cause, a suggestion, and candidate fixes where they exist.
- **`read_logs`**, so agents can debug failures themselves.
- Typical agent workflow: query through MCP to get stable names, edit the timeline as text or through commands, then `check`.

## Errors and logging

`tracing` is used throughout. Span IDs are passed into worker requests so a failure can be traced from the user action to the feature to the OCCT call. Worker crash reports are captured and attached to the error. Error codes are stable and documented.

## Performance

- Recompute runs in the background, is cancellable, and discards superseded results. The viewport never waits on it.
- Independent DAG branches recompute in parallel across the worker pool. Tessellation is parallel too.
- The GPU handles rendering, ID-buffer picking, and instancing for large assemblies. Geometry modeling stays on the CPU, in OCCT.
- The per-feature cache makes edits recompute only what changed downstream.

## Rendering and UI

**UX benchmarks.** When a UX choice is unclear, don't invent one, and don't copy FreeCAD:

- **Behavior follows SolidWorks:** what a tool does, sketch inference and auto-constraints, selection and pre-selection, dragging, how features react to edits, and rebuild and error feedback.
- **Visuals and UI follow Fusion 360:** layout (browser tree, timeline bar, toolbar organization), dialogs and in-canvas manipulators, and the overall look.

Rerun's `re_ui` provides the dark styling toolkit (color, type, density) used to build that UI.

- egui with `egui_tiles` for docking.
- The viewport is built on `re_renderer`, pinned to a version. Fork it only if CAD features need changes to its internals.
- The hard parts are in the viewport: crisp edges, hidden-line display, silhouettes, precise picking, and sketch overlays.

## Testing

Silico has one maintainer and mostly agent-written code. Tests stand in for line-by-line human review.

- **Unit tests** cover every behavior change. Expected values must come from an independent source, not from the code under test.
- **Oracle tests** (`tests/oracle/`, protected): run the same operation in Silico and in headless FreeCAD, and compare volume, area, bounding box, topology counts, and validity.
- **Golden corpus** (`tests/golden/`, protected): real models with fingerprints. Rebuilds must match.
- **Ported FreeCAD tests** for every subsystem that has them.
- **Determinism tests:** rebuild twice and compare bytes.
- **Property tests** (proptest) biased toward edge values, **fuzzing** (cargo-fuzz) for everything that parses input, and `/break-it` for hostile inputs.
- **Mutation testing:** `cargo mutants --in-diff` on every change.
- **Timeouts:** `cargo nextest` with slow-test timeouts, so hangs fail.

## Scope: FreeCAD parity

| Stage | Scope |
|---|---|
| 1. Parity | Sketcher, Part and PartDesign (merged into one workflow), Assembly, Measure, expressions and parameters, STEP/STL/3MF import and export |
| 2 | Surface modeling, mesh editing, text-to-geometry (engraving), DXF/SVG import into sketches |
| 3 | CAM, FEA, TechDraw (2D engineering drawings) |
| Out of scope | BIM, Robot, Draft as a separate workbench |

Draft's useful pieces are absorbed into the single workflow instead of becoming a second 2D system. ShapeString becomes a text feature, DXF/SVG import feeds sketches, and Draft's arrays become pattern features.

FreeCAD's spreadsheet *capability* (named parameters driving the model) is in stage 1. Its *UX* is not decided; see Open questions.

**First milestone (vertical slice):** sketch → pad/pocket → fillet/chamfer → recompute with stable names → STEP and 3MF export, all usable from both the UI and MCP.

## License

GPL-3.0-or-later. Silico's dependencies are LGPL (FreeCAD, OCCT, PlaneGCS, OndselSolver) and MIT/Apache (egui, wgpu, Rerun crates), and all of these licenses are compatible with GPL.

## Open questions

- The syntax of topological names in files.
- Sketch entity ordering: creation order or sorted by ID. A semantic git merge driver.
- IPC transport (pipes or shared memory), and whether an out-of-process sketch solver meets the latency budget for interactive drags.
- Interfaces: published, semver-versioned geometry contracts that parallel design work builds on. Also CAD CI.
- Version control beyond git.
- Multi-file projects: how assemblies reference parts in other files.
- Parameter UX: a FreeCAD-style spreadsheet, a Fusion-style parameter table, SolidWorks-style global variables, inline editing in dimensions, or a mix. Design tables and configurations (variants of one part) belong in the same decision.