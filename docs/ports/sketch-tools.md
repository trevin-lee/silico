# Sketch tools: the relations they infer

Sources: the study `g5-redundant-autoconstraints-study.md` and Fable's decision (2026-11-08, `g5-redundant-autoconstraints-fable.md`); FreeCAD 1.1.3 (tag `145529fe74`, fetched from GitHub; not in the maintainer's clone) and main at `876271d660`. Paths are under `src/Mod/Sketcher/`; line numbers are 1.1.3's unless marked *main*. SolidWorks' help says nothing on this (docs/ux/solidworks.md §2; docs/study/sketch-tool-relations.md §1), so FreeCAD 1.1.3 is followed where it is the fallback (that study's §10).

## 1. Relations the sketch already implies

### What FreeCAD does

FreeCAD's drawing tools leave out an auto-constraint the sketch already implies, by one of two mechanisms.

- **The H/V rule** (`Gui/Utils.cpp:619-696`, `removeRedundantHorizontalVertical`). It runs before the suggestions become constraints and removes the end's `Horizontal` or `Vertical` suggestion when:
  - both ends are coincident, directly or through other coincidences, with external geometry, the origin included; or
  - one end is coincident with the origin and the other is on an axis.
  
  It never removes the point's relation.
- **The solver filter** (`Gui/DrawSketchDefaultHandler.h:961-1026`, `removeRedundantAutoConstraints`). The sketch, with the shape's own constraints already in it, is diagnosed with the auto-constraints appended (`SketchObject::diagnoseAdditionalConstraints`, `App/SketchObject.cpp:1427-1443`). They get the highest tags, so PlaneGCS's popularity contest, on a tie in popularity and rows, flags them before the sketch's own (`planegcs/GCS.cpp:5494-5528`). Then:
  - every redundant auto-constraint is removed;
  - partially redundant ones are kept;
  - every auto-constraint in a conflicting group is removed (since `f9eaf9b568`);
  - a flagged constraint that isn't an auto-constraint throws, and the tool adds none of its auto-constraints.
- **The preference.** *Avoid redundant* (`AvoidRedundantAutoconstraints`, default true; `Gui/ViewProviderSketch.cpp:311-315`, `:562-566`) gates only the H/V rule, and only with auto-constraints on (`6a6c34480a`; `DrawSketchDefaultHandler.h:638-648`). It has had no UI since `41a7b146f5`. The solver filter always runs.
- **Which tool uses which.**
  - Line: rule, then filter.
  - Polyline (and its arcs): the rule only, line segments only (`Gui/DrawSketchHandlerLineSet.h:592-611`, `:676-693`). Main's new polyline uses the filter (`d77c9647c3`).
  - Rectangle, circle, arcs, polygon, slot, arc slot, ellipse, B-spline: the filter.
  - Point: neither.
- **The order** (`DrawSketchDefaultHandler::finish`, `:509-545`): the geometry and the shape's own constraints are committed; the widget's constraints are added; then the auto-constraints are generated, ruled, and filtered; then they're added in one transaction. A throw while generating keeps the geometry and drops the auto-constraints.

### Edge cases

- **The L from the origin.** The polyline keeps P2's on-axis and P6's on-axis, and drops line 1's horizontal and the closing line's vertical (the rule's second case; the closing coincidence makes the first point's origin coincidence reach the rule through `getAllCoincidentPoints`).
- **Both ends on one axis, no origin.** The Line tool drops the horizontal (the filter: the newest tag). The 1.1.3 polyline keeps all three.
- **A direction between two fixed points that doesn't hold.** FreeCAD drops it: the rule's first case, or the filter's conflict branch.
- **Overlapping groups.** One diagnosis can flag fewer constraints than there are dependencies, since the contest is greedy and one constraint in two groups satisfies both. So one pass can leave the batch redundant (rows y1, y2, y1 − y2, y3, y2 − y3).
- **A redundancy the sketch already had** is listed again and isn't an auto-constraint, so the newer tools add no auto-constraints to such a sketch.
- **Fewer rows wins.** An existing one-row constraint is flagged before a new two-row one (contest step 2), which throws.
- **Both redundant and conflicting in one diagnosis.** The conflict loop's indices are taken before the redundant erasures, so it can erase the wrong auto-constraint (`:1013-1023`; main `Gui/DrawSketchHandler.cpp:1566-1578`, unfixed).
- **A tool's own relation that repeats an inferred one.** It can't be filtered, and throws. The slot's fix makes the alignment an auto-constraint instead (`af4ed78c01`).
- **The rule ignores which axis**, and its erase loop skips one element after an erase. Both are harmless.

### Bug fixes from FreeCAD's history

| Commit | What it fixed |
|---|---|
| `93721e8ca9` | Coincidence through other points (`getAllCoincidentPoints`); flags reset each call |
| `c093cb1821` | The old check in `createAutoConstraints` removed |
| `5475368ea4` | Crash: the erase invalidated the reverse iterator |
| `6a6c34480a` | *Avoid redundant* only with auto-constraints on |
| `f9eaf9b568` | Widget constraints before auto-constraints; conflicting auto-constraints removed instead of thrown (#17722) |
| `af4ed78c01` | Slot: a tool's own H/V repeating an inferred one (#24248) |
| `2c2f1d7db7` *main* | The filter returns false instead of throwing; shared with drag |

Origins: `90028d4ced`, `497478fabe`, `25a28b0851` (#3079), `473c2efc38`, `677694b0ba`.

### What Silico does

- **An inferred relation the sketch already implies, or conflicts with, is left out by the tool before it sends its `sketch_add`.** The tool's own relations always stay:
  - a rectangle's two `horizontal` and two `vertical`;
  - a centre rectangle's `symmetric`;
  - a tangent arc's `tangent_at`;
  - a chain's and a rectangle's shared points, which are structure (D2).
- **Always on**, as in SolidWorks, whose help names no such option. (FreeCAD's *Avoid redundant* is still a sketch view property, `Gui/ViewProviderSketch.cpp:562-566`, shown nowhere since `41a7b146f5`.) Ctrl and *Automatic relations* off infer nothing, so they leave nothing to filter.
- **The rule** (Fable, 2026-11-08; `crates/silico-core/src/sketch_tools/implied.rs`).
  - Two partitions over the sketch's points and the origin: **Y**, the points level with each other, and **X**, the points plumb with each other. A class is **fixed** when it holds the origin, or a point the read marks fully defined (`over_defined` doesn't count).
  - Built from the sketch's constraints (the read's and the pending adds'), each read for the equation it asserts: `horizontal(L)` joins L's ends in Y; `horizontal_points(a, b)` joins a and b; `on(p, x_axis)` joins p and the origin; a driving `distance_y` of exactly 0 joins its two points. X likewise, with `vertical`, `vertical_points`, `on(p, y_axis)`, and a driving `distance_x` of 0. Through the lines, to a fixpoint: `on(p, L)` or `midpoint(p, L)`, L a line whose ends share a class, puts p in it; each line is taken up once in each partition, when a join first puts its ends in one class, so the work is about linear in the constraints. Nothing else is read. The classes are built once for a view, the first time a relation is judged on it, from the view (the read's sketch with the pending adds merged) and the read's states, and kept while those stay the same; each judgement copies them.
  - **Rule E.** An inferred relation asserting two points level (`horizontal`, `horizontal_points`, an `on` of the x axis or of a level line) is left out when they're already in one Y class, or both their Y classes are fixed (where it could only repeat them or conflict with them). X likewise.
  - **Rule F.** An inferred `perpendicular`, `parallel`, or `tangent_at` is left out when every point it reads (both lines' ends; for `tangent_at`, the line's ends, the arc's or circle's centre, and its point) is fixed both ways.
  - The tool's own relations are never judged, and nothing of them is merged before the inferred ones are. The inferred ones go in the order the gesture made them: the start point's relations, the end point's (`midpoint` before its `on`s, the `on`s in `Reference`'s order), then the line's directions (`tangent_at` before the axis relation beside it), each kept merged before the next. So the newest member of each cycle goes, FreeCAD's tie-break without the contest: the point keeps its relation and the line loses its direction.
  - It runs where the preview and the add both take their relations from: `Pointer::snap` judges the `on`s beside a `midpoint` (the only point relations that can be implied, a new point being a class of its own; for a segment's end, `Pointer::snap_after` judges them with its new start's relations merged first), and `line.rs`'s `end_of` judges a segment's directions. What the pointer shows is what's added.
  - Classes, never IDs, decide. IDs are random, and Silico's tags follow them (`sketch.rs:1141`).
  - The L from the origin: segment 1 sends no `horizontal` (its ends are both level with the origin), and the closing segment no `vertical`; two relations fewer, no `constraints_redundant`, and no SD7 offer for a dimension that over-defines nothing.
  - A conflicting inferred relation goes too, by the same rule: a line drawn between two fully defined points 1.5° off level gets no `horizontal` (FreeCAD's H/V rule's first case, and the solver filter's conflict branch, `f9eaf9b568`).
- **Partials stay.** A two-row relation (`midpoint`) is only partly implied (its place along the line no class holds), so it always stays, as FreeCAD keeps partials; `implied_by` says the same of it.
- **Judged against the newest read.** Which points are fixed is the newest read's states; what the tool has added since, pending, is in the classes as relations but fixes nothing until a read of it says so. So a gesture right after an add, before its read, can keep a relation the read would have left out (and the read then marks it `redundant`). The app's tests that close onto points they just added wait for the read first.
- **Only the direction goes in a conflict.** FreeCAD's solver filter, when an auto-constraint conflicts, removes every auto-constraint in the conflicting group (`f9eaf9b568`); Silico leaves out only the relation the rule judges, so a segment from the origin to a fully defined point 1.43° off level keeps its start's relations and loses only its `horizontal` (a deliberate divergence: the start's relations are each sound on their own, and the rule never needs the solver's diagnosis to know which one conflicts).
- **Only what the gesture makes is judged.** A redundancy the sketch already had is never the gesture's, unlike FreeCAD's throw.
- **`sketch_add` is unchanged** (SD7): it adds exactly what it's sent.
- **What the rule doesn't catch** is added and read `redundant` (addendum 2): two `on`s where curves touch, and dependencies through distances, angles, radii, or `equal` on geometry the read doesn't yet show fixed.

### FreeCAD's tests to port

None exist. Nothing in `SketcherTests/` or `tests/src/Mod/Sketcher/` covers auto-constraints or their redundancy. Port, as Silico's own, the cases this note gives from FreeCAD's code and history: the L; both ends on an axis; the fixed-ends conflict; the overlapping chain along an axis; and `8505b0eff4`'s vertical line with its ends symmetric about the x axis, as a soundness probe.

### Silico's own tests

In `crates/silico-core/src/sketch_tools/tests/implied.rs` unless named, each expected value by hand from the two rules:
- the L from the origin (no `horizontal` on its first line, no `vertical` on its last), and a chain along the x axis (no direction at all);
- a line between two fixed points, 1.5° off level or level: no `horizontal`, and no glyph; from a fixed point to a free end: kept, the end moved onto the level;
- a point `on` the x axis to a fixed point on it (what fixing every point alone wouldn't catch), and a point on a level line to a point level with it (the propagation);
- the Point tool at the middle of a line along the axis (`midpoint` alone) and of one the axis crosses there (both); at the middle of a level line another runs through (one `on` or none, as the classes are one or two);
- Rule F's `perpendicular` and `tangent_at`, between fixed points (gone) and to a free end (kept);
- a rectangle's and a centre rectangle's own relations at the origin, all sent; *Automatic relations* off and Ctrl, nothing inferred;
- IDs handed out in reverse order: the same adds;
- in every test the harness drives, what the pointer shows at the accepting click is what's added (`Drive::send`);
- `a_direction_the_sketch_already_fixes_is_neither_shown_nor_added` (tests.rs), re-pointed from `a_redundant_relation_the_pointer_shows_is_added` with the amended G;
- against PlaneGCS (crates/silico-sketch/tests/sketch_tools.rs): each relation the rule leaves out, added back and solved, is flagged `redundant` or `conflicting` or leaves `dofs` as it was; and the 1.5° line between fixed points, its `horizontal` added by hand, is over constrained, where the tool's add solves.

The study's P3 (a `parallel` collinear with its line) and P5 (an axis relation beside a `tangent_at` whose centre is plumb with the join by structure) aren't caught: they're added and read `redundant`.
