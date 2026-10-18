# Profile to face

How a solved sketch becomes the flat faces a pad, pocket, or revolve sweeps: milestone M3.5 (docs/roadmap/m3-sketcher.md). This note builds on docs/ports/sketcher.md (*The sketch feature* and *Profiles to faces*) and corrects it where the source says otherwise (the last section). Decisions 2, 3, and 8 are taken: the core decides closure, crossing, and nesting from the solved 2D geometry, and the worker only builds the faces it's told to; a pad of disjoint regions is refused (at M3.6); regions of a crossing sketch are a later step.

Sources: FreeCAD **1.1.3** (tag `145529fe74`, read at the tag, which the maintainer's clone doesn't have), its OCCT 7.9.3, and main at `876271d660` (2026-10-04) for history and for where main differs. Line numbers are 1.1.3's. `Part/` is `src/Mod/Part/App/`, `PartDesign/` is `src/Mod/PartDesign/App/`, `Sketcher/` is `src/Mod/Sketcher/App/`, and OCCT's paths are under its `src/`. "Measured" means the oracle's FreeCAD (1.1.3 on OCCT 7.9.3, `.pixi/envs/oracle/bin/freecadcmd`) on 2026-10-18: a sketch of unconstrained geometry, `Part.makeFace(sketch.Shape, "Part::FaceMakerBullseye")`, and a PartDesign pad 1 long in a body, refine off.

## What FreeCAD does

### 1. The sketch's shape: wires by distance

`SketchObject::buildShape` (`Sketcher/SketchObject.cpp:327-433`) runs after every solve.

