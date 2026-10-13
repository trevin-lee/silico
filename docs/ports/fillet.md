# Fillet

The constant-radius fillet, ported in M0.8 as a kernel operation (`silico_kernel::fillet`, `Op::Fillet`). The feature layer (M1 and M2) builds on it.

## What FreeCAD does

There are two entry points, both ending in OCCT's `BRepFilletAPI_MakeFillet`:

- **`Part.Shape.makeFillet`** (`src/Mod/Part/App/TopoShapeExpansion.cpp`, `TopoShape::makeElementFillet`, around line 4148). Its only guards are a null input and `IsDone`.
- **The PartDesign Fillet feature** (`src/Mod/PartDesign/App/FeatureFillet.cpp`). It adds the guards listed below.

Edges are numbered from 1 in `TopExp::MapShapes` order (FreeCAD's `Edge1`, `Edge2`, ...). Silico numbers them the same way, so the oracle compares edge for edge. Silico calls `Add(r, E)` where FreeCAD calls `Add(r, r, E)`; with equal radii OCCT takes the same constant-radius path, and the results are bit-identical.

## Guards, from FreeCAD's history

| FreeCAD commit | Guard | In Silico |
|---|---|---|
| `5ccf289f29` (2011) | Null or invalid input shape | Kernel. A B-rep that doesn't read, or fails its CRC, is refused before OCCT builds anything. |
| `32cd807f3c` (2011) | `IsDone` checked before taking the result | Kernel. A fillet that isn't done is `kernel_failed`. |
| `87293fac9b` (2016) | **The result must be valid where the fillet changed it** ("Resulting shape is invalid"): `BRepAlgo::IsValid(input, result, false, false)` | Kernel: the same check, and when the input is valid, the whole result must be too. See below. |
| `9ff76fc813` (2017) | If the result is invalid, limit tolerances to `Precision::Confusion()` (`ShapeFix_ShapeTolerance::LimitTolerance`), run `ShapeFix_Shape`, then check again | **Not ported.** See below. |
| `fc1e8b8d17` (2015) | Before calling OCCT, drop edges that don't have exactly two adjacent faces, edges between tangent faces (not C0), and repeats, warning about each (`DressUp::getContinuousEdges`) | **Feature layer (M1),** with the Fillet feature and its warnings. The kernel operation refuses repeats outright. |
| `883a726d52` (2018, #3401), `eb52dd4a9a` (2025) | The result must be a single solid (unless the body allows compounds) | **Feature layer (M1/M2).** The kernel operation fillets whatever shape it's given; whether a body may hold several solids is a feature rule. |
| `5382ca4bbb` (2025, #25671) | Catch the C++ exceptions OCCT throws instead of crashing | Covered by design: every C++ exception becomes an error, and OCCT runs in a worker, so even an abort is a `worker_crashed` error. |
| `7863a63659` (2017) | Radius must be positive | Kernel, before OCCT, and in the core before sending: a radius must be a length above 1e-7 mm (OCCT's tolerance) and at most a kilometre, as a chamfer's distance must, or it's `invalid_input`. FreeCAD checks only that it's positive, and builds a fillet of 1e-8, which leaves nothing OCCT can tell from the edge (a deliberate difference, since the M1 review; `box_fillet_through_worker_matches_freecad`). |
| `f6ce9caf79`, `be8f843ce1`, `8985f085db` (2011) | Fillet an untransformed copy of the base and apply its placement afterwards (`baseShape.setTransform(Base::Matrix4D())` in today's `FeatureFillet.cpp`) | **Feature layer (M2),** when features have placements. The kernel operation takes the shape as it is; its validity check compares faces with their locations. A dress-up keeps its base's placement, however the base was made: FreeCAD 1.1.3's base feature drops one held in its shape, and `123a1c80e1` fixes that only for some bases (docs/ports/chamfer.md). |
| `8311b1e66d` (Part's fillet, #4285; `73f086f73d` for chamfer) | After every fillet in the Part workbench, run `TopoShape::fix(Confusion, 2·Confusion, 4·Confusion)` | **Not ported, on purpose.** It marks broken results valid without fixing them: the second fillet below (volume 6251, larger than its 6000 box) is still 6251 after `fix`, and `isValid()` turns True. Porting it would switch the validity check off. |

### The validity guard, and what became of it in FreeCAD

OCCT reports some fillets done whose result isn't valid. Filleting every edge of a 10 × 20 × 30 box at r = 5.01, 5.5, or 9 "succeeds" with volumes larger than the box (7968, 7792), and a 10 cube's every edge at r = 6 gives an area of −7.7e22.

- **`87293fac9b` (2016) refused these** with `BRepAlgo::IsValid`, which checks only the faces of the result that aren't in the input (those the fillet made or changed) and the orientation of its shells, after repairing SameParameter flags on those faces' edges.
- **`9ff76fc813` (2017) stopped the refusal from firing for these inputs.** Before refusing, it limits tolerances and runs `ShapeFix_Shape`. For these results, `LimitTolerance` alone repairs nothing (checked with FreeCAD 1.1.3's `limitTolerance` on all four cases above), and `ShapeFix_Shape` turns the three 10 × 20 × 30 cases into a valid compound holding one shell and no solid (the 10 cube at r = 6 stays invalid). For those three the recheck passes, and the feature's single-solid step finds no solid, so FreeCAD 0.17 to 0.21 gave an empty feature instead of an error.
- **The topological naming merge (`b72a8c4222`, `ecf7e51ab3`, 2024)** removed the error return altogether, and `Part.Shape.makeFillet` never checked. **FreeCAD 1.1.3 returns these invalid solids.**

Silico runs `87293fac9b`'s check (`BRepAlgo::IsValid`, through the bridge's `is_valid_where_changed`) and refuses what fails it. The refusal is its own kind of failure (`FailureKind::InvalidResult`, which the core reports with its own code, `invalid_result`), so an agent can tell it from a fillet OCCT couldn't build (`kernel_failed`). The check is decided in Rust (`checked_result` in `silico-kernel`), not in the C++, so M1's operations share it. The oracle test records the difference from FreeCAD 1.1.3: for those inputs FreeCAD's result has `isValid() == False`, and Silico must refuse.

`9ff76fc813`'s repair isn't ported. `ShapeFix_Shape` would turn these refusals into shells passed off as results, and `LimitTolerance` on its own repairs no known input, so it would be code no test can reach. If an input turns up that it does repair, port it then, with that input as its test.

FreeCAD's check alone misses some broken results, so Silico is stricter where it can be. A second fillet on a filleted box (Edges 1, 3, 5 at r = 3, then Edge5 at r = 5) gives a "solid" of volume 6251, larger than the 6000 box, whose changed faces pass `BRepAlgo::IsValid`. FreeCAD would accept it. When the input passes `BRepCheck_Analyzer`, Silico requires the whole result to pass it too: an invalid result from a valid input is the operation's fault (`a_fillet_that_breaks_a_valid_filleted_box_is_refused`). `BRepAlgo::IsValid` repairs SameParameter flags on the new faces' edges in place, so the whole-result check runs after it and judges the shape that's returned; no input tested so far needs that repair (a probe over the whole test suite found the whole-result answer the same before and after it), so the order hasn't yet changed an answer. The input's validity isn't checked again: it comes with the shape, from the worker that made it (`ShapeData::valid`), so each shape is checked as a whole once.

When the input was already invalid (an imported shape that fails `BRepCheck` somewhere else, say), only the changed faces count, so the input's problem isn't blamed on the fillet: the fillet is accepted, and the result's summary still says it isn't valid. A check of the whole result would refuse every fillet on such a shape and name an edge that isn't at fault (`a_problem_the_input_already_had_isnt_blamed_on_the_fillet`).

## Where Silico differs on purpose

- **Invalid results are refused** (above).
- **A repeated edge is refused** (`invalid_input`, naming it). OCCT skips an edge it already has, so FreeCAD silently ignores repeats.
- **The failing edges are named** (`elements`), as OCCT reports them, where its report can be trusted (M2.0). When OCCT can't build a fillet, it keeps the contours it couldn't build (`BRepFilletAPI_MakeFillet::NbFaultyContours`, `FaultyContour`), with a status for each (`StripeStatus`), and the corners it couldn't blend (`NbFaultyVertices`, `FaultyVertex`). FreeCAD reads none of it: its fillet fails without naming an edge. OCCT 7.9.3 builds in three steps, each only if the last succeeded (`ChFi3d_Builder::Compute`):
  1. Each contour's surfaces. A contour whose surfaces fail is faulty, nearly always with an error status (`StartsolFailure`, `WalkingFailure`, ...). These are the contours at fault, so where any faulty contour has an error status, Silico names the given edges in every faulty contour.
  2. Each corner. A corner that fails is a faulty vertex, and Silico names the given edges in the contours that reach it.
  3. Each pair of contours, checked for a collision, stopping at the first. Only the first contour of the colliding pair is made faulty, with no error status, though the other may be the only one at fault. On a top face 2.85 wide with its four edges at r = 1.45, OCCT lists Edge2's contour as faulty, though Edge2 builds with the others, not the colliding Edge10 and Edge12; on a cube with all its edges at r = 5, it lists Edge1's alone of twelve alike. So a faulty contour with no error status names nothing, and where nothing else is named, the refusal names no edge and suggests filleting the edges one at a time, as before.

  Trying each edge alone across the pool, as the roadmap first planned, was weighed and not taken. From cases run on OCCT 7.9.3:
  - A 10 × 10 × 2 box with all 12 edges at r = 2.5: OCCT reports the eight horizontal edges, beside the 2 mm sides, with error statuses, and those are exactly the edges that fail alone.
  - A pyramid 4 × 20 × 4 with every edge at r = 2: OCCT reports its apex as a corner it couldn't blend, and the four edges meeting there are named. Alone, each fillets.
  - A 10 × 10 × 2 plate fused with a boss of radius 3 and height 1 centered on its top (`Part.makeCylinder(3, 1, V(5, 5, 2))`), every edge but the boss's seam (which Silico refuses before building) at r = 0.5: it fails, and alone none of those edges fails (only the seam does).

  So OCCT's report, from the same run, costs nothing more, and names edges only where it says they failed. Failures that come only from edges meeting (the boss, the 10 × 10 × 2 box at r = 1) are found by the pair check, so they name no edge, as trying the edges alone wouldn't either. Nor does OCCT say why an edge failed, so the suggestion offers a smaller radius if the fillet runs past a face beside the edges, another if not, or leaving them out (M1.5, with the chamfer; docs/ports/chamfer.md). A chamfer doesn't do the same: OCCT's chamfer builder (`ChFi3d_ChBuilder`) keeps the same report, but `BRepFilletAPI_MakeChamfer`, which FreeCAD's chamfer uses and Silico follows, keeps its builder private. Reading it would mean driving the builder directly, off FreeCAD's path and its history, so a chamfer of several edges that fails names none.
- **The edges are a set** (M1.5, with the chamfer): OCCT is given them in increasing order, so the order they're listed in doesn't change the result.
- **An edge OCCT won't fillet is refused, naming it and why** (M1.5, with the chamfer, which shares the rule; docs/ports/chamfer.md): a seam, a degenerate edge, an edge between faces that meet smoothly (OCCT's own verdict, from `Contour(E)`), an edge on one face or none, and edges joining more than two faces, ending at a corner where the shape touches itself or another solid at a point, or continuing smoothly into an edge that does, which OCCT never builds (it segfaults on them). One refusal names every edge at fault. OCCT throws "There are no suitable edges for chamfer or fillet" for such an edge alone, and skips it silently among others, as FreeCAD's `Part.makeFillet` does; FreeCAD's feature layer drops it with a warning (`fc1e8b8d17`).

## Same as FreeCAD

Checked against the oracle:

- Radii too large for an adjacent face (r = 10 and 11 on a 10-wide face) fail in both. r = 9.99 succeeds in both.
- Radii far below or above any sensible length (1e-12, 1e300) fail in both; 1e-8 succeeds in both, barely changing the box.

Edges that can't be filleted (seams, degenerate edges, edges between smoothly joined faces) are refused before OCCT builds anything (above), tested with the chamfer's (`an_edge_occt_wont_take_is_refused_naming_it` in `chamfer.rs`).

## FreeCAD tests

| Test | Status |
|---|---|
| `src/Mod/PartDesign/PartDesignTests/TestFillet.py` `testFilletCubeToSphere` | Ported: `freecad_fillet_cube_to_sphere` (every edge of a 10 cube at r = 4.999999 gives nearly a sphere of radius 5, places = 3). Bit-identical to FreeCAD. |
| `tests/src/Mod/Part/App/TopoShapeExpansion.cpp` `makeElementFillet`, geometry | Ported: `freecad_fillet_every_edge_of_a_unit_cube` (area 5.739646, which is also analytic, and the volume). Its wire count (26) isn't: the summary doesn't count wires yet. |
| The same test's element map, and `src/Mod/Part/parttests/TopoShapeTest.py` `testTopoShapeMakeFillet` (element map size 42) | M2, with topological naming. |
| `tests/src/Mod/Part/App/FeatureFillet.cpp` `testOtherEdges` (125.57079), `testMostEdges` (118.38763) | With refine: they fillet a fused shape with `Refine = true`. |
| `TestFillet.py`'s other cases (`UseAllEdges`, face selections) | M1/M2, with the Fillet feature. |
| `TestFillet.py` `testDeletingPreviousFeatureRelinksUniqueMatchingBaseEdge`, `testDeletingPreviousFeatureDoesNotRelinkUnsafeBaseEdge` | M2, with topological naming. |
| `src/Mod/PartDesign/PartDesignTests/TestTopologicalNamingProblem.py` `testPartDesignElementMapFillet`, `testApplyFillet`, `testPartDesignTNPFillet` | M2, with topological naming. |

## Notes

- A binary B-rep written by FreeCAD 1.1.3 doesn't read with OCCT's `BinTools::Read` (`Standard_OutOfRange: NCollection_IndexedMap::FindKey`), though both run OCCT 7.9.3. The cause is FreeCAD's layout, not OCCT's version: after the shape set, FreeCAD writes the root shape as three integers (shape index, location index, orientation; `TopoShape::exportBinary`, `TopoShape.cpp`) and reads it back the same way (`importBinary`), where `BinTools::Write` writes an orientation and a reversed index. Read FreeCAD's way (`BinTools_ShapeSet::Read`, then the three integers), the same file gives the right shape: a 10 mm cube saved by the oracle FreeCAD with `SaveBinaryBrep` on, read so by a probe linked against Silico's OCCT build, is valid with volume 1000. Compare with FreeCAD by values (volume, area, topology, validity), never by B-rep bytes.
- The chamfer has its own port note, docs/ports/chamfer.md, and shares this note's result check and edge rules.
