# Chamfer

Edges cut back by a flat face, ported in M1.5 as a kernel operation (`silico_kernel::chamfer`, `Op::Chamfer`) beside the fillet (docs/ports/fillet.md), whose rules it shares. The feature layer (M2) builds on it.

## What FreeCAD does

Three entry points, all ending in OCCT's `BRepFilletAPI_MakeChamfer`:

- **`Part.Shape.makeChamfer(d, edges)` and `makeChamfer(d1, d2, edges)`** (`src/Mod/Part/App/TopoShapePyImp.cpp`), through `TopoShape::makeElementChamfer` (`TopoShapeExpansion.cpp`).
- **`Part::Chamfer`** (`src/Mod/Part/App/FeatureChamfer.cpp`): two distances per edge, no check of the result.
- **The PartDesign Chamfer feature** (`src/Mod/PartDesign/App/FeatureChamfer.cpp`), through `makeElementChamfer`, with three types:
  - **Equal distance:** `Size` back from the edge along both faces (`Add(Size, Size, E, F)`).
  - **Two distances:** `Size` along the reference face, `Size2` along the other (`Add(Size, Size2, E, F)`).
  - **Distance and angle:** `Size` along the reference face, the chamfer face at `Angle` to it (`AddDA(Size, toRadians(Angle), E, F)`). On a square edge the cut reaches `Size · tan(Angle)` along the other face.

  `FlipDirection` swaps the faces: without it, the reference face is the first face `TopExp::MapShapesAndAncestors` lists for the edge, and with it, the last. On a solid that's the edge's lower-numbered face, and its higher-numbered one. Checked on FreeCAD 1.1.3: the 10 × 20 × 30 box's Edge1 lies between Face1 (x = 0) and Face3 (y = 0). With sizes 1 and 3, the cut runs from (0, 1) to (3, 0), and flipped from (0, 3) to (1, 0). At 1 and 30°, it runs from (0, 1) to (0.57735, 0).

Edges are numbered as for the fillet: from 1, in `TopExp::MapShapes` order.

OCCT extends a chamfer along edges that continue its edge smoothly (a contour), as it does a fillet. It takes no edge it can't chamfer: an edge that's degenerate (a sphere's pole, a cone's tip), that isn't between two faces, or whose faces meet smoothly (a seam, or a fillet's edge). It judges the last by `ChFi3d::IsTangentFaces`, which takes the continuity recorded on the edge if it's better than C0 (as primitives and fillets record it where faces are tangent), and otherwise measures: normals within 0.1 radian (5.7°) of each other count as tangent, so OCCT also refuses a real corner shallower than that. (FreeCAD's feature layer filters on the recorded continuity alone.) Given only such edges, it throws "There are no suitable edges for chamfer or fillet". Given them with others, it chamfers the others and skips them without a word.

## Guards, from FreeCAD's history

