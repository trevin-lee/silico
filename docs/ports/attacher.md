# Sketches on planar faces (FreeCAD's attacher, `FlatFace`)

A sketch whose `plane` names a planar face of the model, instead of an origin plane, and where it then sits: its normal, its origin, and its x and y axes. This is the M4 plan's amendment A4, deferred from M4 and built as M5.9 (docs/roadmap/m5-app.md); the frame is decisions F1 and F2. This note covers what FreeCAD does, how 1.1.3 differs from main, the fixes in FreeCAD's history, where Silico departs from FreeCAD on purpose, and the tests to port. The face is found by its stable name, as an edge is (docs/ports/references.md). The sketch's own frame on the origin planes is M3.2's D6.

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

The list starts at 113: `Attacher.cpp:81` (it runs on past `Folding`, through `InertialCS`, `OZX` and the rest) and is declared at `Attacher.h:56-119` (`mmFlatFace` at :61). Main adds `MidPlane`, a plane between two faces (`edb4b684d7`, main: `Attacher.cpp:1666-1683`).

Only `FlatFace` on one face is in scope. FreeCAD's origin planes are `FlatFace` attachments too: a sketch on `XZ_Plane` is `FlatFace` on that datum. That's how Silico's `xy`, `xz`, and `yz` frames came about (D6). `AttachmentOffset`, an extra placement applied in the sketch's own frame after attaching (`plm *= this->attachmentOffset`, 113: `Attacher.cpp:2299`; the property is at 113: `AttachExtension.cpp:150-155`), is out of scope: offset planes are their own later step.

### FlatFace, step by step (113: `Attacher.cpp:1520-1582`, then `placementFactory`, 113:324-435)

1. **The face.** `readLinks` takes each reference's sub-shape in global coordinates (`extractSubShape`, 113:1006-1064). A name that finds nothing throws `AttachEngine3D: subshape not found <obj>.<sub>` (113:1033-1037). FlatFace's own `no subobjects specified (needed one planar face)` (113:1521-1526) can't be reached, read from code: `readLinks` sizes its shapes to the objects linked (113:948), and an empty support throws `ExceptionCancel` first (113:1335-1337), which leaves the sketch silently unattached.
2. **The plane.**
   - If the sub-shape is a face whose surface OCCT types as `GeomAbs_Plane`, its `gp_Pln` is used (113:1546-1548).
   - Otherwise the face's surface goes through `GeomLib_IsPlanarSurface(surface, precision)`. If that says planar, its fitted plane is used. If not, the attach throws `No planar face in AttachEngine3D::calculateAttachedPlacement()!` (113:1549-1559).
   - A sub-shape that isn't a face (an edge or a wire) goes to `TopoShape::findPlane` instead (113:1536-1541). That's out of scope.
3. **The tolerance.**
   - `precision` defaults to 1e-7 (113: `Attacher.h:426`). A sketch's engine is built with `AttachEnginePlane::planarPrecision()`, 2e-7 (113: `Part2DObject.cpp:64`, `Attacher.cpp:2379-2382`).
   - The GUI's New Sketch refuses a face that fails the same test (113: `src/Mod/PartDesign/Gui/SketchWorkflow.cpp:139`, through `TopoShape::isPlanar`, 113: `TopoShape.cpp:4475-4494`).
   - Unverified: a sketch whose attacher is recreated by `changeAttacherType` gets a fresh engine with the default 1e-7 (113: `AttachExtension.cpp:307-330`). A saved sketch's type already matches, so restoring doesn't take that path.
4. **What "planar within a tolerance" means** (OCCT, `GeomLib_IsPlanarSurface.cxx`):
   - **B-spline, Bézier, offset, and other surfaces** (:239-258) are sampled on a grid of (8 + 3·NbU) × (8 + 3·NbV) points over the surface's whole parameter range, not the face's trimmed part. The points' principal axes are found (`GeomLib::Inertia`). The surface is planar if the RMS spread along the least axis is under the tolerance, and the spread along the middle axis is over `Confusion` (:43-44; the spread is √ of the least eigenvalue, `GeomLib.cxx:2042`).
   - **Surfaces of revolution and of extrusion** (:160-237) have one iso-curve sampled against a trial plane.
   - **Cylinders, cones, spheres, and tori** are never planar.
   - So a flat B-spline face is accepted (`2611f072b7`). So is a slightly bent one, if its RMS spread is under 2e-7. And so is a bumpy one whose bumps fall between the samples. Its plane passes through the samples' barycentre, with its x near the surface's dU (:44-70).
   - **A bug, confirmed by a run of FreeCAD 1.1.3:** the face's location is fetched (`BRep_Tool::Surface(face, loc)`) and never applied to the fitted plane (113:1549-1553; main: `Attacher.cpp:111-118`). A flat B-spline face of an object placed by a location puts the sketch where the face would be with no placement: a flat B-spline face placed at z = 5 and tilted 30° gave its FlatFace sketch the identity placement, off the face (the port review's run). It matters to the FreeCAD importer, which must place such a sketch on the face, not where FreeCAD puts it.
