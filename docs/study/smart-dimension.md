# Smart Dimension in SolidWorks' help

A study note for M5.0's open item 28 ("M5.11 reads Smart Dimension from the help before it's built") and for M5.11, sketch mode's dimensions. It records what SolidWorks' Smart Dimension places for each selection: the kind of dimension and how it's picked, the arc conditions, radius and diameter, driven and driving, the Modify box, and dimensions to a centreline. It then maps each result to Silico's constraint forms and says where a form is missing or doesn't fit. It decides nothing. The decisions stay with M5.0 and the maintainer.

**Sources.**

- The SOLIDWORKS 2027 Design Help (help.solidworks.com/2027, English), read on 2026-10-29. Every page cited was also read in the 2026 help, and each says the same in both, word for word. URLs below are the 2027 ones; the 2026 page is the same URL with `2026` in place of `2027`.
- One page outside the help, linked from the help's own *Dimensions between Arcs or Circles*: a SOLIDWORKS Tech Blog post written by a reseller (GSC). It is marked *unofficial* where used.
- Silico's forms: `crates/silico-core/src/document/sketch.rs` (`Form`, `Reference`, `Takes`) and docs/design/m3.2-sketch-format.md (D4), at `m5-sketch-tools-study` (`18622a5d`). PlaneGCS as vendored (`third_party/planegcs`, FreeCAD 1.1.3's).
- M5.0's design on branch `m5.0-app-design` (`docs/design/m5.0-app.md`): section 9, *Sketching*, and section 12, item 28.
- docs/ux/solidworks.md, which this note extends. Its *Not covered* lists dimensioning as not read.

## In brief

