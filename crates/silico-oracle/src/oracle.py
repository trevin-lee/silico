# FreeCAD's side of the oracle (silico-oracle's freecad.rs runs it with freecadcmd): builds
# each case in the JSON file SILICO_ORACLE_CASES names, from the one SILICO_ORACLE_FIRST
# numbers (from 0), with the calls Silico's port notes name, measures it, and prints its
# outcome as a line, "SILICO_ORACLE_CASE <json>", as soon as it has it, so a crash on one
# case loses only that one. "SILICO_ORACLE_READY" comes first, once the cases are read. A
# measure case (silico-oracle's measures.rs) builds the bodies a pair of elements is in and
# measures the pair instead, its outcome "measured".
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


def edge_numbers(s, edges):
    """Each of `edges` as its number in `s`, from 1: a number as it is (Silico's numbering,
    which a case built the same way on both sides shares), or an edge as Silico describes
    it, found by its geometry (`element`), for a measure case's fillet or chamfer."""
    return [int(element(s, e)[len("Edge"):]) if isinstance(e, dict) else e for e in edges]


def edges(s, numbers):
    # Numbered from 1, as Silico numbers them; 0 or below is no edge (Python would take it
    # from the end).
    numbers = edge_numbers(s, numbers)
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
    if kind == "placed":
        return MAKE[a["shape"]](V(*a["position"]), a["values"])
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
        numbers = edge_numbers(holder.Shape, a["edges"])
        edges(holder.Shape, numbers)  # refuses an edge that isn't there
        ch.Base = (base, ["Edge%d" % i for i in numbers])
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
        # PartDesign's base feature drops a placement held in its shape (recompute syncs the
        # shape to the feature's own identity Placement, PartFeature.cpp:1548): put it back.
        # FreeCAD's 123a1c80e1 restores it only for a base made through Body.BaseFeature, not
        # this way, so this stays after an upgrade.
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
    if kind == "part_design":
        return part_design(doc, a["features"])
    raise ValueError("unknown operation " + kind)


# Silico's primitives as Part makes them at a position: the shape's geometry is placed, with
# no placement of its own for PartDesign's base feature to drop.
MAKE = {
    "box": lambda p, v: Part.makeBox(v[0], v[1], v[2], p),
    "cylinder": lambda p, v: Part.makeCylinder(v[0], v[1], p),
    "sphere": lambda p, v: Part.makeSphere(v[0], p),
    "cone": lambda p, v: Part.makeCone(v[0], v[1], v[2], p),
    "torus": lambda p, v: Part.makeTorus(v[0], v[1], p),
}


def recomputed(doc, o):
    doc.recompute()
    if not o.isValid():
        raise RuntimeError("%s: %s" % (o.Label, o.getStatusString()))
    return o


def sketch_axis(sketch, curves, line):
    """The sub-element a Revolution takes for a line of its sketch: a construction line's
    AxisN (N counting the sketch's construction line segments, from 0), any other line's
    edge of the sketch's shape, found by its ends."""
    index, g = curves[line]
    if g["construction"]:
        n = sum(1 for i, geo in enumerate(sketch.Geometry)
                if i < index and sketch.getConstruction(i) and isinstance(geo, Part.LineSegment))
        return "Axis%d" % n
    local = sketch.Placement.inverse()
    want = sorted([tuple(g["start"]), tuple(g["end"])])
    for i, e in enumerate(sketch.Shape.Edges):
        ends = sorted((local.multVec(v.Point).x, local.multVec(v.Point).y) for v in e.Vertexes)
        if len(ends) == 2 and all(abs(a - b) < 1e-9 for p, q in zip(ends, want) for a, b in zip(p, q)):
            return "Edge%d" % (i + 1)
    raise RuntimeError("line %s isn't an edge of its sketch" % line)


def part_design(doc, features):
    """A sketch-feature case (silico-oracle's features.rs): its features in one PartDesign
    body, sketches attached to the body's origin planes with their curves where the
    document stores them, and FreeCAD's Pad, Pocket, Revolution, and Groove, each
    unrefined. The last feature's shape."""
    body = doc.addObject("PartDesign::Body", "Body")
    planes = {o.Role: o for o in body.Origin.OriginFeatures}
    sketches = {}
    last = None
    for f in features:
        kind = f["kind"]
        if kind == "primitive":
            if last is not None:
                raise ValueError("a primitive is only a body's first feature")
            holder = doc.addObject("Part::Feature", "Shape")
            holder.Shape = MAKE[f["shape"]](V(*f["position"]), f["values"])
            base = body.newObject("PartDesign::FeatureBase", "Base")
            base.BaseFeature = holder
            last = recomputed(doc, base)
        elif kind == "sketch":
            sk = body.newObject("Sketcher::SketchObject", "Sketch")
            sk.AttachmentSupport = (planes[f["plane"]], [""])
            sk.MapMode = "FlatFace"
            curves = {}
            for g in f["geometry"]:
                c = g["curve"]
                if c == "line":
                    geo = Part.LineSegment(V(g["start"][0], g["start"][1], 0), V(g["end"][0], g["end"][1], 0))
                else:
                    circle = Part.Circle(V(g["center"][0], g["center"][1], 0), V(0, 0, 1), g["radius"])
                    geo = circle if c == "circle" else Part.ArcOfCircle(circle, g["from"], g["to"])
                curves[g["id"]] = (sk.addGeometry(geo, g["construction"]), g)
            sketches[f["id"]] = (recomputed(doc, sk), curves)
        elif kind in ("pad", "pocket"):
            o = body.newObject("PartDesign::Pad" if kind == "pad" else "PartDesign::Pocket", kind)
            o.Profile = sketches[f["sketch"]][0]
            # Not refined (design S8): its default comes from the user's preferences.
            o.Refine = False
            o.SideType = f["side_type"]
            o.Type = f["method"]
            if f["side_type"] == "Two sides":
                o.Type2 = "Length"
            o.Length = f["length"]
            o.Length2 = f["length2"]
            o.Reversed = f["reversed"]
            last = recomputed(doc, o)
        elif kind in ("revolution", "groove"):
            o = body.newObject("PartDesign::Revolution" if kind == "revolution" else "PartDesign::Groove", kind)
            sk, curves = sketches[f["sketch"]]
            o.Profile = sk
            (where, axis), = f["axis"].items()
            o.ReferenceAxis = (sk, [axis if where == "sketch" else sketch_axis(sk, curves, axis)])
            o.Refine = False
            o.Type = "Angle"
            o.Angle = f["angle"]
            o.Midplane = f["midplane"]
            o.Reversed = f["reversed"]
            last = recomputed(doc, o)
        else:
            raise ValueError("unknown feature " + kind)
    if last is None or body.Tip != last:
        raise RuntimeError("the body's tip isn't the case's feature")
    return last.Shape.copy()


