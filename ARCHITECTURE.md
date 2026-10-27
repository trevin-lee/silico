# Silico architecture

Silico is a parametric CAD system with a Rust core, an egui/wgpu UI, and OpenCascade (OCCT) as the geometry kernel. FreeCAD is the reference implementation: we study how it solves hard problems such as topological naming and the edge cases its history records, then re-implement that behavior in idiomatic Rust. Humans and agents are equal clients of the same core.

This document records decisions. The **Invariants** section is enforced by the review skills in `.claude/skills/`. Change an invariant only with the maintainer's explicit agreement, and record why.

## Invariants

1. **No C++ runs in the app process.** OCCT, PlaneGCS, and OndselSolver run only in worker processes. A worker crash becomes a structured error, never an app crash.
2. **Workers are dumb.** A geometry operation goes in; a shape plus raw history (input sub-shape → output sub-shapes) comes out. Topological naming, the feature graph, recompute, caching, and error reporting all live in the Rust core. The render worker is dumb the same way: a scene (the kernel's meshes, the core's camera and highlights) goes in; a PNG and, when asked, the ID buffer come out; the core has decided every name and every refusal but a worker fault before it asks.
3. **One headless core, many clients.** The egui app, the MCP server, the CLI, and the tests all go through the same command API. Anything the UI can do, the API can do.
4. **The UI thread never blocks** on recompute, worker calls, or IO.
5. **Recompute is bit-identical on every checked platform.** The same document rebuilt by the same build of Silico's workers gives byte-identical shapes, names, and saved documents in every run and on every checked platform. Checked platforms are macOS ARM and Linux x86; a platform joins the list once it passes `cross_platform_determinism`. The *Deterministic kernel* section explains how. Any difference is a bug: a rebuild that doesn't match a stored fingerprint fails as drift and is never smoothed over.
6. **Documents are git-native text.** The file stores the feature timeline, never B-rep. B-rep and meshes are rebuildable caches and are never committed. No LFS.
7. **One modeling workflow.** There is no Part vs PartDesign split. Primitives, booleans, and sketch-based features are all features in one timeline, and a feature can target any body.
8. **`unsafe` appears only in FFI crates**, and in one OS-boundary exception: `silico-worker`, for a worker's inherited descriptors only (taking its socket, descriptor 3, and closing the others it inherited), agreed by the maintainer on 2026-10-16 so both workers share one copy. No panics on paths reachable from user, agent, file, or worker input. Those return structured errors.
9. **Errors are structured:** a stable code, the feature, the sub-elements at fault (every one, or, where there are too many to list, the first hundred and how many there are), the parameter at fault, the cause, and a suggestion. `tracing` spans follow work across the worker boundary.
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
      ┌────────────┬────────────┬──────┴──────┬─────────────┬─────────────┐
    kernel       kernel       sketch       assembly       render         STEP
    worker       worker       worker        worker        worker        worker
    (OCCT)       (OCCT)     (PlaneGCS)  (OndselSolver)  (lavapipe)   (OCCT STEP)