5. **The normal** (113:1562-1577). The plane's axis is used, reversed when the face is `TopAbs_REVERSED`. If the plane's own axes are left-handed, they're made right-handed (`UReverse`) and the reversal toggles. The result is the face's outward normal, as the solid sees it.
6. **The origin** (113:1341-1343, 1579-1581). `refOrg` is the position of the support object's placement. That's the `Placement` of the object the reference names (`GeoFeature::getGlobalPlacement`, 113: `src/App/GeoFeature.cpp:315-324`, which delegates to `DocumentObject::getPlacementOf`, `src/App/DocumentObject.cpp:1620`; for a feature naming its own face it's its own `Placement`). `refOrg` is projected onto the face's plane, so the origin can lie off the face. FlatFace never sets an x direction (113:1349 leaves it zero).
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
  - The sketch keeps its last placement. `positionBySupport` sets the placement to the identity first (113: `AttachExtension.cpp:354`), then restores it and rethrows on `Base::Exception` or `Standard_Failure` (:387-394). An `ExceptionCancel` is caught and not rethrown (:382-386).
  - The sketch's recompute fails with the message, before its solve (113: `src/Mod/Sketcher/App/SketchObject.cpp:262-272`), and the end of the recompute prints it as an error (113: `src/App/Document.cpp:2792-2800`). The sketch is left `Invalid` and `Touched`, as any object whose recompute fails is (the general rule, tested on `App::FeatureTestAttribute` by `src/Mod/Test/Document.py` `testInvalidAttribute`, 113: 2619-2625).
  - **The pads that use the sketch aren't failed: they're skipped.** `Document::recompute` puts a failed object's whole in-list, its pads and every later feature, into a filter and doesn't recompute them (113: `Document.cpp:2718, 2737-2738`). They keep their last shape and get no error. The filter sets no touch bit, so they stay as touched as they were: a pad touched only through the sketch reads up to date. `isValid()` reads only the error bit (113: `src/App/DocumentObject.h:203-206`), so a skipped pad reads as valid. Read from code; no FreeCAD test asserts it.
- **A non-planar face** fails the same way, with "No planar face". `FlatFace` on a whole solid with no face named fails, and the object stays where it was (the C++ test below).
- **A deleted support object** doesn't fail. Deleting it clears the link (`removeObject` → `breakDependency` → `PropertyLinkSubList::breakLink`, 113: `Document.cpp:3317, 3365-3368`, `src/App/PropertyLinks.cpp:3222-3248, 3728-3744`). The change re-attaches at once, the engine has no objects left, and `ExceptionCancel` takes the silent path above. The sketch becomes free-standing at its last placement, with `MapMode` still `FlatFace` and no error. Read from code; `src/Mod/PartDesign/PartDesignTests/TestSketch.py` `testIssue17553` asserts `AttachmentSupport == []` once its plane is gone (113: 68-70). `object '<doc>#<name>' not found` (113: `Attacher.cpp:1106-1127`) needs stale names in the engine, which this path never leaves.
- **The support itself failed:** the sketch is in its in-list, so it's skipped too, at its old placement, with no error of its own.
- **At file load** the attach runs and every exception is swallowed (113: `AttachExtension.cpp:710-737`, `SketchObject.cpp:10855-10863`): a broken attachment isn't reported when a file opens.

### AttachExtension: when the attachment runs, and what it depends on

Read from code at 1.1.3. `AE` is `src/Mod/Part/App/AttachExtension.cpp`.

- **At every recompute, before the solve.** `SketchObject::execute` calls `Part2DObject::execute` first, which reaches `AttachExtension::extensionExecute` → `positionBySupport` (113: `SketchObject.cpp:262-272`, `Part2DObject.cpp:69-72`, `DocumentObject.cpp:178-196`, `AE:419-436`). External geometry, the solve, and the shape come after (`SketchObject.cpp:276, 289, 317`). `isTouched_Mapping()` always returns true (113: `AttachExtension.h:115-118`), so the attach runs whether or not the support changed.
- **At once on an edit** of the support, mode, reversal, or offset (113: `AE:438-460`, `6025ebd4be`), and again at the next recompute. An error there sets the sketch's error status and logs to the console, with no recompute entry (`AE:449-457`).
- **The dependency.** `AttachmentSupport` is a `PropertyLinkSubList` of global scope (113: `AE:120`), so the support is in the sketch's out-list (`PropertyLinks.cpp:3152-3174`; `testIssue17553` asserts `sketch.OutList == [plane]`, 113: 55). The support recomputes first, and forces the sketch to recompute after it (113: `Document.cpp:2742-2748`).
- **What's read.** `updateAttacherVals` gives the engine each reference's index name and mapped name (113: `AE:346, 782-796`, `Attacher.cpp:228-255`). The face is then taken from the support's current shape.
- **How the reference is stored.** The mapped name (`;…Face6`) and the index name (`Face6`, or `?Face6` once the mapped name is missing), both updated during the support's own recompute, before the sketch attaches (113: `PropertyLinks.cpp:416-561, 2751-2798`, `src/App/GeoFeature.cpp:226-259`).
- **Where FreeCAD rewrites it silently** (1.1.3):
  - (a) A mapped name found at a new index has the index written back: `Face8` becomes `Face9` (113: `PropertyLinks.cpp:519-561`).
  - (b) A missing face re-pointed to one sharing the old face's vertices, by geometry ("auto change element reference", :468-500).
  - (c) The attacher's TNP recovery written back into `AttachmentSupport` (113: `Attacher.cpp:1129-1199`, `AE:368-377`; above).
  - (d) At the support's first recompute after a file opens, a search by geometry when the element map's version changed (113: `GeoFeature.cpp:238-247`, `PropertyLinks.cpp:468-500, 528-536`). The pass at open (`src/App/Application.cpp:917`) searches only for missing elements, as (b) does.

  Main adds (e) `rescueDriftedElement`, a search with an adaptive tolerance (`d45ff368a7`, not in 1.1). Silico stores only the stable name and has no index to rewrite, so (a) has no counterpart. (b) to (e) are re-points, which Silico never makes: at most, the face they'd pick is a candidate.
- **PartDesign reads the support too:** a feature with no base copies the support's placement (above); with no `BaseFeature`, a profile feature's base is its sketch's support, if that's a `Part::Feature` (`ProfileBased::getBaseObject`, 113: `FeatureSketchBased.cpp:670-710`, at :692-699, a legacy path outside a body); and the up-to modes and revolve read the support face (`getSupportFace`, 113: `FeatureSketchBased.cpp:557-659`; its callers main: `FeatureExtrude.cpp:911-914`, `FeatureRevolved.cpp:714-718`). Silico never takes a feature's base from its sketch's face: a base is always explicit.

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

`git log -- src/Mod/Part/App/Attacher.cpp` has 110 commits. Most are refactors, other modes, or cleanups. The ones below bear on a sketch on a flat face. The M4 plan's list (amendment A6) is `b900ccc94b`, `42ecc24ec5`, `40592e6055`, `d7d6bcda37`, `2611f072b7`, `eb8629d077`, `b5b00f7aff`, and `e8f826d0ed`; the others are added here.

| Commit | What it did | In 1.1.3 | In Silico |
|---|---|---|---|
| `b900ccc94b` (2011-11) | Before it, only a face exactly normal to an axis had a fixed x (±X gives ±Y, ±Y gives ∓X, ±Z gives +X); any other face took OCCT's default, or +X within 0.1 rad of ±Y. It brought in today's table, in global axes, by the nearest axis (`Part2DObject.cpp`). | Yes | The default token's table (F1) |
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

**AttachExtension and Part2DObject.** `git log -- src/Mod/Part/App/AttachExtension.cpp src/Mod/Part/App/AttachExtension.h src/Mod/Part/App/Part2DObject.cpp` has 118 commits. 43 bear on a sketch on a face. Ten of them are in the table above (`b900ccc94b`, `a5c8113aaa`, `42ecc24ec5`, `40592e6055`, `4800957858`, `eebb7f7829`, `b5b00f7aff`, `244282f855`, `27f077c62b`, `a4cd98bf3c`). Of the rest, these matter to Silico, every one in 1.1.3 unless it says otherwise (checked by `git merge-base --is-ancestor <c> af2d42a4aa`, and later ones against the 1.1.3 file). Importer-only renames and legacy-file readers (`fa441a9623`, `7e1334b1f9`, `926b844409`, `a8ae56e06a`, `025210845f`, `061747f367`) go to docs/roadmap/freecad-import.md.

| Commit | What it did | In 1.1.3 | In Silico |
|---|---|---|---|
| `9dcfe40a73` (2011-11) | The origin is the support's origin projected onto the plane. Before it, an axis-aligned face took the plane's own `gp_Pln` location as the origin, and only an oblique one projected | Yes (`refOrg`, step 6) | The global origin projected (F2): FreeCAD's own fix of an origin that depended on how the plane was parametrized |
| `fad7696ad4` (2011-12) | `positionBySupport` reads the support itself, where it was handed the face; the reversed face's flip, from `120ca87015` (2011-10), kept | Yes, its messages since changed | The outward normal, step 5 |
| `043dc60403` (2012-12) | A missing face became a FreeCAD error, "Face in support shape doesn't exist!", not an OCC exception | Replaced by "subshape not found <obj>.<sub>", which names it | A missing face is `reference_broken`, naming it |
| `1ec99c5d32` (2013-04) | A deleted support threw "Sketch support has been deleted" | No: reverted by `31e4fd8994` | Kept as an error: the gate refuses a name whose maker is gone |
| `4280334751`, `fe2e490645`, `ba17aa2839` (2013) | The sub-name `"back"` on a base plane reversed the normal | No: `MapReversed` replaced it (`42ecc24ec5`) | `reversed` is a field, never a word inside the name |
| `31e4fd8994` (2015-05, "fix free-standing sketches") | A null support returns instead of throwing: a sketch whose support is deleted stands free, silently | Yes (the `ExceptionCancel` path) | **Not followed** |
| `d0d4d6bc07` (2016-11) | Ported to `AttachExtension`: attach at every execute; a failure on an edit sets the error status | Yes | Resolved at every rebuild |
| `c3eb3e7fc7`, `0a32b91b6b`, `9cfca05c90` (2017-04) | An attach error fails the sketch's recompute with the attacher's message | Yes (113: `AE:424-433`) | Failure as data, on the sketch |
| `507b39e3fc` (2016-08) | An attached sketch's `Placement` is read-only (carried into `AttachExtension` by `d0d4d6bc07`) | Yes (113: `AE:765-769`) | A face sketch stores no placement |
| `a16f5bf9aa`, `65155936a6` (2017-10, 2024-07) | Attach at file load, every exception swallowed | Yes (113: `AE:710-737`) | **Not followed:** opening a file rebuilds it and reports a broken face |
| `2b79025ad9` (2019-08) | `isAttacherActive`, an attach test that doesn't touch the object | Yes | Resolving a face is never an edit: a rebuild writes nothing |
| `105c3363b8` (2024-03) | TNP: the attacher's re-pointed names written back into `AttachmentSupport` | Yes (113: `AE:368-377`) | **Not followed:** never re-pointed |
| `84ae6b9b7b` (2024-05) | The placement set to the identity before attaching, and restored on failure | Yes (113: `AE:338-395`) | No stored placement to restore |
| `6025ebd4be` (2024-07) | The re-attach on an edit restored, with `updateAttacherVals` first: why a deleted support is silent | Yes (113: `AE:346, 443-459`) | — |
| `0b439270ab` (2026-05) | A `string_view` change inverted `setAttacher`'s test, so the type is set only when it already matches (main: `AE:290-295`) | No (113: `AE:289-294` is right) | A main-only bug, read from code, mostly masked by `27f077c62b`'s sync; not the base attacher. An upstream candidate |
| `d45ff368a7` (2026-09) | `rescueDriftedElement`: a missing element found again by a search with an adaptive tolerance | No | The importer should expect main's files to carry references re-pointed so |

## What Silico does

The frame is F1 and F2's.

- **Stored:** `"plane": {"face": "<name>", "x": "+y", "reversed": false}`. `x` is one of `+x`, `-x`, `+y`, `-y`, `+z`, `-z`, and is always written. The face is a name in its feature's element map, resolved at each rebuild (M2.8's lookup).
- **The normal n:** the face's outward normal, with the face's orientation applied, as in FreeCAD's step 5. M4.4's `describe` reports it.
- **x:** the stored axis a, projected onto the face: x = (a − n(n·a)) / |a − n(n·a)|. This is FreeCAD's `gp_Ax3` projection.
- **y:** n × x.
- **`reversed`:** flips n and x, and keeps y, as `MapReversed` does. The token is always the table's for the unreversed normal. So a reversed sketch's x points against its token's projection, as FreeCAD's does.
- **The origin:** o = n(n·p), for any point p on the face's plane. That's the global origin projected. It isn't stored and has no tolerance. The projection itself can't fail; what a frame lets a sketch reach can (*How a rebuild places it*).
- **The default token** is written once: when the sketch is made, and by the FreeCAD importer. It comes from FreeCAD's table (step 7) in global axes. `describe` reports it as `default_x` for each planar face.
- **Refusals:**
  - A stored axis with |a − n(n·a)| < 1e-12 is `reference_broken` on the sketch, parameter `plane.x`, with the working axes as candidates.
  - A face that's gone is `reference_broken` with candidates, never re-pointed.
  - A face OCCT doesn't type as a plane is `reference_broken`, naming it, with the planar faces as candidates.
  - A face of a later feature is a cycle, refused by the gate.

