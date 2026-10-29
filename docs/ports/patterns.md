# Patterns and mirror

The linear pattern, the polar pattern, and mirror: copies of features (or of a whole solid) moved, turned, or reflected, and fused into or cut from the solid they're patterned on. ROADMAP.md puts patterns and mirror after the vertical slice, in "the full feature set" of stage 1. This note is the study they start from. It says what FreeCAD does, the edge cases, the fixes in FreeCAD's history, the tests to port, where SolidWorks differs, and what Silico should do. **This note decides nothing.** The decisions at the end are open, each with a recommendation, for the step's design and the maintainer to take or change.

## Sources

- **FreeCAD 1.1.3** (tag `145529fe74`), read from a sparse clone of the tag, since the maintainer's clone has no tags. Line numbers without a prefix are 1.1.3's, in `src/Mod/PartDesign/App/`: `FeatureTransformed.cpp` (FT), `FeatureLinearPattern.cpp` (LP), `FeaturePolarPattern.cpp` (PP), `FeatureMirrored.cpp` (MI), `FeatureMultiTransform.cpp` (MT).
- **FreeCAD's main** at `876271d660` (2026-10-07), for history. FreeCAD 1.1 branched from main at `af2d42a4aa` (2025-11-13). A commit before that is in 1.1.3. After it, only three of these files' commits were picked into 1.1.3: `5ae67ee2f9`, `b0331ed979` and `4fae2b83f8`. Everything else after 2025-11-13 is main only.
- **Main differs a lot.** `c334ac5062` (2026-06, #30417) moved the linear and polar logic into Part (`src/Mod/Part/App/LinearPatternExtension.cpp`, `PolarPatternExtension.cpp`). Main also added circular, path and point patterns (`173276b175`, `c633c5c88e`, `f5abab2768`), instances to skip (`cdb4624675`), and a cap on occurrences (`293726c5d8`). **Silico follows 1.1.3**, with main's fixes where they matter (§4).
- **The oracle, FreeCAD 1.1.3**, run headless (`.pixi/envs/oracle/bin/freecadcmd`) on probe scripts in `/tmp/patprobe/`. A claim marked *(1.1.3)* was observed. The probes use PartDesign primitives (`AdditiveBox`, `AdditiveCylinder`, `SubtractiveCylinder`) as originals, at their own placements, in a Body with its default `AllowCompound = True` and `Refine = True` unless they say otherwise.
- docs/ports/naming.md, docs/design/m2.7-naming.md (N2's "copy marker"), docs/design/m3.6-sketch-features.md (S5 one solid, S7 a change of nothing, S8 no refine), docs/ports/booleans.md (compound tools), and docs/ports/tessellation.md (mirrored locations).

## 1. What FreeCAD does

### `Transformed`, the base class

`Transformed` (FT, header `FeatureTransformed.h:40`) is a `FeatureRefine`. Its subclasses supply only a list of `gp_Trsf` (`getTransformations`). The base class does the rest.

**Inputs.**
- `Originals`: the features to copy, a link list (FT:67).
- `TransformMode`: `Features` (copy the originals' tools) or `Whole shape` (copy the whole base solid) (FT:63; the strings were renamed in `9535371265`, same indices).
- `Refine` (true by default since `e490531510`).
- The support is the pattern's `BaseFeature`, the previous feature in the Body (`getBaseObject`, FT:84-114). An original is never the support, except in old files.
- Suppressed originals are dropped, as if they weren't listed (`getOriginals`, FT:116-138; `79f9da420c`).
- Only a `FeatureAddSub` can be an original: pad, pocket, revolution, groove, the primitives, and dress-ups. Anything else is refused: "Only additive and subtractive features can be transformed" (FT:386-392).

**How the result is built** (`execute`, FT:294-439).
1. The support's shape is taken without its placement (FT:351-355). Each original's tool is moved into that frame by `feature location × support location⁻¹` (FT:400). That order is wrong when the two placements don't commute: main's `5d8162107a` (2026-06) reverses it, after a primitive with its own rotated placement was patterned from the wrong place.
2. Each original gives a fuse shape, a cut shape, or both (`getAddSubShape`, FT:394). A pad gives a fuse shape and a pocket a cut shape. A dress-up gives both: what it added and what it removed (`7fadfd0439`, `a59dac1675`). Neither is "Shape of additive/subtractive feature is empty" (FT:395-399).
3. **The first transformation is skipped**, so the first occurrence is the original itself, which the support already holds (FT:361-362; `a0746a0c02`). Re-fusing the original's own tool used to undo edits made between it and the pattern, such as refilling a later pocket.
4. For each original, in `Originals`' order, the copies of its fuse shape are fused into the support in **one boolean**, the support as the argument and each copy a separate tool: `makeElementFuse({support, copy1, copy2, …})` (FT:407-413). Then the copies of its cut shape are cut the same way (FT:414-420). One boolean replaced earlier schemes that fused copies one by one or compounded them (`5ab7d17f67`, #6641).
5. The result is refined if `Refine` is on (FT:433). Then `Shape = getSolid(result)` (FT:435). The rest of the result's solids go in `rejected` (FT:436, `getRemainingSolids` FT:441-459), which the GUI shows red with "N transformed shapes do not intersect the support".
6. `getSolid` (`Feature.cpp:174-193`) returns the whole result when the Body's `AllowCompound` is true, the default since `406c3ba42c` (2025-09). When it's false, it returns the result's **first solid** and drops the others silently.

Whole-shape mode copies the support itself, with the same skipped first transformation, and fuses (FT:423-430). It has no cut.

**What isn't checked.**
- **No intersection or overlap check.** The "does not intersect support" rejection, the overlap error, and the `Overlap` property were all removed by 2022 (§4).
- **No single-solid error** in the pattern. With `AllowCompound` on, copies that miss the base give a compound of several solids, reported as valid.
- **No check that a copy changed anything.**
- **A pattern in Features mode with no originals does nothing and reports no error.** It's taken for a MultiTransform's child (`isMultiTransformChild`, FT:169-194), returns at once (FT:296-298), and **keeps its previous shape** *(1.1.3: a mirror switched from Whole shape back to Features with `Originals = []` keeps the whole-shape result, `Up-to-date`)*.

### `LinearPattern` (LP)

**Properties** (LP:57-197, from the Transform rework, `5d2037c820`, 2025-08):
- `Direction`: a link to a reference.
- `Reversed`.
- `Mode`: `Extent` or `Spacing`.
- `Length`, default 100, used in `Extent` mode.
- `Offset`, the step, default 10, used in `Spacing` mode.
- `Occurrences`, default 2, constrained to [1, INT_MAX] (LP:52, `7f1b47d118`).
- `Spacings`: one entry a gap, −1 meaning `Offset`.
- `SpacingPattern`: experimental, cycled.
- A second direction with the same eight properties (`Direction2` … `Occurrences2`, default 1).

**Steps** (`getTransformations`, LP:227-260):
- Fewer than one occurrence in either direction is "At least one occurrence required" (LP:231). The property's constraint clamps first, so 0 and −1 become 1 *(1.1.3)*.
- One occurrence in both directions returns the identity before anything else is checked (LP:235-237; `c001732663`, so an expression can turn a pattern off). The pattern then changes nothing *(1.1.3)*.
- In each direction (`calculateOffsetVector`, LP:262-296), `Length` below `Precision::Confusion()` is "Pattern length too small" (LP:279-282). That check runs in **both modes**. In `Spacing` mode, `Length` is kept at `Offset × (n − 1)` by `onChanged` (LP:566-586; `1c70983c60`, `fa0702956c`), so an `Offset` of 0 also fails as "Pattern length too small" *(1.1.3)*. `Length` and `Offset` are `PropertyLength`s, so a negative value is clamped to 0 *(1.1.3: −5 becomes 0)*.
- Extent mode's step is `Length / (n − 1)`. Spacing mode's is the unit direction times each cumulative spacing: a `Spacings` entry if it isn't −1, else the `SpacingPattern` entry, else `Offset` (LP:298-349).
- **`Spacings` entries aren't checked.** 0 puts a copy on the one before it, and a negative value goes backwards *(1.1.3: `Spacings = [0, 10]` gives two cylinders, not three; `[-5, 10]` puts copies at x = 5 and 15, overlapping the original)*.
- A missing `Direction` gives a zero step (LP:271-274; `7e57b6e7b0`), so every copy lies on the original and the pattern changes nothing, without an error *(1.1.3)*.
- **Two directions:** the copies are every sum of a direction-1 step and a direction-2 step, direction 1 outer (LP:250-257). Instance (i, j) is at index `i·n₂ + j`, and index 0 is the original.

**Direction** (`getDirectionFromProperty`, LP:351-468):
- From a sketch: `H_Axis`, `V_Axis`, `N_Axis`, or `AxisN` (the Nth construction line), placed by the sketch. Or a sketch's straight `EdgeN`, already in global coordinates, so not placed again (`19cd040415`; `4c509c03ef` fixed the double placement).
- A datum line's direction. A datum plane's or origin plane's normal (`880587a627`, `cf0412b7e2`). An origin axis (`b3a1fd9676`).
- A feature's straight edge (its line's direction) or planar face.
- **A face's direction is its surface's axis, not its outward normal** (LP:440). A box's Face1, whose outward normal is −X, patterns along +X, and its Face5 (outward −Z) along +Z *(1.1.3)*.
- Refused:
  - "Direction face must be planar" (a cylindrical face);
  - "Direction edge must be a straight line" (a circle);
  - "Direction reference must be edge or face" (a vertex);
  - "Direction reference must be edge/face of a feature or a datum line/plane".
  
  All observed *(1.1.3)*.
- The direction is put into the feature's frame (LP:464-465).

### `PolarPattern` (PP)

**Properties** (PP:57-130): `Axis`, `Reversed` (counter-clockwise by default, clockwise when set), `Mode` (`Extent` or `Spacing`), `Angle` (default 360), `Offset` (default 120), `Occurrences` (default 3, [1, INT_MAX]), `Spacings`, `SpacingPattern`.

**Angles.** 1.1.3 declares a constraint of [Angular°, 360] (PP:52) but never applies it: `5ae67ee2f9` (#25621, picked into 1.1.3) removed it to allow negative angles. `Angle` and `Offset` keep `PropertyAngle`'s default range, so −400 becomes −360 and 400 or 720 become 360 *(1.1.3)*.

**Steps** (`getTransformations`, PP:143-325):
- One occurrence returns the identity (PP:150; `356db441a6`).
- **Extent mode:**
  - When `|Angle − 360| < Precision::Confusion()` (1e-7 degrees), the step is `Angle / n`, so the last copy doesn't land on the original. Otherwise it's `Angle / (n − 1)` (PP:270-280).
  - A step under `Precision::Angular()` (1e-12 rad) is "Pattern angle can't be null" (PP:282; `eccc5bfe1a`). *(1.1.3: 0 and 1e-12 degrees are refused.)*
  - Copy i is turned by `step × i` (PP:319).
- **Spacing mode:**
  - Copy i is turned by the cumulative sum of the spacings (PP:304-317; `0f07a936d9` fixed an off-by-one).
  - **Nothing is checked.** *(1.1.3: an `Offset` of 0 with four occurrences puts every copy on the original. 120° with five wraps past a turn, copies landing on copies, with no error.)*
- **The full-turn rule is a discontinuity.** *(1.1.3, a cylinder r 3 at radius 20 on a disk, four occurrences:)*

  | Angle | Copies at | Distinct |
  |---|---|---|
  | 360 | −90, 0, 90, 180 | 4 |
  | 359.9999999 (just past the 1e-7 tolerance) | −120, 0, 120 (the fourth lands 1e-7° from the original and fuses into it) | 3 |
  | 359.99 | the fourth beside the original, leaving slivers (5 cylinder faces) | 3 and a sliver |
  | −360 (not taken as a turn: the test is `Angle − 360`) | the last on the original | 3 |
  | 180 | 0, 60, 120, 180 | 4 |
  | −90 | 0, −30, −60, −90 | 4 |

**Axis** (PP:156-266):
- From a sketch: `H_Axis`, `V_Axis`, `N_Axis`, `AxisN`, and since `b0331ed979` (#26722, picked into 1.1.3) a straight `EdgeN`. Before that fix, a sketch edge left the axis zero, and the feature failed.
- A datum line or origin axis, by its base point and direction (`077745d737` fixed an `App::Line` turned about the origin instead).
- A feature's straight edge, or a **circular edge, by its centre and normal** (`52c0558f91`) *(1.1.3: a disk's rim edge works as the axis)*.
- A face is refused, "Axis reference must be an edge", though the property's tooltip offers "the normal of a planar face". A plane is refused, "Axis reference must be edge of a feature or datum line". *(1.1.3, both.)*
- An axis parallel to the base's face isn't refused: the copies swing out of it *(1.1.3: about X, two solids, reported valid)*.

### `Mirrored` (MI)

- **One property, `MirrorPlane`** (MI:47-56). The transformations are the identity and `gp_Trsf::SetMirror(gp_Ax2)` (MI:200-214).
- **The plane, tried in order** (MI:66-198):
  - **A sketch.** With no sub-element it's `N_Axis`, the sketch's own plane (`7d10f5ed73`, #13238). `H_Axis` mirrors about the plane whose normal is `V_Axis`, so the plane holds the H axis and the sketch's normal, and `V_Axis` likewise. `AxisN` uses the plane through the construction line and the sketch's normal (`39792f0773`); an invalid N is "No valid axis specified".
  - **A datum plane or origin plane.**
  - **A feature's planar face**, through its first vertex (`getPointFromFace`, `Feature.cpp:312-326`).
- **Refused** *(1.1.3)*:
  - a cylindrical face: "Mirror face must be planar";
  - an edge: OCCT's bare "TopoDS::Face", from a failed cast;
  - an axis: "Mirror plane reference must be a sketch axis, a face of a feature or a datum plane".
- **The copy is a real copy.** `makeElementTransform` sees a negative determinant and rebuilds the geometry with `BRepBuilderAPI_Transform` instead of setting a location (`src/Mod/Part/App/TopoShapeExpansion.cpp:3439-3459`; `8453d41515`). Rebuilding shows round-off: a 10 mm-thick box mirrored whole about x = 100 has a bounding box from z = −4.6e-14 *(1.1.3)*.
- **Edge cases** *(1.1.3)*:
  - A plane through the original's own plane of symmetry puts the copy on the original, and the mirror changes nothing.
  - A plane 1 mm off a cylinder's axis makes the copies overlap; the fused volume matches the analytic lens, 20200.241263.
  - A whole-shape mirror about the base's own face fuses the two halves into one solid. About a plane 50 mm away it gives two solids, reported valid.

### `MultiTransform` (MT)

- **What it is.** A list of `Transformed` children, `Transformations`. Their transformation lists are combined: every new transformation multiplied with every old one, new outer and old inner (`nt * ot`, MT:175-183), so the count is the product. `Scaled` is combined diagonally instead (MT:120-166) and needs the centre of mass of the first original (MT:82-93, "will NOT give the expected result for more than one original").
- **The children.** They take the MultiTransform's placement (MT:48-66; `afb3355193`) and build nothing themselves. They're recognised only by having empty `Originals` (above), and their own `Shape` stays null *(1.1.3)*.
- **Bearing on Silico.** It only matters for how indices compose. A 3 × 2 MultiTransform of two linear patterns gives the six positions of one two-direction linear pattern *(1.1.3)*. **Recommended:** don't port it as a feature. A linear pattern's second direction covers its common use, and a pattern of a pattern is two features in the timeline.

## 2. Edge cases and refusals

| Input | FreeCAD 1.1.3 *(observed)* | Silico (recommended, §7) |
|---|---|---|
| 0 or −1 occurrences | Clamped to 1 by the property; nothing happens | Refused by the gate (`invalid_input`, naming `count`): a count is at least 1 |
| 1 occurrence | The identity; nothing else checked | The base unchanged, its references still checked |
| A huge count | Up to INT_MAX (main caps at a preference, `MaximumPatternOccurrences`, default 1000, `293726c5d8`) | Refused above a fixed cap (decision 8) |
| Length 0 (extent) | "Pattern length too small" | Refused by the gate, naming `length` |
| Spacing 0 | "Pattern length too small" (through the synced length) | Refused by the gate, naming `spacing` |
| Negative length or spacing | Clamped to 0, then refused | Refused, naming it: `reversed` says the sense |
| A per-gap spacing of 0 or negative | Taken: copies on copies, or backwards | Not ported (decision 4) |
| No direction | A zero step: every copy on the original, no error | The gate refuses the missing field |
| Direction a circle, a curved face, a vertex | Refused, by message | Refused, naming the element and its kind |
| Direction a planar face | The surface's axis, whatever the face's orientation | The face's outward normal (decision 3) |
| Polar angle 0 or 1e-12° | "Pattern angle can't be null" | Refused by the gate, naming `angle` |
| Polar angle −360, or a spacing past a whole turn | Copies land on the original or on each other, silently | Refused (decision 5) |
| Polar angle 359.9999999 | A step of a third; the last copy 1e-7° off the original | Exactly as given: a step of a third, 359.9999999 written. Only 360 exactly is a whole turn |
| Polar axis a face or a plane | Refused | Refused, naming it |
| Mirror plane a curved face, an edge, an axis | Refused (an edge with "TopoDS::Face") | Refused, naming the element and its kind |
| A copy that doesn't touch the base (additive) | `AllowCompound` on: a compound of several solids, "valid". Off: the first solid only, the copies dropped silently | Refused: more than one solid (S5), naming the instances (decision 6) |
| A pocket's copy that misses the base | Silent: that copy removes nothing | Refused: a copy that changes nothing, naming its instance (decision 6) |
| A slot that cuts the base in two, patterned so its copy cuts it again | A compound of three solids, "valid". `AllowCompound` off: the first, 3717 of 18917 mm³, kept without a word | Refused: more than one solid (S5) |
| A copy on the original (spacing 0, a symmetric mirror) | Silent: it fuses into the original | Refused where it can be seen (decision 6) |
| Copies that overlap each other | Fine: separate tools in one boolean. Three holes r 3, 2 mm apart: 518.221711 mm³ removed, against 518.221713 by integration | The same: separate tools (booleans.md, M3.6), never one compound |
| Copies that touch face to face | Fuse into one solid (10 boxes of 10 along X: 10000, 42 faces unrefined) | The same |
| An original that isn't additive or subtractive | "Only additive and subtractive features can be transformed" | The gate refuses it, naming it (decision 2) |
| Features mode with no originals | Nothing happens; the previous shape kept | The gate refuses an empty `features` |
| Originals in another order | Applied in the list's order (main sorts by the Body's order, `6265896ca8`) | Applied in timeline order, whatever the list says |
| A pattern patterned again | MultiTransform, or a pattern whose original is a pattern | A pattern of a pattern is later; not in the first step (decision 2) |

## 3. Topological naming

### What FreeCAD does

`getTransformedCompShape` (FT:357-371) makes each copy with `makeElementTransform(trsf, opName)`, where `opName = Data::indexSuffix(idx)` with `idx` counting from 1. `indexSuffix` gives "" for 1 and `_k` from 2 (`src/App/ElementNamingUtils.cpp:116-124`; `ELEMENT_MAP_INDEX` is `_`). Because `op` is set, even to "", `makeElementTransform` copies the tool's whole element map as a child map with `op` as its postfix (`copyElementMap`, `TopoShapeExpansion.cpp:787-808`). The boolean then names the result from the support (the argument) and the copies (the tools). This is the form from `25ba8ab223` and `a1ce983035` (2024-08).

A cylinder r 3 on a 100 × 20 × 10 box, three occurrences over 40 along X, gives these side faces *(1.1.3)*:

| Instance | Name |
|---|---|
| 0, the original | `Face1;:M;FUS;:H-fd7:7,F;:Hfd7,F` |
| 1 | `Face1;:H-fd7,F;:H:9,F;:M;FUS;:H-fd7:7,F;:Hfd7,F` |
| 2 | `Face1;:H-fd7,F;_2;:H:c,F;:M;FUS;:H-fd7:7,F;:Hfd7,F` |

So:
- copy 1 differs from the original only by a child level's tag (`;:H:9`);
- copy 2 has the suffix `_2` as well;
- the tops are `Face3;:H-fd7,F;:H:9,F;:Hfd7,F` and `…;_2;:H:c,F;…`;
- a mirror's single copy is named like a pattern's copy 1 (`…;:H:9,F;…`);
- the element map has 47 entries.

The index is the copy's place in the transformation list. A two-direction linear pattern flattens `(i, j)` to `i·n₂ + j`, so changing `Occurrences2` renames every copy beyond the first row. MultiTransform composes indices as its lists do.

With `Refine` on, a refine pass can't always follow the names: "Cannot find RFI modified Face from Face8" *(1.1.3, in the mirror probes; which case raised it wasn't isolated)*.

FreeCAD's tests pin only refined map sizes: 26, a plain box's (§5). Their TODOs give the unrefined sizes: 170 for ten boxes in a row, 66 for four turned, 42 for a mirrored box *(1.1.3, with `Refine = False`, every one as the TODO says)*.

On main, instances to skip keep their index, so the surviving copies keep their names (`cdb4624675`).

### What Silico's naming needs

Silico's N2 (a kept element keeps its name exactly) is safe only because an element's lineage is a tree: a solid is consumed once (M2.1's D2), so one origin can't reach a shape twice. A pattern breaks that. Each copy of a pad's side face would be `p.pad.Face(G s.sketch.Edge.c)`, the same as the original's. naming.md and the M2.7 design reserved "a copy marker" for this.

Each copy needs:
1. **Its own name, from its instance**: the pattern's ID and type, the instance, and the tool element's name, e.g. `t.linear_pattern.Face(C2 p.pad.Face(G s.sketch.Edge.c))`. The instance is the copy's place in the pattern as the document gives it: `2` along one direction, `2,1` in a grid (direction 1's index, then direction 2's), and `1` for a mirror. It never depends on OCCT's lists.
2. **The original keeps its names.** Instance 0 isn't copied: it's in the base, and the pattern's boolean bites the base, so its names pass through as they are (N2, N7). A fillet on the original's edge is untouched by adding a pattern after it.
3. **A copy's elements pass through its boolean by the rules already there.** A copy is a fuse operand or a cut tool:
   - a fuse bites both its operands, so a copy's face kept or modified one from one keeps its copy name;
   - a cut's tool is a piece (`I`), as a pocket's walls are today, except that a sweep's cut bites its own sweep (naming.md): the same rule applies to a copy of a pocket's sweep, so its walls are `t.linear_pattern.Face(C2 q.pocket.Face(G …))`;
   - where a copy meets the base, the section is generated from both, as for any boolean.
4. **Grid indices, not a flat one,** so changing direction 2's count doesn't rename direction 1's copies. Changing a count adds or removes copies at the end and renames none. Changing the length moves copies and renames none, as in FreeCAD.
5. **A skipped instance keeps its number free,** so skipping instance 2 renames nothing else (main's rule, and SolidWorks's coordinates).
6. **Whole-solid mode** copies every element of the base: `t.mirror.Face(C1 b.box.Face6)`. The base's own elements keep their names.
7. **Mirror changes no name.** A reflection reverses orientation, and the history is still one element to one.
8. **The naming scheme.** A new letter in the grammar is a change of scheme (`NAMING_SCHEME`, now 5), and today a document of another scheme is refused (M4.0). Scheme 5's names are a subset of the new grammar's, so the reader must read both, as M4.0's header anticipates. See *Flags*.

## 4. FreeCAD's history

Each commit below was read from its diff. Formatting, includes, renames, translations and modernising are left out.

| Commit | Date, issue | What it fixed or changed | 1.1.3? | In 1.1.3's code? |
|---|---|---|---|---|
| `32628462c2` | 2012-10 | `Part::checkIntersection`. A copy not meeting the support warned "does not intersect support: Removed"; a copy overlapping the original was an error | yes | No |
| `515f30c1a0` | 2012-12, Mantis #906 | Patterning a non-PartDesign object segfaulted on a null tool; now "Shape of additive/subtractive feature is empty" | yes | Yes, FT:394-399 |
| `afb3355193` | 2013-01 | MultiTransform children take its placement; a pattern's placement is read-only | yes | Yes, MT:48-58, FT:69 |
| `39792f0773` | 2013-01 | Mirroring about a sketch construction line used the line as the normal; the plane now holds the line and the sketch's normal | yes | Yes, MI:86-98 |
| `18da371546`, `8235008ae7` | 2013-01, 02 | Touching counts as attached; rejected copies dropped and shown red | yes | No |
| `880587a627` | 2013-05 | Datum lines and planes as references | yes | Yes, LP:407-418, MI:113-138 |
| `fcea39b0d8` | 2013-09 | Several originals, in list order, the support updated after each | yes | Yes, FT:381-421 |
| `1772eb70bc`, `4b2f107cc1` | 2013-12, 2014-01 | Touching copies rejected as overlapping, then the overlap check removed: overlapping and coincident copies legal | yes | No (overlap allowed) |
| `4c9b9adde6` | 2014-01 | `std::abs` to `std::fabs` in the 360° test (an int overload, probably) | yes | Yes, PP:273 |
| `aaeba38e78`, `e47b85994c` | 2014-03 | Crashes editing a pattern with no originals | yes | Yes, FT:151-159 |
| `9a2f5eea65`, `d923bebccc` | 2015-05 | Any additive or subtractive feature (primitives) can be patterned | yes | Yes, FT:386-392 |
| `60062892a6` | 2015-09 | Old files' float `Angle` and `Length` read into the typed properties | yes | Yes, FT:196-216 |
| `83ce80f133` | 2016-05, Mantis #2512 | The result is the first solid | yes | Yes, FT:435 |
| `63ad26491f` | 2016-08 | `Scaled` with no transformations divided by zero | yes | Yes, MT:130-141 |
| `ff188383a1`, `6f18ce63ed` | 2017-06 | Copies fused one by one (so copies touching only copies weren't rejected); any rejection an error "Transformation failed" | yes | No |
| `488c1a1fdf` | 2017-07 | A refine failure keeps the unrefined shape | yes | Yes, `FeatureRefine.cpp:103-114` |
| `5a2acd5645` | 2017-10 | MultiTransform children stayed touched after a recompute | yes | Yes, MT:60-64 |
| `27701ae852` | 2018-01, #2683 | A `Refine` property on each pattern | yes | Yes |
| `01e6cde7aa` | 2018-01, #3317 | Mirroring several features failed when the base wasn't set | yes | Yes, FT:308-312 |
| `883a726d52`, `7e781bcfd0` | 2018 | More than one solid became an error (extra solids had been dropped silently) | yes | No |
| `19cd040415` | 2018-09 | A sketch's straight edge as a linear direction | yes | Yes, LP:385-404 |
| `7fadfd0439` | 2020-03 | A dress-up patterns only what it added or removed; a copy that adds a separate solid rejected | yes | The split, yes (`FeatureDressUp.cpp:307-370`); the rejection, no |
| `4a5cca3359` | 2020-07 | Tools moved into the support's frame; copies were misplaced when placements differed | yes | Yes, FT:400, in the wrong order (`5d8162107a`) |
| `81a5ec5e55`, `356db441a6` | 2020-12, 2021-03 | One occurrence allowed: the original alone | yes | Yes, LP:231-237, PP:145-152 |
| `52c0558f91` | 2021-02 | A circular edge as a polar axis | yes | Yes, PP:239-249 |
| `03da9d3501`, `fecf9c2e84`, `3131b97aa8` | 2021-02 | Per-copy checks removed; one boolean with overlap detection; an `Overlap` property; more than one solid a warning, the first kept, the rest `rejected` | yes | First solid and `rejected`, yes (FT:435-459); detection, no |
| `8453d41515` | 2021-02 | A mirror can't be a location (negative determinant): copy with `BRepBuilderAPI_Transform` | yes | Yes, `TopoShapeExpansion.cpp:3439-3459` |
| `7f1b47d118`, `eccc5bfe1a` | 2021-03 | Occurrences constrained to ≥ 1; the angle check in radians against `Precision::Angular` | yes | Yes (the angle's range constraint, no: `5ae67ee2f9`) |
| `a0746a0c02` | 2022-01, forum t=64900 | The original excluded from the tools again: re-fusing it undid later edits | yes | Yes, FT:361-362 |
| `5ab7d17f67` | 2022-04, #6641 | A polar pattern failed. Detection replaced by one boolean, every copy a separate tool | yes | Yes, FT:407-420 |
| `4358a9ba70`, `a59dac1675` | 2022-04 | `Overlap` deleted; fuse and cut both run for a dress-up | yes | Yes |
| `2b1108439f`, `6d7fea506c` | 2022-05, 2023-08 | Length or offset modes (now Extent, Spacing) | yes | Yes |
| `c26a4c7978` | 2022-12, #7791 | One occurrence failed with "Transformation failed" (no tools) | yes | Yes |
| `c001732663` | 2023-03 | One occurrence returns before length, angle, or direction is checked | yes | Yes, LP:235, PP:150 |
| `1c70983c60`, `fa0702956c` | 2023-08, 2024-01, #12068 | `Length` and `Offset` kept in step; at one occurrence this divided by zero until `5d2037c820` | yes | Yes, LP:530-586 |
| `4c509c03ef` | 2023-10, forum t=82022 | A sketch edge's direction placed twice | yes | Yes |
| `79f9da420c` | 2024-03 | Suppressed originals skipped | yes | Yes, FT:126-135 |
| `45bb606095`, `5dca1d3e04` | 2024-03, 05 | `TransformMode` (whole shape); a MultiTransform child is one with no originals | yes | Yes, FT:63, 169-194 |
| `7d10f5ed73` | 2024-03, #13238 | A sketch as a mirror plane with no sub-element failed; now its own plane | yes | Yes, MI:77-79 |
| `935bdf9a0f` | 2024-05 | The single-solid rule behind the Body's `AllowCompound` | yes | Yes, `Feature.cpp:174-193` |
| `2b97b20a03`, `25ba8ab223`, `a1ce983035` | 2024-08 | Naming of copies (§3). `2b97` left the support out of the fuse; `25ba` included the identity and did the cut as a fuse; `a1ce` fixed both | yes | Yes, FT:357-371 |
| `cf8ad66373` | 2024-10, #17119 | Booleans through FreeCAD's wrappers, with a fuzzy value | yes | Yes |
| `b3a1fd9676`, `077745d737` | 2024-12, 2025-03 | Datums' base and direction honour an enclosing coordinate system; a polar axis on an origin line turned about the origin | yes | Yes, PP:218-224 |
| `5d2037c820` | 2025-08 | The Transform rework: Extent and Spacing; linear's second direction; `Spacings` and `SpacingPattern`; the default occurrences 3 → 2; the step clamped at one occurrence | yes | Yes |
| `0f07a936d9` | 2025-09 | Polar spacings off by one | yes | Yes, PP:306-311 |
| `7e57b6e7b0` | 2025-09, #23637 | A missing direction gives a zero step instead of an error; origin axes link with an empty sub-name | yes | Yes, LP:271-274 |
| `5ae67ee2f9` | 2025-11, #25621 | Polar angle constraints removed: negative angles reverse; −360 isn't a whole turn | picked | Yes, PP:282-284 |
| `b0331ed979` | 2026-01, #26722, #24700 | A sketch edge as a polar axis left the axis zero and failed | picked | Yes, PP:189-206 |
| `4fae2b83f8` | 2026-01 | The preview places tools in the support's frame | picked | Yes, FT:226-282 |
| `123a142292` | 2025-12 | Message: "No features selected to be mirrored/patterned/transformed" | main | No |
| `99422b51cc` | 2026-05 | With `AllowCompound`, `rejected` emptied, so no false "do not intersect" warning | main | No: 1.1.3 warns falsely |
| `5d8162107a` | 2026-06 | The tool's transform in the right order: a primitive with its own rotation was patterned from the wrong place | main | No |
| `6265896ca8` | 2026-06, #30548 | Originals sorted by the Body's order, so selection order no longer changes the result | main | No |
| `6523471254` | 2026-06 | MultiTransform children no longer recompute twice | main | No |
| `cdb4624675`, `5cf3c96149` | 2026-09, 10, #32843 | `SuppressedIndices`: instances to skip, each keeping its index; skipping 0 removes the original's material | main | No |
| `293726c5d8` | 2026-09, #33056 | Occurrences capped at a preference, `MaximumPatternOccurrences`, default 1000 | main | No |

**The intersection checks, in short.**
- **2012–2014.** A copy that didn't meet the support was dropped with a warning, overlap was an error, and the overlap check was removed in 2014.
- **2017–2018.** Rejection became an error, and so did more than one solid.
- **2021–2022.** Everything was removed for one multi-tool boolean, keeping the first solid.
- **Since 2025.** `AllowCompound` defaults to true, so 1.1.3 returns a compound of whatever the boolean gives.

FreeCAD tried to check, and every check it had was removed for speed or for false positives, never because the cases stopped mattering.

## 5. FreeCAD's tests

1.1.3's tests (`src/Mod/PartDesign/PartDesignTests/`) are each a cube (10 mm, or 1 mm for two of the mirror's), a box primitive or a padded square, with its volume as the only real check. The map sizes they assert are refined (26, a plain box's). They're expected values Silico can use, unrefined, with the counts from the probe.

| Test | Asserts | Port? |
|---|---|---|
| `TestLinearPattern.py` `testXAxisLinearPattern`, `testYAxis…`, `testZAxis…` | a 10 cube, 10 occurrences over 90 along an origin axis: volume 1e4, map 26 (refined) | **Yes**: 10000, one solid, unrefined 42 faces, 84 edges, 44 vertices *(1.1.3)*. Ten copies touching face to face |
| `testNormalSketchAxis…`, `testVerticalSketchAxis…`, `testHorizontalSketchAxis…` | the same with a padded square, along the sketch's N, V, H axes | Yes, when sketch lines can be directions (decision 3); until then, the origin axes they equal |
| `TestPolarPattern.py` six tests | a 10 cube turned 4 × about each origin axis and sketch axis, 360°: volume 4000 | **Yes**: 4000, one solid (four cubes meet at the axis), unrefined 16, 32, 18 *(1.1.3)* |
| `TestMirrored.py` `testMirroredSketchCase`, `testMirroredPrimitiveCase` | a unit cube mirrored about the sketch's V axis, or XY: volume 2 | **Yes**: 2, one solid; a 10 cube about XY gives 2000, unrefined 10, 20, 12 *(1.1.3)* |
| `testMirroredOffsetCase` | a unit square at (0, 1)–(1, 2), padded 1 and mirrored about the sketch's H axis, so the copy lies 2 mm from the original: only "Up-to-date" | **Yes, as a refusal**: two solids, refused (decision 6). FreeCAD accepts the compound |
| `TestMultiTransform.py` `testMultiTransform` | mirror × linear 3 × polar 4 of a 10 cube: 20000 | As a pattern of a pattern, later; or as one two-direction linear pattern with its own expected value |
| `testMultiTransformDressup` | two mirrors, then a fillet of two faces of the result (r 3): a closed-form volume | **Yes, later**: it tests that a fillet can name a mirrored copy's edges, which is decision 7's naming. Its volume formula is independent |
| `testMultiTransformBody` | empty (`pass`, a TODO) | No |
| Main's `TestLinearPattern.py` suppression tests (`testSuppressionKeepsGridPosition`, `…Persistence`, `testSuppressOriginal…`, 11 in all) | instances to skip: grid positions kept, the original suppressible | **The first** (positions and names kept when skipping) with decision 7; not the original's suppression (decision 7 refuses it) |
| Main's `testOccurrencesAreClampedToMaximumPatternOccurrences` | the cap | As decision 8's refusal, not a clamp |
| `TestTopologicalNamingProblem.py` | no pattern test in 1.1.3 or main | Silico's own (§7) |
| `tests/src/Mod/Part/App/FeatureMirroring.cpp` | Part's `Mirroring` (a shape's mirror, not PartDesign's) | Its geometry cases, as kernel tests of the mirror request |

There are no C++ tests of PartDesign's patterns (`tests/src/Mod/PartDesign/App/` has Pad, Pipe, Revolution, and others, none of these).

## 6. SolidWorks

From help.solidworks.com:
- [Linear Pattern PropertyManager](https://help.solidworks.com/2017/English/SolidWorks/sldworks/hidd_lpattern.htm), [Linear patterns overview](https://help.solidworks.com/2021/english/SolidWorks/sldworks/t_Linear_Patterns_Overview.htm), and [Selecting instances to skip](https://help.solidworks.com/2021/english/SolidWorks/sldworks/t_selecting_instances_to_skip_patterns.htm);
- [Circular Pattern PropertyManager](https://help.solidworks.com/2018/english/SolidWorks/sldworks/HIDD_CPATTERN.htm);
- [Mirror Feature PropertyManager](https://help.solidworks.com/2024/english/SolidWorks/sldworks/hidd_dve_feat_mirror.htm).

The pages render through scripts and refused direct downloads, so the wording below comes from search excerpts of them, not a full read.

**Linear pattern.**
- One or two directions. Each has a reference (a linear edge, an axis, a dimension, a planar face's normal), a spacing between instances, and a number of instances.
- **Instances to Skip:** instances picked in the graphics area are skipped. Each instance is shown by its coordinates, so it keeps its place in the grid.
- **Geometry pattern:** "creates the pattern using only the geometry (faces and edges) of the features, rather than patterning and solving each instance". It's faster, and can't be used for features whose faces merge with the rest of the part. **Without it (the default), SolidWorks solves each instance anew**: an up-to-surface extrude in each instance runs to that surface.

**Circular pattern.**
- An axis, an angle, and a number of instances, "the number of instances of the seed feature".
- **Equal spacing** sets the angle to 360°. Without it, the angle is the angle between instances.
- Instances to skip, as linear.
- Whether the count includes the seed isn't said in the excerpts. SolidWorks's own dialog counts it (6 equally spaced instances are the seed and five copies, 60° apart): *unconfirmed from the pages*.

**Mirror.**
- A face or plane, and features, faces, or bodies to mirror.
- Geometry pattern, as above.
- **Merge solids**, when mirroring bodies: the mirrored body fused with its seed, or kept as a separate body.

**Where it differs from FreeCAD, and which to follow.**
1. **Re-solving each instance.** Silico's sweep extents are a length, two lengths, symmetric, or through all, none up to a face, so a copy of the tool and a re-solved instance agree, but for **through all**: the original's through-all prism is sized by its own base, and a copy moved far along may not reach through. **Recommended: copy the tool** (FreeCAD's way, SolidWorks's geometry pattern). It's one boolean, deterministic, and cached, and a through-all copy that no longer cuts is refused by decision 6's "changes nothing" check (decision 4). One that cuts only part way isn't caught: that's the geometry pattern's known limit, to document. Re-solving comes with up-to-face extents, if ever.
2. **Instances to skip.** SolidWorks and main have them; 1.1.3 doesn't. **Recommended: port**, by instance index, never the seed (decision 7).
3. **Equal spacing.** SolidWorks's equal spacing is FreeCAD's 360° rule made explicit. **Recommended:** FreeCAD's rule, exact: a total of exactly 360° spreads n instances by 360/n; anything else spreads them by angle/(n−1). Silico's angles are whole nanodegrees, so "exactly" needs no tolerance.
4. **A copy off the body.** SolidWorks's behaviour for a boss instance that doesn't touch the body (a new body, or an error) wasn't confirmed: *unknown*. Silico has one solid per feature (S5), so it refuses either way.
5. **Merge solids.** Silico has no bodies yet. A whole-solid mirror is merged, as FreeCAD's.

## 7. What Silico does (recommended)

### The document

Three feature types, each consuming its `base` (D2) and making one solid. Every field is written (D6), and lengths and angles are `Length` and `Angle`.

```json
{"id": "t0000001", "type": "linear_pattern", "v": 1, "name": "holes", "base": "q0000001",
 "copies": {"type": "features", "features": ["q0000001"]},
 "directions": [
   {"along": "x_axis", "reversed": false, "spacing": {"type": "extent", "length": 40}, "count": 3, "skip": []}
 ]}
{"id": "t0000002", "type": "polar_pattern", "v": 1, "name": "bolt circle", "base": "t0000001",
 "copies": {"type": "features", "features": ["q0000002"]},
 "axis": "z_axis", "reversed": false, "spacing": {"type": "extent", "angle": 360}, "count": 6, "skip": []}
{"id": "t0000003", "type": "mirror", "v": 1, "name": "other half", "base": "t0000002",
 "copies": {"type": "solid"}, "plane": "yz"}
```

- **`base`**: the solid patterned on, the last feature's as for any feature.
- **`copies`**: either `features`, a set of earlier features whose tools are copied, or `solid`, the whole of `base`'s solid. A feature in `features` must be in `base`'s chain: `base` itself or a feature its base chain reaches. Otherwise the original isn't in the solid, and the first instance wouldn't be it. Copying a feature reads its tool and consumes nothing, so D2 holds.
- **`directions`**: one or two. Each has:
  - `along`: `x_axis`, `y_axis`, `z_axis`, or an element name (a straight edge or a planar face) of `base`'s solid, resolved in its element map as a fillet's edges are (M2.8), or a broken reference with candidates;
  - `reversed`;
  - `spacing`: `{"type": "extent", "length"}`, first to last, or `{"type": "spacing", "spacing"}`, between neighbours;
  - `count`, the original included;
  - `skip`.
- **`axis`**, for polar: an origin axis, or a straight or circular edge of `base` (a circle's axis, through its centre).
- **`plane`**, for mirror: `xy`, `xz`, `yz`, or a planar face of `base`.
- **Instance positions are computed from their index, never accumulated.** Linear instance k is at `k · length / (count − 1)` or `k · spacing`, from the integer nanometres. Polar instance k is at `k · angle / count` for exactly 360°, else `k · angle / (count − 1)`, or `k · spacing`, in nanodegrees, converted to radians at the boundary with the libm shim. So no instance's place depends on another's rounding.

### The kernel

- **One new request**, as dumb as the others (invariant 2): `Op::Copies { base, tool, op: Fuse | Cut, placements: Vec<Placement> }`. `Placement` is a translation, a rotation about an axis by radians, a mirror in a plane, or a translation of a rotation for a grid.
- **The worker:**
  - makes each copy of `tool`: a location for a rigid move, as `translate` does today; a rebuilt copy (`BRepBuilderAPI_Transform` with copy) for a mirror, since `from_brep` refuses a location that mirrors (tessellation.md), which is FreeCAD's `makeElementTransform` rule;
  - runs **one boolean**: `base` the argument, each copy a separate tool (`SetTools`), as the cut already expands a compound tool (booleans.md, M3.6). Never one compound of copies, which OCCT cuts wrongly when they overlap;
  - returns the result and its history, with `base` as input 0 and copy k as input k. Each copy's elements are numbered as the tool's, so the core names copy k from the tool's map without asking.
- **No fuzzy value, no parallel mode, no refine,** as Silico's other booleans (booleans.md, S8). The oracle sets `Refine = False`.
- **Several originals are one step each, in timeline order** (FreeCAD's per-original loop, main's ordering). A pad's copies are fused, then a later pocket's cut. Each step is keyed and cached like any other (M2.5).
- **The tool** is the original's sweep step result (the `Prism`, `PrismThroughAll`, or `Revolution` key the original's plan already has), so it's never rebuilt. For `solid`, it's `base`'s result.
- **The result check** is every operation's (fillet.md): valid where it changed, and whole if its inputs were.

### The refusals

The gate refuses, naming the field:
- `count` below 1 or above the cap;
- a `length`, `spacing`, or `angle` of 0 or less;
- a polar angle over 360;
- a spacing angle whose instances pass a whole turn;
- an empty `features`, or one listed twice;
- a feature that isn't a pad, pocket, revolve, or groove (decision 2);
- a feature not in `base`'s chain;
- a `skip` of 0, out of range, or repeated.

At rebuild, it refuses:
- a reference of the wrong kind, naming the element and its kind ("`b.box.Edge(…)` is a circle; a direction is a straight edge or a planar face");
- a result of more than one solid (S5), naming the instances whose copies make the extra solids;
- a pattern whose result changes nothing (S7's volume rule);
- a single instance that changes nothing (decision 6), naming it, with the suggestion to skip it.

### The naming

§3: a copy marker with the instance's index. The original's names are unchanged.

### The tests

Expected values from hand or analytic results, or FreeCAD as the oracle.

**Volumes:**
- §5's ported cases: 10000, 4000, 2000, 2.
- The probe's cylinder pattern: 20000 + 3·π·9·5 = 20424.115008.
- Three overlapping holes: 518.2217 removed, by integration.
- A mirror 1 mm off a cylinder's axis: 20200.241263, the lens by hand.

**Positions:**
- Each copy's centre, by hand.
- 360°/4 at 0, 90, 180, 270.
- 180°/4 at 0, 60, 120, 180.
- `reversed` the other way.
- A grid's (i, j).

**Refusals:**
- Each row of §2's table.
- Copies off the base.
- A pocket's copy that misses.
- Copies cutting the base in pieces.
- A circle as a direction.
- −360 written as 360 with `reversed`.

**Names:**
- Copy k's faces carry `Ck`, and the original's are byte for byte the names without the pattern.
- Adding an instance at the end renames none.
- Changing direction 2's count renames none of direction 1's.
- Skipping instance 2 renames instance 3 not at all.
- A fillet on copy 2's edge follows a change of length and breaks, loudly, when the count drops below 3.
- A fillet on a mirrored copy's edge (`testMultiTransformDressup`'s rule).

**Determinism:** a pattern rebuilt twice, and on both checked platforms, byte for byte (the mirror's rebuilt copy included).

**Oracle:** a new file in `tests/oracle/`, which the maintainer adds, with `Refine = False` on every FreeCAD feature, comparing volume, area, centre of mass, the tight box, counts, and validity.

**`/break-it`:** counts at the cap, a kilometre of length, a turn of 360° less a nanodegree, a mirror plane through the original, a grid of overlapping copies, and a reference that breaks.

## Open decisions

None is decided. Each has a recommendation for the step's design and the maintainer to take, change, or refuse.

1. **Three features or one.** FreeCAD has a feature each (and MultiTransform); SolidWorks has a feature each. **Recommended:** three types, `linear_pattern`, `polar_pattern`, `mirror`, sharing `copies` and the kernel's `Copies` request. No MultiTransform: a linear pattern's second direction covers its common use, and a pattern of a pattern later covers the rest.
2. **What can be copied.** FreeCAD copies any additive or subtractive feature, dress-ups included (what they added and removed), and the whole solid. **Recommended:** in the first step, pads, pockets, revolves, and grooves, whose tools exist as steps, and `solid`. Fillets and chamfers are refused, naming them: a dress-up's "added and removed" is two more booleans per original, and SolidWorks's geometry pattern refuses faces merged with the part. A `fuse` or `cut` (whose tool is a feature's solid) and a pattern of a pattern follow as their own step.
3. **Directions and planes.** FreeCAD takes sketch axes, sketch lines, datums, edges, and faces; a face's direction is its surface's axis whatever the face's orientation *(1.1.3)*. **Recommended:** the origin's axes and planes, and named elements of `base`: straight edges, planar faces, and, for polar, circular edges. A face's direction is its **outward normal** (as `describe` gives it, M4.4), so a box's bottom face patterns downwards, as a person picking it means. That's a deliberate difference from FreeCAD, tested both ways in the oracle. Sketch lines as references come once a sketch's elements can be named outside its features.
4. **Spacing.** FreeCAD has Extent and Spacing, per-gap `Spacings` (unchecked: zeros and negatives taken), and an experimental `SpacingPattern`. **Recommended:** extent and spacing only, each a positive `Length` or `Angle`, with the sense in `reversed`. No per-gap spacings until someone needs them (SolidWorks's "instances to vary" is the model then). Through all copies as it is (geometry pattern), and a copy that then doesn't reach through is found by decision 6.
5. **Polar's whole turn.** FreeCAD treats `|angle − 360| < 1e-7°` as a whole turn, not −360, and takes spacings past a turn silently, so copies land on copies. **Recommended:** the angle is in (0°, 360°], the sense in `reversed`. Exactly 360° spreads `count` instances by 360/count; anything else by angle/(count − 1). A spacing with (count − 1) · spacing ≥ 360° is refused, naming `spacing`: some instance would reach or pass the original. Exact in nanodegrees, so 359.999999999° is no turn and is built as written.
6. **Copies that miss, or change nothing.** FreeCAD 1.1.3 returns a compound when copies miss (or, with `AllowCompound` off, keeps the first solid and drops the rest, a pocket pattern's 15200 mm³ included, silently), and a pocket's copy that misses removes nothing, silently. **Recommended:**
   - **More than one solid is refused** (S5), naming each instance one of whose copied elements is in a solid other than the one holding the base's, found from the history.
   - **An instance changes nothing** when none of its copy's faces survives in the result, kept, modified, or generating, also from the history. It's refused, naming it, with the suggestion to skip it.
   - A copy exactly on the original or another copy (a mirror about the original's own plane of symmetry) may be reported by OCCT as merged rather than deleted. Whether the history tells it apart is *unknown* until the step's probes, and the step should find out.
   
   S7's volume rule refuses a whole pattern that changes nothing either way.
7. **Instances to skip, and names.** FreeCAD 1.1.3 has none; main's `SuppressedIndices` keeps each index and lets the original be suppressed. **Recommended:** `skip` lists instance indices (`[2]`, or `[2, 1]` in a grid) from 1, never the original, in range, no repeats, written sorted. A copy's names carry its instance index (§3), so skipping, adding, or removing instances at the end renames no other copy, and changing direction 2's count renames none of direction 1's. FreeCAD's flat index would.
8. **A cap on instances.** 1.1.3 allows INT_MAX; main clamps to a preference, 1000 by default. **Recommended:** a fixed cap in the format, 1000 instances in all, the original included, refused above it, naming `count`, never clamped (a preference would make a document mean different things on different machines). The kernel's request carries no more than that.
9. **Mirror plane through the original.** FreeCAD's mirror about the original's plane of symmetry changes nothing, silently. **Recommended:** decision 6's rule, so it's refused where the history shows it. Mirroring a whole solid about one of its own faces fuses it into one solid, as FreeCAD does, and is allowed.

## Flags for the maintainer

These are important and hard to reverse:

- **The copy marker is a change to the naming grammar** (decision 7, §3). A document's `naming` header refuses any scheme but the current one (M4.0). Scheme 6 must read scheme 5's names as they are, so no saved reference breaks. The step's design should agree the letter, the index form (`C2`, `C2,1`), and that rule before any pattern's names are written.
- **Outward normal for a face's direction** (decision 3) departs from FreeCAD 1.1.3, which an imported FreeCAD file (docs/roadmap/freecad-import.md) would then have to translate face by face.
- **Refusing what FreeCAD builds silently** (decisions 5, 6, 9). Copies off the base, copies that change nothing, and polar patterns past a whole turn are refused. FreeCAD files that rely on them won't import as patterns.
