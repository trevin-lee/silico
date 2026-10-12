# Tessellation for display

The mesh a shape is drawn with: triangles per face, a normal per vertex for smooth shading, and a polyline per edge. Built in M0 as `silico_kernel::tessellate` (`Op::Tessellate`, `silico_protocol::Mesh`); M1.7 adds the normals and each triangle's face, and gives `silico-view` a library half. The app's viewport (M4) draws this mesh.

## What FreeCAD does

`ViewProviderPartExt::updateVisual` (`src/Mod/Part/Gui/ViewProviderExt.cpp`) meshes the shape with `BRepMesh_IncrementalMesh` and walks its faces in `TopExp::MapShapes` order. For each face:

- **Nodes:** the face's own triangulation nodes, moved by the face's location. Nodes aren't shared between faces, so each face shades on its own, and an edge between faces stays sharp.
- **Triangles:** the face's triangles, two corners swapped when the face isn't `TopAbs_FORWARD`, so every triangle winds counterclockwise seen from outside the solid.
- **Normals:** with the preference `NormalsFromUVNodes` on (the default), `Part::Tools::getPointNormals(face, triangulation, normals)` (`src/Mod/Part/App/Tools.cpp`). That's OCCT's `BRepLib_ToolTriangulatedShape::ComputeNormals` written out: the normal of the face's surface at each node's (u, v), from `GeomLib::NormEstim`; where that fails (status above 1: a point where the surface's derivatives vanish and the second derivatives don't settle it), the average of the unit normals of the triangles around the node, or +Z if they cancel. Then reversed if the face is `TopAbs_REVERSED`, and turned by the face's location. With the preference off, each node gets the sum of its triangles' unnormalized normals (an area-weighted average, normalized for shading), not turned by the face's location.
- **Parts:** how many triangles each face has (`SoBrepFaceSet::partIndex`), which is how a picked triangle is mapped back to its face.

Edges are each edge's polygon on the triangulation, in `TopExp::MapShapes` order.

## Guards, from FreeCAD's history

| FreeCAD commit | Guard | In Silico |
|---|---|---|
| `d5ee7a6efe` (2015) | Vertex normals from the surface at each node's (u, v), not averaged from triangles: averaging makes a curved face look faceted, and blends a face's shading with its neighbors' where nodes are shared. | Normals from the surface (`ComputeNormals`), per face. |
| `a701c71ed8` (2018, issue 3180, "Wrong normal of part object") | A preference to fall back to flat triangle normals, for faces whose surface normals came out wrong. | Not ported: no preference. The surface normal is what the face is; a wrong one is a wrong surface, which validity checks report. The flat fallback where the surface can't give a normal is kept (inside `ComputeNormals`). |
| `03be15cc6b` (2021) | With OCCT 7.6, `Poly_Triangulation::AddNormals()` before `SetNormal`, or the normals are written nowhere. | Inside `ComputeNormals`, which OCCT 7.6+ provides. |

## Where Silico differs on purpose

