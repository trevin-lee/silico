# FreeCAD's side of the oracle (silico-oracle's freecad.rs runs it with freecadcmd): builds
# each case in the JSON file SILICO_ORACLE_CASES names, from the one SILICO_ORACLE_FIRST
# numbers (from 0), with the calls Silico's port notes name, measures it, and prints its
# outcome as a line, "SILICO_ORACLE_CASE <json>", as soon as it has it, so a crash on one
# case loses only that one. "SILICO_ORACLE_READY" comes first, once the cases are read.
import json
import os

import FreeCAD
import Part

V = FreeCAD.Vector

# Silico's primitive, FreeCAD's feature, and each value's property.
PRIMITIVES = {
    "box": ("Box", {"dx": "Length", "dy": "Width", "dz": "Height"}),
    "cylinder": ("Cylinder", {"radius": "Radius", "height": "Height"}),
    "sphere": ("Sphere", {"radius": "Radius"}),
    "cone": ("Cone", {"radius1": "Radius1", "radius2": "Radius2", "height": "Height"}),
    "torus": ("Torus", {"major_radius": "Radius1", "minor_radius": "Radius2"}),
}

CHAMFER_TYPES = {
    "equal": "Equal distance",
    "two_distances": "Two distances",
    "distance_angle": "Distance and Angle",
}


def dot(a, b):
    return sum(x * y for x, y in zip(a, b))


def unit(v):
    # Scaled first, so a direction far longer than a millimetre doesn't overflow.
    largest = max(abs(c) for c in v)
    v = [c / largest for c in v]
    n = dot(v, v) ** 0.5
    return [c / n for c in v]


def edges(s, numbers):
    # Numbered from 1, as Silico numbers them; 0 or below is no edge (Python would take it
    # from the end).
    for i in numbers:
        if i < 1:
            raise IndexError("there's no Edge%d" % i)
    return [s.Edges[i - 1] for i in numbers]


def newell(points):
    """The polygon's unit normal, by the right-hand rule of its corners' order."""
    n = [0.0, 0.0, 0.0]
    for i, p in enumerate(points):
        q = points[(i + 1) % len(points)]
        n[0] += (p[1] - q[1]) * (p[2] + q[2])
        n[1] += (p[2] - q[2]) * (p[0] + q[0])
        n[2] += (p[0] - q[0]) * (p[1] + q[1])
    return unit(n)


def face(points):
    return Part.Face(Part.makePolygon([V(*p) for p in points], True))


def build(doc, spec):
    (kind, a), = spec.items()
    if kind in PRIMITIVES:
        feature, properties = PRIMITIVES[kind]
        o = doc.addObject("Part::" + feature, feature)
        for value, prop in properties.items():
            setattr(o, prop, a[value])
        doc.recompute()
        if not o.isValid():
            raise RuntimeError(o.getStatusString())
        return o.Shape.copy()
    if kind == "translate":
        s = build(doc, a["shape"]).copy()
        s.translate(V(*a["by"]))
        return s
    if kind in ("fuse", "cut", "common"):
        base = build(doc, a["base"])
        tool = build(doc, a["tool"])
        return getattr(base, kind)(tool)
    if kind == "fillet":
        s = build(doc, a["shape"])
        return s.makeFillet(a["radius"], edges(s, a["edges"]))
    if kind == "chamfer":
        holder = doc.addObject("Part::Feature", "Shape")
        holder.Shape = build(doc, a["shape"])
        body = doc.addObject("PartDesign::Body", "Body")
        base = body.newObject("PartDesign::FeatureBase", "Base")
        base.BaseFeature = holder
        ch = body.newObject("PartDesign::Chamfer", "Chamfer")
        # Not refined: its default comes from the user's preferences (RefineModel), and
        # Silico's results are FreeCAD's unrefined ones.
        ch.Refine = False
        edges(holder.Shape, a["edges"])  # refuses an edge that isn't there
        ch.Base = (base, ["Edge%d" % i for i in a["edges"]])
        (size_type, size), = a["size"].items()
        ch.ChamferType = CHAMFER_TYPES[size_type]
        ch.Size = size["distance"]
        if "other" in size:
            ch.Size2 = size["other"]
        if "angle" in size:
            ch.Angle = size["angle"]
        ch.FlipDirection = a.get("flip", False)
        doc.recompute()
        if ch.Shape.isNull() or not ch.isValid():
            raise RuntimeError(ch.getStatusString())
        # PartDesign's chamfer drops a placement held in its base's shape (it resets the
        # shape's transform; fixed after FreeCAD 1.1.3, in 123a1c80e1): put it back.
        r = ch.Shape.copy()
        r.Placement = holder.Placement.multiply(r.Placement)
        return r
    if kind == "extrude":
        points = a["profile"]
        normal = newell(points)
        direction = a.get("direction")
        if direction is None:
            vector = [c * a["length"] for c in normal]
        else:
            d = unit(direction)
            vector = [c * a["length"] / abs(dot(d, normal)) for c in d]
        return face(points).extrude(V(*vector))
    if kind == "brep":
        s = Part.Shape()
        s.read(a["path"])
        return s
    if kind == "revolve":
        return face(a["profile"]).revolve(V(*a["origin"]), V(*a["direction"]), a["angle"])
    raise ValueError("unknown operation " + kind)


def measure(s):
    # An empty result (a common of shapes that don't overlap) is refused by Silico, which
    # FreeCAD returns as a shape with nothing in it: the same outcome, a failure.
    if s.isNull() or not s.Faces:
        raise RuntimeError("the result is empty")
    b = s.optimalBoundingBox(False, False)
    bounds = [[b.XMin, b.YMin, b.ZMin], [b.XMax, b.YMax, b.ZMax]] if b.isValid() else None
    # The center of the whole shape's volume, as Silico measures it: a real part can carry
    # shells and faces beside its solids, which count in its volume too.
    center = None
    if s.Solids and s.Volume != 0:
        center = list(s.CenterOfGravity)
    return {
        "volume": s.Volume,
        "area": s.Area,
        "bounds": bounds,
        "center": center,
        "faces": len(s.Faces),
        "edges": len(s.Edges),
        "vertices": len(s.Vertexes),
        "valid": s.isValid(),
    }


with open(os.environ["SILICO_ORACLE_CASES"]) as f:
    cases = json.load(f)[int(os.environ["SILICO_ORACLE_FIRST"]):]
# Each line begins on a line of its own: FreeCAD's recompute progress ends without one.
print("\nSILICO_ORACLE_READY", flush=True)
for case in cases:
    doc = FreeCAD.newDocument()
    try:
        # Written here, so a measure that isn't finite (a shape too large to measure) is
        # the case's failure.
        outcome = json.dumps({"built": measure(build(doc, case["shape"]))}, allow_nan=False)
    except Exception as e:
        outcome = json.dumps({"failed": str(e) or type(e).__name__})
    FreeCAD.closeDocument(doc.Name)
    print("\nSILICO_ORACLE_CASE " + outcome, flush=True)
