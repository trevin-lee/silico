# Sketch mode's constraint tools, as SolidWorks and Fusion have them, in Silico's forms

**Decided:** M5.0's open item 34 (docs/design/m5.0-app.md, section 12), Fable's C1 to C9 of 2026-11-14, kept whole in section 9 below. Where anything in sections 4 to 8 differs from them, the decisions govern. They depart most from this note in C2 (Fix is two dimensions from the origin, not a new form), C4 (Symmetric takes two points and a mirror only), and C5 (one rule: a relation that implies an `on` replaces it). They also narrow C3 (two rounds about one centre are added, not refused; a picked point only says which of several places), C6 (`symmetric` is either way round too), C8 (the origin is written as picked, never always `b`), and the refusals (one sentence per tool; the gate says the rest in its own words). So sections 4 to 8 are superseded wherever they say otherwise, §7's tests 9 and 10 and §8's plan among them.

A study note for the ten constraint buttons on sketch mode's toolbar: Horizontal, Vertical, Coincident, Tangent, Equal, Parallel, Perpendicular, Midpoint, Symmetric, and Fix. Today each is `Does::NotBuilt` (crates/silico-app/src/toolbar.rs, `constraint` at :367 and `CONSTRAINTS` at :473). The approved prototype only toasts "Select the curves or points first; the relation applies to them." (docs/design/m5-ui/js/interact.js:273; its buttons at app.js:387). So its model is: select first, then click the relation. M5.12's slice had to give the bracket's holes two diameters where the agent wrote `diameter` and `equal`, because Equal isn't built (docs/walkthrough/m5.md:66).

This note records what SolidWorks does (behaviour) and what Fusion does (UI only). For each tool it gives the selections it takes, the Silico form or edit each writes, and what it refuses, with the message. It ends in numbered decisions, C1 to C9, each with options and a recommendation. It decides nothing; the decisions stay with M5.0 and the maintainer.

**Sources.**

