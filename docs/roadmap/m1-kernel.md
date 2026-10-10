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
- `fillet_history`: filleting one edge of a box reports that edge deleted and generating 1 new face, the 2 faces it joins and the 2 end caps modified (their outlines change), and the 2 faces it doesn't touch unchanged. (Corrected: the plan said 2 modified and 4 unchanged, which isn't what OCCT reports; see docs/ports/history.md.)
- `cut_history`: cutting a cylinder through a box reports the box's top and bottom faces modified, and the cylinder's lateral face modified into the hole's face (corrected: not generated).
- `history_indices_valid` (proptest over operations): every index in every history map is within range of its shape.

**As built (the fillet; booleans, extrude and revolve, and chamfer added theirs in M1.5):**
- The port note is [docs/ports/history.md](../ports/history.md): OCCT's history pitfalls (`IsDeleted` wrong for fillets, unchanged elements unreported, results not in the shape, coarse results, uncaptured end caps), and what Silico does about each.
- The bridge reports facts (each input element's match in the result by identity, and OCCT's `Modified` and `Generated`, each numbered in the result or 0 where the result doesn't have it); Rust builds the history (`ElementFate`: `kept`, `modified`, `generated`; deleted is neither kept nor modified). Operations on shapes reply `Reply::Built(BuiltData { shape, history })`, and `KernelWorker` checks a history before returning it: for as many inputs as the operation had, naming only elements the result has (else the worker is stopped). Each reported element keeps its position in OCCT's list (FreeCAD's names use it), and each list is sorted by element. The bridge's counting of positions is verified by booleans, against FreeCAD's `;:G2` and `K-3` markers (M1.5).
- Tests (`crates/silico-kernel/tests/history.rs`): `fillet_history_matches_freecads_element_names` reads FreeCAD's element names (`Face3;:M;FLT`, `Edge1;:G;FLT`, `Face2;:H`) as an oracle: every relation they state directly is in Silico's history, and the kept elements are exactly the same, for one edge, two meeting edges, and all twelve. `filleting_one_edge_keeps_modifies_and_generates_what_the_geometry_says` is the corrected `fillet_history`, from the box's geometry alone. `history_indices_valid` runs 48 fillets of random edge sets and radii. Also: the history the app receives through a worker is the kernel's; a worker whose history names what the result doesn't have is refused and stopped; histories are the same every time and survive the trip through B-rep bytes. `cut_history` came with booleans (M1.5).

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

**As built (booleans):**
- The port note is [docs/ports/booleans.md](../ports/booleans.md). `Op::Boolean { op: Fuse | Cut | Common, base, tool }` replies with the result and a history for both inputs (base is input 0). OCCT's Boolean Operations as FreeCAD calls them (non-destructive, so changed elements are reported as modified), but single-threaded and at OCCT's own fuzzy value (as PartDesign). Refine is deferred to its own step before M2. An empty input or an empty result is refused with a suggestion; an invalid input isn't (FreeCAD, `3f5a2ae6b8`), and the result check blames the boolean only when both inputs were valid.
- A valid result with volume is accepted as OCCT gives it, as FreeCAD does (a result of solids with no volume, which OCCT gives for some booleans it gets wrong, is refused as empty), though OCCT gets some booleans wrong without a warning (the port note lists them; Silico tried refusing them during review, and the maintainer chose FreeCAD's behavior over the cost). `summarize` integrates volumes and areas adaptively to 1e-9: OCCT's fixed-order default, FreeCAD's too, measured some boolean results 1% off.
- `Op::Translate { shape, offset }` places shapes for now (one location, the move composed with the shape's own; each element modified into its moved self; a move by nothing keeps everything).
- Tests (`crates/silico-kernel/tests/boolean.rs`): `boolean_fuse`, `boolean_common`, `boolean_cut` with areas, centers, and counts by hand; `box_minus_cylinder` with `cut_history`; FreeCAD's Part, PartDesign, and TopoShape boolean tests, and the recovered `testHistory`; fourteen booleans compared with FreeCAD on measures and on the history its element names state, positions included; refusals (an empty operand, an empty result, a bad offset), each followed by a working operation on the same worker; determinism, in-process and through a worker. `adversarial_boolean.rs` (from `/break-it`): identical, touching, tangent, nested, and split operands, tolerance-sized gaps and overlaps, scale, moves, and a property test of box booleans against interval arithmetic.

**As built (extrude and revolve):**
- The port note is [docs/ports/extrude-revolve.md](../ports/extrude-revolve.md). A profile is a closed, flat polygon (`Profile`), checked in `silico-protocol` (three to a thousand corners, finite, distinct, not crossing itself, enclosing an area, flat to 1e-7) so the core refuses a bad one before sending it. `Op::Extrude { profile, length, direction }` sweeps along the profile's normal, or along a direction far enough to rise the length along the normal (as SolidWorks and FreeCAD's Pad measure); `Op::Revolve { profile, axis, angle }` sweeps about an axis in the profile's plane, touching it at most, by more than 0 and at most 360 degrees. Each replies with the solid and the profile's history (`Edge1` from its first point to its second, `Vertex1` its first point), and `History::ends`, the faces the sweep starts and ends with (OCCT's `FirstShape` and `LastShape`), where FreeCAD guesses them.
- Tests (`crates/silico-kernel/tests/sweep.rs`, and the rules in `silico-protocol`'s `profile.rs`): `extrude_rect` (1000) and `revolve_ring_full` and `_quarter` (160π, 40π) with areas and centers; the ends and each side face found where the geometry puts them; FreeCAD's `FeatureExtrusion`, `FeatureRevolution`, and `makeElementPrism`/`makeElementRevolve` tests; concave, triangular, and tilted profiles; six sweeps compared with FreeCAD on measures and on the history its element names state, including the face it names the extrusion's far end; every refusal, in the core (never sent) and in the worker (which goes on); determinism.

**As built (chamfer):**
- The port note is [docs/ports/chamfer.md](../ports/chamfer.md). `Op::Chamfer { shape, edges, size, flip }` with FreeCAD's three kinds (`ChamferSize`: `Equal`, `TwoDistances`, `DistanceAngle`), `distance` measured along each edge's lower-numbered face, or its higher-numbered one when flipped (FreeCAD 1.0 and later). Sizes are checked in `silico-protocol` (distances above 1e-7 and at most a kilometre, an angle above 0 and below 180 degrees), so the core refuses a bad one before sending it. The result passes the fillet's check, and an invalid one is refused where FreeCAD returns it.
- Chamfer and fillet share an edge rule: an edge OCCT won't take (a seam, a degenerate edge, faces turning by less than 0.1 radian, one face or none, more than two) is refused, naming it and why, where OCCT skips it silently among others; the bridge asks OCCT which edges it took (`Contour`). Edges joining more than two faces, or ending (themselves or along the smooth chain OCCT follows from them) at a corner where solids touch, on which OCCT segfaults, are never built. The bridge reports facts and Rust decides (`dressing.rs`), and one refusal names every edge at fault. Their edges are a set, given to OCCT in increasing order, since OCCT's result depends on the order. A failure's suggestion offers a smaller size, another, or leaving the edge out, since OCCT doesn't say why it failed.
- Tests (`crates/silico-kernel/tests/chamfer.rs`): `chamfer_edge` (5940) with area and center; every kind and the flip against the edge's geometry; FreeCAD's `testChamferCubeToOctahedron`, `makeElementChamfer`, and `testOther`'s formula; a cylinder's rims by Pappus; 23 chamfers compared with FreeCAD on measures, and the history its element names state; every edge OCCT won't take, for chamfer and fillet; refusals in the core and the worker; determinism through the worker. `adversarial_chamfer.rs` (from `/break-it`): hostile sizes against the rule, every kind and flip on random boxes and convex prisms against the chamfer's corners by hand, concave edges, countersinks, chains along fillets, corners just either side of OCCT's tangency limit, solids touching along an edge or at a corner, and huge edge lists.

### M1.6 – Worker pool

**Builds:** a pool of workers sized to the CPU count, with a request queue, per-operation timeouts, and respawning of crashed workers. Requests carry the document generation, and results for superseded generations are dropped. Crashes are never retried automatically, since the same input would likely crash again.

**Tests:**
- `concurrent_requests_correct`: 1,000 mixed operations submitted at once all return correct results.
- `kill9_mid_burst`: during a burst of requests, random workers are killed with `kill -9`. Each affected request gets `WorkerCrashed`, all other requests succeed, and the pool returns to full size.
- `stale_generation_dropped`: a slow request from generation 1 that finishes after generation 2 has started is discarded and never delivered.
- `pool_no_zombies`: after the kill test, the only child processes are the live pool workers.
- Benchmark: throughput of 10,000 small operations, recorded as the baseline.

**As built:**
- `silico_core::pool::KernelPool`: one thread per worker (as many as the machine has cores, or a size given), taking requests from one queue. `submit(generation, request)` returns a `Pending<T>` whose `wait` gives an `Outcome<T>`: `Done(result)`, or `Superseded`. Superseded isn't an error code (the codes are a protected contract): it's internal to recompute, never something an agent sees.
- Requests are typed: `silico_core::kernel::KernelRequest<T>` holds an operation's values, checked when it's made (so a value that can't work never reaches a worker), and how its reply is read and checked. `KernelWorker` runs them on one worker (its typed methods are now one line each) and the pool on many, so the checks live in one place.
- A worker that crashes, hangs (past the request's timeout, 60 s unless `with_timeout` says otherwise), or sends a reply that can't be trusted is stopped, its request fails with a structured error, and a new worker takes its place at once. A request is never retried. A worker found dead between requests (killed from outside) is replaced before it's given one, and an idle slot checks its worker every second, so the pool returns to full size on its own.
- `advance(generation)` starts a newer generation: requests of older ones still waiting are dropped without running, and those running are stopped (their workers killed and replaced) once they've run `STOP_AFTER` from their start (50 ms; `set_stop_after` changes it): at once if they already have, and otherwise by the pool's stopper thread when they do, unless they finish first, their results discarded. Replacing a worker takes about 30 ms of wall time and CPU: stopping a request saves time when more than that is left (four 3 s requests kept a new generation waiting 2.8 s), and costs time when less is, as for most operations on simple parts (killing a 5 ms fillet with 3 ms left made the next request wait 33 to 58 ms, not 8 to 11). Stopping only at `advance` left a request superseded in its first 50 ms running to its end, however long (found by the review): a superseded request now holds its worker at most `STOP_AFTER` from its start, and the worker is then replaced. A slot's request ends under the lock that decides its outcome, so an `advance` just after a result never stops the worker that gave it (found by the review: one in about a hundred did). A request submitted for an older generation is superseded at once. A slot's state (idle, running a generation since a moment, or condemned) changes only under the slots' lock that stopping takes, and every path that ends a request replaces a condemned worker before it's given another, so stopping stale work never fails a current request (a race the review found, where `advance` landed between a slot marking a request running and checking its generation, did). One generation counter per pool, until M2 decides how documents share pools.
- A request runs in the tracing span it was submitted in, so its worker's call and logs trace back to the feature and the user action (invariant 9), as on one worker. A request that failed after it was superseded is logged, with its error: at warn, as a worker failing, unless the pool's kill found its worker alive (the failure its own stopping), at debug (`pool_logs.rs` tests this, and the span; `pool_tracing.rs` the worker call's span). The log, and freeing an outcome whose caller stopped waiting, come after the pool's lock is released, so a log that blocks or a large result doesn't hold up the pool (`pool_blocked_log.rs` tests the log; unit tests in `pool.rs` test the freeing, and that the outcome is sent before the lock goes). A worker that crashed an instant before the kill is told apart by the kill, which checks first whether the process has exited (found by the review: such crashes were logged at debug as stopped).
- Workers close every descriptor they inherit above their socket when they start, so nothing the app holds without close-on-exec (a library's, or a test runner's jobserver pipe) stays open in a worker.
- Workers start one at a time, process-wide (`Worker::spawn` holds a lock from making the socket pair until the process runs): on macOS the standard library marks new descriptors close-on-exec only after making them, so workers started together inherited each other's sockets and pipes, and a start could wait forever on a pipe a sibling held (found by `/break-it`). A pool takes 1 to 1,024 workers, finds its worker executable once, at start, and replaces a worker that has sent a reply no request asked for before giving it another.
- Dropping the pool stops it at once: its workers are killed through `WorkerKiller`, a handle that kills a worker from another thread and never signals a process that's been reaped (the worker's process handle is shared behind a lock taken for every kill and reap, and not held while a killed worker is waited for, so a kill is never held up behind a large worker's teardown; that last part is untested, since a test would need a worker slow to die), and every request still waiting or running ends with an error saying the pool stopped.
- A worker still in the moment of exiting (killed from outside an instant before) can't be told from a live one: a request given to it then fails as a crash, as one whose worker is killed under it does.
- Tests (`crates/silico-kernel/tests/pool.rs`, and `adversarial_pool.rs` from `/break-it`: descriptors workers inherit, drops racing a replacement, generations raced from many threads, every way a worker fails, leaks over many replacements, a binary deleted under a pool, sizes, and relative paths): `concurrent_requests_correct` (1,000 requests of eight kinds through a pool of four, each byte for byte the result one worker gives); `kill9_mid_burst` (240 requests while four workers are killed with SIGKILL: at least one and at most one request per kill fails, as a crash; the rest are right; the pool returns to four live workers), which also checks `pool_no_zombies` (the test process's only children are the live workers, none a zombie, and none of the pool's outlives it); `stale_generation_dropped`; requests running in parallel; a worker killed while idle failing no request; an idle pool replacing a dead worker on its own; a crash, a timeout, and an untrustworthy reply, each failing its own request and leaving a new worker; dropping the pool mid-request; a pool that can't start.
- Benchmark (`benches/pool.rs`, `pixi run cargo bench -p silico-kernel --bench pool`), the baseline on the maintainer's 14-core Mac: 10,000 empty echoes at 129,000/s through the pool against 23,800/s on one worker, and 10,000 10 × 20 × 30 boxes at 16,400/s against 1,000/s.

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
- **OCCT's libraries name a newer macOS than the binaries.** The linker warns that OCCT's dylibs were built for macOS 27 while Silico's executables say 11, so the executables wouldn't run on an older macOS. Set the deployment target in OCCT's recipe to match (M1.9, or when Silico is first run on another Mac).
- **In-process and worker results differ in the last bit.** A mesh made from a shape in memory and one made from the same shape after a trip through B-rep bytes differ by one unit in the last place in some coordinates; topology and element order don't change. Every worker operation starts from bytes, so worker results agree with each other, but in-process tests measure a hair-different shape. Find what the round trip changes (M1.9).
- **Shapes whose geometry isn't finite.** A B-rep with NaN written into a plane, with a checksum that matches (a badly stored or hostile file), summarizes as valid with a wrong volume (5000 for a 6000 box): `BRepCheck_Analyzer` doesn't look at the numbers. Check a shape's geometry is finite when it's read from outside a worker's reply (M1.9). Meshing such a shape is already refused.
- **The parameter at fault, structured.** An invalid primitive value names its parameter only in the message; agents should get it as data. That's an error-contract change (M2, with the error's `feature`).
- **Element references, not names, from workers.** The worker writes `Edge3` and decides which edge to blame. Have `Failure` carry the element's kind and index, and the core name it and apply the blame rule, ready for M2's stable names.
- **Finding the failing edge of a multi-edge fillet** belongs in the core: one command that tries the edges (in parallel across the pool) and returns the failing ones as `candidates`, instead of a suggestion to try them one at a time.
- **A crash inside OCCT's fillet.** Filleting an edge of a filleted box again with a much larger radius (Edge1 at r = 4, then Edge1 at r = 14.9) segfaults in `BRepFilletAPI_MakeFillet`. The worker contains it (`worker_crashed`), but the error has no suggestion and no element. Refuse radii that can't fit before calling OCCT, or give the crash a suggestion (M1.5, M1.9).
- **Booleans OCCT gets wrong without a warning** (docs/ports/booleans.md, Known wrong results): accepted, as FreeCAD accepts them, unless the result has no volume (refused as empty). Report them to OCCT, with the inputs the note lists (M1.9). Silico ran OCCT 8.0.1 until M1.5, and moved to 7.9.3 (its FreeCAD oracle's) when 8.0.1 turned out to have a regression 7.9.3 doesn't.
- **`summarize` mismeasures where its faces' integrals cancel** (from `/break-it` on extrude and revolve): a 1e-3 mm quadrilateral extruded to rise 1000 along (0, 4.6, 1) measures 6.1e-5 too much volume and its center 0.02 mm off, though its mesh's volume matches to 1.5e-10; a micron square revolved 2 km from the axis is 1.3e-4 off in volume. The 1e-9 precision is relative to each face's integral, far larger here than what they cancel to (M1.9).
- **`tessellate` fails on thin, valid solids** (from the same): a 100 × 100 square extruded 1e-6 (or a box primitive of that size) fails with "faces couldn't be meshed" and a suggestion that the geometry is broken; a 6e-4 diamond 1.1e-7 thick at z = 10 meshes with one triangle per side face, a mesh with holes that passes `Mesh::validate` (M1.7).
- **`summarize` is 5 to 10 times slower** since it integrates adaptively (a plate with 60 spherical pockets: 67 ms against 6.4 ms; 96 faces with torus grooves: 195 ms against 21 ms), about 2 ms a curved face. Fine for the M1 parts; before M1.6 makes recompute chains hot, measure it on large models, and consider carrying a shape's volume with it so it's measured once (M1.9).
- **A fillet radius below OCCT's tolerance** (from chamfer, M1.5): the fillet accepts any positive radius (1e-8 builds, as in FreeCAD), where the chamfer, primitives, and sweeps refuse lengths at or below 1e-7. Bring the fillet in line, with its oracle case (M1.9).
- **Every refused edge as data** (from the chamfer's review, M1.5): a fillet or chamfer refusal names every edge at fault, but only the first reaches `Failure.element` and the core `Error`; the rest are in the message and suggestion. Carry them as a list (an `elements` field on `Failure` and `Error`) before the feature layer or MCP consumes refusals. It changes the error contract (`tests/contract/`), so it needs the maintainer (M2).
- **OCCT across threads in one process** (from the same review): two fillet tests in `fillet.rs` fail intermittently under plain `cargo test`, whose tests share a process across threads, and pass one per process (nextest, which `pixi run test` uses) or with `--test-threads=1`. OCCT's fillet appears to depend on other OCCT work running at the same time in the process. Workers are single-threaded, so nothing ships this way; find the cause before anything calls OCCT from two threads (M1.9).
- **A worker test's one-second bound under load** (from the chamfer's review). Done in M1.6: the test guards that a request over the frame limit is refused before sending, not by waiting out the timeout, so its clock no longer includes building the 512 MiB request, and its bound is a quarter of the timeout.
- **Worker start, checked** (from the pool's `/break-it`): a pool doesn't confirm its workers answer (`start_workers("/usr/bin/true", 2)` succeeds, and every request then fails as a crash), and a fresh worker's first request includes the worker's start: macOS held some newly copied worker binaries at launch for 17 s and more, so a first request could time out with a suggestion that blames it. Consider a handshake at start, kept out of the request's timeout (M1.9).
- **One build per pool** (from the same): a worker binary rebuilt at the same path is used for later replacements, so one pool can mix worker builds, which bears on determinism (invariant 5). Pin the build a pool runs (a hash of the binary at start, say) when updates can happen under a running app (M5).
- **Queued requests hold copies of their shapes** (from the pool's review): every `KernelRequest` clones its input shapes, and `Blob` is an owned `Vec<u8>`, so a recompute that queues 2,000 requests on a 50 MB body holds 100 GB. Make shape data cheap to share (`Arc<[u8]>` or `Bytes` in `Blob`) before the recompute scheduler is built on the pool (M2).
- **Generations per document, and notification** (from the same): one generation counter per pool would let one document supersede another's requests, and recompute features an edit didn't touch; the M2 scheduler, which knows the dependencies, may want per-request cancel instead, or generations keyed by document. And `Pending` can only be waited on or polled: a scheduler waiting on several, or an egui client, will want a completion callback or a shared channel (M2).
- **The worker runtime, for every worker** (from the pool's review): closing inherited descriptors, the socket handshake, and the floating-point trap checks live in `silico_kernel::worker`, which links OCCT; M3's sketch worker can't use it without linking OCCT too. Move the runtime to a crate of its own, linking no OCCT, when the sketch worker arrives (M3).
- **Small cleanups:** a test-only count of whole-shape validity checks, proving each shape is checked once; a unit test for `PatientReader` passing on read errors other than "would block"; porting the geometric half of FreeCAD's `testDeletingPreviousFeatureRelinksUniqueMatchingBaseEdge`.

