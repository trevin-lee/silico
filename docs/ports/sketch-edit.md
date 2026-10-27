# Sketch editing

The sketch edits M4 left out (docs/design/m4.1-command-api.md, section 5's *Not in M4*):
- adding and removing a point, a curve, or a constraint;
- splitting a curve at a point;
- trimming a curve back to its neighbours;
- pulling a shared point apart;
- deleting a curve while keeping its neighbours' shared point;
- merging ends that touch (decision 17's `--merge-touching`).

M5's sketch mode needs them, and the draft M5 plan (#196) puts them in a step of their own, M5.10. This note is the study it starts from. It says what FreeCAD does, the edge cases, the fixes in FreeCAD's history, the tests to port, and where Silico's shared points make it differ. **This note decides nothing.** The decisions at the end are open, each with a recommendation, for M5.0's design and the maintainer to take or change. Wherever the note says what Silico does, "recommended" or a column headed *Silico*, it's that recommendation, not a settled rule.

## Sources

- **FreeCAD's main** at `876271d660` (2026-10-07), read only. Paths are relative to `src/Mod/Sketcher/App/` unless they say otherwise. In 2026 `SketchObject.cpp` was split (`79aca8950f`). Trim, split, extend and `replaceGeometries` are now in `SketchObjectOperations.cpp`. `transferConstraints`, `deriveConstraintsForPieces` and `delConstraintOnPoint` are in `SketchObjectConstraints.cpp`. `delGeometry` is in `SketchObjectGeometry.cpp`, and the missing-coincidence analysis in `SketchAnalysis.cpp`.
- **The oracle, FreeCAD 1.1.3,** run headless (`.pixi/envs/oracle/bin/freecadcmd`) on probe scripts. A claim marked *(1.1.3)* was observed; the rest come from reading main. Main moved to 1.2 at `e8f826d0ed` (2025-11-13), and several fixes below landed after it, so **1.1.3 and main differ on split** (§3). The clone has no release tags, so whether a fix was backported to 1.1 wasn't checked; the probes show 1.1.3's behavior where it matters.
- docs/ports/sketcher.md (the sketcher study), and docs/design/m3.2-sketch-format.md (D2 shared points, D4 constraint forms, D7 construction, and the gate's rules).

## 1. The model each side edits

**FreeCAD.** Every curve has its own ends: `start`, `end`, and `mid` (a centre). A joint is a `Coincident` constraint between two ends, or an endpoint-to-endpoint `Tangent` or `Perpendicular`, which imply one. Geometry and constraints are addressed by **index**, so every deletion renumbers what follows (`changeConstraintAfterDeletingGeo`, `SketchObjectConstraints.cpp:1439-1475`), expressions included (`testIssue3245`). A lone `GeomPoint` is geometry of its own.

**Silico** (D2, approved 2026-10-16):
- A sketch lists its points. A line goes `from` one point `to` another; an arc has `center`, `start` and `end` points, counter-clockwise; a circle has a `center` point and a `radius`.
- Two curves that meet **share a point**, so there is no coincidence constraint. Every reference is an 8-character ID, so nothing renumbers.
- The fixed references `origin`, `x_axis` and `y_axis` can be named but not edited.
- The gate (D2, *The gate's rules for sketches*) refuses, among other things:
  - a line whose ends are one point;
  - an arc whose start or end is its centre, or whose start and end are one point;
  - geometry of size 1e-7 mm or less;
  - a curve's own point constrained onto that curve;
  - an edge `tangent` between two curves that share an end, which must be `tangent_at`.

  Every edit's result has to pass it.

**What follows for the port:** FreeCAD's whole joint-repair layer has no counterpart in Silico. That layer is `transferConstraints`, `delConstraintOnPoint`'s search for a replacement point, `getDirectlyCoincidentPoints`, and renumbering. About a dozen of FreeCAD's fixes below repair that layer, and they don't apply as code. The rules they protect still apply, and each becomes a test.

## 2. Adding and removing

### Add a point, a curve, a constraint

**What FreeCAD does:**
- **`addGeometry`** (`SketchObjectGeometry.cpp:148-207`) checks nothing ("a sketchobject managed operation", :186). It copies the geometry with a fresh tag (`71a555a6bb`, so adding one Python object twice makes two curves) and appends it.
  - Degenerate input is refused only by Part: a line's ends within `gp::Resolution` raise "Both points are equal" (`Part/Geometry.cpp:4822`).
  - Since `0e174da56d` and `f32d5e5fd9` (2026), the Gui's tools refuse non-finite positions and zero-size rectangles before they add anything.
  - `3db4633f9d` stopped forcing every new point to be construction.
- **`addConstraint`** (`SketchObjectConstraints.cpp:930-962`) checks only index ranges (`evaluateConstraint`, :2172-2233; Python raises "Constraint has invalid indexes"). A constraint whose form doesn't fit is found later, by the solver, as malformed. That has been an error at recompute since `150e50c700`.
  - It records a distance's or tangent's side, and the kind of tangency, **from the current geometry** (`setOrientation`, `AutoLockTangencyAndPerpty`, :948-951).
  - Python's list forms skip items of the wrong type silently.
- **The Coincident command** (`Gui/CommandConstraints.cpp:5310-5782`, from `e38154474a`, `084379651a` and `cb2780e2fa`, 2025–2026) refuses:
  - points already coincident, even through other points ("already coincident");
  - a coincidence that would bring two ends of one curve together, directly or through a chain ("would collapse geometry to zero length", #20771);
  - any result whose solve is worse than before, which it rolls back.
  - Its tests are the six in `SketcherTests/TestCoincidentCommandGui.py`.
- **Tangency substitution:** adding a tangency between curves whose ends are joined gives an endpoint tangency, and adding a coincidence where an edge tangency exists does the same (`180d722a53`, `620188073e`, #3299).

**What Silico does differently:**
- Every add goes through the gate, so a wrong form, a dangling ID, a degenerate curve or a non-finite coordinate is refused when it's made, naming the field. FreeCAD finds these at the next solve or not at all.
- Each new element gets a fresh ID, and the gate refuses duplicates.
- `side` and `direction` are part of the request (D4: stored, never decided from geometry).
- `construction` is as given, for points too (D7, `3db4633f9d`).
- A batch of adds is checked whole before any is applied.
- The tangency substitution is already the gate's rule (`tangent_at` where two curves share an end).

### Remove a constraint

`delConstraint(s)` (`SketchObjectConstraints.cpp:964-1019`) erases by index and shifts the rest. Deleting a `Coincident` unjoins two ends **without moving them**.

In Silico a constraint is removed by ID and nothing shifts. There's no coincidence constraint to delete: unjoining is its own edit (§5).

### Remove a curve

**What FreeCAD does** (`delGeometry`, `SketchObjectGeometry.cpp:226-288`; the Gui's batch form `delGeometries`, `SketchObjectOperations.cpp:115-167`):

1. **Joints.** For each of the curve's `start`, `end` and `mid`, `getDirectlyCoincidentPoints` (`SketchObjectConstraints.cpp:2002-2042`) lists the ends joined to it, in constraint order: through `Coincident`s, and since `f8f159c4d3` through endpoint-to-endpoint `Tangent`s too. If any are listed:
   - `delConstraintOnPoint(onlyCoincident)` (:1036-1159) picks the partner of the **first** `Coincident` as a replacement, re-points every other coincidence to it, and drops the one between them. So at a joint of three or more curves the others stay joined.
   - `transferConstraints` (:1161-1229) moves the deleted end's other constraints onto the first listed partner: locks, `PointOnObject`, `Symmetric` (its third element since `51cf34a596`), and point-form horizontal and vertical.
     - An endpoint `Tangent` or `Perpendicular` becomes a `Coincident` (`9ce5756011`, #3291: a tangency nobody asked for was imposed on the neighbour).
     - An `Angle` is dropped (`e66849378b`, #3589).
2. **Dropping.** Every constraint naming the curve in any slot is dropped, and every later index shifts (`0016d6f2f8`, `aeabffeed8`). An end used by no other curve disappears with the curve, and so do its constraints.
3. **One solve,** after the whole edit (`65310f00ab`, `339717c96f`).

| Deleting the middle of a chain, or a curve at a joint of three *(1.1.3)* | Result |
|---|---|
| D between A and B, each joint its own (`Coincident(A.e, D.s)`, `Coincident(D.e, B.s)`) | Both coincidences go. A and B are free. D's lock on its start moves to `A.e` |
| D the hub of a three-way joint with A and B | `Coincident(A.e, B.s)`: they stay joined |
| Two arcs whose centres are joined through a deleted circle's centre | The arcs' centres stay coincident |
| A lone point joining two line ends | The lines stay coincident; the point's lock moves to `A.e` |

**FreeCAD's defects here, observed in 1.1.3:**
- **The result depends on the order of the constraints.** When an endpoint `Tangent` is listed before the `Coincident`, the replacement (from the first `Coincident`) and the transfer target (the first listed partner) differ. The coincidence is dropped, the tangency isn't moved, and **the two neighbours end up unjoined**. With the constraints listed the other way round, they stay joined.
- **An endpoint-to-edge `Tangent` on the deleted end becomes a malformed `Coincident(point, edge)`,** still carrying its angle value.

**What Silico does differently:**
- **Shared points do all of this by construction.** A point that another remaining curve uses survives, and every constraint on it stays as it is: no re-pointing and no transfer.
- A `tangent_at` naming the deleted curve is removed, never moved onto a neighbour (`9ce5756011`'s intent).
- Nothing depends on constraint order.
- **What Silico has to decide** is what happens to a point **only** the deleted curve used. FreeCAD's end disappears with the curve. Recommended (decision 7): the same. A point no remaining curve uses is deleted with its constraints, as a line's endpoints go with it in SolidWorks, unless it was a standalone point before the curve used it. With shared points that can't be told apart, so the rule is simply "used by no curve after the edit".
- **Batch delete:** deleting a set equals deleting its members one by one. It tolerates duplicates and members that depend on others in the set, and it's solved once (`41c91574cf`, `4ffb6c5e2a`).
- **Refused:** deleting `origin`, `x_axis` or `y_axis` (`2a401ee895`, `83d14b785e`).

### Remove a point

FreeCAD's Gui Delete on a vertex does one of two things (`Gui/ViewProviderSketch.cpp:4885-4952`):
- On a `GeomPoint`, it deletes that geometry. Its constraints move onto a joined end if it had one *(1.1.3)*.
- On any curve's end, it *unjoins* that end (`delConstraintOnPoint`, §5).

FreeCAD has no "delete a point that curves end at". In Silico such a point *is* those curves' end, so deleting it would have to delete the curves too. Recommended (decision 7): delete a point only if no curve uses it, and otherwise refuse, naming the curves that use it.

## 3. Split

**What FreeCAD does** (`split`, `SketchObjectOperations.cpp:1151-1282`):

1. **Refusals.** Only the index is checked: out of range raises "Not able to split curve with the given index". The kind isn't checked: the curve is taken by a `static_cast` (`SketchObject.h:285-288`), and splitting a `GeomPoint` **segfaults** *(1.1.3, in `GeomCurve::getFirstParameter`)*. Main has the same path, by reading; trim's equivalent was fixed by `5efb8e5e76`, and split never got the check. Only the Gui restricts kinds.
2. **The split position** is the given point's projection onto the curve (`closestParameter`). A point off the curve splits at its projection *(1.1.3: (4, 3) on the x axis splits at (4, 0))*.
3. **At or beyond an end, it fails inside OCCT,** with "Geom_TrimmedCurve::U1 == U2", and the sketch is left unchanged *(1.1.3)*. `82633fb5df` (2022) refused this with "Split point is at one of the end points of the curve."; the refactor `215fbc5187` (2025) lost the check.
4. **Near an end, nothing is refused:** a split 1e-12 mm from the start makes a 1e-12 mm line *(1.1.3)*.
5. **Pieces.**
   - An open curve gives two pieces: the start side keeps the curve's index and geometry ID (`82610ca8fa`, a refactor that changed behavior), and the other is appended.
   - **A circle gives one arc from the split angle to the same angle plus a full turn,** whose start and end are at one place and aren't joined *(1.1.3)*.
6. **Constraints on the curve's ends** stay with the outer ends (`transferConstraints` with `doNotTransformTangencies`). The refactors `6c8cb8e58d` (lines, 2022) and `c6f5739763` (arcs, 2024) turned an end tangency into a coincidence, and `6d06b61c7e` fixed it on main; 1.1.3 still turns it into a `Coincident` *(1.1.3)*.
7. **Constraints on the curve itself** go through `deriveConstraintsForPieces` (`SketchObjectConstraints.cpp:1506-1747`), shared with trim. The table below gives the rules.
8. **The joint between two pieces:**
   - Arcs get a `Coincident` at the split point and one between the centres.
   - Lines on main get an endpoint **`Tangent`**, which holds them collinear (`6d06b61c7e`, 2026-09).
   - Lines in **1.1.3 get a `Coincident`**: the pieces can kink unless a direction constraint was copied to both.
9. **No expression is kept** on a constraint split rebuilds *(1.1.3: a length's `5*2` was lost)*; trim keeps them.
10. **One solve** at the end (`c6c084f22c`, #23711: a solve halfway through moved geometry before the pieces were built).

| Constraint on the split curve | FreeCAD 1.1.3 *(probed)* | FreeCAD main | Silico (recommended) |
|---|---|---|---|
| `horizontal`, `vertical`, `parallel` on a line | on both pieces; with `parallel` too, one redundant constraint | on the first piece only; the tangent joint carries it | as main (decision 1) |
| `perpendicular` (in D4, between two lines' directions, never a contact) | on both pieces *(probed with a line it doesn't touch)* | dropped when neither piece touches the other line: it goes through the edge-contact test (:1563-1590, marked "TODO: Actually check that there was perpendicularity earlier"), a bug | on the start-side piece, with the direction constraints (decision 1); main's drop isn't copied |
| `angle` between lines | **segfault**: a piece not yet in the sketch reached `78e5729520`'s angle branch as a null (`std::ranges::any_of` over `newGeos`, in the crash report) | on the first piece; the crash is fixed by `bbbff1f8f1`, which passes the new pieces in | as main (decision 1) |
| `length` | `distance` between the outer ends | the same (`4d6b1f3eb8`, `2779f0b29f`) | `distance` between `from` and `to` |
| `equal` on a line | dropped | dropped (`857190a20a`, #25008; `edb87a0663`) | dropped |
| `radius`, `diameter`, `equal` on an arc or circle | first piece only | the same (`f0d30cb570`) | on one piece: the pieces share the centre, so their radii are equal anyway |
| `on`, `point_line_distance` to the curve | **dropped** (#28240) | to the piece whose span holds the point's projection; outside every span, the nearer end's piece (`closestParameter`'s fallback, `Part/App/Geometry.cpp:1073-1083`) (`bbbff1f8f1`, `263331f6fb`) | as main. With decision 1 the pieces lie on the original's line, or share its circle, so the constraint means on either piece what it meant before |
| edge `tangent` | on both pieces | to the piece that touches the other curve, else dropped (`6d06b61c7e`, `c823a6b270`) | to the piece whose span holds the contact; else the start-side piece, whose line or circle is the original's (decision 2) |
| `midpoint` of the line | kept on the outer ends, as a `Symmetric` | the same | `symmetric` of `from` and `to` about the point |
| `symmetric` about the line | dropped | dropped | on the start-side piece, whose line is the original's (decision 2) |
| `tangent_at`, `tangent_on` at an end of the split curve (it's `a`) | made `Coincident` (the regressions above) | kept | kept: the end is the same point |
| `tangent_on` onto the split curve (it's `b`: another curve's end `at` lies on it) | not probed | a point-located constraint goes to the piece holding its point (`findPieceContainingPoint`, :1490-1504; `0800791a0f` for an angle via a point) | `b` becomes the piece whose span holds `at`. When `at` is the split point itself, it becomes `tangent_at` between `a` and that piece, its `direction` mapped from the stored one, never derived, since the gate refuses a `tangent_on` whose `at` is an end of `b` |

**What Silico does differently:**
- **The pieces share one new point** at the split position, and two arc pieces share the original centre point. Nothing is added to join them, so there's no joint constraint.
- Constraints on the original end points don't move, because those points are the pieces' outer ends.
- **Splitting at a point the sketch already has** (a point held `on` the curve): the pieces share that point, and its `on` constraint is removed, since the gate refuses a curve's own point `on` it. A `tangent_on` whose `at` is that point becomes a `tangent_at` (the table's last row).
- **The start-side piece keeps the curve's ID** (`82610ca8fa`): from `from` for a line, counter-clockwise from `start` for an arc. The other piece gets a new ID. Both keep `construction`, and so does the new point.
- **A circle can't split as FreeCAD splits it,** since an arc whose start and end are one point is refused (decision 3).
- **Refusals, as data naming the curve:** a point (the gate's kinds); a split position at, beyond, or within 1e-7 mm of an end (`82633fb5df`'s rule restored, with the gate's size floor); a position farther than 1e-7 mm off the curve (decision 4).

## 4. Trim

**What FreeCAD does** (`trim`, `SketchObjectOperations.cpp:921-1149`; the cut search in `Part/Part2DObject.cpp:104-261`):

1. **What may be trimmed:** any sketch curve that isn't internal geometry. A `GeomPoint` finds no cut and **is deleted** *(1.1.3)*. Before `5efb8e5e76` (#19425) it crashed.
2. **What cuts:** every other curve, including construction (`715f98d1f3`, #624) and external geometry. The axes cut only with main's opt-in `includeSketchAxes` (`196da068a0`), and then as infinite lines. **A sketch point never cuts** *(1.1.3)*. A cut is found three ways:
   - OCCT's curve-curve intersections at 1e-12;
   - both ends of an overlap or tangent-contact segment (`3cbe309330`);
   - every end of another bounded curve that lies on the trimmed curve within 1e-7 mm (`e10068a648`, #2463), because OCCT misses an end touching tangentially.

   **Tangency counts as a cut** *(1.1.3)*.
3. **Which cuts are used:** the nearest on each side of the picked point's projection, or for a closed curve the two nearest around it.
   - A cut within 1e-8 **in the curve's parameter** (`Precision::PApproximation`: mm on a line, radians on an arc, so r · 1e-8 mm) of an open curve's own end is no cut (`226d24792d`, :638-643), so a neighbour joined at the end doesn't make a zero-length piece.
4. **When the whole curve is deleted** (through delete, so joints are kept; `2d817128d0`):
   - no cut on either side (`testTrimWithoutIntersection`);
   - two cuts within **5e-5 mm** of each other *(1.1.3: two cutters 3e-5 mm apart, picked between them)*;
   - a circle with fewer than two distinct cuts (`testTrimCircleEnd`). Main reaches this by writing into an empty vector, which is undefined behaviour that happens to give the right answer.
5. **Pieces.**
   - A cut on one side shortens the curve.
   - Cuts on both sides of an open curve give two pieces.
   - A circle gives one arc, counter-clockwise from the cut after the pick to the cut before it.
   - The first piece keeps the index and ID (`7d8a8b7cc8`, `226d24792d`). Every piece keeps `construction` (`23ecf94ab6`, #12715).
6. **Each new end at a cut** is joined to the cutter. Each cut is decided on its own and from geometry (`7d9cfeb2c5`, `07cf5b10d7`):
   - If the cut is within **5e-5 mm** of the cutter's start or end (`dec01d1df0`, 2020: "needed in order to not falsely distinguish points calculated with seekTrimPoints"), the new end is made `Coincident` with that end.
   - Otherwise it gets `PointOnObject` on the cutter.
   - A cutter end already held `PointOnObject` on the trimmed curve at that cut becomes the `Coincident` (`6e6a0ccfb4`).
7. **A tangency between the cutter and the trimmed curve** (edge to edge) becomes an endpoint tangency at the new end (`dec01d1df0`). This applies only to the cutter at that cut: a tangency to an unrelated curve stays edge to edge (`08a66afbc8`, AstoCAD #24, `testTrimEndEffectOnUnrelatedTangent`).
8. **Constraints on a kept end** move with it. Those on a removed end are dropped. A constraint naming **both** ends survives only if both do (`f0d30cb570`, `502b7b9a3f`; `testTrimEndEffectOnFullLengthConstraints`, `testTrimEndEffectOnSymmetricConstraints`).
   - Trimming away an end that a neighbour shares detaches the neighbour *(1.1.3)*.
9. **Constraints on the curve itself:** as for split (§3's table), with three differences:
   - `horizontal`, `vertical`, `parallel`, `perpendicular`, an edge `tangent` on a line, and `angle` go to **every** line piece (`assumeTangency = false`, :1547-1552) *(1.1.3: a `Perpendicular` to a line it doesn't touch is on both pieces)*. Nothing joins two line pieces, which haven't been collinear since `2d817128d0`.
   - A `tangent_on` onto the trimmed curve goes to the piece holding `at`, and is dropped when `at` is on the removed part.
   - An `on` whose point lies on the removed part is dropped (`333ca836c0`).
   - Two arc pieces get coincident centres but **no `equal`** (`f0d30cb570`: it made constraints redundant), so the second arc's radius is free.
10. **Expressions follow** the constraints they're on (`bb5afb911a`, #27505; *1.1.3 keeps them*).
11. **One solve** at the end, never in between (`350a416708`, `fa2e5cd5fe`, `1dcde4e4cc`).

**Worked example** *(1.1.3)*. Line 0 runs from (0,0) to (10,0), with `Horizontal`, a length of 10 and `Equal` to line 3:
- cutter 1 crosses it at x = 3;
- cutter 2 *starts* on it at x = 7, held by `PointOnObject`;
- neighbour 6 is joined at its start;
- point 4 is `on` it at x = 5, on the part that's removed;
- point 5 is `on` it at x = 9.

Picked at x = 5, the result is:
- the pieces are 0 from (0,0) to (3,0) and 7 from (7,0) to (10,0);
- `Horizontal` is on both, and the length becomes `Distance(0.start, 7.end) = 10`;
- piece 7's start is joined to cutter 2's start;
- piece 0's end gets `PointOnObject` on cutter 1;
- point 5's `on` moves to piece 7;
- the neighbour stays joined;
- `Equal` and point 4's `on` are dropped.

**What Silico does differently:**
- "Coincident with the cutter's end" becomes **the piece's end is the cutter's point**, shared, but only when a constraint holds the cutter's end on the trimmed curve (decision 5). FreeCAD's own tests draw the cutter there with no constraint, and Silico departs from them (§9).
- "PointOnObject on the cutter" becomes **a new point with an `on` constraint** to the cutter. The cutter isn't split.
- Two arc pieces share the centre point.
- Trimming away a shared end gives the piece a new end point; the neighbour keeps the shared point.
- **FreeCAD's 5e-5 mm "at the cutter's end" test merges points that aren't the same.** That's the snapping Silico's exactness rule refuses (decision 5).
- **Where intersections come from:** FreeCAD uses OCCT. Silico's curves are lines and circular arcs, whose intersections are closed-form (decision 6).
- **Line pieces stay independent,** with direction constraints on each, as FreeCAD's trim does. Unlike split, nothing joins them.
- **Where nothing can take a constraint,** because the trim's pieces really are independent (an arc's edge `tangent` that no piece touches, `symmetric` about the trimmed line), FreeCAD drops it; Silico refuses the trim, naming it (decision 2).
- **A `tangent` turned into an end tangency** needs a `direction` (`tangent_at` or `tangent_on`), which D4 stores and never derives from geometry (decision 8).

## 5. Pulling a shared point apart

**What FreeCAD does.** It has no shared points, and **no gesture to unjoin**: dragging a joint moves the whole coincident group. Two things unjoin ends:

- **Deleting the `Coincident` constraint.** Both ends stay where they are, now independent.
- **Selecting a vertex and pressing Delete** (`Gui/ViewProviderSketch.cpp:4922-4952`). This runs `delConstraintOnPoint(geo, pos)`:
  - That one curve's end leaves its group, and the rest stay joined, through re-pointed coincidences (`0358ff7ce2`, 2024: before it, every coincidence at the point was dropped, so a three-way joint fell apart).
  - The end's other constraints stay with that curve.
  - **The same commit also drops the curve's own length** (`Distance`, `DistanceX` or `DistanceY` on the whole curve) when it unjoins either end *(1.1.3)*. Nothing asked for that, and Silico doesn't port it.
  - On a point with no coincidence it used to raise an error; since `bc4d463cdf` (#2275) it does nothing.

**What Silico does differently.** Unjoining is an explicit edit: one curve's use of a shared point moves to a new point at the same coordinates. Recommended (decision 9):
- The other curves keep the point and every constraint on it.
- The detached curve gets the new point with no constraints.
- A `tangent_at` between the detached curve and another at that point is removed, since they no longer meet. This is refused if the point isn't shared, naming the point (`bc4d463cdf`).
- Every dimension of the detached curve stays (not `0358ff7ce2`'s side effect).
- With `origin` as the shared point, the detached curve gets a new point, and `origin` stays.
- Afterwards the two points are at one place. `check` then warns of them as `ends_not_joined` until something moves them, which is true and intended.

## 6. Merging touching ends

**What FreeCAD does.** The *Validate sketch* task (`Gui/TaskSketcherValidation.cpp`) and `SketchAnalysis` (`detectMissingPointOnPointConstraints`, `SketchAnalysis.cpp:505-550`):

- **Tolerance.** 1e-4 mm by default (`Confusion × 1000`; the task offers 1e-9 to 1e-2 and takes typed text). It's compared **per axis**, inclusive: a square, not a distance *(1.1.3: ends 0.9e-4 apart in both x and y are found)*. The history shows it is a user's tolerance, not a modelling rule: 0.1 mm in `957cedc1ff` (2013), then 0.001 (`b29fbc5f93`), then a choice defaulting to 1e-4 (`97a2b516bb`).
- **Clusters.** Ends are sorted with a tolerance (not a strict weak order). A cluster is the ends within tolerance of its **first** end, **not chained** *(1.1.3: ends at x = 0, 0.6e-4 and 1.2e-4 give one missing, not two)*.
- **Which ends count:** line, arc and conic ends only. **Not** arc or circle centres, not `GeomPoint`s, and not an end lying on a full circle *(1.1.3: 0 for each)*. External geometry never counts. Construction counts in Python's default, but the task's "Ignore construction geometry" is checked by default (`0d21e94d3f`, `10c0af4a8a`).
- **Joined** is meant to be `Coincident`, or an endpoint `Tangent` or `Perpendicular` (`7463aa7efd`, #2994). That fix also judged two curves sharing both ends per end.
  - `c036128aa0` (2022) made the grouping transitive, with k groups giving k − 1 coincidences (`testThreeLinesWithCoincidences_1`/`_2`).
  - The same commit starts the list from **every** constraint. So, *(1.1.3)*, two touching ends report **0 missing** whenever one of these holds:
    - a `Distance`, `Horizontal` or `Vertical` sits between them;
    - a deactivated `Coincident` joins them;
    - one of them carries an `on` to an unrelated curve or a lock (the end is dropped from the analysis).
- **Fix** (`makeMissingPointOnPointCoincident`) adds k − 1 `Coincident`s in one go, each chained from the first end of the previous group, ordered by (GeoId, PosId). **The solver then decides where the ends land.** Its one-by-one form solves after each and throws on failure.
  - `5696ee821c` (#14240): Fix once used a different analysis from Find and added nothing.
- **`autoconstraint`** first deletes every constraint. Not relevant to a merge.

**What Silico has already:** `check`'s `ends_not_joined` (crates/silico-core/src/warnings.rs, #189):
- profile curves' ends that are separate points, strictly closer than FreeCAD's **wire-joining** distance, 1e-7 mm, Euclidean;
- chained as `BRepLib_MakeWire` chains them, one warning per cluster;
- construction never warns.

That is deliberately not Validate's 1e-4 box. Ends 1e-7 to 1e-4 mm apart, which Validate would flag, are what M3.5's open-profile refusal names when a feature sweeps the sketch. **Silico doesn't carry `c036128aa0`'s bug:** joined means the same point, and no other constraint hides an end. Those cases warn where 1.1.3 says 0, and that divergence is deliberate.

**What Silico does differently.** The merge is an explicit edit that makes several points one, never a constraint the solver satisfies. Recommended (decision 10):
- **The request names the points.** `--merge-touching` names each `ends_not_joined` cluster's points.
- **The first point the request names survives,** at its own coordinates; `--merge-touching`, whose points are within 1e-7 mm, keeps each cluster's least ID. The others' curves and constraints are re-pointed to it, and the others are removed. (A FreeCAD coincidence is solved, not placed: it lands where the solve takes it, at its `First` point in a two-line probe either way round *(1.1.3)*; Validate chains from the first end by (GeoId, PosId), creation order, which Silico's random IDs don't keep.)
- **Refused, naming them:**
  - points that are already one point (`cb2780e2fa`);
  - a merge after which a curve would use one point twice (a line's ends, an arc's centre and an end, or its two ends; `084379651a`, #20771, through any chain);
  - a merge after which a constraint would name one point twice (a `distance` or `horizontal_points` between merged points);
  - an edge `tangent` between two curves that would then share an end (the gate's `tangent_at` rule, decision 8).
- **No automatic fix-up:** the solve after the merge is the edit's, and a conflict it finds refuses the edit as data (M3.3), as FreeCAD's command rolls back a worse solve.

## 7. Extend

**What FreeCAD does** (`extend`, `SketchObjectOperations.cpp:503-549`): lines and arcs only.
- **Lines.** A line's chosen end moves by `increment` along the line, through the solver's drag, so constraints win: a line with a length of 10, extended by 5, slides instead of growing *(1.1.3)*.
- **Arcs.** An arc's start or end angle moves by `increment` **radians**, written straight into the geometry, then solved.
- **Unchecked:** a negative increment may shrink past the other end and flip a line *(1.1.3: −15 on a 10 mm line)*, or invert an arc.
- **Other kinds and the `mid` position.** 1.1 refused these. Main returns success and does nothing (`d8cf415e4f`, `a502762119`, 2026), a regression by reading.
- The Gui extends to the cursor and then adds an auto-constraint (`on`) at the moved end. The App layer never looks for the next curve. FreeCAD has no App test of extend.

**What Silico does differently.** Moving an end is moving its point. Every curve that shares it follows, as FreeCAD's drag pulls a coincident neighbour. The order matters: dragging the end to its target and letting the solve follow gives FreeCAD's result, but writing the end there and then solving doesn't *(1.1.3: a 10 mm horizontal line with its length, extended by 5: (5,0)–(15,0) by the drag, (3.5706,0)–(13.5706,0) by write-then-solve)*. Recommended (decision 11):
- A line's end goes through the sketch worker's drag session (M3.7's begin, move, end), as FreeCAD's `moveGeometry` does; an arc's range is written and the sketch solved, as FreeCAD does.
- Refuse anything but a line or an arc, at anything but its two ends, as 1.1 did.
- Refuse a result of size 1e-7 mm or less, or a flip.
- Express an arc's increment as an angle (`Angle`, exact) and a line's as a `Length`.

## 8. Bug fixes from FreeCAD's history

The fixes that set a rule the port must carry, newest first within each operation. Fixes to FreeCAD's joint-repair layer, index renumbering, external geometry, or ellipses and B-splines are listed only where they imply a Silico test. Every hash was checked against the clone.

| Commit | Date | Fixed | Silico |
|---|---|---|---|
| **Trim** | | | |
| `1dcde4e4cc` | 2026-09 | `6d06b61c7e` had inverted the solve guard, so a trim without recompute was never solved | One solve after the edit; test that a trim's result is a solved sketch |
| `bb5afb911a` | 2026-02, #27505 | Expressions lost on trim | Re-point constraints by ID rather than recreate them; a test once expressions exist |
| `857190a20a` | 2025-11, #25008 | A line's `Equal` moved to a piece and broke polygons | A line's `equal` dropped; an arc's kept on one piece |
| `fa2e5cd5fe`, `350a416708` | 2025-09, 2025-04 | A solve between removing the old constraints and adding the pieces' moved geometry | One solve, after the whole edit |
| `114d3dcd87`, `018ac7b9dd` | 2025-07, #22352 | Constraints assigned from pieces that didn't exist yet (crash) | Build the pieces first; assign constraints from their own geometry |
| `5efb8e5e76` | 2025-04, #19425 | Trimming a point crashed | Refused, naming it |
| `23ecf94ab6` | 2025-03, #12715 | Pieces lost `construction` | Pieces and new points keep it |
| `08a66afbc8` | 2025-03, AstoCAD #24 | An unrelated edge tangency was turned into an end tangency | Only the cutter's tangency at its cut becomes an end tangency; port the test |
| `2779f0b29f` | 2025-03, #19770 | A line's length with one piece left crashed | `length` dropped with one piece, a `distance` with two |
| `f0d30cb570` | 2025-03 | An `Equal` added between arc pieces made constraints redundant; constraints on both ends survived when one end was removed | No `equal` added; a constraint on both ends dropped unless both survive |
| `502b7b9a3f` | 2025-01 | A refactor transferred end constraints only for closed curves, losing them on every line and arc | A kept end's constraints and its neighbour survive the trim of the other end |
| `226d24792d` | 2024-12 | Degenerate pieces at a cut on the curve's own end | No zero-length piece; the touching end keeps its point |
| `7d8a8b7cc8` | 2024-08 | Trimming GeoId 0 broke topological naming | The first piece keeps the curve's ID (names of sketch edges read curve IDs) |
| `333ca836c0` | 2024-06 | An `on` on the removed part stayed on the shortened curve | Dropped |
| `2d817128d0` | 2021-01 | Trim with no cut, or nothing left, now deletes the curve; line pieces no longer kept collinear | Delete when there's no cut; line pieces independent |
| `dec01d1df0` | 2020-12 | Tangency lost trimming an arc tangent to a line; the 5e-5 mm cut tolerance | Tangency carried to the new end; the tolerance is decision 5 |
| `edb87a0663` | 2020-12, #4510 | A trimmed line's `Equal` forced it back to full length | As `857190a20a` |
| `7d9cfeb2c5` | 2020-04 | A circle's second cut copied the first cut's joint | Each cut decided on its own, from geometry |
| `3cbe309330` | 2020-02 | Tangent contacts and overlaps weren't cuts | A line tangent to a circle cuts it; overlapping lines cut at the overlap's ends |
| `07cf5b10d7` | 2018-11 | A cutter crossing twice got its end joined at the wrong crossing | Test a line crossing a circle twice, ending on it at one crossing |
| `e10068a648` | 2018-10, #2463 | OCCT missed an end touching tangentially | Another curve's end on the trimmed curve is a cut |
| `715f98d1f3` | 2012-04, #624 | Construction lines didn't cut | Construction cuts |
| **Split** | | | |
| `6d06b61c7e` | 2026-09 | Line pieces now held collinear; end tangencies kept; direction constraints on one piece | Decision 1; `tangent_at` at the outer ends kept |
| `bbbff1f8f1` | 2026-07, #28240 | `on` and point-to-line distances on the split curve silently dropped (still in 1.1.3) | Kept, on the piece holding the point; test a point on each half |
| `78e5729520` | 2025-12 | An `Angle` on the split curve dropped (1.1.3, which has this branch, segfaults instead, fixed on main by `bbbff1f8f1`) | Kept (decision 1); test |
| `c6c084f22c` | 2025-09, #23711 | A solve halfway through the split moved geometry | One solve; untouched points keep their exact coordinates |
| `215fbc5187` | 2025-03 | A refactor lost the refusal to split at an end | Refused as data, within 1e-7 mm of an end too |
| `82610ca8fa` | 2025-01 | The start-side piece now keeps the original's ID | The same; pinned by a test |
| `0800791a0f` | 2025-12 | An angle via a point went to every piece | A constraint at a point on the curve goes to the piece holding the point: `tangent_on` onto the split curve |
| `c823a6b270` | 2024-06 | An arc's edge tangency went to the longer piece; now dropped silently | Refused, naming it, if no piece touches the other curve (decision 2) |
| `263331f6fb` | 2024-06 | Which piece holds a point: a tolerance, and a full-turn wrap for closed curves | A point `on` a circle stays on the arc at any angle |
| **Delete and transfer** | | | |
| `0358ff7ce2` | 2024-12 | (a) A three-way joint now survives deleting one curve; (b) unjoining now drops the curve's length | Carry (a) as a test; don't port (b) |
| `358942b771` | 2025-03 | A missing `break` dropped every transferred tangency, losing joints | A joint held by `tangent_at` survives deleting a third curve there |
| `51cf34a596` | 2025-01 | A `Symmetric`'s third element wasn't transferred | A `symmetric` whose `about` point survives keeps it |
| `f8f159c4d3` | 2024-02 | Endpoint tangencies count as joints on delete; leaves 1.1.3's order dependence | Test: the result never depends on constraint order |
| `41c91574cf`, `4ffb6c5e2a` | 2021-01 | Batch delete missed dependents, and erred on ids already removed | A batch equals one by one; dependents and duplicates are harmless |
| `e66849378b` | 2018-10, #3589 | An angle moved to a neighbour measured something else | An `angle` goes with its line |
| `9ce5756011` | 2017-12, #3291 | An end tangency moved onto a neighbour imposed a tangency | A `tangent_at` naming the deleted curve is removed, never moved |
| `bc4d463cdf` | 2018-01, #2275 | Unjoining a point with no joint raised an error | Refused as data (decision 9) |
| `339717c96f`, `65310f00ab` | 2015-06 | Stale DoF after a delete; a solve on half-edited data crashed | One solve after the whole edit |
| `2a401ee895`, `83d14b785e` | 2012, 2026-03 | Deleting at the root point, or deleting the axes | `origin` and the axes can't be deleted |
| `0016d6f2f8`, `aeabffeed8` | 2012, 2015 | A `Symmetric`'s third slot wasn't renumbered (crash) | Every reference slot counts: deleting a `symmetric`'s `about` line drops it |
| **Add and merge** | | | |
| `cb2780e2fa`, `084379651a`, `e38154474a` | 2026-09, 2025-11 | Coincidences between points already joined, or collapsing a curve through a chain; a worse solve kept | The merge's refusals (decision 10) |
| `0e174da56d`, `f32d5e5fd9` | 2026-06 | Non-finite or zero-size geometry from the tools | The gate refuses it at the command |
| `3db4633f9d` | 2024-08 | New points forced to construction | `construction` as given |
| `150e50c700` | 2021-01 | Malformed constraints silently ignored | The gate refuses every malformed form |
| `71a555a6bb` | 2017-04 | One object added twice gave two curves one tag | Fresh IDs; duplicates refused |
| `180d722a53`, `620188073e` | 2018-01, #3299 | A tangency plus a joint at the same ends | The gate's `tangent_at` rule; decision 8 |
| **Missing coincidences** | | | |
| `c036128aa0` | 2022-02 | Grouping made transitive; also made every constraint count as a join | Carry the grouping (built in #189); not the bug |
| `5696ee821c` | 2024-05, #14240 | Fix acted on a different analysis from Find | The merge acts on the same check of the same document state |
| `7463aa7efd` | 2017-04, #2994 | End tangencies reported as missing; a two-curve loop judged as one pair | Two curves sharing both ends are judged per end |
| `10c0af4a8a`, `0d21e94d3f` | 2018-01, #3298 | Internal and construction geometry gave false reports | Construction never warns (built) |
| **Extend** | | | |
| `a502762119`, `d8cf415e4f` | 2026-08/09 | Extending an unsupported kind returns success and does nothing (by reading) | Refused, as 1.1 did |
| `c3c94e8698` | 2017-06 | Shrinking flipped which end moved (Gui) | A negative increment shrinks; past the other end it's refused |

## 9. Tests

### FreeCAD's, to port

**Where Silico departs from them:** in `testTrimLineSegmentEnd`, `testTrimLineSegmentMid`, `testTrimCircleMid`, and `testTrimArcOfCircleEnd`/`Mid`, the cutter's end is only drawn on the trimmed curve, and FreeCAD joins it through its 5e-5 mm test (`getNewConstraintAtTrimCut`, :716-741). Under decision 5 Silico gives the piece a new point `on` the cutter instead, and `check` warns of the two points. Each test is ported twice: as drawn, with Silico's result, and with an `on` holding the cutter's end, with FreeCAD's.

FreeCAD's split and trim tests count curves and `Coincident` and `PointOnObject` constraints. In Silico those counts become counts of shared points and `on` constraints. Their geometry is `tests/src/Mod/Sketcher/App/SketcherTestHelpers.cpp`'s:
- a line from (1,2) to (3,4);
- a circle with centre (1,2) and radius 3;
- an arc with centre (1,2) and radius 3, from π/3 to 1.5π.

| Test (`tests/src/Mod/Sketcher/App/SketchObjectChanges.cpp`) | Checks | Silico |
|---|---|---|
| `testTrimWithoutIntersection` | no cut: the curve is deleted | the same |
| `testTrimLineSegmentEnd` | a cutter starting on the line at 0.5, picked at 0.2: 1 curve, 1 `Coincident` | as drawn (no constraint holds the cutter): 1 line with a new end point `on` the cutter, and an `ends_not_joined` warning naming it and the cutter's start (decision 5). With an `on` holding the cutter's start on the line: 1 line whose end is the cutter's start point |
| `testTrimLineSegmentMid` | cutters at 0.3 (starting on it) and 0.7 (crossing): 2 curves, 1 each | 2 lines, each with a new point `on` its cutter, and one warning; held by an `on`, the first shares the cutter's point |
| `testTrimCircleEnd` | one cutter: the circle is deleted | the same |
| `testTrimCircleMid` | two cutters: 1 arc, 1 `PointOnObject` and 1 `Coincident` | as drawn: 1 arc with two new points `on` the cutters, and one warning; held, one shared point and one `on` |
| `testTrimArcOfCircleEnd`, `testTrimArcOfCircleMid` | 1 arc and 1 `Coincident`; 2 arcs, 1 `PointOnObject` and 2 `Coincident` | as above for the cutter's end (decision 5); the two arcs share the centre point, with no constraint for it |
| `testTrimEffectOnConstruction` | every piece of a construction circle is construction | pieces and new points |
| `testTrimEndEffectOnFullLengthConstraints` | a length gone when one end is trimmed | the same |
| `testTrimEndEffectOnSymmetricConstraints` | ends symmetric about a point gone when one end is trimmed | the same |
| `testTrimEndEffectOnUnrelatedTangent` | a vertical line at x = 1 from y −2 to 2, tangent to a circle of radius 1, cut by a circle of radius 1.5, picked at 0.2: the tangent stays edge to edge | the same |
| `testSplitLineSegment` | 1 curve added, 1 joint (`Tangent` on main) | 1 line added sharing a new point, plus decision 1's joint |
| `testSplitCircle` | one arc (index unchanged) | decision 3 |
| `testSplitArcOfCircle` | 1 arc added, 2 `Coincident` (end and centres) | 1 arc added; the shared end and centre are points, with no constraints |

Others:
- `SketcherTests/TestCoincidentCommandGui.py`'s six tests become the merge command's tests: already one point, collapsing a curve (directly and through a chain), and a merge whose solve conflicts, refused.
- `TestSketchValidateCoincidents.testSingleMissingCoincidentCase` and `TestSketcherSolver.testThreeLinesWithCoincidences_1`/`_2` are already ported for `ends_not_joined` (#189). They become the merge's cases too: merging the found cluster leaves no warning.
- `testDegenratedGeometryCase` waits for a degenerate-curve warning (curves of 1e-7 mm or less that are distinct points), reported, never removed.

**Not ported, by kind:**
- `testConstraintAfterDeletingGeo` and `testIssue3245` test renumbering. Silico's IDs avoid it, though `testIssue3245_2`'s rule returns with expressions: a deleted constraint takes its expression and no other.
- The ellipse and B-spline variants.
- `testValidateConstraintsCase`, which the gate covers.

### Silico's own

FreeCAD has no test for how constraints move on split, for extend, or for most of the transfer rules. Silico's tests, with values worked by hand:

- **Split:**
  - a line with `horizontal` and `parallel` splits with no redundancy (1.1.3 reports one);
  - an `angle` on the split line survives (1.1.3 segfaults);
  - a point `on` each half goes to its own piece;
  - a `point_line_distance` from beside the second half goes to that piece;
  - a `length` becomes the `distance` between the outer points;
  - an arc's `radius` lands on one piece;
  - `tangent_at` at both of an arc's ends survives;
  - a `perpendicular` to a line neither piece touches survives, on the start-side piece (main drops it);
  - a `tangent_on` whose `at` lies on the second half moves to that piece; one whose `at` is the split point becomes `tangent_at`;
  - a point `on` the line's extension beyond `to` goes to the end piece;
  - refused, naming the curve: a split at `from`, at `to`, beyond an end, within 1e-7 mm of one, off the curve, or of a point;
  - after a split of a fully constrained sketch, every untouched point keeps its exact coordinates.
- **Delete:**
  - at a three-way joint every other curve still shares the point;
  - with a `tangent_at` to the deleted curve listed first or last, the result is the same (1.1.3 differs);
  - deleting one of two lines with an `angle` removes it;
  - deleting a `symmetric`'s `about` line removes it;
  - a point no curve uses after the edit is gone;
  - a batch equals one by one;
  - `origin` and the axes are refused.
- **Unjoin:**
  - the others stay joined;
  - the detached line keeps its `length`;
  - unjoining an unshared point is refused.
- **Merge:**
  - ends with a `horizontal_points`, an `on` to another curve, or a lock still warn and merge (1.1.3 reports 0 missing);
  - a merge that collapses a line through a chain is refused;
  - a merge that would leave an edge `tangent` at a shared end is refused (decision 8).
- **Trim:**
  - a tangent line cuts a circle, and one off by more than decision 6's contact tolerance doesn't (both sides of it, and of the end-on-curve tolerance);
  - collinear overlapping lines cut at the overlap's ends;
  - a sketch point doesn't cut;
  - a cutter end held `on` the trimmed curve becomes the piece's shared end, and its `on` is removed;
  - an `on` on the removed part is dropped;
  - undoing a trim restores the document byte for byte.
- **Extend:**
  - a line with a `length` slides rather than grows, to (5,0)–(15,0), as 1.1.3 does;
  - a shrink past the other end is refused;
  - a circle is refused.

## Open decisions

Open: none is decided. Each has a recommendation, for M5.0's design to propose and the maintainer to approve, change, or refuse, as M4.1's decisions were. They follow FreeCAD where it gives one answer, choose between 1.1.3 and main where they differ, and follow Silico's exactness rule where FreeCAD's tolerances would snap.

1. **A split line's two pieces.** 1.1.3 leaves them free to kink and copies `horizontal`, `vertical`, `parallel` and `angle` to both, which makes a constraint redundant when two are present. Main, since `6d06b61c7e`, holds them collinear with an endpoint tangency and puts those on the first piece. **Recommended: main's.** Join the pieces with `tangent_at` (`same`) at the split point, and put direction constraints on the start-side piece only. The shape and its freedoms are what they were, plus the split point's slide along the line, and nothing is redundant. Trim's pieces stay independent, as FreeCAD's do.
2. **A constraint that no piece can take.** With decision 1, a split's pieces lie on the original's line or share its circle, and D4's `on`, `point_line_distance`, edge `tangent` and `symmetric` about a line act on the whole line or circle, so every such constraint means on a piece what it meant before. **Recommended:** in a split, each goes to the piece whose span holds its point, else the nearer end's, else the start-side piece; nothing is refused or dropped. In a trim, where the pieces really are independent, a constraint no piece can take (an arc's edge `tangent` that no piece touches, `symmetric` about the trimmed line) is refused, naming it, so the person or agent removes it first. FreeCAD drops these silently.
3. **Splitting a circle.** FreeCAD gives one arc whose ends are one place, which the gate refuses. **Recommended: a circle splits at two points into two arcs**, as SolidWorks's Split Entities needs two points on a closed curve. A circle split at one point is refused, naming it.
4. **The split position.** FreeCAD projects any point onto the curve. **Recommended:** the request gives a position, and the new point goes at its projection onto the curve. A position farther than 1e-7 mm from the curve is refused, naming the distance: the gate's floor, not a snap. So is a position at, beyond, or within 1e-7 mm of an end.
5. **Trim's "the cut is the cutter's end".** FreeCAD merges at 5e-5 mm. **Recommended: share the cutter's point only when the cutter's end is held on the trimmed curve by a constraint** (an `on`, a `tangent_on` with the trimmed curve as `b`, which then becomes `tangent_at` by decision 8, or the end is already a point of the trimmed curve). Then it's exact, and the `on` is removed as implied. Otherwise the piece gets a new point `on` the cutter. A cutter end that's merely drawn there leaves two points at one place, which `ends_not_joined` names when they're within 1e-7 mm and `--merge-touching` merges. Likewise "two cuts at one place" means within 1e-7 mm, not 5e-5. This departs from five of FreeCAD's trim tests, whose cutters are only drawn (§9).
6. **Where trim finds intersections.** **Recommended: in the core, in closed form,** with the libm shim's math, the same on every platform. Lines and circular arcs are the only curves, so no worker is needed and nothing depends on OCCT's intersector. FreeCAD's three sources of cuts (crossings, tangent contacts and overlaps, other curves' ends on the curve) each become a case of that computation, with tests. Each needs a stated tolerance, since exact comparison finds a solved tangency only when rounding falls the right way. FreeCAD's intersector cuts a circle by a line at y = 5 + δ for δ = 1e-12 but not 1e-10 *(1.1.3, radius 5)*, and takes another curve's end within 1e-7 mm (`e10068a648`). Recommended: a tangent contact or an overlap counts when the gap is at most 1e-7 mm (`LENGTH_TOLERANCE`, the gate's floor), and so does an end on the curve, each tested on both sides. `x_axis` and `y_axis` don't cut, as by default in FreeCAD (main's opt-in, `196da068a0`, isn't in 1.1.3).
7. **Deleting points.** **Recommended:** deleting a curve deletes each of its points that no curve uses afterwards, with their constraints. Deleting a point is allowed only when no curve uses it; otherwise it's refused, naming the curves.
8. **A tangency at a new or merged shared end.** Trim turns the cutter's edge tangency into an end tangency, and a merge can make an edge `tangent` illegal (the gate's `tangent_at` rule). Both forms carry a `direction` that D4 stores and never derives. **Recommended:** the edit writes the `direction` the sketch as last solved shows. That is an act of the edit, done once, the way FreeCAD's tool records it, and stored. The edit's solve must then agree, or the edit is refused as M3.3 refuses a side that flips.
9. **Unjoining.** **Recommended** (§5): the curves that stay keep the shared point and every constraint on it. The detached curve's end gets a new point at the same coordinates with none. A `tangent_at` between the detached curve and another at that point is removed. Unjoining a point no other curve shares is refused.
10. **Merging.** **Recommended** (§6): the request names the points (or `--merge-touching` names each `ends_not_joined` cluster). The first point named survives at its own coordinates (`--merge-touching`: the cluster's least ID, its points being within 1e-7 mm); the others' curves and constraints are re-pointed to it. Refused, naming them: points already one, a curve using one point twice, a constraint naming one point twice, and an edge `tangent` left at a now-shared end (decision 8). The edit's solve decides the rest, and a conflict refuses it.
11. **Extend.** **Recommended:** a line's end is dragged by a `Length` along the line through the sketch worker's drag session (M3.7), so constraints win as in FreeCAD; an arc's start or end moves by an `Angle`, written, then the sketch solves. Refused: other kinds, the `center`, and a result of 1e-7 mm or less or flipped (FreeCAD doesn't check). Extending to the next curve, as SolidWorks's Extend Entities does, is a client's computation of the increment, as in FreeCAD's Gui.
