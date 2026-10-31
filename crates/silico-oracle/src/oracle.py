# FreeCAD's side of the oracle (silico-oracle's freecad.rs runs it with freecadcmd): builds
# each case in the JSON file SILICO_ORACLE_CASES names, from the one SILICO_ORACLE_FIRST
# numbers (from 0), with the calls Silico's port notes name, measures it, and prints its
# outcome as a line, "SILICO_ORACLE_CASE <json>", as soon as it has it, so a crash on one
# case loses only that one. "SILICO_ORACLE_READY" comes first, once the cases are read. A
# measure case (silico-oracle's measures.rs) builds the bodies a pair of elements is in and
# measures the pair instead, its outcome "measured". A sketch-edit case (silico-oracle's
# sketch_edit.rs) builds a sketch, edits it with Sketcher's own calls, solves it, and gives
# the sketch as it is then, its outcome "edited".
import json
import math
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
    also distToShape's least distance, how far Silico's witness points are from the
    elements they're on, and the distance from one's centre to the other (centre_to)."""
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
    out["center_to_element"] = centre_to(shapes)
    if m["witness"] is not None:
        out["witness"] = [shape.distToShape(Part.Vertex(V(*p)))[0] for shape, p in zip(shapes, m["witness"])]
    return out


def centre(shape):
    """A vertex's point, or a circle's or arc's centre; None for any other element."""
    if shape.ShapeType == "Vertex":
        return shape.Point
    if shape.ShapeType == "Edge" and isinstance(shape.Curve, Part.Circle):
        return shape.Curve.Center
    return None


def flat(shape):
    """A planar face or a straight edge."""
    if shape.ShapeType == "Face":
        return isinstance(shape.Surface, Part.Plane)
    return shape.ShapeType == "Edge" and isinstance(shape.Curve, Part.Line)


def centre_to(shapes):
    """Where one element has a centre (a vertex's point, a circle's or arc's centre) and the
    other is a planar face or a straight edge, the centre made a vertex and its least
    distance to the other: a vertex's own least distance, or a circle's centre as
    Measurement::circleCenterDistance builds it (BRepBuilderAPI_MakeVertex, then
    BRepExtrema_DistShapeShape; Measurement.cpp); else None."""
    for this, other in (shapes, shapes[::-1]):
        point = centre(this)
        if point is not None and flat(other):
            return Part.Vertex(point).distToShape(other)[0]
    return None


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


# Sketch edits (silico-oracle's sketch_edit.rs; docs/ports/sketch-edit.md, "The oracle"). A
# case is a sketch in FreeCAD's model (sketch.rs's freecad_sketch), each constraint named
# "s_" and its Silico ID, and an edit made with Sketcher's own calls: delGeometries, the
# vertex Delete's delConstraintOnPoint, Coincident constraints, split, and trim. Where the
# port note says Silico deliberately differs from FreeCAD 1.1.3, the result is brought to
# the note's form after the edit, and the note's line is listed in "applied": everything
# else is FreeCAD's own. Geometry is followed by its geometry ID, which edits keep (a split
# or trim's start-side piece keeps the curve's; a delete renumbers indices, not IDs).

GEO_UNDEF = -2000
# Within this of each other, two places are one (the gate's floor, LENGTH_TOLERANCE).
EDIT_TOLERANCE = 1e-7
POSITIONS = {"line": (1, 2), "arc": (1, 2, 3), "circle": (3,), "point": (1,)}
DIMENSIONS = ("Distance", "DistanceX", "DistanceY", "Radius", "Diameter", "Angle")


def sketch_geometry(g):
    k = g["kind"]
    if k == "point":
        return Part.Point(V(g["at"][0], g["at"][1], 0))
    if k == "line":
        return Part.LineSegment(V(g["start"][0], g["start"][1], 0), V(g["end"][0], g["end"][1], 0))
    c = Part.Circle(V(g["center"][0], g["center"][1], 0), V(0, 0, 1), g["radius"])
    if k == "circle":
        return c
    return Part.ArcOfCircle(c, g["from"], g["to"])


