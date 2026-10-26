# Booleans

Fuse, cut, and common of two shapes, ported in M1.5 as a kernel operation (`Op::Boolean`, `silico_kernel::boolean`), with a translation (`Op::Translate`) to place shapes, since every primitive sits at the origin. Each returns its result and its history for both inputs (docs/ports/history.md). Refine (merging faces that end up coplanar) is deferred; see below.

## What FreeCAD does

Two families: the Part workbench features (`src/Mod/Part/App/FeaturePartBoolean.cpp`, `FeaturePartFuse.cpp`, `FeaturePartCut.cpp`, `FeaturePartCommon.cpp`) and PartDesign's Boolean and additive/subtractive features (`src/Mod/PartDesign/App/FeatureBoolean.cpp`, `FeatureAddSub.cpp`). Both end in `TopoShape::makeElementBoolean` (`src/Mod/Part/App/TopoShapeExpansion.cpp`, around line 6082), which builds an `FCBRepAlgoAPI_*` wrapper around OCCT's `BRepAlgoAPI_Fuse`, `BRepAlgoAPI_Cut`, or `BRepAlgoAPI_Common` (`src/Mod/Part/App/FCBRepAlgoAPI_BooleanOperation.cpp`, `cf8ad66373`, 2024).

FreeCAD's wrapper sets three options on every boolean:

| Option | FreeCAD | Silico |
|---|---|---|
| `SetNonDestructive(true)` | Always (`cf8ad66373`; first in `c8ebc7f9c2`, 2016, because fuzzy booleans changed their inputs' tolerances) | **The same.** In OCCT's default mode a boolean reuses the inputs' own sub-shapes in the result, which changes the history (an untouched input face is reported as kept rather than modified, and the result shares sub-shapes with cached inputs). In non-destructive mode the history matches FreeCAD's names. |
| `SetRunParallel(true)` | Always (`e892d09f2e`, 2016; the wrapper's default since `cf8ad66373`), and `OSD_Parallel::SetUseOcctThreads(true)` process-wide (`10d95b9ce9`) | **Off.** Silico's workers are single-threaded (ARCHITECTURE.md, Deterministic kernel). FreeCAD's history has no report of parallel booleans being nondeterministic, and none proving they aren't; Silico's determinism tests decide, not hope. |
| Fuzzy value | Part features (`Part::Cut`, ...): automatic, `BooleanFuzzy (1.0) × bounding diagonal × Precision::Confusion()`, set by the wrapper's two-shape constructor (`cf8ad66373`). PartDesign: a `FuzzyTolerance` property, 0 by default, meaning OCCT's own (`73f848a3d5`, 2026), because automatic fuzzy "caused several regressions" (forum t=100093; `1d89f9c23c`, CAM). Python's `shape.cut(other)`: OCCT's own unless a tolerance is given. | **OCCT's default**, as PartDesign and Python do. A fuzzy value can come later as an explicit parameter, never as a global (FreeCAD's `withBooleanFuzzy` swaps a process-wide static). The FreeCAD oracle uses Python's booleans, so both sides use the same value. |

## Guards, from FreeCAD's history

| FreeCAD commit | Guard | In Silico |
|---|---|---|
| `e8cb83a48b` (2014, #1429) | A null base or tool is refused | Kernel: a B-rep that doesn't read is refused before OCCT builds anything. |
| `107b784d13` (2012, #684), `e490531510` (2024), `e299c46a94` | The result is checked with `BRepCheck_Analyzer` ("Resulting shape is invalid"); on by default since 2024 | Kernel: the same result check as every operation (docs/ports/fillet.md): the faces the boolean made or changed must be valid, and the whole result too when both inputs are. |
| `947f1ca235` (2025), `45543df93b` reverted by `3f5a2ae6b8` (2026, #30856) | An invalid **input** is a warning, not a refusal: a small defect elsewhere in a model blocked booleans that would have worked | The same: inputs aren't checked before the boolean. |
| `3f5a2ae6b8` | A degenerate edge with no parametric curve crashes OCCT before 7.9.3 | Not needed: Silico's OCCT is 7.9.3. |
| `e55b718cd6` (2019, #3694) | When OCCT fails, the error names an input that isn't a solid | The error says why OCCT failed (its own message) and suggests checking that the shapes overlap and are solids. |
| `488c1a1fdf` (2017), `c107ca3100` (2021) | A refine failure keeps the unrefined result | Refine is deferred (below). |
| `dfd3d86b22` (2018, #3333), `10d95b9ce9` | A result made only of loose faces is turned into a shell | Not needed yet: both inputs are solids from Silico's own operations. |
| `27c9c9d0ac`, `648bb8f5e4`, `f8f525e62d` (#17497), `abacccb690`, `c664e93bf4` | Compound tools and arguments: a compound argument is cut child by child; a compound tool is fused first; infinite recursion and single-leaf cases fixed | Not needed yet: inputs are single shapes. Compound inputs come with multi-body documents (M2); these are the cases to port then. |
| `883a726d52`, `eb52dd4a9a`, `84afc15c91` | The single-solid rule (PartDesign; off by default since 2025) | The feature layer refuses it (Fable's decision, 2026-10-18; docs/design/m3.6-sketch-features.md, S5): a `fuse`, `cut`, or `common` feature whose result is more than one solid is `kernel_failed`, naming it, and what uses it is blocked (`sweep::check_boolean_solids`). The kernel operation returns whatever OCCT builds: a fuse of separate boxes is a compound of two solids. |
| (TopoShape::makeElementBoolean, 1.1.3) | A fuse's or cut's tool that's a compound is expanded into its shapes, each a tool of its own (`expandCompound`) | The kernel's cut takes a tool compound of more than one shape as that many tools (M3.6: a pocket's or groove's sweep of several regions, whose solids may overlap, which one compound tool OCCT cuts wrongly); a compound of one is given as it is, so single-tool booleans are unchanged. A fuse's tool isn't expanded yet: none has several shapes (a pad or revolve of several regions is refused, and an M2 boolean's tool is one solid); it's expanded like a cut's once multi-region pads or revolves exist. A common's tool is never expanded, as FreeCAD 1.1.3's isn't; later FreeCAD expands it too (`c664e93bf4`), to port with compound tools. |

## Where Silico differs on purpose

- **An empty input is refused** (`invalid_input`, naming the base or the tool). OCCT fuses an empty shape with another as if it were nothing, and cuts nothing from a shape without complaint; an empty input means the operation that made it went wrong, and the boolean shouldn't hide that.
- **An empty result is refused.** A common of shapes that don't overlap, or a cut that removes everything, gives FreeCAD an empty compound (volume 0, an invalid bounding box: `FeaturePartCommon.cpp` `testNonIntersecting`). Silico refuses it (`kernel_failed`: the result is empty), as SolidWorks does: nothing downstream can use an empty shape, and an agent should know at once. The suggestion says what OCCT found ("OCCT found no overlap"), and what to do if that's wrong: OCCT can miss an overlap (below). A boolean of solids whose result has no volume to speak of is empty too: no solid, or a mean thickness (volume over area) within OCCT's tolerance, 1e-7, of none, either side. A result whose volume is further below none than that isn't empty but a solid turned inside out, and is refused as an invalid result, whatever OCCT's own check says of it: a cylinder fused with a box it touches along a line came back one shell of volume −5968 (below), and a torus of radii 200 and 100 cut by a copy of itself moved 50 one of −3.15e6. (FreeCAD refuses only a null result, `FeaturePartCommon.cpp`; both checks are Silico's, as SolidWorks refuses an empty result.) OCCT returns such things for some booleans it gets wrong, a torus cut by a copy of itself into two whole torus surfaces of volume −8e-11 with a center 2.5e11 away (mean thickness under 1e-11). Real results are far thicker however little of their box they fill: a 1e-4 sliver is 5e-5, a 0.01 mm wire 0.005, two cubes 2 m apart 0.17. The quick fixed-order volume clears most results, against the cube of their bounding diagonal (on broken shapes its error grows with the model's size: the cut of a torus of radii 200 and 100 by a copy moved 30 reads +1.9 for a volume of −1.7e-4); a result it doesn't clear, which includes one that fills little of its box, is measured precisely and judged by its mean thickness.
- **Parallel off, fuzzy at OCCT's default** (above). FreeCAD's automatic fuzzy value gets one of the known wrong results right (flat faces 2e-7 apart), but not the curved ones, and PartDesign turned it off for its regressions.
- **Volumes are measured precisely.** The summary integrates adaptively, to a relative 1e-9 (below). FreeCAD's `Shape.Volume` uses OCCT's fixed order of points a face.
- **No refine yet.** FreeCAD's Part booleans refine by default (`e77f181393`, 2025; PartDesign since `e490531510`, 2024): faces that end up coplanar, like the two halves of a fused box's top, are merged. SolidWorks merges them too. Silico doesn't yet: refine needs its own history (merged faces are recorded as modified, `d1f706050d`), and FreeCAD uses its own `BRepBuilderAPI_RefineModel` (`modelRefine.cpp`, with a long list of fixes, from `48f8d67b50` to `7ff88754d3`) rather than OCCT's `ShapeUpgrade_UnifySameDomain`. It's its own step, with its own port note, before booleans reach users in M2. Until then, results match FreeCAD's with `Refine = False`.

## History

Both inputs get a history (input 0 the base, input 1 the tool). What OCCT reports, in non-destructive mode (verified on 8.0.1 for docs/ports/history.md, and by the tests on 7.9.3):

- **A cylinder through a box (cut):** the box's untouched side faces are kept; its top and bottom faces are modified, and each generates the hole's circle edge and its seam vertex. The cylinder's side face is modified into the hole's face, and generates the two circle edges. Its caps, circle edges, and vertices are deleted.
- **A face shared by both inputs** can be modified into one result face from both (a fuse's top).
- **Positions:** a box's face that meets the cylinder generates two elements (a circle edge and a vertex), so lists longer than one appear, and FreeCAD's names record the position (`Face6;:G2(Face1;K-2...)`). This is the first operation that tests the bridge's position counting.

## Moving a shape

`Op::Translate` moves a shape by an offset, as FreeCAD places a feature's shape: a new location on the same geometry, not new geometry. The location is one transformation, the move composed with the shape's own (`TopoDS_Shape::Located`); OCCT's `Moved` (and `BRepBuilderAPI_Transform` without copying) would add to a chain instead, about 105 bytes a move, without end. A shape moved back where it was has no location again, the same bytes as before. A moved element isn't the same sub-shape (its location differs), so the history has each face, edge, and vertex modified into its moved self, numbered the same; the bridge checks that each is (the same `TShape` and orientation) rather than assuming it. A move by nothing keeps every element. The result's validity is the input's, which a move can't change. The offset must be finite and at most `MAX_LENGTH` (a kilometre) each way, as a primitive's lengths are.

This is a stopgap until placements exist (M2): it exists so booleans can be tested on shapes that aren't all at the origin.

## Known wrong results

OCCT sometimes returns a valid solid that's simply wrong, without a warning that says so. Unless it has no volume (refused as empty, above), Silico accepts it, as FreeCAD does: FreeCAD's `makeElementBoolean` accepts any valid result, and on the OCCT both run (7.9.3) every row below gives the same wrong answer in each, with the torus built as FreeCAD's `Part::Torus` builds it, which is how Silico builds it. (Silico tried refusing them during review, with OCCT's warnings, volume bounds, and points sampled on both inputs' faces; the sampling cost up to 30 s a boolean on curved models, and the maintainer chose FreeCAD's behavior.) Found by `/break-it` and the review council, with the right values from closed forms, integrals, or Monte Carlo:

| Inputs | OCCT | Right |
|---|---|---|
| A 1 × 1.0000001 × 1 box, and a unit cube at (0.5, −2e-7, 0.5): flat faces 2e-7 apart | fuse is the box alone; cut removes nothing; common empty (OCCT warns `BOPAlgo_AlertAcquiredSelfIntersection`) | fuse 1.75000015, cut 0.75000015, common 0.24999995. FreeCAD's Part features, with their automatic fuzzy value, get it right. |
| A 3 × 0.5 × 0.5 box, and a 1 × 1 × 1.0000001 box at (0, 0, −0.4999998): flat faces 3e-7 apart | fuse and common empty, refused as empty; cut removes nothing, 0.75 (OCCT warns `BOPAlgo_AlertBadPositioning` and `BOPAlgo_AlertAcquiredSelfIntersection`). `Part::MultiFuse`, with its automatic fuzzy value, fuses it to 1.49999996 | fuse 1.5000001, cut 0.5, common 0.25 (`boxes_with_flat_faces_3e7_apart_are_right_or_occts_known_wrong_answer`) |
| A cylinder (r 1, h 2) and a cone (r 1 to 0, h 2), the cone's axis 1e-7 or 2e-7 along x | fuse is the cone alone; cut removes nothing; common empty | fuse 2π, cut 4π/3, common 2π/3 |
| Two unit spheres, the second at (1, −1e-7, 0.5) | treated as apart: fuse 8.378, common empty | the lens is 1.0423 |
| Two tori (R 2, r 1), the second moved along y (0.5, 0.7, or 1), or (R 1.5, r 0.5) moved 0.3 to 0.5 along y | taken as one (fuse is one torus), or as apart; some cuts and commons come out with no volume, and are refused as empty | by symmetry, what the same move along x gives, which OCCT gets right for these (`torus_booleans_occt_gets_right_are_accepted_and_right`) |
| Two tori (R 1.5, r 0.5), the second moved 0.8 along y | fuse 8.951, cut 1.549, common 5.854 | fuse 11.773, cut 4.371, common 3.031 |
| Two tori (R 1.5, r 0.5), the second moved 0.3 along x | common 6.277 (fuse 8.528, cut 1.125); a mesh of it measures 6.270, so it's the shape, not the measurement | common 5.6236 (integrated), 5.622 ± 0.005 (Monte Carlo) |
| A torus (R 1.5, r 0.5) and a torus (R 2, r 1) at (−0.3568, −0.4123, −1.4127) | treated as apart: fuse 46.881, common empty | common 0.1122 (Monte Carlo) |
| A torus (R 2, r 1) and a 2 mm cube at (0.999999, 0, 0) | treated as apart: fuse 47.478 | as at (1, 0, 0), within a millionth |
| A tube (r 1 to 5, z −4 to 4) whose outer wall has a band of arc turning 1e-6 rad (radius 150, 1.5e-4 high), revolved a whole turn about z, less the box beyond y = 0.5; the same with the arc's circle reaching the axis or not (a torus), and from its angle 0 | the tube whole, 603.186, valid, 14 faces, with no warning; turning 1e-5 rad, or a band of line, it's right. Found from radius 10 to 1,000, turning 5e-9 to 8e-6 rad | 333.873 (`a_band_turning_a_micro_radian_is_cut_by_a_box_or_is_occts_known_wrong_answer`) |

OCCT also returns an invalid fuse, which Silico refuses (and FreeCAD's `Shape.fuse` returns as it is, invalid), for a cylinder touching a box's face along a line from outside: one shell, its volume A − B, or below zero. Five of SolidCraft's random boolean identity cases (its kernel's fuzz generator, seeds 424, 753, 768, 773, 817), run through Silico and FreeCAD 1.1.3 alike (2026-10-18). A cylinder along x is a rectangle revolved about its axis, as SolidCraft's generator and Silico's test build it: its seam isn't where `BRepPrimAPI_MakeCylinder` puts it, and the third case depends on that (built by `Part.makeCylinder` along x, OCCT fuses it right, two solids of 15687.776); the first two fail either way. A report to OCCT should give the revolve.

| Inputs | OCCT's fuse (both) | Silico | Right |
|---|---|---|---|
| A box 30 × 12.5 × 15 at (2.5, 5, 7.5); a cylinder r 7.5 along x from (27.5, 12.5, 30), 20 long, on the box's top | invalid, volume 2090.708 | refused, invalid result | 9159.292 |
| A box 30 × 10 × 10 at (0, 7.5, 17.5); a cylinder r 2.5 along x from (17.5, 20, 22.5), 20 long, on the face y = 17.5 | invalid, 2607.301 | refused, invalid result | 3392.699 |
| A cylinder r 7.5 along x from (27.5, 17.5, 20), 27.5 long; a box 22.5 × 27.5 × 17.5 at (10, 10, 27.5), on the cylinder's top | invalid, −5968.474 | refused, invalid result (a solid inside out, above) | 15687.776 |
| A box 15 × 27.5 × 27.5 at (22.5, 12.5, 0); a cylinder r 5 along z at (27.5, 7.5, 10), 25 high, on the face y = 12.5 | invalid, 9380.255 | refused, invalid result | 13307.245 |
| A box 17.5 × 22.5 × 12.5 at (7.5, 30, 17.5); a cylinder r 7.5 along z at (22.5, 22.5, 15), 7.5 high, on the face y = 30 | invalid, 3596.516 | refused, invalid result | 6247.234 |

Their commons are empty and refused as empty, and their cuts are the first shape whole, as they should be (`a_cylinder_tangent_to_a_boxs_face_along_a_line_never_fuses_wrong`). A cylinder inscribed in a box, touching it from inside, OCCT gets right (`cylinder_inscribed_in_a_box_tangent_to_four_faces`). These are for reporting to OCCT too.

The tori moved along y cross Silico's torus's seam, which is on its inner equator, as `Part::Torus` builds it; along x, OCCT gets most of them right, but not all (0.3 above). OCCT's own torus (seamed on the outer equator) gets some of these right and fails others, so the seam isn't shown to be the cause. These are for reporting to OCCT.

A torus (R 12.531, r 9.057) less a torus (R 23.399, r 12.985) moved (−3.965, 0.164, 1.576), the oracle sweep's `generated_15_96` (seed 15):
- **OCCT 7.9.3 finds no intersection** and takes the tool as inside the base: fuse is the base alone (20290.10), common the whole tool (77877.31).
- **FreeCAD 1.1.3's cut** is one solid turned inside out, two whole toroidal faces of volume −57587.21 (the base less the whole tool), which its `isValid` and `check(True)` pass. That's with the torus seamed on its inner equator, as `Part::Torus` and Silico build it; `Part.makeTorus`, seamed on the outer equator, gives an empty cut instead.
- **Silico** refuses it as an invalid result (a solid inside out, above).
- **The right answer:** the axes are 3.968 apart, and the tool's tube comes no nearer its axis than 10.414. So everything of the base within 6.446 of its axis is left: at least 908 mm³ by the integral, and about 5854 by Monte Carlo.
- **Pinned by** `a_torus_cut_by_a_larger_torus_occt_turns_inside_out_is_right_or_refused` (right or refused, never inside out or empty); the case is in the oracle's known list for generated cases (`generate::KNOWN`).

Silico ran OCCT 8.0.1 until M1.5. It gets all of these wrong too, and one more that 7.9.3 and 7.8.1 get right: a box cut across the slit of a revolve within about 1.5e-6 degrees of a full turn (the rectangle x 100..200, y 0..1 about y by 360 − 1e-7, and a 0.5 × 0.5 × 1 box at (120, 0.2, −0.5)) removes nothing, in a stock 8.0.1 build and in FreeCAD 26.3 on it alike. That regression is why Silico runs 7.9.3, the same release as its FreeCAD oracle (`a_cut_across_a_large_nearly_full_ring_removes_what_it_covers` checks 7.9.3 cuts it right).

**Volumes are integrated adaptively.** OCCT's default integration, a fixed number of Gauss points a face, which FreeCAD's `Shape.Volume` uses, measures some results up to 1% off: Silico's common of two tori (R 2, r 1) 2.2 apart measured 12.238 where its volume is 12.3586 (FreeCAD, whose OCCT builds that common with one more face, measures 12.3598), and a cylinder and torus at (0, 1.999999, 0) measured 43.769 for a fuse of 43.089. The summary now integrates to a relative 1e-9: 12.3586093, and 43.0891. Volumes and areas agree with FreeCAD's to about 2e-8 on the oracle tests; a horn torus's center came out 7.5e-6 off its axis (of a torus 40 across), where the fixed order is exact, until patch 0003 (2026-10-25: docs/ports/measure.md) gave its surface of revolution three spans: now it's at the axis to 1e-11 (1e-12 of its radius). The stress test measures on its own, so its reference doesn't change.

**Invalid inputs are accepted**, as FreeCAD accepts them (`3f5a2ae6b8`): a defect elsewhere in a model shouldn't block a boolean. Their results can be wrong too: a common of the open box (a box missing a face) and a cube reaching past the missing face comes back valid with volume 64, where at most 32 is possible.

**The two-input changed-faces check** (`is_valid_where_changed_from_two`) is tested only in the accepting direction: no boolean is known that leaves a changed face invalid.

For the in-process API only: a boolean of a shape with itself, or with its own moved copy, shares sub-shapes OCCT then treats as shared (every element kept); the worker reads each input from its own bytes, so it never sees that.

**Moving a shape renames nothing in FreeCAD** (`TopoShape::makeElementTransform` without copying keeps the element map). Silico's translate reports each element modified into its moved self; the placements of M2 must report a moved element as the same element, not build names on these fates.


## Tests

In `crates/silico-kernel/tests/boolean.rs`, and hostile inputs in `adversarial_boolean.rs` (identical, touching, tangent, and nested operands, gaps and overlaps around the tolerance, splits, scale, and moves). From FreeCAD, with expected values:

| Test | Status |
|---|---|
| `tests/src/Mod/Part/App/FeaturePartCommon.cpp` `testHistory` (two 1 × 2 × 3 boxes, the second at (0, 1, 0); recovered from `ecf7e51ab3^`): base faces `{0:[0], 1:[5], 2:[], 3:[2], 4:[3], 5:[1]}`, tool faces `{0:[0], 1:[5], 2:[4], 3:[], 4:[3], 5:[1]}` (0-based input → result) | Ported (`freecads_common_history_test`): OCCT 8.0.1 gives exactly these. |
| `FeaturePartFuse.cpp`, `FeaturePartCut.cpp`, `FeaturePartCommon.cpp` with the six-box fixture (1 × 2 × 3 boxes at (0,0,0), (0,1,0), (0,3,0), (0,2,0), (0, 2+1e-7, 0), (0, 2−1e-4, 0)): fuse volumes 9, 12, 12, 12, 12 − 3e-4; cut 3, 6, 6, 6, 6 − 3e-4; common 3, empty, empty, empty, 3e-4 | Ported (`freecads_part_boolean_tests`), with Silico refusing the empty commons, and `testMapping`'s 26 elements. |
| `TopoShapeExpansion.cpp` `makeElementCut` (0.75), `makeElementFuse` (1.75), `makeElementBooleanCommon` (0.25) on two unit cubes offset by (0.5, 0.5, 0): volumes, and the history their names state | Volumes ported; the history their names state is checked by the oracle test below on the same kind of case. |
| `src/Mod/PartDesign/PartDesignTests/TestBoolean.py`: two 10³ boxes offset by 5 in x: fuse 1500, cut 500, common 500 | Ported (`freecads_partdesign_boolean_and_topo_shape_tests`). |
| `src/Mod/PartDesign/PartDesignTests/TestPrimitive.py`: each primitive minus a smaller one sharing its origin: 11³ − 10³, π·10·(11² − 10²), 4/3·π·(6³ − 5³), cones with a shared apex, tori | Ported (`subtracting_a_primitive_from_a_larger_one_sharing_its_origin`: coincident faces, a shared apex, hollowed solids). |
| `FeaturePartFuse.cpp` `testRefine`, `TopoShapeMakeElementRefine.cpp`, `ModelRefine.cpp`, and the refined counts in `FeatureFillet.cpp` / `FeatureChamfer.cpp` | With refine. |
| `FuzzyBoolean.cpp` (cylinder fused with a helix: fuzzy 10 clean, 0.01 faulty, 1e10 fails); `makeElementBooleanAcceptsShapeRejectedByAnalyzer` | With a fuzzy parameter, and with imported shapes. Their fixtures are FreeCAD B-rep files; copying them into `tests/oracle/` needs the maintainer's approval. |
| `testPartCompoundCut1/2`, `makeElementBooleanCommonWithCompoundTool`, `FeaturePartFuse.cpp` `testCompound` and `testRecursiveCompound` | With compound inputs (M2). |
| `TopoShapeExpansion.cpp` `makeElementBooleanImpossibleCommon` (volume 0, an empty map) | A deliberate difference: Silico refuses the empty common. |
| `src/Mod/Part/parttests/TopoShapeTest.py` `testPartCommon`, `testPartCut`, `testPartFuse`, `testTopoShapeFuse`, `testTopoShapeCommon`, `testTopoShapeCut` (bounds, and element-map sizes 58, 26, 26 without refine) | Not yet: their counts are covered by the oracle test's. Their bounds can be checked now that the kernel measures a tight box (`measure_as_freecad`, M1.8): port them in M1.9. |
| `PartDesignTests/TestBoolean.py` `testBooleanCutUses*Placement`, `testBooleanFirstFeature*` | With bodies and placements (M2). |

And Silico's own:

- **FreeCAD as an oracle** (`booleans_match_freecad_with_the_history_its_names_state`): fourteen booleans of placed primitives (cubes, boxes, a cylinder through a box, a sphere and a cylinder, a box through a cylinder's side, a torus sliced by a slab, a cone and a sphere), each compared with FreeCAD's Python boolean (OCCT's own fuzzy value, as Silico's) on volume and area (to 1e-6: FreeCAD measures with a fixed order), counts, validity, and history. FreeCAD's element names state, for each result element, its first source with its position in OCCT's list (`Face6;:G2`), and up to four more sources with theirs (`(Face3;K-3;...)`, negative for generated); every one of those facts is in Silico's history, position included, and the kept elements are exactly FreeCAD's. Facts past position 1 occur, so the bridge's counting is checked.
- **From the roadmap, by hand:** `boolean_fuse` (1875), `boolean_common` (125), `boolean_cut` (875), each with its area, center, and counts (`overlapping_cubes_match_a_hand_calculation`); `box_minus_cylinder` (1000 − 40π) with `cut_history` (`a_cylinder_cut_through_a_box_leaves_a_hole_with_the_history_the_geometry_says`).
- **Refusals FreeCAD never tests:** an empty input, an empty result (whose suggestion says what OCCT found), an offset that isn't finite or is too far, an invalid result, each with a suggestion, and the worker answering the next operation after each.
- **An invalid input** isn't refused when the boolean leaves its problem where it was (the open box, cut on its far side).
- **Determinism:** the same boolean twice gives the same bytes and history; through a worker, the same as in-process from the same B-rep.