### How a rebuild places it (M5.9)

**The document.** `plane` is either an origin plane's word (`"xy"`, `"xz"`, `"yz"`, D6) or the face plane, `{"face": "<name>", "x": "+y", "reversed": false}`: those three keys, in that order, each always written (`reversed` too, `false` included, as an extent's is). The reader takes nothing else in it. `face` is a name in Silico's form (`Name::parse`), of a face.

**The gate** refuses, naming the sketch and `plane.face` or `plane.x`:
- a `face` that isn't a name, or names an edge or a vertex (FreeCAD's `TestAllModesBoundaries` tests that a whole solid isn't a plane; FreeCAD takes a planar edge or wire through `findPlane`, step 2, which Silico leaves out);
- a name with a maker that isn't a feature before the sketch: a cycle, as a fillet's edge of a later feature is (M2.8);
- an `x` that isn't one of the six tokens.

What the gate can't know without building is the rebuild's.

**Where the face is looked for.** In the solids live at the sketch's place in the timeline: those made by features before it, consumed by none before it (a suppressed feature's base passed through, `document::suppression`). Exactly the rule M4.1 section 5 gives `add_feature`. The name is read in each one's element map as a fillet's edge is in its base's (M2.8): a name that finds nothing, or finds an element by an unstable name (N3), is `reference_broken` on the sketch, parameter `plane.face`, with candidates (N4), never re-pointed. A face of a solid whose feature failed or was blocked blocks the sketch, as a fillet's base does.

