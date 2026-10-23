# Sketches on planar faces (FreeCAD's attacher, `FlatFace`)

A sketch whose `plane` names a planar face of the model, instead of an origin plane, and where it then sits: its normal, its origin, and its x and y axes. This is M4's step "Sketches on faces" (the M4 plan's amendment A4; the frame is decisions F1 and F2). This note covers what FreeCAD does, how 1.1.3 differs from main, the fixes in FreeCAD's history, where Silico departs from FreeCAD on purpose, and the tests to port. The face is found by its stable name, as an edge is (docs/ports/references.md). The sketch's own frame on the origin planes is M3.2's D6.

**Sources.** The oracle is FreeCAD 1.1.3, tag `145529fe74`. That commit isn't in the local clone, so the files were read from GitHub at the tag. Main is the fork's `876271d660`. Release 1.1 branched from main at `af2d42a4aa`; `e8f826d0ed`, which bumps main to 1.2, is main's first commit not in 1.1. A commit is "in 1.1.3" here if GitHub's compare puts it behind the tag, or if its code is at the tag (a backport). In citations, `113:` is a line at 1.1.3 and `main:` a line on main. A bare `Attacher.cpp` is `src/Mod/Part/App/Attacher.cpp`, and `TNP` is `src/Mod/PartDesign/PartDesignTests/TestTopologicalNamingProblem.py`. OCCT lines are from 7.9.3, the version Silico pins. Nothing here was run in FreeCAD; where a claim comes from reading code, not from a test, it says so.

## What FreeCAD does

### The plane modes, for context

A sketch carries an attacher, `AttachEnginePlane`, which reuses the 3D engine's modes (113: `Attacher.cpp:2349-2377`). The plane modes are:
- `ObjectXY`, `ObjectXZ`, `ObjectYZ`: an object's own placement, or a conic's.
- `FlatFace`.
- `TangentPlane`.
- `NormalToEdge` and the Frenet modes.
- `Concentric` and `SectionOfRevolution`.
- `ThreePointsPlane`, `ThreePointsNormal`, `Folding`.
- `InertialCS`.
- The origin-and-axes modes `OZX`…`OYX`.
- `ParallelPlane`, `Midpoint`.

The list is at 113: `Attacher.cpp:81-97` and `Attacher.h:100-120`. Main adds `MidPlane`, a plane between two faces (`edb4b684d7`, main: `Attacher.cpp:1666-1683`).

Only `FlatFace` on one face is in scope. FreeCAD's origin planes are `FlatFace` attachments too: a sketch on `XZ_Plane` is `FlatFace` on that datum. That's how Silico's `xy`, `xz`, and `yz` frames came about (D6). `AttachmentOffset`, an extra placement applied in the sketch's own frame after attaching (`plm *= this->attachmentOffset`, 113: `Attacher.cpp:2299`; the property is at 113: `AttachExtension.cpp:150-155`), is out of scope: offset planes are their own later step.

### FlatFace, step by step (113: `Attacher.cpp:1520-1582`, then `placementFactory`, 113:324-435)

1. **The face.** `readLinks` takes each reference's sub-shape in global coordinates (`extractSubShape`, 113:1006-1064). A name that finds nothing throws `AttachEngine3D: subshape not found <obj>.<sub>` (113:1033-1037). A reference with no faces at all throws `no subobjects specified (needed one planar face)` (113:1521-1526).
2. **The plane.**
   - If the sub-shape is a face whose surface OCCT types as `GeomAbs_Plane`, its `gp_Pln` is used (113:1546-1548).
   - Otherwise the face's surface goes through `GeomLib_IsPlanarSurface(surface, precision)`. If that says planar, its fitted plane is used. If not, the attach throws `No planar face in AttachEngine3D::calculateAttachedPlacement()!` (113:1549-1559).
   - A sub-shape that isn't a face (an edge or a wire) goes to `TopoShape::findPlane` instead (113:1536-1541). That's out of scope.
3. **The tolerance.**
   - `precision` defaults to 1e-7 (113: `Attacher.h:426`). A sketch's engine is built with `AttachEnginePlane::planarPrecision()`, 2e-7 (113: `Part2DObject.cpp:64`, `Attacher.cpp:2379-2382`).
   - The GUI's New Sketch refuses a face that fails the same test (113: `src/Mod/PartDesign/Gui/SketchWorkflow.cpp:139`, through `TopoShape::isPlanar`, 113: `TopoShape.cpp:4475-4494`).
   - Unverified: a sketch whose attacher is recreated by `changeAttacherType` gets a fresh engine with the default 1e-7 (113: `AttachExtension.cpp:307-330`). A saved sketch's type already matches, so restoring doesn't take that path.
