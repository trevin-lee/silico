# B-reps OCCT wrote, for `from_brep`'s tests

Binary B-reps (`BinTools::Write`, OCCT 7.9.3, the current format version), without the
checksum Silico appends; the tests append it. Silico has no operation that makes these, so
they were written by a small OCCT program during the M1.7 review.

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