def curve_kind(geo):
    if isinstance(geo, Part.LineSegment):
        return "line"
    if isinstance(geo, Part.ArcOfCircle):
        return "arc"
    if isinstance(geo, Part.Circle):
        return "circle"
    if isinstance(geo, Part.Point):
        return "point"
    return type(geo).__name__


def refs_of(c):
    """A constraint's references, [geometry, position] each, position 0 for a whole curve."""
    refs = []
    for g, pos in ((c.First, c.FirstPos), (c.Second, c.SecondPos), (c.Third, c.ThirdPos)):
        if g == GEO_UNDEF:
            break
        refs.append([g, int(pos)])
    return refs


def gid(s, g):
    """Geometry `g`'s ID, which edits keep; the root point and axes (negative) as they are."""
    return s.getGeometryId(g) if g >= 0 else g


def index_of(s, geometry_id):
    if geometry_id < 0:
        return geometry_id
    for i in range(len(s.Geometry)):
        if s.getGeometryId(i) == geometry_id:
            return i
    return None


def captured(s, i):
    """Constraint i, its references by geometry ID, to put back after an edit."""
    c = s.Constraints[i]
    return {
        "kind": c.Type,
        "refs": [[gid(s, g), pos] for g, pos in refs_of(c)],
        "value": c.Value,
        "name": c.Name,
        "driving": c.Driving,
    }


def put_back(s, cap, refs=None):
    """Adds `cap` again, with `refs` (by index) in place of its own if given."""
    import Sketcher

    if refs is None:
        refs = [[index_of(s, g), pos] for g, pos in cap["refs"]]
    ints = []
    for g, pos in refs:
        ints.append(g)
        if pos:
            ints.append(pos)
    value = [cap["value"]] if cap["kind"] in DIMENSIONS else []
    i = s.addConstraint(Sketcher.Constraint(cap["kind"], *(ints + value)))
    if cap["name"]:
        s.renameConstraint(i, cap["name"])
    if not cap["driving"]:
        s.setDriving(i, False)
    return i


def drop(s, indices):
    if indices:
        s.delConstraints(sorted(set(indices)))


def named(s, name):
    return any(c.Name == name for c in s.Constraints)


def round_parameter(geo, point, first):
    """The angle of `point` about round `geo`'s centre, in [first, first + 2 pi)."""
    a = math.atan2(point.y - geo.Center.y, point.x - geo.Center.x)
    while a < first:
        a += 2 * math.pi
    while a >= first + 2 * math.pi:
        a -= 2 * math.pi
    return a


def contact(geo, other):
    """Where round or line `geo` touches `other` tangentially: the foot of a round's centre
    on a line, or the point of `geo` on the line through two rounds' centres, on the side
    of the other round, or away from it when `geo` lies inside it."""
    kind, okind = curve_kind(geo), curve_kind(other)
    if kind == "line" and okind in ("arc", "circle"):
        return foot(geo, other.Center)
    if okind == "line":
        f = foot(other, geo.Center)
        d = f - geo.Center
        return geo.Center + d * (geo.Radius / d.Length) if d.Length > 0 else f
    d = other.Center - geo.Center
    if d.Length == 0:
        return None
    if geo.Radius < other.Radius and abs(d.Length - (other.Radius - geo.Radius)) <= EDIT_TOLERANCE:
        d = d * -1
    return geo.Center + d * (geo.Radius / d.Length)


def foot(line, p):
    a, b = line.StartPoint, line.EndPoint
    u = b - a
    t = (p - a).dot(u) / u.dot(u)
    return a + u * t


class EditRefused(Exception):
    """FreeCAD's own edit call raised: its refusal. Anything else that raises is the harness's."""


# What a call raises when the harness calls it wrongly (a wrong argument, a missing name), never
# FreeCAD refusing the edit: these surface as the harness's own failure.
HARNESS_ERRORS = (TypeError, AttributeError, NameError, KeyError, AssertionError, ImportError)