**The frame**, from the face found, exactly as F1 and F2 say it:
- n is the face's normal as `describe` reports it: the surface's, reversed where the face is, so out of the solid (step 5).
- p is any point on the face's plane: F2's o = n(n·p) keeps only n·p, the plane's offset from the global origin, which is the same for every point of the plane. So the origin is a function of the plane alone, not of how OCCT parametrized it: where the plane's own location lies within it (`gp_Pln::Location()`, which depends on how the face was built) is projected away. That location is used as p only because it lies on the plane by definition, exactly as OCCT stores the plane. `describe`'s centroid lies on it only to rounding, being a rounded average of points on it. Agreement on the faces a test measures wouldn't show it agrees on every face, so exactness takes the location, at the cost of `describe` reporting it.
- o = n(n·p); x = (a − n(n·a)) / |a − n(n·a)| for the token's axis a, projected once more (x ← (x − n(n·x)) / |…|), since the first division leaves x·n off by about 1e-16 / |a − n(n·a)|; y = n × x, normalized; `reversed` negates n and x. Every component is written + 0, so a zero is +0 whatever sign `describe` gave it.
- The frame sent to the kernel is its origin, x, and y; its normal is x × y, the one normal the kernel's plane, the fake kernel, and every sweep use. x and y are unit by the sweeps' own rule (4ε, `UNIT_SLACK`) and square to 1e-14: at the largest reach, 1e6 mm, that puts a placed point within 1e-8 of where OCCT, which normalizes x and rebuilds y as n × x, puts an arc's end, inside its 1e-7. The origin's length is within √3·1e6, since o = n(n·p) can lie further out on an axis than the face does.
- A face OCCT doesn't type as a plane (FreeCAD's `GeomLib_IsPlanarSurface` path, not followed) is `reference_broken` on `plane.face`, saying the face isn't planar, with the live solids' planar faces as candidates: what the name finds is no longer something a sketch can sit on, so the reference is broken, loudly, as FreeCAD's "No planar face" is an error.
- |a − n(n·a)| < 1e-12 is `reference_broken` on `plane.x`, with the tokens that work as candidates.
- A profile that, placed on the frame, reaches past 1e6 mm on any axis, the model's reach (`MAX_LENGTH`), is refused, naming the sketch and `plane`: a frame up to √3·1e6 out and a sketch up to 1e6 across can place a point about 2.4e6 out, and without this the refusal would come later, from a sweep, naming a parameter the document doesn't have.