- **Normals are 32-bit.** OCCT stores a triangulation's normals as `float`, so the mesh carries them as `[f32; 3]` and doesn't claim a precision they don't have. Positions stay `f64`.
- **No second mesh for display.** The edge polylines are sampled from each edge's curve (`GCPnts_TangentialDeflection`) rather than taken from the triangulation, as built in M0; they number edges as `Op::Fillet` does.
- **The face of each triangle is derived** from the per-face counts (`Mesh::triangle_faces`), not sent: the counts already say it.
- **Single-threaded meshing of a copy,** as built in M0, so the same shape always gives the same mesh and the shape isn't changed.
- **A B-rep placed by a location that mirrors or scales is refused** when it's read (`from_brep`), by OCCT's test for one (`TopoDS_Shape::validateTransformation`) applied to each link of every location in the shape's tree, raised to its power (stricter than OCCT, which tests the composed transform, and only when asked; the geometry's own locations, such as a face's surface location, are checked the same way since M1.9). OCCT sets and reads such a location without complaint, but its operations then disagree about which side is solid: a mirrored box measures volume -6000, a big box less it is the box, and its mesh drew it as solid. FreeCAD's reader accepts one; its document object turns a mirror at the top into a 180° rotation, and its raw tessellation winds the triangles inward. When Silico places shapes by a transform, FreeCAD's guard is the one to port: `TopoShape::makeTransform` rebuilds the geometry instead of setting a location when the transform mirrors or scales.
- **A B-rep whose locations aren't finite, or with a face that has no surface, is refused** too, in the same walk. OCCT reads both, then reports NaN measures for the first. For the second, its validity check segfaults on a face with edges (`BRepCheck_Edge::InContext` uses the missing surface), and its booleans segfault on a face of only a triangulation (what OCCT's STEP and glTF readers make of a tessellated face), which OCCT otherwise measures and meshes; FreeCAD reads both and crashes the same ways (its fuse, cut, common, extrude, and revolve of such a face). Refused until Silico works on meshes. When mesh import comes, FreeCAD's guard is the one to port: its glTF reader rebuilds every face of only a triangulation as planar faces, sewn, with splitters removed (`ReaderGltf::fixShape`, `37e477d3e7`, fixing issue 8666, and its crash fix `1683f271c6`). Its STEP import has no such fix, though OCCT reads tessellated STEP faces this way by default (`read.step.tessellated`), so a boolean on one crashes FreeCAD.
- **The geometry's own numbers are checked when read** (M1.9): a value that isn't finite, or a location on the geometry that mirrors or scales, is refused (`from_brep`).
- **INTERNAL and EXTERNAL faces:** Silico winds a face's triangles the other way only when the face is `REVERSED`, as it reverses its normals, so triangles and normals agree (as OCCT's own display, `StdPrs_ShadedShape`, does). FreeCAD swaps corners for every face that isn't `FORWARD` but reverses normals only for `REVERSED`, so on such a face its triangles and normals disagree. Pinned by `internal_and_external_faces_mesh_with_triangles_and_normals_agreeing`, on boxes made in the FreeCAD oracle.
- **A face OCCT's mesher leaves unmeshed gets no triangles, on a valid shape:** a sliver too narrow for OCCT's tolerance (a box chamfered nearly through has several, a few 1e-7 mm²) can't be meshed at any deflection. FreeCAD's data path gives such a face an empty domain (`TopoShape::getDomains`), as Silico gives it 0 triangles. Silico refused the whole mesh until the M1 review, calling valid geometry broken. On a shape that isn't valid an unmeshed face is refused, naming it (`Face4 couldn't be meshed`): there it means something is broken (a face whose edges cross). The bridge reports the unmeshed faces; Rust decides. FreeCAD's display goes further, meshing the face's untrimmed surface over its parameter range instead (`Part::Tools::triangulationOfFace`, from `9e8516002b`, for infinite datum faces); not ported, since it draws surface the face doesn't have.
- **`Mesh::validate` refuses a normal that isn't a unit vector** (to 1e-6), as well as a missing or non-finite one, so a worker can't hand the viewport a normal that shades as NaN.

## What OCCT gets wrong, accepted as FreeCAD accepts it

Found by `/break-it`; FreeCAD's display runs the same code, so it shows the same normals. The ball, the hollow, and the needle are pinned by tests in `crates/silico-kernel/tests/adversarial_mesh.rs`, so a change in OCCT shows, and kept for reporting to OCCT.

- **A small ball's north pole** (radius under about 0.0316 mm) has its normal pointing straight in, (0, 0, -1); a hollow of that size has (0, 0, 1) at its north pole, pointing into the material. `GeomLib::NormEstim` takes a pole's normal from second derivatives, with either sign, and orients it by the surface's normal a step (1e-5 in v) away, but skips that when the step's normal, r² · 1e-5 long, is under its absolute 1e-8.
- **A needle's apex** (a cone 1000 times as tall as its base radius, or more slender) gets one ruling's normal, reversed at a top apex, instead of the average of its triangles that a stouter cone's apex gets: the same absolute 1e-8, against |dP/du| ~ 1e-5 r/h.
- **A horn torus's center** (minor radius equal to the major), where the surface pinches to a point, has no normal; what OCCT gives there is left as it is, and isn't pinned.

A triangle with all three corners at one point (at a pole, a degenerate edge that's a single point) is kept as OCCT gives it: it has no area, so it can't be seen or picked.

## Same as FreeCAD

- Faces in `TopExp::MapShapes` order, each with its own nodes.
- Triangles of a reversed face wound the other way, and its normals reversed, so both face out of the solid (for `REVERSED` faces; see above for INTERNAL and EXTERNAL).
- Normals from the surface at each node, with OCCT's flat fallback.

## FreeCAD tests

FreeCAD has no test of display normals or face parts. `tests/src/Mod/Part/App/BRepMesh.cpp` tests merging face meshes into one welded mesh for export (`Part::BRepMesh::getFacesFromDomains`), which belongs with STL export, not here. Silico's tests take expected values from the surfaces themselves:

- `box_mesh`: 12 triangles, every face index from 0 to 5 present, every vertex inside the box, and every normal the face's axis direction.
- `closed_solid_mesh_watertight`: for a box, a cylinder, and the box minus the cylinder, with vertices joined where their coordinates are equal, every mesh edge is shared by exactly two triangles.
- `cylinder_deflection_respected`: every vertex of the side lies on the r = 5 surface, the largest sagitta between neighbouring vertices is at most the deflection asked for, and every normal of the side is radial.
- Normals of a sphere point from its center to the vertex; on the hole of the box minus the cylinder (a reversed face) they point toward the hole's axis; every triangle's winding agrees with its vertices' normals.