# Silico's kinds of curve and surface (silico-oracle's measures.rs), as FreeCAD's types.
CURVES = {"line": "Part::GeomLine", "circle": "Part::GeomCircle", "ellipse": "Part::GeomEllipse"}
SURFACES = {
    "plane": "Part::GeomPlane",
    "cylinder": "Part::GeomCylinder",
    "cone": "Part::GeomCone",
    "sphere": "Part::GeomSphere",
    "torus": "Part::GeomToroid",
}

# How near FreeCAD's element must be to Silico's description of it, in mm (relative to the
# larger of 1 and the size measured): both are the same OCCT's geometry of the same solid.
NEAR = 1e-6


def near(a, b, size=1.0):
    return abs(a - b) <= NEAR * max(1.0, abs(size))


def on(shape, p):
    return shape.distToShape(Part.Vertex(V(*p)))[0] <= NEAR * max(1.0, *(abs(c) for c in p))


def element(s, signature):
    """The sub-element of `s` that `signature` describes, as Silico's rebuild describes it
    (silico-oracle's measures.rs, `Element`), by its name in `s` ("Face3"): a vertex at its
    point; an edge on its kind of curve, of its length, with its ends and middle on it; a
    face on its kind of surface, of its area, with its centre of area. Exactly one must fit:
    the two builds needn't number their elements alike."""
    (kind, d), = signature.items()
    if kind == "vertex":
        prefix, elements = "Vertex", s.Vertexes
        fits = lambda v: on(v, d["point"])
    elif kind == "edge":
        prefix, elements = "Edge", s.Edges
        fits = lambda e: (e.Curve.TypeId == CURVES[d["curve"]] and near(e.Length, d["length"], d["length"])
                          and all(on(e, d[k]) for k in ("start", "end", "middle")))
    elif kind == "face":
        prefix, elements = "Face", s.Faces
        fits = lambda f: (f.Surface.TypeId == SURFACES[d["surface"]] and near(f.Area, d["area"], d["area"])
                          and all(near(a, b, b) for a, b in zip(f.CenterOfMass, d["centroid"])))
    else:
        raise ValueError("unknown element " + kind)
    found = [i for i, x in enumerate(elements) if fits(x)]
    if len(found) != 1:
        raise RuntimeError("%d of its %ss fit %s" % (len(found), kind, json.dumps(signature)))
    return "%s%d" % (prefix, found[0] + 1)


def measure_pair(doc, m):
    """A measure case (silico-oracle's measures.rs): each body the pair is in built as a
    Part::Feature, each element found in it by its geometry, and the pair measured by
    MeasureDistance or MeasureAngle as the GUI's Measure tool makes them; for a distance,
    also distToShape's least distance, and how far Silico's witness points are from the
    elements they're on."""
    bodies = {}
    for body, spec in sorted(m["bodies"].items()):
        o = doc.addObject("Part::Feature", "Body")
        o.Shape = build(doc, spec)
        bodies[body] = o
    picked = [(bodies[e["body"]], element(bodies[e["body"]].Shape, e["element"])) for e in m["elements"]]
    kind = m["kind"]
    if kind not in ("distance", "angle"):
        raise ValueError("unknown measure " + kind)
    o = doc.addObject("Measure::MeasureDistance" if kind == "distance" else "Measure::MeasureAngle", "Measure")
    o.Element1 = (picked[0][0], [picked[0][1]])
    o.Element2 = (picked[1][0], [picked[1][1]])
    doc.recompute()
    if not o.isValid():
        raise RuntimeError(o.getStatusString())
    out = {"elements": [name for _, name in picked]}
    if kind == "angle":
        out["angle"] = o.Angle.Value
        return out
    shapes = [b.Shape.getElement(name) for b, name in picked]
    out["distance"] = o.Distance.Value
    out["from"] = list(o.Position1)
    out["to"] = list(o.Position2)
    out["dist_to_shape"] = shapes[0].distToShape(shapes[1])[0]
    if m["witness"] is not None:
        out["witness"] = [shape.distToShape(Part.Vertex(V(*p)))[0] for shape, p in zip(shapes, m["witness"])]
    return out


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
        if "measure" in case:
            outcome = json.dumps({"measured": measure_pair(doc, case["measure"])}, allow_nan=False)
        else:
            outcome = json.dumps({"built": measure(build(doc, case["shape"]))}, allow_nan=False)
    except Exception as e:
        outcome = json.dumps({"failed": str(e) or type(e).__name__})
    FreeCAD.closeDocument(doc.Name)
    print("\nSILICO_ORACLE_CASE " + outcome, flush=True)