4. **What "planar within a tolerance" means** (OCCT, `GeomLib_IsPlanarSurface.cxx`):
   - **B-spline, Bézier, offset, and other surfaces** (:239-258) are sampled on a grid of (8 + 3·intervals)² points over the surface's whole parameter range, not the face's trimmed part. The points' principal axes are found (`GeomLib::Inertia`). The surface is planar if the RMS spread along the least axis is under the tolerance, and the spread along the middle axis is over `Confusion` (:43-44; the spread is √ of the least eigenvalue, `GeomLib.cxx:2042`).
   - **Surfaces of revolution and of extrusion** (:160-237) have one iso-curve sampled against a trial plane.
   - **Cylinders, cones, spheres, and tori** are never planar.
   - So a flat B-spline face is accepted (`2611f072b7`). So is a slightly bent one, if its RMS spread is under 2e-7. And so is a bumpy one whose bumps fall between the samples. Its plane passes through the samples' barycentre, with its x near the surface's dU (:44-70).
   - **A latent bug, found by reading only:** the face's location is fetched (`BRep_Tool::Surface(face, loc)`) and never applied to the fitted plane (113:1549-1553; main: `Attacher.cpp:111-118`). A flat B-spline face of an object placed by a location would put the sketch where the face would be with no placement.
5. **The normal** (113:1562-1577). The plane's axis is used, reversed when the face is `TopAbs_REVERSED`. If the plane's own axes are left-handed, they're made right-handed (`UReverse`) and the reversal toggles. The result is the face's outward normal, as the solid sees it.
6. **The origin** (113:1341-1343, 1579-1581). `refOrg` is the position of the support object's placement. That's the `Placement` of the object the reference names (`GeoFeature::getGlobalPlacement`, which for a feature naming its own face returns its own `Placement`, 113: `src/App/DocumentObject.cpp:1620-1633`). `refOrg` is projected onto the face's plane, so the origin can lie off the face. FlatFace never sets an x direction (113:1349 leaves it zero).
7. **The x axis** (`placementFactory`, 113:351-418; main:474-541, the code A4 cites as main:492-531).
   - A zero x turns on `makeYVertical` (113:351-353), and `FlatFace` asks for the legacy orientation (113:2296), so the legacy branch runs (113:369-418).
   - It takes the support object's own axes in global space, dX, dY, dZ, from that same placement's rotation (113:380-385). They are not the global axes.
   - It takes |n·dX|, |n·dY|, |n·dZ|, and `std::max_element` picks the largest (113:386-394). On a tie the first wins: X, then Y, then Z.
   - Nearest dX: x = dY if n·dX > 0, else −dY. Nearest dY: x = −dX if n·dY > 0, else +dX. Nearest dZ: x = dX, whatever the sign (113:396-418).
   - `gp_Ax3(origin, n, x)` projects that x onto the plane, x = n × (a × n), and sets y = n × x (OCCT `gp_Ax3.hxx:83-89`). It fails only for an a parallel to n within `gp::Resolution()`, which the table can't pick: the axis it picks is never the nearest one.
8. **`MapReversed`** (113:420-423). After the table, `ZReverse()` and `XReverse()`: n and x flip, y stays (`gp_Ax3.hxx:97,103`). The property's description is "Reverse Z-direction (flip sketch upside down)" (113: `AttachExtension.cpp:134-140`). The table always reads the unreversed normal.
9. **The placement** is the inverse of that frame's transform (113:425-434), then `AttachmentOffset` is applied (113:2299).

**Where the support's axes come from.**
- **Origin planes.** `XY_Plane` is the identity. `XZ_Plane` is a quarter turn about X: dX = +X, dY = +Z, dZ = −Y. `YZ_Plane` is a third of a turn about (1, 1, 1): dX = +Y, dY = +Z, dZ = +X (113: `src/App/Datums.cpp:268-270`). On each, dZ is the normal, so the table gives x = dX. That's exactly D6's frames.
- **PartDesign features.** A feature's `Placement` is copied from its base feature's. A feature with no base copies its sketch's support (113: `src/Mod/PartDesign/App/FeatureSketchBased.cpp:126-143`; dress-ups, main: `FeatureDressUp.cpp:81-85`). So every feature in a body whose first feature is a pad of an `XZ_Plane` sketch carries `XZ_Plane`'s rotation, and the table on its faces runs in that rotated frame.
- **Part primitives.** For a `Part::Box` placed by its `Placement`, both `refOrg` and the axes are the box's own.
- **Silico's oracle** builds primitives with their geometry placed and an identity placement (`crates/silico-oracle/src/oracle.py`, `MAKE`). In its cases, the axes are global and `refOrg` is the global origin, except on bodies begun on `xz` or `yz`.