- The SOLIDWORKS 2027 Design Help (help.solidworks.com/2027, English), read on 2026-11-14. Every page cited was also read in the 2026 help, and its text is the same, word for word, the year aside. Pages are linked at the end.
- Autodesk's Fusion help (help.autodesk.com, unversioned), read on 2026-11-14: *Constraints in sketches* and its twelve pages, one per constraint. Also one post on Autodesk's own Fusion blog. Neither is the help, so the blog is cited by name, not marked *unofficial*; docs/ux/fusion.md treats Autodesk's blog the same way.
- Marked *unofficial*: two SOLIDWORKS resellers' guides (Javelin, GoEngineer); Product Design Online's Fusion course; and two Autodesk Community forum threads. Each is used only where the help is silent.
- FreeCAD's constraint commands, `src/Mod/Sketcher/Gui/CommandConstraints.cpp`, in the maintainer's clone at `876271d660` (main). Each behaviour cited was also found at the `1.1.3` tag. FreeCAD is reference only: never its UX.
- Silico at `main` (`9b756de9`):
  - `Form` and `Takes` in crates/silico-core/src/document/sketch.rs (:761, :895);
  - the gate's rules in crates/silico-core/src/document/sketch_gate.rs;
  - the solver's reading of `tangent_at` and `tangent_on` in crates/silico-core/src/sketch.rs (:1564 to :1613);
  - the sketch tools in crates/silico-core/src/sketch_tools/ (Smart Dimension in `dimension.rs` is the closest analogue: a selection turned into a form);
  - sketch mode's selection in crates/silico-app/src/sketch/ (`mod.rs`, `hit.rs`);
  - docs/design/m5.0-app.md: section 7, *The sketch edits*; section 9, *Sketching*; open items 28 and 32;
  - docs/study/sketch-tool-relations.md (#278's decisions A to G) and docs/study/smart-dimension.md (SD1 to SD9);
  - docs/ports/sketch-edit.md, §2 and §6.

## In brief

- **SolidWorks has two ways in.** You can select entities, then pick a relation the PropertyManager or the context toolbar offers for them. Or you can open *Add Relations* first and select entities into it. The help's own words: "Add entities to the list by selecting them in the graphics area." A resellers' guide (*unofficial*) calls Ctrl-select first "the preferred way". The list "includes only relations that are possible for the selected entity", so SolidWorks never meets a relation that doesn't fit. The help gives no message for one.
- **Fusion's help gives only the tool-first flow,** the same in all twelve constraint pages: click the tool, its icon shows by the cursor, select the geometry, and the geometry snaps into place. Autodesk's blog and a forum user add that a selection made first is used when the button is pressed. So Fusion takes both orders.
- **Several entities at once.** SolidWorks' Horizontal and Vertical take "one or more lines or two or more points". Equal takes "two or more lines or two or more arcs". Parallel, Equal, Collinear, and Coradial make one *relation group* for many entities. Silico's forms are pairwise, so a selection of n gives n − 1 constraints, or n for one-line forms. The help never says which entity of a pair moves. A reseller (*unofficial*) and Fusion's help both say the first selected is the base.
- **Over-defining.** SolidWorks adds a relation that conflicts: the sketch goes over defined and its entities turn yellow. A redundant relation leaves the sketch "still fully defined". Relations have no *driven* state, so the *Make Dimension Driven?* prompt is for dimensions only. Silico's `sketch_add` already behaves this way (SD7, with open item 32 as amended). Fusion refuses such a constraint instead ("Sketch geometry is over constrained", forum, *unofficial*); that is behaviour, and SolidWorks governs it.
- **After a relation is added, the selection stays** in SolidWorks, so more relations can be added to the same entities. FreeCAD clears it after every constraint.
- **Most tools map onto today's forms exactly:** `horizontal`, `vertical`, `horizontal_points`, `vertical_points`, `on`, `tangent`, `tangent_at`, `tangent_on`, `equal`, `parallel`, `perpendicular`, `midpoint`, and `symmetric`.
- **Three need a decision:**
  - Coincident of two points is SolidWorks' *Merge Points*, which "simply removes" the extra point (*unofficial*): Silico's `sketch_merge` (C1).
  - Fix has no form (C2).
  - Tangent has three forms, chosen by the sketch's structure (C3).
- **Not among the ten:** Collinear and Concentric. Both are on Fusion's panel and in SolidWorks' table, but not in the prototype. Silico would write them as two `on`s and as a merge of centre points.
- **The app's selection lacks two things:**
  - it holds members by ID in a `BTreeSet`, with no pick order;
  - the origin and the axes can't be selected.
  Coincident's surviving point, Equal's first member, and Symmetric's mirror line need pick order. Relations to the origin or an axis need the origin and axes (C9).
- **Format steward:** only C2's recommended `fix` form changes the schema. Everything else is written with today's forms and commands.

## 1. What SolidWorks does

### Adding relations

- *Add Relations* (Dimensions/Relations toolbar, or Tools > Relations > Add) opens the *Add Relations/Properties* PropertyManager. Its *Selected Entities* box "Displays the names of selected sketch entities. Add entities to the list by selecting them in the graphics area." *Add Relations* "Adds relations to the selected entities from the list." It also shows the selection's *Existing Relations* and its status ("Fully Defined, Under Defined, and so on") ([Add Relations PM]).
- Relations can also be added "when you select two existing entities. Existing Relations and Add Relations sections appear in all sketch entity PropertyManagers." "When you create relations, at least one of the items must be a sketch entity. Other items can be either a sketch entity or an edge, face, vertex, origin, plane, axis, or curve from another sketch" ([Geometric Relations]).
- In a line's or a point's PropertyManager, *Add Relations* reads: "The list includes only relations that are possible for the selected entity" ([Line Properties PM], [Point PM]).
- "If you right-click with one or more sketch entities selected, the toolbar that displays stays visible to allow you to add multiple relations. For example, you have two lines selected. You can add vertical and parallel relations to the lines and make them equal without making changes in the PropertyManager or displaying the toolbar" ([Sketch Relations Overview]).
- Context toolbars "appear and provide access to frequently performed actions" when items are selected, in sketches too ([Context Toolbars]). Its example: "Ctrl + select two lines. On the context toolbar, click Make Parallel" ([Context Toolbar dimensions]). A line being edited near horizontal or vertical gets *Make Horizontal* or *Make Vertical* outlined there ([Horizontal and Vertical]). No page lists which relations the context toolbar offers for each selection.
- *Unofficial:* "the preferred way is to use the control key, select the entities, and then use the PropertyManager to define the relation." "The available relations can change depending on the entities you have selected." After one is added: "Clear the selected entities to make another relation." "If you use Add Relations, you do not have to hold down the control key for selecting entities" ([Javelin 2026]).

### What each relation takes

The help's table ([Description of Sketch Relations]), for the ten tools and their neighbours:

| Relation | Entities to select | Resulting relations |
|---|---|---|
| Horizontal or Vertical | "One or more lines or two or more points." | "The lines become horizontal or vertical (as defined by the current sketch space). Points are aligned horizontally or vertically." |
| Coincident | "A point and a line, arc, or ellipse." | "The point lies on the line, arc, or ellipse." |
| Merge Points | "Two sketch points or endpoints." | "The two points are merged into a single point." |
| Tangent | "An arc, ellipse, or spline, and a line or arc." | "The two items remain tangent." |
| Equal | "Two or more lines or two or more arcs." | "The line lengths or radii remain equal." |
| Parallel | "Two or more lines." | "The items are parallel to each other." |
| Perpendicular | "Two lines." | "The two items are perpendicular to each other." (Coradial's sentence follows in the same cell, as docs/study/sketch-tool-relations.md §1 notes.) |
| Midpoint | "Two lines or a point and a line." | "The point remains at the midpoint of the line." |
| Symmetric | "A centerline and two points, lines, arcs, or ellipses." | "The items remain equidistant from the centerline, on a line perpendicular to the centerline." |
| Fix | "Any entity." | "The entity's size and location are fixed. However, the end points of a fixed line are free to move along the infinite line that underlies it. Also, the endpoints of an arc or elliptical segment are free to move along the underlying full circle or ellipse." |
| Intersection | "Two lines and one point." | "The point remains at the intersection of the lines." |
| Collinear | "Two or more lines." | "The items lie on the same infinite line." |
| Concentric | "Two or more arcs, or a point and an arc." | "The arcs share the same center point." |

Below the table: "When you create a relation to a line, the relation is to the infinite line … When you create a relation to an arc segment or elliptical segment, the relation is actually to the full circle or ellipse." That is Silico's `on` (docs/ux/solidworks.md §2).

- **Merge.** *Unofficial:* "After this relation has been made, there is no green icon showing this relation; the extra point is simply removed" ([GoEngineer 2025]). The help's table says only "merged into a single point". Neither says which point stays.
- **Midpoint of "two lines".** The table doesn't say what two lines give. *Unofficial:* "When two lines are used, one of the lines' endpoints will be selected as well as the second line … sets the endpoint of the first line to be the same as the midpoint of the second line" ([GoEngineer 2025]). So it is really a point and a line.
- **Symmetric of lines and arcs.** The table's result, "equidistant … on a line perpendicular to the centerline", is a rule for points. It doesn't say whether two lines' ends are paired, or whether two arcs' radii are made equal.
- **Fix.** The table above is the help's only text on it. Its Fix is a relation with no value shown. Fixing a line fixes its underlying infinite line, its ends left free along it.
- **Circles.** The help writes "arcs" where circles plainly count (Equal, Tangent, Concentric). A circle is taken here as an arc that closes.

### Several entities at once

- "You can create relations to multiple sketch entities as a single relation group … Relation groups are available for the following relation types: Parallel, Equal, Collinear, and Coradial." "Entities in the relation group have the same number." "To add an entity to an existing relation group, select an entity that is not in the relation group and press Ctrl + select the relation group icon" ([Relation Groups]).
- Horizontal and Vertical take "one or more lines or two or more points" ([Description of Sketch Relations]). The help doesn't say whether three lines made horizontal are one relation or three.
- **Which entity moves.** The help doesn't say.
  - *Unofficial:* for Equal, "the first item selected is considered the base item and the second item will be set to match it. This is not the case if one entity is defined by a dimension … This relation cannot be used if both items are defined by dimensions." For Symmetric, "the first entity selected sets the baseline and the second item will follow suit" ([GoEngineer 2025]).
  - Fusion's help says the same of its Equal (§2).

### Over-defining and redundant relations

- "A redundant perpendicular relation is added. The sketch is still fully defined." Then: "A redundant dimension is added. The sketch is over defined" ([FAQ Sketch Relations]).
- *Item Conflicts*: "Dimensions or relations are in conflict or are redundant. When you insert dimensions, they are assumed to be driving dimensions." "Over defined relations display the over defining sketch entities and relation callouts in yellow." Fixes: delete the relation's symbol, delete it in the entity's PropertyManager, or use *Display/Delete Relations* ([Item Conflicts]).
- SketchXpert: "If you select Always open this dialog when sketch error occurs, the SketchXpert PropertyManager appears as soon as you over-define 2D or 3D sketches" ([SketchXpert PM]).
- The *Make Dimension Driven?* prompt appears "when you add a dimension that over defines a sketch" ([Item Conflicts]). No page offers anything like it for a relation, and relations have no driven state.
- So SolidWorks adds a conflicting relation and the sketch reads over defined, and it adds a redundant one and the sketch keeps its state. No page says SolidWorks refuses a relation.

### After the relation

- The context toolbar "stays visible to allow you to add multiple relations" to the same selection ([Sketch Relations Overview]). The reseller's "Clear the selected entities to make another relation" ([Javelin 2026], *unofficial*) says the same: the selection stays.
- The help doesn't say whether one Undo takes back a relation group or one relation.

### What the help doesn't say

- What a relation that doesn't fit the selection does: the list never offers one.
- Which relations the context toolbar offers for each selection.
- Which entity moves when a relation is added.
- Whether a relation already present (a second Horizontal on a horizontal line) is offered again.
- What Symmetric does with lines' ends and arcs' radii; what Midpoint does with two lines.
- Which point a merge keeps.

## 2. What Fusion does (UI only)

Behaviour follows SolidWorks. These lines are kept for the UI. Where a Fusion line is behaviour, it says so.

- **The panel.** *Sketch > Constraints*: Horizontal/Vertical, Coincident, Tangent, Equal, Parallel, Perpendicular, Fix/UnFix, Midpoint, Concentric, Collinear, Symmetry, Curvature ([Fusion constraints]).
  - Fusion has one Horizontal/Vertical tool, which takes "whichever is closer to the current alignment". The approved prototype has two buttons, Horizontal and Vertical, and the prototype governs the UI.
  - The prototype has no Concentric, Collinear, or Curvature.
- **Each tool, tool first.** Every one of the twelve pages gives the same steps:
  - "Click Sketch > Constraints > [tool]."
  - "The [tool] constraint icon displays next to the cursor in the canvas."
  - "In the canvas, select the sketch geometry you want to constrain."
  - The geometry snaps into place, and "A constraint badge displays next to the constrained geometry in the canvas."

  (For example [Fusion horizontal/vertical], [Fusion parallel].)
- **What each takes, in Fusion's words:**
  - Horizontal/Vertical: "a single line, or two points".
  - Coincident: "two points or a point and a line or curve" ([Fusion coincident]).
  - Tangent: "a curve and another object so that they touch at a single point but never cross each other" ([Fusion tangent]).
  - Equal: "The sketch geometry snaps to match the size of the first object you selected" ([Fusion equal]).
  - Parallel: "two lines". Perpendicular: "two objects".
  - Midpoint: "a point or object to the midpoint of another object" ([Fusion midpoint]).
  - Symmetry: "select the sketch geometry you want to constrain. Select the line of symmetry", so the line comes last ([Fusion symmetry]).
  - Fix/UnFix: "The size and location of the sketch geometry is locked. The selected geometry turns green". "To unfix sketch geometry, select the Fix/UnFix tool again, then click the geometry in the canvas" ([Fusion fix]).
  - Concentric: "two or more arcs, circles, or ellipses to the same center point". Collinear: "two or more objects so that they share a common line".
- **Coincident's look.** "Point to point coincident constraints show no dot or constraint icon. Point to curve coincident constraints show the coincident icon." "To remove a coincident constraint, select the point then right-click and select Delete Coincident" ([Fusion coincident points]). That is Silico's shared point and `sketch_unjoin`.
- **Selection first.** Autodesk's blog: "Fusion also indicates valid constraints based on selected objects", and "Right-clicking reveals available constraints for selected objects" ([Fusion blog 2024]). *Unofficial:* "when pressing one of the constraint buttons, the existing selection when pressing the button is used to begin defining the constraint", and likewise for dimensions ([Fusion forum, selection]). So a selection made first seeds the tool's picks.
- **Repeating.** *Unofficial:* a course applies Tangent to an arc and the right line, then "selecting the arc again, followed by the left line", and "At any time, press the Escape key to clear the active Sketch Constraint" ([PDO day 17]). So the tool stays on until Esc. The help doesn't say.
- **A pick that doesn't fit.** The help doesn't say. Fusion's command API pre-highlights only entities valid for the active input (docs/ux/fusion.md, *Selection look*, [cmds]).
- **Over-constraining** (*behaviour, reference only*). Fusion refuses a constraint that would over-constrain, saying "Sketch Geometry is Over-constrained", and "Fusion does not highlight the constraint that is causing the error" ([Fusion forum, over-constrained], *unofficial*). SolidWorks adds the relation and marks the conflict (§1), and SolidWorks governs.

## 3. FreeCAD, where both are silent (reference only)

From `CommandConstraints.cpp` at the clone's `876271d660`, each also found at the `1.1.3` tag:

- **Equal and Parallel chain** a selection pairwise in selection order, `ids[i]` with `ids[i + 1]` (Equal :9661, Parallel :6845).
- **Coincident is a star.** The unified command constrains every point to the first one selected (`activatedCoincident`, :5312). Points and curves give a point-on-object for each point and each curve, skipping silently a point of the curve itself (`activatedPointOnObject`, :5235). Curves alone, if all are conics, give concentric.
- **Horizontal and Vertical** (`horVerActivated`, :4214):
  - lines get one constraint each;
  - points are chained pairwise;
  - with any line selected, the points are ignored;
  - a line that already has the constraint is refused: "The selected edge already has a horizontal constraint!" (`canHorVerBlock`, :4154).
- **Lock** (FreeCAD's Fix) writes a `DistanceX` and a `DistanceY` of the point from the origin, at its current coordinates (`CmdSketcherConstrainLock::activated`, :4593 to :4645).
- **Tangent substitutes.** Where one curve's end is held on the other by a point-on-object, the command deletes that constraint and adds an endpoint-to-edge tangency in one transaction: "Endpoint to edge tangency was applied. The point on object constraint was deleted." (`3a656d8597`, 2021; `substituteConstraintCombinations`, :7667).
- **Symmetric** in 1.1.3 takes "two points and a symmetry line, two points and a symmetry point or a line and a symmetry point". The third selected is the mirror. A line and a point make the line's ends symmetric about the point, its midpoint (:9774).
- **The selection is cleared** after every constraint (`finishTransactionAndUpdate`, :469-474).
- **An empty selection** arms the tool, which then waits for picks ("ContinuousConstraintMode", default on; `getConstraintSelection`, :4190).

## 4. Each tool, in Silico's forms

### Rules common to all ten

- **The selection.** The tools read sketch mode's selection in pick order (C9). It may hold the sketch's points, lines, circles, and arcs, construction or not, and the origin and the axes. A circle's or an arc's centre is a point (D2), so picking a centre picks a point.
- **One click, one edit.** A click on a button whose selection fits makes one edit:
  - a `sketch_add` of its constraints (one batch, checked whole by the gate);
  - or a `sketch_merge` (Coincident of points, C1);
  - or, where C5's substitution applies, a removal and an add, sent as one step.

  The new IDs come from the tool's `NewIds`, as the drawing tools' do. The add is made whatever the solve then finds (SD7).
- **Order.** `a` is the first picked and `b` the second, wherever the form has an `a` and a `b` and the structure doesn't fix them.
- **Read from the drawing.** Every choice the form must store (a tangent's `side`, a `tangent_at`'s or `tangent_on`'s `direction`, a mirror's pairing) is read from the newest `sketch` read's solved coordinates. Smart Dimension reads `point_line_distance`'s `side` the same way (SD1; D4: "given, never decided from geometry" means the request carries it).
- **Several members** (Horizontal and Vertical of points, Equal, Parallel): each other member is paired with the first picked (C8).
- **Refused for every tool, saying why** (the core's words; the app shows them in its toast, as the prototype toasts):
  - nothing selected: the prototype's own words, "Select the curves or points first; the relation applies to them.";
  - a constraint or a dimension selected: "a relation applies to points and curves: deselect constraint kt000003";
  - only the origin and the axes: "the origin and the axes are fixed: relate a member of the sketch" (Smart Dimension's `FixedOnly`, and the gate's rule that every constraint names a member of its sketch);
  - every constraint the click would write already in the sketch, the same type on the same references, either order for the symmetric forms: "line ne000001 is already horizontal: nothing was added" (C6).
- **Nothing is inferred and nothing filtered.** #278's G leaves out an inferred relation the sketch already fixes. A constraint tool's relations are the person's own, as "a tool's own relations always stay" (M5.0 §9), so G doesn't apply.

### Horizontal (Vertical is the same, with `vertical` and `vertical_points`)

| Selection | Writes |
|---|---|
| One or more lines | One `horizontal {line}` per line, in pick order |
| Two or more points (the origin may be one) | One `horizontal_points {a, b}` per point after the first, paired with the first picked (C8); the origin is always `b`, as the drawing tools write #278's A (`infer.rs`:373) |

Refused:

- one point alone: "Horizontal takes two or more points: select another, or the origin";
- lines and points together: "Horizontal takes lines, or points, not both". FreeCAD ignores the points; Silico does what was asked, or nothing;
- a circle or an arc: "Horizontal takes lines or points: circle ce000001 is neither; select its centre to line it up";
- an axis: "x_axis is fixed: only a line of the sketch can be made horizontal".

A line that's `vertical` already is made horizontal too, and the sketch reads the conflict (SD7, §6).

### Coincident

| Selection | Writes |
|---|---|
| A point and a curve (line, circle, arc, or axis) | `on {point, curve}` |
| One or more points and one or more curves | One `on` for each point and each curve, points in pick order, then curves. One point and two lines is SolidWorks' *Intersection*; the product is FreeCAD's |
| The origin and a curve | `on {point: origin, curve}`: the curve through the origin (`on`'s `point` takes the origin) |
| Two or more points of the sketch | `sketch_merge {points}` in pick order: the first picked survives (C1) |
| One point and the origin | `horizontal_points {a: point, b: origin}` and `vertical_points {a: point, b: origin}` (#278's A, as the drawing tools write it) |

Refused:

- curves only: "Coincident takes a point and a curve, or two or more points: to make circles concentric, select their centres";
- one point alone: the same words;
- the origin with two or more points: "Coincident to the origin takes one point: make the points one first";
- a curve's own point: "point pe000003 is an end of line ne000002 already" or "point pe000005 is circle ce000001's centre: it can't lie on its own circle". These are the gate's rule (a curve's own point isn't constrained onto it), said before the add;
- a point an end of a curve that an edge `tangent` makes tangent to the target. The gate refuses that `on` ("a curve ending on another is tangent where it ends"); C5 recommends substituting a `tangent_on`;
- whatever `sketch_merge` refuses: points already one; a curve that would use one point twice ("merging pa000004 and pa000005 would give line na000004 one point at both ends"); a constraint that would name one point twice; and a merge whose solve is worse than before (C1).

### Tangent

| Selection | Writes |
|---|---|
| A line (or an axis) and a circle or an arc that share no end, neither's end held on the other | `tangent {a, b, side}`, `side` `left` or `right`: the side of the line, facing from its `from` to its `to` (an axis faces +x or +y), that the round's centre is drawn on |
| Two circles or arcs, likewise | `tangent {a, b, side}`, `side` `outside` where the centres are at least the larger radius apart, else `inside` (FreeCAD's own rule, `d ≥ r1 and d ≥ r2`, docs/ports/sketcher.md, *Constraints*) |
| A line and an arc, or two arcs, sharing one end | `tangent_at {a, b, at: the shared point, direction}`, `same` where the drawing runs on through the point (less than a right angle's turn), else `opposite` (C3) |
| Two curves, one's end held `on` the other | `tangent_on {a: the curve that ends there, at: the end, b, direction}`, `same` where `a`'s direction leaving `at` and `b`'s own direction there agree; the `on` is replaced (C5) |
| Two curves and a point: an end of both, or an end of one held on the other | `tangent_at` or `tangent_on` at that point (FreeCAD's three-element tangent; C3) |

Refused:

- two lines, or a line and an axis, sharing an end or not: "two lines can't be tangent: use Parallel, or Collinear". These are the gate's words; SolidWorks' Tangent takes "an arc … and a line or arc";
- other counts: "Tangent takes two curves: a circle or arc, and a line or another circle or arc";
- a round's centre on the line exactly: "the centre of circle ce000002 is on line ne000002: which side it's tangent on is unclear; move one off the other first". Smart Dimension refuses a point on its line the same way (`Message::NoDistance`);
- curves sharing both their ends, no point picked: "line ne000007 and arc ae000002 meet at two points, pe000011 and pe000012: select the one they're tangent at too";
- two rounds about one centre point: "circles ce000001 and ce000003 share their centre: they can't touch at one point".

### Equal

| Selection | Writes |
|---|---|
| Two or more lines | One `equal {a, b}` per line after the first, `a` the first picked (C8) |
| Two or more circles or arcs, in any mix | The same |

Refused:

- lines with circles or arcs: "an equal takes two lines, or two circles or arcs" (the gate's words);
- a point: "Equal takes lines, or circles and arcs: point pe000007 is neither";
- an axis: "an axis has no length: Equal takes lines, or circles and arcs" (FreeCAD: "Sketch axes cannot be used in equality constraints");
- one curve: "Equal takes two or more lines, or two or more circles and arcs".

### Parallel

| Selection | Writes |
|---|---|
| Two or more lines, an axis among them or not | One `parallel {a, b}` per member after the first, `a` the first picked (C8) |

Refused: a point, circle, or arc ("Parallel takes two or more lines"); both axes alone (fixed only).

### Perpendicular

| Selection | Writes |
|---|---|
| Two lines, or a line and an axis | `perpendicular {a, b}` |

Refused: more or fewer than two, or a circle, arc, or point: "Perpendicular takes two lines". SolidWorks' table says "Two lines". FreeCAD's perpendicular of a line and a circle (a normal) has no form. Both axes: fixed only.

### Midpoint

| Selection | Writes |
|---|---|
| A point (or the origin) and a line | `midpoint {point, line}` |

Refused:

- a point and an arc: "a point at an arc's middle has no form yet: Midpoint takes a point and a line". `midpoint` takes a line (docs/study/sketch-tool-relations.md §9);
- a point and an axis: "an axis has no middle: Midpoint takes a point and a line";
- two lines: "Midpoint takes a point and a line: select the end you mean and the other line" (§1: SolidWorks' "two lines" is an end and a line);
- the line's own end: "point pe000001 is an end of line ne000001 itself" (the gate's rule);
- more than one point or line: "Midpoint takes one point and one line".

A point already held `on` the line gets the `midpoint`, and the `on` stays, reading `redundant` (C5).

### Symmetric

| Selection | Writes |
|---|---|
| Two points and a line or an axis | `symmetric {a, b, about: the line}` |
| Two points and a third point, or the origin | `symmetric {a, b, about: the point}`. SolidWorks requires a centreline; this is FreeCAD's (C4) |
| Two lines and a line or an axis | Two `symmetric`s, each end of the first line paired with an end of the second; a shared end gets `on {point, about}` instead (C4) |
| Two circles, or two arcs, and a line or an axis | `symmetric {a: first's centre, b: second's centre, about}` (C4) |

`about`, the mirror, is decided by kinds where they decide, else it is the last picked (C4):

- the axis, the origin, or the one line among points;
- among three lines, the one construction line or axis.

Refused:

- other counts: "Symmetric takes two points, lines, circles, or arcs, and the line or point they mirror about";
- two of different kinds mirrored: "Symmetric mirrors two of a kind: line ne000001 and circle ce000001 aren't";
- `about` one of the mirrored (the gate's); a line's own ends mirrored about it ("a symmetric's a and b, pe000001 and pe000002, are the ends of ne000001 itself"); an end of `about` mirrored ("… mirrors onto itself"), each in the gate's words.

### Fix

| Selection | Writes |
|---|---|
| A point (a circle's or an arc's centre included) | C2 (a), recommended: `fix {point, x, y}` at the newest read's coordinates, rounded by the core to the nanometre, as SD2 rounds a skipped Modify box. C2 (b), the fallback: `distance_x {a: origin, b: point, value: x, driving: true}` and `distance_y` likewise |
| A line, circle, or arc | Refused in M5: "Fix takes points for now: select the line's ends". SolidWorks' fix of a curve (§1) is C2's follow-up |

Refused: the origin ("the origin is fixed already"); an axis; a point already fixed (C6).

## 5. Decisions

**C1. Coincident of two points.** Silico has no coincidence between points: two points that coincide are one point (D2).

- **(a) `sketch_merge`, the points in pick order.** It is SolidWorks' *Merge Points*, whose extra point "is simply removed" (§1). The rules are M5.0 section 7's, and docs/ports/sketch-edit.md §6 (decision 10):
  - the first point named survives at its own coordinates;
  - every curve and constraint naming another is re-pointed to it, and the others are removed;
  - an edge `tangent` between two curves that then share an end becomes a `tangent_at`, keeping its ID, its `direction` from the tangent's `side` (open item 13);
  - an `on` the merge makes hold a curve's end on that curve is removed as implied (open item 13b);
  - it makes no member and takes no `ids`;
  - refused: points already one (`cb2780e2fa`); a curve that would use one point twice, through any chain (`084379651a`, #20771); a constraint that would name one point twice; a merged sketch the gate refuses;
  - refused too: a solve worse than the one before, by FreeCAD main's ranking (`shouldRejectCoincidentSolve`, `cb2780e2fa`). A solve that fails from the kept point's place is refused as `solve_failed`, suggesting "name the point the constraints hold first, or move the points together".
- **(b) A new point-to-point coincidence form.** Rejected by D2's design: a shared point is the coincidence.
- **(c) Refuse two points.** Leaves Coincident half built.

*Recommended:* (a), with the first picked surviving. Fusion's Equal matches "the first object you selected", and the resellers' "first is the base" (§1) reads the same way. The point and the origin stay #278's A pair.

**The cost:** Coincident of points refuses a merge whose solve is worse, where the other nine tools add and let the sketch read the conflict (SD7). SolidWorks would add it and turn the sketch yellow. The refusal is M5.0 section 7's common rule for an edit that writes structure, and FreeCAD main's Coincident command rolls back the same cases. So this note keeps it, as a stated divergence from SolidWorks, shown as the refusal's message and elements.

**C2. Fix.** No form holds a point where it is. A sketch's coordinates are the solver's output (D3), so a fix has to carry its place.

- **(a) A new relation form, `{"type": "fix", "point": ID, "x": Length, "y": Length}`.**
  - It has no `driving`, so it is a relation, as SolidWorks' Fix is.
  - The solver rows are two fixed coordinates, PlaneGCS's, as FreeCAD's Lock sends them. So FreeCAD's Lock (its `DistanceX` and `DistanceY` of one point) is the oracle.
  - The approved prototype draws Fix as a glyph (`GLYPH.fix`, docs/design/m5-ui/js/sketch.js:8), not as two dimensions.
  - Fix of a line, circle, or arc (SolidWorks: a line's infinite line fixed, its ends free along it) comes later, needing its own rows and oracle cases.
  - **A schema change** (`CONSTRAINT_TYPES`, the JSON Schema, the protected `tests/contract/`), for the format steward and the maintainer.
- **(b) `distance_x` and `distance_y` from `origin`,** driving, at the read's coordinates rounded to the nanometre. This is FreeCAD's Lock exactly, and needs no schema change. But a Fix the person placed shows as two dimensions, which SolidWorks wouldn't show, and the prototype's Fix glyph is never drawn. Fusion's UnFix (another click, §2) would have to recognise its own pair, a heuristic.
- **(c) Leave Fix out of M5.** The button stays `NotBuilt`, its tooltip saying so.

*Recommended:* (a) for points, flagged to the format steward. The button stays `NotBuilt` until the form is approved, so (c) until then. (b) only if the maintainer wants Fix in M5 without a schema change.

**C3. Tangent's three forms.** The gate already chooses among them by structure, so the tool follows it:

- curves sharing an end are tangent there or not at all (`tangent_at`; `180d722a53`);
- a curve whose end is held on the other is tangent where it ends (`tangent_on`; `3a656d8597`);
- otherwise, an edge `tangent`.

What the tool must add is the choice each form stores, read from the drawing:

- **`tangent`'s side.** For a line and a round, the side its centre is drawn on: the sign of `(to − from) × (centre − from)`, positive `left`, negative `right`. Zero is refused, as above. For two rounds, `outside` where the centres are at least the larger radius apart. That is FreeCAD's own rule, which picks the nearer of the two contacts: inside where `d < max(r1, r2)`, since `max(r1, r2)` is midway between `|r1 − r2|` and `r1 + r2`.
- **`tangent_at`'s direction.** The solver's meaning is "a path along `a` into `at` and on along `b`" being smooth (`same`; sketch.rs:1595). So take `a`'s direction arriving at `at` and `b`'s direction leaving it. `same` where their dot product is at least 0, else `opposite`. Each direction is a line's `to − from`, or an arc's own counter-clockwise tangent, reversed where the curve's end or start makes it so. A right angle exactly goes to `same`, as trim's rule for a `tangent_on`'s direction breaks the tie (`drawn`, crates/silico-core/src/document/sketch_edit/trim.rs:520 to :546: `>= 0.0` is `same`).
- **`tangent_on`'s direction.** `a`'s direction leaving `at` against `b`'s own (sketch.rs:1570): `same` where their dot product is at least 0, trim's own rule for it.
- **Two curves sharing both ends** (a line and an arc closing a D, or a split circle's two arcs) have no one point. They are refused unless the person picks the point too. That three-element form is FreeCAD's; SolidWorks' table doesn't have it.
- **Two lines** are refused, as SolidWorks' Tangent takes an arc. The gate allows `tangent_at` of two lines (split holds a line's pieces collinear with one), but the tool doesn't offer it: that is Collinear.

*Recommended:* as above.

**C4. Symmetric's selections.**

- **Two points and a line or an axis.** SolidWorks' case. `about` may be any line, not only a construction line: the gate takes any line, and with points the line is unambiguous.
- **Two points and a point, or the origin.** Not SolidWorks' (it requires a centreline). It is FreeCAD's, and the form takes it. *Recommended:* offered, since the form exists and the selection says it exactly.
- **Two lines and a line.** Two `symmetric`s, pairing the ends.
  - Of the two pairings, from-to-from and from-to-to, take the one whose ends lie nearer the mirror images of the first line's ends, summing the two distances. A tie goes from-to-from.
  - A shared end of the two lines gets `on {point, about}` in place of a pair: a point mirrored onto itself lies on the mirror. Nothing is written where it is `about`'s own end.
  - SolidWorks' help doesn't say whether its line-to-line Symmetric holds the ends or only the infinite lines (§1). Pairing the ends is the stronger reading, and what Mirror's result needs.
- **Two circles, or two arcs, and a line.** `symmetric` of their centres only, as the help's "equidistant" rule reads for a point. Radii are Equal's to hold. Mirroring an arc's ends crosswise as well would write one constraint more than its freedoms take (an arc's five), so arcs stay by their centres. The person adds what more they mean.
- **Which member is `about`.** Kinds decide where they can: the one axis or origin, the one line among points, the one construction line or axis among three lines. Otherwise it is the last picked, as Fusion's steps ("Select the line of symmetry" last) and FreeCAD's take it. That needs pick order (C9).

*Recommended:* all five as above.

**C5. A relation that meets an `on` the sketch has.** Three cases:

- **Tangent at an end held `on` the other curve.** A `tangent` there is refused by the gate's tangent rule, whose advice is "make the two one tangent_on". Adding the `tangent_on` beside the `on` leaves the `on` implied, and it reads `redundant`.
- **Coincident onto a curve that an edge `tangent` already makes tangent to the point's curve.** The gate refuses the `on`, with the same advice.
- **Midpoint of a point already `on` the line.** The `on` is implied by the `midpoint`, and reads `redundant`. The gate allows both.

Options:

- **(a)** Add, and leave the `on`. It reads `redundant` with its own glyph, and the sketch reads by its freedoms (open item 32 as amended). The two gate-refused cases are then refused, in the gate's words.
- **(b)** Substitute, as FreeCAD's tangent and coincident commands do (`3a656d8597`, `620188073e`): remove the `on` (or the edge `tangent`) and add the `tangent_on`, as one step. There are two ways:
  - (b1) a `sketch_remove` then a `sketch_add`, grouped by the app as one undo step, as a dialog's OK'd edits are one (M5.0 §9). The remove is undone if the add is refused. No contract changes, and an agent does the same two calls.
  - (b2) one typed edit (a `remove` list on `sketch_add`, say). That is atomic, but a change to the protected command contract.
- **(c)** Refuse, with the gate's advice. The person can't follow that advice from the UI.

*Recommended:* (b1) where the gate's rule asks for the substitution (the first two cases), since it is FreeCAD's and the gate's own advice. (a) where the new relation only implies the `on` (Midpoint), since SolidWorks keeps a redundant relation and the sketch stays as defined as it was (§1).

**C6. A relation already there.** Same type, same references (either order for `parallel`, `perpendicular`, `equal`, `horizontal_points`, and `vertical_points`).

- (a) Add it anyway. The core takes it (SD7), and it reads `redundant`.
- (b) Add nothing, and say so. That is FreeCAD's ("The selected edge already has a horizontal constraint!"), and #278's D ("adds nothing, and the tool says so").

SolidWorks' list "includes only relations that are possible", and whether it offers a second Horizontal is unknown (§1). *Recommended:* (b), member by member. Where only some of a selection's constraints repeat, the rest are added, and the message counts those left out. A relation that is implied but not identical (a perpendicular between a horizontal and a vertical line) is added and reads `redundant`, exactly as SolidWorks' FAQ shows it.

**C7. Selection first, tool first, or both.**

- **(a) Selection first only.** The approved prototype's toast, and SolidWorks' context toolbar and Ctrl-select (§1).
- **(b) Both, as Fusion.** With nothing selected, a button arms the tool: its icon by the cursor, picks gathered, applied when they complete, the tool kept until Esc (§2; *unofficial* for the repeat). A selection already made seeds the picks (the forum).
- **(c) Tool first only.** Fusion's help alone.

*Recommended:* (a) for this work, as the prototype draws it. The core function from a selection to an edit is the same for all three. (b) is the Fusion way and costs a picks accumulator in the core and an armed state, a cursor glyph, and Esc in the app. Those change the approved prototype, so it is the maintainer's to ask for (open item 1).

**C8. Several members: a star or a chain.**

- **(a) A star:** each member paired with the first picked.
- **(b) A chain:** each with the next, FreeCAD's Equal, Parallel, and Horizontal points.

Both write n − 1 constraints and hold the same geometry. They differ when a member is deleted: deleting a curve removes every constraint naming it.

- In a chain, deleting any member but the ends leaves the two halves unrelated.
- In a star, only deleting the first does.

SolidWorks' relation group, one numbered relation for all, survives either. FreeCAD's own Coincident is a star. Fusion's Equal and the resellers make the first the base.

*Recommended:* (a). It has fewer ways to lose a relation silently, and it reads as the first-is-base the sources describe. **Not promised:** which member moves. The solve decides, since `sketch_add` writes no positions, so the help's "the second item will be set to match" isn't honoured as such (open item 3).

**C9. The selection the tools read.** Sketch mode's selection is `BTreeSet<Hit>`, members by ID and kind, in ID order (crates/silico-app/src/sketch/mod.rs:298; `Hit`, hit.rs:27). It has no pick order, and `hit` never returns the origin or an axis.

- C1's survivor, C4's `about`, C8's first, and `a` and `b` need pick order.
- Coincident, Horizontal, and Vertical with the origin, Parallel or Perpendicular to an axis, Symmetric about one, Tangent to an axis, and the origin at a line's middle need the origin and axes.

*Recommended:*

- The selection becomes an ordered set in pick order (`IndexSet`, which CLAUDE.md admits). It is deterministic, being the person's own input. Open item 24's rules stay: a plain click replaces, a Ctrl-click toggles, and empty space or Esc clears.
- Its members become `Reference`s, and the origin and the axes are hit as Smart Dimension picks them (`View::pick`, crates/silico-core/src/sketch_tools/infer.rs:657): a point or the origin, whichever is nearer (the point on a tie), else the nearest curve, an axis anywhere along it.
- This amends M5.7's selection in sketch mode, so it is named here as a decision, not an implementation detail.

## 6. The interaction

- **Selection first** (C7): select with clicks, Ctrl-clicks, or a box, then click the relation's button.
  - Buttons stay enabled whenever a sketch is open, as the prototype draws them (no `disabled`). Their tooltip stays "Select the curves or points first".
  - SolidWorks only offers relations that fit, and Fusion "indicates valid constraints" (§1, §2). Dimming the buttons that don't fit would be a change to the prototype, so it is left as open item 2.
- **A click with an unfit selection** adds nothing and shows the tool's reason in the toast, as the prototype toasts:
  - with nothing selected, the prototype's words: "Select the curves or points first; the relation applies to them.";
  - otherwise the core's message (§4), naming the member that doesn't fit and what the tool takes.

  The selection is left as it was, so the person can fix it.
- **The selection stays** after a relation is added, as SolidWorks' context toolbar "stays visible to allow you to add multiple relations" (§1). FreeCAD clears it; Silico doesn't follow.
  - Members the edit removed (a merge's other points) leave it when the read comes, as `SketchMode::took` already drops members a read no longer holds (mod.rs:416).
- **Undo.** One click is one step: one `sketch_add` or one `sketch_merge`, one revision, and sketch mode's Undo takes it back. C5's substitution is two commands grouped as one step, undone together. The step is named as sketch mode names its other edits, by the relation ("Horizontal").
- **A relation that is redundant or over-defines** is added (SD7; M5.0 §7, `sketch_add`; sketch-edit §2: FreeCAD 1.1.3 keeps a conflicting constraint too). The `sketch` read then says what the solve found (open item 32 as amended; M5.0 section 7, `sketch_add`):
  - a conflict reads `over_defined`, the constraints in it `conflicting`, the members it governs `over_defined`;
  - a redundant relation reads `redundant` itself and marks no member, and the sketch reads by its freedoms.

  SolidWorks reads the same: "A redundant perpendicular relation is added. The sketch is still fully defined", and conflicting relations "display … in yellow" (§1).
  - Relations have no driven state, so SD7's three-way offer (driven, leave it, cancel) is for dimensions only, as SolidWorks' *Make Dimension Driven?* is. The person deletes a relation they don't want, or undoes it.
  - SketchXpert's automatic opening has no counterpart in M5. What's Wrong and the error card show the conflict.
  - The exception is C1's merge, refused where its solve is worse.
- **Esc** leaves nothing to cancel with selection first: a click is one edit. Esc clears the selection, as open item 24 has it.

## 7. Tests, each worked by hand

Each case is a headless test of the core function (`sketch_tools`, beside Smart Dimension), given a sketch, a selection in pick order, and a source of IDs. It returns the edit, which the test then puts through the gate (`sketch_edit::add` or `merge`), as the dimension tests do (crates/silico-core/src/sketch_tools/tests/dimension.rs). The expected values come from the forms' definitions and plain geometry, never from the tool. New constraint IDs are the test source's, written here `kt000101` and on.

**The sketch**, `s0000005` on `xy`:

- Points:
  - pe000001 (0, 0), pe000002 (40, 1), pe000003 (40, 21), pe000004 (2, 20);
  - pe000005 (20, 10), centre of circle ce000001, radius 5;
  - pe000006 (60, 10), centre of circle ce000002, radius 4;
  - pe000007 (20, 30) and pe000008 (−10, 30), lone points;
  - pe000009 (50, 21), centre of arc ae000001; pe00000a (50, 31), its start;
  - pe00000c (30, −10) and pe00000d (30, 50); pe00000e (50, 0) and pe00000f (55, 5).
- Lines:
  - ne000001 pe000001→pe000002; ne000002 pe000002→pe000003; ne000003 pe000003→pe000004; ne000004 pe000004→pe000001;
  - ne000005 pe00000c→pe00000d, construction (the centreline x = 30);
  - ne000006 pe00000e→pe00000f.
- Arc ae000001: about pe000009, from pe00000a to pe000003, counter-clockwise from 90° to 180°, radius 10 at both ends.
- Constraint kt000001: `vertical {line: ne000002}`.

1. **Horizontal**, [ne000001, ne000003]. One `horizontal` each, in pick order:

   ```json
   {"session": "s1", "revision": 7, "sketch": "s0000005", "points": [], "lines": [], "circles": [], "arcs": [],
    "constraints": [{"id": "kt000101", "type": "horizontal", "line": "ne000001"},
                    {"id": "kt000102", "type": "horizontal", "line": "ne000003"}]}
   ```

   The same selection with pe000007 added is refused: "Horizontal takes lines, or points, not both".
2. **Vertical**, [pe000007, pe000005]: `[{"id": "kt000101", "type": "vertical_points", "a": "pe000007", "b": "pe000005"}]`. [ce000001] alone is refused: "Vertical takes lines or points: circle ce000001 is neither; select its centre to line it up".
3. **Coincident:**
   - [pe000007, ne000003]: `[{"id": "kt000101", "type": "on", "point": "pe000007", "curve": "ne000003"}]`.
   - [pe000008, pe000004]: `sketch_merge` `{"session": "s1", "revision": 7, "sketch": "s0000005", "points": ["pe000008", "pe000004"]}`. pe000008 survives at (−10, 30): ne000003 and ne000004 then end there, and pe000004 is removed.
   - [pe000007, origin]: `[{"id": "kt000101", "type": "horizontal_points", "a": "pe000007", "b": "origin"}, {"id": "kt000102", "type": "vertical_points", "a": "pe000007", "b": "origin"}]`.
   - [pe000003, ne000002] is refused: "point pe000003 is an end of line ne000002 already".
4. **Tangent:**
   - [ne000002, ce000002]. ne000002's direction is (0, 20) and the centre less its `from` is (20, 9). The cross product is 0 × 9 − 20 × 20 = −400 < 0, so the circle is on its right: `[{"id": "kt000101", "type": "tangent", "a": "ne000002", "b": "ce000002", "side": "right"}]`.
   - [ne000002, ae000001]. They share pe000003, ne000002's `to` and ae000001's `end`. Arriving along ne000002 is (0, 20). The arc's own direction at (40, 21) is (−(21 − 21), 40 − 50) = (0, −10); leaving it from its end, reversed, that is (0, 10). The dot product is 200 ≥ 0, so `same`: `[{"id": "kt000101", "type": "tangent_at", "a": "ne000002", "b": "ae000001", "at": "pe000003", "direction": "same"}]`.
   - [ne000001, ne000003] is refused: "two lines can't be tangent: use Parallel, or Collinear".
5. **Equal**, [ce000001, ce000002, ae000001]. A star from the first (C8): `[{"id": "kt000101", "type": "equal", "a": "ce000001", "b": "ce000002"}, {"id": "kt000102", "type": "equal", "a": "ce000001", "b": "ae000001"}]`. [ne000001, ce000001] is refused: "an equal takes two lines, or two circles or arcs".
6. **Parallel**, [ne000001, ne000003]: `[{"id": "kt000101", "type": "parallel", "a": "ne000001", "b": "ne000003"}]`. [ne000001, ce000001] is refused: "Parallel takes two or more lines".
7. **Perpendicular**, [ne000001, ne000004]: `[{"id": "kt000101", "type": "perpendicular", "a": "ne000001", "b": "ne000004"}]`. [ne000001, ne000003, ne000004] is refused: "Perpendicular takes two lines".
8. **Midpoint**, [pe000007, ne000001], or the same in the other order: `[{"id": "kt000101", "type": "midpoint", "point": "pe000007", "line": "ne000001"}]`. [pe000007, ae000001] is refused: "a point at an arc's middle has no form yet: Midpoint takes a point and a line".
9. **Symmetric:**
   - [pe000007, pe000008, ne000005]: `[{"id": "kt000101", "type": "symmetric", "a": "pe000007", "b": "pe000008", "about": "ne000005"}]`.
   - Lines, [ne000001, ne000006, ne000005]. About x = 30, ne000001's ends mirror to (60, 0) and (20, 1).
     - From-to-from pairs (60, 0) with pe00000e (50, 0), 10, and (20, 1) with pe00000f (55, 5), √1241 = 35.228: 45.228.
     - Crosswise pairs (60, 0) with pe00000f, √50 = 7.071, and (20, 1) with pe00000e, √901 = 30.017: 37.088.
     - Crosswise is nearer: `[{"id": "kt000101", "type": "symmetric", "a": "pe000001", "b": "pe00000f", "about": "ne000005"}, {"id": "kt000102", "type": "symmetric", "a": "pe000002", "b": "pe00000e", "about": "ne000005"}]`.
   - [ne000001, ce000001, ne000005] is refused: "Symmetric mirrors two of a kind: line ne000001 and circle ce000001 aren't".
10. **Fix**, [pe000007]:
    - with C2 (a): `[{"id": "kt000101", "type": "fix", "point": "pe000007", "x": 20, "y": 30}]`;
    - with (b): `[{"id": "kt000101", "type": "distance_x", "a": "origin", "b": "pe000007", "value": 20, "driving": true}, {"id": "kt000102", "type": "distance_y", "a": "origin", "b": "pe000007", "value": 30, "driving": true}]`.
    - [ne000001] is refused: "Fix takes points for now: select the line's ends".
11. **Repeat** (C6): [ne000002] with Vertical adds nothing and says "line ne000002 is already vertical: nothing was added", since kt000001 is that constraint.

**App tests** (`egui_kittest`, as sketch mode's are):

- select ne000001 and, with Ctrl, ne000003; press Parallel through the accessibility tree; the next read holds one `parallel` naming both, and both lines are still selected;
- press Equal with nothing selected: the toast reads the prototype's words, and no command is sent;
- Coincident on two points sends one `sketch_merge`, and the removed point leaves the selection when the read comes;
- Undo after a relation takes it back in one step.

## 8. Open items, and a plan

**For the maintainer.**

1. **Tool first** (C7): Fusion's armed tool, which changes the approved prototype (a cursor glyph, an armed button, Esc).
2. **Buttons that don't fit the selection:** dimmed, as SolidWorks offers only what fits and Fusion "indicates valid constraints", or enabled with a toast, as the prototype draws them.
3. **"The first is the base"** (C8, Fusion's Equal, the resellers') isn't promised: the solve decides what moves.
4. **Collinear and Concentric** aren't in the prototype, though both are on Fusion's panel and in SolidWorks' table.
   - Collinear is two `on`s, both ends of one line on the other. Lines sharing an end could take a `tangent_at` (`same`) instead.
   - Concentric is a merge of centre points. A point and an arc is that point merged with the centre.
   - Adding the buttons is a prototype change.
5. **Fix** (C2): a new form, a schema change, for the format steward and the maintainer.
6. **Unknown in the help:** what SolidWorks' Symmetric does with lines' ends and arcs' radii (C4); what Midpoint does with two lines; which point Merge keeps (C1); whether an existing relation is offered again (C6).
7. **docs/ux/solidworks.md** has no lines on adding relations by hand (§2 covers automatic relations; *Not covered* doesn't list them). §1 of this note could go there, as Smart Dimension's study did.

**Format steward:** C2 (a), the `fix` form. C5 (b2) would change the command contract instead, and isn't recommended. No other schema change.

**A plan, core first** (each PR after `/adversarial-review`, with `/break-it` on the core function):

1. **Core: the relations, headless.** A `sketch_tools::relate(sketch, relation, picks, ids)`, from the newest read's members and a selection in pick order to one edit: an add, a merge, or C5's remove and add. It has a `Message` per refusal (§4) and is tested by §7's cases and the refusals. Nine tools, Fix aside. No command or contract changes.
2. **App: the selection.** C9: pick order, the origin and axes hit as `View::pick` picks them, open item 24's rules kept, and the tests of M5.7's sketch selection updated.
3. **App: the buttons.**
   - `Does::Constraint(relation)` in place of `Does::NotBuilt` for nine buttons; a click sends what `relate` returns through the tab's queue (`sketch_add`, `sketch_merge`, or the grouped pair).
   - A refusal shows in the toast; the step is named for Undo.
   - The `egui_kittest` cases above.
   - The walkthrough's Equal finding is struck (docs/walkthrough/m5.md:66), and the slice could then write the holes' `equal` as the MCP bracket does.
4. **Fix,** once C2 is settled: the form (reader, writer, schema, gate, solver rows, an oracle case against FreeCAD's Lock), then the button.
5. **Later, if asked:** tool first (C7), Collinear and Concentric, Fix of curves, a midpoint of an arc.

## 9. As decided

Fable's decisions on C1 to C9, for the maintainer, 2026-11-14, taken on this note at Silico `main` (`9b756de9`); the maintainer may veto. M5.0's open item 34 states them; this section keeps them whole, with the refusal sentences, the tie rules, and the tests the code must pass, worked by hand.

*Amended 2026-11-14 (Fable, on the port review of #478; the maintainer may veto):* C3's places count every hold (an `on`, a `tangent_on`, a `midpoint`); C5's remove is sent alone where the relation is already there; C6's list gains `tangent` and `tangent_at`.

### The design in one paragraph

One headless function in the core, `sketch_tools::relate(read, relation, picks, ids)`, turns a selection in pick order into one edit, for all ten buttons. It checks only what it needs to choose a form and its roles. Everything else that's wrong with a selection is the gate's to refuse, in the gate's words, when the command runs. The edit is a `sketch_add`, except in two places where the document's structure already is the relation: points made coincident are merged (`sketch_merge`), and a new relation that implies an `on` already there replaces it (a `sketch_add`, then a `sketch_remove`, one undo step). No new form, no new command, and no protected file changes.

Where this departs from the study, it has fewer cases:

| Study | Decided |
|---|---|
| Fix as a new `fix` form (C2 a) | Two dimensions from the origin (C2 b): no schema change |
| Symmetric of points, lines, circles, and arcs, the lines' ends paired by nearness | Two points and a mirror only |
| The mirror chosen by four kind rules, else last picked | The one line or axis picked, else the last point picked |
| Substitution for Tangent and Coincident, the `on` left for Midpoint (C5) | One rule: a relation that implies an `on` replaces it. Coincident never removes anything |
| A message per refusal, the gate's rules said ahead of the add | One sentence per tool for a selection it can't read; the gate says the rest |
| Three-pick Tangent for an end of both or an end held on | A picked point only says which of several places |
| Two circles about one centre refused by the tool | Added; the solve says so (SD7) |
| "The origin is always `b`" | Written as picked |

### C1. Coincident of two points

**Decided:** `sketch_merge` of the picked points, in pick order, so the first picked survives. Its refusal of a worse solve stands beside SD7.

**Why.** Silico has no coincidence between points: a shared point is the coincidence (D2), so the tool has nothing else to write. `sketch_merge` already exists, with its rules settled. The first picked surviving is `sketch_merge`'s own rule read from the selection, so the tool adds no rule.

The refusal doesn't contradict SD7. The line between the two is already drawn in M5.0 section 7: an add writes what the person wrote, and deleting it takes it back. A merge writes structure and a placement of its own, removes a point, and re-points curves and constraints, so its solve judges it. The tools inherit that line; they don't make a second one. SolidWorks would merge and turn the sketch yellow. That divergence is already M5.0's, and it's loud: the refusal names the constraints at fault.

**The rest of Coincident:**

- one or more points and one or more curves: an `on` for each point and each curve, points in pick order, then curves. The origin counts as a point here, and an axis as a curve;
- one point and the origin: the pair the drawing tools write (#278's A), by the same function, whichever was picked first;
- the origin with two or more points: refused, as a selection Coincident can't read;
- nothing is skipped. A curve's own point in the product is refused by the gate, whole. FreeCAD skips it silently; Silico doesn't.

### C2. Fix

**Decided:** (b). Fix writes, for each picked point, a driving `distance_x` and a driving `distance_y` between `origin` and the point, at the newest read's coordinates rounded to the nanometre. No `fix` form. No protected change is needed, so none is approved.

**Shape, exactly.** For each point, in pick order, the two dimensions Smart Dimension writes for the picks (origin, point), by its own function (`dimension.rs`, the `ordered` closure and `rounded`): `a` is the one to the left (below), so the value is never negative, and equal coordinates give `{a: origin, b: point, value: 0}`. `distance_x` comes first. One writer, so Fix and Smart Dimension can't spell the same dimension two ways.

**Why not (a).**

- A fix has to carry its place. A place is two numbers from the origin. The document already has exactly that, and `fix {point, x, y}` would send the solver the same two rows.
- It would be a second spelling of one thing, carried from then on by the reader, the writer, the schema, the gate, the solver's translation, the definition states, the read, the drawing, every sketch edit, and every agent choosing between them.
- The glyph argument is weaker than the study has it. The prototype's `GLYPH.fix` is an entry in a map. No sketch the prototype draws shows a fixed point.
- (b) can become (a) later by adding a form. (a) can't be taken back once documents hold it.
- I also weighed `fix {point}` with no coordinates, pinning the point where the file has it. It fails silently: merging a free point (first) with a fixed one would re-point the fix to the survivor and pin it at a place the person never fixed. Rejected.

**How it behaves,** all of it existing behaviour of dimensions:

- **In the file and the read:** two dimensions. They draw as dimensions, take the Modify box, and can be made driven. Editing a value moves the pin, which a relation couldn't do.
- **Drags:** a fixed point has no freedom, so a drag leaves it where it is, as any fully dimensioned point.
- **Split, trim, merge, unjoin:** whatever they do with any constraint naming a point. A merge that puts a fixed point somewhere else conflicts, and is refused by the merge's own rule, loudly.
- **Unfix:** delete the dimensions. No Fix/UnFix toggle, which would have to recognise its own pair.
- **Fixed already** falls out of C6: both dimensions are there, at these values, so nothing is added.
- **Refused:** a curve, a constraint, or nothing a point ("Fix takes points"). The origin alone is the gate's refusal (a constraint names a member of its sketch). A coordinate `Length::rounded` can't hold is Smart Dimension's `TooFar`.

**The rounding** is forced, not a heuristic: a `Length` is whole nanometres. The point moves by at most half a nanometre on each axis, as a skipped Modify box already does (SD2).

**What's lost.** SolidWorks shows an anchor and no values, and fixes curves. Here Fix shows two dimensions and takes points only. A circle is fixed by its centre and a radius dimension. This is the one matter of taste in the set; the maintainer may veto C2 for a `fix {point, x, y}` form, added later without changing anything built now.

### C3. Tangent

*Amended 2026-11-14 (the coordinator, in building): a `tangent_on`'s hold is a place too, so a second click on a pair it holds is C6's repeat.*

*Amended 2026-11-14 (in building, on #478's robustness review): a line's side is the centre's distance off the line as Smart Dimension measures a point off its line, refused where it rounds to 0 nm, not the sign of the cross product alone; a line of no length is refused in its own words; and a point picked with the curves that isn't one of their places is refused naming the places.*

**Decided:** the form follows the sketch's structure, by one rule.

The places two curves can be tangent *at* are the ends they share, and the ends of one that a constraint holds on the other. Three forms hold a point on a curve: an `on`, a `tangent_on` (its `at` on its `b`), and a `midpoint` (its `point` on its `line`).

- **None:** an edge `tangent {a, b, side}`.
- **One:** tangent there. A shared end gives `tangent_at {a, b, at, direction}`. An end held on gives `tangent_on {a: the curve that ends there, at, b, direction}`, and replaces each `on` that holds it (C5). Where a `tangent_on` already holds it, that's C6's repeat. Where a `midpoint` holds it, the `tangent_on` is added beside the midpoint, which stays: its point-on-line is then said twice and reads redundant, the plain kind, where an edge `tangent` there would be redundant only at the solution, as the gate refuses it beside an `on`.
- **More than one:** refused, unless a point picked with the curves is one of them, and then it's there.

`a` and `b` are in pick order wherever the structure doesn't fix them.

**What's read from the drawing** (the newest read's coordinates):

- **A line's side:** the sign of `(to − from) × (centre − from)`, positive `left`, negative `right`. An axis runs toward +x or +y. Zero is refused: "the centre of circle ce000002 is on line ne000002: which side it's tangent on is unclear; move one off the other first", as Smart Dimension refuses a point on its line.
- **Two rounds:** `outside` where the centres are at least the larger radius apart, else `inside`. This is FreeCAD's rule.
- **`tangent_on`'s direction:** `same` where `a`, leaving `at`, runs as `b`'s own direction there, the dot product at least 0. This is trim's `drawn` (now in `document/sketch_edit.rs`, beside the leaving-direction helper both use), and the tool calls that function, not a copy.
- **`tangent_at`'s direction:** `same` where `a` arriving at `at` and `b` leaving it have a dot product of at least 0. It uses the same leaving-direction helper.

**Refused by the tool:** a selection with no circle or arc among its curves, in the gate's own words, "two lines can't be tangent". The check is needed because two lines sharing an end would otherwise get a `tangent_at`, which the gate takes (split writes one). That is Collinear, not Tangent.

**Not refused by the tool:** two rounds about one centre point. The tool writes `inside`, and the solve says what it finds (SD7). A guard for it would be a special case with no rule behind it.

**Why.** The gate already forces the form by structure, so the tool's only choices are the ones the forms store. Each is a sign read from the drawing, with the tie rules trim already set. Counting the places makes the D shape, the split circle, and two curves each ending on the other one case.

### C4. Symmetric

**Decided:** two points and a mirror, exactly three picks, writing one `symmetric {a, b, about}`.

- **The mirror** is the one line or axis picked. With three points (the origin counts as one), it's the last picked.
- **`a` and `b`** are the other two, in pick order.
- **Anything else** is refused: "Symmetric takes two points and the line or point they mirror about, the point picked last".
- A circle or an arc is mirrored by picking its centre. A line is mirrored by its ends, a pair at a time.

**Why.**

- The form relates points, so the tool takes points.
- SolidWorks' help doesn't say what its Symmetric does with lines' ends or arcs' radii. An unknown behaviour isn't guessed.
- The study's line pairing chose between two pairings by summed distance, with a tie rule. That is a heuristic where no exact rule exists, in code that's two clicks' saving.
- The mirror rule has no case to chase: a line among points can only be the mirror, and among three points only pick order can say. Fusion and FreeCAD both put the mirror last.
- Lines and rounds can be added later without changing anything written now.

### C5. A relation that meets an `on`

**Decided:** one rule. A relation the tool writes that implies an `on` already in the sketch replaces it: a `sketch_add` of the relation, then a `sketch_remove` of the `on`, one undo step.

- It applies to exactly two forms, the two that hold a point on a curve themselves: `tangent_on` (its `at` on its `b`) and `midpoint` (its `point` on its `line`).
- **The order is add, then remove.** If the add is refused, nothing else is sent, so there's no remove to take back. A removal of a constraint by ID isn't refused by the gate.
- **Already there.** Where the relation is in the sketch already (C6) and an `on` it implies is too (the pair interrupted, or both written by an agent), the remove is sent alone, and the toast names what was removed. The edit is whatever separates the sketch from the relation there and its implied `on`s gone.
- **The step** is the tab's existing undo group (`tab.rs`, `Group`, a dialog's OK'd edits). No contract changes, and an agent makes the same two calls.
- **Coincident never substitutes.** An `on` where an edge `tangent` already makes the curves tangent is refused by the gate, in its words. The person deletes the tangent and uses Tangent with the `on` in place.

**Why.**

- Left in, the `on` is redundant, and which of the two PlaneGCS flags is its own choice. It could flag the tangent, and "delete the redundant one" would then delete the wrong one.
- `silico check` would warn `constraints_redundant` on a sketch the app itself made.
- Drawing a line that ends on a circle and then making it tangent is the common path, and FreeCAD substitutes for this reason (`3a656d8597`).
- `sketch_merge` already removes an `on` it makes implied (open item 13b).
- The study left Midpoint's `on` in and substituted Tangent's. One rule is simpler than two.

**The cost.** SolidWorks keeps the coincident relation beside a midpoint. Here, deleting the midpoint later leaves the point free, not on the line. Undo restores both.

### C6. A relation already there

**Decided:** (b), by exact equality. A constraint the click would write is left out when the sketch holds one equal to it in everything but its ID. `a` and `b` may be either way round in the forms where they're interchangeable: `horizontal_points`, `vertical_points`, `parallel`, `perpendicular`, `equal`, `symmetric`, `tangent` (the `side` is the line's, or symmetric in two rounds), and `tangent_at` (a smooth path is smooth both ways).

- All left out: nothing is sent, and the toast says so, for example "line ne000002 is already vertical: nothing was added".
- Some left out: the rest are added, and the toast says how many were already there.

**Why.** A literal duplicate is never meant, and the selection staying after an add makes a second click easy. Equality is exact, so there's no judgement in it. Because it compares every field, the cases fall out:

- a tangent on the other side isn't a repeat, and is added to conflict;
- a driven dimension isn't a repeat of the driving one Fix writes;
- a point fixed already is one.

A relation that's implied but not identical is added and reads `redundant`, as SolidWorks' FAQ shows it.

This is the tool's rule only. `sketch_add` still adds whatever it's sent (SD7).

### C7. Selection first, or tool first

**Decided:** (a), selection first only. With nothing selected, the button toasts the prototype's words.

**Why.** It's what the approved prototype draws and what SolidWorks' context toolbar does. An armed tool would add a picks accumulator, an armed state, a cursor glyph, and Esc, and would change the approved prototype. `relate` takes picks in order, so tool first can be added later with no change to it.

### C8. Several members

**Decided:** (a), a star. Each member after the first is paired with the first picked, which is `a`.

**Why.** A chain loses the relation between its halves when any middle member is deleted. A star loses it only when the first is. It also matches "the first is the base" in Fusion's help.

- **Not promised:** which member moves. `sketch_add` writes no positions, so the solve decides.
- **No special case for the origin:** `a` is the first picked, whatever it is. C6's either-order equality already treats the two spellings as one.

### C9. Sketch mode's selection

**Decided:** as recommended. The selection keeps pick order and admits the origin and the axes. This amends M5.7's selection in sketch mode.

- An ordered set in place of the `BTreeSet`. Open item 24's rules are unchanged: a plain click replaces, a Ctrl-click toggles (a member toggled back in joins at the end), and empty space or Esc clears.
- Members that join together, as a box's would, join in ID order.
- The origin and the axes are hit as `View::pick` ranks them: a point or the origin, whichever is nearer, then the nearest curve, an axis anywhere along it. The sketch's own member wins a tie.
- Delete ignores the origin and the axes.

**Why.** C1's survivor, C4's mirror, and C8's first each need pick order, and pick order is the person's own input, so it's deterministic. Symmetric about an axis and Coincident with the origin need the two to be selectable, and the forms and the gate already take them.

**Residual:** a press on an axis selects it, so a box can't start there. That's a visible annoyance, not a wrong result.

### The interaction, as decided

- **One click, one undo step,** named by the relation ("Horizontal").
- **The selection stays** after the edit, as SolidWorks' does. Members the edit removed leave it when the read comes (`SketchMode::took`).
- **A refusal** adds nothing, shows in the toast, and leaves the selection as it was.
- **The buttons stay enabled** in sketch mode, as the prototype draws them.
- **What the tool itself refuses,** one sentence per tool, with the selection as the error's elements:

| Tool | Sentence |
|---|---|
| Any, nothing selected | "Select the curves or points first; the relation applies to them." |
| Horizontal, Vertical | "Horizontal takes lines, or two or more points" |
| Coincident | "Coincident takes points and the curves they lie on, or two or more points to make one" |
| Tangent | "Tangent takes two curves, at least one a circle or an arc"; and the two in C3 |
| Equal | "Equal takes two or more lines, or two or more circles and arcs" |
| Parallel | "Parallel takes two or more lines" |
| Perpendicular | "Perpendicular takes two lines" |
| Midpoint | "Midpoint takes a point and a line" |
| Symmetric | "Symmetric takes two points and the line or point they mirror about, the point picked last" |
| Fix | "Fix takes points" |

  Lines and points together under Horizontal or Vertical are refused by that sentence. FreeCAD ignores the points; Silico does what was asked, or nothing.
- **Everything else is the gate's,** shown as any refused command's error: a curve's own point, only the origin and axes, a line and a circle made equal, a mirror among its own points. The tool doesn't say the gate's rules ahead of it.

### Tests that must exist

Headless tests of `relate`, beside Smart Dimension's. Each gives a sketch, picks in order, and a source of IDs, and puts the returned edit through the gate (`sketch_edit::add`, `merge`, `remove`). Expected values are worked here by hand. New IDs are `kt000101` and on.

**The sketch** is the study's §7 sketch, `s0000005`, with these added:

- points pe000010 (70, 0), pe000011 (70, 20), pe000012 (70, 10);
- line ne000007, pe000010 → pe000011;
- arc ae000002 about pe000012, from pe000010 to pe000011, counter-clockwise (−90° to 90°), radius 10. With ne000007 it closes a D, sharing both ends;
- points pe000013 (20, 15) and pe000014 (0, 15); line ne000008, pe000014 → pe000013;
- constraint kt000002: `on {point: pe000013, curve: ce000001}` (ce000001 is about (20, 10), radius 5, so (20, 15) is on it);
- constraint kt000003: `on {point: pe000008, curve: ne000001}`.

**Kept from the study's §7 as written:** 1 (Horizontal, and its mixed refusal), 2 (Vertical of points; a circle refused by Vertical's sentence), 3's first three (an `on`, the merge, the origin pair), 4's first two and its two-lines refusal, 5's star, 6, 7, 8's first, 9's first, and 11.

**Changed or added:**

1. **Coincident's own point,** [pe000003, ne000002]: `relate` returns the `on`, and the gate refuses it. The test asserts the gate's error, not a message of the tool's.
2. **Coincident, the origin either way,** [origin, pe000007] gives the same pair as [pe000007, origin].
3. **Coincident, the origin and two points,** [pe000007, pe000008, origin]: refused by Coincident's sentence.
4. **Equal of a line and a circle,** [ne000001, ce000001]: `relate` returns the `equal`, and the gate refuses it, in its words.
5. **Tangent, several places,** [ne000007, ae000002]: refused, naming pe000010 and pe000011.
6. **Tangent, the place picked,** [ne000007, ae000002, pe000011]. The line arrives along (0, 20). The arc's own direction at (70, 20) is (−(20 − 10), 70 − 70) = (−10, 0); pe000011 is its end, so leaving it is (10, 0). The dot product is 0, and the tie goes to `same`: `tangent_at {a: ne000007, b: ae000002, at: pe000011, direction: same}`.
7. **Tangent at an end held on,** [ne000008, ce000001]. One place, pe000013, ne000008's `to`. Leaving it along the line is (0 − 20, 15 − 15) = (−20, 0). The circle's own direction at (20, 15) is (−(15 − 10), 20 − 20) = (−5, 0). The dot product is 100, so `same`. The edit is an add of `tangent_on {a: ne000008, at: pe000013, b: ce000001, direction: same}`, then a remove of kt000002. Put through the gate in that order, both are taken. The same picks in the other order give the same edit, since the structure fixes `a`.
8. **Tangent, a centre on the line.** A circle whose centre is (40, 11), on ne000002: refused, the cross product being 0 × 10 − 20 × 0 = 0.
9. **Tangent of two rounds,** [ce000001, ce000002]. The centres are 40 apart and the larger radius is 5: `outside`. A second pair, radii 10 and 4 with centres 7 apart: `inside`.
10. **Midpoint over an `on`,** [pe000008, ne000001]: an add of `midpoint {point: pe000008, line: ne000001}`, then a remove of kt000003.
11. **Symmetric, three points,** [pe000007, pe000008, pe000005]: `symmetric {a: pe000007, b: pe000008, about: pe000005}`. [pe000005, pe000007, pe000008] gives `about: pe000008`.
12. **Symmetric, the line picked first,** [ne000005, pe000007, pe000008]: the same constraint as the study's 9, `about: ne000005`.
13. **Symmetric of lines,** [ne000001, ne000006, ne000005]: refused by Symmetric's sentence.
14. **Fix,** [pe000007] at (20, 30): `distance_x {a: origin, b: pe000007, value: 20, driving: true}` and `distance_y {a: origin, b: pe000007, value: 30, driving: true}`.
15. **Fix left of the origin,** [pe000008] at (−10, 30): `distance_x {a: pe000008, b: origin, value: 10, driving: true}` and `distance_y {a: origin, b: pe000008, value: 30, driving: true}`.
16. **Fix at the origin's place,** [pe000001] at (0, 0): both `{a: origin, b: pe000001, value: 0}`.
17. **Fix rounds,** a point read at (20.00000049, 30): `value: 20`. At (20.0000006, 30): `value: 20.000001`.
18. **Fix twice:** the result of 14 applied, then Fix on pe000007 again: nothing added. With the `distance_x` made driven first: a driving `distance_x` is added, and only that.
19. **Fix of a line,** [ne000001]: refused, "Fix takes points".
20. **A repeat either way round:** `parallel {a: ne000003, b: ne000001}` in the sketch, then Parallel on [ne000001, ne000003]: nothing added.
21. **A partial repeat:** `horizontal {line: ne000001}` in the sketch, then Horizontal on [ne000001, ne000003]: only ne000003's is added, and the message counts one left out.
22. **Not a repeat:** `tangent {a: ne000002, b: ce000002, side: left}` in the sketch, then Tangent on the same two: the `right` one is added.
23. **A constraint selected** with any tool: refused by the tool's sentence.

**A property test:** for every tool and every selection of up to three members of the sketch, `relate` returns an edit or a refusal and never panics, and an edit the gate takes names only the picked members.

**One real solve** (the sketch worker): a line ended on a circle by an `on`, then Tangent. After the two commands the sketch solves with no constraint flagged redundant. The same add without the remove flags one. This is the reason for C5, so it has to be able to fail.

**App tests** (`egui_kittest`): the study's four, and:

- Ctrl-click three lines in an order that isn't ID order, then Equal: each `equal`'s `a` is the first clicked;
- a click at the origin selects it, and with a point selected too, Coincident sends the pair;
- Tangent over an `on`: two commands sent, and one Undo takes back both;
- a refused click leaves the selection as it was and sends nothing more.

### The plan, amended

1. **Core:** `relate`, all ten tools, with the tests above. `drawn` moves from `trim.rs` to where both call it. No command, contract, or schema change.
2. **App, the selection:** C9.
3. **App, the buttons:** `Does::Constraint(relation)` for all ten; the add-then-remove pair in the tab's undo group; the toast; the step's name. The walkthrough's Equal finding is struck.
4. **Docs:** M5.0's item 34 and section 9's bullets, solidworks.md's *Relations added by hand*, and this note into docs/study/ (#477).

The study's separate Fix step, with its form, schema, and oracle case, is gone.

### Residual risk

- **Fix looks like dimensions, not an anchor.** Visible, and reversible by adding a form.
- **The add-then-remove pair isn't atomic.** If the app dies between the two, the sketch keeps both, and the `on` reads redundant. That's loud (`constraints_redundant`) and harmless, and the same click again removes the `on`.
- **I read the gate as taking a `tangent_on` beside an `on` of the same point and curve** (its tangent rule pairs an edge `tangent` with either). I didn't run it; test 7 proves it. If it's refused, the order becomes remove, then add, with an undo to the revision when the add is refused.
- **Which member moves** when a relation is added is the solver's choice, and isn't promised.
- **A pin on an unsolved sketch** is taken at the coordinates the read shows. The same is true of Smart Dimension.
- **A `tangent_on` beside a `midpoint`** of the same point and line reads redundant. Loud and harmless, and the same whichever was added first. The gate's tangent rule doesn't yet count a `midpoint` hold as it counts an `on`'s; it should, by the same list, in a PR of its own.

## Method

- **SolidWorks.** Each page was fetched from help.solidworks.com with `curl`, using a browser's user agent, since the site refuses `curl`'s own. Its text was read from the page's Next.js data (`__NEXT_DATA__`), stripped of HTML by a short Python script outside the repository. Each page was read at `2027` and `2026` and compared ignoring white space, the year aside, and every one cited matched.
  - Some guessed URLs returned the site's placeholder text (Latin filler) and were dropped. No page in the help is about Fix, Merge, or the sketch context toolbar's contents alone.
  - The help's pictures weren't read.
- **Fusion.** Fusion's help pages are static HTML and were read with `curl`. Autodesk's blog and the two forum threads refuse `curl` (403) and were read in a browser.
- **Unofficial pages.**
  - The Javelin and GoEngineer guides were read with `curl`, as was Product Design Online's course.
  - A Japanese course page that search results quoted on Fusion's repeat-until-Esc redirected to its home page, so it isn't cited.
- **FreeCAD.** Read in the maintainer's clone at `876271d660` (main). The cited behaviours were found again in `CommandConstraints.cpp` at the `1.1.3` tag, fetched from `raw.githubusercontent.com`:
  - Equal's and Parallel's chains, Lock's `DistanceX`, Horizontal's double-constraint refusal, the tangent substitution, the cleared selection, and 1.1.3's three Symmetric selections.
  - The two-element "element and a symmetry line" form is main's, not 1.1.3's.
  - Nothing in the clone was changed.
- **Silico.** Read at `main` (`9b756de9`). Nothing was built or run.

[Add Relations PM]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_SKETCH_DEFAULT.htm
[Geometric Relations]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Geometric_Relations.htm
[Line Properties PM]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_SKETCH_LINE.htm
[Point PM]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_SKETCH_POINT.htm
[Sketch Relations Overview]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Relations_Overview.htm
[Context Toolbars]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Context_Toolbars.htm
[Context Toolbar dimensions]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Control_Dimension_Context_Toolbar.htm
[Horizontal and Vertical]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_horizontal_vertical_sketch_relations.htm
[Description of Sketch Relations]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Description_of_Sketch_Relations.htm
[Relation Groups]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_sketch_relation_group.htm
[FAQ Sketch Relations]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_FAQ_Sketch_Relations.htm
[Item Conflicts]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDE_SKETCH_OVERDEFINED.htm
[SketchXpert PM]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_SKETCH_XPERT.htm
[Javelin 2026]: https://www.javelin-tech.com/blog/2026/08/complete-guide-to-solidworks-sketch-relations/
[GoEngineer 2025]: https://www.goengineer.com/blog/solidworks-sketch-relations-guide
[Fusion constraints]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-CONSTRAINTS.htm
[Fusion horizontal/vertical]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-CONSTRAIN-HORIZONTAL-VERTICAL.htm
[Fusion coincident]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-CONSTRAIN-COINCIDENT.htm
[Fusion tangent]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-CONSTRAIN-TANGENT.htm
[Fusion equal]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-CONSTRAIN-EQUAL.htm
[Fusion parallel]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-CONSTRAIN-PARALLEL.htm
[Fusion midpoint]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-CONSTRAIN-MIDPOINT.htm
[Fusion symmetry]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-CONSTRAIN-SYMMETRY.htm
[Fusion fix]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-CONSTRAIN-FIX-UNFIX.htm
[Fusion coincident points]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/GUID-685B9D79-E3BC-4EB8-AA4E-096A34E94249.htm
[Fusion blog 2024]: https://www.autodesk.com/products/fusion-360/blog/mastering-sketch-constraints-autodesk-fusion-tutorial/
[Fusion forum, selection]: https://forums.autodesk.com/t5/fusion-design-validate-document/option-to-clear-selection-for-sketch-constraints-and-dimension/m-p/12018820
[Fusion forum, over-constrained]: https://forums.autodesk.com/t5/fusion-design-validate-document/help-quot-sketch-geometry-is-overconstrained-quot-won-t-add-new/td-p/6495302
[PDO day 17]: https://productdesignonline.com/day-17-of-learn-fusion-360-in-30-days-for-complete-beginners-2023-edition-learn-all-12-fusion-360-sketch-constraints/
[cmds]: https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/Commands_UM.htm