def edit_call(call, *args):
    """Calls FreeCAD's edit `call`; what it raises is FreeCAD's refusal, but for an error of
    the harness's own (HARNESS_ERRORS), which is the harness's failure."""
    try:
        return call(*args)
    except HARNESS_ERRORS:
        raise
    except Exception as e:
        raise EditRefused(str(e) or type(e).__name__)


def edit_delete(s, e, applied):
    import Sketcher

    gone = [i for i, c in enumerate(s.Constraints) if c.Name in set(e["constraints"])]
    if gone:
        edit_call(s.delConstraints, sorted(set(gone)))
    # An end-to-end tangency deleted whose ends nothing else joins: Silico's removal leaves
    # the point shared (sketch-edit.md:97), so the ends are joined again, a note form.
    for g1, p1, g2, p2 in e["rejoin"]:
        s.addConstraint(Sketcher.Constraint("Coincident", g1, p1, g2, p2))
    if e["geometries"]:
        edit_call(s.delGeometries, sorted(e["geometries"]))
    # 1.1.3 moves an endpoint-to-edge Tangent on a deleted end onto its neighbour as a
    # malformed Coincident(point, edge) (sketch-edit.md:120); Silico removes the tangency
    # with its curve (:124).
    bad = [i for i, c in enumerate(s.Constraints)
           if c.Type == "Coincident" and (c.FirstPos == 0 or c.SecondPos == 0)]
    if bad:
        drop(s, bad)
        applied.append(120)


def edit_unjoin(s, e, applied):
    g, pos = e["geometry"], e["position"]
    # An end-to-end tangency between the curve and another at the point: Silico removes it,
    # since they no longer meet (sketch-edit.md:272); the vertex Delete keeps it, and with
    # it the joint, which FreeCAD unjoins by deleting the tangency.
    ends = [i for i, c in enumerate(s.Constraints)
            if c.Type in ("Tangent", "Perpendicular") and c.FirstPos != 0 and c.SecondPos != 0
            and ((c.First == g and c.FirstPos == pos) or (c.Second == g and c.SecondPos == pos))]
    if ends:
        drop(s, ends)
        applied.append(272)
    joined = any(c.Type == "Coincident" and [g, pos] in refs_of(c) for c in s.Constraints)
    if ends and not joined:
        return
    # The vertex Delete drops the curve's own length (0358ff7ce2), which Silico keeps
    # (sketch-edit.md:266, 267): put back.
    lengths = [captured(s, i) for i, c in enumerate(s.Constraints)
               if c.Type in ("Distance", "DistanceX", "DistanceY") and c.First == g
               and c.FirstPos == 0 and c.Second == GEO_UNDEF]
    edit_call(s.delConstraintOnPoint, g, pos)
    for cap in lengths:
        if not named(s, cap["name"]):
            put_back(s, cap)
            applied.append(273)


def edit_vertex_delete(s, e, applied):
    # The Gui's Delete on a curve's end (sketch-edit.md:133-135), as it is: no note form.
    edit_call(s.delConstraintOnPoint, e["geometry"], e["position"])


def edit_merge(s, e, applied):
    import Sketcher

    for g1, p1, g2, p2 in e["coincidences"]:
        # The Coincident command's substitution (substituteConstraintCombinationsCoincident,
        # Gui/CommandConstraints.cpp, from 180d722a53; sketch-edit.md:68): two ends joined
        # whose curves have an edge Tangent get an end-to-end Tangent in its place.
        tangents = [i for i, c in enumerate(s.Constraints)
                    if c.Type == "Tangent" and c.Third == GEO_UNDEF and c.FirstPos == 0
                    and c.SecondPos == 0 and sorted([c.First, c.Second]) == sorted([g1, g2])]
        if tangents and p1 in (1, 2) and p2 in (1, 2):
            edit_call(s.delConstraint, tangents[0])
            edit_call(s.addConstraint, Sketcher.Constraint("Tangent", g1, p1, g2, p2))
            applied.append(68)
        else:
            edit_call(s.addConstraint, Sketcher.Constraint("Coincident", g1, p1, g2, p2))


