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
| `7863a63659` (2017) | Radius must be positive | Kernel, before OCCT: a radius that isn't a positive, finite length is `invalid_input`. |
| `f6ce9caf79`, `be8f843ce1`, `8985f085db` (2011) | Fillet an untransformed copy of the base and apply its placement afterwards (`baseShape.setTransform(Base::Matrix4D())` in today's `FeatureFillet.cpp`) | **Feature layer (M1),** when features have placements. The kernel operation takes the shape as it is; its validity check compares faces with their locations. |
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
- **The failing edge is named** when a single-edge fillet fails (`element: Edge3`), with the suggestion to try a smaller radius. OCCT doesn't say which of several edges failed, so for several the element is left out rather than guessed, and the suggestion is to fillet them one at a time to find it.

## Same as FreeCAD

Checked against the oracle:

- Radii too large for an adjacent face (r = 10 and 11 on a 10-wide face) fail in both. r = 9.99 succeeds in both.
- Radii far below or above any sensible length (1e-12, 1e300) fail in both; 1e-8 succeeds in both, barely changing the box.

Not yet checked, because a box has no such edges: edges that can't be filleted alone (seams, degenerate edges, edges between smoothly joined faces). In OCCT they throw "There are no suitable edges for chamfer or fillet", and mixed with filletable edges they're skipped silently; FreeCAD's feature layer filters them first (`fc1e8b8d17`). Add oracle cases when M1 brings cylinders.

## FreeCAD tests

| Test | Status |
|---|---|
| `src/Mod/PartDesign/PartDesignTests/TestFillet.py` `testFilletCubeToSphere` | Ported: `freecad_fillet_cube_to_sphere` (every edge of a 10 cube at r = 4.999999 gives nearly a sphere of radius 5, places = 3). Bit-identical to FreeCAD. |
| `tests/src/Mod/Part/App/TopoShapeExpansion.cpp` `makeElementFillet`, geometry | Ported: `freecad_fillet_every_edge_of_a_unit_cube` (area 5.739646, which is also analytic, and the volume). Its wire count (26) isn't: the summary doesn't count wires yet. |
| The same test's element map, and `src/Mod/Part/parttests/TopoShapeTest.py` `testTopoShapeMakeFillet` (element map size 42) | M2, with topological naming. |
| `tests/src/Mod/Part/App/FeatureFillet.cpp` `testOtherEdges` (125.57079), `testMostEdges` (118.38763) | M1, once booleans exist: they fillet a fused shape. |
| `TestFillet.py`'s other cases (`UseAllEdges`, face selections) | M1/M2, with the Fillet feature. |
| `TestFillet.py` `testDeletingPreviousFeatureRelinksUniqueMatchingBaseEdge`, `testDeletingPreviousFeatureDoesNotRelinkUnsafeBaseEdge` | M2, with topological naming. |
| `src/Mod/PartDesign/PartDesignTests/TestTopologicalNamingProblem.py` `testPartDesignElementMapFillet`, `testApplyFillet`, `testPartDesignTNPFillet` | M2, with topological naming. |

## Notes

- A B-rep written by FreeCAD 1.1.3 (OCCT 7.9.3) doesn't read in Silico's OCCT 8.0.1 (`Standard_OutOfRange` in `BinTools`), CRC or not. Compare with FreeCAD by values (volume, area, topology, validity), never by B-rep bytes.
- Chamfer (M1) has its own guards in FreeCAD (a degenerated edge, an edge with no adjacent face; `TopoShapeExpansion.cpp` `makeElementChamfer`, `d214306d1b`, 2026, not in FreeCAD 1.1.3; `19bc5077c8` is the chamfer's validity check, its counterpart of `87293fac9b`), and gets its own port note.
