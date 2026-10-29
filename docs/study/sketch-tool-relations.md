# The relations SolidWorks' sketch tools and snaps leave, in Silico's forms

A study note for M5.0's open item 26, "which relations tools and snaps add" (M5.11). The item's recommendation is "the relations SolidWorks' help lists as added while sketching". This note checks it against the help, tool by tool and snap by snap. It then maps each relation to Silico's sketch model and names the gaps. It describes what was found. The decisions stay with M5.0 and the maintainer.

**Sources.**

- The SOLIDWORKS 2027 Design Help, read on 2026-10-29. Every 2027 page listed below was also read in the 2026 help. The text of each is the same in both.
- One page from the 2025 help, *Sketch Relations* in the guide for AutoCAD users ([AutoCAD Sketch Relations]). It isn't in the 2026 or 2027 help (both URLs return no page). It's cited only where it adds a detail the current pages lack.
- Some images on these pages, viewed where a caption alone doesn't say what's shown. The help's animated videos couldn't be fetched (their GIFs return an error page), so they weren't seen.
- docs/ux/solidworks.md (read from the 2027 help on 2026-10-27), docs/design/m3.2-sketch-format.md, and `Form` in crates/silico-core/src/document/sketch.rs, at `main` (`c93a7735`).
- docs/design/m5.0-app.md on branch `m5.0-app-design`: section 9's *Sketching* and section 12's items 26 and 32.
- FreeCAD 1.1.3's sketcher tools, read from the `1.1.3` tag on GitHub (`src/Mod/Sketcher/Gui/`: `DrawSketchHandler.cpp`, `DrawSketchDefaultHandler.h`, `DrawSketchHandlerLineSet.h`, `DrawSketchHandlerRectangle.h`), only where SolidWorks' help is silent.

The help pages read, all under `help.solidworks.com/2027/english/SolidWorks/sldworks/`:

- Relations in general: [Inferencing], [Using Automatic Relations], [Relations/Snaps Options], [Sketch Relations Overview], [Areas that Rely on Sketch Relations], [Sketch Relations Icons], [Description of Sketch Relations], [Automatically Adding Relations], [FAQ Sketch Relations], [Geometric Relations], [Horizontal and Vertical Sketch Relations], [Selecting Midpoints with Dynamic Highlighting], [Continuity Relations].
- Snaps: [Snaps], [Sketch Snaps], [Quick Snaps], [Grid/Snap].
- Tools: [Sketch Modes], [Insert Line PropertyManager], [Sketching Lines], [Line Properties PropertyManager], [Modifying Lines by Dragging], [Autotransitioning], [Rectangles], [Corner Rectangles], [Center Rectangles], [3 Point Corner Rectangles], [3 Point Center Rectangles], [Parallelograms], [Rectangle PropertyManager], [Construction Lines and Centerlines], [Circles], [Sketching Circles], [Perimeter Circles], [Circle PropertyManager], [Arcs], [Centerpoint Arcs], [Tangent Arcs], [3 Point Arcs], [Arc PropertyManager], [Sketching Points], [Point PropertyManager].

## In brief

