# M1 – The kernel layer

**Goal:** a complete, crash-proof geometry service. Workers run the core modeling operations and return shapes, topology summaries, and raw history. A worker pool runs them in parallel. Every operation is checked against FreeCAD.

**Exit tests:** the `kill -9` suite (M1.6) passes, every oracle case (M1.8) matches FreeCAD, the determinism sweep (M1.9) is byte-identical, and the council approves with `/adversarial-review --milestone`.

**Tolerances used below:** relative 1e-9 for shapes made only of planes, relative 1e-6 for curved shapes. Tighten them if OCCT turns out to be more precise. These are for comparing against analytic values and FreeCAD. They're far too loose for cross-platform determinism, which uses the tolerance measured in M0.9.

---

### M1.1 – Shape transport

**Builds:** shapes move between the core and workers as serialized OCCT binary B-rep (`BinTools`), wrapped in a `ShapeBlob` that carries a content hash. Workers are stateless: every request includes the shapes it needs.

**Tests:**
- `blob_roundtrip_preserves_shape`: serializing and deserializing leaves volume, area, topology counts, and validity unchanged.
- `blob_hash_is_deterministic`: the same operation run 100 times, across fresh workers, gives an identical hash. If it isn't, that's a determinism bug: find the cause and fix it (with an upstream issue or patch if it's in OCCT). Never hash around it.
- `corrupt_blob_is_error`: a truncated or bit-flipped blob returns a structured error.
- Benchmark: round-trip time for a 1,000-face shape, recorded as the baseline.

### M1.2 – Topology summary

**Builds:** a `summarize(shape)` operation. It returns volume, area, bounding box, and validity. For each face: its index, surface type, area, and normal at the center. For each edge: its index, curve type, and length. For each vertex: its position. This is the basis for fingerprints (M2) and for agent perception (M4).

**Tests:**
- `box_summary`: a 10 × 20 × 30 box has 6 planar faces with areas {200, 200, 300, 300, 600, 600}, 12 straight edges of lengths 10, 20, or 30 (4 of each), and axis-aligned normals.
- `cylinder_summary`: r = 5, h = 10 has 1 cylindrical face and 2 planar faces, lateral area 100π, and volume 250π.
- `invalid_shape_reported`: a deliberately broken shape (a face with an open wire) gives `valid: false` with the reason, and the worker doesn't crash.

### M1.3 – Primitives

**Builds:** box, cylinder, sphere, cone, and torus. Parameters are validated in Rust before anything is sent to a worker: zero, negative, `NaN`, infinite, or out-of-range values return `InvalidParameter`, naming the parameter.

**Tests (analytic):**

| Test | Shape | Volume | Area |
|---|---|---|---|
| `cylinder_analytic` | r = 5, h = 10 | 250π | 150π |
| `cone_analytic` | r₁ = 5, r₂ = 0, h = 10 | 250π/3 | — |
| `sphere_analytic` | r = 5 | 500π/3 | 100π |
| `torus_analytic` | R = 10, r = 2 | 80π² | 80π² |

- `primitive_volume_property` (proptest): random valid dimensions from 1e-3 to 1e4 always give the formula volume within tolerance.
- `invalid_params_never_reach_worker`: every invalid parameter returns `InvalidParameter` without sending a worker request. The test asserts the worker's request count is zero.

**As built:**
- The port note is [docs/ports/primitives.md](../ports/primitives.md). Each primitive is built as FreeCAD's Part features build it (the cylinder extruded from its bottom disc, the torus revolved from a circle, a cone with equal radii as a cylinder), because the construction decides the order of faces and edges. `primitives_match_freecad_face_for_face_and_edge_for_edge` compares ten cases with FreeCAD 1.1.3, face for face and edge for edge (each one's extent, in order), plus volume, area, center, vertices, and validity.
- `Op::MakePrimitive(Primitive)` replaces `Op::MakeBox`. `Primitive::check` (in `silico-protocol`) is the one rule for which values make a solid; the core applies it before sending, the worker again before OCCT. An invalid value is `invalid_input` (the plan's `InvalidParameter`; the error codes are a protected contract, and `invalid_input` already means this), naming the primitive and the parameter, with a suggestion. Silico refuses some values FreeCAD accepts and builds something broken from, and lengths above 1e6 (a kilometre), where OCCT starts to measure some solids wrongly (the note lists them).
- A primitive's result passes the same validity check as an operation's: an invalid one is `invalid_result`.
- The typed core API, from the M0 review: `silico_core::kernel::KernelWorker` (`make`, `fillet`, `summarize`, `tessellate`) finds the worker (`SILICO_KERNEL_WORKER`, or next to the running executable, or one directory up from cargo's `deps`), matches each reply to its operation (stopping a worker that answers with the wrong kind), and validates meshes. It's the core's layer over one worker, which recompute and the pool will build on (the name leaves `Kernel` for M2.3's trait); `silico-view` uses it and no longer depends on `silico-protocol`. `DebugOp::Misbehave` makes the test worker send a broken mesh or the wrong kind of reply, to prove the kernel API refuses them.
- Tests (`crates/silico-kernel/tests/primitives.rs`): the analytic table above, plus the frustum (5, 2, 10), with volume, area, and center within relative 1e-9 (tighter than planned); every invalid value of every parameter (negative, NaN, infinite, at or below 1e-7, and zero except a cone's radius), a cone with no radius, and a torus whose tube crosses its axis, with zero requests sent; the worker's own refusal; pointed cones either way up; horn tori; a sphere too large to build; and the 64-case volume property over 1e-3 to 1e4.

### M1.4 – History capture

**Builds:** every operation returns raw history from OCCT's `BRepBuilderAPI_MakeShape`. For each input sub-shape, identified by type and index, it lists what the sub-shape was modified into, what it generated, or whether it was deleted. This is the input that topological naming (M2.7) will consume.

**Tests:**
- `fillet_history`: filleting one edge of a box reports that edge deleted, its 2 adjacent faces modified, 1 new face generated from the edge, and the 4 faces it doesn't touch unchanged.
- `cut_history`: cutting a cylinder through a box reports the top and bottom faces modified, and the cylinder's lateral face generating the hole's face.
- `history_indices_valid` (proptest over operations): every index in every history map is within range of its shape.

### M1.5 – Modeling operations

**Builds:** extrude (a face along a vector), revolve (a face around an axis by an angle), the booleans fuse, cut, and common, and fillet and chamfer on edge lists. Each returns the shape, its summary, and its history.

**Tests (analytic):**

| Test | Setup | Expected volume |
|---|---|---|
| `extrude_rect` | 10 × 20 rectangle extruded 5 | 1000 |
| `revolve_ring_full` | 2 × 10 rectangle at x = 3..5, revolved 360° about z | 160π |
| `revolve_ring_quarter` | the same, revolved 90° | 40π |
| `boolean_fuse` | 10³ box at the origin fused with a 10³ box at (5, 5, 5) | 1875 |
| `boolean_common` | the same two boxes | 125 |
| `boolean_cut` | the same two boxes | 875 |
| `box_minus_cylinder` | 10³ box minus an r = 2 cylinder through its center | 1000 − 40π |
| `chamfer_edge` | 10 × 20 × 30 box, chamfer d = 2 on one 30-long edge | 5940 |

Topology counts for the booleans come from the FreeCAD oracle, not from hand calculation.

**Failure-path tests** (each expects a specific structured error):
- extrude with length 0 or a direction in the face's plane;
- revolve with 0° or an axis that crosses the profile;
- a boolean with an empty operand;
- fillet or chamfer with an edge index out of range;
- overlapping fillets: r = 6 on all four edges around a face 10 wide.

After each expected failure, the next valid operation on the same worker must succeed.

### M1.6 – Worker pool

**Builds:** a pool of workers sized to the CPU count, with a request queue, per-operation timeouts, and respawning of crashed workers. Requests carry the document generation, and results for superseded generations are dropped. Crashes are never retried automatically, since the same input would likely crash again.

**Tests:**
- `concurrent_requests_correct`: 1,000 mixed operations submitted at once all return correct results.
- `kill9_mid_burst`: during a burst of requests, random workers are killed with `kill -9`. Each affected request gets `WorkerCrashed`, all other requests succeed, and the pool returns to full size.
- `stale_generation_dropped`: a slow request from generation 1 that finishes after generation 2 has started is discarded and never delivered.
- `pool_no_zombies`: after the kill test, the only child processes are the live pool workers.
- Benchmark: throughput of 10,000 small operations, recorded as the baseline.

### M1.7 – Tessellation and the Rerun debug viewer

**Already built in M0** (see m0-foundations.md, Debug viewer): `Op::Tessellate { shape, deflection }` replying `silico_protocol::Mesh` (positions, outward triangles, triangles per face, and a polyline per edge numbered as `Op::Fillet` numbers them), a deflection floor relative to the shape's size, and `silico-view`, which builds parts through a worker and logs them to Rerun (`pixi run view`, or `--save` to a recording).

**Builds:** extends that mesh rather than adding a second one: per-vertex normals from the surfaces (so the viewer stops estimating them from triangles), and the face index of every triangle, derived from the per-face counts (used later for picking). `silico-view` gains a library half that tests and scripts call to log a shape, instead of a separate `silico-debug` crate.

**Tests:**
- `box_mesh`: 12 triangles, every face index from 0 to 5 present, and all vertices inside the bounding box.
- `closed_solid_mesh_watertight`: for a box, a cylinder, and the box minus the cylinder, every mesh edge is shared by exactly two triangles.
- `cylinder_deflection_respected`: every mesh vertex lies exactly on the r = 5 surface, and the maximum sagitta between vertices is at most the requested deflection.
- Manual check, recorded in the PR: a Rerun screenshot of each test shape.

### M1.8 – Oracle harness

**Builds:** the `silico-oracle` crate. An oracle case describes an operation and its parameters. The harness runs each case through Silico and through FreeCAD, then compares volume, area, bounding box, topology counts, and validity. Cases live in `tests/oracle/`, which is protected. A generator produces random valid cases for broad coverage.

**Tests:**
- Every M1.3 and M1.5 operation has at least 3 oracle cases, including one near a tolerance boundary.
- `harness_catches_wrong_result` (a self-test): injecting a 0.1% volume error, or one extra face, makes the harness fail.
- `generated_cases` (nightly CI): 500 random cases match FreeCAD.

### M1.9 – Hardening

**Builds:** nothing new. Run `/break-it` on the kernel layer and the worker pool, using the catalog sections for numbers, features, workers and IPC.

**Tests:**
- `determinism_sweep`: every oracle case run twice on fresh pools gives byte-identical blobs and summaries. Once M0.9 lands, the same cases are bit-identical on the workstation too.
- `scrambled_hash_sweep`: the stress corpus run on a diagnostic OCCT build whose shape hashes are deliberately scrambled. Any result that changes reveals an algorithm whose output depends on hash order. Each one found becomes an upstream issue and, if needed, a patch.
- Every break `/break-it` finds is fixed, or recorded in this file as an accepted limitation with the reason.

### Carried over from the M0 milestone review

Raised by the review council and left for M1, where the code they concern is built:

- **Typed core functions.** Done in M1.3 (`silico_core::kernel::KernelWorker`); each new operation gets its typed function as it's added.
- **Shapes whose geometry isn't finite.** A B-rep with NaN written into a plane, with a checksum that matches (a badly stored or hostile file), summarizes as valid with a wrong volume (5000 for a 6000 box): `BRepCheck_Analyzer` doesn't look at the numbers. Check a shape's geometry is finite when it's read from outside a worker's reply (M1.9). Meshing such a shape is already refused.
- **The parameter at fault, structured.** An invalid primitive value names its parameter only in the message; agents should get it as data. That's an error-contract change (M2, with the error's `feature`).
- **Element references, not names, from workers.** The worker writes `Edge3` and decides which edge to blame. Have `Failure` carry the element's kind and index, and the core name it and apply the blame rule, ready for M2's stable names.
- **Finding the failing edge of a multi-edge fillet** belongs in the core: one command that tries the edges (in parallel across the pool) and returns the failing ones as `candidates`, instead of a suggestion to try them one at a time.
- **A crash inside OCCT's fillet.** Filleting an edge of a filleted box again with a much larger radius (Edge1 at r = 4, then Edge1 at r = 14.9) segfaults in `BRepFilletAPI_MakeFillet`. The worker contains it (`worker_crashed`), but the error has no suggestion and no element. Refuse radii that can't fit before calling OCCT, or give the crash a suggestion (M1.5, M1.9).
- **Small cleanups:** a test-only count of whole-shape validity checks, proving each shape is checked once; a unit test for `PatientReader` passing on read errors other than "would block"; porting the geometric half of FreeCAD's `testDeletingPreviousFeatureRelinksUniqueMatchingBaseEdge`.