**In rounds.** A sweep's steps are planned from its sketch's frame: its direction is the normal, a revolve's axis is placed by the frame. And a face sketch's frame is known only once the solid holding its face is built and described. So a recompute plans in rounds:
1. Every face sketch whose frame isn't known yet waits, with every feature that needs it.
2. The rest is planned and built as now.
3. Each waiting sketch whose candidates are built and described has its frame resolved, or its refusal. Its candidates are the live solids at its place built from one of its face name's makers: only those can hold the name, so a slow or failed branch it doesn't come from never delays or blocks it. One whose candidate failed is blocked by that feature. The other live solids' faces are read, if built, only for `reference_broken`'s candidates.
4. The plan is made again with those frames, and steps already built are kept, by key, in memory.

It ends when a round resolves nothing new. A sketch still waiting then is blocked, naming the feature it waits on. A sketch on a face of a sketch on a face takes a round each, so the rounds are as many as face sketches nest. The model is published once, from the last round. Each round's plan is a function of the document and the frames before it, so the result doesn't depend on which thread finishes first.

**The key.** A faces step's line holds the frame, its origin and axes by their bits, and never the face's name: an upstream edit that moves the face changes the key, so a warm cache can't place the sketch where the face was (the roadmap's key test). Two sketches with the same profile on faces with the same frame share the step. The origin planes' lines are unchanged (`faces xy`), so no step's key moves. The cache's format goes from 8 to 9 with `describe`'s plane location, and the format is in every entry's key, so a cache made before rebuilds once. A sketch's solve's key holds `plane` as the file writes it, as it holds everything else the file says of the sketch, though the solve, in the sketch's own 2D, reads none of it.

**What else reads the face.**
- `timeline`'s `inputs` gain the feature that makes the face: the name's own feature (`k7f2qx3m` of `k7f2qx3m.box.Face6`).
- Suppressing a maker of the name suppresses the sketch with it (M5.1's dependents, which already list A4's face).
- `move_feature` and `remove_feature` treat the name as a fillet's edge: a move to before a maker, or a removal of one, is refused by the gate as a cycle or a broken reference.
- `sketch` reports the frame as resolved.
- `add_feature` and `set_field` take the face plane (M4.1 section 5). `add_feature` may leave out `plane.x`: the face is looked up as above in the model rebuilt up to the sketch's place, so the edit waits for that rebuild, and `x` is written as the face's `default_x`. A name that finds nothing is refused as `reference_broken`, with candidates, and the edit isn't made.

### Where Silico differs from FreeCAD on purpose

