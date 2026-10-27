# Mesh export (3MF)

A part written as a 3MF file a slicer prints from: each solid tessellated at an export tolerance, its faces' triangles joined into one closed mesh, checked, and written as a `type="model"` object in millimetres, in an archive whose bytes are always the same. This is M4.8 (docs/roadmap/m4-mcp.md on the `m4-plan` branch). It builds on the display mesh (docs/ports/tessellation.md), which meshes each face on its own, and doesn't repeat it.

**Built (the export core, as a library; /break-it's findings fixed in the same branch):** `silico_kernel::export_mesh` and `Op::ExportMesh` report each face's triangulation and its polygons on its edges; `silico_core::export::printable` joins and checks them in the order below; `silico_core::export::three_mf` writes the archive. The worker meshes with the tolerance less the most 32-bit rounding can move the part's vertices (`export_deflection`, from its bounding box): OCCT's own edge figure is the deflection it was asked for, not a measurement, so only meshing finer leaves room for the rounding, and a tolerance whose rounding would take more than half of it is refused, naming it. **Built (M4.8, the command):** the `export` command (`silico_core::command::export`, `Sessions::export`, on the read lane, reading the current model) and `silico export` (docs/cli.md), as M4.1's `export` designs them: the request checked in the design's order, each solid meshed on the read's own kernel client, joined and checked by `printable`, written by `three_mf` once the read settles, through the writer every command's files share (decision 35). STEP (decision 6) follows, its study in docs/ports/step-export.md.

Citations are to FreeCAD 1.1.3 (tag `1.1.3`, commit `145529fe74`) unless marked **main** (`876271d660`, 2026-10-07). OCCT citations are to 7.9.3, the version Silico and the oracle share. The 3MF Core Specification is v1.4.0 (`3MFConsortium/spec_core`, `373e91d066`), cited as `core.md:line`; PrusaSlicer is 2.9.6 (`src/libslic3r/Format/3mf.cpp`), cited as `PS 3mf.cpp:line`. The fetched copies are in `/tmp/mesh-export-study/`.

## What FreeCAD does

`Mesh.export(objects, filename, tolerance)` (`src/Mod/Mesh/App/AppMeshPy.cpp:76-85`, `209-304`) picks the format from the file's extension (`:265`). For 3MF it makes an `Exporter3MF` (`:277-283`), adds each object (`:298`), and writes the file when the exporter is destroyed (`:301`). The GUI's File > Export runs the same function (`src/Mod/Mesh/Init.py:24`).

### The tolerance

- It's the `tolerance` argument, or the preference `Mod/Mesh/MaxDeviationExport`, or 0.1 (`AppMeshPy.cpp:219`). The GUI accepts 1e-5 to 1e8 mm (`src/Mod/Mesh/Gui/DlgSettingsImportExport.ui:46-56`). The Python path doesn't check the value at all (`AppMeshPy.cpp:224-238`).
- It reaches the mesher as a `float`, because `Exporter::addObject(App::DocumentObject*, float tol)` takes one (`src/Mod/Mesh/App/Exporter.h:62`). So the deflection is 0.1f, which is 0.10000000149011612, not 0.1.
- `TopoShape::getFaces` (`src/Mod/Part/App/TopoShape.cpp:3650-3669`) runs `BRepMesh_IncrementalMesh(shape, tol, isRelative = false, defaultAngularDeflection(tol), isInParallel = true)`. The deflection is **absolute**, in millimetres.
- `defaultAngularDeflection(tol)` is `min(0.1, 5·tol + 0.005)` radians (`TopoShape.cpp:224-232`, from `487215d2f5`). At 0.1 mm that is 0.1 rad. OCCT allows face interiors twice that angle, 0.2 rad (`AngleInterior = 2·Angle`, `BRepMesh_IncrementalMesh.hxx:103-105`): a looser limit inside a face, not a finer one.
- **The angle governs most curves, not the 0.1 mm.** A circle of radius r needs n ≥ 2π/0.1 = 63 segments to meet the angle (OCCT's mesh of a cylinder of r 5 has 126 per circle, in FreeCAD's export as in Silico's). It needs only n ≥ π/arccos(1 − 0.1/r) segments to meet the deflection, which is 16 at r = 5. The two limits cross at r = 0.1/(1 − cos 0.05) ≈ 80 mm. Below that radius, FreeCAD's export mesh is set by the angle.
- `MeshPart.meshFromShape` (`src/Mod/MeshPart/App/Mesher.cpp:227-239`, `createStandard`) is a different path. It is not export. It runs `BRepTools::Clean` first, takes its own `LinearDeflection`, `AngularDeflection` (default 0.5) and `Relative` (`Mesher.h:228-231`), and then joins faces through the same `Part::BRepMesh` as export.

OCCT treats the deflection as a target, not a bound:

- An edge's deflection is raised to the gap between its vertices and its curve's ends (`BRepMesh_Deflection.cxx:68-95`).
- A face's deflection is the largest of three values: the one asked for, the mean of its wires' deflections, and twice its largest tolerance (`BRepMesh_Deflection.cxx:127-152`).
- After meshing, OCCT estimates each face's deviation, as the distance from each triangle's centroid, and each interior link's midpoint, to the surface point at its UV (a sample, not a bound), skipping boundary links and the triangles at degenerate edges. It stores the result as the triangulation's `Deflection()` (`BRepLib::UpdateDeflection`, `BRepLib.cxx:2591-2687`, called from `BRepMesh_ModelPostProcessor.cxx:184`).
- `getFaces` doesn't clean the shape first. OCCT keeps an existing triangulation whose deflection is under 1.1 times the one asked for (`BRepMesh_Deflection::IsConsistent`, `BRepMesh_Deflection.cxx:159-170`, used by `BRepMesh_ModelPreProcessor.cxx:64-94`).
  - In a GUI session, a finer display mesh on the same faces could therefore be exported instead. Not verified: whether the view provider's shape shares its faces with the shape that `getFaces` meshes.
  - Headless, as the oracle runs, there is no earlier mesh.

### Meshing and joining the faces

1. **Per face** (`TopoShape::getDomains`, `TopoShape.cpp:3582-3628`): faces come in `TopExp_Explorer` order. Each face's nodes are moved by its location, and two corners of each triangle are swapped when the face isn't `FORWARD` (`Tools::getTriangulation`, `src/Mod/Part/App/Tools.cpp:255-326`, swap at `:318-320`). A face with no triangulation gets an empty domain (`TopoShape.cpp:3595-3600`). The faces' meshes are left as they are, so an unmeshed face leaves a hole.
2. **Exact merge** (`Part::BRepMesh::getFacesFromDomains`, `src/Mod/Part/App/BRepMesh.cpp:241-298`): every node goes into a `std::set` ordered on exact coordinates, so bitwise-equal points become one. Each point keeps the index of its first occurrence (`:253-259`). A triangle whose corners now repeat an index is dropped (`:275-279`). This is what removes the triangles at a sphere's pole.
3. **Tolerance merge** (`MergeVertex`, `BRepMesh.cpp:67-237`, run at `:292-297`): points within `Precision::Confusion()` (1e-7) on every axis are merged. This step has three problems:
   - It sorts with a comparator that tests "differ by at least 1e-7" axis by axis (`:128-139`), which isn't a strict weak ordering. Close points can sort apart, and which index survives depends on the library's `std::sort`.
   - Triangles that this step makes degenerate are **not** dropped (`:175-182`).
   - Nothing stops it from merging two distinct nodes that happen to lie closer than 1e-7, as on a sliver face.
4. **To the mesh object**: points become `float` (`src/Mod/Mesh/App/Mesh.cpp:656`), and neighbours are rebuilt (`MeshKernel::Adopt`, `Core/MeshKernel.cpp:110-118`). The mesh carries the object's placement as a transform (`Exporter.cpp:132`) and is named by its `Label` (`Exporter.cpp:143`).

How the merge tolerance changed:

| Commit | Merge rule |
|---|---|
| `120ca87015` (2011, a refactor of the DLL defines, already with the merge) | a fuzzy `std::set` at `gp::Resolution()` (about 1e-290), which in practice means exact |
| `ad605d6e23` (2024, #11985) | the same fuzzy set at 10·DBL_EPSILON |
| `eb32abe7ef` (2024, #12744) | the exact set, then the 1e-7 pass |

### The checks

`Writer3MF::GetType` (`src/Mod/Mesh/App/Core/IO/Writer3MF.cpp:151-155`) returns `"model"` if `forceModel` is set, or if `MeshEvalSolid` passes; otherwise it returns `"surface"`. `forceModel` defaults to true (`Writer3MF.h:109`, and the preference `Export3mfModel`, default true, at `AppMeshPy.cpp:222`). So **FreeCAD checks nothing before writing**: every object is `type="model"`, closed or not.

When the evaluator does run (preference off), here is what each check does:

| Evaluator (`src/Mod/Mesh/App/Core/`) | What it checks | Run on export? |
|---|---|---|
| `MeshEvalSolid` (`Evaluation.cpp:294-306`) | no border edge, i.e. no triangle edge without a neighbour (`MeshKernel::GetEdges`, `MeshKernel.cpp:1266-1287`). An edge with 3 or more triangles gets no neighbours (`RebuildNeighbours`, `Evaluation.cpp:974-1044`, "a non-manifold that is ignored", `:1007`), so it also counts as a border. | Only with `Export3mfModel` off |
| `MeshEvalOrientation` (`Evaluation.cpp:139-160`) | that neighbouring triangles wind their shared edge in opposite directions. A mesh wound inward everywhere passes. | No |
| `MeshEvalTopology` (`Evaluation.cpp:340-475`) | edges with more than two triangles | No |
| `MeshEvalPointManifolds` (`Evaluation.cpp:476-522`) | points where triangle fans meet without sharing an edge | No |
| `MeshEvalCorruptedFacets` (`Degeneration.cpp:1172-1184`) | triangles with a repeated point index | No |
| `MeshEvalDegeneratedFacets` (`Degeneration.cpp:473-483`, via `MeshGeomFacet::IsDegenerated`, `Elements.cpp:639-680`) | zero-area triangles: \|u×v\|² ≤ ε·max(\|u\|², \|v\|²) | No |
| `MeshEvalSelfIntersection` (`Evaluation.cpp:603`) | triangles that cross | No |

There is no check that the mesh faces outward. `MeshKernel::GetVolume` returns the absolute value of a `float` sum (`MeshKernel.cpp:1207-1231`), so a mesh wound inside out reports a positive volume.

### The archive

Written by `Writer3MF` (`Writer3MF.cpp:38-212`) through zipios++ (`src/3rdParty/zipios++/`, which is the same at 1.1.3 and main).

- **Entries**, in this order: `3D/3dmodel.model` (`:41`), `_rels/.rels` (`:94`), `[Content_Types].xml` (`:100`), then any extra resources (`:105-109`). The only extra resource is a thumbnail, and only in a GUI session (`src/Mod/Mesh/Gui/ThumbnailExtension.cpp:78-98`).
- **Content types** (`:198-212`): `Default` entries for `rels` and `model`, and one more `Default` for each resource. Two thumbnails therefore give two `png` defaults.
- **Relationships** (`:180-196`): `/3D/3dmodel.model` as `rel0` with the 3dmodel type, then resources as `rel2`, `rel3`, and so on.
- **Model** (`:57-75`, `:114-149`):
  - `<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">`, with `<metadata name="Application">FreeCAD</metadata>`.
  - One `<object id="N" type="model">` per mesh, numbered from 1. Its vertices are in the object's own coordinates.
  - Its placement goes on the build item: `<item objectid="N" transform="m00 m10 m20 m01 … m23"/>`, transposed as the specification orders it (`:157-178`). An identity placement is written out in full too.
  - **No `name` attribute at 1.1.3.** `Exporter3MF::addMesh` discards the name (`Exporter.cpp:307`). Main writes `name="Label"`, escaped (`49c80ebe5c`).
- **Numbers**: `float` values written with the stream's default formatting (`Writer3MF.cpp:131-132`), which gives 6 significant digits. A coordinate of 123.4567 becomes `123.457`. From 1000 mm up, the step is 0.01 mm, and two nodes closer than the step print the same.
- **Zip**:
  - Each entry is deflated at level 6 by zlib (`zipoutputstreambuf.cpp:26`, `deflateoutputstreambuf.cpp:68`).
  - Each entry's time is the local wall clock when it closes (`zipoutputstreambuf.cpp:151-156`).
  - The external attributes are fixed at `0x81B40000` (`ziphead.h:115`).
  - The local header is rewritten in place after the data (`zipoutputstreambuf.cpp:159-161`), so there are no data descriptors.

### Open meshes and several solids

- **An open mesh** (a face OCCT couldn't mesh, an open shell, a sheet) is written as `type="model"` with its holes (`7bbc93e218`). A shape with no faces, such as a wire, gives an `<object>` with no triangles. Neither `Exporter::addObject` (`Exporter.cpp:105-149`) nor `Writer3MF::AddMesh` (`Writer3MF.cpp:77-82`) checks for this, although `ExporterAMF` does (`Exporter.cpp:415-417`). The specification wants at least one triangle, and four for a model (core.md:648).
- **Several solids in one object** (a compound) go through one `getFaces` call. They become **one** 3MF object whose points are merged exactly across the solids. So two solids that share a face or an edge come out with edges used by four triangles. This follows from `BRepMesh.cpp:253-297`; we didn't run it.
- **Several document objects** give one 3MF object and one build item each (`Exporter.cpp:108-147`).

## What PrusaSlicer requires

From PrusaSlicer 2.9.6's reader:

- It looks for the model at `3D/3dmodel.model`, "the only format of the string which works with CURA" (PS 3mf.cpp:88, `:708`, `:753`). An archive without a `.model` entry is refused (`:809`).
- **Only objects of type `model`, or with no type, are loaded.** It skips `surface`, `support`, `solidsupport` and `other` (`:210-221`, `:320-331`, `:2015`). When every object is skipped, the user sees "The size of the object is zero" (FreeCAD issue #10075).
- It applies the `unit` (`:301-318`, `:1961`) and reads coordinates as `float` (`:250-256`, `:2103-2111`).
- It uses the object's `name`, or the file's name and a number when there is none (`:2025-2027`).
- It refuses out-of-range indices (`:2588`, `:2618`).
- An object with no build item is dropped (`:2016`).
- When a mesh has open edges, it warns and offers to repair it (FreeCAD issue #11985, from the report; we didn't trace this in PrusaSlicer's source).

The 3MF specification asks the same of a `model` mesh:

- Every edge is shared, by vertex index, with exactly one other triangle (core.md:575).
- Neighbouring triangles have consistent orientation (`:576`).
- Normals point outward, so the volume is positive (`:577`).
- A triangle's three indices are distinct (`:671`).
- Triangles SHOULD have non-zero area (`:588`).
- Self-intersection and overlapping objects are allowed for consumers, but producers SHOULD NOT make them (`:587`).
- Producers SHOULD NOT repeat vertices "unless coalescing duplicates would create non-manifold edges" (`:622`).
- Zip entries are stored or deflated, and plain zip SHOULD be used rather than ZIP64 (`:155`).

## Guards, from FreeCAD's history

All of these are in 1.1.3 (checked with `gh api compare`), except where marked main only.

| FreeCAD commit | Guard | In Silico |
|---|---|---|
| `e3ebe4bc9f` (2021) | 3MF export, every object `type="model"` | `type="model"`, after the checks below |
| `79df90a45c` (2022, #7018) | No leading `/` in the zip entry's name (`/3D/3dmodel.model` made a 3MF PrusaSlicer couldn't load) | Entry names without a leading slash |
| `580c124b22` (2022) | `type="surface"` for a mesh `MeshEvalSolid` rejects; the placement moved from the vertices to the build item | Not ported: a mesh that isn't closed is refused, never written as a surface |
| `7bbc93e218` (2023, #10075) | `type="model"` always by default (`Export3mfModel`), because PrusaSlicer and 3D Builder drop `surface` objects | Kept in effect: only `model` is ever written. FreeCAD writes open meshes this way; Silico refuses them (below). |
| `487215d2f5` (2020) | Angular deflection `min(0.1, 5·tol + 0.005)` instead of OCCT's 0.5, because OCCT didn't hold the linear deflection on curved B-spline faces | Ported, so the meshes match the oracle's |
| `ad605d6e23` (2024, #11985) | Merge points within 10·DBL_EPSILON (a placed clone's 3MF had open edges) | Not ported: faces are joined at the edges they share, not by distance (below) |
| `eb32abe7ef` (2024, #12744) | Exact merge, then a 1e-7 pass; the regression `ad605d6e23` caused; tests in `tests/src/Mod/Part/App/BRepMesh.cpp` | Not ported, for the same reason. Its tests are ported (below). |
| `0d37ea2629` (2024) | One joining code (`Part::BRepMesh`) for export and `meshFromShape` | One weld, in Rust |
| `e267eac7bb`, `0f9eba7d96` (2018, #3329), `b52694c87b` (2023, #10545), `b1a96bde5f` (2023, #11539) | Placements applied exactly once (global placement, array members, links) | Not needed until features have placements. Vertices are written in document coordinates. |
| `04005b5e45` (2018, #3680) | Refuse an export with nothing that can be meshed ("None of the objects can be exported") | An export with no solid is refused |
| `da0f810282` (2017) | No empty meshes (AMF; the 3MF path never got this check) | A solid with no triangles is refused |
| `8f18cfaf3a` (2024) | Reading: zips zipios++ can't open are rewritten through Python's `zipfile` (`Core/MeshIO.cpp:112-130`, `:261-268`) | Write plain zip (sizes in the local headers, no data descriptors, no ZIP64, no extra fields), so the oracle reads it directly |
| `49c80ebe5c` (2026, main only) | Each object's `name` attribute, escaped | Ported: each object named by its feature |
| `2aaf297522` (2026, #20432, main only) | One thumbnail for the package, not one per object | Not ported: no thumbnail (optional, core.md:193) |

## FreeCAD 1.1.3 and main

These are the differences in the files this note reads. The rest are formatting or lint changes.

- **`Writer3MF.cpp`, `Exporter.cpp`**: main writes `name="…"` (`49c80ebe5c`), and makes one thumbnail from all the meshes merged (`2aaf297522`, `abf55bb76c`). 1.1.3 writes no name and makes one thumbnail per mesh. `FileException` became `FileWritePermissionException` (`0bdd8f3767`), and `-Werror` fixes went in (`8271904dee`). No change to geometry, type or layout.
- **`Reader3MF.cpp`**: main reads `name` (`5872f6a05f`), reads multi-file 3MF, and loads build items when there are no components (`23cc0c6822`). 1.1.3 reads only `3D/3dmodel.model` (`Reader3MF.cpp:50`, `:58`), ignores `unit` and `type`, and parses coordinates with `std::stof` (`:435-437`). The oracle is 1.1.3, so it never sees Silico's names.
- **`AppMeshPy.cpp`, `Mesher.cpp`, `Core/MeshIO.cpp`, `Core/MeshKernel.cpp`, `Mesh.cpp`**: logging, exception types, and the `Save3MF` name. No change to the tolerance or the mesher.
- **`TopoShape.cpp` (`getDomains`, `getFaces`), `BRepMesh.cpp`, `Evaluation.cpp`, `Degeneration.cpp`, zipios++**: identical.
- **`MeshTestsApp.py`**: the GUI's `PivyTestCases` moved to their own file (`1a467bf9a9`).

## A printable mesh, as Silico settles it

A solid's export mesh is printable when all of the following hold, checked in this order before anything is written. Each refusal names the faces at fault, as the display mesh's refusal does (the first hundred, and the total).

1. **Every face meshed.** A face with no triangles leaves a hole. The refusal names the face, whether or not the B-rep is valid. This is stricter than the display mesh, which leaves out an unmeshed sliver on a valid shape (docs/ports/tessellation.md).
2. **Joined at the edges the faces share, without a tolerance.** Nodes on a B-rep edge are identified through each face's polygon on its triangulation for that edge (`BRep_Tool::PolygonOnTriangulation`; a seam has one polygon for each orientation), and nodes on a B-rep vertex through that vertex. Every node of a degenerated edge (a sphere's or cone's pole) welds to that edge's vertex: its polygon belongs to one face only, so pairing polygons alone wouldn't merge them. Each welded vertex takes its position from the first face in `TopExp::MapShapes` order.
   - This is exact where FreeCAD's coordinate merge isn't. It neither misses nor wrongly merges.
   - It doesn't depend on two faces with different locations rounding their shared nodes the same way. Today's watertightness test (`closed_solid_mesh_watertight`) joins by bitwise-equal coordinates and passes for a box, a cylinder, and a box minus a cylinder.
   - A face's own nodes at exactly one place (the same coordinates, bit for bit) are one vertex. OCCT leaves two inside a cone's face at times (NIST's CTC 01 and FTC 06 at 0.1 mm, Face58 and Face60 of CTC 01). This is exact, and within one face only, so two faces still meet only at their edges.
   - A triangle whose corners weld to a repeated index is dropped, as `BRepMesh.cpp:275-279` drops it. Its two remaining edges cancel each other, so closure is unaffected. A sphere's poles, and those coincident nodes, are the known cases. With them, CTC 01 exports with FreeCAD 1.1.3's 10,876 facets.
3. **Closed and manifold**: every edge, by vertex index, has exactly two triangles (core.md:575).
   - An edge with one triangle names its face.
   - An edge with three or more names all of them. This is a solid that touches itself along an edge, which OCCT builds and calls valid (docs/ports/fillet.md lists its edges).
   - The specification would allow duplicating vertices to split such an edge (core.md:622), but pairing the faces needs their order around the edge. Left for later.
   - Solids are welded separately, so two touching solids never share an index.
   - A vertex where two fans meet only at a point is allowed, as the specification allows it.
4. **Consistently oriented**: each shared edge runs opposite ways in its two triangles (core.md:576). A conflict names both faces.
5. **No two corners of a triangle at one place in the coordinates as written**, i.e. `f32` (core.md:671 asks for three distinct indices; a reader merging by coordinate would see two). This catches vertices that are apart in 64 bits but one in 32, such as a part too thin for its distance from the origin. The refusal names the face, and says to move the part nearer the origin or make the face larger. A triangle whose corners are apart but on a line (a needle) is written, with no repair: 3MF asks only that triangles SHOULD have area (core.md:588), FreeCAD writes them, PrusaSlicer 2.9.6 removes only triangles with a repeated index and keeps a needle (`its_remove_degenerate_faces`, `TriangleMesh.cpp:797-801`), and OCCT leaves them on cones (Face60 of CTC 01).
6. **Facing outward**: each shell's signed volume, computed in `f64` from the coordinates as written, is positive for the solid's outer shell and negative for a cavity's shell (`BRepClass3d::OuterShell` says which). A shell with no triangles encloses none and is refused. Each solid's total is positive, so a cavity larger than its solid, or a solid of a cavity alone, is refused. FreeCAD has no such check, and its volume is an absolute value.
7. **Meshed at the tolerance** (decided, m4.8-export-fable). The export tolerance is the mesher's linear deflection, with FreeCAD's angle, as FreeCAD, SolidWorks, and Fusion take it. Silico guarantees exactly what follows, from OCCT's own figures and the B-rep's data:
   - Every vertex is a node OCCT placed on a face, an edge, or a vertex of the solid, then rounded once to `f32`, by at most max|coordinate|·2⁻²⁴ on each axis, so by r = √3·max|coordinate|·2⁻²⁴ in all. That rounding isn't small beside every tolerance: a 10 mm cube at x = 5·10⁴ mm rounds by 3e-3 mm. So the worker meshes with the tolerance less r, from the shape's bounding box (`export_deflection`), and the export is refused, naming the tolerance, when r would take more than half the tolerance. The finest tolerance, 1e-5 mm, is then taken out to 48 mm from the origin.
   - Each edge's polygon's deflection, OCCT's figure for it after any raise (`Poly_PolygonOnTriangulation::Deflection()`), is within the tolerance less r, or the faces on that edge are refused, naming them. OCCT raises an edge's deflection to the gap between its vertex and its curve's end (`BRepMesh_Deflection.cxx:95`).
   - Each joined vertex's nodes are within the tolerance less r of the node it's placed at.
   - No face was meshed more coarsely for its own tolerance (revised, m4.8-export-fable-2b). OCCT meshes a face no finer than twice the largest tolerance of the face, its edges, and its vertices (`BRepMesh_ShapeTool::MaxFaceTolerance`, `BRepMesh_Deflection.cxx:149-152`). Where twice that is past the deflection (OCCT's own test, `needs_unraised`), the worker meshes the shape a second time with `IMeshTools_Parameters::ForceFaceDeflection`, which drops only that floor and the wires' mean (it's read only in `BRepMesh_Deflection::ComputeDeflection`, `BRepMesh_Deflection.cxx:137`). Each raised face's nodes are compared with its unraised nodes, by their bits, in any order:
     - The same nodes are taken, and logged. The raise changes nothing on a plane, whose splitter places no nodes inside it (`BRepMesh_DefaultRangeSplitter.cxx:103-107`), or on a cylinder, whose splitter's V loop never runs (`BRepMesh_CylinderRangeSplitter.cxx`: `Reset` sets the scale before `myDu`, and `nbV` is 0). On a sphere, cone, or torus it often rounds to the same step count. The triangles can differ where the nodes don't: the face's deflection also sets the scale a cylinder is triangulated in (`myDu`, `BRepMesh_CylinderRangeSplitter.cxx:29-32`, through `computeDelta`, `:92`), so its quads' diagonals can fall the other way (FreeCAD 1.1.3, a 100 mm cylinder 0.06 mm loose at 0.1: 252 points either way, other diagonals; and the port review's 16 cylinders cut obliquely, of radius 100 to 400: the same nodes in each, other diagonals in three, and the worst distance from the surface the same in all).
     - Other nodes are refused, naming the faces and the tolerance, with a tolerance to export at: twice the loosest refused face's tolerance plus the rounding margin the worker meshed with, rounded up to the nanometre (a nanometre more where the worker's own subtraction would land short). For example, a 50 mm sphere whose sub-shapes carry 0.02 mm is meshed at 0.04 when asked for 0.01, 3.9 times off, its face's 25,333 nodes unraised becoming 13,476 (FreeCAD's merged points: 25,217 and 13,360). It's refused, suggesting 0.040006 mm. At 0.04 it's raised from 0.04 less the margin, places the same nodes, and is exported. A B-spline face moves its nodes with its deflection (deflection control, `BRepMesh_DelaunayDeflectionControlMeshAlgo.hxx:423`), so the same sphere as a B-spline is refused at 0.01.
     - The exported mesh is always the first, FreeCAD's. The unraised one is only compared with.
   - **A face's interior is OCCT's to meet.** FreeCAD never compares a face's deviation with the tolerance, and OCCT's estimate of it (`Poly_Triangulation::Deflection()`, `BRepLib.cxx:2687`, a sample at each triangle's centroid in the face's parameters) is neither a bound nor a measure: it overstates a cone's by 10 to 1000 times (1.277 mm where the mesh is 1.0e-3 mm off). It is kept in the report for diagnosis only. Measured analytically, interiors hold on planes, cylinders, cones, and tori. On a sphere they miss: OCCT's sphere mesher does no deflection control (`BRepMesh_MeshAlgoFactory.cxx:74-77`), and by the seam its Delaunay loses the grid's nearest nodes. A band of triangles there spans 1.7 grid steps, their sagitta up to 2.1 times the tolerance. This is FreeCAD's export too, and it's pinned in kernel adversarial_export.rs as worst deviation over tolerance:

     | Sphere radius | Tolerance | Worst / tolerance |
     |---|---|---|
     | 5 | 0.01 | 0.386 |
     | 20 | 0.01 | 1.545 |
     | 50 | 0.01 | 1.471 |
     | 100 | 0.01 | 2.078 |
     | 50 | 0.1 | 1.314 |
     | 300 | 0.1 | 0.761 |
     | 1000 | 0.1 | 2.078 |

     OCCT's `EnableControlSurfaceDeflectionAllSurfaces` doesn't cure it: it leaves spheres 1.2 to 1.6 times off, makes one case worse, and gives a flat cone 7 times the triangles for nothing. The exact way to keep the promise for interiors, if it's wanted, is a closed-form deviation per triangle for spheres and cylinders, and a bounded remesh at a finer deflection. It's not built (the maintainer's guidance: follow FreeCAD on OCCT's wrong results, no costly verification).

## Why 32 bits

Every vertex is rounded to `f32` before any check, and written as the shortest decimal that reads back to that `f32`. The readers parse coordinates into 32-bit floats: PrusaSlicer 2.9.6 through `fast_float` into `float` (PS 3mf.cpp:250-256, :2103-2111), and FreeCAD 1.1.3 through `stof` into its mesh's `float` points (`Reader3MF.cpp`, `src/Mod/Mesh/App/Mesh.cpp:656`). So the checks are made on what they will read, and the file says exactly what was checked. Digits past `f32`'s would be dropped by every reader. Two vertices distinct in 64 bits but one in 32 would merge there, and a triangle could go flat after Silico had called it printable. Rounding first makes rule 5 catch that case, and core's tests/export.rs `vertices_apart_in_64_bits_but_one_in_32_are_refused_before_anything_is_written` holds it. Writing `f64` text would be no less deterministic, but it would say more than was checked.

## Open

- **A mesh too large to make in time** (decided, m4.8-export-fable): FreeCAD's angular rule gives 0.01 rad at 1e-3 mm whatever the part's size. A 1 mm sphere then makes 805,418 triangles, which take OCCT about three minutes (FreeCAD's own export takes 201 s). No exact rule says before meshing how many triangles OCCT will make, and a deadline would depend on the machine, so the request ends as a loud `worker_timeout` naming the export. The deterministic limit is a count of the triangles made (decided, M4.1's table of the tools' limits): 10,000,000, `MAX_EXPORT_TRIANGLES`, applied by the worker to each mesh it makes before it sends it (the unraised one too), and by the 3MF over every part. Past it, the export is `invalid_input` naming the tolerance, a coarser one making fewer. An archive past a plain zip's 4 GiB is refused the same way; ten million triangles make about 0.9 GB. The `export` command's advice for the timeout: "export with a coarser tolerance: a finer one makes a larger mesh, which takes longer". The timeout bounds memory too: a mesh at 0.001 mm on curved faces takes OCCT about 50 s and 3 GB, so an export meshes one solid at a time, and every session of a process takes turns with the others (`command::export::MeshTurns`, held while a solid is meshed and checked, given in the order asked for, so an export asking again for its next solid goes behind those waiting, and a wait for it ended by a cancel): one export mesh in flight at once, however many bodies or sessions, never one for each core. For M4.1's design: take the deviation and the angle as two settings, as SolidWorks does, the angle defaulting to FreeCAD's rule (`487215d2f5` itself calls it a heuristic).
- **Faces in no solid:** a STEP file can carry faces outside its solid, as 11 of NIST's 33 parts do. FreeCAD exports them into the mesh, which is then open. Silico refuses them, naming them, at every tolerance. An option to leave them out could come with the `export` command.

## Where Silico differs on purpose

- **Refused, never written.** FreeCAD writes every mesh as `type="model"`, holes and all (`7bbc93e218`). A slicer then repairs it, or prints it wrong. Silico refuses a mesh that fails the checks above, naming the faces, and writes nothing.
- **The tolerance is a length, in f64.** 0.1 means 0.1, not 0.1f. If 0.1f vs 0.1 ever changes a node count, the oracle case below shows it. Within limits, and not the display mesh's: its floor, max(1e-4 × the diagonal, 1e-7) (`crates/silico-kernel/src/lib.rs:1576`, `:1593`), would refuse the 0.1 mm default on any part over 1000 mm across, which FreeCAD exports. Recommended: FreeCAD's GUI range, 1e-5 to 1e8 mm, absolute (`DlgSettingsImportExport.ui:46-56`; its Python path has no limits), with the mesh's size bounded by a triangle count in the tools' limits table rather than by the tolerance (decided: 1e-5 to 1e8 mm, and ten million triangles, above).
- **Meshed single-threaded, on a copy** (as the display mesh is). FreeCAD meshes in parallel (`TopoShape.cpp:3664`, since `7822daf1f2`, 2020). Not verified: whether OCCT's parallel meshing gives the same triangles.
- **Faces joined by topology, not by distance** (above).
- **At most ten million triangles** (above, under Open). FreeCAD exports a mesh of any size.
- **A face OCCT meshed coarser for its own tolerance is meshed again, unraised, and refused where its nodes differ** (rule 7). FreeCAD exports it as raised, without a word.
- **`INTERNAL` and `EXTERNAL` faces are left out, and said** (decided, m4.8-export-fable). They bound no change of material: OCCT's own volume integrates only FORWARD and REVERSED faces (`BRepGProp.cxx:264-270`), as `BRepCheck_Shell` checks only them, so the mesh's volume matches the fingerprint's. Including them makes non-manifold edges. FreeCAD includes them, wound as reversed (`Tools.cpp:318-320`). The worker reports them apart (`unoriented_faces`), including a shell made only of them, as OCCT's general fuse makes of a face inside a solid. The export logs them, carries them on the mesh (`left_out`), and names them in the cause of any refusal of the solid. A box whose x = 0 face is unoriented is refused as open, saying so.
- **One object per feature**, its solids in one mesh but each joined on its own, so two that touch share no vertex (FreeCAD 1.1.3 reads them back as they are, 16 points for two cubes touching along an edge). Named by the feature's `name`, with the feature's ID as `partnumber` (an attribute the core schema has, core.md:1073). FreeCAD 1.1.3 writes no name, and main uses the `Label`. Names are XML-escaped. Tab, LF and CR are written as character references, because a parser would turn them into spaces in an attribute. A name with any other control character can't be written in XML 1.0 at all, so the export is refused, naming the feature.
- **Vertices in document coordinates, with no `transform`** (it's optional, core.md:1126). FreeCAD writes each placement as the item's transform, including identity.
- **Coordinates as the shortest decimal that reads back to the same `f32`** (Rust's `{}` for `f32`). This is exact for every known reader, matches FreeCAD's `float` precision without its 6-digit rounding, and doesn't depend on the locale. `-0` is written as `0`.
- **No thumbnail, no `xml:lang`, `Application` = `Silico`** with no version.
- **The features, or every body.** FreeCAD exports what's selected, in the selection's order, and refuses an export with nothing selected (`StdCmdExport::activated`, `CommandDoc.cpp`); it writes an object selected twice twice, and returns without a word for an empty selection. Silico's request lists the features, in the file's order, or gives `null` for every body, in the timeline's order (M4.1's `export`); a list that's empty or names a feature twice is refused, naming it.
- **Every body of a model with a failed feature is refused.** A failed feature consumes nothing, so its inputs stay bodies (a cut's tool among them, written as an object of its own) and the file wouldn't be the part the document describes. FreeCAD never exports a failed boolean's tool, which it keeps hidden, unselected; SolidWorks warns before it saves a part with rebuild errors as a mesh. Silico refuses, giving the kernel fault that stopped a feature, if one did, once, then each failure as `check` gives it (a fault alone is the whole refusal); a feature listed is exported as it built.

## Same as FreeCAD

- A 3MF of `type="model"` in millimetres, at `3D/3dmodel.model`, with one build item per object.
- Default tolerance 0.1 mm, absolute (Silico meshes at it less the rounding margin, rule 7). Angular deflection `min(0.1, 5·tol + 0.005)`. The points and facets equal FreeCAD's own export's (kernel export.rs), but where the margin meshes finer: a 1000 mm disc at 0.01 mm takes 3,992 facets to FreeCAD's 3,972.
- A reversed face's triangles wound the other way.
- Triangles that weld to a repeated index dropped.

## Determinism

What varies in FreeCAD's file:

- the zip times (wall clock);
- zlib's deflate output (it depends on the zlib FreeCAD is built with);
- which of two points within 1e-7 survives the tolerance merge (`std::sort` with an invalid comparator);
- maybe the parallel mesher (not verified);
- maybe the stream's locale (not verified).

What doesn't vary: the entry order, the vertex order (first occurrence, in face order), the object numbering, and the external attributes.

What Silico fixes:

- **Entries**: `[Content_Types].xml`, `_rels/.rels`, `3D/3dmodel.model`. Both readers find entries by name or by scanning (`Reader3MF.cpp:50`, PS 3mf.cpp:757-795), so the order is free, and Silico fixes it.
- **Zip headers**:
  - every DOS time and date the zip epoch (1980-01-01 00:00);
  - no extra fields;
  - flags 0, so no data descriptors and no UTF-8 flag, since the names are ASCII;
  - fixed "version made by" and external attributes;
  - no comment;
  - plain zip, with an archive over 4 GiB refused rather than written as ZIP64.
- **Compression**: none: every entry is stored, so the bytes depend on no compressor's version. A digest test pins a box's and a cylinder's archives on every platform CI checks.
- **The XML**: written by hand, with fixed attribute order, indentation and newlines.
- **The mesh**: deterministic only if OCCT's triangulation is bit-identical across platforms. Not verified here: whether the determinism sweep (`tests/determinism/`) covers tessellation.

## FreeCAD tests

FreeCAD has no test of `Writer3MF`'s output, of `MeshEvalSolid` on a mesh that fails, or of `defaultAngularDeflection`. These are the tests it has:

| Test | What it asserts | Port |
|---|---|---|
| `tests/src/Mod/Part/App/BRepMesh.cpp:133` `testNoDomains`, `:144` `testEmptyDomains` | No faces, or two faces with no triangles, give no points and no triangles | Analytic: the weld of no faces is empty. An export whose faces are all unmeshed is refused, naming them. |
| `BRepMesh.cpp:155` `testConnectedDomains` | Two squares sharing an edge, 4 nodes each: 6 points and 4 triangles | Analytic: the same two faces welded at their shared edge give 6 vertices and 4 triangles, with the shared edge used twice in opposite directions |
| `BRepMesh.cpp:166` `testUnconnectedDomains` | Shared nodes 1e-10 apart are merged: 6 points, 4 triangles | Ported for the topological weld: nodes identified by the edge are welded even when their coordinates differ by 1e-10. Two nodes 1e-10 apart that share no edge are **not** merged (FreeCAD would merge them). |
| `tests/src/Mod/Mesh/App/Importer.cpp:20` `Test3MF` | A box read from 3MF: 8 points, 18 edges, 12 triangles | Analytic: a 10 × 20 × 30 box's 3MF has 8 vertices, 18 edges and 12 triangles (V − E + F = 2), and its signed volume is 6000 exactly. The test's file has a second object too (`ids.size() == 2`: 652 points, 1950 edges, 1300 triangles); Silico's two-object case is a box beside a cylinder, each object's counts checked on its own |
| `tests/src/Mod/Mesh/App/Exporter.cpp:78` `TestSingleMesh`, `:102` `TestMultipleMeshes`, `:127` `TestMeshesInPart` | A 10 mm cube (centered) placed at (10, 5, 2) exports with bounding box [5, 15] × [0, 10] × [−3, 7], alone, beside another, and inside a placed `App::Part` | Analytic, with Silico's translate: a box moved by (10, 5, 2) has those bounds in the 3MF exactly. The `App::Part` case waits for placements. |
| `src/Mod/Mesh/App/MeshTestsApp.py:96-197` `MeshSplitTestCases` | After splitting a box's facets, `isSolid()`, no non-manifolds, no corrupted facets | Analytic, for the checks: a 12-triangle box passes. Remove one triangle and it's open (naming that face); duplicate one and it's non-manifold; flip one and it's inconsistently oriented; flip all and it faces inward. FreeCAD tests only the passing case. |
| `MeshTestsApp.py:55` `testCorruptedFacet` | A facet with a repeated point is removed by `fixIndices` (6 → 5) | Analytic: a triangle that welds to a repeated index is dropped, and the mesh still closes |
| `MeshTestsApp.py:439` `testSelfIntersection` | Self-intersection is detected | Not ported: the specification only asks producers not to make self-intersections (core.md:587). A valid B-rep's mesh within the tolerance doesn't intersect itself except where faces come closer than the tolerance. |
| `tests/src/Mod/MeshPart/App/MeshPart.cpp:34`, `:80` | SMESH's Mefisto and StdMeshers node counts | Not ported: not the standard mesher |

Silico's own tests, from independent sources:

- **A cylinder's volume** (r = 5, h = 10, tolerance 0.1, angle 0.1):
  - Every vertex lies on the surface, so the mesh is inscribed and V < πr²h = 785.398.
  - Read n, the number of segments on a cap, from the mesh. Assert n ≥ ⌈2π/0.1⌉ = 63 and V = (n/2)·r²·h·sin(2π/n) to 1e-6 relative: V is computed from the coordinates as written, in `f32`, about 6e-8 relative each. That gives V ≥ 784.097 at n = 63.
  - The deflection alone would allow n = 16 (V ≥ 765.367), which shows the angle is ported.
  - Looser, and true of any mesh whose sagitta is within t: V ≥ π(r − t)²h = 754.296.
- **A sphere** welds closed. Its pole triangles are dropped, and no vertex is repeated.
- **Refusals**:
  - a box minus a cylinder touching it along an edge (edges with four triangles), refused naming the faces;
  - a feature name with a control character, refused;
  - a valid chamfered box with an unmeshable sliver face, refused naming the sliver.
- **Bytes**: one export, identical across runs and platforms (golden bytes). The archive lists exactly the three entries, each with the zip epoch as its time.
- **Oracle (FreeCAD 1.1.3)**, for each oracle case Silico exports:
  - `m = Mesh.Mesh(path)`;
  - `m.isSolid()`, `not m.hasNonManifolds()`, and `m.countNonUniformOrientedFacets() == 0` (`src/Mod/Mesh/App/Mesh.pyi:226`, `:231`, `:267`);
  - the vertex and triangle counts equal Silico's;
  - the volume computed in Python doubles from `m.Topology` equals Silico's mesh volume to 1e-12. This avoids `m.Volume`, which is a `float` sum and an absolute value (`MeshKernel.cpp:1207-1231`). The sign must be positive.
  - Also: FreeCAD's own `Mesh.export` of the same part at 0.1 gives the same counts. This shows whether the f64 tolerance or the serial meshing differs from FreeCAD.
  - Note that `Mesh.Mesh` merges every object of a 3MF into one mesh (`Core/MeshIO.cpp:911-931`), so multi-object cases compare totals.

## Not verified

- whether OCCT 7.9.3's parallel meshing gives the same triangles as serial;
- whether two faces that share an edge always give its nodes bitwise-equal coordinates (Silico's weld doesn't rely on it);
- whether a FreeCAD GUI session exports its display mesh;
- the locale FreeCAD's stream formats numbers in;
- what zipios++ issue 43 is about (data descriptors are the likely cause);
- whether PrusaSlicer's repair of open edges is in its reader or its model code;
- whether the determinism sweep covers tessellation.

The claims about FreeCAD's several-solid and empty-shape output come from reading the code, not from running it.

## For M4.8's design

What M4.8's row in the M4 plan (#50) should say, given this study:

- **The oracle's volume is the mesh's.** "FreeCAD reads every exported 3MF back as a solid with the volume Silico measured" has to compare with the mesh's own volume, not the B-rep's: at 0.1 mm a cylinder's mesh is 0.17% under it. FreeCAD's mesh `Volume` is a 32-bit sum that ignores the sign, so the case doesn't use it: it compares the volume computed in doubles from `m.Topology`, as *FreeCAD tests* says.
- **The tolerance names the angle too.** "Within the export tolerance (FreeCAD's 0.1 mm by default)" leaves out FreeCAD's angular deflection, `min(0.1, 5·tol + 0.005)` rad (`487215d2f5`), which sets the mesh below r ≈ 80 mm. The row should say whether Silico takes both, as FreeCAD does (recommended, so the oracle's meshes are comparable), or the length alone.
- **Object names are main's, not 1.1.3's.** "Each object named by its feature" follows main (`49c80ebe5c`). 1.1.3 writes no names and its reader ignores them, so no oracle case can check a name: the name is tested by reading the archive's XML.
- **Open meshes are refused, on purpose.** FreeCAD's fix for #10075 (`7bbc93e218`) writes every mesh as `model`, open or not. Silico's refusal is the opposite, and the row should call it a deliberate departure.
- **A valid solid with a face OCCT can't mesh can't be exported.** Every face must be meshed (rule 1 above), so such a solid is refused, naming the face, until OCCT meshes it. The row's refusal case covers it; the docs should say there's no workaround.
- **The tolerance's limits are the export's own.** The display mesh's floor (1e-4 × the diagonal) would refuse the 0.1 mm default on any part over 1000 mm across. Recommended: 1e-5 to 1e8 mm absolute, as FreeCAD's GUI takes, with the mesh's size bounded by a triangle count in the tools' limits table (amendment A1).
- **The f32 rounding counts against the tolerance.** Rounding to `f32` moves a vertex by up to √3·max|coordinate|·2⁻²⁴, about 5e-3 mm at 5·10⁴ mm. The row's "within the export tolerance" should include it, so a part far from the origin is refused rather than written outside it.
- **"Tessellated at the export tolerance", not "within" it.** The tolerance is the mesher's deflection. What's checked exactly is in rule 7; a sphere's interior misses by up to 2.1 times, as FreeCAD's does. No message, doc, or the row should claim a face's interior is within the tolerance.

## Notes

- STEP export (decision 6) has its own note when it comes.