- **The kind follows the selection; the pointer picks among kinds for some.** The help: "Dimension types are determined by the sketch entities that you select. For some types of dimensions (point-to-point, angular, circular), the location where you place the dimension also affects the type of dimension that is added." A right-click locks the kind the preview shows.
- **What each selection places** (sections 2 to 5):
  - a line: its length;
  - a circle: its diameter, "displayed as linear or diameter, depending on placement";
  - an arc: its radius; its true length if the two end points are picked too;
  - two parallel lines: the distance between them; two lines otherwise: an angle, which of the angles by where the pointer is (the help's pictures show 45°, 135°, and 315° for one pair);
  - a line, a point, or another circle with a circle or an arc: a distance to the centre, by default, "even when you select the circumference";
  - two circles or arcs: centre to centre by default, edge to edge (Min) with Shift;
  - a point and a line: the perpendicular distance;
  - two points: horizontal, vertical, or aligned, by where the pointer is.
- **Arc conditions.** A distance to a circle or arc is measured at its Center (the default), Min (closest point), or Max (farthest point). The first condition applies to the first arc or circle, the second to the second "when both items are arcs or circles". They're set in the Dimension PropertyManager's Leaders tab, or with Shift while picking.
- **Radius or diameter** is a display choice on the same dimension (Leaders tab: Radius, Diameter, Linear), not a different dimension.
- **A centreline.** A centreline and a line or point give a radius on the near side and a diameter (a "doubled distance") on the far side. The help ties this to revolve sketches.
- **Driven or driving.** No page says in one sentence that a placed dimension is driving. Each page that makes one driven names a condition:
  - the over-defining options;
  - a closed profile's last entity;
  - *Sketch Dimension Driven* while sketching;
  - the right-click *Driven* toggle.
  With *Prompt to set driven state* on, an over-defining dimension opens *Make Dimension Driven?*: make it driven, leave it driving, or Cancel (it isn't placed). The help doesn't give the options' defaults.
- **The Modify box** opens when a dimension is placed if *Input dimension value* is on, or on a double-click. It takes a number, arithmetic ("as a calculator"), units, or, after `=`, an equation or a global variable.
- **The first dimension scales the whole sketch** to keep its proportions ("only applies when you add the very first dimension to a sketch"). This is behavior, not display, and Silico has nothing like it.
- **Silico's forms cover:**
  - a line's length;
  - a circle's diameter and an arc's radius (and each the other way);
  - centre distances, and point to point aligned, horizontal, and vertical;
  - a point's distance from a line;
  - the 45° and 315° angles.
- **Silico has no form for:**
  - an arc's length;
  - a Min or Max arc condition (a gap to a circle's edge);
  - a distance between two lines;
  - a doubled distance to a centreline;
  - an angle shown as the supplement (135° for lines at 45°).

  PlaneGCS 1.1.3 already has an arc length and the Min-type gaps (point, line, or circle to a circle's edge), and FreeCAD 1.1.3 has them as `Distance`, so those could be checked against the oracle. Max and the doubled distance have no PlaneGCS row of their own. The supplement needs only a new field, since FreeCAD sends the same angle row with one line reversed.
- **A conflict between M5.0's own lines.** Section 7's sketch edits refuse an edit whose solve conflicts. Section 9 keeps a conflicting dimension "as the document holds it" so the person can leave it driving. #196's M5.11 test refuses it. Which one holds is for M5.0 to settle (recommendation 9).

## 1. How Smart Dimension works

- Smart Dimension dimensions 2D and 3D sketch entities. "The default dimension type is Parallel." Right-click > *More Dimensions* offers Horizontal, Vertical, Ordinate, Horizontal Ordinate, and Vertical Ordinate ([Dimensioning a 2D Sketch](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Dimensioning_a_2D_Sketch.htm)).
- The steps: pick the items, move the pointer, click to place. "As you move the pointer, the dimension snaps to the closest orientation." Esc undoes the last pick ([Dimensioning a 2D Sketch](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Dimensioning_a_2D_Sketch.htm)).
- **Locking the kind.** Move the pointer until the preview shows the kind you want, then right-click to lock it. Then "Measurement is locked; dimension position is available" ([Locking Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Locking_Dimensions.htm)).
- **Horizontal and Vertical Dimension** are separate tools. Horizontal is "between two entities" ([Creating Horizontal Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Creating_Horizontal_Dimensions.htm)), and Vertical "between two points" ([Creating Vertical Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Creating_Vertical_Dimensions.htm)). Each direction is the sketch's.
- **A preview without the tool.** With *Preview sketch dimension when selected*, selecting an entity shows its dimension, and clicking that preview edits it ([Previewing Sketch Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_previewing_sketch_dimensions.htm)).
- **Auto Insert Dimension.** The page's two lines are first made parallel (Ctrl-select them, then *Make Parallel* on the context toolbar). Ctrl-selecting them again, the context toolbar offers *Auto Insert Dimension*, which opens the Modify box ([Controlling Dimensions with the Context Toolbar](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Control_Dimension_Context_Toolbar.htm)). The page doesn't say which dimension it inserts.
- **The first dimension scales the sketch.** "When you specify the first dimension in a sketch for parts or assemblies, all entities in the sketch profile automatically scale uniformly based on the dimension." It "only applies when you add the very first dimension to a sketch. Angular and scalar dimensions are not supported" ([Maintaining Proportions in a Sketch Profile](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_maintain_proportion.htm)). Whether it can be turned off is unknown.

## 2. One entity

The help's table ([Dimensioning a 2D Sketch](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Dimensioning_a_2D_Sketch.htm)):

| To dimension the… | Click… | Note |
|---|---|---|
| Length of a line or edge | The line. | |
| Radius of an arc | The arc. | |
| True length of an arc | The arc, then the two end points. | |
| Diameter of a circle | The circumference. | "Displayed as linear or diameter, depending on placement." |

- **A line.** Its length. Whether moving the pointer gives a line's horizontal or vertical extent instead is unknown. The help's sentence on placement names only point-to-point, angular, and circular dimensions.
- **A circle.** Its diameter. Placement decides whether it's drawn as a diameter or as a linear dimension. *Linear* "Specifies the display of a diameter dimension as a linear dimension (not radial)", so it's still a diameter ([Dimension Leaders PropertyManager](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_DIMENSION_LEADERS.htm)).
- **An arc.** "The default dimension type for an arc is radius. You only need to select the arc for this dimension type" ([Creating Arc Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_creating_arc_dimensions.htm)).
- **An arc's length.** Select the arc, then "Press Ctrl and select the two arc endpoints" ([Creating Arc Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_creating_arc_dimensions.htm)). The 2D sketch table says "The arc, then the two end points", without Ctrl. The two pages disagree on Ctrl.
- **Radius or diameter of either.** It's a display choice on the dimension, made after it's placed: Leaders tab > *Witness/Leader Display*:
  - *Radius*: "the dimension on an arc or circle displays the radius";
  - *Diameter*: "displays the diameter".
  ([Dimension Leaders PropertyManager](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_DIMENSION_LEADERS.htm); [Creating Circular Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Creating_Circular_Dimensions.htm), which also offers it from right-click > *Display Options*). Whether the shown value changes with it (a radius of 5 for a 10 diameter) isn't said in words. Whether a radius can be chosen while placing is unknown.
- **Two points of one arc.** Shift-select a quadrant of the arc, then the centre or another quadrant ([Dimensioning Two Points of the Same Arc](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_dimension_two_points_same_arc.htm)).

## 3. Two entities

### Two lines

- **Parallel:** "Distance between two lines | Two parallel lines or a line and a parallel model edge" ([Dimensioning a 2D Sketch](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Dimensioning_a_2D_Sketch.htm)).
- **Not parallel:** an angle. "Placement of the dimension affects the way that the angle is measured" ([Dimensioning a 2D Sketch](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Dimensioning_a_2D_Sketch.htm)). "The angle to be dimensioned changes based on the pointer position" ([Creating Angular Dimensions Between Two Lines](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Creating_Angular_Dimensions_Between_Two_Lines.htm)).
- **Which angles.** [Angular Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Angular_Dimensions.htm) shows four dimensions "created by selecting the two sketch lines, then selecting a different location for each dimension". Its pictures, a horizontal line and one at 45° from a shared end, read 45°, 135°, 135°, and 315°. The text doesn't say how the pointer's position selects among them.
- **Unknown:**
  - how parallel two lines must be to give a distance (a parallel relation, or within a tolerance);
  - whether a distance between lines that are parallel but not related adds a parallel relation, or holds only while they stay parallel.
- **A line and an imaginary line.** "You can use the Smart Dimension tool to create an angle dimension between a line and an imaginary horizontal or vertical line" ([Angular Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Angular_Dimensions.htm)).
- **Three points.** Separately, an angle can be placed at a vertex point and two other points ([Creating Angular Dimensions Between Three Points](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Creating_Angular_Dimensions_Between_Three_Points.htm)).

### A line and a circle or arc

- "Distance when entities are an arc or a circle | The centerpoint or the circumference of the arc or circle, and the other entity (line, edge, point, etc.) | By default, distance is measured to the centerpoint of the arc or circle, even when you select the circumference" ([Dimensioning a 2D Sketch](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Dimensioning_a_2D_Sketch.htm)).
- So a line and a circle give the centre's distance from the line, by default.
- The First arc condition can then be Min or Max (section 6). The help says to pick the circumference for another location: "Use circumference if you do not want the dimension location to be the center of the arc or circle" ([Dimensions between Arcs or Circles](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Dimensions_Between_Arcs_or_Circles.htm)). That the conditions are available only then is the blog's (*unofficial*). Whether Shift picks Min for a line and a circle, as it does for two arcs, is unknown in the help.

### Two circles, two arcs

- Default: centre to centre. "By default, distances are measured to the center of an arc or circle" ([Dimensions between Arcs or Circles](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Dimensions_Between_Arcs_or_Circles.htm)).
- [Creating Dimensions Between Two Arcs](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_creating_dimensions_edges_arcs.htm) gives three ways:
  - "Select the edge of the first arc, and then select the edge of the second arc." This "Applies a minimum dimension between the first and second arc centers." The wording is the page's own: a centre-to-centre distance it calls minimum.
  - "Press Shift, and then click to apply the dimension between the two edges." This "Applies a minimum dimension between the first and second arc edges."
  - "Select the edge of the first arc, press Shift and then select the edge of the second arc." This "Applies a minimum dimension between the center of the first arc and the edge of the second arc."
  - Each "Opens the Dimension PropertyManager where you can change the way the distance is measured in all three instances."
- **Concentric circles.** "Click the edge of one concentric circle, then click the edge of the second concentric circle", and place it. The picture shows a dimension across the gap between the edges ([Creating Dimensions Between Concentric Circles…](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_creating_dimensions_concentric_circles.htm)). Whether that is a radial gap or a difference of diameters isn't said in words.
- **Unknown:** whether a centre-to-centre distance offers horizontal and vertical by placement, as two points do.
- **The help says "arcs" and "circles" alike.** The pages above treat the two the same way. No page gives arcs a rule of their own.

## 4. A point and an entity

- **A point and a line:** "Perpendicular distance from a point to a line | The point and the line or model edge" ([Dimensioning a 2D Sketch](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Dimensioning_a_2D_Sketch.htm)).
  - Unknown: whether placement offers the point's horizontal or vertical distance to the line instead.
- **A point and a circle or arc:** the row quoted in section 3: the distance to the centre by default, even when the circumference is picked. The first arc condition can make it Min or Max (section 6).
- **A point and a centreline:** section 7.

## 5. Two points

- "You can place a horizontal, vertical, or linear dimension between two sketch points, sketch segment endpoints, or model vertices. You can also use the model origin as a point. Select the two points, then move the pointer around to observe the dimension preview" ([Creating Dimensions Between Two Points](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Creating_Dimensions_Between_Two_Points.htm)).
- The 2D sketch table: "These distance dimensions were all created by selecting the same two points, then selecting a different location for each dimension."
- Unknown: which region of the pointer gives which kind. Only pictures show it.
- **Signs.** "You can specify zero and negative values for sketch dimensions. This is helpful when you want to flip an entity's sense of direction." The Modify box's *Reverse the sense of the dimension*, or typing a negative number, reverses a position dimension ([Creating Zero and Negative Value Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_zero_negative_dimensions.htm)). The help doesn't say which dimensions count as position dimensions.

## 6. Arc conditions

- **The setting.** "First or Second arc condition | Select Center, Min, or Max." "First Arc Condition specifies where on the arc or circle the distance is measured. Second Arc Condition specifies where on the second item the distance is measured, when both items are arcs or circles." The pictures are labelled "Center (default)", "Min (closest point)", and "Max (farthest point)" ([Dimension Leaders PropertyManager](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_DIMENSION_LEADERS.htm), *Arc Condition*).
- **Where it's set.** Click the dimension, open the Leaders tab, and set both conditions. "To display both First arc condition and Second arc condition in the Dimension PropertyManager, select the edges of both arcs with the Smart Dimension tool" ([Changing the Way the Distance is Measured](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_changing_distance_measured.htm)).
- **While picking:** Shift, as in section 3. Holding Shift while picking a circle "toward the inside" gave Min there, "based on where I selected" (*unofficial*, [the blog](https://blogs.solidworks.com/tech/2020/03/using-solidworks-smart-dimension-tool-when-sketching-arcs-circles.html), linked from the help).
- **From the circumference.** The help: "Use circumference if you do not want the dimension location to be the center of the arc or circle" ([Dimensions between Arcs or Circles](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Dimensions_Between_Arcs_or_Circles.htm)). That the conditions are available only then is the blog's: "Center is the default, but min and max are options when you do dimension to an arc by picking its circumference" (*unofficial*).
- **Ordinates.** Smart Dimension can also place "Ordinate dimensions to minimum, center, and maximum arc extents between arcs or between a line or a point and an arc" ([Dimensions between Arcs or Circles](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Dimensions_Between_Arcs_or_Circles.htm)).
- **Min on an arc.** For an arc, it's unknown whether Min and Max are measured to the arc or to its full circle. The blog's transcript says of Min, "we go to the theoretical insight if this was a full circle", as transcribed. The help's sketch relations treat an arc as its full circle (docs/ux/solidworks.md, §2).

## 7. Centrelines and revolve sketches

- **Radius or diameter by side.** "In a sketch with a centerline and lines or points", select "the centerline and a line or point. To create a radius, move the pointer to the near side of the centerline. To create a diameter, move the pointer to the far side of the centerline." Later picks reuse the centreline: "Select other lines or points in the sketch to create additional dimensions." It's for "sketches for revolved geometry that require several diameter dimensions" ([Using Centerlines to Create Radial and Diametric Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_dimensioning_centerlines.htm)).
- **Doubled distance.** "The doubled distance between a sketch entity and centerline | The sketch entity, then the centerline. Then drag the pointer to the opposite side of the centerline." It "create[s] a value that is twice the distance of the sketch entity to the centerline… helpful when you create profile sketches for revolved features" ([Dimensioning a 2D Sketch](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Dimensioning_a_2D_Sketch.htm)).
  - The two pages give the picks in opposite orders: centreline first in one, entity first in the other.
  - The help also lists Doubled Distance among its sketch relations: "A centerline and any sketch entity", "The sketch entity is dimensioned at twice the distance from the centerline" ([Description of Sketch Relations](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Description_of_Sketch_Relations.htm)).
- **In the Revolve page.** "When you dimension a revolve feature inside the centerline, you produce a radius dimension for the revolve feature. When you dimension across the centerline, you produce a diameter dimension for the revolve feature." Also: "You must rebuild the model to display the radius or diameter dimension symbol" ([Revolves](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_revolves_folder.htm)).
- **What a centreline is.** "Points and centerlines are always construction entities", and "Construction geometry uses the same line style as centerlines" ([Construction Geometry](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Construction_Geometry.htm)).
  - Unknown: whether any construction line (one converted *For construction*) acts as a centreline for this, and whether the origin's axes do.
  - Unknown: what a line not parallel to the centreline gives here, beyond the symmetric angles below.
- **Symmetric angles.** Select "the centerline and a non-parallel line". Moving the pointer gives a half angle; holding Shift gives the full angle, twice it ([Creating Symmetrical Angular Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_create_sym_ang_dims.htm)).

## 8. Driven and driving

- **What driven means.** "Driven | Appears as gray in the graphics area. Indicates a dimension that is redundant and cannot be modified." A tip follows: "When you add a redundant dimension, you can select Make this dimension driven and click OK in the dialog box. The dimension changes from red (over defined) to gray" ([Sketch Geometry Status](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Geometry_Status.htm)).
- "Primary Value is displayed for driving dimensions and can be changed to alter the model… Driven (reference) dimensions list a value and name, but you cannot change them" ([Dimension Value PropertyManager](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_DIMENSION_VALUE.htm)).
- Driven dimensions are shown in parentheses by default (*Add Parentheses*, same page).
- **When a placement is driving.** No page says in one sentence that Smart Dimension places a driving dimension. Each page that makes one driven names its condition, below. Outside them, the Modify box sets a value that changes the geometry.
- **When a placement is driven.**
  - **Over-defining, by option.** Sketch Options > *Over defining dimensions* has two checkboxes ([Sketch Options](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_OPTIONS_SKETCH.htm)). Together they make "one of four actions":
    - *Prompt to set driven state* "Displays a dialog box with the question, Make Dimension Driven? when you add an over defining dimension to a sketch";
    - *Set driven by default* "Sets the dimension to be driven by default when you add an over defining dimension to a sketch";
    - the four actions: "A dialog box appears that defaults to driven. A dialog box appears that defaults to driving. The dimension is driven. The dimension is driving."
    - Which checkboxes are on by default is unknown.
  - **The prompt.** "*Make Dimension Driven?* Dialog Box" ([Relations/Snaps Options](https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_relations_snaps_options.htm)):
    - "Click Cancel to cancel the operation (the dimension is not placed), or select one of the following options, then click OK";
    - "Make This Dimension Driven | The over defined or unsolvable state is repaired";
    - "Leave This Dimension Driving | The sketch remains over defined or unsolvable. Look for other dimensions or relations that you can delete or change";
    - "Select Don't ask again to suppress this dialog box. The option you last used will apply by default";
    - "Use the right-click menu Driven option to toggle between dimension driven and dimension driving."
    - So the prompt covers an unsolvable sketch as well as an over-defined one.
  - **A closed profile.** "If you dimension a closed-profile sketch that would cause an over defined sketch, the dimension is automatically changed to driven." The page's example is the dimension added with the last entity of a closed profile, which clashes with inferred relations ([Automatic Driven Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Driven_Dimensions.htm)). It doesn't say whether this overrides the Sketch Options above. The two pages may disagree for a Smart Dimension on a closed profile.
  - **While sketching.** With *Sketch Numeric Input*, *Sketch Dimension Driven*, and *Add Dimension* on, sketching a line, rectangle, circle, or arc inserts driven dimensions ([Inserting Driven Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Inserting_Reference_Dimensions.htm)).
  - **Afterwards.** Select dimensions, right-click > *Driven*; or the PropertyManager's Other tab > Options > *Driven* ([Setting Multiple Dimensions to Driven](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_settings_multiple_dimensions_to_driven.htm)).
- **Over-defining includes redundant.** The status page calls a driven dimension "redundant", and the prompt fires on "an over defining dimension". The help doesn't separate a dimension that agrees with the sketch (redundant) from one that conflicts. Both are "over defined" in its states (docs/ux/solidworks.md, §2).

## 9. The Modify box

- **When it opens.** System Options > General > *Input dimension value* "Opens the Modify dialog box when you insert a dimension" ([System Options - General](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_OPTIONS_SYSTEM_GENERAL.htm)). The default is unknown. "If the Modify box does not appear, either double-click the dimension or select Input dimension value" ([Creating Circular Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Creating_Circular_Dimensions.htm)).
- **Order.** Some pages set the value before the click that places the dimension: [Two Points](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Creating_Dimensions_Between_Two_Points.htm), [Two Lines](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Creating_Angular_Dimensions_Between_Two_Lines.htm), and [Arc Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_creating_arc_dimensions.htm). Others set it after: [Circular](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Creating_Circular_Dimensions.htm) and [Locking](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Locking_Dimensions.htm). The help doesn't settle it.
- **Entering the value** ([Modify Dimension](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_modify_dimension_dialog_box.htm)):
  - by typing, or the arrows, thumbwheel, or mouse wheel, by the spin increment (Ctrl ×10, Alt ÷10);
  - a slider instead of the thumbwheel "when the numeric value is bounded, such as an angle that must be greater than 0 and less than 90 degrees";
  - "You can use the dimension box as a calculator by entering values and arithmetic symbols directly. You can also assign units of measurement to a value by using the type-ahead list or by typing the unit directly."
- **The buttons:**
  - "Save the current value and exit the dialog";
  - "Restore the original value and exit the dialog";
  - "Regenerate the model with the current value";
  - "Reverse the sense of the dimension";
  - reset the spin increment;
  - mark the dimension for a drawing.
  The configuration choices are outside Silico.
- **Equations.** "In the second line of the Modify dialog box, enter = (equal sign). Then enter an equation in the same way as in the Equations dialog box." An Equations icon then marks the dimension, and "You cannot change the evaluated value of an equation in the Modify dialog box" ([Creating an Equation](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_creating_equation.htm)). A dimension driven by an equation or a global variable shows its icon in the box ([Modify Dimension](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_modify_dimension_dialog_box.htm)).
- **In the canvas.** Instant2D: "Click the dimension value and enter the new dimension in the dimension input box", or drag the dimension's handles, "without opening the Modify Dimension dialog box" ([Modifying a 2D Sketch Dimension with Instant2D](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Modifying_2D_Sketch_Dimension_Instant2D.htm)). M5.0's section 8 already has dimensions edited in the canvas and accepted with Enter, Fusion's UI.

## 10. Where the help's pages disagree

- **An arc's length:** "The arc, then the two end points" ([Dimensioning a 2D Sketch](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Dimensioning_a_2D_Sketch.htm)), against "Press Ctrl and select the two arc endpoints" ([Creating Arc Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_creating_arc_dimensions.htm)).
- **A centreline's picks:** entity first, then the centreline ([Dimensioning a 2D Sketch](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Dimensioning_a_2D_Sketch.htm)), against the centreline first ([Using Centerlines…](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_dimensioning_centerlines.htm)).
- **Two arcs' edges, no Shift:** a "minimum dimension between the first and second arc centers" ([Creating Dimensions Between Two Arcs](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_creating_dimensions_edges_arcs.htm)), where every other page calls centre to centre the default, Center.
- **Horizontal Dimension** takes "two entities", and **Vertical Dimension** "two points" (section 1).
- **Over-defining:** "automatically changed to driven" for a closed profile ([Automatic Driven Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Driven_Dimensions.htm)), against the prompt and driven-by-default options ([Sketch Options](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_OPTIONS_SKETCH.htm)). Both may hold if the first covers only dimensions made while sketching.
- **The Modify box's order** (section 9).
- **The colour of an over-defined dimension:** red in the driven tip, yellow under *Item Conflicts* on the same page. This is already in docs/ux/solidworks.md, §2.

## 11. Silico's forms against each selection

Silico's dimensional forms (D4; `Form` in `sketch.rs`), each with `driving` always written:

- `length` (a line);
- `distance` (two points, the origin among them);
- `distance_x` and `distance_y` (two points, signed: x(b) − x(a));
- `point_line_distance` (a point, a line or axis, and the `side` of the line it's on);
- `angle` (two lines or axes, from `a`'s direction to `b`'s, counter-clockwise, signed, at most 360 either way);
- `radius` and `diameter` (a circle or arc).

A circle's or arc's centre is a sketch point that the curve names (D2), so picking a centre is picking a point. Values are `Length` (whole nanometres) and `Angle` (whole nanodegrees), read exactly.

| Selection | SolidWorks places | Silico | Fit |
|---|---|---|---|
| A line | its length | `length {line}` | fits |
| A circle | its diameter (shown as a diameter or linear) | `diameter {curve}` | fits |
| A circle, shown as a radius | the same dimension, shown as a radius | `radius {curve}` | fits, as a retype (below) |
| An arc | its radius | `radius {curve}` | fits |
| An arc, shown as a diameter | the same, shown as a diameter | `diameter {curve}` | fits, as a retype |
| An arc and its two ends | its true length | none | **missing** |
| Two parallel lines | the distance between them | none of its own; `point_line_distance` from an end of one line to the other | **doesn't fit** (below) |
| Two lines, not parallel | an angle, by the pointer | `angle {a, b}` | fits for 45° and 315°; **doesn't fit** for the supplement, 135° (below) |
| A line and an imaginary horizontal or vertical line | an angle | `angle {a: x_axis or y_axis, b: line}` | fits |
| A line and a circle, Center | the centre's distance from the line | `point_line_distance {point: centre, line, side}` | fits |
| A line and a circle, Min or Max | from the line to the circle's nearest or farthest point | none | **missing** |
| Two circles or arcs, Center/Center | centre to centre | `distance {a: centre, b: centre}` | fits |
| Two circles or arcs, Min or Max on either | edge to edge, centre to edge, … | none | **missing** |
| Concentric circles' edges | the gap between the edges | none | **missing** |
| A point and a line | its perpendicular distance | `point_line_distance {point, line, side}` | fits |
| A point and a circle, Center | to the centre | `distance {a: point, b: centre}` | fits |
| A point and a circle, Min or Max | to the nearest or farthest point of its edge | none | **missing** |
| Two points, aligned | the distance | `distance {a, b}` | fits |
| Two points, horizontal or vertical | the x or y distance | `distance_x` or `distance_y {a, b}` | fits; signed (below) |
| A centreline and a point or line, near side | a radius (the distance) | `point_line_distance {point, line: the centreline, side}` | fits for a point |
| A centreline and a point or line, far side | a diameter (twice the distance) | none | **missing** |

**Where a form is missing or doesn't fit:**

- **An arc's length.** None of Silico's forms measures along a curve. PlaneGCS 1.1.3 has `addConstraintArcLength` (`GCS.h:479`; its error is `rad · (end − start) − length`, `Constraints.cpp:3135-3153`). FreeCAD 1.1.3's `Distance` on an arc is an arc length (docs/ports/sketcher.md, *Constraints*). A new form, say `arc_length {arc, value, driving}`, could be checked against the oracle.
- **The Min and Max arc conditions.** A distance to a circle's edge isn't a distance to a point, so none of today's forms holds it while the radius changes.
  - PlaneGCS 1.1.3 has gaps of the Min kind: point to circle (`addConstraintP2CDistance`), line to circle (`addConstraintC2LDistance`), and circle to circle (`addConstraintC2CDistance`), `GCS.h:476-478`. The circle-to-circle gap also covers concentric circles (its inner case, `Constraints.cpp:2927`). FreeCAD 1.1.3 has each as `Distance`.
  - None of them stores a side. 1.1.3 keeps a side "only because the solver starts there" (docs/ports/sketcher.md, *Constraints*, which already leaves the circle–line distance tests unported for want of a form). Silico's forms store choices like that (D4: `side`, `direction`), so a new form would store a side and check it after the solve, as `tangent` does.
  - Max (a farthest point) has no PlaneGCS row and no FreeCAD form, so no oracle.
  - Centre to Min and similar mixes are a point-to-circle gap from the centre point.
- **A distance between two parallel lines.** Silico has no line-to-line distance.
  - The nearest form is `point_line_distance` from an end of one line to the other. FreeCAD's dimension tool writes exactly that for two parallel lines: the second line's start to the first line (`CommandConstraints.cpp:3667-3671`, at the clone's `876271d660`).
  - It holds the lines apart only at that end. They stay parallel only if a relation keeps them so (a `parallel`, or both `horizontal` or `vertical`).
  - SolidWorks' help asks for "two parallel lines" and doesn't say what holds them (section 3).
  - The choices are to write it that way, saying which end is used, or to add a `line_distance` form that also implies parallel. That second form has no single PlaneGCS row: it would be a point-line distance plus a parallel.
- **An angle's supplement.** `angle` is from `a`'s direction to `b`'s, and a line's direction is from its `from` to its `to`.
  - For the help's pair, a horizontal line `a` and a line `b` at 45° from a shared end, 45° is `angle {a, b}` = 45.
  - 315° is `angle {b, a}` = 315. PlaneGCS's angle row compares directions modulo a turn (`ConstraintL2LAngle::error`, `Constraints.cpp:1293-1304`), so 315 holds that same geometry.
  - 135° needs one line's direction reversed, and Silico's form can't say so: 135 in either order describes a different sketch.
  - FreeCAD stores it by naming which end of each line the angle is measured from: `Sketch::addAngleConstraint` with a start or end position reverses a line (`Sketch.cpp:3851-3887`, at the clone's main).
  - So showing the supplement needs a new field (each line's end, or a reversal flag), or the app writes the 45° form and shows 135° itself. The second means the stored value isn't the one the person typed.
- **A doubled distance to a centreline.** SolidWorks' help lists Doubled Distance among its sketch relations (section 7). No Silico form holds twice a distance. FreeCAD 1.1.3's types have none either (the port note's list), and PlaneGCS has no row for it. A new form is needed, say `point_line_distance` with a doubling, or a `diametral_distance {point, axis, side, value, driving}`. Its row would be the point-line distance at half the value.
  - Halving a `Length` in whole nanometres is exact only for an even count. A 25.000001 mm diameter (25,000,001 nm) has no exact radius in the file's unit, so the row has to take the half as a float. The worker converts lengths there already.
  - A line parallel to the centreline brings in the parallel-lines question above as well.
  - Silico has no centreline entity. The analogue is a construction line, or `x_axis` or `y_axis`, which a revolve can turn about (a revolve's `axis`: `x_axis`, `y_axis`, or a sketch line).
- **Radius or diameter as a display choice.** In SolidWorks the toggle changes how one dimension is shown. In Silico, `radius` and `diameter` are two forms, so the toggle is a retype that keeps the ID (as open item 10 retypes a split's `length`), with the value doubled or halved.
  - Doubling is exact. Halving an odd count of nanometres isn't: the retyped value would be rounded, which the exactness rule forbids.
- **Signed horizontal and vertical.** `distance_x` and `distance_y` are signed, x(b) − x(a). The help doesn't say which sign a horizontal or vertical distance shows when it's placed. It allows zero and negative values, and reverses a position dimension with a negative value or *Reverse the sense* (section 5). A signed form holds all of that. Which order of `a` and `b` a placement writes, and so the starting sign, is M5.11's to choose. Typing a negative value then flips the entity, as the help describes.
- **A side, given by the client.** `point_line_distance`'s `side` is "given, never decided from geometry" (D4; M5.0 §7, `sketch_add`). So the app reads the side from the solved geometry it draws (the `sketch` read) when it places the dimension.
- **Which `driving` a placement writes.**
  - D4 gives every dimensional form a `driving`. A reference dimension's `value` is "what the last solve measured, written back by M3.4, never solved for".
  - The help (section 8) implies driving unless the dimension over-defines. Silico's driven forms can be written by `sketch_add` (with `driving: false`) or by `set_field` of `constraints.ID.driving`.
  - What's missing is a way to keep a conflicting dimension driving (recommendation 9).

## For M5.0 and the maintainer

1. **What M5.11's Smart Dimension places**, where Silico has the form:
   - **A line:** `length`.
   - **A circle:** `diameter`; an arc: `radius`. Each shown as the other is a retype that keeps the ID, with the value doubled, or halved when the halving is exact.
   - **Two points:** `distance`, `distance_x`, or `distance_y` by where the pointer is. The horizontal and vertical forms are ordered so the value starts positive.
   - **A point and a line:** `point_line_distance`, its `side` read from the drawn geometry.
   - **A line, a point, or a circle with a circle or arc:** to the centre, by default (`point_line_distance` from the centre, or `distance` between centres or from the point).
   - **Two non-parallel lines:** `angle` for the angle between them and for its reflex (by the order of `a` and `b`).
   - **A centreline and a point, near side:** `point_line_distance` to the construction line or axis.
   - **Esc** undoes the last pick, and a right-click locks the kind, as SolidWorks has it.
2. **Driving.** A placement writes `driving: true`. Its value is the one the Modify box (or the canvas box) returns, so a single `sketch_add` writes the dimension with the value the person typed. If the box is skipped, it's the measured value, rounded to the nearest nanometre or nanodegree, since `Length` and `Angle` can't hold more. The core rounds it, not the app, so every client writes the same value.
3. **Arc length** (**new form**, flagged): `arc_length {arc, value, driving}`, from PlaneGCS 1.1.3's `ConstraintArcLength`, oracle-checked against FreeCAD's `Distance` on an arc. Until it exists, the arc-and-ends pick places nothing and says why.
4. **Min arc conditions and concentric gaps** (**new forms**, flagged): a point-to-circle, line-to-circle, and circle-to-circle gap, each with a stored side checked after the solve, from PlaneGCS 1.1.3's rows and FreeCAD's `Distance`. Max has no PlaneGCS row or oracle. *Recommended:* Center and Min in M5.11 or after, Max deferred. That's for the maintainer, as the forms change the schema.
5. **Two parallel lines** (**decision**): `point_line_distance` from one line's end to the other, as FreeCAD's tool writes (which end: the second line's `from`, say), or a new `line_distance` form. The first holds only with a parallel relation. Whether SolidWorks requires or adds one is unknown.
6. **An angle's supplement** (**decision or new field**): the 135° placement for lines at 45° can't be stored as typed. Either `angle` gains each line's end (FreeCAD's way), or M5.11 offers only the angle and its reflex.
7. **A diameter to a centreline** (**new form**, flagged): twice a point-line distance, for revolve sketches, with a stored side. Its row takes the half as a float. **Decision:** what counts as a centreline. *Recommended:* any construction line and the two axes, since SolidWorks' centrelines are always construction entities, and its construction geometry "uses the same line style as centerlines" ([Construction Geometry](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Construction_Geometry.htm)). Whether SolidWorks itself accepts a converted construction line is unknown. The symmetric full angle (twice an angle) is the same kind of form for angles.
8. **The first dimension scales the sketch** (**decision**). SolidWorks scales all entities in the sketch profile uniformly when the very first dimension is added, unless it's angular or scalar. Whether a later linear dimension then scales is unknown. That's behavior, which Silico follows SolidWorks on. It would be an edit of every point (a core edit, to keep the coordinates exact). Whether M5.11 does it, and whether it can be turned off, are the maintainer's.
9. **Over-defining** (**decision**, an inconsistency in M5.0). Section 9 offers the three choices and keeps a conflicting driving dimension "as the document holds it". Section 7's common rule refuses any sketch edit whose solve conflicts, `sketch_add` included. #196's M5.11 test also expects "a conflicting dimension is refused showing M3.3's conflict". *Recommended*, following SolidWorks:
   - the app places the dimension with `driving: true`;
   - if the edit is refused as a conflict, or accepted with the new dimension listed redundant, the app asks: make it driven, leave it driving, or Cancel.
   - **Driven** is `sketch_add` with `driving: false` (after a refusal) or `set_field` of `driving` (after a redundant one).
   - **Cancel** is nothing (after a refusal) or `undo` (after a redundant one).
   - **Leave it driving** after a refusal needs an edit that keeps a conflicting sketch, which section 7 doesn't allow. Section 7 should name the exception, or M5.11 offers only two choices for conflicts and three for redundancy.
   - The prompt's two options (prompt, driven by default) and *Don't ask again* are SolidWorks settings. Whether the app has them is the maintainer's.
10. **The Modify box.** A number, read exactly as the document reads a value. SolidWorks' arithmetic ("as a calculator") and units can be evaluated only if the result is exact in nanometres. `10/3` isn't. Equations wait for expressions, for which the format reserves strings. **Decision:** plain numbers only in M5.11. *Recommended:* yes.
11. **Unknown in the help, for the maintainer if it matters:**
    - a line's horizontal or vertical extent by placement;
    - which pointer regions give which kind;
    - Shift for a line or point and a circle;
    - the over-defining options' defaults and *Input dimension value*'s;
    - whether Min and Max on an arc measure to its full circle.

## Method

- **Fetching.** The pages were fetched from help.solidworks.com with `curl`, using a browser's user agent; the site refuses `curl`'s own. The help is a Next.js page with the topic's HTML inside its JSON data. A short script took the HTML between the topic's `<main>` tags, unescaped it, and turned it into text, with images by their alt text. The scripts and pages were in `/tmp`, outside the repository.
- **Coverage.** Thirty-nine pages were read, each at `2027` and at `2026`, and thirty-seven are cited. Each pair's text was compared ignoring white space, and all were the same. The list of pages started from the help's [Dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_dimensions_top.htm) topic and its children, with searches restricted to help.solidworks.com for the rest.
- **Pictures.** The four pictures on *Angular Dimensions* were downloaded and read for the angles they show: 45°, 135°, 135°, 315°. No other picture was read; where a page shows something only in a picture, this note says so.
- **Outside the help.** The SOLIDWORKS Tech Blog post the help links to was read once (2026-10-29) and is marked *unofficial*. FreeCAD's code was read in the maintainer's clone at `~/Git/FreeCAD` (`876271d660`, main, not tagged 1.1.3), only to say what a missing form's counterpart is. PlaneGCS was read as vendored in `third_party/planegcs` (1.1.3).
- Nothing was built or run against Silico.