- **The x axis is stored, not derived.** FreeCAD works x out from the face's normal at every recompute (step 7). So when an edit tilts the face across a boundary of the table, the sketch turns, silently. Silico stores which axis x follows and projects it at each rebuild. On an unedited model whose body FreeCAD places by the identity (begun on XY, with a primitive, or by a boolean, as every oracle case is), the two agree. After a tilt, Silico's sketch turns continuously. Any rule worked out from the normal alone must jump somewhere.
- **How far FreeCAD jumps depends on the boundary,** which F1's "90° at 45°" understates. These are worked from the table:
  - **X and Y** (rotation about Z): no jump. Both sides give x = (−sin θ, cos θ, 0) for n = (cos θ, sin θ, 0).
  - **−Y and Z:** no jump. Both sides give +X.
  - **±X and Z:** 90°. From ±Y to (sin θ, 0, ∓cos θ).
  - **+Y and Z:** 180°. From −X to +X.
- **A face that turns until its stored axis is parallel to its normal is refused.** FreeCAD carries on: a box's +X face turned 90° about Z takes FreeCAD to x = −X smoothly. Silico's `+y` is then parallel to n and is `reference_broken`. The agent picks another token from the candidates.
- **Global axes, not the support's** (decided 2026-10-22, open point 1 below). FreeCAD's table runs in the support object's axes (`a5c8113aaa`, written for a `Part::Box` with a rotated `Placement`). In a PartDesign body the support's axes come from how features copy placements: `ProfileBased::positionByPrevious` and `DressUp::positionByBaseFeature` copy the base's `Placement`, while primitives and `PartDesign::Boolean` keep their own, the identity by default (113: `FeatureSketchBased.cpp:126-143`, main: `:149-163`; `FeatureDressUp.cpp`). So the default depends on which feature a sketch names, not on the face. Measured in FreeCAD 1.1.3, x on a pad's +X face:

  | Body | Feature whose face is sketched | FreeCAD's x |
  |---|---|---|
  | A pad on XZ | The pad | +Z |
  | A pad on XZ, then an `AdditiveBox` | The box's result | +Y |
  | A pad on XZ, then a `PartDesign::Boolean` | The boolean's result | +Y |
  | An `AdditiveBox`, then a pad on XZ | The pad | +Y |

  Silico features have no placement of their own, so the default is the table in global axes: a function of the face's normal alone, which `describe` reports. It's FreeCAD's on every body placed by the identity, and departs from it on bodies begun with a pad, pocket, revolve, or groove on `xz` or `yz` (the tables under *Worked frames*). The FreeCAD importer never takes the default: it derives the token from the sketch's actual `Placement`, and refuses, naming the sketch, one whose x is no global axis's projection. The oracle harness sets each face sketch's `AttachmentOffset` so FreeCAD's x is the document's token, so no case depends on the two defaults agreeing.
- **The origin is the global origin's projection.** FreeCAD's is the support's placement position projected. They agree whenever that position is zero: in PartDesign bodies built on the origin planes or on a base feature, and in Silico's oracle.
- **No tolerance for planarity.** FreeCAD accepts a face that's planar to 2e-7 RMS (step 4), and a flat B-spline face, both of which STEP imports carry. Silico refuses either for now (F1's residual). Faces Silico makes itself are expected to come out of OCCT typed as planes, but that's unverified for every feature.
- **Never re-pointed**, where FreeCAD's TNP recovery switches the reference when the placement is unchanged.
- **Failures as data:** `reference_broken` with candidates, for a face that's gone and for one that isn't planar alike. FreeCAD instead fails the sketch with a message and leaves it at its last placement.
- **Dependents blocked, never skipped.** FreeCAD skips a failed sketch's pads and every later feature, which keep their last shapes and read as valid. Silico's are blocked by the sketch, each saying so, and draw nothing of a stale build.
- **A deleted face's feature is refused, never stood free.** FreeCAD's sketch stands free at its last placement, silently (`31e4fd8994`). Silico's gate refuses a name whose maker isn't a feature before the sketch, so the removal is refused first (`remove_feature` lists the sketch as a user), or the document is.
- **A failed face feature blocks the sketch,** as FreeCAD's skip does, but said: the sketch's outcome names the feature it waits on.
- **Reported at open.** FreeCAD swallows attach errors at load. Silico rebuilds a document it opens, so `check` reports a broken face as it reports any other failure.
- **Not in this step:** `AttachmentOffset`, the other modes, `MidPlane`, external geometry, and faces planar within a tolerance.

## Open points (contradictions with A4 and F1)

1. **"An unedited model's placement equals FreeCAD's exactly" holds only when the support's axes are global.**
   - Because of `a5c8113aaa`, FreeCAD's table runs in the support's frame, and a PartDesign body begun with a pad of an `xz` or `yz` sketch carries that plane's rotation into every feature.
   - On a box-shaped pad of an `xz` sketch, FreeCAD's x differs from F1's default on 4 of 6 faces (table below). On a pad of a `yz` sketch, it differs on 5 of 6.
   - No FreeCAD test has a face sketch on such a body. Read from code (113: `FeatureSketchBased.cpp:126-143`, `Datums.cpp:269-270`, `Attacher.cpp:1341, 380-418`), and confirmed by runs of FreeCAD 1.1.3: all 12 faces match the tables below.
   - The options were (a) pick the default in FreeCAD's frame, by walking the face's feature's `base` chain to the first feature, or (b) keep global axes.
   - **Decided (b), 2026-10-22,** for the maintainer, who may veto. FreeCAD's frame is an artefact of how its features copy placements, and fragile: the same face gets another default one feature later (the table under *Where Silico differs on purpose*). (a) would give Silico a body frame it has nowhere else, a rule for every feature kind, and worse defaults: on an `xz` body FreeCAD's x points down on the −X face. F1's claim now reads "equals FreeCAD's on every body FreeCAD places by the identity", and two tests pin the line: on pads of an `xz` and a `yz` sketch, FreeCAD's x on all six faces equals the measured tables while Silico's `default_x` is the global table's; and on an XY-begun body, a primitive-begun one, and a fused pair, the two frames are equal on every planar face.