- **The help gives one general rule, not a list per tool.** "If the pointer displays a relation … and you click to accept the sketch entity while the relation is displayed, the relation is added automatically to the entity" ([Inferencing]). That holds when *Automatic relations* is on. No page says which relations a given tool or snap leaves. The rectangle's page says only that its relations were "inferenced automatically during sketching".
- **What the help does tie to sketching.** The pointer's six relations: horizontal, vertical, coincident, midpoint, perpendicular, tangent ([Using Automatic Relations]). The *Sketch Relations Icons* page marks these *Inferred*: coincident, intersection, midpoint, parallel, perpendicular, vertical (and symmetric, from Mirror). Its examples include a vertical relation between a circle's centre and the origin, a second line perpendicular to the first, two lines parallel, and a point at two lines' intersection. Concentric, coradial, collinear, equal, and fix are marked only *Added*.
- **The pages disagree in places.** The icons page marks horizontal and tangent *Added*, yet the pointer adds both while sketching. Its *Type* column seems to describe each example, not the relation. The *Description of Sketch Relations* table has coradial's text in perpendicular's row. The help's "coincident" is a point on a curve. Two points made one is *Merge Points*, and the help doesn't say which one a click on an endpoint makes.
- **Snaps.** The help says snaps and Quick Snaps "add relations between sketch entities", but not which ones. The icons page and the tool pages tie a relation to these: endpoints (coincident), midpoints (midpoint), intersections (intersection), tangent (tangent), perpendicular, parallel, and horizontal or vertical to points. Nearest, length, and grid only place the pointer. For centre points, quadrants, horizontal or vertical to lines, and the angle snap, the help doesn't say.
- **Inference lines** "display relations" and "guide" the pointer. Only the pointer's relation is said to be added. A 2025 example shows a dotted line marking a vertical relation between two endpoints. Its text says only the pointer's horizontal relation is added.
- **"Automatic Relations" off.** With it on, "the relations are added". Clearing *Enable snapping* clears it too. Ctrl suppresses it while held. The help doesn't say what still happens with it off: whether a click on an endpoint still joins, a chain still joins, or a rectangle still gets its relations. In FreeCAD 1.1.3, with auto-constraints off, nothing is inferred, but a tool's own constraints stay. That covers a polyline's joins and a rectangle's corners and sides.
- **In Silico's forms, most relations map exactly.** A coincidence between ends is a shared point (D2), and a point on a curve is an `on`. Horizontal, vertical, perpendicular, parallel, and midpoint (of a line) have forms of the same name. Points level or one above the other are `horizontal_points` and `vertical_points`. A tangency is `tangent`, `tangent_at`, or `tangent_on`, as the gate requires. Concentric is one shared centre point. Coradial is that plus `equal`. An intersection is two `on`s.
- **What has no form.** A point at the origin (the origin isn't a point curves can share). An arc normal to an arc at a shared end. A midpoint of an arc. A quadrant point (two forms do it). An infinite line.
- **Item 26's text needs four corrections.** Horizontal and vertical come from the pointer, not "within the angle snap". The help gives no tolerance. Parallel and the points' horizontal and vertical are missing. Tangent is the pointer's for any tool, not only the tangent arc's. And FreeCAD 1.1.3's auto-constraints aren't "the same set": they have no perpendicular or parallel.
- **Found in passing, for item 32.** The help's FAQ shows "a redundant perpendicular relation is added. The sketch is still fully defined." Only a redundant dimension makes a sketch over defined. Item 32 reads every agreeing redundancy as `over_defined`, "SolidWorks' meaning". So a tool that infers a redundant relation would turn the sketch red in Silico but not in SolidWorks.

## 1. The general rule

### What the help says

- "As you sketch, the pointer changes shape to show you which relations can be created. When Automatic relations is selected, the relations are added." Its pointers: horizontal, vertical, coincident, midpoint, perpendicular, tangent. "Depending on the sketch entities and the position of your pointer, more than one sketch relation can display simultaneously." ([Using Automatic Relations])
- "If the pointer displays a relation … and you click to accept the sketch entity while the relation is displayed, the relation is added automatically to the entity." ([Inferencing])
- Automatic relations "rely on: Inferencing, Pointer display, Sketch Snaps and Quick Snaps" ([Sketch Relations Overview]).
- Among the "areas that rely on sketch relations": "Inferencing as you sketch to create sketch relations", "Sketch Snaps and Quick Snaps to add relations between sketch entities", and the "Orientation option in the Insert Line PropertyManager to specify the relation of sketched lines" ([Areas that Rely on Sketch Relations]).
- *Automatic relations*: "Creates geometric relations as you add sketch elements" ([Relations/Snaps Options]).
- Each entity's PropertyManager lists "relations inferenced automatically during sketching or created manually" (line, circle, arc, point, and rectangle: [Line Properties PropertyManager], [Circle PropertyManager], [Arc PropertyManager], [Point PropertyManager], [Rectangle PropertyManager]).

The help doesn't say whether "click to accept" includes the release that ends a click-drag. It doesn't give the pointer's tolerances (how near a point, how near horizontal).

### The relations the help marks *Inferred*

[Sketch Relations Icons] gives each relation a *Type*, *Inferred* or *Added*, with an example. For 2D sketches:

| Relation | Type | The page's example |
|---|---|---|
| Coincident | Inferred | "Second circle sketched coincident to the first circle." (The image shows the second circle's centre on the first circle.) |
| Intersection | Inferred or added | "Relation added at the intersection of the point and two lines. If you sketch the lines and points so they intersect, the relation is inferred." |
| Midpoint | Inferred or Added | "Construction line sketched from the midpoint of the diagonal line." |
| Parallel | Inferred | "Two lines sketched with parallel relation." |
| Perpendicular | Inferred | "Second line sketched perpendicular to first line." (The image shows it drawn from the first line's end.) |
| Vertical | Inferred | "Circle sketched with center inferred to sketch origin." (The image shows the centre straight above the origin.) |
| Symmetric | Inferred or Added | "Relations added using the Mirror Entities or Dynamic Mirror tool." |
| Horizontal | Added | "Horizontal relation added to spline handles." |
| Tangent | Added | "Tangent relation added to two splines." |
| Colinear, Concentric, Coradial, Equal, Fix, Merge | Added | (by hand) |

Offset, On Edge, and On Surface are inferred by other tools (Offset, Convert Entities) or in 3D. Along X, Y, and Z are 3D only.

### Where the pages disagree

- **Horizontal and tangent.** The icons page marks both *Added*. But [Using Automatic Relations] lists both among the pointer's relations, and [Inferencing] uses horizontal as its example of one added automatically. [Automatically Adding Relations] draws a perimeter circle "using the tangent icon". The *Type* column seems to describe the example (spline handles, two splines), not the relation.
- **Perpendicular and coradial.** In [Description of Sketch Relations], coradial's row has no result, and perpendicular's reads "The two items are perpendicular to each other.The items share the same center point and radius." The second sentence is coradial's.
- **Coincident.** The help's coincident is "A point and a line, arc, or ellipse. The point lies on the line, arc, or ellipse" ([Description of Sketch Relations]). Two points made one is a separate relation, *Merge Points*, which the icons page marks *Added*. The help doesn't say which relation a click on an existing endpoint makes. [Using Automatic Relations] shows only a coincident pointer.
- **A midpoint first reads as coincident.** "A midpoint highlights and the pointer shows that a coincident relation is possible at its current position." Then "The midpoint has changed color and the pointer shows that it recognizes the midpoint" ([Inferencing]). So near a line the pointer offers coincident (on the line), and at its middle, midpoint.

### Lines and arcs extended

"When you create a relation to a line, the relation is to the infinite line … When you create a relation to an arc segment or elliptical segment, the relation is actually to the full circle or ellipse" ([Description of Sketch Relations]). Silico's `on` is the same: "a line extended, or an arc's whole circle".

### Redundant relations

"A redundant perpendicular relation is added. The sketch is still fully defined." Then "A redundant dimension is added. The sketch is over defined." ([FAQ Sketch Relations]). So in SolidWorks a redundant relation doesn't over-define a sketch. M5.0's item 32 reads an agreeing redundancy as `over_defined`. Tools that infer two relations implying each other would then over-define a Silico sketch where SolidWorks' wouldn't.

## 2. Snaps and inference lines

### What snaps are

- "Each Sketch Snap allows you to automatically snap to selected entities as you sketch. By default, all Sketch Snaps except Grid are enabled." "On sketches with multiple entities, only the current sketch entity displays snaps." ([Sketch Snaps])
- "Quick Snaps are instantaneous, single operation Sketch Snaps." "Selecting a particular Quick Snaps filters out other Sketch Snaps." They work "regardless of whether Enable snapping or the Sketch Snap is selected". In click-drag mode Quick Snaps can't be picked from the toolbar mid-gesture; in click-click mode they can ([Quick Snaps]).
- "Sketch Snaps are global sketch settings that apply to all sketch commands. Quick Snaps are single, independent selections." ([Snaps])

### Each snap

The second column is the [Sketch Snaps] page's wording. The third is the relation the help ties to that snap, anywhere. "Unknown" means no page says.

| Snap | What it does | Relation it leaves | Silico |
|---|---|---|---|
| End points and sketch points | "Snaps to the end of … lines, polygons, rectangles, … points … Snaps to the center of arcs." | Coincident: the pointer's (§1). Whether a coincidence or a merge: unknown. | A shared point (D2) |
| Center points | "Snaps to the center of … circles, arcs, …" | Unknown | A shared centre point, if it takes the centre |
| Midpoints | "Snaps to the midpoints of lines, … arcs, …" | Midpoint, for lines ([Sketch Relations Icons]; [Description of Sketch Relations]: "Two lines or a point and a line"). For an arc: unknown. | `midpoint` (line only). An arc's: no form (§9) |
| Quadrant points | "Snaps to the quadrants of circles, arcs, …" | Unknown | No point on a circle; `on` and `horizontal_points` or `vertical_points` with the centre (§9) |
| Intersections | "Snaps to the intersections of entities that meet or entities that intersect." | Intersection, for two lines and a point ([Sketch Relations Icons]). For other curves: unknown. | Two `on`s |
| Nearest | "Clear Nearest Snap, to enable all snaps … Select Nearest Snap, and snaps are enabled only when the pointer is in the vicinity of the snap point." | None: it changes when other snaps act | — |
| Tangent | "Snaps to tangents on circles, arcs, …" | Tangent ([Automatically Adding Relations], the perimeter circle) | `tangent`, `tangent_at`, or `tangent_on` (§9) |
| Perpendicular | "Snaps a line to another line." | Perpendicular ([Sketch Relations Icons]) | `perpendicular` |
| Parallel | "Creates a parallel entity to lines." | Parallel ([Sketch Relations Icons]) | `parallel` |
| Horizontal/vertical lines | "Snaps a line vertically to an existing horizontal sketch line, and horizontally to an existing vertical sketch line." | Unknown: vertical, horizontal, or perpendicular | `vertical`/`horizontal`, or `perpendicular` |
| Horizontal/vertical to points | "Snaps a line vertically or horizontally to an existing sketch point." | Vertical, in the icons page's circle-centre example. For a line's end: see *Inference lines* below. | `vertical_points`, `horizontal_points` |
| Length | "Snaps lines to the increments that are set by the grid … press and hold Shift while sketching." | None said | — |
| Grid | "Snaps sketch entities to the grid's vertical and horizontal divisions." Off by default. | None said | — |
| Angle | "Snaps to angles." The snap angle counts from the sketch's axes, or from the existing line when you sketch from its end ([Relations/Snaps Options]). | None said, beyond the pointer's horizontal or vertical | — |

*Snap to model geometry* "Allows sketch entities to snap to model geometry such as edges" ([Relations/Snaps Options]). Whether that leaves a relation is unknown. Silico's sketches can't name model edges, so it has no form for one.

### Inference lines

- "Inferencing lines are dotted lines that appear as you sketch, displaying relations between the pointer and existing sketch entities (or model geometry). When your pointer approaches highlighted cues such as midpoints, the inferencing lines guide you relative to existing sketch entities." ([Inferencing])
- The help says only the pointer's relation is added (§1). No page says a dotted line's relation is added on its own.
- The 2025 page describes the same image [Inferencing] shows (a line drawn level with a rectangle's corner): "The inferencing line shows a vertical relation between the endpoints of the two lines. The [icon] in the pointer display indicates that the line being sketched is horizontal. The horizontal relation is added to the entity properties automatically." ([AutoCAD Sketch Relations]). It names only the horizontal relation as added. It doesn't say the vertical one isn't.
- The icons page's vertical example, a circle's centre above the origin, is marked *Inferred* (§1). What the pointer showed there isn't said.
- docs/ux/solidworks.md cites one unofficial source: blue lines guide only, and lines in another colour capture a relation. The help says nothing about the lines' colours.

So the help's rule is the pointer. Whether an inference line to a point leaves a vertical or horizontal relation is unknown. The one example that names a relation between points (the circle's centre) says it does.

## 3. The line

- **Modes.** "If you click the first point and drag, you are in click-drag mode. If you click the first point and release the pointer, you are in click-click mode." ([Sketch Modes])
- **The chain.** "When the Line and Arc tools are in click-click mode, they create a chain of segments as you click." A double-click or *End chain* ends it and keeps the tool. Esc ends it and leaves the tool ([Sketch Modes]). The help doesn't say the chain's segments are joined by a relation. "Chain" is its word. docs/ux/solidworks.md and M5.0 section 9 take it as shared points.
- **Ending on an endpoint.** "When you sketch entities in click-click mode and end a line or arc on the endpoint of an existing sketch entity, the tool stays active but does not show a preview from the last location." ([Sketch Modes]). It says nothing about a relation there.
- **Each end.** The help gives nothing particular to a line's ends. The general rule applies: at the click that accepts it, the pointer's relations are added (§1).
- **Orientation.** *Horizontal* and *Vertical* sketch "a horizontal line" or "a vertical line" ([Insert Line PropertyManager]). The orientation option "specif[ies] the relation of sketched lines" ([Areas that Rely on Sketch Relations]). *Angle* is "created relative to the horizontal". With *Add dimensions*, the length (and the angle) are dimensioned ([Sketching Lines]). "If the line has a vertical or horizontal relation, delete the vertical or horizontal relation … before dragging to a new angle." "If you selected Add dimensions under Parameters, you must delete the relation to change the length or the angle of the line." ([Modifying Lines by Dragging])
- **Options.** *Midpoint line* "Creates a line that is symmetrical from the midpoint of the line." What it leaves at the midpoint is unknown. *Infinite length* "Creates a line of infinite length that you can later trim." ([Insert Line PropertyManager])
- **Autotransition.** From a line's end, "Move the pointer back to the endpoint, then away again. The preview shows a tangent arc." A toggles line and arc ([Autotransitioning]). It isn't said whether the arc gets a tangent relation. The tangent arc tool's own page doesn't say either (§6).

## 4. Rectangles

- **Corner rectangle.** "Click to place the first corner of the rectangle and drag to create the shape. To create a square, press Shift before you release." ([Corner Rectangles])
- **Center rectangle.** "Click to define the center. Drag to sketch the rectangle with centerlines." ([Center Rectangles]). The PropertyManager: "Sketches a rectangle that includes a center point." ([Rectangle PropertyManager])
- **3 point corner rectangle.** Click the first corner; drag, rotate, release for the first edge; then "Click and drag to sketch the other three edges." ([3 Point Corner Rectangles])
- **3 point center rectangle.** Click the centre; "Drag and rotate to set one half the length of the centerline"; then the other lines ([3 Point Center Rectangles]).
- **Parallelogram.** First corner, first edge, then "Rotate, and drag to set the angle and length of the other three edges." Shift makes "a parallelogram with equal sides." ([Parallelograms])
- **Construction lines.** *From Corners* "Adds centerlines from corner to corner". *From Midpoints* "Adds centerlines from midpoint of line segments." ([Rectangle PropertyManager]; [Construction Lines and Centerlines])
- **Relations.** The PropertyManager lists "relations inferenced automatically during sketching". No page names them, for any type. The images on these pages show no relation icons. Whether Shift leaves an equal relation is unknown. docs/ux/solidworks.md cites an unofficial source: a corner rectangle has vertical relations on its sides and horizontal ones on its top and bottom.

## 5. The circle

- **Centre.** "Click to place the center of the circle. Drag and click to set the radius." ([Sketching Circles])
- **Perimeter.** "Click to place the perimeter. Drag left or right to sketch the circle. Right-click to set the circle." ([Perimeter Circles])
- **Relations.** The general rule. The help has two examples. A second circle with its centre on the first gets coincident ([Sketch Relations Icons]). A perimeter circle drawn "using the tangent icon" is tangent to the first circle ([Automatically Adding Relations]). Concentric circles are an example of a relation added by hand ([Manually Adding Relations](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Manually_Adding_Relations.htm)). What a centre clicked on an existing centre leaves (coincident, merge, or concentric) is unknown.

## 6. The arcs

- **Centrepoint arc.** "Click to place the center of the arc. Release and drag to set the radius and the angle. Click to place a start point. Release, drag, and click to set an end point." ([Centerpoint Arcs])
- **Tangent arc.** "Click on the end point of a line, arc, ellipse, or spline. Drag the arc to create a shape and release." "SOLIDWORKS Design infers from the pointer motion whether you want a tangent or normal arc. There are four intent zones, with eight possible results … You can switch between tangent and normal arcs by returning to the end point and moving away in a new direction." ([Tangent Arcs]). The page doesn't name the relations left. Tangent is among the pointer's relations (§1). For a normal arc, the help names no relation. Its perpendicular takes "Two lines" only ([Description of Sketch Relations]).
- **3 point arc.** "Click to set a start point. Drag the pointer, then click to set an end point. Drag to set the radius. Click to set the arc." ([3 Point Arcs])
- **Continuity.** When an arc or a line ends on another entity's end, a pop-up toolbar offers tangency "as soon as you finish sketching". The help's example is a spline ending on a 3 point arc ([Continuity Relations]). That relation is offered, not inferred.

## 7. The point

"Click in the graphics area to place the point. The Point tool remains active so you can continue to insert points." ([Sketching Points]). Its relations follow the general rule. The intersection example, "If you sketch the lines and points so they intersect, the relation is inferred" ([Sketch Relations Icons]), is the only one specific to points. What a point clicked on an existing point does is unknown.

## 8. With "Automatic Relations" off

### What the help says

- "You can choose whether geometric relations are automatically created as you create sketch entities." "When Automatic relations is selected, the relations are added." "To temporarily suppress automatic relations, hold down Ctrl while sketching." ([Using Automatic Relations])
- "Relations are not displayed if Enable snapping is cleared. When you clear Enable snapping, it also clears Automatic Relations." ([Inferencing])
- "How do I avoid getting relations that I don't want? Turn off automatic relations." ([FAQ Sketch Relations])
- Quick Snaps work with *Enable snapping* cleared ([Quick Snaps]). Clearing *Enable snapping* clears *Automatic relations* too. So whether a Quick Snap then leaves a relation is unknown.

### What the help doesn't say

- whether a click on an existing endpoint still joins the two (a coincidence or a merge), or only places the new end there;
- whether a click-click chain's segments are still joined;
- whether a rectangle still gets its relations, or a tangent arc its tangency;
- whether a line drawn with the *Horizontal* orientation still gets its relation.

### FreeCAD 1.1.3, for comparison

- With auto-constraints off, `seekAutoConstraint` returns nothing and `createAutoConstraints` returns at once (`DrawSketchHandler.cpp`, "If Autoconstraints property is not set quit"). So a click on an existing endpoint leaves the new end unjoined, at the same place.
- A tool's own constraints don't go through that path, so they stay:
  - the polyline's join at each segment is a `Coincident`, or a `Tangent` or `Perpendicular` for its arc transitions (`DrawSketchHandlerLineSet.h`);
  - the rectangle's four corner coincidences and its alignment constraints (`DrawSketchHandlerRectangle.h`, `finishRectangleCreation`).

## 9. Each relation in Silico's forms

Silico's sketch model: points listed by ID, curves naming their points (a line's `from` and `to`, a circle's `center`, an arc's `center`, `start`, `end`), and constraints of one form each (`Form` in sketch.rs; M3.2's D4). The fixed references `origin`, `x_axis`, and `y_axis` may be named by constraints, not by curves.

| SolidWorks' relation, while sketching | Silico | Exact? |
|---|---|---|
| Coincident, an end on an existing point | The new curve names that point (D2). No constraint. | Yes |
| Merge Points | The same: one point (`sketch_merge` after the fact) | Yes |
| Coincident, a point on a line, arc, circle, or axis | `on` (`point`, `curve`; the curve may be `x_axis` or `y_axis`) | Yes: both extend lines and arcs |
| Coincident, a point on the origin | **No form.** Curves can't name `origin`, and `on` takes a curve. Closest: `horizontal_points` and `vertical_points`, each with `origin` (two relations, no value). Or `distance_x` and `distance_y` from `origin`, value 0 (two dimensions, as M3.2's example places a corner). | No single form |
| Horizontal or vertical, a line | `horizontal`, `vertical` (`line`) | Yes |
| Horizontal or vertical, two points | `horizontal_points`, `vertical_points` (`a`, `b`; either may be `origin`) | Yes |
| Midpoint, a point and a line | `midpoint` (`point`, `line`) | Yes |
| Midpoint, a point and an arc | **No form.** `midpoint` takes a line. Closest: `on` the arc, with no hold at its middle. | No |
| Perpendicular, two lines | `perpendicular` (lines or axes) | Yes |
| Parallel, two lines | `parallel` (lines or axes) | Yes |
| Tangent, a line and a circle or arc, along their edges | `tangent` (`side`: the line's left or right) | Yes |
| Tangent, two circles or arcs, along their edges | `tangent` (`side`: `outside` or `inside`) | Yes |
| Tangent, at an end two curves share (a tangent arc, an autotransition) | `tangent_at` (`direction`: `same` for a smooth join) | Yes. The gate requires it over `tangent` here. |
| Tangent, where one curve's end lies on the other | `tangent_on` (`at`, `direction`) | Yes. The gate requires it over `on` with `tangent`. Not for two lines. |
| A normal arc, at a line's end | **No relation form.** The arc's centre `on` the line holds the same geometry: the radius at that end then runs along the line. | Geometry yes, relation no |
| A normal arc, at an arc's end | **No form.** `perpendicular` takes lines only. (FreeCAD's polyline makes an endpoint-to-endpoint `Perpendicular`.) | No |
| Intersection, a point and two lines | Two `on`s | Yes |
| Concentric | One shared centre point: both curves name the same `center` | Yes |
| Coradial | One shared centre and `equal` | Yes |
| Collinear | Two `on`s: both ends of one line on the other | Yes |
| Equal | `equal` | Yes |
| Symmetric | `symmetric` (two points about a line or a point) | For points. Lines and arcs need one per pair of points. |
| Fix | **No form.** `distance_x` and `distance_y` from `origin` (D4) | No |
| A quadrant point | **No form.** `on` the circle, and `horizontal_points` or `vertical_points` with its centre | Yes, as two |
| An angle (Line orientation *Angle*, with *Add dimensions*) | `angle` (`a`: `x_axis`, `b`: the line, `driving`: true), and `length` | Yes |
| An infinite line | **No form.** Silico's lines have two points. | No |

Each "no form" is an addition to the protected schema (`tests/contract/`), so it needs the maintainer.

## 10. FreeCAD 1.1.3, where SolidWorks is silent

From the `1.1.3` tag:

- **What it infers** (`seekAutoConstraint`):
  - a point under the pointer gives `Coincident`;
  - a curve under the pointer gives `PointOnObject`;
  - within 5% of a line's length from its middle, `Symmetric` of its ends about the point (Silico's `midpoint`);
  - a direction within 2° of horizontal or vertical gives `Horizontal` or `Vertical`;
  - a curve the new one leaves along gives `Tangent`.
  
  On creation, a coincidence plus a tangency becomes an endpoint-to-endpoint `Tangent`, and an on-object plus a tangency an endpoint-to-edge one (`createAutoConstraints`). These are Silico's `tangent_at` and `tangent_on`.
- **What it doesn't infer: perpendicular and parallel.** Those auto-constraints came to FreeCAD's main with the polyline rework (`d77c9647c3`, 2026-05-18). They aren't in 1.1.3.
- **The rectangle** (`finishRectangleCreation`, `addAlignmentConstraints`):
  - four corner coincidences;
  - for a corner or centre rectangle, or any at angle 0, two `Horizontal` and two `Vertical`;
  - for a 3-point one at an angle, two `Parallel`, and `Perpendicular` only if its corner is square;
  - a centred one adds a construction point at the centre with `Symmetric` of two opposite corners about it (`finishCenteredRectangleCreation`).
- **Redundancy.** With *Avoid redundant* on, the polyline drops a horizontal or vertical auto-constraint the sketch already implies (`removeRedundantHorizontalVertical`). Its default wasn't read.

## For M5.0 and the maintainer

### Item 26's text, checked against the help

- "horizontal and vertical when a line is within the angle snap of an axis": **not the help's wording.** They're added when the pointer shows them (§1). The angle snap is a separate snap that the help ties to no relation. The tolerance isn't published; FreeCAD 1.1.3's is 2°.
- "coincident when a click lands on a point (a shared point)": **supported**, with one caveat. The help doesn't say whether that's a coincidence or a merge. In Silico both are a shared point.
- "`on` when it lands on a curve": **supported** ([Inferencing]; the icons page's coincident).
- "midpoint at a midpoint": **supported for lines.** For an arc's midpoint, the relation is unknown and Silico has no form.
- "tangent for a tangent arc": **partly.** The tangent arc's page names no relation. Tangent is the pointer's relation for every tool ([Using Automatic Relations]; the perimeter circle example).
- "perpendicular where the pointer shows it": **supported** (icons page).
- **Missing:** parallel (icons page, *Inferred*); vertical and horizontal between points (the circle-centre example); intersection, as two `on`s (icons page); the line's *Horizontal* and *Vertical* orientations.
- "Snaps that only guide add nothing": **consistent with the help for nearest, length, and grid.** The help ties no relation to centre points, quadrants, horizontal or vertical to lines, or angle, and doesn't say they add none.
- "FreeCAD's autoconstraints add the same set where SolidWorks is silent": **not the same set.** FreeCAD 1.1.3 infers no perpendicular or parallel (§10). Where SolidWorks is silent (the rectangle's relations, a chain's joins), FreeCAD 1.1.3 is the fallback that's there.

### Recommended for M5.11, by tool

Every inferred relation below is added only when the pointer shows it at the accepting click, with *Automatic relations* on and Ctrl not held (§1).

1. **Line, both modes.** At each end:
   - an existing point: that point (shared);
   - a curve or an axis: a new point and `on`;
   - a line's middle (the pointer shows midpoint): a new point and `midpoint`;
   - the origin: decision A.
   
   Along the line:
   - `horizontal` or `vertical` when the pointer shows it;
   - `perpendicular` or `parallel` to the line the pointer names.
   
   In a chain, each segment shares its start with the last one's end, as section 9 of M5.0 has it.
2. **Line options.** *Horizontal* and *Vertical* orientations add `horizontal` or `vertical` (the help: "specify the relation"). *Angle* with *Add dimensions* adds `angle` from `x_axis` and `length`. *Midpoint line* and *Infinite length* stay out of M5.11: what the first leaves is unknown, and the second has no form.
3. **Autotransition and the tangent arc.** The arc starts at the end it leaves (shared). A tangent arc adds `tangent_at`, `direction` `same`. A normal arc is decision B. Its other end is placed as a line's end.
4. **Corner rectangle.** Four lines and four shared corners, two `horizontal`, two `vertical`. That's FreeCAD 1.1.3 and #196's key test. The help doesn't name them. The first and the opposite corner take what a line's end takes. Shift (a square) adds nothing until decided: the help doesn't say whether it leaves `equal`.
5. **Other rectangles,** if M5.11 builds them. FreeCAD 1.1.3's constraints, in Silico's forms:
   - 3 point corner: two `parallel` and one `perpendicular`;
   - parallelogram: two `parallel`;
   - centre: decision C;
   - 3 point centre: two `parallel`, one `perpendicular`, and the centre as decision C.
6. **Circle.** The centre takes what a line's end takes. A centre on an existing centre is a shared point, which is concentric. The radius click:
   - on a point: `on` (the point, the circle);
   - with the pointer showing tangent: `tangent`, its `side` from what was drawn.
   
   A perimeter circle's clicks are radius clicks.
7. **Arcs.** A centrepoint or 3 point arc's centre, start, and end take what a line's end takes. A 3 point arc's last click with the pointer showing tangent: `tangent`, or `tangent_at` where it shares an end. The gate decides which.
8. **Point.** On a curve: `on`. At a line's middle: `midpoint`. At two lines' crossing: two `on`s. On an existing point: decision D.
9. **Snaps.**
   - **Add their relation:** endpoints (shared), midpoints of lines (`midpoint`), intersections (two `on`s), tangent, perpendicular, parallel.
   - **Guide only:** nearest, length, grid, angle.
   - **Decide:** horizontal or vertical to points (decision E), and centre points, quadrants, and horizontal or vertical to lines. For the last three, recommended: centre points give a shared centre (a click on the point); quadrants and horizontal or vertical to lines guide only, since the help names no relation for them.
10. **Inference lines** guide only; the pointer's relations are added (decision E).
11. **Automatic Relations off, and Ctrl:** decision F.

### Decisions

- **A. A point at the origin.** (a) Two relations, `horizontal_points` and `vertical_points` with `origin`: no new form, no value. (b) A new form, a point at the origin, matching SolidWorks' one coincident. That changes the protected schema. (c) Two `distance_x`/`distance_y` of 0: dimensions, which SolidWorks wouldn't show. Recommended: (a), unless the maintainer wants one relation per SolidWorks relation.
- **B. A normal arc** (the tangent arc's normal zones). The help names no relation. For a line, `on` of the arc's centre on the line is exact. For an arc, there's no form. Recommended: in M5.11, only tangent arcs, or normal arcs from lines only (`on`); a form for curves meeting at right angles later, if wanted.
- **C. A centre rectangle's centre.** (a) FreeCAD's: a construction point with `symmetric` of two opposite corners about it, no centerlines. (b) SolidWorks' look: two construction diagonals sharing the corners, and the centre `midpoint` of one of them. Not both: with shared corners, a second `midpoint` is implied, so it's a redundancy, which item 32 reads `over_defined`. Recommended: (b), since SolidWorks draws the centerlines; the relations themselves are unknown.
- **D. A point clicked on an existing point.** The help is silent. FreeCAD 1.1.3 makes a second point with a coincidence. Silico has no coincidence between points. Recommended: nothing is added, and the tool says so.
- **E. Points level with or above others.** The icons page's one example (a circle's centre above the origin) says a vertical relation is inferred. A 2025 example names only the pointer's relation as added. FreeCAD infers none. Recommended: `horizontal_points` or `vertical_points` only when the pointer shows the relation, never from the dotted line alone.
- **F. Automatic Relations off.** The help says only that relations aren't added. Recommended, as FreeCAD 1.1.3:
  - a tool's own structure stays: a chain's shared points, a rectangle's corners and relations, a tangent arc's `tangent_at`, a line orientation's relation;
  - nothing is inferred, including a join to an existing point: the new end is a new point at the snapped place.
  
  The cost: two ends at one place that aren't one point, which M3.5 refuses in a profile ("share the point"). The alternative: a click on an existing point always shares it, since D2 makes that geometry, not a relation. Ctrl does the same while held.
- **G. Redundant inferred relations.** SolidWorks keeps a redundant relation and the sketch stays fully defined ([FAQ Sketch Relations]). Item 32 reads it `over_defined`. Recommended: tools don't add a relation the sketch already implies, as FreeCAD's *Avoid redundant* drops horizontal and vertical ones. Separately, item 32's "SolidWorks' meaning" holds for dimensions but, per this FAQ, not for relations. Worth a look before M5.11 colours sketches.

## Method

- Each page was fetched from help.solidworks.com, 2027 and 2026, with `curl`. The pages are built by Next.js, so the text was read from each page's `__NEXT_DATA__` JSON (its `helpText`), stripped of HTML by a short Python script outside the repository. The 2027 and 2026 texts were compared with `diff`, the year aside: every page cited matched.
- Linked pages were followed from each page's related links. The help's table of contents loads only in part, so a page not linked from these could be missed.
- Images were fetched where a caption alone doesn't say what's shown: the icons page's coincident, vertical, midpoint, intersection, parallel, and perpendicular examples; the inferencing image; the rectangles' images; the tangent arc's zones. The animated videos (`Automatic_Relations01.gif` and others) returned an error page at every path tried, so they weren't seen.
- FreeCAD 1.1.3's files were fetched from `raw.githubusercontent.com/FreeCAD/FreeCAD/1.1.3/`. The local clone has no tags. `d77c9647c3` was found in the clone's history with `git log -S`. Nothing in the clone was changed.
- Silico's forms were read from sketch.rs at `main` (`c93a7735`), and the gate's rules from docs/design/m3.2-sketch-format.md.

[Inferencing]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_inferencing.htm
[Using Automatic Relations]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Using_Automatic_Relations.htm
[Relations/Snaps Options]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_relations_snaps_options.htm
[Sketch Relations Overview]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Relations_Overview.htm
[Areas that Rely on Sketch Relations]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_areas_reliant_sketch_relations.htm
[Sketch Relations Icons]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Relations_Icons.htm
[Description of Sketch Relations]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Description_of_Sketch_Relations.htm
[Automatically Adding Relations]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Automatically_Adding_Relations.htm
[FAQ Sketch Relations]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_FAQ_Sketch_Relations.htm
[Geometric Relations]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Geometric_Relations.htm
[Horizontal and Vertical Sketch Relations]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_horizontal_vertical_sketch_relations.htm
[Selecting Midpoints with Dynamic Highlighting]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Selecting_Midpoint_with_Dynamic_highlight.htm
[Continuity Relations]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_add_continuity_relations.htm
[Snaps]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Snaps.htm
[Sketch Snaps]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Snaps.htm
[Quick Snaps]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Quick_Snaps.htm
[Grid/Snap]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_OPTIONS_GRID.htm
[Sketch Modes]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Modes.htm
[Insert Line PropertyManager]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_SKETCH_LINE_CMD.htm
[Sketching Lines]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_Lines.htm
[Line Properties PropertyManager]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_SKETCH_LINE.htm
[Modifying Lines by Dragging]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Modifying_Lines_By_Dragging.htm
[Autotransitioning]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Autotransitioning.htm
[Rectangles]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Rectangles.htm
[Corner Rectangles]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_Corner_Rectangles.htm
[Center Rectangles]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_Center_Rectangles.htm
[3 Point Corner Rectangles]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_3_Point_Corner_Rectangles.htm
[3 Point Center Rectangles]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_3_Point_Center_Rectangles.htm
[Parallelograms]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_Parallelograms.htm
[Rectangle PropertyManager]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_SKETCH_RECTANGLES.htm
[Construction Lines and Centerlines]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_add_construction_lines_and_centerlines.htm
[Circles]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Circles.htm
[Sketching Circles]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_Circles.htm
[Perimeter Circles]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_Perimeter_Circles.htm
[Circle PropertyManager]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_SKETCH_CIRCLE.htm
[Arcs]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Arcs.htm
[Centerpoint Arcs]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_Centerpoint_Arcs.htm
[Tangent Arcs]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_Tangent_Arcs.htm
[3 Point Arcs]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_3_Point_Arcs.htm
[Arc PropertyManager]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_SKETCH_ARC.htm
[Sketching Points]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_Points.htm
[Point PropertyManager]: https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_SKETCH_POINT.htm
[AutoCAD Sketch Relations]: https://help.solidworks.com/2025/English/SolidWorks/acadhelp/c_Sketch_Relations_AcadHelp.htm
