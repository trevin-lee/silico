# Sketcher

Constrained 2D sketches solved by PlaneGCS in a worker, turned into faces, and driving pad, pocket, and revolve: milestone M3 (docs/roadmap/m3-sketcher.md). This note is the study that plans it. PlaneGCS is linked, not ported (CLAUDE.md); everything around it (the sketch model, the gate, what a solve means, faces, the features) is ported as behavior.

Sources: FreeCAD at `876271d660` (main, 2026-10-04) and the oracle's release, FreeCAD **1.1.3** (tag `145529fe74`, not in the maintainer's clone; its PlaneGCS was read from the release's source archive). Paths are relative to `src/Mod/Sketcher/App/` unless they say otherwise. On main, in 2026 (after 1.1; 1.1.3 has one `SketchObject.cpp`), FreeCAD split `SketchObject.cpp` into `SketchObject.cpp` (recompute, the shape, restore), `SketchObjectConstraints.cpp` (solve, constraint edits), `SketchObjectGeometry.cpp`, `SketchObjectExternal.cpp`, and `SketchObjectOperations.cpp` (editing tools); line numbers are main's.

**Main is not 1.1.3.** FreeCAD's main moved to 1.2 at `e8f826d0ed` (2025-11-13). Main has behavior the oracle doesn't: signed point–line, circle–line, and circle–circle distances and oriented tangents (`3c8a254356`, `f432d1c6a7`, `5a6be36bc3`, migration `e1e72c9195`), reference constraints evaluated after the solve instead of solved (`92894ea3a8`), a changed elimination threshold in the conflict analysis (`0b44a3f6c8`), point–point–line perpendicularity (`be1ec84709`), group and text constraints, new face makers (`24ab301685`, `7186d30f6e`), and a refactored, two-sided revolve. It also has a regression 1.1.3 doesn't (`a502762119`, below). Silico follows 1.1.3, as it follows 1.1.3's OCCT, and this note says where main differs.

## The layers

| FreeCAD | What it does | Silico |
|---|---|---|
| `planegcs/` (13,300 lines, LGPL-2.1-or-later) | The solver: parameters as `double*`, constraints as error functions with gradients, DogLeg / Levenberg–Marquardt / BFGS / SQP, and the QR diagnosis (degrees of freedom, conflicting, redundant) | **Linked** in the `silico-sketch` worker (M3.1), at 1.1.3 |
| `Sketch.cpp` | Builds a throwaway solver system from the sketch on every solve: parameters per geometry, solver constraints per sketch constraint, tags, the fallback chain, writing back | The worker's request handler (C++ only where it calls PlaneGCS) and the core's sketch model: which parameters, which constraints, in what order (M3.2–M3.3) |
| `SketchObject*.cpp`, `PropertyConstraintList.cpp` | The document object: what recompute does with the diagnosis, validation, the shape (wires), restore, deletion | The document's sketch feature, the validation gate, recompute (M3.2–M3.4) |
| `SketchAnalysis.cpp` | Advisory checks: missing coincidences, degenerate geometry, open vertices | Later, advisory only (never a fix-up) |
| `src/Mod/Part/App/FaceMakerBullseye.cpp` | Closed wires to faces with holes and islands | M3.5 |
| `src/Mod/PartDesign/App/FeatureSketchBased.cpp`, `FeatureExtrude.cpp`, `FeatureRevolution.cpp`, `FeatureGroove.cpp` | Profiles, pad, pocket, revolve, groove | M3.6 |

## PlaneGCS

### Building it

Five `.cpp` files (`GCS`, `Constraints`, `Geo`, `SubSystem`, `qp_eq`), and `Util.h`'s typedefs. It needs:

- **Eigen 3.4** (FreeCAD requires ≥ 3.4.0; the oracle's FreeCAD 1.1.3 bundles exactly 3.4.0). `Eigen/Core`, `QR`, `Dense`, `Sparse`, `OrderingMethods`. The `FullPivLU::compute` override in `GCS.cpp` only compiles for Eigen < 3.3 and is dead with 3.4.
- **Boost, headers only**, for one call: `boost::connected_components` over an `adjacency_list`, splitting the system into independent parts (`GCS.cpp` `initSolution`). Two other Boost includes are unused. The oracle bundles Boost 1.86.
- **Stubs for four FreeCAD headers:** `SketcherGlobal.h`/`FCGlobal.h` (an export macro), `Base/Console.h` (`log` and `warning`; even at the default debug level, every redundancy solve logs two lines), `FCConfig.h`, and on main `Base/Tools.h`. No Qt, OCCT, or Python.
- **C++20** for 1.1.3 (main needs C++23 for `std::unreachable`), with `NDEBUG` as FreeCAD's release builds have it, and `-pthread` on Linux (below).

### Its API, as Silico uses it

- Parameters are raw `double*` the caller owns. Geometry structs hold pointers: `Point{x, y}`, `Line{p1, p2}`, `Circle{center, rad}`, `Arc` (circle plus `startAngle`, `endAngle`, and `start`, `end` points tied to them only by `addConstraintArcRules`), `Ellipse{center, focus1, radmin}` (centre, a focus, and the minor radius, not axes and an angle), arcs of ellipse, hyperbola, and parabola, and `BSpline` (poles, weights, knots, multiplicities, degree, periodic).
- Each `addConstraint*(…, tag, driving)` adds error rows. **The tag is the protocol**: a tag ≥ 1 is a sketch constraint, and the diagnosis reports conflicting, redundant, and partially redundant constraints as sorted lists of tags. **Tag 0 is never reported** (GCS.cpp's popularity contest skips it), so FreeCAD uses 0 for implicit rows (the arc rules) and Silico's tags start at 1. Negative tags are temporary, lower-priority rows (dragging, the SQP fallback). `driving = false`, with the value pointer passed to `declareDrivenParams`, makes a reference constraint.
- The life cycle is `declareUnknowns` → `initSolution` (saves the starting values, diagnoses, splits into independent parts, and substitutes parameters for every tagged `Equal`, so coincidences cost nothing) → `solve` (each part from the saved starting values, never from a previous attempt) → `applySolution` or `undoSolution`. `diagnose()` called on its own can leave parameters changed; always go through `initSolution`.
- **Defaults:** DogLeg, at most 100 iterations, convergence 1e-10 (DogLeg succeeds when the largest residual is ≤ 1e-10), QR pivot threshold 1e-13, dense QR below 1000 parameters and sparse (COLAMD) above (`c8106982de`, in 1.1.3 as `b7e3bb1f7c`: the sparse QR misjudged rank on aligned slots).
- **Status:** `Success`, `Converged` (minimized, but the error isn't zero: a failure), `Failed`. A redundant constraint whose error stays above convergence downgrades `Success` to `Converged`.

### Diagnosis: degrees of freedom, conflicting, redundant

`diagnose()` (GCS.cpp) builds the dense Jacobian of the driving rows with tag ≥ 0 (tag-0 rows, an arc's rules, count toward the rank, which is why an arc has 5 degrees of freedom; they're only never removed or reported) and takes a full-pivot Householder QR of its transpose: degrees of freedom = unknowns − rank. When there are more rows than the rank, it groups the dependent rows, and a **popularity contest** removes, from each group, the constraint in the most groups (ties: the one with fewer rows, then the higher tag; every row of a removed tag goes together, `b626caffdc`; tag 0 and internal-alignment rows are never removed, `bffb82d531`, `aa67808f24`). It solves the rest. A removed constraint whose squared error is then below 1e-10, so whose error is below **1e-5**, is **redundant** (`GCS.cpp:5588` in 1.1.3, `convergenceRedundant`); the groups without one are **conflicting**, every tag in them reported. If that solve itself fails, nothing is redundant and every group is conflicting. So FreeCAD calls two dimensions that disagree by under 1e-5 (a length of 10 and a horizontal distance of 10.000005 on a horizontal line) redundant, not conflicting. A constraint with several rows (a coincidence is two) of which only some were redundant is **partially redundant** (`65fb44df6c`). When over-constrained, degrees of freedom = unknowns − non-redundant rows (negative). No unknowns gives −1; no driving rows gives 0.

A second QR, of the untransposed Jacobian, finds the **dependent parameters** (which coordinates the constraints don't fix): FreeCAD colours geometry "fully constrained" from them. FreeCAD runs it on a `std::async` thread, joined before its result is read.

### Determinism

Recompute must be bit-identical on macOS ARM and Linux x86 (ARCHITECTURE.md, invariant 5). PlaneGCS gets the deterministic recipe (`-ffp-contract=off`, the libm shim, the pinned clang and libc++) and `EIGEN_DONT_VECTORIZE`, as ARCHITECTURE.md already plans. Beyond the recipe:

| Risk | Where | What Silico does |
|---|---|---|
| **The order of unknowns is the order of their addresses.** `SubSystem::initialize` intersects two `std::set<double*>`, and the SQP solver sorts its parameters by pointer. That order is the Jacobian's column order, which decides pivoting ties. FreeCAD allocates each parameter with its own `new double`, so its own order depends on the allocator. | `SubSystem.cpp:59-70`, `GCS.cpp:4542` | The worker allocates every parameter (geometry, constraint values, the SQP copies) from one array, reserved once, in Silico's canonical order. Address order is then index order everywhere. |
| **Eigen's matrix-product blocking depends on the CPU.** Eigen 3.4 reads cache sizes by CPUID on x86 and uses fixed defaults on ARM, so a product deeper than about 336 (Levenberg–Marquardt's `JᵀJ` on a sketch with that many constraints) sums in a different order. The `EIGEN_DEFAULT_L*` macros only change the fallback. | `Eigen/src/Core/products/GeneralBlockPanelKernel.h` | `Eigen::setCpuCacheSizes` with fixed values when the worker starts, and a stress case large enough to cross the threshold. |
| `diagnose()` starts a thread (`std::async`, default policy) for the dependent parameters. It's joined before its result is read, so results don't depend on scheduling, but ARCHITECTURE.md wants workers single-threaded. | `GCS.cpp:4938, 5015` | Patch to `std::launch::deferred`: a single-threading patch, not a determinism one. OCCT's patch policy doesn't cover PlaneGCS; extending it is open decision 7. |
| Pointer-keyed sets and maps elsewhere (`redundant`, `conflictingMap`, `pIndex`, `c2p`) | `GCS.h` | Only looked up, or their output is re-sorted by tag: no effect. Checked by the stress corpus, not assumed. |

No randomness, time, `unordered_*`, or OpenMP. The oracle's FreeCAD.app is built with NEON and fused multiply-add, so Silico's results differ from the oracle's in the last bits: oracle comparisons use tolerances, as for OCCT.

### Failure modes the worker guards

- **No input checks.** NaN and infinity go straight in; degenerate geometry (a zero-length line, a zero radius, repeated knots, concentric circles in a tangency before `86d5eea787` / `7023e83843`) gives NaN by division by zero; every solver then stops and reports failure. B-spline indexing is unchecked, with undefined behavior for a periodic spline whose first multiplicity exceeds degree + 1 and a `size_t` underflow under two points (`Geo.cpp:830-857, 1118-1140`; one such crash fixed in `e7e410323e`). The core refuses non-finite and degenerate input before a request is sent (the validation gate, M3.2), and the worker refuses non-finite output.
- **No `throw`, but exceptions anyway:** `std::map::at` in the conflict analysis, `bad_alloc` (the diagnosis Jacobian is dense, constraints × unknowns, and dense QR is O(n³)), and asserts (gone under `NDEBUG`). The worker catches everything at its boundary; anything else is a crash, which the core already turns into an error.
- **Long solves:** every loop is bounded (100 iterations, the inner Levenberg–Marquardt loop 50, the popularity contest by its groups since `aa67808f24` fixed an infinite loop), but the diagnosis and the four-solver fallback chain can take seconds on a large sketch. The pool's timeout applies.
- **`addConstraintProportional` is half right.** Its error, `p1 − ratio·p2`, is right, but its gradient for `p2` ignores the ratio, and `initSolution` substitutes any tagged one whose two parameters are both unknowns as a plain equality. FreeCAD builds `Diameter` with it (radius = 0.5 × diameter, `GCS.cpp:1169-1176` in 1.1.3). A driving diameter is a fixed value, so only the radius's gradient counts and it's right; a reference diameter is an unknown, gets substituted as diameter = radius, and FreeCAD doubled it back when writing it out (`727dc9fa9e`, until main evaluated reference values instead). Silico sends a diameter as a radius of half its value (exact in binary), and never uses `Proportional`.
- **Main's status regression:** `a502762119` (2026-09) made `System::solve` assign each independent part's status instead of keeping the worst, so a failed part followed by a successful one reports success. 1.1.3 doesn't have it. The worker checks every constraint's residual after a solve anyway.

## The sketch model (Sketch.cpp, Constraint.cpp)

### Geometry and its parameters

| Kind | Parameters, in FreeCAD's order | Implicit rows (tag 0) | Degrees of freedom |
|---|---|---|---|
| Point | x, y | | 2 |
| Line segment | start x, y; end x, y | | 4 |
| Circle | centre x, y; radius | | 3 |
| Arc of circle | start; end; centre; radius; start angle; end angle | arc rules: start and end on the circle at the angles (4) | 5 |
| Ellipse | centre; focus 1; minor radius | | 5 |
| Arc of ellipse, of hyperbola | start; end; centre; focus 1; minor radius; two angles | arc rules (4) | 7 |
| Arc of parabola | start; end; vertex; focus 1; two angles | arc rules (4) | 6 |
| B-spline | poles; weights; start; end. **Knots aren't unknowns.** | start and end coincide with the first and last poles only for a non-periodic spline whose end multiplicities exceed the degree (`Sketch.cpp:1584` in 1.1.3) | 3 per pole, plus 2 for each end that isn't tied (so 3n + 4 for a periodic spline: its end points are always unknowns, `Sketch.cpp:1529, 1542`) |

- **Arcs reach the solver counter-clockwise** in the sketch plane (`getStartPoint(emulateCCW)`, `getRange(emulateCCW)`, from `0767cd35f8` after reversed arcs broke), angles in radians with end > start. **The solver never normalizes angles:** if a solve leaves the end angle below the start angle, writing it back gives the complementary arc, and equal angles give a full circle. Nothing in FreeCAD detects this "arc flip".
- **Ellipses** are (centre, focus, minor radius): the major radius is written back as √(|focus − centre|² + b²). A solution whose minor radius passes the major one makes OCCT refuse the ellipse, and the solution is rolled back (`e423c308af` fixed the order of the setters).
- **B-splines need a second solve** after write-back, since some of their constraints read OCCT state (`0641e243e0`). FreeCAD also nudges a lone weight of exactly 1 to 0.99 × another before solving (`80e7ece429`), so OCCT doesn't renormalize: a heuristic Silico won't copy.
- **Construction geometry is solved like any other** and only left out of the shape.
- **External geometry, and the sketch's axes and origin, are fixed parameters**, not constraints: no arc rules or B-spline end rows, which would over-constrain them (`c6de527391`, `adfcd154c6`). FreeCAD's ids: `H_Axis` −1 (its start is the root point), `V_Axis` −2, externals −3 and below.
- **Internal-alignment geometry** (ellipse axes and foci, a parabola's focus and focal axis, B-spline control points as construction circles whose radius is the weight, knot points) is ordinary geometry tied to its parent by an `InternalAlignment` constraint with an index. FreeCAD only `assert`s those indexes (`Sketch.cpp:4611, 4640`): undefined behavior in a release build.

### Constraints

`ConstraintType` (`Constraint.h:52-77`): `Coincident, Horizontal, Vertical, Parallel, Tangent, Distance, DistanceX, DistanceY, Angle, Perpendicular, Radius, Equal, PointOnObject, Symmetric, InternalAlignment, SnellsLaw, Block, Diameter, Weight` (and on main `Group`, `Text`). A constraint names up to three elements, each a geometry and a position (`none` the edge, `start`, `end`, `mid` the centre), plus a value, `isDriving`, and `isActive`.

**A constraint's meaning is its type and the shape of its references.** `Distance` alone is point–point, point–line, point–circle, circle–circle, circle–line, a line's length, or an arc's length, chosen by which elements and positions are set. `DistanceX` with one element and a position fixes a coordinate. `Tangent` with positions is endpoint-to-curve or endpoint-to-endpoint. `Angle` on one line is its direction, on two lines the angle from the first to the second (counter-clockwise positive), and on one arc its sweep (`6a3437e28e`). A combination no branch handles returns −1, which FreeCAD reports as "malformed". Silico's format makes each form explicit and the gate refuses what doesn't fit, so the solver never sees a malformed constraint.

Values: lengths in mm, angles in radians (Silico's file stores degrees, converted at the worker boundary), signed where the type says so: `DistanceX` is x(second) − x(first), and angles are counter-clockwise. Radius, diameter, and weight must be positive and a distance non-zero, but FreeCAD checks that only when a value is edited (`setDatum`, `85b2fec8f6`), never when a constraint is added.

**Choices made from the current geometry.** These make the same document solve differently from different starting coordinates, so Silico stores each choice in the constraint:
- An endpoint tangency or perpendicularity stores which of its two directions it means in its value (tangent −π/2 or +π/2, perpendicular π/2 or 3π/2). The value 0 (files from before `3838bddcdb`, 2014) means "decide from the geometry at every solve". FreeCAD decides once when the constraint is added (`AutoLockTangencyAndPerpty`).
- A line–circle tangency and point–line, circle–line, and circle–circle distances keep the side they started on in 1.1.3 only because the solver starts there. Main stores the side (`Orientation`) and makes the distances signed. 1.1.3's PlaneGCS can't hold a side through a large change (`testCircleToLineDistanceOriented` is main's test of exactly that).
- A symmetry of an arc's endpoints about a line through its centre adds only the midpoint row (`365af33fc6`), decided from the current geometry.
- B-spline curve parameters are seeded from the closest point.

### Reference constraints, inactive constraints

A reference (driven) constraint's value is an output: in 1.1.3 its value is an unknown (`bc85670ce9`, 2015), and since `d4bfa82161` (2025-11, in 1.1.3) reference constraints sit in the lower-priority subsystem, so any independent part with both driving and reference constraints is solved by SQP; on main it's evaluated from the solved geometry (`92894ea3a8`). It never counts toward degrees of freedom or conflicts (`96f6c46949`, `7055b5ef0f`). A driven angle is written back reduced with `fmod(v, 2π)` (`ba78b194b3`, after two other reductions). An inactive constraint, and a `Block`, adds no rows.

### Tags

**One sketch constraint is one tag: its index + 1, so reports name constraints.** FreeCAD keeps a running counter instead, and that broke the mapping at least four times: untagged coincidences (`7cbb16f2e8`), a knot alignment that didn't count (`bffb82d531`), a parabola axis that counted twice (`cdf0097dec`), and skipped constraints (`20a83b82f6`). Main still shifts every later tag when a constraint is malformed. Silico sets each tag from the constraint's position, never a counter.

### Solving, and what a solve means

FreeCAD's `SketchObject::solve` (`SketchObjectConstraints.cpp:78-183`) diagnoses first and then:

| Outcome | When | Solved? | Recompute |
|---|---|---|---|
| Over-constrained | degrees of freedom < 0 | no | error, with the conflicting constraints |
| Conflicting | conflict groups | no | error, with them |
| Malformed | a constraint the solver couldn't build | no | error, with them (`150e50c700`, `2bd2041bfe`: they used to be ignored) |
| Redundant | redundant constraints, none of the above | yes, but **the geometry isn't written back** | error, with them |
| Solver failure | DogLeg, then Levenberg–Marquardt, BFGS, and SQP (anchoring every unknown at its start with tag −1) all fail to reach `Success` | | error |
| Invalid solution | a solve succeeded but OCCT refuses the geometry it describes; rolled back, next solver tried (`fc128d357d`, `2b88396504`) | | error if every solver ends there |
| Success | | yes | the geometry is written back, and "fully constrained" is degrees of freedom = 0 (`b7945b6a15`) |

Partially redundant is a warning only (`b01313c0d8`). On an error FreeCAD keeps the previous shape; Silico's recompute fails the feature (SolidWorks' behavior, and recompute's since M2.4). Every attempt starts from the stored coordinates, and a failure leaves them as they were. Redundant constraints are never deleted on recompute; FreeCAD's auto-removal is an editor preference, off by default.

**Silico differs on redundancy:** SolidWorks solves an over-defined sketch whose extra dimensions agree and marks them, and FreeCAD's refusal to write back a solve it did makes a model's result depend on whether a constraint is spelled twice. Silico follows SolidWorks: a sketch whose only problem is redundancy solves and keeps its result, and the redundant constraints come back as data (a warning naming them). Since PlaneGCS calls constraints redundant when they disagree by up to 1e-5, keeping its result would quietly take one of two dimensions that disagree: Silico's own tolerance for "agree" is open decision 5 (docs/roadmap/m3-sketcher.md).

### Dragging

`initMove` diagnoses once, adds tag −1 rows pinning the dragged element to the cursor (centre pins weighted 0.01 so an edge moves before its centre, `4d7a9435c1`), then each mouse move rewrites the targets and runs DogLeg only, always from the state at `initMove`; it re-initializes after drifting 20× the first step, refuses a sketch with conflicts, and clears the temporary rows before any normal solve (`751929c87b`, `eff8529ec6`). Bugs not to copy: a `size_t` underflow writing out of bounds in relative mode (`Sketch.cpp:5495`), a B-spline centroid that averages every other pole, and dangling constraint pointers after hiding constraints mid-drag (`51b37da53e`). In Silico a drag is a session in the worker (begin, move, end), the final result a normal solve: M3.7 measures its latency.

## The sketch feature (SketchObject)

- **The shape:** the solved, non-construction geometry, in geometry order; edges joined into wires greedily (the first edge that connects, at 1e-7; `TopoShapeExpansion.cpp` `makeElementWires`), each wire passed through `ShapeFix`; non-construction points become free vertices; one wire, or a compound. An empty sketch has an empty shape and no error; an edge shorter than 1e-7 only warns. Bugs fixed: the shape built from stored instead of solved geometry (`d2637ec881`), every edge added twice (`46a11b6538`), null edges from degenerate geometry and a crash on lone points (`4e5f0a3850`, `c8046b1c60`).
- **Validation in FreeCAD is weak.** `evaluateConstraint` checks only that geometry indexes are in range (`SketchObjectConstraints.cpp:2172-2233`; `7d8b1f30c3`, `6a211b9dbe`, `6a1cb47e30`, `d83c02387c` after crashes). Kinds, positions, and arity reach the solver and come back malformed. Worse, while any index is out of range `Constraints.getValues()` returns **an empty list** (`PropertyConstraintList.h:106-111`, `c35d8be9a3`), so the sketch solves with no constraints at all and only a console line says why. Silico's gate refuses instead, naming the constraint and the reference: every reference exists and is of a kind and position the constraint's form accepts, internal-alignment indexes are in range, values are finite and in range, and a driving dimension names at least one geometry of the sketch's own (`3d7871fca5`).
- **Restore is lenient.** Unknown constraint types are dropped, shifting later indexes (`462ec49297`); exceptions are swallowed; duplicate geometry ids are renumbered; a missing external reference keeps its constraints without error (`27ca64a201`). Silico's strict reader refuses each of these, except that a broken external reference is an error naming it, constraints kept.
- **Deleting geometry** (an editor operation, but the behavior SolidWorks users expect): constraints on it go, later indexes shift down (`0016d6f2f8`, `aeabffeed8`: every element slot, not just the first two), and coincidences through a deleted point are re-pointed so its neighbours stay joined (`SketchObjectConstraints.cpp:1036-1247`). With stable IDs Silico doesn't renumber, but keeps "neighbours stay joined" when the command API deletes.
- **Names:** the sketch's own names are `Edge<n>` counting construction geometry, `Vertex<n>`, `ExternalEdge<n>`, `H_Axis`, `V_Axis`, `RootPoint`; the shape's indexed edges follow wire assembly, so FreeCAD's later features name them through the element map (`Edge<n>` with the `SKT` op code). FreeCAD reuses a geometry id when a new curve's endpoints match a deleted one (`geoHistoryLevel`): a heuristic. Silico names sketch edges from their stable IDs (M2.7).

Out of scope for M3: the editing tools (trim, extend, split, fillet, offset, mirror, arrays, carbon copy), external geometry projected from other features (it needs naming), `Block`, `SnellsLaw`, and the `SketchAnalysis` checks.

## Profiles to faces (FaceMakerBullseye)

PartDesign makes faces with **FaceMakerBullseye** (`AllowMultiFace`, set for every new feature, `FeatureSketchBased.cpp:144-147`; the older FaceMakerCheese has no islands and only serves old files; the switch was `7786e24fcd`, #6366):

1. Every wire must be closed: OCCT's `BRep_Tool::IsClosed`, a **vertex-parity** test, so a T-junction or two loops sharing an edge (one wire after the greedy assembly) is "Wire is not closed.", and a figure-8 passes.
2. The plane is fitted (`BRepLib_FindSurface`), "Wires are not coplanar" if none fits; its normal's sign is arbitrary. PartDesign takes the extrusion direction from the sketch's placement, never from the face.
3. Wires are sorted largest first by the squared diagonal of their bounding box, with a tolerance (1e-7) that isn't a strict weak order (`std::stable_sort` keeps tied wires in input order).
4. Each wire, in that order, is classified against the faces made so far by **one vertex** (`FaceDriller::hitTest`): on a face's material it becomes a hole there; inside a hole it becomes a new face (an island); otherwise a new face. That's even–odd nesting to any depth. The loop runs newest first without stopping at a hit, so the oldest face that hits wins. A wire sharing a vertex with a face's outer boundary is "no hit" and becomes an overlapping face; one sharing a vertex with a hole is a hit, another hole of that face (`2161725848`; 1.1.3 `FaceMakerBullseye.cpp:132-147`, the hole case `:239-250`).
5. Outer wires are made counter-clockwise about the plane and holes clockwise, whatever the input's orientation (`getWireDirection`; `9c17fe359b` fixed a crash when its face failed).
6. Stand-alone points are ignored (`26ebefd651`). An empty result is "Cannot make face from profile". Several disjoint regions give several faces, and a pad then makes several solids.

**FreeCAD never checks for wires that cross, overlap, or touch.** Classifying one vertex gives either an invalid face (a warning, after `ShapeFix`) or two faces that pad later fuses into their union. SolidWorks refuses such a contour. Silico refuses it as data, naming the sketch elements that cross or touch: a deliberate divergence. Silico also passes the sketch's plane rather than fitting one, and sorts by an exact key (the diagonal, then the index).

**Region profiles (a sketch's internal faces).** FreeCAD has a second path. With a sketch's `MakeInternals` on (1.1.3: off by default, `SketchObject.cpp:168`; main: on for new sketches since `be1d53cf5f`, 2026-03), the sketch also builds `InternalShape`: crossing and overlapping geometry split into regions, `InternalFace1..n` (1.1.3: `FaceMakerRing` over a `WireJoiner`, `:485`; main: `FaceMakerBuildFace`, `24ab301685`, after the old pipeline missed regions where three or more shapes overlap, and `0939408c21` for self-intersecting B-splines and dangling edges). A pad can take one region as its profile (`Profile = (sketch, ["InternalFace1"])`, resolved in `FeatureSketchBased.cpp:216-246`). The oracle confirms it: two overlapping rectangles, (0,0)–(20,10) and (10,5)–(30,15), give three regions of 50, 150, and 150 mm², and a pad of a 150 mm² region by 5 is a valid 750 mm³ solid. Fusion picks profile regions the same way, and SolidWorks has Contour Select. So refusing crossing profiles (below) is the whole-sketch profile's rule; choosing regions of a crossing sketch is a separate, later capability (open decision 8, docs/roadmap/m3-sketcher.md). Main's `TestSketchInternalFaces.py` (45 tests: T-junctions, figure-8 B-splines, cross patterns, dangling chains, three overlapping circles, names stable after recompute) is its test suite.

FreeCAD's tests (`tests/src/Mod/Part/App/FaceMakerBullseye.cpp`, added in `88c80b3ed8`, 2026-04, after 1.1 but testing behavior 1.1 has) are analytic: a 4 × 3 rectangle (12), with a hole of radius 0.5 (12 − π/4), several holes, nested islands (two faces, 100 − 16π + 4π), 25 holes in a 100 × 100 square (10000 − 25π), a tilted plane, reversed wires. None tests an error.

## Pad, pocket, revolve (PartDesign, 1.1)

- **Direction:** pad along the sketch's normal, pocket along its negation; `Reversed` flips the final direction once (`1da079b34b`). Length must not total zero (`|L + L2| < 1e-7`, "Cannot create a pad with a total length of zero.", `6167e9f77a`); negative lengths extrude backwards (the per-side minimum was removed in `b2520d489a`). SolidWorks refuses a depth ≤ 0 and offers reverse; Silico follows SolidWorks.
- **Modes:** one side; symmetric (**one** prism from −L/2 to +L/2, not two fused, `c384a52fa5`); two sides (one prism from +L back to −L2, `a01cab21cc`); through all for pocket (2.02 × the diagonal of the base and the profile's box, `43de589732`); up to face and up to first later.
- **Pocket with no base** silently becomes its tool. Silico refuses it.
- **Revolve modes in 1.1.3:** revolution has `Angle`, `UpToLast`, `UpToFirst`, `UpToFace`, `TwoAngles` ("Angles of revolution nullify each other" when they sum to zero); groove has `Angle`, `ThroughAll`, `UpToFirst`, `UpToFace`, `TwoAngles`. A revolution's `UpToLast` is its `ThroughAll` (`FeatureRevolution.h:86-94`): a full 2π, midplane ignored. Only up to first is refused ("not yet supported"). M3.6 builds `Angle` and symmetric; the rest wait. Main's two-sided revolve with signed angles is newer.
- **Revolve:** angle in (0°, 360°] ("too large", "too small" below 1e-12 rad, `3694da3102`, `c0de42335b`); right-handed about the axis, which `Reversed` reverses (a profile at +X about +Y turns toward −Z); symmetric turns the profile by −angle/2 first. The axis is a sketch axis (`H_Axis`, `V_Axis`), the n-th construction line, an origin axis, or a line or circle edge; it must not be parallel to the sketch's normal (|cos| at 1e-12, both senses, `501c3dc657`, `fdf66b3e88`); it may touch the profile or run along an edge, but not cross it ("Revolve axis intersects the sketch", `8371982dfc` and the 2012 refinements). Groove is a revolve that cuts.
- **The result** fuses with or cuts the base; the result must hold a solid, every solid counted (`eb52dd4a9a`).
- **Refine** has been on by default since `e490531510` (2024): oracle comparisons of face and edge counts set it explicitly.

## Where Silico differs on purpose

- PlaneGCS at 1.1.3, the oracle's, not main: none of main's signed constraints, and not its status regression.
- Constraint forms explicit in the file, refused at the gate when they don't fit; never "malformed" at solve time; never a hidden, unconstrained solve.
- Every choice FreeCAD makes from the current geometry (tangency direction, sides) stored in the constraint.
- Tags from constraint positions, never a counter.
- Redundant-only sketches solve and keep their result, the redundancy reported (SolidWorks), where FreeCAD fails the feature, with Silico's own tolerance for constraints that agree (open decision 5).
- Crossing, overlapping, and touching profile wires refused, naming the elements.
- Pad and pocket depth > 0, with `reversed`; pocket without a base refused.
- No heuristics: no weight nudge, no geometry-id reuse by endpoint matching, no auto-removal, no fix-up of faces.

## Tests to port

FreeCAD's own tests barely test solving: there are two PlaneGCS tests, none of the translation layer, and **none that asserts a failing status, a conflicting or redundant list, or degrees of freedom**. Silico writes those from hand counts and confirms them through the 1.1.3 oracle (headless FreeCAD's `sketch.solve()`, `.DoF`, `.ConflictingConstraints`, `.RedundantConstraints`, `.PartiallyRedundantConstraints`, `.MalformedConstraints`), as protected oracle cases.

| FreeCAD | What | Silico |
|---|---|---|
| `tests/src/Mod/Sketcher/App/planegcs/Constraints.cpp` `tangentBSplineAndArc` | Arc and B-spline tangent through a point | M3.3 (B-splines aren't in M3.1's requests), through the worker, with the angle fixed (FreeCAD's lists it as an unknown, so it isn't imposed) and tangency checked against a derivative computed independently, not at FreeCAD's 0.005 |
| `SketcherTests/TestSketcherSolver.py` `CreateRectangleSketch` | Rectangle: 4 lines, 4 coincidences, 2 horizontal, 2 vertical, a corner's x and y, 2 distances | M3.1 (solves; corners analytic), M3.3 (degrees of freedom 0) |
| `TestSketcherSolver.py` `testBoxCase`, `testSlotCase` | `testBoxCase` asserts nothing: its corners after two datums, (90, −50) and (−97.573036, 31.370787), are derived by hand. `testSlotCase` asserts 4 edges, then 9; its apex (67.1279195, 80) is derived | M3.3, the derived values confirmed by the oracle |
| `TestSketcherSolver.py` `testIssue3245`, `_2`; `SketchObject.cpp` `testConstraintAfterDeletingGeo` | Deleting geometry drops its constraints and keeps the rest | M3.2's gate (stable IDs), the command API later |
| `TestSketchValidateCoincidents.py` `testValidateConstraintsCase` | A constraint naming missing geometry | M3.2: the gate refuses it, naming it |
| `TestCoincidentCommandGui.py` fixtures (`cb2780e2fa`, 2026-09) | A coincidence that conflicts (conflicting 1, 2, 3; degrees of freedom 5), a horizontal stated twice (redundant: the later), coincidences collapsing a line (solved, then refused as invalid geometry) | M3.3: the solver's outcomes, pure solver fixtures despite the file's name |
| `TestSketcherSolver.py` circle–line distance (`:530`, `:561`, `:589`) | A driving distance of 10 measures 10 ± 1e-7; a reference distance reads back +10 | M3.3 |
| `TestSketcherSolver.py` oriented and signed distances and tangents (`:618`, `:675`, `:715`, `:793`) | Sides kept through a 10× change | M3.3, if signed constraints come (main's, not 1.1.3's: open decision 1, docs/roadmap/m3-sketcher.md) |
| `TestSketcherSolver.py` `testBlockConstraintEllipse` | Block | Later; FreeCAD's test re-checks a stale status four times |
| `TestSketcherSolver.py` `testConstructionToggleTNP` (bug 15484) | Toggling construction lines changes a pad's faces; a sketch attached to `Face6` must follow to `Face9` | Waits for sketches attached to faces, which no milestone plans yet (FreeCAD's `Attacher`); M3.6 tests its essence with what it has: a later feature naming a pad's face or edge keeps it when construction lines in the pad's sketch are toggled |
| `tests/src/Mod/Sketcher/App/SketchObject.cpp` `testGetPointFromGeom*` (10 tests) | What `start`, `end`, and `mid` are for each kind of geometry, including arcs of conics and periodic and non-periodic B-splines | M3.2: what a point reference means, for each kind M3.2 has |
| `SketchObject.cpp` `testGeoIdFromShapeType*` (6 tests), `testGetElementName` | The names `Edge<n>`, `Vertex<n>`, `ExternalEdge<n>`, `H_Axis`, `V_Axis`, `RootPoint` | Not ported: Silico names elements by stable ID (M2.7), and imports FreeCAD's names only in the FreeCAD importer |
| `tests/src/Mod/Part/App/FaceMakerBullseye.cpp` (16 tests) | Areas with holes and islands, many holes, a tilted plane, reversed wires | M3.5, every one |
| `PartDesignTests/TestPocket.py` `testPocketDimensionCase`; `testPocketThroughAllCase` | 75; 62.5 | M3.6, oracle cases with analytic volumes |
| `TestPad.py` `testPadTwoDimensionsCase`; `tests/src/Mod/PartDesign/App/Pad.cpp` `TestMidPlaneTwoLength` | 4; 3000π | M3.6 |
| `TestPad.py` `testPadToConcaveCase` | 700π + ½√80 + 40.5·asin(1/9) = 2208.0963: also fixes the revolve's sense | M3.6 (once up-to-first exists) |
| `tests/src/Mod/PartDesign/App/Revolution.cpp` (main only, `b2da06bfe0`, 2026-08: it tests main's `SideType`) | Torus segments, 2π²·R·r²·θ/360 | M3.6: new oracle cases modelled on it, in 1.1.3's single-angle and symmetric modes |
| `SketchObject.cpp` `testDeleteExposeInternalGeometryOf{Ellipse,Hyperbola,Parabola,BSpline}`, `testDeleteOnlyUnusedInternalGeometryOfBSpline` | Deleting a curve deletes its unused internal-alignment geometry | With those curves (after M3.2, which has points, lines, circles, and arcs), and the command API's delete |
| `SketchObject.cpp` `testReverseAngleConstraintToSupplementaryExpression*` (10 in 1.1.3); `SketchObjectChanges.cpp` (34 in 1.1.3: split, trim, knot multiplicity, join); main's `SketchObjectSymmetric.cpp` | Editing tools and expressions | Not ported: out of scope for M3 (above) |
| `tests/src/Mod/Sketcher/App/Constraint.cpp` (18 `ConstraintPointsAccess` tests); `planegcs/GCS.cpp` `clearConstraints` | FreeCAD's element-ID storage in its XML (`substituteIndex`, `involvesGeoId`, legacy attributes); clearing a system | Not ported: FreeCAD's file plumbing (the FreeCAD importer's, later); Silico builds a fresh system for each solve |
| `PartDesignTests/TestPad.py` `testBoxCase`, `testSketchOnBasePlane`, `testSketchOnDatumPlane` | Padding the slot plate gives 6 faces (whatever refine does) | M3.6 (the datum-plane case once sketches sit on datum planes) |
| `TestPad.py` `testPadToFirstCase`, `testPadtoLastCase`, `testPadToFaceCase`, `testPadToShapeCase`, `testPadToPlaneCustomDir`; `TestPocket.py` `testPocketToFirstCase`, `testPocketToFaceCase` | Up-to modes | Wait for the up-to modes, after M3.6 |
| `TestSketchExpression.py` `testConstraintWithExpression`; `TestPlacementUpdate.py` (4) | Expressions on constraints (cyclic references); placement updates of attached sketches | Later: with expressions and parameters, and with attachment |
| `SketcherTests` on `SketchAnalysis` (`testThreeLinesWithCoincidences_1`, `_2`, `testSingleMissingCoincidentCase`, `testDegenratedGeometryCase`, main's `testMissingVerticalHorizontalConstraints`), external geometry (`testRemovedExternalGeometryReference`, `testSaveLoadWithExternalGeometryReference`, `testTNPExternalGeometryStored`, `testDeleteConstraintsToExternalCase`, `testExternalGeoDeletion`), `TestSketchFillet.py` (11), carbon copy, main's `testPointGeometryExtension` | | Not ported in M3: out of scope (above) |
| Main's `TestSketchInternalFaces.py` (45) | Region profiles: a sketch's crossing geometry split into faces a pad can take one of | With region profiles (open decision 8), after M3.6 |
| Main's `TestSketcherEllipse.py` (`testProjectEllipse1`–`7`), `TestSketcherSolver.py` `testLegacyExternalGeometryOrientationMigration`, `testSymmetricKeepsConstraintOnTheMirroredSide`, `testReversedExternalGeometryLeavesSketchInPlace`; the GUI files (`TestConstraintCommandsGui.py`, `TestOnViewParameterGui.py`, `TestConstraintPreselectionGui.py`, `TestDistanceLabelExtensionGui.py`, `TestExternalFacePreselection.py`, `TestVisualLayerListGui.py`) | External-geometry projection, editing tools, migrations, and GUI interaction | Not ported in M3: out of scope (above); Silico's UI tests its own interaction (M5) |
| `PartDesignTests/TestRevolve.py` `testRevolveFace`, `testGrooveFace` | Face counts after revolving a box's face | Not ported: they revolve a face, not a sketch, and count faces only (refine-dependent) |

Cases FreeCAD lacks, which Silico adds: a free line (4 degrees of freedom); an extra constraint on a fully constrained sketch (over-constrained, naming it); a vertical line, a horizontal line, and a 90° angle between them (redundant); an 80° angle against a horizontal and a vertical (conflicting); a radius of 0 and −1 and a distance of 0 (refused by the gate); construction geometry left out of the profile; the arc flip; an ellipse whose radii cross; for M3.5, open, crossing, overlapping, and touching profiles, a T-junction, a figure-8, and lone points (main's `TestSketchInternalFaces.py` has T-junctions and figure-8s, but as regions, not as profiles refused).

## Decisions this note leaves open

The decisions this study leaves to the maintainer are in docs/roadmap/m3-sketcher.md, *Open decisions*, with the steps that depend on each.