def edit_split(s, e, applied):
    import Sketcher

    g = e["geometry"]
    at = [V(p[0], p[1], 0) for p in e["at"]]
    kind = curve_kind(s.Geometry[g])
    if kind not in ("line", "arc", "circle"):
        # 1.1.3 segfaults splitting a point (sketch-edit.md:143): what it does is the case's.
        edit_call(s.split, g, at[0])
        return
    if kind == "circle" and len(at) != 2:
        # A circle split at one position: 1.1.3's one arc, whose ends aren't joined (:149),
        # which Silico refuses (decision 3, :534).
        edit_call(s.split, g, at[0])
        return
    original = s.Geometry[g].copy()
    keep = gid(s, g)
    ids_before = {gid(s, i) for i in range(len(s.Geometry))}
    # What 1.1.3 does otherwise than the note's Silico column, taken off before the split
    # and put back after as the note has it: an angle, on which 1.1.3 segfaults (:163); an
    # `on` or point-to-line distance, which it drops (:167); a symmetric about the curve,
    # which it drops (:170); and a tangency at the curve's end, which it makes a Coincident
    # (:150, :171); a circle's or arc's edge tangency, which 1.1.3 drops (c823a6b270,
    # :391); and a tangent_on onto the curve, which 1.1.3 puts on both of a line's pieces
    # and drops from a round's (:172).
    held = []
    for i, c in enumerate(s.Constraints):
        refs = refs_of(c)
        if [g, 0] not in refs and not any(r[0] == g for r in refs):
            continue
        if c.Type == "Angle":
            held.append((i, "angle"))
        elif c.Type == "PointOnObject" and refs[1] == [g, 0]:
            held.append((i, "on"))
        elif c.Type == "Distance" and len(refs) == 2 and refs[0][1] != 0 and refs[1] == [g, 0]:
            held.append((i, "on"))
        elif c.Type == "Symmetric" and len(refs) == 3 and refs[2] == [g, 0]:
            held.append((i, "symmetric"))
        elif c.Type == "Tangent" and any(r[0] == g and r[1] in (1, 2) for r in refs):
            held.append((i, "end"))
        elif (c.Type == "Tangent" and kind in ("arc", "circle") and len(refs) == 2
              and all(r[1] == 0 for r in refs)):
            held.append((i, "round_tangent"))
        elif c.Type == "Tangent" and len(refs) == 2 and refs[1] == [g, 0] and refs[0][1] != 0:
            held.append((i, "onto"))
    caps = [(captured(s, i), rule) for i, rule in held]
    drop(s, [i for i, _ in held])
    edit_call(s.split, g, at[0])
    if kind == "circle":
        first = index_of(s, keep)
        arc = s.Geometry[first]
        if arc.LastParameter - arc.FirstParameter < math.pi:
            # 1.1.3's split of a circle can leave an arc whose two parameters are one, not a
            # full turn apart (at 293.8 degrees, for one: OCCT folds the turn's end back onto
            # its start), and its second split then fails ("U1 == U2"). The arc of a full turn
            # the note has it make (:149) in its place; setting the geometry gives every
            # piece a new ID, so each gets its own back.
            ids = [gid(s, i) for i in range(len(s.Geometry))]
            turn = Part.ArcOfCircle(Part.Circle(arc.Center, V(0, 0, 1), arc.Radius),
                                    arc.FirstParameter, arc.FirstParameter + 2 * math.pi)
            geometry = list(s.Geometry)
            geometry[first] = turn
            s.Geometry = geometry
            for i, geometry_id in enumerate(ids):
                s.setGeometryId(i, geometry_id)
            applied.append(149)
        edit_call(s.split, first, at[1])
    g = index_of(s, keep)
    new = [gid(s, i) for i in range(len(s.Geometry)) if gid(s, i) not in ids_before]
    if len(new) != 1 or g is None:
        return
    piece = index_of(s, new[0])
    split_at = s.getPoint(g, 2) if kind != "circle" else s.getPoint(g, 2)
    if kind == "line":
        ts = original.parameter(split_at)
        which = lambda point: g if original.parameter(point) < ts else piece
        span = lambda t: g if 0 <= t <= ts else (piece if ts < t <= original.LastParameter else g)
        param = lambda point: original.parameter(point)
    elif kind == "circle":
        # A circle's two arcs, by angle about its centre, whatever range each one's
        # parameters run over (the second's needn't run on from the first's): the
        # start-side piece holds an angle within its sweep, the other piece the rest.
        def within(geo, t):
            a0, a1 = geo.FirstParameter, geo.LastParameter
            return (t - a0) % (2 * math.pi) <= (a1 - a0) % (2 * math.pi)

        param = lambda point: math.atan2(point.y - original.Center.y, point.x - original.Center.x)
        which = lambda point: g if within(s.Geometry[g], param(point)) else piece
        span = lambda t: g if within(s.Geometry[g], t) else piece
    else:
        first = s.Geometry[g].FirstParameter
        last = s.Geometry[piece].LastParameter
        ts = s.Geometry[g].LastParameter
        which = lambda point: g if original.parameter(point) < ts else piece
        param = lambda point: round_parameter(original, point, first)
        span = lambda t: g if t <= ts else (piece if t <= last else g)
    if kind == "circle":
        # A circle splits at two positions into two arcs that meet at both (decision 3,
        # sketch-edit.md:534); 1.1.3's first split leaves an arc whose ends aren't joined
        # (:149).
        s.addConstraint(Sketcher.Constraint("Coincident", g, 1, piece, 2))
        applied.append(534)
    if kind == "line":
        # Line pieces held collinear by an end-to-end Tangent at the split point, as main
        # since 6d06b61c7e; 1.1.3 joins them with a Coincident (decision 1, :154-155, :532).
        joint = [i for i, c in enumerate(s.Constraints) if c.Type == "Coincident"
                 and sorted(map(tuple, refs_of(c))) == sorted([(g, 2), (piece, 1)])]
        if joint:
            drop(s, joint)
            s.addConstraint(Sketcher.Constraint("Tangent", g, 2, piece, 1))
            applied.append(532)
        # Direction constraints on the start-side piece only, which 1.1.3 copies to both
        # (decision 1, :161-162).
        on_g = {(c.Type, tuple(map(tuple, [r for r in refs_of(c) if r != [g, 0]])))
                for c in s.Constraints if [g, 0] in refs_of(c)}
        copies = [i for i, c in enumerate(s.Constraints)
                  if c.Type in ("Horizontal", "Vertical", "Parallel", "Perpendicular")
                  and [piece, 0] in refs_of(c)
                  and (c.Type, tuple(map(tuple, [r for r in refs_of(c) if r != [piece, 0]]))) in on_g]
        if copies:
            drop(s, copies)
            applied.append(161)
    # An edge tangency, which 1.1.3 copies to both pieces, to the piece whose span holds the
    # contact, else the start-side one (decision 2, :168, :533).
    both = []
    for i, c in enumerate(s.Constraints):
        refs = refs_of(c)
        if c.Type == "Tangent" and len(refs) == 2 and all(r[1] == 0 for r in refs) and [piece, 0] in refs:
            other = [r[0] for r in refs if r != [piece, 0]]
            if other and any(c2.Type == "Tangent" and sorted(map(tuple, refs_of(c2))) == sorted([(g, 0), (other[0], 0)])
                             for c2 in s.Constraints):
                both.append((i, other[0]))
    wrong = []
    for i, other in both:
        point = contact(original, s.Geometry[other]) if other >= 0 else None
        holder = span(param(point)) if point is not None else g
        if holder == piece:
            wrong += [j for j, c2 in enumerate(s.Constraints) if c2.Type == "Tangent"
                      and sorted(map(tuple, refs_of(c2))) == sorted([(g, 0), (other, 0)])]
        else:
            wrong.append(i)
    if wrong:
        drop(s, wrong)
        applied.append(168)
    # The constraints taken off, put back as the note has them.
    for cap, rule in caps:
        refs = [[index_of(s, r), pos] for r, pos in cap["refs"]]
        if rule == "angle":
            put_back(s, cap, refs)
            applied.append(163)
        elif rule == "on":
            point_ref = refs[0]
            point = s.getPoint(point_ref[0], point_ref[1])
            meets = [pos for pos in ((1, 2) if kind == "circle" else (2,))
                     if (s.getPoint(g, pos) - point).Length <= EDIT_TOLERANCE]
            if meets and cap["kind"] == "PointOnObject":
                # A point held on the curve at the split position is the split point,
                # shared, its `on` implied (:177).
                s.addConstraint(Sketcher.Constraint("Coincident", g, meets[0], point_ref[0], point_ref[1]))
                applied.append(177)
            else:
                refs[1] = [which(point), 0]
                put_back(s, cap, refs)
                applied.append(167)
        elif rule == "symmetric":
            refs[2] = [g, 0]
            put_back(s, cap, refs)
            applied.append(170)
        elif rule == "round_tangent":
            # To the piece whose span holds the contact, else the start-side one (decision
            # 2, :168, :533).
            other = [r for r in refs if r[0] != g][0][0]
            point = contact(original, s.Geometry[other]) if other >= 0 else None
            holder = span(param(point)) if point is not None else g
            refs = [[holder, 0] if r == [g, 0] else r for r in refs]
            put_back(s, cap, refs)
            applied.append(391)
        elif rule == "onto":
            # To the piece holding its point, as main's findPieceContainingPoint (:172).
            point = s.getPoint(refs[0][0], refs[0][1])
            refs[1] = [which(point), 0]
            put_back(s, cap, refs)
            applied.append(172)
        elif rule == "end":
            # The outer ends: the curve's start stays the start-side piece's, its end is now
            # the other piece's.
            refs = [[piece, 2] if (r == [g, 2] and kind != "circle") else r for r in refs]
            put_back(s, cap, refs)
            applied.append(171)


