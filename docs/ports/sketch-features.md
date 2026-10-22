# Sketch-driven features: pad, pocket, revolve, groove

A sketch's profile swept into a solid, then fused with a base (pad, revolve) or cut from it (pocket, groove): milestone M3.6 (docs/roadmap/m3-sketcher.md). This note builds on docs/ports/sketcher.md (*Pad, pocket, revolve*), docs/ports/profile.md (the profile and its faces, M3.5), docs/ports/extrude-revolve.md (M1's kernel sweeps), and docs/ports/naming.md. The decisions are in docs/design/m3.6-sketch-features.md; S1 to S10 below are its sections.

Sources: FreeCAD **1.1.3** (tag `145529fe74`, read from a shallow clone of the tag, which the maintainer's clone doesn't have) and main at `876271d660` (2026-10-04) for history and for where main differs. Paths are relative to `src/Mod/PartDesign/App/` unless they say otherwise. Line numbers are 1.1.3's (`113:`) or main's (`main:`).

**1.1.3 is not main here.** In 1.1.3, Pad and Pocket already have `SideType` (*One side*, *Two sides*, *Symmetric*, from `a346c266e7`), and old files' `TwoLengths` and `Midplane` are migrated on load. Revolution and Groove still have the older `Type`, `Midplane`, and `TwoAngles`, with no `SideType` and no `FeatureRevolved.cpp`. Main adds, after 1.1.3: `SideType` and signed angles for revolves (`b2da06bfe0`), start offsets (`bcc3e296fa`, `06a4b1db99`), a *Common* operation for pads (`4a71de647d`), projected axes (`03cc6671e3`), and up-to-first and up-to-last revolves (`80664a0d30`). Silico follows 1.1.3.

## What FreeCAD does

### Pad and pocket (`FeatureExtrude::buildExtrusion`, 113:322–831)

**Modes.**
- Pad's `Type`: `Length`, `UpToLast`, `UpToFirst`, `UpToFace`, `?TwoLengths` (old files only), `UpToShape` (113: `FeaturePad.cpp:43`). It has no through all.
- Pocket's `Type`: `Length`, `ThroughAll`, `UpToFirst`, `UpToFace`, `?TwoLengths`, `UpToShape` (113: `FeaturePocket.cpp:44`).
- `SideType`: *One side*, *Two sides*, *Symmetric* (113: `FeatureExtrude.cpp:50`). A second side has its own `Type2`.
- Defaults: Length and Length2 10 for a pad, 5 for a pocket. Neither is constrained, so negative values are allowed. `Reversed` false. `AlongSketchNormal` true.
- Old files (`onDocumentRestored`, 113:962–1012): `TwoLengths` becomes *Two sides*, toggling `Reversed` and swapping the lengths (and their expressions) to keep the old face order (`49d7d77274`, `bdea174d13`, `e07d1b074b`); `Midplane` becomes *Symmetric*. A script that sets `Midplane` gets a deprecation warning (`67948d60a2`, `3208b6de5f`, `8b9048a784`).