2. **The origin differs for primitives FreeCAD positions by `Placement`** (a `Part::Box` at (px, py, pz) puts the origin at the box's corner projected). This matters only to the importer, which must move the sketch's geometry by the in-plane part of that offset, rounded to the document's nanometres. Silico's oracle builds geometry already placed, so its cases agree.
3. **A4's tilt test has to cross the right boundary.** "A face tilted through 45° turns its sketch continuously, never a 90° snap" tells Silico from FreeCAD only across an X–Z or +Y–Z boundary. Across X–Y, FreeCAD doesn't snap either. Suggested: a box's top edge over its +X face chamfered with two distances (tilts between +X and +Z, a 90° snap in FreeCAD), and the edge over its +Y face (a 180° flip).
4. **Ties are decided on bits.** A tie is exact only if the normal's components are bitwise equal in magnitude. Across X–Y either winner gives the same frame, but the stored token differs, and so does what a later tilt does. Across X–Z and Y–Z, a 1-ulp difference turns the default 90° or 180°. On the X–Y boundary, OCCT's components are bitwise equal: on `testPartDesignTNPChamfer`'s Face8, |nx| = |ny| = 0x1.6a09e667f3bcdp-1, with nz = −5.9e-18 (the port review's run). Whether an X–Z or Y–Z chamfer comes out as clean is unverified. FreeCAD's own 45° test (`testPartDesignTNPChamfer`) sits on the X–Y boundary, where it doesn't matter. Silico should take the tie on the normal exactly as it gets it (exactness over heuristics), and pin both X–Y winners.
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
| TNP `testPartDesignTNPFillet` (1822-1913) | A box filleted on Edges 1, 5, 7; a sketch on the fillet's `Face2`, the top face (FreeCAD 1.1.3 with `Refine` on, as the test has it); a 2 × 5 rectangle pocketed 3. Then Edge1 dropped. Counts (12, 30, 20) and volumes 1000 − 3·f − 24 and 1000 − 2·f − 24, with f = 10 − 10π/4 (1900-1913) | Analytic: the face frame is n = +Z, token `+x`, o = (0, 0, 10), y = +Y (checked by the port review's run). The pocket's `Direction` tilts nothing: with `UseCustomVector` off, FreeCAD overwrites it to (0, 0, −1), so the pocket is along the normal and removes (4..6, 6..10, 7..10), 2 × 4 × 3 = 24; volume3 is 969.56194490192, as the test expects. The test's volumes port as they are. Oracle for the counts |
| TNP `testPartDesignTNPChamfer` (1726-1820) | The same on a chamfer: `Face8`, the 45° face; volumes 1000 − 15 − cut and 1000 − 10 − cut (1817-1820) | Analytic: the X–Y tie of the table above (x = (1, 1, 0)/√2, o = (4.5, −4.5, 0)). Pin both tokens' x. Oracle for the placement |
| TNP `testBaseFeatureAttachmentSupport` (2520-2629) | Two boxes fused, the second at (5, 5, 5); a sketch on `Face8` (y = 5, n = −Y), padded along −Y. Box001's width changed: counts (17, 42, 28), and the reference moves `Face8` → `Face9` (2622-2628) | Silico: a fuse of two boxes; the face sketch's token `+x`, o = (0, 5, 0), y = +Z; after the width change the name in the document is unchanged and resolves. Oracle for the counts |
| TNP `testPartDesignElementMapPocket` (824-857) | A 10 mm box; a 1 × 1 square at (1, 1) on `Face6`, pocketed 5: 995, (11, 24, 16) | Analytic 995, and the pocket's corners at (1..2, 1..2, 5..10). Oracle for the counts |
| TNP `_testPartDesignElementMapRevolution` (572-650) | A pad 3 × 2 × 3 at y 1..3; a sketch on its bottom `Face5` (n = −Z); a 1 × 2 rectangle at sketch y −3..−1 turned 180° about the sketch's y axis: 5π + 18 (608) | Analytic: on the bottom face y = −Y, so sketch y −3..−1 is global y 1..3. 5π + 18 holds only for that frame |
| TNP `testChangeSketch` (1171-1283), `testApplyFillet` (1285-1370) | A circle sketched on a pad's top, padded 10. The base sketch gains an inner loop (or a fillet). The body's box stays 0..31.37, 0..25.2, 0..20 | Analytic box. The circle has to stay on the face, at o = (0, 0, 10) with x = +X |
| TNP `testPadsOnBaseObject` (54-120) | Three pads stacked by face sketches; `AttachmentOffset` (0.5, 0, 0) on the middle one; each stays valid | Its essence, without the offset (out of scope): the middle sketch's square moved 0.5 along x; each pad still resolves. Assert each sketch's frame and each pad's volume, analytic, not health alone: FreeCAD's `isValid()` reads only the error bit, which a pad skipped behind a failed sketch passes. Its `assertFalse(Pad2.isValid())` branch (118) runs only without element maps, so never at 1.1.3 |
| `TestSketch.py` `testIssue17553` (34-70) | `sketch.OutList == [plane]` (55); once the plane is undone away, `AttachmentSupport == []` (68-70) | Its dependency: a face sketch's `timeline` `inputs` hold the face's feature, and it rebuilds after it. The rest isn't followed: the face's feature can't go from under the sketch |
| TNP `testRectanglewithArcChangeinGlobalCenter` (2661-2724), `…UpperRight` (2726-2908) | An arc cut into the base sketch under a face sketch; the face sketch stays valid | Its essence: the face sketch still resolves after the base profile's edit. Volumes by the oracle |
| `src/Mod/Sketcher/SketcherTests/TestSketcherSolver.py` `testConstructionToggleTNP` (672-731) | A sketch attached to a pad's `Face6` follows it to `Face9` when construction lines are toggled | The face is the sketch's `plane.face`; its name doesn't change, and it resolves (M3.6's essence becomes the real test) |
| `tests/src/Mod/Part/App/Attacher.cpp` `TestAllModesBoundaries` (187-207) | `FlatFace` on a whole box (no face named) leaves the attached box where it was | Silico: a `plane.face` naming a solid or an edge is refused by the gate, naming it |
| TNP `testPadChange_UpToFirst_to_Dimension` (2910-3085), `testPartDesignElementMapHole` (859-888), `testShapeBinder` (1372-1549), `testSubShapeBinder` (1551-1724) | Up-to-first, Hole, and binders | Later, with those features |
| `TestPlacementUpdate.py` (19-200), `TestSketch.py` (34-102), `TestDatum.py` `testXYDatumPlane` (71-80), `tests/src/Mod/Part/App/AttachExtension.cpp` `testPlanePlane` (39-56), `src/Mod/Part/parttests/regression_tests.py` `test_TangentMode3_Issue24254` (131-…) | GUI editing transforms; undo of a sketch's links; a datum plane on XY; no assertion; `TangentPlane` | Not ported: GUI, undo, datums, or another mode |