def edit_trim(s, e, applied):
    import Sketcher

    g = e["geometry"]
    keep = gid(s, g)
    ids_before = {gid(s, i) for i in range(len(s.Geometry))}
    geo = s.Geometry[g]
    kind = curve_kind(geo)
    ends = [s.getPoint(g, pos) for pos in (1, 2)] if kind in ("line", "arc") else []
    # Cutter ends held on the curve, by a PointOnObject or an endpoint-to-edge Tangent: the
    # only ones Silico's pieces share (decision 5, :238, :536).
    held = set()
    for c in s.Constraints:
        if c.Type in ("PointOnObject", "Tangent") and c.FirstPos in (1, 2) and c.Second == g and c.SecondPos == 0:
            held.add((gid(s, c.First), int(c.FirstPos)))
    joined = {frozenset([(gid(s, a), pa), (gid(s, b), pb)])
              for c in s.Constraints if c.Type == "Coincident"
              for (a, pa), (b, pb) in [refs_of(c)]}
    about = [captured(s, i) for i, c in enumerate(s.Constraints)
             if c.Type == "Symmetric" and len(refs_of(c)) == 3 and refs_of(c)[2] == [g, 0]]
    edit_call(s.trim, g, V(e["pick"][0], e["pick"][1], 0))
    g = index_of(s, keep)
    if g is None:
        return
    pieces = {keep} | {gid(s, i) for i in range(len(s.Geometry)) if gid(s, i) not in ids_before}
    # A new end at a cut joined to the cutter's end by 1.1.3's 5e-5 mm test, where no
    # constraint held that end on the curve: Silico gives the piece a new point on the
    # cutter instead (decision 5, :238, :536).
    swap = []
    for i, c in enumerate(s.Constraints):
        if c.Type != "Coincident":
            continue
        (a, pa), (b, pb) = refs_of(c)
        ka, kb = (gid(s, a), pa), (gid(s, b), pb)
        if frozenset([ka, kb]) in joined:
            continue
        for (pg, pp), (og, op) in ((ka, kb), (kb, ka)):
            if pg in pieces and og not in pieces and og >= 0 and (og, op) not in held:
                at = s.getPoint(index_of(s, pg), pp)
                if any((at - end).Length <= EDIT_TOLERANCE for end in ends):
                    continue
                swap.append((i, index_of(s, pg), pp, index_of(s, og)))
                break
    if swap:
        drop(s, [i for i, *_ in swap])
        for _, pg, pp, og in swap:
            s.addConstraint(Sketcher.Constraint("PointOnObject", pg, pp, og))
        applied.append(536)
    # A cutter's perpendicular, which 1.1.3 makes an end-to-edge perpendicular at its cut
    # (:210): Silico's is the perpendicular between the directions and the new end's on
    # the cutter, which together are FreeCAD's (:246).
    ends = [(i, refs_of(c)) for i, c in enumerate(s.Constraints)
            if c.Type == "Perpendicular" and len(refs_of(c)) == 2
            and refs_of(c)[0][1] != 0 and refs_of(c)[1][1] == 0
            and gid(s, refs_of(c)[0][0]) in pieces]
    if ends:
        caps = [captured(s, i) for i, _ in ends]
        drop(s, [i for i, _ in ends])
        for cap, (_, ((pg, pp), (og, _))) in zip(caps, ends):
            put_back(s, cap, [[index_of(s, cap["refs"][0][0]), 0], [index_of(s, cap["refs"][1][0]), 0]])
            s.addConstraint(Sketcher.Constraint("PointOnObject", index_of(s, cap["refs"][0][0]), pp, index_of(s, cap["refs"][1][0])))
        applied.append(246)
    # A line only shortened keeps a symmetric about it, which 1.1.3 drops (:247).
    if len(pieces) == 1 and kind == "line":
        for cap in about:
            if not named(s, cap["name"]) and all(index_of(s, r) is not None for r, _ in cap["refs"]):
                put_back(s, cap)
                applied.append(247)


