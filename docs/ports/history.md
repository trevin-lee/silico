# Shape history

What becomes of each face, edge, and vertex of an operation's inputs: kept, modified into other elements, or generating new ones. Captured from OCCT's builders and returned with an operation's result (the fillet since M1.4; each operation of M1.5 as it's added); topological naming (M2.7) is built on it. This note covers capture only. How FreeCAD turns history into element names is M2's port note.

## What FreeCAD does

FreeCAD reads history in `TopoShape::makeShapeWithElementMap` (`src/Mod/Part/App/TopoShapeExpansion.cpp`, around lines 1420 to 2010), brought in with the topological naming code from realthunder's LinkStage3 (`d768706ebe`, `671b43c4d5`, `c1b296dab8`, 2024). For every operation:

1. **Unchanged elements first, by identity.** Each input face, edge, and vertex that appears in the result as the same OCCT sub-shape (same `TShape` and location: `IsSame`) keeps its name. Nothing else is asked about it.
2. **Then OCCT's history, for vertices, edges, and faces** (never wires, shells, or solids): `Modified(s)` and `Generated(s)` of each input sub-shape, through a mapper (`MapperMaker`, `MapperHistory`, and per-operation variants).
3. **Deletion is implicit.** FreeCAD never calls `IsDeleted` in its naming code (only the 2012 colour history in `PartFeature.cpp` does).
4. **What's left is derived** from neighbours (a face named from its edges, an edge from its vertices). That's naming, not history.

## OCCT's pitfalls, and what Silico does about each

