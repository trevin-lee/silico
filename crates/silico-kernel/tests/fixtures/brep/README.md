# B-reps OCCT wrote, for `from_brep`'s tests

Binary B-reps (`BinTools::Write`, OCCT 7.9.3, the current format version), without the
checksum Silico appends; the tests append it. Silico has no operation that makes these, so
they were written during the M1.7 review by a small OCCT program, some from text B-reps made
in the FreeCAD oracle.

- `meshonly.brep`: OCCT's text form of a face with no surface and no edges, only a
  triangulation (a 10 × 20 rectangle in two triangles), as OCCT's STEP and glTF readers make
  for tessellated faces; `from_brep` refuses it, as OCCT's booleans segfault on it.
  `meshonly.bin` is it read with `BRepTools::Read` and written with `BinTools::Write`.
- `powered_far.bin`: a 10 × 20 × 30 box placed by one location link, a move of (1e300, 0, 0)
  raised to the power 1e9: each datum is finite, the placement isn't.
- `powered_scale.bin`: the same box placed by a scale of 1 + 5e-15 (within OCCT's
  `ScalePrec`) raised to the power 1e9: a scale of about 1.000005.

The two powered B-reps came from:

```cpp
gp_Trsf t;
t.SetTranslation(gp_Vec(1e300, 0, 0));            // powered_far
// t.SetScale(gp_Pnt(0, 0, 0), 1.0 + 5e-15);      // powered_scale
TopLoc_Location powered = TopLoc_Location(t).Powered(1000000000);
TopoDS_Shape box = BRepPrimAPI_MakeBox(10, 20, 30).Shape().Located(powered);
std::ofstream out(path, std::ios::binary);
BinTools::Write(box, out, false, false, BinTools_FormatVersion_CURRENT);
```

- `box_external_face1.bin`, `box_internal_face1.bin`: a 10 × 20 × 30 box whose face at
  x = 0 is EXTERNAL, or INTERNAL, in its shell (OCCT's STEP reader never makes either; OCCT's
  own operations do). Made in the FreeCAD oracle (1.1.3, OCCT 7.9.3): the box's faces copied,
  the first's `Orientation` set to `"External"`, a solid made of them, written as a text
  B-rep (`exportBrep`). FreeCAD's `Part.Solid` drops an INTERNAL face, so the INTERNAL one is
  that text with the face's reference `e11` changed to `i11`. Each text B-rep was then read
  with `BRepTools::Read` and written with `BinTools::Write`, as `meshonly.bin` was (FreeCAD's
  own `exportBinary` writes a form `from_brep` doesn't read). The two differ in one byte, the
  face's orientation.
- `nist_ctc_01_asme1_ap242-e1.brep`: NIST's CTC 01 test part (the MBE PMI validation and
  conformance test models, AP242 edition, which "can be used without any restrictions"),
  read from its STEP file by FreeCAD 1.1.3 and written with `exportBrep`, as the corpus
  keeps it (`tests/corpus/run.sh`). At a 0.1 mm export tolerance OCCT meshes two of its
  cones with coincident nodes inside the face (tests/adversarial_export.rs).

- `mirrored_box.brep`: a text B-rep (not binary: read with `from_text_brep`) of a 10 × 20 × 30
  box mirrored in the plane x = 0 by FreeCAD 1.1.3 (`Part.makeBox(10, 20, 30).mirror(V(0, 0, 0),
  V(1, 0, 0)).exportBrep(...)`): the mirror is in its geometry, not a location, so each plane's
  axes are left-handed (its normal, Du × Dv, opposite to its axis). For `describe`'s tests.

- `curves_and_surfaces.brep`: a text B-rep (read with `from_text_brep`) of a compound FreeCAD
  1.1.3 made of what no primitive is: Bézier edges of two and three poles, B-spline edges of
  degree 1 with two poles and of degree 3 with five, a ruled surface between two cubic
  B-splines, a Bézier surface, the five-pole B-spline extruded 2 along +z, and the three-pole
  Bézier turned 90° about +x (`Part.BezierCurve`, `Part.BSplineCurve.buildFromPoles`,
  `Part.makeRuledSurface`, `Part.BezierSurface`, `extrude`, `revolve`, then `exportBrep`).
  FreeCAD's types, degrees, pole counts, and lengths for its faces and edges, in order, are
  in `describe`'s test of it.

- `offsets_and_conics.brep`: a text B-rep (read with `from_text_brep`) of a compound FreeCAD
  1.1.3 made of what no Silico operation makes yet: an offset surface (a biquadratic
  B-spline patch offset 0.25, `Part.OffsetSurface`), a parabola of focal length 1 from −2 to
  2, a hyperbola of radii 2 and 1 from −1 to 1, and a cubic B-spline offset 0.5 in its plane
  (`Part.OffsetCurve`), each made a shape and compounded (`exportBrep`). FreeCAD's types and
  lengths are in `describe`'s test of it. `offsets_and_conics.py` makes it
  (`freecadcmd offsets_and_conics.py`, from this directory).

- `extruded_bspline.brep`: a text B-rep (read with `from_text_brep`) of the five-pole cubic
  B-spline of `curves_and_surfaces.brep` (its Edge4: poles (0, 4), (1, 5), (2, 4), (3, 5),
  (4, 4) at z = 0), closed by its chord, made a face, and extruded 2 along +z: a valid solid
  whose volume OCCT integrates wrongly (docs/ports/measure.md, Known wrong results). Written
  by OCCT 7.9.3 (`BRepTools::Write`) from:

  ```cpp
  TopTools_IndexedMapOfShape edges;
  TopExp::MapShapes(curves_and_surfaces, TopAbs_EDGE, edges);
  TopoDS_Edge edge4 = TopoDS::Edge(edges(4));
  double first, last;
  Handle(Geom_Curve) c = BRep_Tool::Curve(edge4, first, last);
  gp_Pnt a = c->Value(first), b = c->Value(last);
  TopoDS_Wire w = BRepBuilderAPI_MakeWire(BRepBuilderAPI_MakeEdge(c, first, last),
                                          BRepBuilderAPI_MakeEdge(b, a));
  TopoDS_Shape prism =
      BRepPrimAPI_MakePrism(BRepBuilderAPI_MakeFace(w, true), gp_Vec(0, 0, 2)).Shape();
  ```