def sketch_edit_outcome(doc, job):
    """A sketch-edit case's outcome: "edited", FreeCAD's edit refusing ("failed"), or the
    harness's own code failing around it ("harness"), never taken for FreeCAD's refusal."""
    try:
        return json.dumps({"edited": sketch_edit(doc, job)}, allow_nan=False)
    except EditRefused as e:
        return json.dumps({"failed": str(e)})
    except Exception as e:
        return json.dumps({"harness": "%s: %s" % (type(e).__name__, e)})


def edit_extend(s, e, applied):
    # FreeCAD's own extend (SketchObject::extend): a line's end through moveGeometry's drag,
    # an arc's range written by setRange, each then solved (sketch-edit.md section 7).
    edit_call(s.extend, e["geometry"], e["increment"], e["position"])
    if e["target"] is None:
        return
    # A line's moving end, written at the request's target after the drag, as decision (3)
    # writes Silico's (sketch-edit.md:333), at each of FreeCAD's places that are Silico's
    # point, so both solves start alike; the rest of FreeCAD's drag frame as it left it. An
    # arc is set as Silico's model takes one from its points (arc_from_points): the radius
    # from its centre to its start, the angles of its start and end. The geometry list set
    # whole is a property change, which FreeCAD takes without a solve; a point's copy loses
    # its geometry ID and construction, so each is set back as it was.
    at = V(e["target"][0], e["target"][1], 0)
    ids = [(i, s.getGeometryId(i)) for i in range(len(s.Geometry))]
    construction = [s.getConstruction(i) for i in range(len(s.Geometry))]
    geos = s.Geometry
    for g, pos in e["places"]:
        geo = geos[g]
        kind = curve_kind(geo)
        if kind == "line" and pos == 1:
            geo.StartPoint = at
        elif kind == "line" and pos == 2:
            geo.EndPoint = at
        elif kind == "circle" and pos == 3:
            geo.Center = at
        elif kind == "arc" and pos in (1, 2, 3):
            c, a, b = geo.Center, geo.StartPoint, geo.EndPoint
            c, a, b = [at if pos == p else q for p, q in ((3, c), (1, a), (2, b))]
            start = math.atan2(a.y - c.y, a.x - c.x)
            end = math.atan2(b.y - c.y, b.x - c.x)
            if end < start:
                end += 2 * math.pi
            geo.Center = c
            geo.Radius = math.hypot(a.x - c.x, a.y - c.y)
            geo.setParameterRange(start, end)
        elif kind == "point" and pos == 1:
            geo.X, geo.Y = at.x, at.y
        else:
            raise RuntimeError("extend: geometry %d's position %d (a %s) can't be written" % (g, pos, kind))
    s.Geometry = geos
    s.setGeometryIds(ids)
    for i, was in enumerate(construction):
        if s.getConstruction(i) != was:
            s.setConstruction(i, was)
    applied.append(333)