```

- **Worker pool.** Up to one kernel worker a core, each started when a request first needs it and kept, so a rebuild pays only for the workers it uses at once. Independent branches of the feature DAG recompute in parallel across them. OCCT isn't reliably thread-safe, so process-level parallelism is the safe route.
- **Failure handling.** Every request has a timeout. A crashed or hung worker is killed and respawned, and the request fails with a structured error naming the feature. Each request carries a generation number of the client that submitted it (the pool's own, or a client the pool gives, `KernelPool::client`, each with generations of its own: a session's rebuilds take one, and its reads of the published model one or more others (docs/design/m4.1-command-api.md, decision 33)): results for that client's superseded generations are discarded, and work still running for them is stopped once it has run longer than replacing a worker takes (50 ms by default), unless it finishes first. One client's newer generation never touches another's work, and the pool serves its clients' waiting requests in turn.
- **Worker output.** Serialized B-rep (OCCT binary format), raw history maps, and tessellation when requested. The render worker's: a PNG, and the ID buffer (each pixel's body and face or edge number) when asked.
- **The render worker.** One per `Sessions`, started on first use, scenes rendered in turn, 30 s each, killed on timeout or cancel, stopped after a large render or when idle (docs/design/m4.6-render.md). Its rasterizer is Mesa's lavapipe, conda-forge's build pinned in `pixi.lock` (26.2.5 at M4.6), compiled-in paths and its own re-exec, never the ambient environment; the machine's GPU only without it. Mesa moves when Silico chooses, with the render tests on both checked platforms as the check; no reference depends on it.
- **Sketches solve in the sketch worker, drags included** (the maintainer's decision, 2026-10-16, from M3.7's measurements). PlaneGCS stays linked in its worker: no port, no shared memory, no solver in the app process. A drag is a session in the worker (begin diagnoses once and pins the dragged point; each move solves from the begin state; end hands back to a normal solve), so a frame depends only on the begin state and the cursor. The round trip costs about 0.05 ms; what costs is the solver's own work (dense SQP on a large connected part), which no transport changes. For big connected sketches the UI solves only the newest cursor, with one request in flight, and drops stale frames. A sketch worker holds at most one session, so a drag's moves go to the worker that began it, and that worker isn't stopped for a superseded generation while the session lasts (these two, the pool's, are planned for M5, which brings the core's drag client; the rest of this paragraph is the worker's, built in M3.7) (a move can take longer than the 50 ms the pool waits); the session ends with a normal solve, a new begin, an exception in the solver, or the worker's restart, and a move after it is refused, so the client begins again. The exemption is from supersession only: each move is a request like any other, under its timeout and the crash and hang handling, so a move that hangs or crashes ends in a worker error and a restart, which ends the session. A drag's session holds only the part of the sketch its pins move (everything else held where it began), and takes at most 256 unknowns there, where SQP's dense work stays under a second a move.
- **Workers are Rust binaries.** They handle protocol, timeouts, and errors in Rust, and call OCCT through a thin `cxx` bridge. The C++ side only calls OCCT and returns shapes and history.
- **OCCT 7.9.3**, built from source with the recipe below: the same release as the FreeCAD Silico references (conda-forge's FreeCAD 1.1.3, whose OCCT package carries no geometry patches), so FreeCAD is a clean reference. Silico's OCCT moves only when its reference FreeCAD's does, and only after the new release is checked against the cases Silico knows about: OCCT 8.0.1 was tried first, and has a boolean regression 7.9.3 doesn't (docs/ports/booleans.md). The recipe changes the last bits of results, so oracle comparisons still use tolerances, but the same cases succeed and fail on both sides.
- **Platforms.** macOS first; Linux and Windows come later.
- **Checks run on our own hardware.** `pixi run gate` runs every check on the committed tree before a merge, on the Mac (ARM macOS). CI runs the same script, plus heavy jobs, through GitHub Actions on a self-hosted runner on the maintainer's k3s workstation (x86-64 Linux), so a private repository uses no paid minutes. That covers both operating systems and both CPU architectures, at no cost.
- **Headless mode.** `silico serve` runs the core without the UI, for CI and remote agents.

### Deterministic kernel

Stock OCCT is not deterministic. Its results differ in the last bits between CPU architectures and math libraries, and some algorithms differ between two runs on one machine because they iterate maps hashed by memory address. Silico builds all C++ in the workers whose output is a result, a shape and its history, a solve, a measure, a mesh (the kernel, sketch, and assembly workers: OCCT, the `cxx` shim, PlaneGCS, OndselSolver), so that it is:

1. **From source**, at a pinned release, with Silico's patch set applied (OCCT's lives in `third_party/occt/`).
2. **With one pinned toolchain** on every platform: clang and libc++ (headers and runtime, same version) from pixi. OCCT calls libc++ at runtime (`std::sort` and more), so the runtime is part of the recipe.
3. **With no fused, reordered, or build-time-evaluated math**: `-ffp-contract=off` and no fast-math, so the compiler never fuses or reorders floating-point operations differently per CPU, and `-fmath-errno` with `-fno-builtin-<fn>` for every inexact math function, so it never computes a math call with constant arguments at build time using the build machine's own math library. The function list (`third_party/occt/scripts/inexact-math.txt`) also generates a canary that fails the build if any call gets folded. Exact builtins such as `sqrt` stay inline. Libraries that fuse operations explicitly are configured not to: Eigen, used by PlaneGCS, is built with vectorization off (`EIGEN_DONT_VECTORIZE`).
4. **With the platform math library replaced**: a shim backed by the Rust `libm` crate (`third_party/occt/libm-shim/`) is linked into every worker library as a private copy, so `sin`, `pow`, `exp10`, and the rest are the same code everywhere and no library exports them to others. A check fails the build if any worker library or binary imports a platform math function that isn't exact by IEEE definition (`sqrt`, `floor`, `fmod`, and the like). `third_party/occt/build.sh` writes the recipe (compiler, flags, the shim object, the flag that hides it) to `target/occt/recipe.env`, which every other piece of worker C++ builds from. A shared library links a private copy with the hide flag. The `cxx` bridge is a static archive inside `silico-kernel`, whose `build.rs` links the shim into every executable of that crate (the worker, test binaries, benches), covering the bridge's math and Rust's own. `silico-sketch` (PlaneGCS) does the same for its own bridge. Only these two worker crates link C++, and the import check runs on their executables (never on an archive or object file): every one but the sketch crate's test worker, which can switch traps on, so imports `feenableexcept` on Linux.
5. **Single-threaded inside each worker.** Parallelism comes from the worker pool.
6. **With OCCT's input checks compiled in.** OCCT's Release default (`-DNo_Exception`) removes them, so invalid input (a zero scale, a zero-length direction) returns quiet nonsense instead of a `Standard_Failure` the worker can turn into a structured error. A check proves the built OCCT throws.
7. **With floating-point traps off.** Workers never enable FP exception traps (no `OSD::SetSignal(true)`; `OSD::SetFloatingSignal(false)`), and assert that none are enabled at startup and after every request. Only OCCT's TKernel may import `feenableexcept`, and the sketch worker's test binary, which switches traps on to prove the check. With traps on, glibc raises SIGFPE where macOS ARM carries on with NaN, so the same document could succeed on one platform and fail on the other.

**The render worker is outside this recipe** (Fable for the maintainer, 2026-10-25, recorded as docs/design/m4.6-render.md's R5; the maintainer's veto open). Its output is pixels: a PNG and, when asked, the ID buffer, which M4.1 (section 10, decision 5) declares not results. It draws with `re_renderer` on Mesa's lavapipe, conda-forge's build pinned by version and hash in `pixi.lock` rather than this recipe; llvmpipe runs two threads (it rasterizes tile by tile, so its result doesn't depend on the thread count); and it doesn't check floating-point traps (it links no C++ to read the register through, nothing it loads enables them, and no result of its could change if they were on; its `Platform` says they're unchecked). In exchange, one rule: **nothing it returns reaches a result, a name, a document, the cache, or a stored expected value.** The core computes the camera, decides which bodies are in view and at what mesh level, resolves every highlight name, and decides every refusal but a worker fault before the scene is sent; the worker's only refusal of a scene is of one that fails `RenderScene::check`, which the core never sends; beyond that it fails only as a worker does, a fault of the machine and not the scene: no adapter it may draw on (`Refusal::NoAdapter`, which the core words as `worker_unavailable`); the GPU (lavapipe's: the machine's memory) out of memory (`Refusal::OutOfMemory`, `resource_exhausted`); any other GPU or renderer error, as an internal one; a crash; a timeout. After a failed render it keeps no render context. No command returns the ID buffer. Tests read it only against a projection computed by hand: exact inside a face, within one pixel of an element's boundary (a rasterizer's quantization, which the vertex transform's `f32` math can move by a subpixel between platforms), and never against a stored image or buffer; a PNG's bytes and `png_bytes` are never compared. The viewport's picking (M5) reads it as a selection, resolved to a name at once. Everything else in this document binds it as any worker: Rust only, a timeout that kills it, a crash as a structured error, no `unsafe` of its own, the workspace's lints (its own math through `libm`).

Each component is verified with the stress-test method when it arrives: OCCT in M0.2, PlaneGCS in M3, OndselSolver with assembly.

Evidence, in `tests/determinism/`: with OCCT 7.9.3, PR #1600, and the full shim, a 605-case randomized stress test (booleans, fillets, chamfers, shells, offsets, revolves, prisms, lofts, sweeps, meshes) is bit-identical across repeated runs and between macOS ARM and Linux x86, with exactly the same failures as stock OCCT 7.9.3, and the import check passes on both. (With OCCT 8.0.1, the first release Silico pinned, the same held on macOS ARM, macOS x86, and Linux x86.) The recipe costs about 6% speed against stock conda-forge OCCT on the stress test. `pixi run stress` reruns it; run it on every OCCT upgrade, patch change, and toolchain change.

**OCCT patch policy.** Patches live in `third_party/occt/patches/`, applied in order to the pinned release. Each patch must fix determinism or a crash that Silico can't work around, or expose what OCCT already computes, as accessors that change no result (the stress test stays byte-identical). Each must exist upstream as a PR or issue, and its header names it. No other features. A patch may also fix a wrong result of OCCT's precise measurement (`BRepGProp` with an `Eps`) that Silico can't work around exactly on its own side, if it leaves the default integration (FreeCAD's: `measure_as_freecad`, the oracle, the stress reference) byte-identical, its expected values are by hand and independent of OCCT, and it names every committed fingerprint and protected reference it moves. A patch is dropped once an OCCT release includes it. The first patch is upstream PR #1600, which makes arc-join offsets iterate their faces in the order they were created rather than by memory address. The second gives the chamfer the fillet's accessors for where a failed build failed, so a chamfer's error names its failing edges. The third splits a surface of revolution's angle into three spans in the precise measurement, as OCCT already splits a torus's, so a revolved barrel or dome is measured to round-off, not 2.2e-7 low. The fourth refines the precise measurement along a face's U to the asked precision of each strip's magnitude, as OCCT already refines along its edges, so extrusions of B-splines are measured to it, not up to 9.4e-4 low in volume and 5% off in area, and B-spline faces' areas not 1.3e-5 off.

## Crates (planned)

| Crate | Role |
|---|---|
| `silico-core` | Document model, feature DAG, recompute, topological naming, cache, serialization, command API |
| `silico-protocol` | IPC message types shared by the core and workers. Few dependencies. |
| `silico-kernel` | Thin C++ shim over OCCT plus the worker binaries: the kernel worker, and the STEP worker (OCCT's STEP writer, `TKDESTEP`, linked into it alone; started for each export, M4.8). The only crate that links OCCT. |
| `silico-sketch` | PlaneGCS binding and sketch worker |
| `silico-worker` | What every worker process is: the inherited socket, the request loop, and logging, shared by every worker; the floating-point trap checks, by those whose output is a result |
| `silico-assembly` | OndselSolver binding and assembly worker (later) |
| `silico-render` | `re_renderer` rendering: the render worker binary (headless PNGs on lavapipe, M4.6) and the viewport (M5) |
| `silico-app` | egui UI client |
| `silico-mcp` | MCP server client, including the perception tools |
| `silico-cli` | `check`, `fmt`, `show`, `export`, `serve`, CI commands |
| `silico-oracle` | Differential test harness against FreeCAD |
| `silico-view` | Debug views of geometry in the Rerun viewer, until the app exists (`pixi run view`) |

`silico-core` must never depend on egui, wgpu, or any client crate.

## Document model

A `.silico` file is strict JSON describing the feature timeline. Silico rebuilds geometry from it.

```json
{
  "silico": 1,
  "units": {"length": "mm", "resolution": "1nm"},
  "naming": 5,
  "features": [
    {
      "id": "k7f2qx3m", "type": "sketch", "v": 1, "name": "base", "plane": "xy",
      "points": [
        {"id": "0a1b2c3d", "x": 0, "y": 0, "construction": false},
        {"id": "2p3q4r5s", "x": 0, "y": 20, "construction": false},
        {"id": "4e5f6g7h", "x": 40, "y": 0, "construction": false},
        {"id": "8j9k0m1n", "x": 40, "y": 20, "construction": false}
      ],
      "lines": [
        {"id": "6t7v8w9x", "from": "0a1b2c3d", "to": "4e5f6g7h", "construction": false},
        {"id": "a0b1c2d3", "from": "4e5f6g7h", "to": "8j9k0m1n", "construction": false},
        {"id": "j8k9m0n1", "from": "8j9k0m1n", "to": "2p3q4r5s", "construction": false},
        {"id": "p2q3r4s5", "from": "2p3q4r5s", "to": "0a1b2c3d", "construction": false}
      ],
      "circles": [],
      "arcs": [],
      "constraints": [
        {"id": "c0d1e2f3", "type": "horizontal", "line": "6t7v8w9x"},
        {"id": "g4h5j6k7", "type": "length", "line": "6t7v8w9x", "value": 40, "driving": true},
        {"id": "m8n9p0q1", "type": "distance_x", "a": "origin", "b": "0a1b2c3d", "value": 0, "driving": true},
        {"id": "r2s3t4v5", "type": "vertical", "line": "a0b1c2d3"},
        {"id": "w6x7y8z9", "type": "length", "line": "a0b1c2d3", "value": 20, "driving": true},
        {"id": "x0y1z2a3", "type": "horizontal", "line": "j8k9m0n1"},
        {"id": "y4z5a6b7", "type": "distance_y", "a": "origin", "b": "0a1b2c3d", "value": 0, "driving": true},
        {"id": "z8a9b0c1", "type": "vertical", "line": "p2q3r4s5"}
      ]
    },
    {
      "id": "a91cw0de", "type": "pad", "v": 1, "name": "body", "sketch": "k7f2qx3m",
      "extent": {"type": "one_side", "length": 10, "reversed": false},
      "note": "Wall thickness matches the motor mount interface",
      "fingerprint": {"volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}
    }
  ]
}
```

- **The timeline is the array order** of `features`.
- **Sketches** (docs/design/m3.2-sketch-format.md) list points their curves share, lines, circles, arcs, and constraints, each element with its own ID and each list sorted by ID; points' coordinates are the solver's, written in full. (Canonical form writes a feature that holds objects one member per line; the example is compressed.)
- **IDs** are random 8-character lowercase base32, unique within the document, so parallel branches don't collide. Display names are separate and don't need to be unique.
- **Units** are declared in the `units` header. Version 1 accepts only millimetres at 1 nm resolution, with angles in degrees at 1 nanodegree resolution. The header exists so other scales, such as nanometre-scale documents, can be added later without breaking the format. The length unit is also the kernel's working unit, because OCCT's tolerances are absolute: a 5 nm feature in a millimetre document would be only 50× OCCT's tolerance. Display units are a UI setting. Expressions are strings starting with `=` (`"length": "=width * 2"`).
- **The naming scheme** the document's element names were written under is declared in the `naming` header (5, M4.0). A document of another scheme is refused, naming it, so a name is never read under a scheme other than its own; a document saved before the header recorded it is read as scheme 5.
- **Parameters** are named, typed values with units, stored at the top of the document. Expressions reference them. This is the data model only. How parameters are presented and edited (a spreadsheet, a parameter panel, inline in dimension fields) is a separate UX decision.
- **Sketches store solved coordinates.** On load they are the solver's initial guess. If the constraints changed, for example after a hand edit, the sketch is re-solved and `silico fmt` writes the corrected coordinates back.
- **Fingerprints** record volume, area, bounding box, and face/edge/vertex counts for each feature, much as a lockfile does. A rebuild that doesn't match its fingerprint is flagged loudly, and CI fails on drift. Fingerprints also make PR diffs meaningful. As with a lockfile, an edit by hand or in a merge leaves the fingerprints it affects stale until `format` (or `silico fmt`) accepts them, naming each; a commit writes new fingerprints only for the features its edits reached (docs/design/m4.1-command-api.md, decision 28), and a feature that fails to build keeps its last fingerprint, unless the session's edits reached it: then it's saved with none, since a fingerprint measured from other values is no record of it (decision 28, amended).
- **Features are versioned** with `v`. Changing a feature's behavior bumps its version. Old documents keep rebuilding with the old behavior until they're explicitly upgraded. Until the first release, every feature's `v` stays 1, as the schema's `silico` does: no document has shipped, so a behavior change is agreed with the maintainer instead of bumping it.
- **Notes** are a data field, not comments, so they survive UI saves and are visible in the UI and through MCP.
- **Canonical form.** Keys are written in schema order, numbers as described in *Numbers in the file*, one leaf entity per line, LF line endings, and a trailing newline. `silico fmt` produces canonical form, and every save writes it.
- **Imported geometry** is an `import` feature that references a STEP file committed next to the document.
- **Cache.** `.silico-cache/` stores each kernel step's result (the B-rep, the worker's word on whether it's valid, the history, and its measure, or the kernel's refusal to measure it if that's a fact about its values, kept as the worker said it, as data, and worded by the core that reads it back), under one seal (so it isn't checked again), keyed by a hash of the step, its inputs, the kernel's identity (the worker executable's SHA-256, which names the OCCT it loads), and the platform; a fillet's or chamfer's step also by its base's naming key, which holds the naming scheme's version, so a result made from edges an older Silico's names found is never taken for today's. The seal is an HMAC under a secret of the user's, outside any document, so a cache that comes with a document isn't trusted. It ignores itself (a `.gitignore` of `*`). It is bounded, 2 GiB a directory by default: a put that would pass the bound evicts the entries least recently used, and partial files of killed writes are removed after an hour. An eviction is only a miss. A fresh clone rebuilds once.

### Numbers in the file

These rules keep a document's bytes identical on every machine and make any drift visible.

- **Lengths and angles are fixed-point integers:** `i64` nanometres and nanodegrees, behind `Length` and `Angle` types. They're JSON numbers, written as plain decimals (`10.5`) and parsed straight from the number's text, never through a float; an exponent (`1e-05`, as scripts write small values) is read exactly too. Strings in their place are reserved for expressions. NaN, infinity, and `-0` can't be represented, typed values are exact, and equality and hashing are exact. Values with more decimals than the resolution, and values out of range, are rejected with a structured error. Computation converts to `f64` at the boundary.
- **Silico's own math is deterministic.** The Rust core uses a pure-Rust math library (`libm`) instead of the platform's, so expressions and other core calculations give bit-identical results everywhere. `clippy.toml` bans the platform versions (`f64::sin`, `powf`, and the like). An `allow` can lower that lint; review catches it.
- **Computed values are written exactly as computed.** Fingerprints are rounded to fixed precision before they're written, so diffs read `8000` rather than `7999.999999999998`. Solved sketch coordinates are written in full. Nothing is kept from a previous save to hide a difference.
- **Drift is an error.** A rebuild whose results don't match a stored fingerprint fails `silico check` and CI, naming the feature. It's either a determinism bug or a kernel change, and both need a human to look. Kernel upgrades rebuild the golden corpus and show every fingerprint that moved, for review.
- **Cross-platform identity is tested, not assumed.** M0.9 rebuilds the stress corpus, the golden corpus, and every oracle case on the Mac and the Linux workstation and requires bit-identical results.

FreeCAD avoids this problem by storing each feature's B-rep in the file and loading it instead of recomputing. That's also what makes its files impossible to diff. It writes floats with 16 significant digits, which isn't enough for an exact round trip. Silico recomputes from text, so it needs these rules instead.

**One gate for every write.** The UI, MCP, hand edits to the text, and git merges all go through the same steps: parse → validate → recompute → structured errors. Agents may edit `.silico` files directly, then run `silico check`. The app watches open files and reloads when they change on disk.

**Human-readable views.** `silico show` prints a readable outline of a model. A git `textconv` driver makes `git diff` show that outline. CAD CI posts a readable diff and before/after renders on each PR.

## Topological naming

- The kernel returns history. The core builds the element map from it, as a port of FreeCAD's `ElementMap` and `TopoShapeExpansion`.
- Names stored in files are human-readable. No opaque hashes. A name is the ID of the feature that made the element, its type, the element's kind, and how it came about, every source written out: `m3tq8v0z.cut.Edge(G k7f2qx3m.box.Face6, G a91cw0de.cylinder.Face1)`, the edge a cut generated from a block's top face and a pin's side (docs/design/m2.7-naming.md). Every element says its feature's type, a primitive's (`k7f2qx3m.box.Face6`) and an operation's (`m3tq8v0z.cut.…`), read from the name and never from the feature, so a feature made another type under its ID breaks references to its elements. An element an operation keeps of what lies outside its other operand keeps its name exactly, kept whole or modified one from one as its sole result (a cut's base, a fuse's operands, a fillet's or chamfer's input: `pppppppp.box.Face6`, a plate's top face after any number of holes), so a name grows only where elements split, merge, or are generated. What an operation keeps inside its other operand (a cut's tool, a common's operands), kept whole or trimmed, is a piece, the feature's own, its source written `I` (`m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face1)`, the hole's wall; `cccccccc.cut.Edge(I1/2 aaaaaaaa.box.Edge9)`, a split edge's piece), so a cut made a common, or its base and tool swapped, takes no reference to a piece for another. A source's position in OCCT's list is written only where two elements' names would otherwise be the same, as the element's rank among them and how many there are (`G1/2`, `M2/3`): OCCT's lists reorder for reasons of their own, and a name that holds no position doesn't change with them. An edge or vertex no history names is named by every named face (or edge) it bounds, never by its place among a face's edges (`cccccccc.Edge(U cccccccc.Face(G aaaaaaaa.box.Edge7), aaaaaaaa.box.Face5)`), so an edit that takes it away breaks a reference to it rather than passing its name on. A sketch's elements are named by their IDs, and only as sources in what a feature made of them: a pad's side face is `p0000001.pad.Face(G s0000001.sketch.Edge.6t7v8w9x)`, its top `p0000001.pad.Face(G s0000001.sketch.Face1)`, every element of a pad, pocket, revolve, or groove its own (docs/design/m3.6-sketch-features.md, S4).
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
- **Errors** carry a code, the feature, the elements at fault and how many there are, the parameter at fault, the cause, a suggestion, and candidate fixes where they exist.
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
- **Oracle tests** (`tests/oracle/`, protected): run the same operation in Silico and in headless FreeCAD, and compare volume, area, bounding box, center of mass, topology counts, and validity.
- **Golden corpus** (`tests/golden/`, protected): real models with fingerprints. Rebuilds must match.
- **Ported FreeCAD tests** for every subsystem that has them.
- **Determinism tests:** rebuild twice and compare bytes.
- **Property tests** (proptest) biased toward edge values, **fuzzing** (cargo-fuzz) for everything that parses input, and `/break-it` for hostile inputs.
- **Mutation testing:** `cargo mutants --in-diff` on every change.
- **Timeouts:** `cargo nextest` with slow-test timeouts, so hangs fail.

## Scope: FreeCAD parity

| Stage | Scope |
|---|---|
| 1. Parity | Sketcher, Part and PartDesign (merged into one workflow), Assembly, Measure, expressions and parameters, STEP/STL/3MF import and export, and importing FreeCAD files (one way: Silico never writes `.FCStd`; docs/roadmap/freecad-import.md) |
| 2 | Surface modeling, mesh editing, text-to-geometry (engraving), DXF/SVG import into sketches |
| 3 | CAM, FEA, TechDraw (2D engineering drawings) |
| Out of scope | BIM, Robot, Draft as a separate workbench |

Draft's useful pieces are absorbed into the single workflow instead of becoming a second 2D system. ShapeString becomes a text feature, DXF/SVG import feeds sketches, and Draft's arrays become pattern features.

FreeCAD's spreadsheet *capability* (named parameters driving the model) is in stage 1. Its *UX* is not decided; see Open questions.

**First milestone (vertical slice):** sketch → pad/pocket → fillet/chamfer → recompute with stable names → STEP and 3MF export, all usable from both the UI and MCP.

## License

GPL-3.0-or-later. Silico's dependencies are LGPL (FreeCAD, OCCT, PlaneGCS, OndselSolver) and MIT/Apache (egui, wgpu, Rerun crates), and all of these licenses are compatible with GPL.

## Open questions

- A semantic git merge driver.
- Interfaces: published, semver-versioned geometry contracts that parallel design work builds on. Also CAD CI.
- Version control beyond git.
- Multi-file projects: how assemblies reference parts in other files.
- Parameter UX: a FreeCAD-style spreadsheet, a Fusion-style parameter table, SolidWorks-style global variables, inline editing in dimensions, or a mix. Design tables and configurations (variants of one part) belong in the same decision.