Not ported: TNP `testPartDesignElementPadSketch` (544-571), which A4 lists. It has no sketch on a face, so A4's list should drop it. The `testPD_TNPSketch{Rot,Pocket,Loft,Pipe}SketchMove` entries (2399-2411) are stubs, and the `FlatFace` uses in `TestPad.py`, `TestPocket.py`, and `TestDatum.py` attach to origin planes.

**Silico's own tests (analytic).**
- The six faces of a box at a non-zero position, each sketch's frame and origin as in the table, with `reversed` both ways. A point (u, v) is checked at o + u·x + v·y.
- The ties of the table: both winners on the X–Y boundary, and X over Z, Y over Z, X over all three.
- The tilt: the +X–Z and +Y–Z chamfers at 44° and 46°. Silico's frame is continuous. The oracle gives FreeCAD's snap, documented as the departure.
- The parallel refusal: `reference_broken` on `plane.x`, with candidates. A case at |…| = 1e-10 is accepted and pinned.
- A face that's gone: `reference_broken` with candidates. A cylinder's side and a flat B-spline face (a STEP face) are refused, naming the face. A later feature's face is refused as a cycle.
- `default_x` from `describe` equals the token written at creation.
- The document round trip: the fixture writes back byte for byte.
- Open point 1's departure pinned: pads of an `xz` and a `yz` sketch, FreeCAD's x on all six faces as measured (`xz`: −Z, +Z, +X, +X, +X, −X; `yz`: +Y, +Y, −Z, +Z, +Y, −Y, for normals −X, +X, −Y, +Y, −Z, +Z), and Silico's `default_x` the global table's (`-y`, `+y`, `+x`, `-x`, `+x`, `+x`).
- The oracle harness honours the token: each face sketch's `AttachmentOffset` turns FreeCAD's x onto the document's, so cases on `xz` and `yz` bodies may use any token.

**M5.9's key tests** (docs/roadmap/m5-app.md, docs/design/m5.0-app.md):
- A sketch on a box's top face places its profile at the face's height and in its frame: its pad's volume, bounds, and faces' positions by hand.
- An upstream edit that moves the face (the box's height) moves the sketch with it, with the cache warm: a key holding the name would hit the stale placement, so the test fails if the key does.
- An upstream edit that removes the face is `reference_broken` with candidates, and the sketch's pad is blocked.
- Each of a cube's six faces' `default_x` is F1's table, and `add_feature` with no `plane.x` writes it.
- A face sketch's frame is reached in its own round: a sketch on the top of a pad that's itself of a face sketch takes two, and gives the same model built cold, warm, and from the disk cache.
- The schema snapshot changes only by the face plane. A sketch on a box's top face has the box in its `timeline` `inputs`.