SKETCH_EDITS = {
    "delete": edit_delete,
    "unjoin": edit_unjoin,
    "vertex_delete": edit_vertex_delete,
    "merge": edit_merge,
    "split": edit_split,
    "trim": edit_trim,
    "extend": edit_extend,
}


def solve_state(s):
    code = s.solve()
    return {
        "code": code,
        "dof": s.DoF,
        "conflicting": list(s.ConflictingConstraints),
        "redundant": list(s.RedundantConstraints),
        "partially_redundant": list(s.PartiallyRedundantConstraints),
        "malformed": list(s.MalformedConstraints),
    }


def geometry_now(s):
    out = []
    for i, geo in enumerate(s.Geometry):
        kind = curve_kind(geo)
        points = {}
        for pos in POSITIONS.get(kind, ()):
            p = s.getPoint(i, pos)
            points[str(pos)] = [p.x, p.y]
        out.append({
            "id": s.getGeometryId(i),
            "kind": kind,
            "points": points,
            "radius": geo.Radius if kind in ("arc", "circle") else None,
            "construction": bool(s.getConstruction(i)),
        })
    return out


def built(doc, name, geometry, job):
    """A sketch of `geometry` with the job's construction and constraints, as given."""
    import Sketcher

    s = doc.addObject("Sketcher::SketchObject", name)
    for g, construction in zip(geometry, job["construction"]):
        s.addGeometry(sketch_geometry(g), construction)
    made = []
    for c in job["constraints"]:
        made.append(Sketcher.Constraint(*([c["kind"]] + c["ints"] + ([c["value"]] if c["value"] is not None else []))))
    if made:
        s.addConstraint(made)
    for i, c in enumerate(job["constraints"]):
        if c["name"] is not None:
            s.renameConstraint(i, c["name"])
        if not c["driving"]:
            s.setDriving(i, False)
    return s