| FreeCAD commit | Guard | In Silico |
|---|---|---|
| `19bc5077c8` (2016) | **The result must be valid where the chamfer changed it** (`BRepAlgo::IsValid`): the chamfer's counterpart of the fillet's `87293fac9b` | Kernel: the fillet's check (`checked_result`), the whole result too when the input is valid. |
| `9ff76fc813` (2017) | If the result is invalid, limit its tolerances (and, in 2017, run `ShapeFix_Shape`) and go on. Today's feature only calls `LimitTolerance` and returns the result, valid or not | **Not ported**, as for the fillet: Silico refuses an invalid result. `Part::Chamfer` and `makeChamfer` never checked. |
| `deaef0ae0e` (2018) | Size must be above zero | Kernel and core, before OCCT: every distance must be above OCCT's tolerance (1e-7) and at most a kilometre, as a primitive's lengths are. |
| `cc82cf3e50`, `4669b6c88d`, `de02063f05` (2020) | Distance and angle: the angle must be above 0 and below 180 degrees | The same, and finite, and the cut deeper than OCCT's tolerance: the distance times the angle in radians above 1e-7 (on a square edge the cut reaches the distance times the angle's tangent; OCCT fails at 1e-9 degrees). Whether an angle fits the edge (below 90° on a square edge) is OCCT's to decide: it fails, cleanly, when it doesn't. |
| `262841bb3c`, `e1240fb18d` (2020) | Three chamfer types; `FlipDirection` picks the other face as the reference | The same three (`ChamferSize`), and `flip`. |
| `4d712f44c2` (2025, #19238) | FreeCAD 1.0 swapped which face `Size` goes on for files saved before 1.0, so files from 0.21 are flipped on load | Silico follows 1.0 and later: `distance` goes on the lower-numbered face. The cause isn't recorded; the reference-face code was rewritten between the two (below). It doesn't matter for Silico, which has no old files. |
| `1a3527c06a` (2021, PR #5039), removed in `ecf7e51ab3` (2024) | When the chamfer's edges came from selected faces, `Size` went on the face that wasn't selected (unless flipped). The topological naming rewrite dropped it: FreeCAD 1.0 and later use the edge's first or last face whatever was selected | **Feature layer (M2), to decide there**, when chamfers take faces: SolidWorks measures from the selected face's side. The kernel operation takes edges only. |
| `8985f085db` (2011) | Chamfer an untransformed copy of the base and apply its placement afterwards | **Feature layer (M2),** as for the fillet's `f6ce9caf79`. But in 1.1.3 a placement held in the base's shape is lost before the chamfer runs: the `PartDesign::FeatureBase` syncs its shape to its own identity Placement on recompute (`PartFeature.cpp:1548`), which the oracle has to put back. FreeCAD's `123a1c80e1` ("PD: Preserve external base feature placement", after 1.1.3) restores it only for a base made through `Body.BaseFeature`. The rule for M2: a dress-up keeps its base's placement, however the base was made. |
| `73f086f73d` (2020, #4285), removed in `ecf7e51ab3` | `Part::Chamfer` ran `ShapeFix` on its result, which "was producing invalid shape" | **Not ported, on purpose,** as the fillet's `8311b1e66d`: Silico refuses an invalid result rather than repair it out of sight. |
| `d214306d1b` (2026, not in FreeCAD 1.1.3) | An edge that's degenerate, or has no face, is refused before OCCT sees it ("prevent chamfer crash") | Kernel: refused, with the others OCCT won't take (below). FreeCAD 1.1.3 on OCCT 7.9.3 fails cleanly on them, with "There are no suitable edges". |
| `fc1e8b8d17` (2015), `DressUp::getContinuousEdges` | Before calling OCCT, the feature drops edges that don't have exactly two faces, or whose recorded continuity isn't C0, warning about each | Kernel: Silico refuses them (below). Dropping them with a warning is a feature-layer choice (M2). |
| `883a726d52`, `eb52dd4a9a` | The result must be a single solid | Feature layer (M2), as for the fillet. |
| `5382ca4bbb`, `d214306d1b` | Catch OCCT's exceptions and crashes | Covered by design: every C++ exception is an error, and OCCT runs in a worker. |

## Where Silico differs on purpose

- **An edge OCCT won't chamfer is refused, naming it** (`invalid_input`, `elements: [Edge2]`, every edge at fault since M2.0), with why: degenerate, on one face or none, joining more than two faces, ending at a corner where the shape touches itself or another solid at a point, continuing smoothly into an edge that does, or joining faces OCCT treats as tangent (a seam, a fillet's edge, a corner of a few degrees). Whether OCCT takes an edge is its own verdict (`Contour(E)`), not a copy of its rules. Some are never given to OCCT: degenerate edges and edges on no face (FreeCAD's `d214306d1b`), and two on which OCCT 7.9.3 segfaults, as FreeCAD 1.1.3 does: an edge joining more than two faces (where solids touch along an edge, once fused), and an edge ending at a corner where the faces around it form more than one fan (where the shape touches itself or another solid at a point). OCCT follows each edge it takes along the edges that continue it smoothly, and segfaults too where that chain reaches such a corner, so OCCT's chains are checked before it builds anything. One refusal names every edge at fault, in its message and its suggestion. When OCCT throws taking an edge (as it does for an edge ending on one that joins more than two faces), the edges already refused are named, or if there are none, that edge is blamed. FreeCAD's Part chamfer skips such an edge without a word when it's given others, and its feature skips it with a warning. Silico never builds less than it was asked for. The fillet takes the same rule, for the same reason: OCCT skips the same edges there.
- **The bridge reports facts, and Rust decides** (`crates/silico-kernel/src/dressing.rs`): the bridge says what each edge lies on and where it ends, gives OCCT the edges Rust picks, each with the face Rust picks, and says which contour took each and what its edges are; Rust withholds, refuses, and explains (ARCHITECTURE.md: the C++ side only calls OCCT).
- **The edges are a set.** OCCT's result depends on the order it's given them: every edge of the 10 × 20 × 30 box at 4.999999 builds listed 1 to 12 and fails listed 12 to 1 (in FreeCAD too), and in a chain of edges OCCT chamfers as one, the first edge decides which face `distance` is measured on. Silico gives OCCT the edges in increasing order, so the same set always makes the same chamfer, as a selection does in SolidWorks; in a chain, the lowest-numbered edge listed decides. FreeCAD uses the order of the selection.
- **An edge that ends on one joining more than two faces fails saying so.** OCCT takes it, then fails whatever the size ("ChFi3d_Builder:only 2 faces"); the suggestion says why rather than to try a smaller size.
- **A failure's suggestion doesn't guess.** OCCT doesn't say why a chamfer failed: one too large for a face beside the edge fails, but so do some small ones on curved faces, and some edges fail at every size (an edge from a cone's tip). The suggestion offers a smaller size if the chamfer runs past a face, another if not, or leaving the edge out. The fillet's says the same.
- **Invalid results are refused** (above). Chamfering two meeting edges of FreeCAD's `testOther` fuse, unrefined, gives an invalid solid that FreeCAD 1.1.3 returns.
- **A repeated edge is refused**, as for the fillet.
- **Sizes at or below OCCT's tolerance are refused**, and angles whose cut would be (above). FreeCAD builds a 1e-8 chamfer, whose face is narrower than the tolerance, and fails at 1e-12; at 1e-6 degrees it builds a cut 1.7e-8 deep, and fails at 1e-9. Silico's lengths stop where OCCT's tolerance does. A chamfer can still leave a face narrower than the tolerance where it meets another (every edge of the 10 × 20 × 30 box at 4.99999999 leaves faces 2e-8 wide), as FreeCAD's does: that's the result's, not the size's.
- **The failing edges are named**, as OCCT reports them, as for the fillet (docs/ports/fillet.md), but only through the contours whose surfaces failed. OCCT's chamfer builder (`ChFi3d_ChBuilder`) keeps the same report as the fillet's, in the same three steps, but `BRepFilletAPI_MakeChamfer` doesn't pass it on; Silico's OCCT patch 0002 adds the fillet's five accessors to it (`NbFaultyContours`, `FaultyContour`, `StripeStatus`, `NbFaultyVertices`, `FaultyVertex`), which change no result. FreeCAD reads neither report. From cases run on OCCT 7.9.3 (equal distances, each measured from the edge's lower-numbered face):
  - A 10 × 10 × 2 box with all 12 edges at d = 2, 2.5, or 4: OCCT reports the eight horizontal edges, beside the 2 mm sides, with error statuses, exactly the edges that fail alone. So does the 10 × 20 × 30 box at d = 11: Edge1 and Edge2 given together are both reported, and Edge1 given with Edge11 is reported alone.
  - Collisions are reported as for the fillet, the first contour of the pair with no error status, and name nothing: the same box at d = 1 (Edge2's contour), and the 5.25 × 2.85 × 5 box's top edges at d = 1.45 (Edge2's again, though Edge10 and Edge12 collide).
  - Corners mislead, unlike the fillet's, so a chamfer names no edge through them. The 10 × 10 × 2 box at d = 1.5 reports four corners, which reach all 12 edges, though only the eight horizontal ones collide (two chamfers of 1.5 across each 2 mm side); the vertical four are fine. A 10 mm cube at d = 5.5 or 7.5, and a 4 × 20 × 4 pyramid at d = 2, report corners too.
  - The cost: a chamfer that fails only at a corner names nothing, though one given edge may be at fault. On a 10 × 20 × 30 box with Edge1 filleted at r = 2, the top edge on y = 0 continues smoothly round the arc to the top edge on x = 0, beside a face 10 wide; chamfered at 10.5 with an edge far away, OCCT reports only the chain's far corner, so no edge is named (alone, the edge is named, as any single edge is).
  - Two given edges on one chain are one contour, measured from the face the lower-numbered one picks, so both are named when it fails, though with a different face it might not have: on that box, Edges 9 and 13 (the bottom chain) at a distance of 5.95 and 60° fail only through Edge9's face, and both are named.
  - A 10 mm cube with all 12 edges at d = 5 reports a faulty contour past the 12 given: a corner stripe OCCT added while building, found colliding. It isn't a given contour, and names nothing.

## Same as FreeCAD

Checked against the oracle (`chamfers_match_freecad`):

- A chamfer as wide as a face (10 on a 10-wide face) fails in both, and 9.999 succeeds in both. A pair of distances that runs past a face fails in both.
- On a square edge, an angle of 89° (whose cut runs past the box) fails in both, as do 90° and 135°.
- A cylinder's rim and a frustum's chamfer in both (cone faces), each kind with and without the flip.

Probed with FreeCAD 1.1.3's `Part.makeChamfer`: a seam, a sphere's poles, and a cone's tip fail there with "There are no suitable edges"; Silico refuses them before building, naming the edge.

## What OCCT gets wrong, accepted as FreeCAD accepts it

- **Some chamfers OCCT reports done aren't valid solids.** On FreeCAD's `testOther` fuse, unrefined (a 4 × 5 × 6 box with a 1 × 2 × 3 box on top at (0, 1, 6)), chamfering Edge2 (where the boxes' faces meet) at 0.5 gives an invalid solid, which FreeCAD 1.1.3 returns (volume 125.875, valid False); Silico refuses it (`invalid_result`). Refined, the same chamfers are valid.
- **A distance and an angle of exactly 90° on an acute edge** (a wedge of 30°, 45°, or 60°), unflipped, gives an invalid solid at every distance tried, where 89.99° and 90.01° are fine; FreeCAD returns it, and Silico refuses it.
- **Edges joining more than two faces, and edges ending at a corner where solids touch, segfault OCCT** (above): never given to it.
- **A distance and an angle of exactly 90° on a square edge can hang OCCT.** A 271.70358837955126 × 776.8935171151478 × 1447.974451856977 box, Edge9 at 585.5979126463726 and 90°: a right angle never meets the other face, so there's nothing to build, but `BRepFilletAPI_MakeChamfer::Build` never returns (it loops in `ChFi3d_Builder::PerformIntersectionAtEnd`'s Newton iteration). FreeCAD 1.1.3's PartDesign Chamfer on the same box hadn't finished after 90 s. FreeCAD allows any angle above 0 and below 180°, as Silico does. In Silico the chamfer runs in a worker, which is stopped at its time limit, so the hang is an error with advice (`a_right_angle_chamfer_occt_hangs_on_is_refused_or_stopped`). The box chamfer property test, which calls OCCT in its own process and draws distances up to a face's width and past it, doesn't draw 90°; the in-process tests that do chamfer at 90° use small, fixed sizes on a 10 × 20 × 30 box.
- **B-reps only another program would make** (a box compounded with one of its own faces, or with itself; an inside-out box) fail with "only 2 faces" or an invalid result rather than a clear refusal. Silico's operations don't make them; they matter once shapes are imported (STEP import, after the first slice).

## History

As the fillet's (docs/ports/history.md): the chamfered edge is deleted and generates the chamfer face, the two faces it joined are modified, as are the end caps whose outlines change, and the faces it doesn't touch are kept (`chamfering_one_edge_keeps_modifies_and_generates_what_the_geometry_says`). Checked against FreeCAD's element names (`CHF`), positions included, for one edge (equal and two distances), two meeting edges, and all twelve (`chamfer_history_matches_freecads_element_names`).

## FreeCAD tests

| Test | Status |
|---|---|
| `src/Mod/PartDesign/PartDesignTests/TestChamfer.py` `testChamferCubeToOctahedron` | Ported: every edge of a 10 cube at 4.999999 leaves 8 faces larger than 1e-3 (their areas from the mesh, exact for flat faces), with volume and area by hand (each edge removes a prism of section c²/2, each corner 5c³/6 more). Then the four edges of `Face1` leave 9. `UseAllEdges` and topological naming resilience are feature-layer (M2). |
| `tests/src/Mod/Part/App/TopoShapeExpansion.cpp` `makeElementChamfer`, geometry | Ported: every edge of a unit cube at 0.05 has area 5.640996, which is also analytic. Its wire count (26) isn't ported: the summary doesn't count wires. Its element map is M2. |
| `tests/src/Mod/Part/App/FeatureChamfer.cpp` `testOther` | Its formula is ported: two edges of a box meeting at a corner, chamfered at c, remove `L c²/2 + H c²/2 − c³/3`. The test itself chamfers a refined fuse, and refine comes later (M1, before M2). Unrefined, OCCT's result is invalid, and Silico refuses it. |
| `FeatureChamfer.cpp` `testMost` (121.46667, on the refined fuse) | With refine. |
| `src/Mod/Part/parttests/TopoShapeTest.py` `testTopoShapeMakeChamfer` (element map 42, for 0.1 on the first face's edges of a box) | Ported with naming (M2.7, silico-kernel `tests/naming.rs`), as its fillet twin. |
| `TestChamfer.py` face selections, `UseAllEdges`; the TNP tests (`testPartDesignElementMapChamfer`, `testPartDesignTNPChamfer`) | M2. |

And Silico's own (`crates/silico-kernel/tests/chamfer.rs`): `chamfer_edge` from the roadmap (10 × 20 × 30, d = 2 on a 30-long edge: 5940); every kind and the flip, against the edge's geometry (volume, area, and center by hand); a cylinder's rims, by Pappus, flipped and not; 23 chamfers compared with FreeCAD on measures (5 failing in both); the history its element names state; every edge OCCT won't take, for chamfer and fillet; every refusal, in the core (never sent) and in the worker (which goes on); determinism through the worker. The size rule's unit tests are in `silico-protocol`'s `chamfer.rs`.
