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
| `87293fac9b` (2016) | **The result must be a valid shape** ("Resulting shape is invalid") | Kernel. See below. |
| `9ff76fc813` (2017) | If the result is invalid, limit tolerances to `Precision::Confusion()`, then check again | Kernel. Tried before refusing. |
| `883a726d52` (2018, #3401), `eb52dd4a9a` (2025) | The result must be a single solid (unless the body allows compounds) | **Feature layer (M1/M2).** The kernel operation fillets whatever shape it's given; whether a body may hold several solids is a feature rule. |
| `5382ca4bbb` (2025, #25671) | Catch the C++ exceptions OCCT throws instead of crashing | Covered by design: every C++ exception becomes an error, and OCCT runs in a worker, so even an abort is a `worker_crashed` error. |
| Radius must be positive (`FeatureFillet.cpp`) | | Kernel, before OCCT: a radius that isn't a positive, finite length is `invalid_input`. |

### The validity guard, and a FreeCAD regression Silico doesn't copy

OCCT reports some fillets done whose result isn't a valid solid. Filleting every edge of a 10 × 20 × 30 box at r = 5.01, 5.5, or 9 "succeeds" with volumes larger than the box (7968, 7792), and a 10 cube's every edge at r = 6 gives an area of −7.7e22. `87293fac9b` refused these; `9ff76fc813` first tried to repair tolerances.

The refusal was lost when the topological naming code was merged (`b72a8c4222`, `ecf7e51ab3`, 2024): `FeatureFillet.cpp` still repairs tolerances but no longer returns an error, and `Part.Shape.makeFillet` never checked. **FreeCAD 1.1.3 returns these invalid solids.** Silico checks the result with `BRepCheck_Analyzer`, tries `ShapeFix_ShapeTolerance::LimitTolerance` as `9ff76fc813` did, and refuses what's still invalid (`kernel_failed`, cause "the fillet's result isn't a valid solid"). The oracle test records the difference: for those inputs FreeCAD's result has `isValid() == False`, and Silico must refuse.

## Where Silico differs on purpose

- **Invalid results are refused** (above).
- **A repeated edge is refused** (`invalid_input`, naming it). OCCT skips an edge it already has, so FreeCAD silently ignores repeats.
- **The failing edge is named** when a single-edge fillet fails (`element: Edge3`). OCCT doesn't say which of several edges failed, so for several the element is left out rather than guessed.

## Same as FreeCAD (checked against the oracle)

- Edges that can't be filleted alone (seams, degenerate edges, edges between smoothly joined faces): OCCT throws "There are no suitable edges for chamfer or fillet". Mixed with filletable edges, they're skipped silently, in both.
- Radii too large for an adjacent face (r = 10 and 11 on a 10-wide face) fail in both. r = 9.99 succeeds in both.
- Radii far below or above any sensible length (1e-12, 1e300) fail in both; 1e-8 succeeds in both, barely changing the box.

## FreeCAD tests

| Test | Status |
|---|---|
| `src/Mod/PartDesign/PartDesignTests/TestFillet.py` `testFilletCubeToSphere` | Ported: `freecad_fillet_cube_to_sphere` (every edge of a 10 cube at r = 4.999999 gives nearly a sphere of radius 5, places = 3). Bit-identical to FreeCAD. |
| `tests/src/Mod/Part/App/TopoShapeExpansion.cpp` `makeElementFillet`, geometry | Ported: `freecad_fillet_every_edge_of_a_unit_cube` (area 5.739646, which is also analytic, and the volume). |
| The same test's element map, and `src/Mod/Part/parttests/TopoShapeTest.py` `testTopoShapeMakeFillet` (element map size 42) | M2, with topological naming. |
| `tests/src/Mod/Part/App/FeatureFillet.cpp` `testOtherEdges` (125.57079), `testMostEdges` (118.38763) | M1, once booleans exist: they fillet a fused shape. |
| `TestFillet.py`'s other cases (`UseAllEdges`, face selections) | M1/M2, with the Fillet feature. |

## Notes

- A B-rep written by FreeCAD 1.1.3 (OCCT 7.9.3) doesn't read in Silico's OCCT 8.0.1 (`Standard_OutOfRange` in `BinTools`), CRC or not. Compare with FreeCAD by values (volume, area, topology, validity), never by B-rep bytes.
- Chamfer (M1) has its own guards in FreeCAD (a degenerated edge, an edge with no adjacent face; `TopoShapeExpansion.cpp` `makeElementChamfer`, `19bc5077c8`), and gets its own port note.