def placed_solve(doc, job):
    """For points merged from apart, the merge of the sketch with them placed where Silico's
    merge places them (sketch-edit.md:303), solved: FreeCAD's solve from Silico's start, and
    the geometry it leaves. None where FreeCAD refuses that merge."""
    p = built(doc, "Placed", job["edit"]["placed"], job)
    try:
        edit_merge(p, job["edit"], [])
    except EditRefused:
        return None
    return {"solve": solve_state(p), "geometry": geometry_now(p)}


def sketch_edit(doc, job):
    s = built(doc, "S", job["geometry"], job)
    before = solve_state(s)
    ids_before = [s.getGeometryId(i) for i in range(len(s.Geometry))]
    applied = []
    SKETCH_EDITS[job["edit"]["kind"]](s, job["edit"], applied)
    edited = geometry_now(s)
    after = solve_state(s)
    return {
        "before": before,
        "ids_before": ids_before,
        "edited": edited,
        "geometry": geometry_now(s),
        "constraints": [
            {"kind": c.Type, "refs": refs_of(c), "value": c.Value, "driving": c.Driving, "name": c.Name}
            for c in s.Constraints
        ],
        "after": after,
        "applied": sorted(set(applied)),
        "placed": placed_solve(doc, job) if job["edit"].get("placed") else None,
    }


with open(os.environ["SILICO_ORACLE_CASES"], encoding="utf-8") as f:
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
        elif "sketch_edit" in case:
            outcome = sketch_edit_outcome(doc, case["sketch_edit"])
        else:
            outcome = json.dumps({"built": measure(build(doc, case["shape"]))}, allow_nan=False)
    except Exception as e:
        outcome = json.dumps({"failed": str(e) or type(e).__name__})
    FreeCAD.closeDocument(doc.Name)
    print("\nSILICO_ORACLE_CASE " + outcome, flush=True)
