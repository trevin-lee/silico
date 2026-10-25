# Makes offsets_and_conics.brep (README.md), and prints FreeCAD's types, areas, and lengths
# for describe's test of it. Run from this directory: freecadcmd offsets_and_conics.py
import FreeCAD, Part
from FreeCAD import Vector as V
parabola = Part.Parabola()  # focal 1, apex at the origin, in the xy plane
parabola.Focal = 1.0
e_par = parabola.toShape(-2.0, 2.0)
hyperbola = Part.Hyperbola()
hyperbola.MajorRadius = 2.0
hyperbola.MinorRadius = 1.0
e_hyp = hyperbola.toShape(-1.0, 1.0)
spline = Part.BSplineCurve()
spline.buildFromPoles([V(0, 0, 0), V(1, 2, 0), V(3, -1, 0), V(4, 1, 0)])
offset_curve = Part.OffsetCurve(spline, 0.5, V(0, 0, 1))
e_off = offset_curve.toShape()
patch = Part.BSplineSurface()
patch.buildFromPolesMultsKnots(
    [[V(0, 0, 0), V(0, 1, 0.2), V(0, 2, 0)],
     [V(1, 0, 0.3), V(1, 1, 0.6), V(1, 2, 0.3)],
     [V(2, 0, 0), V(2, 1, 0.2), V(2, 2, 0)]],
    [3, 3], [3, 3], [0, 1], [0, 1], False, False, 2, 2)
offset_surface = Part.OffsetSurface(patch, 0.25)
f_off = offset_surface.toShape()
compound = Part.makeCompound([f_off, e_par, e_hyp, e_off])
import sys
out = "offsets_and_conics.brep"
compound.exportBrep(out)
for i, f in enumerate(compound.Faces, 1):
    print("Face%d" % i, type(f.Surface).__name__, repr(f.Area))
for i, e in enumerate(compound.Edges, 1):
    print("Edge%d" % i, type(e.Curve).__name__, repr(e.Length))