- It takes the **solved** geometry (`solvedSketch.extractGeometry()`, `:356`, since `d2637ec881`) and skips construction geometry (`:359`).
- Each non-construction point becomes a free vertex. Each curve becomes its own edge with its own two vertices (OCCT's `BRepBuilderAPI_MakeEdge`, vertex tolerance `Precision::Confusion()`, 1e-7 mm). An edge shorter than 1e-7 only warns (`:351`).
- No curves and no points give an empty shape and no error (`:401-405`).
- The edges are joined into wires by `TopoShape::makeElementWires` (`:409`; with points, `:419`, then a compound with the vertices, `:425`).

`makeElementWires` (`Part/TopoShapeExpansion.cpp:3114-3215`), on the path the sketch takes:

- It starts a wire with the first edge left, then scans the rest from the start for the first edge `BRepBuilderAPI_MakeWire` accepts, until none is (`:3171-3213`). Anything but `DisconnectedWire` counts as accepted (`:3192`), so a non-manifold join is kept.
- `BRepLib_MakeWire::Add` (OCCT `BRepLib/BRepLib_MakeWire.cxx:214`) joins an edge where one of its vertices is **strictly** closer than either vertex's tolerance to **any** vertex already in the wire, not only to a free end. So a T-junction, a figure-8, and two loops meeting at a point each become one wire.
- The two vertices become one, moved between them, with tolerance (tolW + tolE + d)/2 (`:294-317`). A joined vertex therefore reaches further for the next join.
- The `tol` argument (`:3125`) isn't used on this path.
- Measured: ends 0.99e-7 apart join. So do ends 1e-7 apart along an axis, whose computed distance rounds below 1e-7. Ends 1.0000001e-7 apart along an axis don't, nor do ends 1.004e-7 apart on a diagonal. A rectangle closed across a 1e-7 gap pads to 11.999999967 instead of 12, because the corner moved.
- Each wire then goes through `TopoShape::fix()` (`:3212`; `Part/TopoShape.cpp:3377-3428`): `ShapeFix_Shape` on a copy, kept only if the result is valid.
- One wire is returned as itself, several as a compound.

### 2. The profile

`ProfileBased::getTopoShapeVerifiedFace` (`PartDesign/FeatureSketchBased.cpp:201-366`):

- It takes the sketch's shape with its placement applied. A null shape is "Linked shape object is empty" (`:269`).
- Wires go to `makeElementFace()` with no maker and no plane (`:310`), so FaceMakerBullseye with a fitted plane. A feature with `AllowMultiFace` off uses FaceMakerCheese instead (`:313`); every new feature turns it on (`:121-124`), so only files from before `4dd16c58ed` (2020) have it off.
- No face is "Cannot make face from profile" (`:336`). Several faces are returned as they are (`:347-352`).

`TopoShape::makeElementFace` (`Part/TopoShapeExpansion.cpp:4792-4851`) uses FaceMakerBullseye unless told otherwise (`:4800`). If OCCT calls the result invalid, it runs `ShapeFix_ShapeTolerance` and `ShapeFix_Shape`. If it's still invalid, it only warns: "makeElementFace: resulting face is invalid" (`:4827-4848`).

`FaceMaker` (`Part/FaceMaker.cpp`):

- `addTopoShape` (`:54-87`) keeps wires, makes an edge into a one-edge wire (`:69`), passes faces through, ignores vertices (`:76-79`, `26ebefd651`), and refuses anything else.
- `Build` (`:129-182`) runs `Build_Essence`, then builds each nested compound with a new maker of the same type. That maker isn't given the plane (`FaceMakerBullseye.h:62`, a FIXME).
- `postBuild` names each face after the names of its outer wire's edges (`:208-265`). Later features see faces by those names.

The pad (`PartDesign/FeatureExtrude.cpp:700-800`) extrudes each face. With no base, it fuses the resulting solids into one (`:782-783`). A body allows several solids by default (`AllowCompound`, `f8c47b55cb`, 2024).

### 3. FaceMakerBullseye

`Build_Essence` (`Part/FaceMakerBullseye.cpp:74-181`):

1. **Closed.** Every wire must pass `BRep_Tool::IsClosed`, or "Wire is not closed." (`:81-85`). For a wire that's a **vertex-parity** test (OCCT `BRep/BRep_Tool.cxx:1640-1654`): each vertex must occur an even number of times among its edges' ends. A dangling end (1) fails. A T-junction's foot and a shared edge's ends (3) fail. A figure-8's waist (4) passes, and so does a circle's seam (2: the one edge's two ends).
2. **Plane** (`:88-105`). Bullseye takes `setPlane`'s plane if it was given one, and PartDesign never gives one. Otherwise it fits one with `BRepLib_FindSurface` on copies of every wire (`488419cd73`), or fails with "Wires are not coplanar". The sign of the plane's normal doesn't matter: the pad takes its direction from the sketch's placement.
3. **Sort** (`:107-121`). Each wire gets its tight bounding box (`BRepBndLib::AddOptimal`, no triangulation; empty boxes skipped). The wires are sorted largest first by the box's **squared** diagonal, with `std::stable_sort` and a comparator that calls two wires tied when they differ by 1e-7 mm² or less (`:69-72`). That isn't a strict weak order, since ties don't chain. Tied wires keep their input order, which is the order of their first edges in the sketch.
4. **Classify** (`:123-180`). Each wire, in that order, is tested against the faces made so far. The loop runs newest face first and doesn't stop at a hit (its `break` only leaves the `switch`), so the oldest face that hits wins. `hitTest` (`:201-258`) classifies the wire's **first vertex** only:
   - Against the face's outer boundary (a copy of the face without holes, made at its first hole, `:270-283`): a vertex outside it or **on** it is no hit (`:229-231`).
   - Inside it, against the face with its holes: a vertex on the material is a hit, and the wire becomes a hole of that face (`:242-243`). A vertex on a hole's boundary is also a hit, and the wire becomes another hole (`:244-251`). A vertex inside a hole is no hit, and the wire starts a new face, an island (`:253-254`).
   - With no hit, the wire starts a new face (`:160-163`). That's even–odd nesting to any depth, decided by one point.
   - The shared-vertex shortcuts (`:205-218`) never fire for a sketch. Wires that share a vertex were already joined into one wire.
5. **Orientation.** An outer wire running clockwise about the plane is reversed (`:190-192`), and so is a hole running counter-clockwise (`:273-275`). `getWireDirection` (`:339-358`) fits the wire's own plane (`BRepBuilderAPI_MakeFace(wire, OnlyPlane)`) and compares its normal with the plane's. A wire OCCT can't make a test face from throws "getWireDirection: Failed to create face from wire" (`9c17fe359b`).
6. **Result.** Every face, in the order its outer wire was met. Vertices were dropped earlier. With no faces, the profile fails with "Cannot make face from profile".

Nothing checks that wires don't cross, overlap, or touch, and a single point decides where each wire goes. *Edge cases* shows what that gives.

### FaceMakerCheese (history)

Cheese was PartDesign's face maker until `7786e24fcd` (2022, #6366). Now only features with `AllowMultiFace` off use it. `Part/FaceMakerCheese.cpp`:

- It sorts wires by bounding-box diagonal (`:101-115`; `std::sort`, no rule for ties).
- It takes the largest wire as an outer boundary, and **every** remaining wire whose first vertex is inside that one as its hole (`isInside`, `:117-160`). The test came in with `cc96c416c1` (2011), whose TODO notes that crossing wires go unchecked.
- The rest become other faces (`:200-250`). An island inside a hole becomes a second hole of the outer face.
- The sort has been a FIXME since 2011: `f86c65644c` (#1133) tried another sort and kept the first.

### Where main differs

- `88c80b3ed8` (2026-04) makes `getWireDirection` build its test face on the given plane instead of fitting each wire's own (`BRepBuilderAPI_MakeFace(plane, wire, Inside)`). That's faster, with the same direction for a flat wire. The same commit added FreeCAD's first Bullseye tests (below).
- `7186d30f6e` (2026-06) adds FaceMakerUnified, which fuses overlapping wires and splits crossings into regions (even–odd), for Part's extrusion and loft. PartDesign's profiles still use Bullseye on main (`FeatureSketchBased.cpp:332`).
- Main gives new sketches internal faces (regions) by default (`be1d53cf5f`), built by FaceMakerBuildFace (`24ab301685`). That's decision 8's later step, not M3.5.

## Edge cases

Measured on the oracle. The Silico column is M3.5's rule (*Where Silico differs on purpose*). "Pad valid" means the pad's status was `Valid`, even where OCCT calls the shape invalid.

| Case | FreeCAD 1.1.3 | Silico |
|---|---|---|
| 4 × 3 rectangle | one face, 12 | the same |
| a circle, r 2 | 4π (a one-edge wire, closed at its seam) | the same; a circle is a loop by itself |
| an arc r 2 and its chord | 2π | the same |
| a slot: half-circles r 1 joined by lines 4 long | 8 + π | the same |
| three sides of the rectangle | "Wire is not closed." | refused, naming each open end and the curve ending there |
| the fourth side ending 0.99e-7 short | closed, the corner moved; pad 11.999999967 | refused, naming both points: "share the point" (D2) |
| the same, 1.5e-7 short | "Wire is not closed." | refused as open, naming both ends |
| a line into the rectangle from a point of its outline (a side split there), its far end free | "Wire is not closed." | refused: an open end, and a point where three curves meet |
| a line across the rectangle, its ends on two sides but not at points | "Wire is not closed." (the line is its own open wire) | refused: two open ends, each touching a side |
| two loops sharing a line (θ) | "Wire is not closed." (two vertices occur three times) | refused, naming the two points where three curves meet, and the curves |
| two triangles meeting at a point (figure-8) | one closed wire; two faces of 2; pad: two solids | refused: four curves meet at the point |
| a bow-tie: two sides crossing between points | one closed wire; ShapeFix splits it into two faces of 2; two solids | refused, naming the two crossing lines |
| two unit squares meeting at a corner | one wire; two faces; two solids | refused: four curves meet at the point |
| two unit squares, each with its own line along x = 1 | one wire (the lines' ends merge); one face of 2 | refused: the two lines overlap |
| rectangles (0,0)–(20,10) and (10,5)–(30,15), in that order | tied diagonals keep their order; the second becomes a hole crossing the first: one face of area −0 (200 − 200); pad valid, volume 0 | refused, naming the crossing lines |
| the same, in the other order | two faces; the pad fuses them: 350 | the same refusal |
| a 10 × 2 rectangle across the right side of a 10 × 10 square, its first point inside | a hole crossing the boundary: 80 (neither the difference, 90, nor the union, 110) | refused, naming the crossing lines |
| the same, its first point outside | two faces; pad: the union, 110 | refused |
| circles r 10 and 5, crossing | two faces; pad: their union | refused, naming both circles |
| a triangular hole with a corner at the square's corner | one wire: 110.5 (the triangle added) where the hole gives 89.5 | refused: four curves meet at the point |
| a triangular hole whose first point is on the square's side | a separate face; pad 100: the hole is lost | refused: the point touches the side |
| the same hole, its last point on the side | a hole: 94 | refused |
| a circle r 5 in a 10 × 10 square, touching all four sides | its seam, (cx + r, cy), lies on the right side, so a separate face; pad 100, the hole lost. Drawn as an arc with its seam elsewhere: a hole, 100 − 25π | refused, naming the circle and the four sides |
| a circle inside a hole, tangent to it, the seams at one place | one wire; both counted as holes; invalid face; pad valid | refused, naming both circles |
| two square holes sharing a corner | one wire; the area is right (82); the pad's shape is invalid | refused: four curves meet at the point |
| a hole whose first point is on another hole's side | another hole: 88 | refused |
| a 100 × 100 square, circles r 40, 20, 10 about its centre | two faces, 10000 − 1600π and 300π; two solids | two regions (holes and islands to any depth); a pad refuses two (decision 3) |
| squares 100, 80, and 10, nested | 3600 and 100 | the same two regions |
| the square and circles listed inner first | the same faces | the same |
| a circle drawn twice | one wire of two edges; a face of twice the area, invalid; pad valid | refused: the circles overlap |
| a triangle over half a square, sharing three corners | one wire; a face of −50, invalid; pad valid | refused: points where three or four curves meet, and overlapping lines |
| two lines between the same two points | a closed wire with no area; pad valid, volume 0 | refused: the lines overlap |
| the same inside a 10 × 10 square | a hole with no area: 100 | refused |
| a point beside the rectangle | ignored | ignored |
| only a point | "Cannot make face from profile" | refused: no closed profile |
| an empty sketch | "Linked shape object is empty" (`FeatureSketchBased.cpp:269`) | refused: no closed profile |
| a construction diagonal across the rectangle | left out: 12 | left out |
| two separate squares | two faces; pad: two solids | two regions; a pad refuses them (decision 3) |

FreeCAD's result depends on which point comes first in a wire, on where a circle's seam is, and on the order of the sketch's geometry. Silico's depends on none of them.

## Bug fixes from FreeCAD's history

| Commit | What broke, and the fix | In Silico |
|---|---|---|
| `cc96c416c1` (2011-10-27) | PartDesign's face maker (Cheese's ancestor) took a wire as inside another from one vertex, with a TODO that crossing wires go unchecked. Bullseye still nests this way. | Moot: crossing and touching are refused first, so one point decides containment correctly. |
| `f86c65644c` (2013-07-09, #1133) | Sorting by diagonal could put an outer wire after its hole. Another sort was tried and the first kept. | Moot: nesting comes from containment, not size. |
| `89a5d06926` (2013-08-16, #1216) | Sketches with radiused corners didn't loft: an assembled wire wasn't ordered or connected. `ShapeFix_Wire` (reorder, connect, close) after assembly, `TopoShape::fix()` today. | Moot: the core sends each loop's curves in order, sharing points. |
| `02286a21d1` (2016-09-25) | Bullseye introduced: islands. | Followed. |
| `c833136cde`, `7b70711fb3` (2016-10-02) | A signed loop index over the wires; a null maker from an abstract type. | Moot. |
| `4c3ffb57ca` (2017-04-16, #3009) | A point in a sketch broke its shape (cast to an edge). | Points are left out of the profile. |
| `4dd16c58ed` (2020-03-03) | `AllowMultiFace`: a profile may give several faces. | M3.5 gives several regions; a pad refuses them (decision 3). |
| `488419cd73` (2020-07-30) | `BRepLib_FindSurface` found the wrong plane for transformed circular edges (an OCCT bug), so the wires are copied first. | Moot: the plane is the sketch's. |
| `9c17fe359b` (2020-08-13, #4117) | A wire OCCT couldn't make a test face from crashed the extrusion in `getWireDirection`. Now it throws. | Moot: the core finds each loop's direction from its signed area. |
| `6c956bb80f` (2020-11-07) | Non-construction points became vertices of the sketch's shape, for hole features. | Points stay out of the profile. |
| `4e5f0a3850` (2021-01-08) | A degenerate curve's null edge went into the shape. Now skipped. | The gate refuses degenerate geometry, and a solve that makes some is refused (`sketch::savable`). |
| `7786e24fcd` (2022-02-16, #6366) | PartDesign used Cheese, which has no islands. Bullseye made the default. | Nesting to any depth. |
| `e0ecd213b2` (2024-03-03), `84380b3a56` | "Wire is not closed." made translatable: users see it. | Refusals are data, worded by the core (M2.0). |
| `c8046b1c60` (2024-07-15, #15154) | Recompute crashed on a sketch with points (a vertex with no element map). | Moot. |
| `26ebefd651` (2024-08-24) | A sketch with a point failed with "Shape must be a wire, edge or compound". Vertices are now ignored. | The same: points are ignored. |
| `d2637ec881` (2024-10-15) | The shape was built from the stored geometry, which wasn't accurate enough to close wires. Now it's built from the solve. | The profile is built from the solve (`SketchSolution`), not from the file. |
| `07bce810ae` (2025-04-03), fixed by `46a11b6538` (2025-05-21, #21499) | A refactor added every edge twice, so every vertex occurred four times and passed as closed. Fixed seven weeks later, in no release. | Moot: each curve comes from the sketch once, by ID. |
| `2161725848` (2025-06-12) | FaceMakerRing ported, changing Bullseye: the tight box and a tie tolerance for the sort; a first vertex on a face's outer boundary went from a hole (1.0: `IN \|\| ON`) to a new face; one on a hole's boundary became another hole. In 1.1.3. | Refused: touching. |
| `88c80b3ed8` (2026-04-06, main only) | `getWireDirection` sped up; the first Bullseye tests. | The direction is the core's. |

## Where Silico differs on purpose

- **The core decides; the worker builds** (decision 2). The core finds loops, refusals, nesting, and direction from the solved coordinates. The worker gets the plane and the regions (each an outer loop and its holes, every loop its curves in order, already oriented) and builds faces without classifying anything. FreeCAD fits a plane, sorts by size, and classifies one vertex, all in OCCT.
- **Curves join only at shared points** (D2), never by distance. FreeCAD joins any ends within 1e-7, and a joined vertex then reaches further.
- **An open profile is refused**, naming each open end and its curve. SolidWorks offers a thin feature for an open contour; Silico has no thin features.
- **Near-closed ends are refused** with "share the point" (D2): two open ends within 1e-7, which FreeCAD would join.
- **Every profile point joins exactly two curves.** A point where three or more curves meet is refused, naming the point and its curves: a T, a shared line, a figure-8's waist, loops meeting at a corner. FreeCAD fails the first two and builds the others.
- **Crossing, overlapping, and touching curves are refused**, naming both curves, or the point and the curve where a point touches one. SolidWorks refuses to extrude a whole sketch whose contours intersect and points to Contour Select. Silico's version of Contour Select is decision 8's later step.
- **Zero-area loops are refused** (area ≤ 1e-7 × perimeter, `Profile::check`'s rule). FreeCAD pads them into a zero-volume "valid" solid.
- **Disjoint regions** (loops side by side, or an island in a hole) are made as separate regions. A pad refuses more than one, naming them (decision 3, M3.6). SolidWorks makes a multibody part, and FreeCAD several solids.
- **The plane is the sketch's** (D6), never fitted. So on the XZ plane Silico's faces face −Y, the sketch's normal, where FreeCAD's fitted plane (`BRepLib_FindSurface`) makes them face +Y.
- **The order is exact.** Regions and loops are ordered by a key with no tolerance, ties broken by ID (open question 2). FreeCAD sorts by squared diagonal with a tie tolerance, then by the sketch's geometry order.
- **Direction comes from the signed area**: outer loops counter-clockwise about the sketch's normal, holes clockwise.
- **No fix-up.** No ShapeFix on wires or faces. A face OCCT calls invalid is an `invalid_result`, not a warning.

## What the core computes

The input is the sketch and its `SketchSolution` (points by ID, circles' radii). Construction geometry is left out, and so is any point that no profile curve ends at. An arc's radius is its centre's distance to its start, as M3.3's translation takes it. Its end lies on that circle to the solve's convergence (about 1e-10), far inside 1e-7.

1. **The graph.** Lines run from one point to another, arcs from start to end counter-clockwise about their centre, and circles have no ends. Count each point's curve ends; a point that's only a centre counts 0.
   - No curves: refused, no closed profile.
   - Points with 1: open. Two of them within 1e-7 of each other are refused with "share the point", naming both. Otherwise each open end is named.
   - A point with 3 or more: refused, naming it and its curves.
   - Every point with 2: each connected group is one loop, and a circle is a loop by itself. Each loop is walked from its curve with the smallest ID.
2. **No two curves meet except at a shared point.** Every pair is tested. Two curves meet where they come within 1e-7: crossing, overlapping, touching, or a point of one on the other.
   - Two curves that share a point must not meet anywhere else. Their closeness near the shared point doesn't count: tangent neighbours stay within 1e-7 of each other for about √(2r × 1e-7) from it. So for neighbours, test the other intersections of their lines and circles, and each one's far end against the other curve, as `Profile::check` does for neighbouring sides. That catches a line crossing an arc it ends on, a line folding back along its neighbour, and two arcs on one circle overlapping.
   - Every test is closed-form with +, −, ×, ÷ and √, which IEEE rounds correctly, so the answers are the same on every platform:
     - segment–segment: the signs of orientations, and each end's distance to the other segment;
     - segment–circle: the foot of the centre on the line, the line's two intersections, and each end's distance;
     - circle–circle: the centres' distance against r1 + r2 and |r1 − r2|, and the two intersections.
   - A hit counts only where it lies on both curves. A point of an arc's circle is on the arc if it's counter-clockwise from the start and clockwise from the end, measured by cross products about the centre with no angles. That's only ambiguous within rounding of an arc's end, which is within 1e-7 of the other curve anyway.
   - At `MAX_SKETCH_GEOMETRY` (10,000), testing every pair takes about 5 × 10⁷ tests. A sweep over bounding boxes keeps it fast, and its order can't change the answer.
3. **Area and direction.** A loop's signed area is the shoelace sum over its chords plus each arc's segment, ±(r²/2)(θ − sin θ) by the way the loop runs along the arc. θ, the arc's sweep, comes from `libm`'s `atan2` and `sin`, the same on every platform. A loop is refused if its |area| ≤ 1e-7 × its perimeter. The area's sign gives the loop's direction.
4. **Nesting.** Once no two loops come within 1e-7, every pair of loops is either disjoint or nested, and one loop is inside another if any one of its points is.
   - Test one point of each loop (one of its points, or a circle's (cx + r, cy)) against every other loop by crossing number. Cast a ray to +x, split arcs at their circle's top and bottom so each piece is monotone in y, and apply the half-open rule at the ends.
   - The point is more than 1e-7 from the other loop, while rounding errors are about 1e-13 at a kilometre, so the answer can't flip. That's why the touching rule comes first.
   - A loop's depth is how many loops contain it. A loop at even depth is a region's outer loop. A loop at odd depth is a hole of the smallest even loop around it.
5. **Regions**, in an exact order. Each region has its outer loop counter-clockwise and its holes clockwise. Each loop lists its curves in walking order, and curves that share a point share a vertex. The worker makes one edge per curve and one vertex per point, and builds the face on the sketch's plane. It checks the face (`BRepCheck_Analyzer`) and reports what it built, and nothing else.

**The tolerance.** Decided (the maintainer's side, 2026-10-18): 1e-7 mm, inclusive, wherever M3.5 asks whether two things meet: near-closed ends, two curves within it of each other away from a point they share, and slivers (a loop's |area|, or a region's net area against its total perimeter, at most 1e-7 × the perimeter). It's `silico_protocol::LENGTH_TOLERANCE`, OCCT's `Precision::Confusion()`, FreeCAD's joining distance, and what `Profile::check` already uses. It moves nothing and joins nothing (`exactness-over-heuristics`). Every vertex, edge, and face the kernel builds carries that tolerance, so two curves within it are one place in the B-rep; refusing them is `check_length` refusing a 1e-8 box. The message states the gap as measured (`line a and line b are 5e-8 mm apart: …`), and says "cross" or "overlap" only where they do.

What OCCT does with closer geometry, measured on the oracle (FreeCAD 1.1.3, OCCT 7.9.3: `Part.Face([wires])`, which is `BRepBuilderAPI_MakeFace` and `Add` with no ShapeFix, then `isValid()`), is not "builds them as touching", as this note said before:
- A square hole 1e-9 to 1e-5 from the outer side: the face is **valid with exactly the right area**, and so is its prism. Exactly touching, crossing, or a hole's corner on the side are invalid (the last with area 108 for 92).
- A circular hole 1e-9 from the sides: invalid; at 5e-8, valid. OCCT's 2D intersector (`BRepCheck`'s 1e-10), not `Confusion()`, decides, and its answer flips inside the band.
- A 10 × w rectangle: the face is valid down to w = 1e-9; its prism from w = 1.01e-7.

So exact predicates would let near loops reach the kernel, which builds them and calls them valid, and then (below) the sweep comes back valid and wrong. And no predicate is exact for arcs: an arc's end lies on its circle only to the solve's convergence (about 1e-10), so even "the shared point's intersection" needs a tolerance.

**The profile and the request share their rules.** Whatever the kernel's faces request refuses (`silico_protocol::FaceRegions::check`) the profile refuses first, naming the sketch's elements, by the request's own code on the same numbers: each curve converted once and kept on its loop (`Loop::sent`, which `Profile::faces` sends as it is; every zero as +0), its size by `Curve::size_problem`, an arc's ends on its circle by `Curve::off_circle`, distances by `distance` (`libm`'s `hypot`), each loop's area and perimeter by `area_and_perimeter` about the loop's first point as sent, and slivers by `sliver`. Two computations of one rule disagreed at the boundary (`/break-it`: a strip whose area the core and the request summed about different first points, and corner arcs, chords, and lone curves within the tolerance).

**Known OCCT wrong results, for M3.6** (measured as above; OCCT calls each result valid):
- A pad fused onto a 10 × 10 × 10 base, its profile a hole a wall g from the outer side: for g ≤ 3e-7 the fuse is "valid" with volume 1000 and 6 faces, the whole pad silently dropped. Right from 4e-7.
- The pocket's cut of the same: "valid", with its volume off by about 30 × g, up to g = 3e-7. Right at 1e-6.

The profile refuses walls up to 1e-7 (and slivers up to about 2e-7 wide on a thin rectangle), so walls between those and about 3e-7 still reach them. Recommended for M3.6: refuse a pad whose fused result has the base's volume (an empty result, the cheap "is it anything" check FreeCAD's behavior keeps); otherwise only the fingerprint records it.

**An OCCT trap, for the worker:** `BRepBuilderAPI_MakeEdge` given a circle and an arc's parameter range of under 1e-9 rad builds the full circle: a 0.05 mm arc of radius 100 m became a "valid" face of 3.1e10 mm². Silico's worker builds each arc on exactly the parameter range the core sends (found by M3.5's faces work, branch `m3.5-faces` at `a12fcf4`, `crates/silico-kernel/tests/adversarial_faces.rs`).

## Tests

### FreeCAD's

FreeCAD has no test of a profile that fails.

| FreeCAD | Checks | Silico |
|---|---|---|
| `tests/src/Mod/Part/App/FaceMakerBullseye.cpp` (main, `88c80b3ed8`; 16 tests): `faceDrillerSimpleRectangle`, `faceDrillerReversedOuterWire` | a 4 × 3 rectangle either way round: 12 | M3.5: a sketch with the rectangle's lines running either way |
| `faceDrillerWithOneHole`, `faceDrillerReversedHoleWire`, `buildEssenceOuterWithHole` | the same with a hole r 0.5 at (2, 1.5): 12 − π/4 | M3.5 |
| `faceDrillerFlippedPlane`, `getWireDirectionFlippedPlane` | the same about a −Z normal | M3.5: the same sketch on each of D6's planes (XZ's normal is −Y), same area, the face's normal along the plane's |
| `faceDrillerMultipleHoles` | 10 × 6 with holes r 0.5, 1, 0.75: 60 − 1.8125π | M3.5 |
| `buildEssenceNestedIslands` | 10 × 10, circles r 4 and 2 about its centre: two faces, 100 − 12π in all | M3.5 (two regions) |
| `buildEssenceManyWires` | 100 × 100 with 25 holes r 1: 10000 − 25π | M3.5 |
| `getWireDirection{CCW,CW}{Rectangle,Circle}` | a 3 × 2 rectangle and a circle r 5, each way round: ±1 | M3.5: the core's direction from the signed area |
| `getWireDirectionTiltedPlane`, `buildEssenceTiltedPlane` | 5 × 3 on a plane tilted 45°: 15 | Wait for sketches on other planes |
| `tests/src/Mod/Part/App/TopoShapeExpansion.cpp` `makeElementFaceNull`, `makeElementFaceFromFace` | 3 × 2 with a hole r 1 at its centre: 6 − π | M3.5 refuses it: the hole touches both long sides |
| `makeElementFaceSimple`, `…Params`, `…OpenWire` (a closed rectangle despite the name), `…ClosedWire` | 6, and a circle r 1: π | M3.5, the areas; the rest checks `TopoShape` plumbing |
| `makeElementWiresCombinesAdjacent`, `…CombinesWires` | element-map counts of edges joined by distance | Not ported: Silico doesn't join by distance |
| `tests/src/Mod/Part/App/FeatureExtrusion.cpp` `testExecuteFaceMaker` | Part's extrusion with Cheese | Not ported |
| `src/Mod/PartDesign/PartDesignTests/TestTopologicalNamingProblem.py` `testPD_TNPSketchPadTouching` | four circles r 20 at (±20, ±20), each touching two others; its volume assertion is disabled ("TODO ENABLE THIS ASSERTION WHEN IT PASSES") | M3.5: refused, naming the four touching pairs |
| the same file, `testPD_TNPSketchPadMultipleSolids` (commented out) | four separate circles r 15: 9000π | M3.5: four regions; M3.6: the pad refused |
| the same file, `testPD_TNPSketchPadOverlapping` (commented out) | four overlapping circles r 25, its expected value "FIXME arbitrary guess" | M3.5: refused; with regions (decision 8) |
| `SketcherTests/TestSketcherSolver.py` `testSlotCase` | the slot plate's shape: 4 edges, then 9 with its inner set | M3.5: an oracle case. Measured: one face with two holes, 8607.71344 (the plate alone 15828.157343) |
| `TestPad.py` `testBoxCase`, `testSketchOnBasePlane` | the slot plate padded: 6 faces | M3.6 (docs/ports/sketcher.md); M3.5 has its one loop |
| main's `src/Mod/Part/parttests/TestFaceMakerUnifiedPlanar.py` (`7186d30f6e`), its nesting cases | a circle r 5 in a 40 × 40 square, 1600 − 25π; concentric circles r 10 and 5, 75π; nested squares 30, 20, 10, 600; circles r 25.8, 15.4, 6.4, 2.5, π(r1² − r2² + r3² − r4²) | M3.5, analytic: Bullseye gives the same |
| the same file, its other cases: shared edge, shared corner, T-junction, dangling chains, crossings, bow-tie, figure-8 | regions | M3.5 as refusals; the region areas with decision 8 |
| main's `SketcherTests/TestSketchInternalFaces.py` (45) | a sketch's regions | Decision 8 |
| main's `TestPartApp.py` `PartExtrusionTests` | 10 × 10 with a 6 × 6 hole (64), tapered | M3.5: the face, 64; tapers later |

### Silico's own

- Every row of *Edge cases*, in the core: the refusal, its code, every element named, and the total. And each accepted row's regions: count, depth, and direction.
- In the worker: each face's area, against the analytic value. The nesting rows can also be oracle cases, since FreeCAD gets them right. The other rows are FreeCAD's mistakes, so they stay as core tests (oracle cases are protected: the maintainer adds them).
- `/break-it`:
  - each tolerance boundary, a step either side: ends 1e-7 apart, a hole 1e-7 from its boundary, circles 1e-7 from tangent;
  - tangent arcs and lines at shared points, which must pass;
  - a line that meets its arc's circle again, beyond and within the arc;
  - two arcs of one circle meeting end to end, and overlapping;
  - nesting 50 deep, a thousand loops, and loops at the kilometre limit;
  - a property test on generated nested squares and circles, whose areas are known in closed form.

## Open questions for the design

1. **The tolerance:** 1e-7, inclusive, everywhere (recommended), or exact crossings and 1e-7 only for touching.
2. **The order of regions and of each loop's curves**, which sets the faces' and edges' places in the worker's history (names come from IDs, M2.7). FreeCAD's order (largest squared diagonal first, then the order the sketch lists geometry) would match the oracle's face numbers. But Silico's lists are sorted by random IDs, so their order means nothing to a user. Recommended: regions by their outer loop's smallest curve ID, and each loop starting at its smallest curve ID.
3. **How arcs reach the worker:** centre, start, and end, with the worker computing the angles with the shim's math, or angles the core computes with `libm`, as M3.3 does for the solver. Either way the arc's edge ends at its points' shared vertices.
4. **A request of its own, or part of the sweep's?** A face request would be cached and named like any step. Or M3.6 could send the regions with the extrusion. This decides the cache key and the history that naming reads.
5. **The error code:** `invalid_input`, naming the sketch and its elements as the gate does, or a new code. The list of codes is a protected contract.
6. **When a profile is checked:** only when a feature uses the sketch, as in FreeCAD and SolidWorks, or also as a `silico check` warning on any sketch.
7. **Pockets and revolves of several regions.** Decision 3 covers pads. A pocket's several regions can still leave one solid.
8. **Ellipses and B-splines** (D1's later kinds) need the same tests. Ellipses can be done in closed form; B-splines can't.

## Corrections to docs/ports/sketcher.md

- *The sketch feature*, "edges joined into wires greedily (the first edge that connects, at 1e-7)": an edge joins where one of its ends is strictly within 1e-7 of any vertex already in the wire, not only a free end. The joined vertex moves and its tolerance grows. The `ShapeFix` result is kept only if it's valid.
- *Profiles to faces*, step 4, "A wire sharing a vertex with a face's outer boundary is 'no hit' … one sharing a vertex with a hole is a hit": in a sketch, wires never share a vertex, because assembly would have joined them into one wire. What decides is whether the wire's **first vertex lies on** the outer boundary (a separate face) or on a hole's boundary (another hole). Loops that do share a point become one wire, which Bullseye takes as a single outline: a hole at a corner gives 110.5 where 89.5 is right. The hole case is `:244-251`.
- *Profiles to faces*, "an invalid face (a warning, after ShapeFix) or two faces that pad later fuses into their union": there's a third outcome, a face OCCT calls valid with the wrong area. A hole crossing the boundary gives 80 where the union is 110. Two overlapping rectangles give −0, and a zero-volume pad. The pad's status is `Valid` for OCCT-invalid shapes too.
- *Profiles to faces*, "sorts by an exact key (the diagonal, then the index)": nesting now comes from containment, so the sort only orders the regions (open question 2).
- *Tests to port*, FaceMakerBullseye.cpp's 16 tests, "M3.5, every one": the two tilted-plane tests wait for sketches on other planes, and the `getWireDirection` tests port as the core's direction. Also, `TopoShapeExpansion.cpp`'s `makeElementFace` hole (r 1 in a 3 × 2 rectangle) touches both long sides, so Silico refuses it.