| Pitfall | FreeCAD | Silico |
|---|---|---|
| **`IsDeleted` is wrong for fillets and chamfers:** `BRepFilletAPI_MakeFillet::IsDeleted` is `!(myMap.Contains(F) \|\| IsSplit(...))`, and `myMap` holds only result faces, so every untouched edge and vertex reports deleted (probe, OCCT 8.0.1; the history tests hold on 7.9.3). | Never calls it. | Never calls it. An element is deleted when it isn't kept and wasn't modified. |
| **Unchanged elements aren't reported:** for booleans, `Modified` is empty and `IsDeleted` false for an element that survives as it was; for transforms and sewing, `Modified` returns the element itself. | Names by identity first, then skips anything already named. | `kept`: each input element is looked up in the result's element map (`IsSame`) before its history is read. |
| **`Modified` and `Generated` can name sub-shapes not in the result** (the revolve's are the known case: "some shape from a vertex that never made into the final shape"). | Skips them (`newInfo.find(newShape) == 0`), but counts them first: an element's position in OCCT's list goes into its name (`;:G2`, `K-2`; `TopoShapeExpansion.cpp:1509`, `:1733-1750`). | Dropped, but each kept entry records its position in OCCT's list, counting the dropped ones (`Reported::position`), so naming can reproduce FreeCAD's names. The lists themselves are sorted by element, so their order never depends on OCCT's. |
| **`Generated` can name a coarser sub-shape:** `BRepPrimAPI_MakePrism::Generated(face)` is the whole solid. | Expands it into faces and edges as a last-resort name (wires into edges; shells, solids, and compounds into faces; `:1471-1478`). | A solid is kept as a reference to it (`ElementKind::Solid`); naming decides what to do with it. A wire, shell, or compound is left out for now: no operation reports one yet. When one does (M1.5), it's decided then and recorded here. |
| **`Modified` can name a sub-shape of another kind.** | Skips it with a warning (`:1519-1530`). | The same: a `Modified` result of another kind is left out, so `modified` is always the same kind. |
| **An extrusion's or revolve's end caps have no history:** the bottom is the input face (kept), the top a translated copy that nothing names. | Guesses the top face by a coplanar test (`checkForParallelOrCoplanar`; fixed in `8fff92618c`). | M1's polygon sweeps report `FirstShape` and `LastShape` as `History::ends` (docs/ports/extrude-revolve.md), where FreeCAD's heuristic exists only because it didn't capture them. The sweeps of a profile's faces (M3.6, below) report each element's `FirstShape` and `LastShape` in its own fate. |
| **`Modified` and `Generated` can throw** (`BRepTools_Modifier` throws `Standard_NoSuchObject` for a shape it doesn't know). | Catches it and returns an empty list. | The exception becomes the operation's structured error: a history that silently lost entries would make naming wrong without saying so. |
| **One output can come from several inputs** (in a fuse, both boxes' top faces are modified into one face). | Picks a "first" source by sorting; hashes the others into the name. | Recorded as it is: each input element lists the output; naming decides. |
| **An element can be both modified and generated.** | Marks it `MODGEN` with a warning. | Recorded in both lists. |
| **Seam edges** appear twice in a face's wire. | `IsSame` maps count them once. | The same: elements are numbered by `TopExp::MapShapes`, one per distinct sub-shape. |

## Silico's history

An operation on shapes returns, with its result, a history per input shape (`Reply::Built(BuiltData { shape, history })`): for each of its faces, edges, and vertices, in `TopExp::MapShapes` order (FreeCAD's `Face1`, `Edge1`, ...):

- `kept`: its number in the result, if it's there unchanged;
- `modified`: the result elements of the same kind it became;
- `generated`: the result elements it created (any kind: an edge generates a fillet's face, a face generates an extrusion's solid);

each with its position in OCCT's list. The core checks a history before using it: it's for as many inputs as the operation had, and names only elements the result has. How many elements each input has isn't known to the core yet, so a fate is read with `History::fate`, which can't index past the end.

An element is **deleted** when it's neither kept nor modified (it may still have generated something, as a filleted edge does). Every number refers to the result's own numbering, so a history can be checked against the result alone.

The bridge reports raw facts (each input element's matches in the result, with no number where OCCT names something the result doesn't have); Rust drops what isn't in the result and builds the history. The C++ decides nothing.

## Sweeps of a profile's faces

M3.6's prism (`Op::Prism`, `Op::PrismThroughAll`) and revolution (`Op::Revolution`) sweep the faces `MakeFaces` made of a profile, after moving them to where the sweep starts (a translation or a rotation, as one location, as `Translate` places a shape), and report their history differently from every other operation (docs/design/m3.6-sketch-features.md, S3 and S4). The input is the faces, as one shape; each of its faces, edges, and vertices, in `TopExp::MapShapes` order, reports, from OCCT's own answers on its moved copy:

- its **start** counterpart, OCCT's `FirstShape(element)`, as its one `modified` element, never `kept`: a moved element is modified, as `Translate` reports it, and an unmoved one is reported the same way so the sweep's history doesn't depend on the start offset (a prism from the plane and one from 5 below it report the same history, element for element);
- its **end** counterpart, OCCT's `LastShape(element)`, as `generated`, of its own kind, unless it's the start (an edge or vertex on a revolution's axis, which the turn leaves where it is);
- its **sides**, OCCT's `Generated(element)`, as `generated`: a face its solid, an edge its side face, a vertex its side edge.

The sweeps build with OCCT's `Copy` flag (`BRepPrimAPI_MakePrism(S, V, true)`, `BRepPrimAPI_MakeRevol(S, A, D, true)`): on copies of the faces' edges and vertices, so the faces given are left as they were. Without it OCCT shares the faces' edges with the result's start face and adds the new faces' curves and locations to them, so the faces' bytes change and each later sweep of the same faces in a process writes different bytes (found by break-it: 3649, 4659, 5670 bytes for three identical revolutions). `FirstShape`, `LastShape`, and `Generated` still answer for the faces' own elements, so the history is the same, but for a whole turn's edges square to the axis, which report their flat faces with the copy and nothing without it.

Positions are 1 for the start and the end, which aren't in any OCCT list, and each side's place in `Generated`'s. The kind tells an end from a side: an edge's end is an `Edge`, its side a `Face`. `History::ends` stays empty; M1's polygon sweeps keep theirs. Measured on OCCT 7.9.3 (`crates/silico-kernel/tests/sweep_faces.rs` and `adversarial_sweep_faces.rs`, every case pinned):

| Sweep | Faces | Edges | Vertices |
|---|---|---|---|
| Prism | start, end, solid | start, end, one side face | start, end, one side edge. So the result is exactly 2F + E faces, 2E + V edges, 2V vertices, F solids. |
| Revolution, short of a whole turn | start, end, solid | off the axis (one end on it included): start, end, one side face; along the axis: the start only, which is its end, and no side | off the axis: start, end, and one side edge, an arc from the start to the end; on the axis: the start only, which is its end, and as sides only degenerate edges, each a point at the start: none where no face of revolution closes there (a corner where a side along the axis meets one square to it), one where one does (a cone's apex, a triangle's corner on the axis), two where two do (a diamond's tip: `MakeRevol` copies the degenerate edge two faces would share) |
| Whole turn | nothing | sweeping a face of revolution (a cylinder, a cone, a torus): its start, which is that face's seam, and the face; square to the axis (sweeping a flat annulus or disc): the face, no start; along the axis: nothing | off the axis: its start, itself, and its circle; on the axis: its start and degenerate edges where a face closes there (a diamond's tip), nothing where none does |

The whole turn's face is reported nothing though its solid is in the result: OCCT's `IsUsed` answers false for it in a closed revolution (docs/ports/extrude-revolve.md found the same for M1's revolve). Design S4 expected every seam of a whole turn to be named from above, by the face it bounds; OCCT's `FirstShape` reports the seam of a face of revolution as its edge's start, so under S3's rule that seam has a history (`modified` from its curve) and S4's naming (M3.6's step v) decides whether to use it.

`History::sweep_fault` checks the rule, and the core refuses a reply that breaks it as a protocol error (`KernelRequest::prism`, `prism_through_all`, `revolution`): never kept, at most one start, an end only of the element's own kind, sides only of the next kind up, no result element reported for two input elements, a solid for each face; for a prism, every element with exactly one of each and the counts above; for a turn, the table's row exactly, the arcs and the degenerate edges told apart by the result's topology (an arc's vertices are its start and end, a degenerate edge's one vertex is the start). A whole turn is told by its angle as OCCT tells it (within `Precision::Angular()` of 2π). An earlier rule (an edge or vertex has both an end and a side, or neither) refused every cone's apex as the worker's fault; break-it found it.

The kernel also refuses a sweep that would make a flat "solid", which OCCT builds and calls valid (volume 0): a prism whose vector lies within 1e-7 of the faces' plane (`check_leaves_plane`, M1's extrusion rule) and a turn whose axis leans more than 1e-7 out of it (`check_axis_in_plane`), both `Refusal::SweepFlat`, and any sweep whose result has no volume (`has_no_volume`, as a boolean's). The core's vectors are the plane's normal and its axes lines of the sketch, so these are the kernel's own guards.

## Checked against FreeCAD

FreeCAD's element names record history: a result face named `Face3;:M;FLT;...` was modified from input `Face3` by a fillet, `Edge1;:G;FLT;...` generated from input `Edge1`, and a plain `Face2` kept. Silico's history is compared with those names (the relations FreeCAD's names state directly, not the ones it derives from neighbours).

## The roadmap's tests, corrected

The M1.4 plan's two examples don't match what OCCT reports (verified on OCCT 8.0.1 for this note, and by the tests on 7.9.3):

- **Filleting Edge1 of a 10 × 20 × 30 box (r = 2):** Edge1 is deleted and generates one face. **Four** faces are modified, not two: the two the edge joined, and the two end caps whose outlines change. **Two** faces are kept, not four. Edges 2, 4, 9, and 10 are modified; the rest of the edges, and six of the eight vertices, are kept.
- **Cutting a cylinder through a box:** the box's top and bottom faces are modified, each also generating the hole's circle edge and its seam vertex. The cylinder's side face is **modified** into the hole's face (not generated); it generates the two circle edges. The cylinder's caps, circle edges, and vertices are deleted.

## FreeCAD tests

| Test | Status |
|---|---|
| `tests/src/Mod/Part/App/FeaturePartCommon.cpp` `testHistory`: two 1 × 2 × 3 boxes, the second at (0, 1, 0), common; face histories `{0:[0], 1:[5], 2:[], 3:[2], 4:[3], 5:[1]}` and `{0:[0], 1:[5], 2:[4], 3:[], 4:[3], 5:[1]}` (0-based; the assertions were dropped in `ecf7e51ab3` and are recovered from its parent) | Ported with booleans (`crates/silico-kernel/tests/boolean.rs`). |
| `tests/src/Mod/Part/App/TopoShapeExpansion.cpp` `makeElementFillet`, `makeElementChamfer`, `makeElementPrism`, `makeElementRevolve`, `makeElementCut`, `makeElementFuse`: the raw relations under their element names | Ported with each operation in M1.4 and M1.5, as relations decoded from the names: fillet (`history.rs`), and booleans, prism and revolve, and chamfer with positions too (`boolean.rs`, `sweep.rs`, `chamfer.rs`). |
| `TopoShapeExpansion.cpp` `MapperMaker*`, `MapperHistory*` | Not ported: they test FreeCAD's mapper classes, which Silico doesn't have. |
| `tests/src/Mod/Part/App/PropertyTopoShape.cpp` `testShapeHistory` | Not ported: it tests FreeCAD's legacy history map, whose counts follow from that map's own rules (Edge1's generated face isn't an edge, so it gets no entry), not OCCT's history. |
| `src/Mod/Part/parttests/TopoShapeTest.py:731` `testTopoShapeMakeFillet` (element map 42); `src/Mod/PartDesign/PartDesignTests/TestTopologicalNamingProblem.py:1043` `testPartDesignElementMapFillet` (26 faces, 56 edges, 24 vertices), `:1822` `testPartDesignTNPFillet` | The element maps ported with naming (M2.7, docs/ports/naming.md); `testPartDesignTNPFillet`, an edit, with references (M2.8). |

## For M1.5

Found while researching this note, for the operations that come next:

- **Booleans** (done, docs/ports/booleans.md): in OCCT's default (destructive) mode, a boolean reuses the input's own sub-shapes in the result, so they're reported unchanged, and the result shares sub-shapes with the inputs. In non-destructive mode they're new, reported as modified, as FreeCAD's names expect. FreeCAD always uses non-destructive mode, and so does Silico.
- **Revolve** (done, docs/ports/extrude-revolve.md): a full 360° revolve reports its profile face and its radial edges deleted with nothing generated, though the end faces come from them, and its `FirstShape` and `LastShape` are the input face, which isn't in the result (OCCT's own DRAW test `bug30346_1` expects this). Its history needs its own handling.
- **Extrude** (done): the profile face generates the whole solid and is kept as the bottom (`FirstShape`); `LastShape` is the top; each profile edge generates a side face, and each vertex a side edge.
- **Chamfer** (done, docs/ports/chamfer.md): the same history as the fillet's (the chamfered edge deleted, generating the chamfer face; the faces it joined and the end caps modified), checked against FreeCAD's `CHF` names with positions.
