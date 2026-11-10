# Patterns and mirror

The linear pattern, the polar pattern, and mirror: copies of features (or of a whole solid) moved, turned, or reflected, and fused into or cut from the solid they're patterned on. ROADMAP.md puts patterns and mirror after the vertical slice, in "the full feature set" of stage 1. This note is the study they start from. It says what FreeCAD does, the edge cases, the fixes in FreeCAD's history, the tests to port, where SolidWorks differs, and what Silico should do. **The hard-to-reverse questions are decided:** the naming of copies, a face's direction, and the refusals (decisions 3, 5, 6, 7, 9, 10, and 11), by Fable, 2026-10-29, the maintainer may veto (~/.cache/silico-review/m-patterns-fable.md). The rest (decisions 1, 2, 4, and 8) are recommendations, for the step's design and the maintainer to take or change.

## Sources

- **FreeCAD 1.1.3** (tag `145529fe74`), read from a sparse clone of the tag, since the maintainer's clone has no tags. Line numbers without a prefix are 1.1.3's, in `src/Mod/PartDesign/App/`: `FeatureTransformed.cpp` (FT), `FeatureLinearPattern.cpp` (LP), `FeaturePolarPattern.cpp` (PP), `FeatureMirrored.cpp` (MI), `FeatureMultiTransform.cpp` (MT).
- **FreeCAD's main** at `876271d660` (2026-10-07), for history. FreeCAD 1.1 branched from main at `af2d42a4aa` (2025-11-13). A commit before that is in 1.1.3. After it, only three of these files' commits were picked into 1.1.3: `5ae67ee2f9` (on the branch as `9c5a6ff782`), `b0331ed979` (`6d91e63828`) and `4fae2b83f8` (`199fbfb91a`). Everything else after 2025-11-13 is main only.
- **Main differs a lot.** `c334ac5062` (2026-06, #30417) moved the linear and polar logic into Part (`src/Mod/Part/App/LinearPatternExtension.cpp`, `PolarPatternExtension.cpp`). Main also added circular, path and point patterns (`173276b175`, `c633c5c88e`, `f5abab2768`), instances to skip (`cdb4624675`), and a cap on occurrences (`293726c5d8`). **Silico follows 1.1.3**, with main's fixes where they matter (§4).
- **The oracle, FreeCAD 1.1.3**, run headless (`.pixi/envs/oracle/bin/freecadcmd`) on probe scripts in `/tmp/patprobe/`. A claim marked *(1.1.3)* was observed. A fact-check re-ran the linear, polar, mirror, pocket, and map-size probes and got the same numbers. The probes use PartDesign primitives (`AdditiveBox`, `AdditiveCylinder`, `SubtractiveCylinder`) as originals, at their own placements, in a Body with its default `AllowCompound = True` and `Refine = True` unless they say otherwise.
- **SolidWorks's help**, the 2026 pages read in full (listed in §6), and its API help for what the pages leave out.
- **The decision**, ~/.cache/silico-review/m-patterns-fable.md: Fable's, 2026-10-29, on this note at `23c4ec1a`, the maintainer may veto. It also read FreeCAD main's `LinearPatternExtension.cpp:480` (`adapt.Plane().Axis().Direction()`), and fetched SolidWorks's pages again; they settle none of the three questions.
- docs/ports/naming.md, docs/design/m2.7-naming.md (N2's "copy marker"), docs/design/m3.6-sketch-features.md (S5 one solid, S7 a change of nothing, S8 no refine), docs/ports/booleans.md (compound tools), and docs/ports/tessellation.md (mirrored locations).

## 1. What FreeCAD does

### `Transformed`, the base class

`Transformed` (FT, header `FeatureTransformed.h:40`) is a `FeatureRefine`, not a `FeatureAddSub`. Its subclasses supply only a list of `gp_Trsf` (`getTransformations`). The base class does the rest.

**Inputs.**
- `Originals`: the features to copy, a link list (FT:67).
- `TransformMode`: `Features` (copy the originals' tools) or `Whole shape` (copy the whole base solid) (FT:63; the strings were renamed in `9535371265`, same indices).
- `Refine` (true by default since `e490531510`).
- The support is the pattern's `BaseFeature`, the previous feature in the Body (`getBaseObject`, FT:84-114). An original is never the support, except in old files.
- Suppressed originals are dropped, as if they weren't listed (`getOriginals`, FT:116-138; `79f9da420c`).
- Only a `FeatureAddSub` can be an original: pad, pocket, revolution, groove, loft, pipe, helix, hole (each a `ProfileBased`, `FeatureSketchBased.h:39`), the primitives, and dress-ups. Anything else, a pattern included, is refused: "Only additive and subtractive features can be transformed" (FT:386-392).

**How the result is built** (`execute`, FT:294-439).
1. The support's shape is taken without its placement (FT:351-355). Each original's tool is moved into that frame by `feature location × support location⁻¹` (FT:400). Main's `5d8162107a` (2026-06), "PartDesign: Fix pattern failing with primitive" (no body), reverses the order. That the old order fails when the two placements don't commute, as for a primitive with its own rotated placement, is this note's reading of the diff.
2. Each original gives a fuse shape, a cut shape, or both (`getAddSubShape`, FT:394). A pad gives a fuse shape and a pocket a cut shape. A dress-up gives both by default: what it added and what it removed (`7fadfd0439`, `a59dac1675`). With its `SupportTransform` set, it gives its base feature's shapes instead, and "any dressing that is not applied to the based feature will be ignored" (`FeatureDressUp.cpp:318-362`). Neither is "Shape of additive/subtractive feature is empty" (FT:395-399).
3. **The first transformation is skipped**, so the first occurrence is the original itself, which the support already holds (FT:361-362; `a0746a0c02`). Re-fusing the original's own tool used to undo edits made between it and the pattern, such as refilling a later pocket.
4. For each original, in `Originals`' order, the copies of its fuse shape are fused into the support in **one boolean**, the support as the argument and each copy a separate tool: `makeElementFuse({support, copy1, copy2, …})` (FT:407-413). Then the copies of its cut shape are cut the same way (FT:414-420). One boolean replaced earlier schemes that fused copies one by one or compounded them (`5ab7d17f67`, #6641).
5. The result is refined if `Refine` is on (FT:433). Then `Shape = getSolid(result)` (FT:435). The rest of the result's solids go in `rejected` (FT:436, `getRemainingSolids` FT:441-459), which the GUI reports as a warning, "%d transformed shapes do not intersect the support" (or "One transformed shape …"; `Gui/ViewProviderTransformed.cpp:141-173`).
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
- A datum line's direction. A datum plane's or origin plane's normal (`880587a627`, `cf0412b7e2`). An origin axis, its direction read through `DatumElement::getDirection()` since `b3a1fd9676`.
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
  - A step whose magnitude is under `Precision::Angular()` (1e-12 rad) is "Pattern angle can't be null" (PP:282-284). `eccc5bfe1a` moved the check to radians ("Pattern angle too small"); `5ae67ee2f9` made it `fabs(angle)` and gave it its message. *(1.1.3: 0 and 1e-12 degrees are refused.)*
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
- A face is refused, "Axis reference must be an edge", though the property's tooltip offers "the normal of a planar face". An origin plane is refused, "Axis reference must be edge of a feature or datum line". *(1.1.3, both.)* A PartDesign datum plane is a `Part::Feature` (`Part/App/DatumFeature.h:40`), so it doesn't reach that message: with no sub-element it's "No axis reference specified" (PP:225-228), and with one, "Axis reference must be an edge" (PP:252).
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
  - an origin axis: "Mirror plane reference must be a sketch axis, a face of a feature or a datum plane". (A PartDesign datum line falls to "No mirror plane reference specified" or "No direction reference specified", MI:146-151.)
- **The copy is a real copy.** `makeElementTransform` sees a negative determinant (`trsf.ScaleFactor() * trsf.HVectorialPart().Determinant() < 0.`) and rebuilds the geometry with `BRepBuilderAPI_Transform` instead of setting a location (`src/Mod/Part/App/TopoShapeExpansion.cpp:3439-3459`; the rule came with the TopoShape transfer, `a90ad10d60`, 2024-02). Rebuilding shows round-off: a 10 mm-thick box mirrored whole about x = 100 has a bounding box from z = −4.6e-14 *(1.1.3)*.
- **Edge cases** *(1.1.3)*:
  - A plane through the original's own plane of symmetry puts the copy on the original, and the mirror changes nothing.
  - A plane 1 mm off a cylinder's axis makes the copies overlap; the fused volume matches the analytic lens, 20200.241263.
  - A whole-shape mirror about the base's own face fuses the two halves into one solid. About a plane 50 mm away it gives two solids, reported valid.

### `MultiTransform` (MT)

- **What it is.** A list of `Transformed` children, `Transformations`. Their transformation lists are combined: every new transformation multiplied with every old one, new outer and old inner (`nt * ot`, MT:175-183), so the count is the product. `Scaled` is combined diagonally instead (MT:120-166) and needs the centre of mass of the first original (MT:82-93, "will NOT give the expected result for more than one original").
- **The children.** They take the MultiTransform's placement (MT:48-66; `afb3355193`) and build nothing themselves. They're recognised only by having empty `Originals` (above), and their own `Shape` stays null *(1.1.3)*.
- **Bearing on Silico.** It only matters for how indices compose. A 3 × 2 MultiTransform of two linear patterns gives the six positions of one two-direction linear pattern *(1.1.3)*. **Recommended:** don't port it as a feature. A linear pattern's second direction covers its common use, and a pattern of a pattern is two features in the timeline.

## 2. Edge cases and refusals

| Input | FreeCAD 1.1.3 *(observed)* | Silico (§7, as decided or recommended there) |
|---|---|---|
| 0 or −1 occurrences | Clamped to 1 by the property; nothing happens | Refused by the gate (`invalid_input`, naming `count`): a count is at least 1 |
| 1 occurrence, or every copy skipped | The identity; nothing else checked | The identity: no copies request, the base's solid and names pass through, its references still resolved and checked (decision 6) |
| A huge count | Up to INT_MAX (main caps at a preference, `MaximumPatternOccurrences`, default 1000, `293726c5d8`) | Refused above a fixed cap (decision 8) |
| Length 0 (extent) | "Pattern length too small" | Refused by the gate, naming `length` |
| Spacing 0 | "Pattern length too small" (through the synced length) | Refused by the gate, naming `spacing` |
| Negative length or spacing | Clamped to 0, then refused | Refused, naming it: `reversed` says the sense |
| A per-gap spacing of 0 or negative | Taken: copies on copies, or backwards | Not ported (decision 4) |
| No direction | A zero step: every copy on the original, no error | The gate refuses the missing field |
| Direction a circle, a curved face, a vertex | Refused, by message | Refused, naming the element and its kind |
| Direction a planar face | The surface's axis, whatever the face's orientation | The face's outward normal, as `describe` reports it; a face with no outside refused (decision 3) |
| Polar angle 0 or 1e-12° | "Pattern angle can't be null" | Refused by the gate, naming `angle` |
| Polar angle −360, or a spacing reaching a whole turn | Copies land on the original or on each other, silently | Refused by the gate, in whole nanodegrees; −360 with the advice to write 360 with `reversed` (decision 5) |
| Polar angle 359.9999999 | A step of a third; the last copy 1e-7° off the original | Exactly as given: a step of a third, 359.9999999 written. Only 360 exactly is a whole turn |
| Polar axis a face or an origin plane | Refused | Refused, naming it |
| Mirror plane a curved face, an edge, an origin axis | Refused (an edge with "TopoDS::Face") | Refused, naming the element and its kind |
| A copy that doesn't touch the base (additive) | `AllowCompound` on: a compound of several solids, "valid". Off: the first solid only, the copies dropped silently | Refused, `kernel_failed`: more than one solid (S5), naming the instances (decision 6) |
| A pocket's copy that misses the base | Silent: that copy removes nothing | Built, exactly as asked, with a warning, `instances_changed_nothing`, naming its instance (decision 6) |
| A slot that cuts the base in two, patterned so its copy cuts it again | A compound of three solids, "valid". `AllowCompound` off: the first, 3717 of 18917 mm³, kept without a word | Refused, `kernel_failed`: the base cut in pieces (S5), no instance named (decision 6) |
| A copy on the original (spacing 0, a symmetric mirror) | Silent: it fuses into the original | A spacing of 0 refused by the gate; a pattern that changes nothing refused by volume, as S7 refuses a pad (decisions 6 and 9) |
| Two parallel directions | Not checked: copies on copies | Refused at rebuild (decision 11) |
| Copies that overlap each other | Fine: separate tools in one boolean. Three holes r 3, 2 mm apart: 518.221711 mm³ removed, against 518.221713 by integration | The same: separate tools (booleans.md, M3.6), never one compound |
| Copies that touch face to face | Fuse into one solid (10 boxes of 10 along X: 10000, 42 faces unrefined) | The same |
| An original that isn't additive or subtractive | "Only additive and subtractive features can be transformed" | The gate refuses it, naming it (decision 2) |
| Features mode with no originals | Nothing happens; the previous shape kept | The gate refuses an empty `features` |
| Originals in another order | Applied in the list's order (main sorts by the Body's order, `6265896ca8`) | Applied in timeline order, whatever the list says |
| A pattern patterned again | MultiTransform, or a later pattern in Whole shape mode; a pattern as an original is refused ("Only additive and subtractive features …") | A pattern of a pattern is later; not in the first step (decision 2) |

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

FreeCAD's tests pin only refined map sizes: 26, a plain box's (§5). The linear and polar tests' TODOs give unrefined sizes, 170 for ten boxes in a row and 66 for four turned; `TestMirrored.py` gives none. *(1.1.3, with `Refine = False`: 170, 66, and 42 for a mirrored box.)*

On main, instances to skip keep their index, so the surviving copies keep their names (`cdb4624675`). The index is still flat: `testSuppressionKeepsGridPosition` remaps `SuppressedIndices` `[5]` to `[6]` when `Occurrences2` goes from 3 to 4.

### What Silico names (decided)

Silico's N2 (a kept element keeps its name exactly) is safe only because an element's lineage is a tree: a solid is consumed once (M2.1's D2), so one origin can't reach a shape twice. A pattern breaks that. Each copy of a pad's side face would be `p.pad.Face(G s.sketch.Edge.c)`, the same as the original's. naming.md and the M2.7 design reserved "a copy marker" for this.

The marker is decided: Fable, 2026-10-29, the maintainer may veto (~/.cache/silico-review/m-patterns-fable.md, §1(b)). Scheme 5's grammar (in `naming/parse.rs`) gains one form and three type words:

```text
base     = …                                                scheme 5's five forms, unchanged
         | feature "." type "." kind "(" ( "C" | "S" ) instance " " name ")"
                                      a copy of `name` placed at `instance`: `C` of an element
                                      of a feature's tool, `S` of an element of the base's solid
instance = offset ( "," offset )*     one offset per direction of the feature, in the
                                      document's order (direction 1 first), trailing zeros
                                      left off: the last offset written is 1 or more
offset   = "0" | digit19 digit*       at most 2^32 − 1, no leading zero
digit19  = "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"
digit    = "0" | digit19
type     = … | "linear_pattern" | "polar_pattern" | "mirror"
```

The grammar as built is N10 in docs/design/m2.7-naming.md, with its refusals (Fable, 2026-11-10: the `S` marker and the nonterminal `instance`, named apart from `parse.rs`'s `index`, an element's number).

The rules:
1. **An offset is steps from the seed along a direction, from 0.** Instance k of a linear or polar pattern is k steps along the direction as the document gives it. `reversed` is part of the direction, not of the index. The seed is offset 0 in every direction. The index is the multiplier in the position arithmetic (§7), and main's `SuppressedIndices` counts the same way (0 the original). An index counts steps along `directions` by place, as written. A direction's fields, `along` included, and the directions' order are the pattern's values: an edit to them moves instances and renames none (Fable, 2026-11-10).
2. **The seed is unmarked.** Its elements are the base's and keep their names exactly (N2, N7): it is in the base, and the pattern's booleans bite the base. A fillet on the original's edge is untouched by adding a pattern after it. No `C0`, `C0,0`, or `C1,0` is ever written, and the parser refuses them (and `C`, `C02`, `C2,`): one spelling per instance.
3. **A copy's element is the tool's element, wrapped once:** `t0000001.linear_pattern.Face(C2 p0000001.pad.Face(G s0000001.sketch.Edge.6t7v8w9x))`. Copy k's elements are numbered as the tool's, so the core names them from the tool's map without asking the kernel. It never depends on OCCT's lists. In the structured type it is one new variant, `Name::Copy { feature, operation, kind, copied, index: Vec<u32>, of: Arc<Name> }`, `copied` saying which was copied (`Copied::Tool` for `C`, `Copied::Solid` for `S`), whose `parts()` is its inner name: a copy of an unstable name is unstable, and the depth bound counts a level per copy.
4. **A mirror's copy is index 1: `C1` of a tool, `S1` of the solid.** A mirror is a pattern of two instances, with no separate mark. If a mirror ever takes a second plane (SolidWorks has one), its copies are `C1`, `C0,1`, and `C1,1` by the same rule.
5. **A grid writes direction 1's offset first:** `C2`, `C0,1`, `C2,1`. Never a flat index: FreeCAD's `i·n₂ + j` renames every copy past the first row when `Occurrences2` changes, and main's `testSuppressionKeepsGridPosition` has to remap it.
6. **Nested patterns nest the form, the copy applied last outermost.** A whole-solid mirror of a solid holding a pattern names the pattern's copy 2 `t0000002.mirror.Face(S1 t0000001.linear_pattern.Face(C2 p0000001.pad.Face(G …)))`, and the seed `t0000002.mirror.Face(S1 p0000001.pad.Face(G …))`. A pattern of a pattern as a feature (later, decision 2) nests the same way. No composite index is ever invented.
7. **Whole-solid copies wrap the base's names, marked `S`:** `t0000003.mirror.Face(S1 k7f2qx3m.box.Face6)`. `C<instance> <name>` is a copy of an element of a feature's tool, and `S<instance> <name>` a copy of an element of the base solid, under the same instance grammar and refusals (Fable, 2026-11-10). The base's own elements keep their names.
8. **A copy's elements pass through the pattern's boolean as a sweep's pass through its own.** The pattern's steps bite every input, base and copies (`OpCode::side` is `Outside` for every input at every stage). So a copy's element kept whole, or modified one from one, keeps its `C` or `S` name. One the base or another copy splits is `t.linear_pattern.Face(M1/2 t.linear_pattern.Face(C2 …))`. A section with the base is `t.linear_pattern.Edge(G k7f2qx3m.box.Face6, G t.linear_pattern.Face(C2 …))`. No `I` ever appears: the letter fixes the side. An `S` copy is always fused; a `C` copy's side is its inner type word's (a pad's or a revolve's fused, a pocket's or a groove's cut), which is M3.6's reason a sweep's cut writes no `I`.
9. **`skip` holds the same indices as the names,** by the same rule, one array per instance, so an error's `elements` paste into it: "add 3 and 2,1 to skip" is `"skip": [[3], [2, 1]]`. Sorted numerically, never the seed, each in range, no repeats.
10. **Mirror changes no name otherwise.** A reflection reverses orientation, and the history is still one element to one.

**What stays and what breaks**, for the edits users make (the decision's table):

| Edit | Copies' names |
|---|---|
| `count` up (either direction) | unchanged; new instances get new names |
| `count` down | unchanged for instances that remain; a reference to one removed breaks, loudly, with no silent re-point |
| `skip` an instance, or un-skip it | nothing else renamed; the skipped name is absent, and comes back as it was |
| `length`, `spacing`, `angle`, `reversed`, the plane, the axis | unchanged: the instance moves, as FreeCAD's and SolidWorks's do |
| a second direction added, or removed while direction 1 stays | unchanged: instance (2) and instance (2, 0) are one instance (rule 1 with trailing zeros off), so a fillet on copy 2 survives the user adding direction 2 in the same dialog |
| the two directions swapped, or direction 1 removed with direction 2 kept | kept by index: `C2,1` is two steps along the new direction 1 and one along the new direction 2; with direction 2 the one direction left, `C1` is one step along it, and `C0,1`, two offsets in a pattern of one direction, finds nothing: `reference_broken`, with candidates |
| `copies.features` reordered | nothing: the originals are applied in timeline order, whatever the list says |
| an original removed from `copies.features` | its copies' names gone, loudly; the others unchanged |
| `copies` switched between `features` and `solid` | every copy's name breaks, loudly (`C` to `S` or back), with candidates |
| a seed feature's ID or type changed | as for every name today |

**Why 0-based with the seed unmarked.** The index is the multiplier in the position arithmetic, as main's `SuppressedIndices` counts, and it makes "trailing zeros off" natural: the default is no steps, not one. SolidWorks shows instance coordinates in its Instances to Skip box, but its pages don't say the convention, and nothing in Silico's names needs to match SolidWorks's display.

**Why trailing zeros off.** Always one offset per direction is one rule fewer, but renames every copy when a direction is added, a one-field edit users make after they've filleted copies. With zeros off the same instance keeps the same name under either number of directions. The parser enforces one spelling, so no two names are written alike.

**The scheme** is decision 10: scheme 6, a strict superset of scheme 5, whose names are read as they are.

## 4. FreeCAD's history

Each commit below was read from its diff. Formatting, includes, renames, translations and modernising are left out.

| Commit | Date, issue | What it fixed or changed | 1.1.3? | In 1.1.3's code? |
|---|---|---|---|---|
| `32628462c2` | 2012-10 | `Part::checkIntersection`. A copy not meeting the support warned "does not intersect support: Removed"; a copy overlapping the original was an error | yes | No |
| `515f30c1a0` | 2012-12, Mantis #906 | Patterning a non-PartDesign object segfaulted on a null tool; now "Shape of additive feature is empty" or "… subtractive …" (1.1.3 joins them, FT:397) | yes | Yes, FT:394-399 |
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
| `63ad26491f` | 2016-08 | `Scaled` with no transformations divided by zero | yes | Yes, MT:130-135 |
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
| `8453d41515` | 2021-02 | "Fix regression for Mirrored, and multiple features": every copy made with `BRepBuilderAPI_Transform` instead of `Move`, and the identity included again (undone by `a0746a0c02`) | yes | Superseded: 1.1.3 copies through `makeElementTransform` |
| `a90ad10d60` | 2024-02 | TopoShape's transform transferred to Part: a transform that mirrors or scales (`ScaleFactor() * HVectorialPart().Determinant() < 0.`, or a scale) rebuilds the geometry instead of setting a location | yes | Yes, `TopoShapeExpansion.cpp:3439-3459` |
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
| `2b97b20a03`, `25ba8ab223`, `a1ce983035` | 2024-08 | Naming of copies (§3). `2b97` left the support out of the fuse and included the identity; `25ba` also did the cut as a fuse; `a1ce` fixed all three | yes | Yes, FT:357-371 |
| `cf8ad66373` | 2024-10, #17119 | Booleans through FreeCAD's wrappers, with a fuzzy value | yes | Yes |
| `b3a1fd9676`, `077745d737` | 2024-12, 2025-03 | `DatumElement::getBasePoint()` and `getDirection()` added "to hide implementation details and guarantee consistent behaviour", fixing regressions in `PolarPattern::getTransformations` among others; a polar axis on an origin line turned about the origin | yes | Yes, PP:218-224 |
| `5d2037c820` | 2025-08 | The Transform rework: Extent and Spacing; linear's second direction; `Spacings` and `SpacingPattern`; the default occurrences 3 → 2; the step clamped at one occurrence | yes | Yes |
| `0f07a936d9` | 2025-09 | Polar spacings off by one | yes | Yes, PP:306-311 |
| `7e57b6e7b0` | 2025-09, #23637 | A missing direction gives a zero step instead of an error; origin axes link with an empty sub-name | yes | Yes, LP:271-274 |
| `5ae67ee2f9` | 2025-11, #25621 | Polar angle constraints removed: negative angles reverse; −360 isn't a whole turn; the null check on the step's magnitude, "Pattern angle can't be null" | picked | Yes, PP:282-284 |
| `b0331ed979` | 2026-01, #26722, #24700 | A sketch edge as a polar axis left the axis zero and failed | picked | Yes, PP:189-206 |
| `4fae2b83f8` | 2026-01 | The preview places tools in the support's frame | picked | Yes, FT:226-282 |
| `123a142292` | 2025-12 | Message: "No features selected to be mirrored/patterned/transformed" | main | No |
| `99422b51cc` | 2026-05 | With `AllowCompound`, `rejected` emptied, so no false "do not intersect" warning | main | No: 1.1.3 warns falsely |
| `5d8162107a` | 2026-06 | "Fix pattern failing with primitive": the tool's transform multiplied the other way (FT:400 and the preview's) | main | No |
| `6265896ca8` | 2026-06, #30548 | Originals sorted by the Body's order, so selection order no longer changes the result | main | No |
| `6523471254` | 2026-06 | MultiTransform children no longer recompute twice | main | No |
| `cdb4624675`, `5cf3c96149` | 2026-09, 10, #32843 | `SuppressedIndices`: instances to skip, each keeping its index; skipping 0 removes the original's material | main | No |
| `293726c5d8` | 2026-09, #33056 | Occurrences capped at a preference, `MaximumPatternOccurrences`, default 1000 | main | No |

**The intersection checks, in short.**
- **2012–2014.** A copy that didn't meet the support was dropped with a warning, overlap was an error, and the overlap check was removed in 2014.
- **2017–2018.** Rejection became an error, and so did more than one solid.
- **2021–2022.** Everything was removed for one multi-tool boolean, keeping the first solid.
- **Since 2025.** `AllowCompound` defaults to true, so 1.1.3 returns a compound of whatever the boolean gives.

FreeCAD tried to check, and every check it had was removed: `03da9d3501` let a copy miss the base "for performance reasons but it can also be considered a feature", and `5ab7d17f67` replaced detection to fix a failing polar pattern (#6641).

## 5. FreeCAD's tests

1.1.3's tests (`src/Mod/PartDesign/PartDesignTests/`) are each a cube (10 mm, or 1 mm for the mirror's three), a box primitive or a padded square, with its volume as the only real check. The map sizes they assert are refined (26, a plain box's). They're expected values Silico can use, unrefined, with the counts from the probe.

| Test | Asserts | Port? |
|---|---|---|
| `TestLinearPattern.py` `testXAxisLinearPattern`, `testYAxis…`, `testZAxis…` | a 10 cube, 10 occurrences over 90 along an origin axis: volume 1e4, map 26 (refined) | **Yes**: 10000, one solid, unrefined 42 faces, 84 edges, 44 vertices *(1.1.3)*. Ten copies touching face to face |
| `testNormalSketchAxis…`, `testVerticalSketchAxis…`, `testHorizontalSketchAxis…` | the same with a padded square, along the sketch's N, V, H axes | Yes, when sketch lines can be directions (decision 3); until then, the origin axes they equal |
| `TestPolarPattern.py` six tests | a 10 cube turned 4 × about each origin axis and sketch axis, 360°: volume 4000 | **Yes**: 4000, one solid (four cubes meet at the axis), unrefined 16, 32, 18 *(1.1.3)* |
| `TestMirrored.py` `testMirroredSketchCase`, `testMirroredPrimitiveCase` | a unit cube mirrored about the sketch's V axis, or XY: volume 2 | **Yes**: 2, one solid; a 10 cube about XY gives 2000, unrefined 10, 20, 12 *(1.1.3)* |
| `testMirroredOffsetCase` | a unit square at (0, 1)–(1, 2), padded 1 and mirrored about the sketch's H axis, so the copy lies 2 mm from the original: only "Up-to-date" | **Yes, as a refusal**: two solids, refused (decision 6). FreeCAD accepts the compound |
| `TestMultiTransform.py` `testMultiTransform` | mirror × linear 3 × polar 4 of a 10 cube: 20000 | As a pattern of a pattern, later; or as one two-direction linear pattern with its own expected value |
| `testMultiTransformDressup` | two mirrors, then a fillet of two faces of the result (r 3): a closed-form volume. `AllowCompound = False`, the MultiTransform and both mirrors refined, and the fillet's base `(MultiTransform, ["Face1", "Face2"])`, two faces of the one merged box | **Yes, later**, for its volume, whose formula is independent. It doesn't test naming a copy's elements: the refined result has no copies left to name |
| `testMultiTransformBody` | empty (`pass`, a TODO) | No |
| Main's `TestLinearPattern.py` suppression tests (`testSuppressionKeepsGridPosition`, `…Persistence`, `testSuppressOriginal…`, 10 in all) | instances to skip: grid positions kept (the flat index remapped when a count changes), the original suppressible; volumes and `isInside`, no names | **The first** (positions kept when a count changes) with decision 7, as positions; names are Silico's own tests. Not the original's suppression (decision 7 refuses it) |
| Main's `testOccurrencesAreClampedToMaximumPatternOccurrences` | the cap | As decision 8's refusal, not a clamp |
| `TestTopologicalNamingProblem.py` | one `Transformed` test, `testPartDesignElementMapTransform` (a MultiTransform of a `Scaled` on a box, map 26), in 1.1.3 and main; no linear, polar, or mirror test | Silico's own (§7) |
| `tests/src/Mod/Part/App/FeatureMirroring.cpp` | Part's `Mirroring` (a shape's mirror, not PartDesign's) | Its geometry cases, as kernel tests of the mirror request |

There are no C++ tests of PartDesign's patterns (1.1.3's `tests/src/Mod/PartDesign/App/` has BackwardCompatibility, DatumPlane, Pad, and ShapeBinder; main adds Pipe and Revolution, among others).

## 6. SolidWorks

Read in full from help.solidworks.com, the 2026 help (fetched with a browser's user agent, the page's text taken from its embedded data; the 2027 copies of the PropertyManagers are the same text, and the 2017, 2018 and 2024 copies say the same where they overlap):
- [Linear Patterns and the Linear Pattern PropertyManager](https://help.solidworks.com/2026/english/SolidWorks/sldworks/t_Linear_Patterns_Overview.htm) (and the [2017 PropertyManager](https://help.solidworks.com/2017/English/SolidWorks/sldworks/hidd_lpattern.htm));
- [Circular Pattern PropertyManager](https://help.solidworks.com/2026/english/SolidWorks/sldworks/HIDD_CPATTERN.htm), [Circular Patterns](https://help.solidworks.com/2026/english/SolidWorks/sldworks/t_Circular_Patterns_Overview.htm), and [Bidirectional Circular Patterns](https://help.solidworks.com/2026/english/SolidWorks/sldworks/t_bidirectional_circular_pat.htm);
- [Mirror Feature](https://help.solidworks.com/2026/english/SolidWorks/sldworks/c_Mirror_Feature_Overview.htm) and the [Mirror Feature PropertyManager](https://help.solidworks.com/2026/english/SolidWorks/sldworks/hidd_dve_feat_mirror.htm);
- [Geometry Pattern](https://help.solidworks.com/2026/english/SolidWorks/sldworks/c_Geometry_Pattern_Concept.htm), [Selecting Instances to Skip](https://help.solidworks.com/2026/english/SolidWorks/sldworks/t_selecting_instances_to_skip_patterns.htm), [Pattern Deletion](https://help.solidworks.com/2026/english/SolidWorks/sldworks/c_Pattern_Deletion_Overview.htm), [Limits on Number of Instances](https://help.solidworks.com/2026/english/SolidWorks/sldworks/c_limit_number_instances.htm), and [Patterns and Mirroring](https://help.solidworks.com/2026/english/SolidWorks/sldworks/c_Patterns_and_Mirroring_top.htm);
- the API help's [`IFeatureManager::FeatureCircularPattern5`](https://help.solidworks.com/2026/english/api/sldworksapi/SolidWorks.Interop.sldworks~SolidWorks.Interop.sldworks.IFeatureManager~FeatureCircularPattern5.html) and [`ILinearPatternFeatureData::GeometryPattern`](https://help.solidworks.com/2026/english/api/sldworksapi/SolidWorks.Interop.sldworks~SolidWorks.Interop.sldworks.ILinearPatternFeatureData~GeometryPattern.html).

**Linear pattern.**
- One or two directions. Each has a pattern direction ("Select a linear edge, line, axis, dimension, planar faces and surfaces, conical faces and surfaces, circular edges, and reference planes"), with Reverse Direction, a spacing ("the spacing between pattern instances"), and a number of instances, which "includes the original features or selections". The pages don't say which way a planar face or a circular edge sets the direction. Direction 2 has "Pattern seed only" and "Symmetric".
- **Instances to Skip:** "Skips the pattern instances that you select in the graphics area … The coordinates of the pattern instance appear." A deleted instance is listed by its location too ("Select the coordinates of the deleted instance in the Instances to Skip box", Pattern Deletion), and modified instances "are identified by their column and row number in the pattern". The pages don't say what becomes of a skipped instance's coordinates when a count changes.
- **Geometry pattern:** "Creates the pattern by using only the geometry (faces and edges) of the features, rather than patterning and solving each instance of the feature. Geometry pattern speeds up the creation and rebuilding of the pattern. You cannot create geometry patterns of features that have faces merged with the rest of the part." The Geometry Pattern page: it "copies, but does not solve, the patterned feature. End conditions and calculations are ignored. Each instance is an exact copy of the faces and edges of the original feature." **Without it, each instance is solved**: in the page's example, a cut extruded "Offset from Surface" is, in each instance, "offset from the selected surface by the same amount (the end condition is solved)". The pages don't say which is the default.
- **A limit:** "The upper limit on the number of instances for patterns is 65500." For a large number, "a dialog box prompts you to confirm or revise the number."

**Circular pattern.**
- A pattern axis ("Axis, Circular edge, Linear edge or linear sketch line, Cylindrical face or surface, Revolved face or surface, Angular dimension"), with Reverse Direction; an angle; and a number of instances, "the number of instances of the seed feature". The API says the count includes the seed: `Number` is the "Number of instances of the circular pattern to insert in Direction 1, including the original instance". The bidirectional page agrees: 4 instances in Direction 1 made symmetric give "Three instances of the same seed … in the reverse direction".
- **Equal spacing** "Sets Angle to 360°", and the PropertyManager says Angle "Specifies the angle between each instance". The API says what the angle is in each mode: "Spacing between each instance in Direction 1 of the circular pattern or, if EqualSpacing is true, then the total angle". The pages don't say how instances are spread over a total angle, at 360° or under it.
- A second direction (bidirectional, symmetric or not), and instances to skip, as linear.

**Mirror.**
- A mirror face or plane ("Select a plane or planar face"), and features, faces, or bodies to mirror. A secondary plane may be given ("mirror an item about two planes at a time in one feature").
- **Geometry Pattern**: "Mirrors only the geometry (faces and edges) of the features, rather than solving the whole feature", with the same limit on merged faces.
- **Merge solids** "(Available when mirroring solid bodies.) Merges the seed and mirrored solid bodies into one solid body."

**Bodies.** "When pattern or mirror features split single bodies into multiple bodies, you can select which bodies to keep" (Patterns and Mirroring). None of these pages says what happens when a boss instance misses the body.

**Where it differs from FreeCAD, and which to follow.**
1. **Re-solving each instance.** Silico's sweep extents are a length, two lengths, symmetric, or through all, none up to a face, so a copy of the tool and a re-solved instance agree, but for **through all**: the original's through-all prism is sized by its own base, and a copy moved far along may not reach through. **Recommended: copy the tool** (FreeCAD's way, SolidWorks's geometry pattern, which "copies, but does not solve" and ignores end conditions). It's one boolean, deterministic, and cached, and a through-all copy that no longer cuts at all is warned of by decision 6's `instances_changed_nothing` (decision 4). One that cuts only part way isn't caught: a geometry pattern ignores end conditions the same way, which is this note's reading of SolidWorks's page, to document. Re-solving comes with up-to-face extents, if ever.
2. **Instances to skip.** SolidWorks and main have them; 1.1.3 doesn't. SolidWorks lists them by their coordinates in the pattern. **Decided: port**, by the names' instance indices, never the seed (decision 7).
3. **Equal spacing.** SolidWorks makes it a switch: with Equal spacing the angle is the total (set to 360°), without it the angle between instances. That's Silico's `extent` and `spacing`. Within a total angle, FreeCAD decides from the value: exactly a turn spreads n instances by 360/n, anything else by angle/(n−1). SolidWorks's pages don't say how it spreads them, so this rule rests on FreeCAD alone. **Decided** (decision 5): FreeCAD's rule, exact: a total of exactly 360° spreads n instances by 360/n; anything else spreads them by angle/(n−1). Silico's angles are whole nanodegrees, so "exactly" needs no tolerance.
4. **A copy off the body.** SolidWorks's behaviour for a boss instance that doesn't touch the body (a new body, or an error) isn't in the pages read: *unknown*. A pattern or mirror that splits a body leaves SolidWorks's user to pick the bodies to keep. Silico has one solid per feature (S5), so it refuses either way (decision 6). Letting the user pick bodies waits for bodies: refusing now and loosening then is the reversible order.
5. **Merge solids.** Silico has no bodies yet. A whole-solid mirror is merged, as FreeCAD's.

## 7. What Silico does

### The document

Three feature types, each consuming its `base` (D2) and making one solid. Every field is written (D6), and lengths and angles are `Length` and `Angle`.

```json
{"id": "t0000001", "type": "linear_pattern", "v": 1, "name": "holes", "base": "q0000001",
 "copies": {"type": "features", "features": ["q0000001"]},
 "directions": [
   {"along": "x", "reversed": false, "spacing": {"type": "extent", "length": 40}, "count": 3, "skip": []}
 ]}
{"id": "t0000002", "type": "polar_pattern", "v": 1, "name": "bolt circle", "base": "t0000001",
 "copies": {"type": "features", "features": ["q0000002"]},
 "axis": "z", "reversed": false, "spacing": {"type": "extent", "angle": 360}, "count": 6, "skip": []}
{"id": "t0000003", "type": "mirror", "v": 1, "name": "other half", "base": "t0000002",
 "copies": {"type": "solid"}, "plane": "yz"}
```

- **`base`**: the solid patterned on, the last feature's as for any feature.
- **`copies`**: either `features`, a set of earlier features whose tools are copied, or `solid`, the whole of `base`'s solid. A feature in `features` must be in `base`'s chain: `base` itself or a feature its base chain reaches. Otherwise the original isn't in the solid, and the first instance wouldn't be it. Copying a feature reads its tool and consumes nothing, so D2 holds.
- **`directions`**: one or two. Each has:
  - `along`: `x`, `y`, `z`, or an element name (a straight edge or a planar face) of `base`'s solid, resolved in its element map as a fillet's edges are (M2.8), or a broken reference with candidates. A face points along its outward normal (decision 3);
  - `reversed`;
  - `spacing`: `{"type": "extent", "length"}`, first to last, or `{"type": "spacing", "spacing"}`, between neighbours;
  - `count`, the original included;
  - `skip`: the instances left out, one array per instance in the names' index form, `[[2], [2, 1]]` (decision 7).
- **`axis`**, for polar: an origin axis, or a straight or circular edge of `base` (a circle's axis, through its centre).
- **`plane`**, for mirror: `xy`, `xz`, `yz`, or a planar face of `base`.
- **Instance positions are computed from their index, never accumulated.** Linear instance k is at `k · length / (count − 1)` or `k · spacing`, from the integer nanometres. Polar instance k is at `k · angle / count` for exactly 360°, else `k · angle / (count − 1)`, or `k · spacing`, in nanodegrees, converted to radians at the boundary with the libm shim. So no instance's place depends on another's rounding.

### The kernel

- **One new request**, as dumb as the others (invariant 2): `Op::Copies { base, tool, op: Fuse | Cut, placements: Vec<Placement> }`. `Placement` is a translation, a rotation about an axis by radians, a mirror in a plane, or a translation of a rotation for a grid.
- **The worker:**
  - makes each copy of `tool`: a location for a rigid move, as `translate` does today; a rebuilt copy (`BRepBuilderAPI_Transform` with copy) for a mirror, since `from_brep` refuses a location that mirrors (tessellation.md), which is FreeCAD's `makeElementTransform` rule;
  - runs **one boolean**: `base` the argument, each copy a separate tool (`SetTools`), as the cut already expands a compound tool (booleans.md, M3.6). Never one compound of copies, which OCCT cuts wrongly when they overlap;
  - returns the result and its history, with `base` as input 0 and copy k as input k. Each copy's elements are numbered as the tool's, so the core names copy k from the tool's map without asking;
  - reports each result solid's faces as one more raw list in `Topology` (`solid_faces`, sorted, validated like the rest). The core needs it to say which instances lie off the base, and nothing else can read it: invariant 2 keeps the decision in the core.
- **FreeCAD 1.1.3's automatic fuzzy value, no parallel mode, no refine,** as Silico's other booleans (booleans.md, S8): the box is the base's and every copy's, so copies a hair apart, under 1e-6 of it, merge as they do in 1.1.3's LinearPattern. The oracle sets `Refine = False`.
- **Several originals are one step each, in timeline order** (FreeCAD's per-original loop, main's ordering). A pad's copies are fused, then a later pocket's cut. Each step is keyed and cached like any other (M2.5).
- **The tool** is the original's sweep step result (the `Prism`, `PrismThroughAll`, or `Revolution` key the original's plan already has), so it's never rebuilt. For `solid`, it's `base`'s result.
- **The result check** is every operation's (fillet.md): valid where it changed, and whole if its inputs were.
- **Count 1, or every copy skipped, is the identity.** No copies request is sent (the references are still resolved, and so described), the base's solid and map pass through, and S7 isn't asked (it is a check of a boolean's result). FreeCAD allows it so an expression can turn a pattern off (`c001732663`), and Silico's expressions will want the same (decision 6).

**Built (the kernel layer).** `Op::Copies { base, tool, op, placements }` (`silico_protocol::copies`): `op` a `CopiesOp`, `Fuse` or `Cut`; each `Placement` a `Translation { offset }`, a `Rotation { axis, angle }` (radians, right-handed), a `Mirror { origin, normal }`, or a `TurnedAndMoved { axis, angle, offset }` (turned, then moved). `check_copies` refuses, naming `placements.N.field` (from 0): no placements or more than `MAX_COPIES` (999, decision 8's 1,000 instances less the original); a value that isn't finite, a point past a kilometre, a direction that isn't a unit vector; and a placement that leaves the copy on the tool (a move of no more than 1e-7 mm, a turn within 1e-12 rad of none or of a whole turn, either way). The worker (`make_copies`, `copies_boolean` in the bridge) places each copy by OCCT's `gp_Trsf` (a location for a move or a turn, as `translate`; `BRepBuilderAPI_Transform` with copy for a mirror), checks each copy's elements are the tool's in `TopExp::MapShapes` order, and runs one boolean, the base the argument and each copy a tool of its own (a compound copy's every shape), non-destructive and single-threaded, with the history of the base as input 0 and copy k as input k. It refuses as `boolean` does (an empty base or tool, an empty result, one inside out), words its refusals as the fuse's or the cut's, and checks the result as every operation's is, the copies taking the tool's validity. Every result's `Topology` now holds `solid_faces`, each solid's faces, sorted. The core's `KernelRequest::copies` checks the placements before sending and the reply after (a history for the base and each copy, each copy with as many elements as the first), and `FakeKernel` builds copies symbolically (`copies_fuse(base, tool, [t:…, r:…, m:…, tm:…])`), each copy's elements numbered after the one before, its measures placed. The tests (`crates/silico-kernel/tests/copies.rs`) hold §5's volumes and the note's probes by hand, each copy's place, a grid's (i, j), a turned copy turned before it's moved, a mirrored copy's faces reflected and numbered as the tool's, solids off the base with their faces, and the same bytes from two workers and the pool. `/break-it` (`adversarial_copies.rs`) found no crash, hang, or wrong result: 999 copies in one boolean take about 9 s on a loaded Mac (the worker's timeout is a minute); a turn of 360° less a nanodegree, two copies in one place, and a copy 1.1e-7 mm off the seed (past the check, but within the two copies' tolerances together) each fuse into the seed, valid, changing nothing, which S7's volume rule refuses at the feature.

### The refusals

The gate refuses, naming the field:
- `count` below 1, or more than 1,000 instances in all (decision 8);
- a `length`, `spacing`, or `angle` of 0 or less;
- a polar extent angle over 360;
- a polar spacing with (count − 1) · spacing ≥ 360°, in whole nanodegrees (decision 5);
- an empty `features`, or one listed twice;
- a feature that isn't a pad, pocket, revolve, or groove (decision 2);
- a feature not in `base`'s chain;
- a `skip` of the seed, out of range, repeated, or not in the names' one spelling (decision 7).

At rebuild, it refuses:
- a reference of the wrong kind, naming the element and its kind ("`b.box.Edge(…)` is a circle; a direction is a straight edge or a planar face");
- a face with no outside as a direction (decision 3);
- two parallel directions (decision 11);
- copies off the base, or a base cut into pieces, `kernel_failed` (S5), naming the instances for a fuse (decision 6);
- a pattern step whose result changes nothing, by S7's volume rule, `kernel_failed` (decision 6).

It warns, and builds:
- `instances_changed_nothing`, naming each instance none of whose copy's elements is in the result (decision 6).

### The naming

§3: the copy form `feature.type.Kind(C<instance> <name>)` for a copy of a feature's tool, and `feature.type.Kind(S<instance> <name>)` for a copy of the base's solid, under naming scheme 6 (decision 10). The original's names are unchanged. The step builds:
- `OpCode::LinearPattern`, `PolarPattern`, and `Mirror`, words `linear_pattern`, `polar_pattern`, `mirror`, `side` `Outside` for every input at every stage. FreeCAD's form writes the step's `FUS` or `CUT` and FreeCAD's `_k` postfix (nothing for instance 1, `_k` from 2, the flat index for a grid), for comparison only.
- `Name::Copy`, carrying `copied` (`Copied::Tool` or `Copied::Solid`), with `Display`, `parse` (the instance grammar, with `C0`, `C2,0`, `C0,0`, `C`, `C02`, `C2,` refused, and the same after `S`; a copy form under a word that doesn't copy, the wrong number of offsets for the type, and a `C` whose inner word isn't a sweep's refused too, N10), `Ord`, `Hash`, `kind`, `features`, and `parts`.
- `ElementMap::copy(feature, op, copied, index, &map)`, the tool's map or the base's: every name of every element wrapped, the unstable set carried through `parts()`. The pattern's steps use `ElementMap::operation` with the base's map at input 0 and copy k's at input k, one step per original, in timeline order, chained.

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

**Refusals and the warning** (each expected value independent, from the decision's list):
- Each row of §2's table.
- A pad's copies 3 and 4 placed off a block by hand, refused naming exactly `3` and `4`.
- A slot's copy cutting the block in three, refused with no instances named.
- A mirror about the seed's symmetry plane, and a linear pattern reversed into its base, each refused by volume.
- 20 holes at 10 mm on a 175 mm plate: built, 17 holes by volume, the warning naming `17`, `18`, `19`.
- A copy flush with the base's face, probed on the real kernel, its verdict recorded whichever way it falls.
- The gate: 6 × 90° refused naming instance 4 and "the original"; 5 × 90° refused naming instance 4; 4 × 90° allowed; 7 × 60° refused naming instance 6.
- 359.999999999° built with a step of angle/(count − 1); −360 refused with the `reversed` advice.
- Count 1 passing through with the base's names and fingerprint.
- FreeCAD's `AllowCompound = False` case (18,917 → 3,717 mm³) refused, never 3,717.
- A circle as a direction; a face with no outside; two parallel directions.

**Names:**
- Copy k's faces carry `Ck`, and the original's are byte for byte the names without the pattern.
- Adding an instance at the end renames none.
- Changing direction 2's count renames none of direction 1's.
- Skipping instance 2 renames instance 3 not at all.
- A fillet on copy 2's edge follows a change of length and breaks, loudly, when the count drops below 3, on the real kernel.
- Every row of §3's stability table, on the fake kernel.
- `naming-scheme-5.silico` untouched, read under scheme 6, every reference found on the real kernel (silico-cli's `tests/golden.rs`), and `fmt` of it differing in the `naming` line alone; `tests/naming_scheme.rs`'s pinned names byte-identical for every scheme-5 document.
- A new protected `naming-scheme-6.silico`, saved by `fmt`, with a linear pattern, a grid, a polar pattern, a mirror, a skip, a whole-solid mirror of a solid holding a pattern (the nested form), and fillets on `C2`, `C2,1`, a mirror's `C1` edge, and a whole-solid mirror's `S1` edge, each volume by hand.
- A fillet on a mirrored copy's edge (Silico's own: `testMultiTransformDressup` fillets faces of a refined result, not a copy's).

**Determinism:** a pattern rebuilt twice, and on both checked platforms, byte for byte (the mirror's rebuilt copy included).

**Oracle:** a new file in `tests/oracle/`, which the maintainer adds, with `Refine = False` on every FreeCAD feature, comparing volume, area, centre of mass, the tight box, counts, and validity.

**`/break-it`:** counts at the cap, a kilometre of length, a turn of 360° less a nanodegree, a mirror plane through the original, a grid of overlapping copies, and a reference that breaks.

## Decisions

Decisions 3, 5, 6, 7, 9, 10, and 11 are decided: Fable, 2026-10-29, the maintainer may veto (~/.cache/silico-review/m-patterns-fable.md). Decision 8 is Fable's too. Decisions 1, 2, and 4 are recommendations, for the step's design and the maintainer to take, change, or refuse.

1. **Three features or one.** FreeCAD has a feature each (and MultiTransform); SolidWorks has a feature each. **Recommended:** three types, `linear_pattern`, `polar_pattern`, `mirror`, sharing `copies` and the kernel's `Copies` request. No MultiTransform: a linear pattern's second direction covers its common use, and a pattern of a pattern later covers the rest.
2. **What can be copied.** FreeCAD copies any additive or subtractive feature, dress-ups included (by default, what they added and removed), and the whole solid. **Recommended:** in the first step, pads, pockets, revolves, and grooves, whose tools exist as steps, and `solid`. Fillets and chamfers are refused, naming them: a dress-up's "added and removed" is two more booleans per original, and SolidWorks's geometry pattern refuses faces merged with the part. A `fuse` or `cut` (whose tool is a feature's solid) and a pattern of a pattern follow as their own step.
3. **Directions and planes.** FreeCAD takes sketch axes, sketch lines, datums, edges, and faces; a face's direction is its surface's axis whatever the face's orientation *(1.1.3; main's `LinearPatternExtension.cpp:480`, `adapt.Plane().Axis().Direction()`)*. **Recommended:** the origin's axes and planes, and named elements of `base`: straight edges, planar faces, and, for polar, circular edges. Sketch lines as references come once a sketch's elements can be named outside its features. Later, and additive (adding a reference kind renames nothing): a cylindrical face as a polar axis (SolidWorks takes one; FreeCAD refuses faces), and a circular edge or a conical face as a linear direction (SolidWorks).
   - **Decided: a planar face's direction is its outward normal**, exactly the `normal` `describe` reports for it (M4.4: the surface's normal, reversed where the face is reversed in the shape, as FreeCAD's `Face.normalAt` has it). `reversed` flips it. So a box's bottom face patterns downwards. Fable, 2026-10-29, the maintainer may veto (~/.cache/silico-review/m-patterns-fable.md, §2).
   - **A face with no outside is refused.** OCCT's internal or external face, which a boolean can leave in a valid solid, is refused as a direction at rebuild: `invalid_input`, naming the element and `along`, with the suggestion to pick a face of the boundary or an edge.
   - **Why.** FreeCAD's surface axis has nothing to do with which way the face faces, and it is hidden state: M4.4 found the surface frame left-handed for chamfers, revolves, and mirrored shapes, so under FreeCAD's rule a face's direction depends on how the face was built, and can flip when an upstream edit rebuilds it. The outward normal is defined by the solid, is the arrow `describe` reports and the UI will draw, and gives one source of truth that a test or an agent can predict. SolidWorks takes "planar faces and surfaces" as a direction but its pages don't say which way; what a person picking the face means is the only reading left. For a mirror the choice is moot: a plane has no sense.
   - **A FreeCAD import corrects each face once.** The importer takes the face's outward normal and its surface axis, and where they differ, writes `reversed` the other way. Exact, per face, no tolerance. The difference from FreeCAD is tested both ways in the oracle.
   - **Edges.** A straight edge's direction is from its start to its end as `describe` gives them (its curve's own order, M4.4), and a circular edge's axis is as `describe` gives it. Those senses are the curve's, not the solid's, so a rebuild of a generated edge after an upstream edit can flip them, silently: SolidWorks's and FreeCAD's exposure too. The step tests an edge kept through booleans and a section edge on the real kernel, and records what it finds. If a flip is reproduced, the fix is to refuse edges whose sense isn't the solid's, not to guess.
4. **Spacing.** FreeCAD has Extent and Spacing, per-gap `Spacings` (unchecked: zeros and negatives taken), and an experimental `SpacingPattern`. **Recommended:** extent and spacing only, each a positive `Length` or `Angle`, with the sense in `reversed`. No per-gap spacings until someone needs them (SolidWorks's "instances to vary" is the model then). Through all copies as it is (geometry pattern). A copy that then doesn't reach through at all is warned of (decision 6); one that cuts only part way isn't caught, as SolidWorks's geometry pattern ignores end conditions ("End conditions and calculations are ignored"). Documented, not caught.
5. **Polar's whole turn.** FreeCAD treats `|angle − 360| < 1e-7°` as a whole turn, not −360, and takes spacings past a turn silently, so copies land on copies. **Decided** (Fable, 2026-10-29, the maintainer may veto (~/.cache/silico-review/m-patterns-fable.md, §3(c)); this decision confirmed as written):
   - **Extent:** `invalid_input`, parameter `angle`, for an angle of 0 or less or above 360. Exactly 360° spreads `count` instances by 360/count; anything else, 359.999999999° included, by angle/(count − 1), built as written. −360 is refused, and the message says to write 360 with `reversed`. The discontinuity at 360° is SolidWorks's ("Equal spacing sets Angle to 360°") and FreeCAD's, and in whole nanodegrees it needs no tolerance.
   - **Spacing:** `invalid_input`, parameter `spacing`, when (count − 1) · spacing ≥ 360°, in whole nanodegrees, at the gate. The message names the instance that lands: "polar_pattern t0000002: 6 instances 90 degrees apart reach 450 degrees, a whole turn or more: instance 4, at 360 degrees, lands on the original" (or "on instance 1", when it lands exactly on an earlier instance's angle; else "past the original"). The suggestion: "use at most 4 instances at 90 degrees, or give the total angle with extent".
   - **Why refuse here, when a linear copy past the base's end is only warned of.** Past a turn, copies land on copies exactly, one tool coincident with another in one multi-tool fuse, which is the fragile case for OCCT and never what anyone meant; FreeCAD builds it silently *(1.1.3)*. It is decidable exactly from integers at the gate, before any kernel call, so the refusal costs nothing and can't be spurious. This likely departs from SolidWorks, which (as far as its pages say) would wrap; ARCHITECTURE says to depart where the reference is fragile, and to say so.
6. **Copies that miss, or change nothing.** FreeCAD 1.1.3 returns a compound when copies miss (or, with `AllowCompound` off, keeps the first solid and drops the rest, a pocket pattern's 15200 mm³ included, silently), and a pocket's copy that misses removes nothing, silently. **Decided** (Fable, 2026-10-29, the maintainer may veto (~/.cache/silico-review/m-patterns-fable.md, §3(a) and §3(b)); this replaces the note's earlier recommendation, which refused a single idle instance):
   - **Copies off the base are refused**, `kernel_failed`, the code S5 already uses for a pad whose result is two solids, and M2's booleans. No new code. For a fuse (a pad's, revolve's, or `solid`'s copy that misses): "linear_pattern t0000001 makes 3 solids: the copies of instances 3 and 4 don't touch the base, or meet it only along an edge or at a point". `elements` are the instances whose copies lie in a solid other than the one holding the base's elements, as index text (`3`, `2,1`, as `skip` takes them), the first hundred and `elements_total`. The suggestion: "lower count so every instance touches the base, add them to skip, or move the pattern onto the base". A fuse never splits its base, so the solid holding the base's elements is well defined.
   - **A base cut into pieces is refused**, `kernel_failed`: "linear_pattern t0000001 cuts its base in 3 pieces: the result is 3 solids". No instance is named: every piece holds base faces, and no exact rule picks a culprit. The suggestion: "shorten or move the copies so the base stays one piece".
   - **A pattern that changes nothing is refused by volume**, as S7 refuses a pad: S7's rule, unchanged, applied to each pattern step as to a sweep. V(result) − V(base) at most `MEASURE_PRECISION` × V(base) for a fuse step, V(base) − V(result) for a cut step. `kernel_failed`: "polar_pattern t0000002 changes nothing: the result's volume is the base's, 8000 mm³ (14 faces against the base's 14)". The suggestion: "if every copy lies inside the base or misses it, change the direction, length, count, or plane; a mirror about the original's own plane of symmetry changes nothing". No history witness is needed for it.
   - **A single idle instance is a warning, not a refusal:** a new code, `instances_changed_nothing`, one per pattern. `elements` are the instances none of whose copy's elements is in the result (no face kept, modified, or generating, by the fates), with `elements_total`. The message: "linear_pattern t0000001: the copies of instances 17, 18, and 19 add nothing (no face of theirs is in the result)" ("remove nothing" for a cut step). The suggestion: "lower count or add them to skip if they aren't meant; a copy inside the base, or a pocket's copy that misses it, is built exactly as asked".
   - **Count 1, or every copy skipped, is the identity** (§7): no copies request is sent, and S7 isn't asked.
   - **Not "let the user choose which bodies to keep"**, as SolidWorks does: Silico has one solid per feature (D2) and no bodies. Refusing now and loosening when bodies exist is the reversible order. Accepting a compound now, or FreeCAD's `AllowCompound = False` first solid (3,717 of 18,917 mm³ kept without a word), is not.
   - **Why a warning for one instance.** The geometry is exact: a pocket's copy past the plate's edge removes nothing, which is exactly what "20 holes at 10 mm on a 175 mm plate" says, and a perforated plate or a grille is drawn that way on purpose. FreeCAD's history tried refusing (2012, `32628462c2`), erroring (2017, `ff188383a1`), and warning (2021), and settled on building it ("it can also be considered a feature", `03da9d3501`); SolidWorks's pages don't say. Refusing would make `skip` a stale encoding of the base's size: lengthen the plate and the holes that now fit stay skipped, silently, a wrong result from a right input. With a warning, lengthening the plate makes the holes appear, and the warning goes. `check` and CI list it. Turning a warning into a refusal later would break documents, while the other way wouldn't, so this is also the reversible order.
   - **What the warning can miss.** M3.6's S7 found a history witness fails for a whole feature: a pad reversed into its base from the base's top leaves 7 faces at the base's volume, the split top attributed to both inputs. So a copy flush with a face of the base may escape the warning. That loses a warning, not a build, and the geometry is exact. The step probes the flush case on the real kernel and records the verdict.
7. **Instances to skip, and names.** FreeCAD 1.1.3 has none; main's `SuppressedIndices` keeps each index and lets the original be suppressed. **Decided** (Fable, 2026-10-29, the maintainer may veto (~/.cache/silico-review/m-patterns-fable.md, §1(b))): a copy's name carries its instance index, 0-based steps per direction, trailing zeros dropped, the seed unmarked, a mirror's copy `C1`, a grid writing direction 1 first, nested patterns nesting (§3). `skip` uses the same indices, one array per instance: `[[2], [2, 1]]`, sorted numerically, never the seed, each in range, no repeats. (The note's earlier `[2]` and `[2, 1]` were ambiguous between two skips and one grid cell.) So skipping, adding, or removing instances at the end renames no other copy, and changing direction 2's count renames none of direction 1's. FreeCAD's flat index would.
8. **A cap on instances.** 1.1.3 allows INT_MAX; main clamps to a preference, 1000 by default (`293726c5d8`). SolidWorks's "upper limit on the number of instances for patterns is 65500", and it asks the user to confirm a large number. **Decided** (Fable, 2026-10-29, the maintainer may veto (~/.cache/silico-review/m-patterns-fable.md, "Elsewhere")): a fixed cap in the format, 1,000 instances in all, the original included, refused above it, naming `count`, never clamped (a preference would make a document mean different things on different machines). The kernel's request carries no more than that. The cap is reversible in the direction that matters: raising it invalidates no document. Until it's raised, a grille of 40 × 40 is refused, so the number is revisited on the first real request rather than discovered.
9. **Mirror plane through the original.** FreeCAD's mirror about the original's plane of symmetry changes nothing, silently. **Decided** (Fable, 2026-10-29, the maintainer may veto (~/.cache/silico-review/m-patterns-fable.md, §3(b))): decision 6's volume rule refuses it, as S7 refuses a pad that changes nothing, with no history witness. Mirroring a whole solid about one of its own faces fuses it into one solid, as FreeCAD does, and is allowed.
10. **The naming scheme.** A new form in the grammar is a change of scheme (`naming::NAMING_SCHEME`, `"silico naming 5\n"`, and `NAMING_SCHEME_NUMBER`, now 5, which the header records), and today a document of another scheme is refused (M4.0). docs/ports/naming.md left the next bump to say how scheme 5's names are read, "a migration of the names, or refusal". **Decided** (Fable, 2026-10-29, the maintainer may veto (~/.cache/silico-review/m-patterns-fable.md, §1(a))): neither.
    - **Scheme 6 is a strict superset of scheme 5:** one form added (the copy form, §3) and three type words, nothing changed or removed. Every scheme-5 name is a scheme-6 name, written byte for byte alike, naming the same element. So scheme-5 names are read as they are: no migration, no refusal.
    - `NAMING_SCHEME` becomes `"silico naming 6\n"`, `NAMING_SCHEME_NUMBER` 6. `UNRECORDED_NAMING_SCHEME` stays 5, as its doc comment promises.
    - The reader accepts a `naming` of 5 or 6 (a `READABLE` list beside `NUMBER`), and none as 5. Every save writes 6. The refusal for any other scheme is unchanged. An older Silico refuses a document written under 6, naming it, which is right: it can't read a `C` name.
    - **`naming-scheme-5.silico` stays untouched, as the proof:** read under 6, every reference in it finds its element on the real kernel, and `silico fmt` of it differs from the file in one line, `"naming": 5` to `"naming": 6`.
    - **Why not migrate or refuse.** A migration with nothing to map is code that can only be wrong. Refusal is loud but needless, and would break every document saved so far. The 4-to-5 bump set the precedent: it renamed no element, and every pre-M4.0 golden model still rebuilds to its fingerprints. naming.md's worry, that a missing field mustn't be taken as proof of scheme 5 for a document written after M4.0, is moot under read-as-is: no name is mapped, only parsed, and the parser refuses what it can't read.
    - **Scheme 7.** Read-as-is holds only while each bump is a superset. If 7 ever changes a name's meaning, that bump has to refuse the unrecorded case, which is why `UNRECORDED_NAMING_SCHEME` stays pinned at 5 now.
11. **Two parallel directions.** FreeCAD doesn't check: direction 2 along direction 1 puts copies on copies, silently, decision 5's case in linear clothing. **Decided** (Fable, 2026-10-29, the maintainer may veto (~/.cache/silico-review/m-patterns-fable.md, "Elsewhere")): refused at rebuild, `invalid_input`, parameter `directions.2.along`, when the two resolved unit directions are parallel within `Precision::Angular` (1e-12, OCCT's own, named and bounded). It's a tolerance because no exact rule exists for two floats from the kernel.

## Open questions, for PR B

- **How instances are read when the base splits.** The refusals and warnings above read each result solid's faces (`solid_faces`) to say which copies lie off the base, assuming the base's own faces stay in one solid. Under the fuzzy value every boolean runs with (booleans.md), a far copy grows the box, and so the fuzzy value, for the whole pattern: a pin r 2 on a 20 × 20 plate, copied 1e-3 along x and a kilometre away (f about 1 mm), fuses the pin and its near copy into one, the volume right (400 + 32π), but parts that pin from the plate it stood on: three solids (the plate's 6 faces, two pins' 3) where the union is two. FreeCAD 1.1.3 does the same, by `fuse([near, far], -1.0)` and by a MultiTransform of two LinearPatterns (`a_copy_spread_far_grows_the_fuzzy_value_and_parts_a_near_one_from_the_plate`, crates/silico-kernel/tests/adversarial_copies.rs). The seed's own pin is a solid apart, so "copies off the base" would name the wrong instances, or none. PR B decides how the pattern step reads it: refuse a base whose faces land in more than one solid (S5's words), or name the instances by which solid holds the base's faces.

## Flags for the maintainer

Each was hard to reverse, and each is now decided, by Fable, 2026-10-29; the maintainer may veto (~/.cache/silico-review/m-patterns-fable.md):

- **The copy marker changes the naming grammar** (decisions 7 and 10, §3): scheme 6, a strict superset of 5, which reads scheme 5's names as they are, so no saved reference breaks.
- **Outward normal for a face's direction** (decision 3) departs from FreeCAD 1.1.3. A FreeCAD import (docs/roadmap/freecad-import.md) corrects each such face once, by `reversed`.
- **Refusing what FreeCAD builds silently** (decisions 5, 6, 9, 11). Copies off the base, a base cut into pieces, patterns that change nothing, polar spacings that reach a whole turn, and two parallel directions are refused. FreeCAD files that rely on them won't import as patterns. A single instance that changes nothing is built, with a warning, as FreeCAD builds it.

## Protected files the implementation touches

Each changes by addition, and each needs the maintainer's OK on its PR:

- `crates/silico-core/tests/contract/document-schema.json`: its `naming` admits 6 as well as 5 (decision 10).
- `crates/silico-core/tests/contract/warning-codes.txt`: the new code, `instances_changed_nothing` (decision 6).
- `crates/silico-core/tests/contract/naming-scheme-6.silico`: new, the scheme-6 proof (§7's tests).

`naming-scheme-5.silico` isn't edited. No error code is added.