**Direction** (`computeDirection`, 113:74).
- With no custom vector and no reference axis, the direction is the profile's normal. Pocket's `getProfileNormal()` returns the negated normal (113: `FeaturePocket.cpp:131`), which is the only reason a pocket goes into the material.
- A reference axis must not be parallel to the sketch plane ("Axis must not be parallel to the sketch plane", `501c3dc657`). A pocket takes its negation (`a8cb5ed419`).
- A custom direction is taken as given. With `AlongSketchNormal`, the lengths are divided by |dir · normal|, so a length is measured along the normal; below `Precision::Confusion()` the feature fails with "Creation failed because direction is orthogonal to sketch's normal vector" (113:454).
- `Reversed` reverses the final direction once (113:473). `Direction` shows the unreversed direction (`9283aa0f72` reverted `1da079b34b`'s write of the reversed one, after a forum poll).

**Lengths** (113:338–360). `L` is `Length` for *Length*, the through-all length for *ThroughAll*, and 0 for the up-to modes. `L2` is set only for *Two sides*. If `|L + L2| < Precision::Confusion()`, the feature fails: "Cannot create a pad with a total length of zero." (or "…a pocket…"). That check replaced 1.0's per-side "Length too small" and "Second length too small" (`a346c266e7`), which had replaced `6167e9f77a`'s "height of zero", and `b2520d489a` had already removed a per-side check that refused valid negative lengths. *Symmetric* with a length of 0 isn't caught.

**The prism.**
- The profile is `getTopoShapeVerifiedFace()`: FaceMakerBullseye for every new feature (docs/ports/profile.md).
- *Length* and *ThroughAll*, no taper: `makeElementPrism(sketch, L · dir)`, OCCT's `BRepPrimAPI_MakePrism`, op code `XTR` (113:951). An OCCT failure is "FeatureExtrusion: Length: Could not extrude the sketch!".
- *Symmetric* with a length and no taper (113:553): the profile is moved by −dir · L/2 and swept once by L. **One prism** (`c384a52fa5`, after `a346c266e7` had made it a prism and its mirror). With a taper, two half prisms; in the other modes, the first side's prism and its mirror.
- *Two sides*, both lengths, no taper (113:607–700): the profile is moved by +L · dir and swept by L + L2 along −dir, so it spans [−L2, L] along dir. **One prism** (`a01cab21cc`; `94750baa6b` extended it to a through-all or up-to first side). Otherwise two prisms, joined with `makeElementXor`.
- The up-to modes use `BRepFeat_MakePrism` (op code `PSM`); they're outside M3.6.

**Through all** (`getThroughAllLength`, 113: `FeatureSketchBased.cpp:925`): 2.02 × the diagonal of `BRepBndLib::Add` of the base and the profile, gap 0. The factor was 1.01 until `43de589732` doubled it, so a symmetric through all goes all the way. `BRepBndLib::Add` reads a triangulation when the shape has one, so the length depends on whether the shape was displayed; any length past the base gives the same cut. It's one-sided unless *Symmetric*. With no base it throws ("No base set, sketch support is not Part::Feature").

**With a base** (113:733–809).
- The prism is refined if `Refine` is on, retagged (`-getID()`), and fused with the base (Pad) or cut from it (Pocket): `makeElementBoolean`, op code `FUS` or `CUT`, base first.
- The boolean uses an automatic fuzzy value since `cf8ad66373` (2024): 10 × 1e-7 × the arguments' diagonal (`BooleanFuzzy`, `FCBRepAlgoAPI_BooleanOperation.cpp:73-85`), so 1e-6 × the diagonal. `FuzzyTolerance` (−1 for automatic) exposes it since `73f848a3d5`, after it caused regressions. Silico's booleans use none (docs/ports/booleans.md).
- An OCCT failure is "Fusion with base feature failed", for a pocket too. No solid in the result is "Resulting shape is not a solid".
- The result is refined again, then the single-solid rule applies: "Result has multiple solids: enable 'Allow Compound' in the active body." It's enforced only when the body's `AllowCompound` is false, which is the default no longer: `f8c47b55cb` (2024) added the property, and `406c3ba42c` (2025-09) made it true by default. `eb52dd4a9a` (2025) made the check count solids before `getSolid` keeps one, after `935bdf9a0f` had made it never fire.

**With no base.** A pad is its prism, several solids fused. **A pocket with no base is its own tool, a positive solid, with no error**: the error ("no material to be removed") went with the pre-TNP code in `ecf7e51ab3` (2024). A through-all pocket with no base still fails, through `getThroughAllLength`.

**Refine.** `Refine` comes from the user's `Mod/PartDesign/RefineModel`, true by default since `e490531510` (2024-07; 113: `FeatureRefine.cpp:70`). A refine failure only warns ("Refine failed: …") and keeps the unrefined shape. `70184ba59d` stores the unrefined shape so a change of `Refine` alone re-refines without rebuilding.

**Catch-alls.** An OCCT failure whose message is "TopoDS::Face" becomes "Could not create face from sketch.\nIntersecting sketch entities or multiple faces in a sketch are not allowed."; anything else passes its message on.

### Revolution and groove (113: `FeatureRevolution.cpp`, `FeatureGroove.cpp`)

**Modes.** Revolution: `Angle`, `UpToLast`, `UpToFirst`, `UpToFace`, `TwoAngles` (113:48). Groove: `Angle`, `ThroughAll`, `UpToFirst`, `UpToFace`, `TwoAngles` (113:45). `Angle` defaults to 360 and `Angle2` to 0, each constrained to [0, 360] (`4d80f3ec28`, `df5c75d6b0`; Groove forgets `Angle2`'s constraint). `Midplane` and `Reversed` are booleans.

**Checks** (113: Revolution 116–139; Groove alike, saying "groove"):
- `Angle` > 360: "Angle of revolution too large".
- The angle in radians below `Precision::Angular()` (1e-12) in *Angle* mode: "Angle of revolution too small" (`3694da3102` moved it from degrees against `Confusion` to radians against `Angular`; `c0de42335b` constrained the property).
- *TwoAngles* summing to under `Angular`: "Angles of revolution nullify each other". `generateRevolution` refuses a total under `Angular` again: "Cannot create a revolution with zero angle."
- *UpToFirst*: "Revolve up to first is not yet supported". *UpToLast* for a revolution: `df5c75d6b0` dropped its branch, so, by the code (not run), 1.1.3 throws "ProfileBased: Internal error: Unknown method for generateRevolution()".

**The axis** (`getAxis`, 113: `FeatureSketchBased.cpp:1203–1356`):
- The sketch's `V_Axis` (0, 1, 0), `H_Axis` (1, 0, 0), and `N_Axis` (0, 0, 1), placed by the sketch, and `AxisN`, the Nth (from 0) **construction line segment**, from its start to its end (`SketchObjectGeometry.cpp:86`). These return early, before the checks below, so a revolve about `N_Axis` isn't refused here.
- An origin axis (`App::Line`), a datum line, or an edge of a `Part::Feature` (a sketch's included): a line by its location and direction, a circle by its centre and axis; anything else is "Edge must be a straight line, circle or arc of circle".
- Those are checked: a revolve's axis must not be parallel to the sketch's normal, `IsParallel` at `Precision::Angular()`, both senses ("Axis must not be perpendicular to the sketch plane", `501c3dc657`; `fdf66b3e88` caught the antiparallel case).

**The axis mustn't cross the profile** (`checkLineCrossesFace`, 113: `FeatureSketchBased.cpp:955–1056`; called for each face, 113: Revolution 187–196): "Revolve axis intersects the sketch".
- Only each face's outer wire is tested, by `BRepExtrema_DistShapeShape`; farther than `Confusion` is no crossing.
- At each touching point on an edge, the curve is sampled at ±10 · `Confusion` in its parameter, and the signed distances to the plane through the axis and the face's normal compared: a crossing only if both exceed 5 · `Confusion` and differ in sign. At a vertex, its two edges are sampled ±2 · `Confusion` from it, against `Confusion`.
- So a profile may touch the axis at a point or run along it. A hole crossing the axis would mean the outer wire crosses it too.
- History: `8371982dfc` (2012) added it; `f9cff4c8a2`, `22cfda67de`, and `9cbb88941f` (2012) made it the sign-change test; `f712f5e4ac` (2016) looped it over faces.

**Building** (113: Revolution 379–437; Groove 338–396). The axis `gp_Ax1(pnt, dir)`, reversed if `Reversed`. *Midplane* turns the profile by −angle/2 first; *TwoAngles* by −Angle2 and sweeps Angle + Angle2 (ignoring `Midplane`); a groove's *ThroughAll* sweeps 2π. Then `makeElementRevolve(axis, total)`: OCCT's `BRepPrimAPI_MakeRevol`, op code `RVL`. A positive angle turns right-handed about the (possibly reversed) axis: a profile at +X about +Y turns toward −Z (main's `GrooveStartOffset` test confirms it: a 90° start moves the profile to Z < −15).

**With a base.** A revolution fuses (`base.makeElementFuse(result)`, base first; `FuseOrder` keeps 1.0's files feature-first, `8c399e1fd0`); a groove cuts. Both use the automatic fuzzy value. Refine, then the single-solid rule, as for pad. **With no base**, both keep the revolved solid: a groove with no base is a positive solid (`df5c75d6b0` removed the error).

### Names

Op codes (`src/Mod/Part/App/TopoShapeOpCode.h:54–101`): `FUS`, `CUT`, `CMN`, `XOR`, `FAC` (a face), `XTR` (extrude), `RFI` (refine), `RVL` (revolve), `MIR`, `SKT` (sketch), `PSM` (prism to a face). There's no op code for Pad or Pocket: a pad's elements say `XTR` and then `FUS`.

- **A sketch's elements** are named by each geometry's facade ID, not its place: an edge `g<id>`, a vertex `g<id>v<pos>`, through `makeElementWires(shapes, SKT)` (`src/Mod/Sketcher/App/SketchObject.cpp:393–495`). A 4-edge rectangle has 12 map entries, 8 in its reverse map (`testPartDesignElementMapSketch`). FreeCAD reuses a facade ID for a new curve whose ends match a deleted one (`geoHistoryLevel`).
- **The profile's face** is named after its outer wire's edges by `FaceMaker::postBuild` with `minElementNames = 1`: one edge, the first in history order, then `FAC`. So the face's name survives edits to its other edges.
- **The prism** (`makeShapeWithElementMap`, 113: `TopoShapeExpansion.cpp:1403–1985`): each side face generated from a profile edge (`Edge1;:G;XTR`); each side edge from a vertex (`Vertex1;:G;XTR`); the bottom face is the input face, kept, with the feature's tag; the top face, from the solid the profile face generates, is the first face parallel to the profile, `;:G0` (`checkForParallelOrCoplanar`, 113:1346; `;:G00` for a coplanar one); the top edges and vertices are named from above (`;:U;XTR`, `;:U2;XTR`). Main's `makeElementPrism` test lists them (`tests/src/Mod/Part/App/TopoShapeExpansion.cpp:2959–3004`, a subset check, so the top face isn't listed). `;:G0` was never written for months: `d43fe277a7` (2024-01) passed `parallelFace` by value, fixed by `8fff92618c` (2024-05).
- **A revolve** names alike with `RVL` (main's test at `:2912–2957`). A whole turn's faces from below can carry stale text (docs/ports/naming.md).
- **The boolean** names with `FUS` or `CUT`, base first; refine with `RFI`. Measured on the oracle (Fable's review, a 10 mm box with a 4 × 4 square sketched in its top face, padded or pocketed 5): a blind pocket's walls come back modified one from one though nothing trims them (`#16:n;:M;CUT`), its floor kept (`#24:1;:H`); the rims of a profile drawn in the base's face are the sweep's start edges, modified one from one (`#d:n;:M;FUS`, `#d:n;:M;CUT`), where a sweep through a face gives section edges; the box's top is `Face6;:M;FUS` or `;:M;CUT`, one source. A real 1.1.3 name, a pad to first: `;g815v1;SKT;:H976,V;:L#2;PSM;:H976:9,E;:L#8;PSM;:H976:9,F;:H-977,F` (`TestTopologicalNamingProblem.py:3082`).
- **A defect in 1.1.3's groove:** it passes `sketchshape.getShape()`, a bare `TopoDS_Shape`, to `generateRevolution` (113: Groove 227–229), so the profile's names are lost. `c01b12044a` (#23342) fixed it for the revolution only.
- Face numbers, from the tests: a pad of a 4-edge profile has its sides as `Face1`–`Face4`, its bottom `Face5`, its top `Face6`; a hexagon's top is `Face8`. That's the index a sketch attached to the top follows, and what `testConstructionToggleTNP` is about.

## What M3.6 builds

| FreeCAD 1.1.3 | Silico (docs/design/m3.6-sketch-features.md, S1) |
|---|---|
| Pad, *One side*, *Length* | `pad`, `{"type": "one_side", "length": L, "reversed": …}`, L > 0 |
| Pad or pocket, *Symmetric*, *Length* | `{"type": "symmetric", "length": L}`: one prism, from −L/2 |
| Pad or pocket, *Two sides*, *Length* and *Length* | `{"type": "two_sides", "length": L, "length2": L2}`: one prism, from −L2 to L along the direction |
| Pocket, *One side*, *ThroughAll* | `pocket`, `{"type": "through_all", "reversed": …}` |
| Revolution or groove, *Angle* | `revolve`, `groove`, `{"type": "one_side", "angle": A, "reversed": …}`, 0° < A ≤ 360° |
| Revolution or groove, *Angle* with *Midplane* | `{"type": "symmetric", "angle": A}` |
| The axis: `H_Axis`, `V_Axis`, `AxisN`, a sketch's edge | `axis`: `x_axis`, `y_axis`, or any line of the sketch |

Not in M3.6: the up-to modes, through all both ways, a second side of another mode, offsets, tapers, custom directions and reference axes for pads, `TwoAngles`, a groove's *ThroughAll* (360° says it), and main's start offsets and signed angles.

## Edge cases

"FreeCAD" is 1.1.3 by its code; "measured" means the oracle's FreeCAD (1.1.3, OCCT 7.9.3).

| Case | FreeCAD 1.1.3 | Silico |
|---|---|---|
| a length of 0 | refused, a total of zero | refused by the gate: a length above 0 |
| a negative length | extrudes the other way | refused by the gate; `reversed` says the other way (SolidWorks) |
| *Two sides* summing to 0 (10 and −10) | refused, a total of zero | can't be written: both lengths are above 0 |
| *Two sides* with `Reversed` | the same prism as the lengths swapped | no `reversed` in `two_sides`, so that spelling is gone (`two_sides` of L and L is still `symmetric` of 2L, harmlessly) |
| *Symmetric* with `Reversed` | the same prism, its faces in the other order | no `reversed` in `symmetric` |
| *Symmetric* with a length of 0 | not caught | refused by the gate |
| a pocket with no base | its tool, a positive solid, no error (`ecf7e51ab3`) | refused: `base` is required |
| a groove with no base | a positive solid (`df5c75d6b0`) | refused: `base` is required |
| a through-all pocket, the base on both sides of the sketch | cuts one side only | the same |
| a through-all pocket that misses its base | the base unchanged, no error | refused: removes nothing (S7) |
| a pocket that removes everything | "Resulting shape is not a solid" | refused: the result is empty (M2's boolean) |
| a pad wholly inside its base | the base unchanged, no error | refused: adds nothing (S7) |
| a pad reversed into its base from a sketch in its top face, or a pocket reversed out of it | measured (Fable's review): 7 faces at the base's volume, the top split along the outline, no error | refused by volume: adds (removes) nothing (S7); counts alone would pass it |
| a pad that doesn't touch its base | two solids in one compound (`AllowCompound` true) | refused: the result is 2 solids (S5) |
| a pocket that cuts its base in two | two solids in one compound | refused: the result is 2 solids (S5) |
| a pad of two regions onto a base that joins them | one solid | refused (decision 3, strict) |
| a pad of two separate regions | two solids | refused, naming both regions (decision 3) |
| a pocket of two separate regions | both cut | the same |
| a revolution of two separate regions | two solids | refused, naming both (decision 3) |
| a revolve angle of 0, or above 360 | refused ("too small", "too large") | refused by the gate |
| an angle that moves the profile's far point less than 1e-7 | built if above 1e-12 rad | refused, as M1's revolve |
| an angle short of 360° by so little the profile's far point stops within 1e-7 of where it started (359.999999999° on a profile 4 from the axis, a slit of 7e-11) | built: a valid open turn of 6 faces, its two ends within the tolerance of each other; a pocket across it is accepted, but `Shape.check(True)` calls the result self-intersecting (`BOPAlgo` SelfIntersect). FreeCAD checks only the angle, above `Precision::Angular()` (`3694da3102`, `c0de42335b`) and at most 360° (measured on 1.1.3) | refused, naming `extent.angle`, with the advice to use 360° for a whole turn: ends within the tolerance are one place, as on the near-0 side (`silico_protocol::end_at_start`, M1's revolve's rule too). Within 1e-12 rad of 360° is a whole turn to OCCT, and built as one |
| an axis perpendicular to the sketch | `N_Axis` isn't checked; an edge is refused | not expressible: every axis is a line of the sketch |
| an axis line whose solved ends coincide | OCCT throws on the null direction | refused in the core, `invalid_input`, parameter `axis` (S6) |
| an axis crossing the profile | "Revolve axis intersects the sketch", by sampling each face's outer wire | refused exactly, region by region, naming the crossing region's curves off the axis and the axis line (S6) |
| a groove whose regions lie on opposite sides of its axis, neither crossing it | built: each face is checked alone (measured: two rectangles x 15..25 and −25..−15, z 5..10, a quarter turn into a cylinder r 20, remove 1374.446786) | the same (S6), its volume pinned on the real kernel |
| the same groove turned past 180° (rectangles x 15..25, z 5..10 and x −25..−15, z 7..12, a whole turn into a cylinder r 20 by 20), whose regions' solids overlap | built right, measured: 21284.290228 (at 181° 22377.739, at 180° 22383.848; the second rectangle at x −18..−12, 270°: 21417.023; mirrored, 360°: 22383.848), one solid, valid. Its cut takes the revolution's compound of solids as separate tools: `TopoShape::makeElementBoolean` expands a compound tool (`TopoShapeExpansion.cpp` 5905–5940, `expandCompound` 122–134) before `SetArguments`/`SetTools` (5980–5999), with its automatic fuzzy value; the groove reaches it through `makeElementCut` (`FeatureGroove.cpp` 247), the pocket through `makeElementBoolean(Cut, {base, prism})` (`FeatureExtrude.cpp` 753) | the same: the kernel's cut takes a compound tool of several shapes as that many tools (no fuzzy value, as Silico's booleans), each case matching FreeCAD (oracle `groove_both_sides_*`); one compound tool of overlapping solids is cut wrongly by OCCT (2748.893572 removed for 3848.451) |
| an axis along a side of the profile, or touching a corner | allowed | allowed: within 1e-7 is on the axis |
| a side exactly 1e-7 from the axis | OCCT fails (docs/ports/extrude-revolve.md) | the same failure, passed on with M1's suggestion |
| a whole turn | the start face isn't in the result, no ends | the same; each seam named by history, `Edge(M c)`, as FreeCAD keeps the profile edge's name |
| a sketch on XZ | the normal is −Y: a pad goes toward −Y | the same (D6) |
| a profile wall 1e-7 < g ≤ 3e-7 wide, flush with a face of the base, padded onto it | measured: the pad dropped, the result the base alone, valid (*Known OCCT wrong results*) | the same on Silico's kernel (measured at 2e-7: the cube alone, exactly), refused: adds nothing (S7) |
| the same, pocketed | measured: valid, the volume off by about 20 × g without the fuzzy value | accepted, documented (S7) |

## Bug fixes from FreeCAD's history

Each confirmed against its diff.

| Commit | What broke, and the fix | In Silico |
|---|---|---|
| `8371982dfc` (2012-11), `f9cff4c8a2`, `22cfda67de`, `9cbb88941f` (2012-12), `f712f5e4ac` (2016), `83ce80f133` | A revolve about an axis through its profile made a self-intersecting solid. Refused, by sampling for a sign change; then for every face (`83ce80f133`, multi-face tools). | Refused exactly from the solved 2D geometry (decision 2): each region's outer loop on one side, touching allowed; regions may lie on either side, as FreeCAD checks each face. |
| `fdf66b3e88` (2017-08) | The perpendicular-axis check missed an antiparallel axis; `updateAxis` errors were thrown, not returned. | Moot: an axis is a line of the sketch, so it lies in its plane. |
| `d29bed15d1` (2017-08) | An axis perpendicular to the sketch threw in `suggestReversed`. | Moot. |
| `4cad62e315` (2020-09) | A custom direction's length factor wasn't applied to the second length. | Not in M3.6 (no custom directions); lengths go along the normal. |
| `f9d1538186`, `d819a62db2` (2020-11) | Negative second lengths allowed. | Lengths are above 0, as SolidWorks. |
| `43de589732` (2021-02) | Through all with *Midplane* didn't go all the way: 1.01 × the diagonal, doubled to 2.02. | 2.02 × the diagonal, of the tight box. |
| `c0de42335b`, `3694da3102` (2021-03) | A revolve angle of 0 or tiny reached OCCT; the check compared degrees with a length tolerance. Now radians against `Precision::Angular()`. | Above 0° by the gate; and refused if the profile's far point moves 1e-7 or less (M1). |
| `a8cb5ed419` (2021-11) | A pocket along a reference axis went the wrong way. | No reference axes for pockets in M3.6. |
| `1da079b34b` (2021-11), `9283aa0f72` | `Reversed` wrote the reversed direction into `Direction`, which confused users; reverted after a poll. | `reversed` is a stored choice; nothing is written back. |
| `501c3dc657` (2021-12) | A pad used the revolve's axis check. Each feature now forbids its own (a pad's axis not in the plane, a revolve's not normal to it), and edges give axes. | A revolve's axis is a line of the sketch. |
| `6167e9f77a` (2021-12, issue 4759) | A pad of length 0 reached OCCT. Refused; later replaced by the total-length check. | Lengths above 0. |
| `0e22ada9dd` (2022-07), `e5b2d8c874` (2024-10) | *TwoLengths* with *Midplane* centred on (L + L2)/2, which no one expected. Removed. | `symmetric` and `two_sides` are separate forms. |
| `b2520d489a` (2023-06) | Pad's and pocket's own "too small" checks refused valid negative lengths. | Moot: no negative lengths. |
| `d43fe277a7` (2024-01), `8fff92618c` (2024-05) | A refactor passed `parallelFace` by value, so an extrusion's top face lost `;:G0`, and fillets on a pad's top broke. | The ends are OCCT's own `LastShape` (S4), never found by plane. |
| `f8c47b55cb` (2024-05), `406c3ba42c` (2025-09) | `AllowCompound` added, then made the default: a body may hold several solids. | A sketch feature's result of more than one solid is refused (S5), and decision 3 refuses a pad of several regions; M2's booleans still accept a compound. |
| `ecf7e51ab3` (2024-05) | The TNP port removed the pre-TNP pocket code, and with it the "no material to be removed" error: a pocket with no base became its tool. | A pocket needs a base. |
| `12619b2c2d` (2024-05) | The multiple-solid checks lost in the TNP port restored. | As above. |
| `e490531510` (2024-07) | `RefineModel` true by default. | No refine yet; oracle cases set `Refine = False`. |
| `594010d9f0` (2024-09, #16690) | A pocket's up-to prism was built against the base. | Up-to modes wait. |
| `cf8ad66373` (2024-10, #17119), `73f848a3d5` (2026-02) | Booleans got an automatic fuzzy value, then a property to set it after regressions. | No fuzzy value, as M2's booleans. |
| `70184ba59d` (2024-12, #17008) | Refine decoupled from the build. | With refine. |
| `935bdf9a0f`, `eb52dd4a9a` (2025-01, issue 19002) | The single-solid check ran after `getSolid`, which keeps one solid, so it never fired. | As above. |
| `4d80f3ec28`, `df5c75d6b0` (2025-02) | Revolve angles constrained to [0, 360] so `TwoAngles` can take 0; "nullify each other" added; the groove's no-base error removed; *UpToLast* dropped. | Above 0° and at most 360°; a groove needs a base. |
| `a346c266e7` (2025-08, #21794) | Two-sided extrusion: `SideType`, `Type2`, the total-zero check, *TwoLengths* and *Midplane* deprecated. | `extent`'s forms. |
| `7d3dd2605f` (2025-08) | *Symmetric* didn't halve the length, and `AlongSketchNormal` was ignored. | `symmetric` starts at −L/2: one halving, exact. |
| `aef10a1bc8` (2025-09) | *TwoLengths* compared without its `?`, so it broke. | Moot. |
| `02479a1528` (2025-09) | *Symmetric* with a custom direction mirrored about the wrong plane. | Moot. |
| `c384a52fa5` (2025-09) | After `a346c266e7`, *Symmetric* was a prism and its mirror, fused: one prism again, moved back L/2. | One prism. |
| `a01cab21cc`, `94750baa6b` (2025-09) | *Two sides* was two prisms joined; one prism from +L back L + L2. | One prism, from −L2. |
| `c01b12044a` (2025-09, #23342) | A revolution lost its profile's names (a bare `TopoDS_Shape` passed). Fixed for revolution; groove still has it in 1.1.3. | Groove names alike. |
| `8c399e1fd0` (2025-12) | After TNP the fuse order became result first; base first restored (1.0 files kept feature-first, since order changes refine). | Base first (input 0). |
| `49d7d77274`, `bdea174d13`, `e07d1b074b` (2026-03, 2026-04) | Migrating *TwoLengths* kept the old face order by swapping lengths and toggling `Reversed`. | Moot: `two_sides` has no `reversed`. |

## Where Silico differs on purpose

- **Lengths and angles above 0, with `reversed`** (SolidWorks), never negative. `reversed` only where it changes the solid.
- **`reversed` only where it changes the solid**: `two_sides` and `symmetric` have none. (`two_sides` of L and L and `symmetric` of 2L remain two spellings of one prism.)
- **A pocket and a groove need a base.** FreeCAD makes their tools positive solids.
- **A pad or revolve of several regions is refused** (decision 3, even onto a base that would join them), **and any sketch feature whose result is more than one solid** (S5). FreeCAD makes several solids.
- **The profile is the core's** (M3.5): crossing, touching, and open profiles refused, naming their elements, where FreeCAD's face maker builds what it can.
- **The axis check is exact**, from the solved geometry, on every curve.
- **A feature that leaves its base unchanged is refused**, by its volume alone (S7). FreeCAD builds it.
- **No refine yet, and no fuzzy value**, as M2's booleans.
- **Names**: a sketch's elements by their IDs, as FreeCAD's by facade IDs; a profile's only region as `Face1`, its several regions by their outer loops' smallest curve IDs; every element of a sweep the feature's own (the start reported `modified`, so the sweep bites nothing), the ends from OCCT's `LastShape`, and a pocket's and groove's cut biting their own sweep as their base (S4).

## Known OCCT wrong results

docs/ports/profile.md measured two, near the profile's tolerance, through FreeCAD's PartDesign on OCCT 7.9.3, each valid to OCCT:

- a pad fused onto a 10 × 10 × 10 base, its profile with a wall g from the outer side: for g ≤ 3e-7, the fuse is the base alone (volume 1000, 6 faces), the pad silently dropped; right from 4e-7;
- the pocket of the same: its volume off by about 30 × g, up to g = 3e-7; right at 1e-6.

FreeCAD's PartDesign booleans use an automatic fuzzy value (1e-6 × the diagonal, about 1.7e-5 on that cube), and Silico's don't. So the same cases were measured again on FreeCAD 1.1.3's Part booleans (OCCT 7.9.3), once with no fuzzy value, as Silico's booleans run, and once with PartDesign's. The base is a 10 mm cube. The profile is a 5 mm prism with a square hole, the hole g from the outer side. g runs from 1e-7 to 1e-5. Each face and prism is valid, with the right area.

| Where the wall is | Fuzzy value | Pad | Pocket |
|---|---|---|---|
| The pad's outer square is the cube's top face, so the thin wall lies flush with the cube's side | none, as Silico's | for g ≤ 3e-7 the base alone (1000 mm³, 6 faces), valid; right (15 faces) from 4e-7 | off by 20 × g up to 3e-7; right to 1e-13 from 4e-7 |
| The same | 1.7e-5, as PartDesign's | the base alone at every g measured, up to 1e-5 | off by 20 × g at every g measured |
| The pad's outer square inset 2 mm, so no face of the wall is near a face of the cube | either | right at every g, to 5e-13 | right at every g, to 3e-13 |

So the wrong results need the thin wall to lie flush with a face of the base. Without the fuzzy value, the band is the 1e-7 < g ≤ 3e-7 that the earlier measurement found. PartDesign's fuzzy value widens it to at least 1e-5: FreeCAD drops a flush pad whose wall is up to about the fuzzy value thick. Silico's worker sets the same options as Part's booleans, non-destructive and no fuzzy value, except that it runs single-threaded where FreeCAD runs in parallel (`FCBRepAlgoAPI_BooleanOperation.cpp:43-44`). So Silico is expected to match the first row. Design S7's rule holds whatever the cause: a sweep feature whose result measures as its base is refused, and a volume error that leaves a valid solid is accepted and documented, as FreeCAD accepts both.

**Measured on Silico's own kernel** (`crates/silico-kernel/tests/sweep_faces.rs`, `a_thin_wall_flush_with_the_base_is_dropped_by_occt_and_the_pad_refused_as_adding_nothing`), as above: the flush profile made by the faces request, swept by the prism request from the cube's top up 5 (a start offset of 10), and fused with the cube by the boolean request. It matches the first row. At g = 2e-7 the fuse is valid and is the cube alone: volume exactly 1000, 6 faces, 12 edges, 8 vertices, the pad dropped; the no-op rule (`silico_core::sweep::check_result`) refuses it as adding nothing. At g = 4e-7 it's right: 1000 + 5 × (100 − (5 − g) × 6) to 1e-12, 15 faces, and accepted. Across the band (`the_thin_wall_band_is_freecads_without_the_fuzzy_value`): the pad is the cube alone at g = 1.5e-7, 2e-7, and 3e-7, and the pocket valid with 11 faces and its volume 20 × g too large (to 1e-3 of that); both right to 1e-12 at 4e-7, 1e-6, and 1e-5.

## Refine

FreeCAD refines every PartDesign feature's result by default since `e490531510` (from the user's `RefineModel` preference), merging faces that end up coplanar or on one cylinder, and its face and edge counts depend on it (`TestRevolve.py` accepts 8 or 10 faces for that reason). Silico doesn't refine yet: refine is its own step, with its own port note, as for M2's booleans (docs/ports/booleans.md). So every M3.6 oracle case sets `Refine = False` on each FreeCAD feature, explicitly, as `oracle.py` does for chamfers, and compares counts unrefined. A test whose counts are the same either way (a pad with no base, a pocket into a box's top) says so; one whose counts differ (a pad flush with its base's side, which leaves that side in two faces unrefined) compares with FreeCAD unrefined.

## Tests

### FreeCAD's

| FreeCAD | What | Silico |
|---|---|---|
| `TestPocket.py` `testPocketDimensionCase` | A pad of a 10 × 10 square, length 1, reversed (z from −1 to 0); a pocket of a 5 × 5 square at (2.5, 2.5), length 1: **75** | M3.6, oracle case: `pad` `one_side` reversed, `pocket` `one_side` |
| `TestPocket.py` `testPocketThroughAllCase` | The same, then a through-all pocket on XZ of a 5 × 0.5 rectangle at (2.5, −0.75), toward +Y (the negated −Y normal): it removes 5 × 0.5 × (10 − 5) beyond the first pocket, **62.5** | M3.6, oracle case: `through_all`, not reversed |
| `TestPad.py` `testPadTwoDimensionsCase` | A unit cube padded from a square at (0, 1); a pad on XZ of a unit square at the origin, *Two sides*, 1 and 2, reversed: y from −2 to 1, touching the cube, **4** | M3.6, oracle case: `two_sides` with `length` 2 (along −Y) and `length2` 1, the same prism without `reversed` |
| `tests/src/Mod/PartDesign/App/Pad.cpp` `TestMidPlaneTwoLength` | A circle r 10 on XY, *Two sides*, 10 and 20: its bounding box, x and y ±10, z from −20 to 10. The volume, **3000π**, isn't asserted | M3.6, oracle case: the box and the volume |
| `TestPad.py` `testBoxCase`, `testSketchOnBasePlane` | The slot plate (`CreateSlotPlateSet`: a bottom 200 long from the origin, a side at 0.872665 rad to it, a top, and a half circle r 40 tangent to both) padded 10: **6 faces** | M3.6, oracle case with the side at 50°: 6 faces, and the volume 10 × (16000 − 3200 · cot 50° + 800π) = 158281.553 (by hand: a trapezoid 80 high with sides 200 and 200 − 80 · cot 50°, and a half disc). FreeCAD's 0.872665 rad gives the plate's measured 15828.157343 (docs/ports/profile.md), which the same formula reproduces |
| `TestPad.py` `testSketchOnDatumPlane` | The same on a datum plane | With sketches on other planes |
| main's `tests/src/Mod/PartDesign/App/Revolution.cpp` (`b2da06bfe0`, main only) | A circle r 10 at (30, 0) on XY turned about the Y axis: 2π² · 30 · 10² · θ/360; a groove base, a circle r 45 padded symmetric 100 (202500π) | M3.6, oracle cases in 1.1.3's modes: `TwoSidesAngles`' control (90°), `SymmetricAngle` (180°, min z = −max z), `GrooveTwoSidesThroughAll`'s one-sided control (202500π − 6000π²); the signed-angle, start-offset, and projected-axis tests are main's |
| `TestTopologicalNamingProblem.py` `testPartDesignElementMapPad` | A unit square padded 1: faces, edges, vertices (6, 12, 8) | M3.6: 26 names, one per element (FreeCAD's 30 holds 4 duplicated vertices) |
| `testPartDesignElementMapPocket` | A 10 mm box, a unit square pocketed 5 deep from its top: **995**, (11, 24, 16) with refine | M3.6: a box from z = −10 and a sketch on XY, pocketed 5; the counts are the same unrefined (no coplanar faces to merge) |
| `testPD_TNPSketchPadMultipleSolids` (commented out) | Four separate circles r 15 padded: 9000π | M3.6: refused, naming four regions (docs/ports/profile.md) |
| `SketcherTests/TestSketcherSolver.py` `testConstructionToggleTNP` (bug 15484, `4aaf72dcc2`) | A 30 × 30 square with four construction lines in a zigzag beside its left side; padded 10; a sketch attached to the top, `Face6`. The zigzag made profile and the left side construction: the pad has 9 faces, and the attachment follows to `Face9` | M3.6, its essence (design S9): a chamfer on a top edge and the top face by name, through the same toggles; the area grows by the zigzag's 187.5, so the pad is 10875 |
| `TestPad.py` `testPadToFirstCase`, `testPadtoLastCase`, `testPadToFaceCase`, `testPadToShapeCase`, `testPadToPlaneCustomDir`, `testPadToConcaveCase`; `TestPocket.py` `testPocketToFirstCase`, `testPocketToFaceCase` | Up-to modes (2, 3, 2, 2.58787, 1.5, 2208.0963; 62.5, 50) | With the up-to modes, after M3.6 |
| `TestRevolve.py` `testRevolveFace`, `testGrooveFace`; `testPartDesignElementMapGroove` | A box's face turned about the box's axes or edges; face counts depend on refine | Not ported: they revolve a solid's face, not a sketch |
| `_testPartDesignElementMapRevolution`, `testPadsOnBaseObject`, `testPD_TNPSketchPadSketchMove`, `…SketchDelete`, `…SketchSplit`, `testBaseFeatureAttachmentSupport`, `testPadChange_UpToFirst_to_Dimension`, `testRectanglewithArcChange*`, `testChangeSketch`, `testApplyFillet` | Sketches attached to a pad's faces, followed through edits | With sketches on faces (FreeCAD's attacher), which no milestone plans yet |
| main's `TestPad.py` start-offset tests, `testSketchAttachmentDoesNotImportExternalBody` | Main's features | Not ported |

FreeCAD tests no refusal of these features' inputs, no axis crossing, and no element name of a pad beyond counts.

### Silico's own

Design S9 lists them: every edge case above as a core test (the fake kernel) or a kernel test, each form on each of D6's planes against analytic volumes, the names of S4's tables, the TNP essence, break-it's near-tolerance profiles as pads and pockets, S7's residuals pinned, and determinism.

## Corrections to docs/ports/sketcher.md

Made there, with this note.

- *Pad, pocket, revolve*, "`Reversed` flips the final direction once (`1da079b34b`)": it does, but `1da079b34b` was about writing the reversed direction into `Direction`, which `9283aa0f72` reverted.
- *Pad, pocket, revolve*, "A revolution's `UpToLast` is its `ThroughAll` (`FeatureRevolution.h:86-94`): a full 2π": in 1.1.3's code, `df5c75d6b0` dropped that branch, and *UpToLast* throws "Unknown method for generateRevolution()" (read, not run).
- *Pad, pocket, revolve*, "The result fuses with or cuts the base; the result must hold a solid, every solid counted (`eb52dd4a9a`)": the single-solid rule is enforced only when the body's `AllowCompound` is false, and it's true by default since `406c3ba42c`.
- *Pad, pocket, revolve*, "the n-th construction line": a construction **line segment** (`AxisN`); a sketch's edge, construction or not, also gives an axis, as an edge of a `Part::Feature`.
- *The sketch feature*, "Names: … FreeCAD's later features name them through the element map (`Edge<n>` with the `SKT` op code)": the mapped names are the geometries' facade IDs, `g<id>` and `g<id>v<pos>`; `Edge<n>` is only the index name.
- *Tests to port*, `Pad.cpp` `TestMidPlaneTwoLength`, "3000π": FreeCAD asserts only the bounding box; 3000π is the analytic volume Silico checks.