### When the face is gone, or isn't planar

- **A missing element** (TNP's `?Face6` marker, with the old mapped name kept in the shadow): `AttachEngine::calculateAttachedPlacement` (113:1129-1199) first traces the old name's history (`getRelatedElements`). Failing that, it tries the old index name. It switches the reference to the new name only if the placement that name gives equals the current one, within `Confusion` and `Angular` (113:1171-1196). Otherwise the attach runs on the missing name and throws `subshape not found`.
- **A failed attach.**
  - The sketch keeps its last placement (`positionBySupport` restores it and rethrows, 113: `AttachExtension.cpp:382-394`).
  - The sketch's recompute fails with the message (113: `src/Mod/Sketcher/App/SketchObject.cpp:262-272`).
  - The pads that use the sketch fail with it.
- **A non-planar face** fails the same way, with "No planar face". `FlatFace` on a whole solid with no face named fails, and the object stays where it was (the C++ test below).
- **A deleted support object** throws `object '<doc>#<name>' not found` (113: `Attacher.cpp:1106-1127`).

### 1.1.3 against main

On the `FlatFace` path, nothing differs in behaviour. `placementFactory`, the legacy table, `MapReversed`, the 2e-7 tolerance, and the origin are line for line the same (113:324-435 against main:447-558).

| Where | 1.1.3 | Main | Matters here? |
|---|---|---|---|
| The plane and normal | Inline in the mode (113:1520-1577) | Moved into `getPlanarFaceInfo` (main:88-143), which also computes the face's centre of mass for `MidPlane` (`edb4b684d7`) | No: same steps, same tolerance; `FlatFace` ignores the centre |
| `MidPlane` | Absent | `edb4b684d7`, `ff0a18d976`, `2bb00c9186` | Out of scope |
| A reference "has placement" | When the sub-name is empty (113:986) | When its element name is empty (`ecf918a780`, main:1114-1115) | No: only modes that need a whole object read it |
| Reachable modes | A mode can be listed as both applicable and reachable | Not both (`acea7fa3c0`, main:672-675) | Suggestions only |
| Old files' `Support` | Becomes `AttachmentSupport` with `FlatFace` even when empty (113: `AttachExtension.cpp:497-512`) | Only when not empty (`a4cd98bf3c`, main: `AttachExtension.cpp:506-528`) | Only to an importer of very old files |
| `AttacherEngine` | Not kept in sync with `AttacherType` | Synced (`27f077c62b`) | No |
| `testApplyFillet` | `assertEqual` on the bounding box (113: TNP:1364-1369) | `assertAlmostEqual` (`ef69dcc3ab`) | No: Silico compares within a tolerance |
| `testPD_TNPSketchPadSketchSplit` | Pad volume 7400 after the split (113: TNP:2367) | A recompute, then 8533.33333333333 (`6d06b61c7e`, the sketcher keeping a fillet's corner through a split) | Silico follows 1.1.3; the split is a sketcher edit, not the attacher |
| `remapSupportShape` (`d7d6bcda37`) | An empty stub, never called (113: `FeatureSketchBased.cpp:1058-1065`; its callers went in `ecf7e51ab3`) | Removed (`1e982cae1f`) | No |

## FreeCAD's history

`git log -- src/Mod/Part/App/Attacher.cpp` has 110 commits. Most are refactors, other modes, or cleanups. The ones below bear on a sketch on a flat face. The first eight are the M4 plan's list.

| Commit | What it did | In 1.1.3 | In Silico |
|---|---|---|---|
| `b900ccc94b` (2011-11) | Before it, only a face exactly normal to an axis had a fixed x (±X gives ±Y, ±Y gives ∓X, ±Z gives +X); any other face took OCCT's default, or +X within 0.1 rad of Y. It brought in today's table, in global axes, by the nearest axis (`Part2DObject.cpp`). | Yes | The default token's table (F1) |
| `a5c8113aaa` (2011-12, "fix wrong positioning of Sketch on rotated Support") | Made the table use the support's own axes (`Place`'s rotation), not the global ones. That's what 1.1.3 does. | Yes | **Not followed:** F1 uses the global axes. See *Open points* |
| `d7d6bcda37` (2012-11, bug 871, "Changing size rotates other sketches") | A pad's resize renumbered its faces, and a sketch on `FaceN` moved to another face and turned. When remapping, it kept `FaceN` if the face there was still a parallel plane. | Stub only: TNP replaced it | Names, not numbers: the face is found by its name (M2.8), or is `reference_broken` |
| `42ecc24ec5` (2015-06) | The attacher replaced `Part2DObject::positionBySupport`. `FlatFace` kept the table, the support's axes, and the projected `refOrg`. `MapReversed` became `ZReverse` plus `XReverse`. | Yes | `reversed` flips n and x (F1) |
| `40592e6055` (2015-07) | Split out `placementFactory`. Other modes got "Y vertical"; only `FlatFace` kept the old orientation, by its flag `makeLegacyFlatFaceOrientation`, for compatibility. | Yes | The table is a legacy rule, kept only as the default |
| `4800957858` (2015-07) | A sketch's mode defaults to `Deactivated`. Only files of the old format are read as `FlatFace`. | Yes (113: `AttachExtension.cpp:497-512`) | For the importer |
| `2611f072b7` (2017-09) | Any planar surface, not only `GeomAbs_Plane`: `GeomLib_IsPlanarSurface`, in the attacher and in New Sketch. | Yes | **Not followed:** only a face OCCT types as a plane (F1's residual) |
| `eb8629d077` (2018-09, #2519) | For the origin-and-axes modes, `MapReversed` turned by `Rotation(Y, 180)`, which is 180 radians, not degrees. Fixed to π (113:2132-2134). | Yes | Not that path. Silico flips exactly, by negation, never by an angle |
| `eebb7f7829` (2025-05, issue 21242) | A sketch's planarity tolerance went from 1e-7 to 2e-7, in the attacher (`precision`) and in New Sketch. | Yes | No tolerance: a face is planar if OCCT types it a plane |
| `a9e99b1667` (2024-03) | TNP's recovery: a missing face is followed by history, and re-pointed only if the placement stays the same. | Yes (113:1129-1199) | **Not followed:** never re-pointed; `reference_broken` with candidates |
| `b5b00f7aff` (2025-11, #24254) | 1.1 changed `TangentPlane`'s orientation on datum planes. Files from 1.0 and earlier get their `AttachmentOffset`, and its expressions, rewritten on load when the normal is nearest X or Y. | Yes (113: `AttachExtension.cpp:520-708`; its expressions fixed by `244282f855`, backported) | Shows the cost of a derived orientation rule changing: old files need migrating. F1's stored token can't change under a file |
| `e8f826d0ed` (2025-11) | Main's version becomes 1.2: the branch point | No | Marks what 1.1.3 lacks |
| `edb4b684d7` (2026-07) | `MidPlane`, and the `getPlanarFaceInfo` refactor | No | Out of scope |
| `a4cd98bf3c` (2026-09) | An empty legacy `Support` no longer clears the attachment | No | For the importer |

The other commits were checked and don't bear on the frame: TangentPlane, intersections, Midpoint's normals (`46d1345ca2`, `3e356491e2`, the latter backported), infinite datum shapes (`38df1ae742`), and refactors.

## What Silico does

The frame is F1 and F2's.

- **Stored:** `"plane": {"face": "<name>", "x": "+y", "reversed": false}`. `x` is one of `+x`, `-x`, `+y`, `-y`, `+z`, `-z`, and is always written. The face is a name in its feature's element map, resolved at each rebuild (M2.8's lookup).
- **The normal n:** the face's outward normal, with the face's orientation applied, as in FreeCAD's step 5. M4.4's `describe` reports it.
- **x:** the stored axis a, projected onto the face: x = (a − n(n·a)) / |a − n(n·a)|. This is FreeCAD's `gp_Ax3` projection.
- **y:** n × x.
- **`reversed`:** flips n and x, and keeps y, as `MapReversed` does. The token is always the table's for the unreversed normal. So a reversed sketch's x points against its token's projection, as FreeCAD's does.
- **The origin:** o = n(n·p), for any point p on the face's plane. That's the global origin projected. It isn't stored, has no tolerance, and has no failure mode.
- **The default token** is written once: when the sketch is made, and by the FreeCAD importer. It comes from FreeCAD's table (step 7) in global axes. `describe` reports it as `default_x` for each planar face.
- **Refusals:**
  - A stored axis with |a − n(n·a)| < 1e-12 is `reference_broken` on the sketch, parameter `plane.x`, with the working axes as candidates.
  - A face that's gone is `reference_broken` with candidates, never re-pointed.
  - A face OCCT doesn't type as a plane is refused, naming it.
  - A face of a later feature is a cycle, refused by the gate.

### Where Silico differs from FreeCAD on purpose

- **The x axis is stored, not derived.** FreeCAD works x out from the face's normal at every recompute (step 7). So when an edit tilts the face across a boundary of the table, the sketch turns, silently. Silico stores which axis x follows and projects it at each rebuild. On an unedited model, in the cases below, the two agree. After a tilt, Silico's sketch turns continuously. Any rule worked out from the normal alone must jump somewhere.
- **How far FreeCAD jumps depends on the boundary,** which F1's "90° at 45°" understates. These are worked from the table:
  - **X and Y** (rotation about Z): no jump. Both sides give x = (−sin θ, cos θ, 0) for n = (cos θ, sin θ, 0).
  - **−Y and Z:** no jump. Both sides give +X.
  - **±X and Z:** 90°. From ±Y to (sin θ, 0, ∓cos θ).
  - **+Y and Z:** 180°. From −X to +X.
- **A face that turns until its stored axis is parallel to its normal is refused.** FreeCAD carries on: a box's +X face turned 90° about Z takes FreeCAD to x = −X smoothly. Silico's `+y` is then parallel to n and is `reference_broken`. The agent picks another token from the candidates.
- **Global axes, not the support's.** FreeCAD's table runs in the support object's axes (`a5c8113aaa`). Silico features have no placement of their own, so F1 uses the global axes.
- **The origin is the global origin's projection.** FreeCAD's is the support's placement position projected. They agree whenever that position is zero: in PartDesign bodies built on the origin planes or on a base feature, and in Silico's oracle.
- **No tolerance for planarity.** FreeCAD accepts a face that's planar to 2e-7 RMS (step 4), and a flat B-spline face, both of which STEP imports carry. Silico refuses either for now (F1's residual). Faces Silico makes itself are expected to come out of OCCT typed as planes, but that's unverified for every feature.
- **Never re-pointed**, where FreeCAD's TNP recovery switches the reference when the placement is unchanged.
- **Failures as data:** `reference_broken` with candidates, and the not-planar refusal naming the face. FreeCAD instead fails the sketch with a message and leaves it at its last placement.
- **Not in this step:** `AttachmentOffset`, the other modes, `MidPlane`, external geometry, and faces planar within a tolerance.

## Open points (contradictions with A4 and F1, for the maintainer)

1. **"An unedited model's placement equals FreeCAD's exactly" holds only when the support's axes are global.**
   - Because of `a5c8113aaa`, FreeCAD's table runs in the support's frame, and a PartDesign body begun with a pad of an `xz` or `yz` sketch carries that plane's rotation into every feature.
   - On a box-shaped pad of an `xz` sketch, FreeCAD's x differs from F1's default on 4 of 6 faces (table below). On a pad of a `yz` sketch, it differs on 5 of 6.
   - No FreeCAD test has a face sketch on such a body, so this comes from reading code (113: `FeatureSketchBased.cpp:126-143`, `Datums.cpp:269-270`, `Attacher.cpp:1341, 380-418`), not from a run.
   - **Options:**
     - (a) Pick the default in FreeCAD's frame. Walk the face's feature's `base` chain through pads, pockets, revolves, grooves, fillets, and chamfers to the first feature. A sketch on `xz` or `yz` gives that plane's axes. A primitive or a boolean gives the identity, as Part's do.
     - (b) Keep global axes. Say so in A4, and have the oracle cases on such bodies write FreeCAD's token explicitly.
   - Either way the token can say it: FreeCAD's x is then always ± a global axis.
2. **The origin differs for primitives FreeCAD positions by `Placement`** (a `Part::Box` at (px, py, pz) puts the origin at the box's corner projected). This matters only to the importer, which must move the sketch's geometry by the in-plane part of that offset, rounded to the document's nanometres. Silico's oracle builds geometry already placed, so its cases agree.
3. **A4's tilt test has to cross the right boundary.** "A face tilted through 45° turns its sketch continuously, never a 90° snap" tells Silico from FreeCAD only across an X–Z or +Y–Z boundary. Across X–Y, FreeCAD doesn't snap either. Suggested: a box's top edge over its +X face chamfered with two distances (tilts between +X and +Z, a 90° snap in FreeCAD), and the edge over its +Y face (a 180° flip).
4. **Ties are decided on bits.** A tie is exact only if the normal's components are bitwise equal in magnitude. Across X–Y either winner gives the same frame, but the stored token differs, and so does what a later tilt does. Across X–Z and Y–Z, a 1-ulp difference turns the default 90° or 180°. It's unverified whether OCCT gives bitwise-equal components for a symmetric chamfer. FreeCAD's own 45° test (`testPartDesignTNPChamfer`) sits on the X–Y boundary, where it doesn't matter. Silico should take the tie on the normal exactly as it gets it (exactness over heuristics), and pin both X–Y winners.
5. **Near the 1e-12 threshold the frame is well defined but sensitive.** x's error is about 1e-16 / |a − n(n·a)|: about 1e-4 rad just above the threshold, and 1e-8 at |…| = 1e-8. That's deterministic, but tied to the normal's last bits. A test should pin a case at 1e-10.
6. **F1's citation:** `Attacher.cpp:492-531` is main's numbering and stops short; the table ends at main:541. At 1.1.3 it's 369-418. The rule F1 ports is `b900ccc94b`'s, but in global axes, which is the rule as it was before `a5c8113aaa`.

## Worked frames (analytic, from the table)

A point (u, v) of the sketch lands at o + u·x + v·y.

**A box's six faces.** The box has corner p = (px, py, pz) and sizes L, W, H; its faces are numbered as OCCT's box numbers them.

| Face | n | Nearest axis, sign | Default `x` | x | y = n × x | Origin o |
|---|---|---|---|---|---|---|
| Face1, x = px | −X | X, cos −1 | `-y` | −Y | +Z | (px, 0, 0) |
| Face2, x = px + L | +X | X, cos +1 | `+y` | +Y | +Z | (px + L, 0, 0) |
| Face3, y = py | −Y | Y, cos −1 | `+x` | +X | +Z | (0, py, 0) |
| Face4, y = py + W | +Y | Y, cos +1 | `-x` | −X | +Z | (0, py + W, 0) |
| Face5, z = pz | −Z | Z | `+x` | +X | −Y | (0, 0, pz) |
| Face6, z = pz + H | +Z | Z | `+x` | +X | +Y | (0, 0, pz + H) |

Every side face has its y straight up. With `reversed`, n and x flip and y stays: Face6 reversed has n = −Z, x = −X, y = +Y.

**FreeCAD on a pad of an `xz` sketch** (support axes dX = +X, dY = +Z, dZ = −Y), against the table above, x only:

| n | −X | +X | −Y | +Y | −Z | +Z |
|---|---|---|---|---|---|---|
| FreeCAD | −Z | +Z | +X | +X | +X | −X |
| Silico's default | −Y | +Y | +X | −X | +X | +X |

On a pad of a `yz` sketch (dX = +Y, dY = +Z, dZ = +X), FreeCAD gives +Y, +Y, −Z, +Z, +Y, −Y for the same six normals (−X, +X, −Y, +Y, −Z, +Z). Silico's default differs on all but +X.

**The 45° ties.**

| n | Tie | Token | x | y | If the other axis won |
|---|---|---|---|---|---|
| (1, 1, 0)/√2 | X = Y | `+y` | (−1, 1, 0)/√2 | +Z | `-x`, the same x |
| (1, −1, 0)/√2, `testPartDesignTNPChamfer`'s Face8 | X = Y | `+y` | (1, 1, 0)/√2 | +Z | `+x`, the same x |
| (1, 0, 1)/√2 | X = Z | `+y` | +Y | (−1, 0, 1)/√2 | `+x`: x = (1, 0, −1)/√2, y = +Y, turned 90° |
| (0, 1, 1)/√2 | Y = Z | `-x` | −X | (0, −1, 1)/√2 | `+x`: x = +X, turned 180° |
| (0, −1, 1)/√2 | Y = Z | `+x` | +X | (0, 1, 1)/√2 | `+x`, the same |
| (1, 1, 1)/√3 | all three | `+y` | (−1, 2, −1)/√6 | (−1, 0, 1)/√2 | |

The origin is n(n·p) in each case. For Face8 of `testPartDesignTNPChamfer` (the box's corner at (10, 0) chamfered by 1), n·p = 9/√2, so o = (4.5, −4.5, 0).

**A tilted face's continuous turn (F1).** Take n(θ) = (cos θ, 0, sin θ), from Face2 (θ = 0) toward +Z. A chamfer of the edge between Face2 and Face6, with two distances, is a face at that angle; editing its distances changes θ. The token is `+y`, chosen at θ < 45°.

| θ | Silico x | Silico y | FreeCAD x | FreeCAD y |
|---|---|---|---|---|
| 30° | +Y | (−0.5, 0, 0.866) | +Y | (−0.5, 0, 0.866) |
| 44° | +Y | (−sin 44°, 0, cos 44°) | +Y | the same |
| 45° (tie, X wins) | +Y | (−1, 0, 1)/√2 | +Y | (−1, 0, 1)/√2 |
| 46° | +Y | (−sin 46°, 0, cos 46°) | (sin 46°, 0, −cos 46°) | +Y |
| 60° | +Y | (−0.866, 0, 0.5) | (0.866, 0, −0.5) | +Y |

Silico's y turns smoothly with the face. FreeCAD's sketch turns 90° about its normal between 45° and 46°: a hole sketched at (10, 0) moves from the face's +Y side to down its slope. Across the +Y–Z boundary (n = (0, cos θ, sin θ), token `-x`), Silico keeps x = −X, and FreeCAD's x goes from −X to +X: 180°.

**Parallel.** Face2 (token `+y`) of a profile edited so the face turns 90° about Z has n = +Y. Then |a − n(n·a)| = |cos 90°|, about 6e-17 in floating point, under 1e-12: `reference_broken` on `plane.x`. The candidates are the working tokens (`+x`, `-x`, `+z`, `-z`). At 89.99999999°, |…| ≈ 1.7e-10: accepted, with x ≈ −X.

## Tests to port

Line numbers are at 1.1.3. "Oracle" means FreeCAD 1.1.3, through the harness, gives the expected values: a sketch's placement and the result's measures. "Analytic" means a hand calculation does. Every case marks its FreeCAD feature `Refine = False` where counts are compared (docs/ports/sketch-features.md).

| FreeCAD test | What it asserts | Silico's port |
|---|---|---|
| TNP `testPD_TNPSketchPadSketchMove` (2046-2152) | A 40 × 20 pad on XY_Plane, 10 high; a sketch on its `Face6` with a 20 × 5 rectangle, padded 10. Its edges are moved by (−5, 0) and (0, −5). The face sketch's `AttachmentOffset` is the identity (2149), and its `Placement` is a pure translation by (0, 0, 10) (2152) | Analytic: the top face's frame is token `+x`, o = (0, 0, 10), before and after the edit, and the face's name still resolves. Volumes analytic (8000, then 8000 + 1000). Oracle: the sketch's placement |
| TNP `testPD_TNPSketchPadSketchDelete` (2154-2255) | A hexagon pad; a sketch on `Face8` (the top). A side is deleted and its ends joined. Placement (0, 0, 10), identity (2252, 2255) | Analytic, as above; oracle for the volume |
| TNP `testPD_TNPSketchPadSketchSplit` (2257-2373) | A side split in two and moved: pad 8000, then 7400 (2360, 2367); the face sketch valid, placement (0, 0, 10) (2368-2373) | Its essence: Silico's sketch has no split, so the edit is the same profile with that side as two lines, one moved in by 2. Analytic 7400 |
| TNP `testPartDesignTNPFillet` (1822-1913) | A box filleted on Edges 1, 5, 7; a sketch on the fillet's `Face2` (x = 10); a 2 × 5 rectangle pocketed 3 at 45°. Then Edge1 dropped. Counts (12, 30, 20) and volumes 1000 − 3·f − 24 and 1000 − 2·f − 24, with f = 10 − 10π/4 (1900-1913) | Analytic: the face frame is token `+y`, o = (10, 0, 0), y = +Z. The 24 holds only for that frame. The pocket's slanted direction needs a direction parameter Silico's pocket lacks, so it pockets along the normal, with a hand-computed volume. Oracle for the counts |
| TNP `testPartDesignTNPChamfer` (1726-1820) | The same on a chamfer: `Face8`, the 45° face; volumes 1000 − 15 − cut and 1000 − 10 − cut (1817-1820) | Analytic: the X–Y tie of the table above (x = (1, 1, 0)/√2, o = (4.5, −4.5, 0)). Pin both tokens' x. Oracle for the placement |
| TNP `testBaseFeatureAttachmentSupport` (2520-2629) | Two boxes fused, the second at (5, 5, 5); a sketch on `Face8` (y = 5, n = −Y), padded along −Y. Box001's width changed: counts (17, 42, 28), and the reference moves `Face8` → `Face9` (2622-2628) | Silico: a fuse of two boxes; the face sketch's token `+x`, o = (0, 5, 0), y = +Z; after the width change the name in the document is unchanged and resolves. Oracle for the counts |
| TNP `testPartDesignElementMapPocket` (824-857) | A 10 mm box; a 1 × 1 square at (1, 1) on `Face6`, pocketed 5: 995, (11, 24, 16) | Analytic 995, and the pocket's corners at (1..2, 1..2, 5..10). Oracle for the counts |
| TNP `_testPartDesignElementMapRevolution` (572-650) | A pad 3 × 2 × 3 at y 1..3; a sketch on its bottom `Face5` (n = −Z); a 1 × 2 rectangle at sketch y −3..−1 turned 180° about the sketch's y axis: 5π + 18 (608) | Analytic: on the bottom face y = −Y, so sketch y −3..−1 is global y 1..3. 5π + 18 holds only for that frame |
| TNP `testChangeSketch` (1171-1283), `testApplyFillet` (1285-1370) | A circle sketched on a pad's top, padded 10. The base sketch gains an inner loop (or a fillet). The body's box stays 0..31.37, 0..25.2, 0..20 | Analytic box. The circle has to stay on the face, at o = (0, 0, 10) with x = +X |
| TNP `testPadsOnBaseObject` (54-120) | Three pads stacked by face sketches; `AttachmentOffset` (0.5, 0, 0) on the middle one; each stays valid | Its essence, without the offset (out of scope): the middle sketch's square moved 0.5 along x; each pad still resolves and is valid. Volumes analytic |
| TNP `testRectanglewithArcChangeinGlobalCenter` (2661-2724), `…UpperRight` (2726-2908) | An arc cut into the base sketch under a face sketch; the face sketch stays valid | Its essence: the face sketch still resolves after the base profile's edit. Volumes by the oracle |
| `src/Mod/Sketcher/SketcherTests/TestSketcherSolver.py` `testConstructionToggleTNP` (672-731) | A sketch attached to a pad's `Face6` follows it to `Face9` when construction lines are toggled | The face is the sketch's `plane.face`; its name doesn't change, and it resolves (M3.6's essence becomes the real test) |
| `tests/src/Mod/Part/App/Attacher.cpp` `TestAllModesBoundaries` (187-207) | `FlatFace` on a whole box (no face named) leaves the attached box where it was | Silico: a `plane.face` naming a solid or an edge is refused by the gate, naming it |
| TNP `testPadChange_UpToFirst_to_Dimension` (2910-3085), `testPartDesignElementMapHole` (859-888), `testShapeBinder` (1372-1549), `testSubShapeBinder` (1551-1724) | Up-to-first, Hole, and binders | Later, with those features |
| `TestPlacementUpdate.py` (19-200), `TestSketch.py` (34-102), `TestDatum.py` `testXYDatumPlane` (71-80), `tests/src/Mod/Part/App/AttachExtension.cpp` `testPlanePlane` (39-56), `src/Mod/Part/parttests/regression_tests.py` `test_TangentMode3_Issue24254` (131-…) | GUI editing transforms; undo of a sketch's links; a datum plane on XY; no assertion; `TangentPlane` | Not ported: GUI, undo, datums, or another mode |

**Silico's own tests (analytic).**
- The six faces of a box at a non-zero position, each sketch's frame and origin as in the table, with `reversed` both ways. A point (u, v) is checked at o + u·x + v·y.
- The ties of the table: both winners on the X–Y boundary, and X over Z, Y over Z, X over all three.
- The tilt: the +X–Z and +Y–Z chamfers at 44° and 46°. Silico's frame is continuous. The oracle gives FreeCAD's snap, documented as the departure.
- The parallel refusal: `reference_broken` on `plane.x`, with candidates. A case at |…| = 1e-10 is accepted and pinned.
- A face that's gone: `reference_broken` with candidates. A cylinder's side and a flat B-spline face (a STEP face) are refused, naming the face. A later feature's face is refused as a cycle.
- `default_x` from `describe` equals the token written at creation.
- The document round trip: the fixture writes back byte for byte.
- If open point 1 goes with option (a): a pad of an `xz` sketch's six faces against the oracle.
