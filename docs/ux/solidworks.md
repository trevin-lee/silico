# SolidWorks' behavior

A study note for M5 (docs/roadmap/m5-app.md, *Study*): what SolidWorks does in the parts of the app M5 builds, so M5.0's design can cite it and a test can be written from each line. It was read from the SOLIDWORKS 2027 Design Help (help.solidworks.com/2027, the current release) on 2026-10-27. Every page cited was also read in the 2026 help, and each says the same in both, except the What's Wrong dialog, whose assistant is renamed, and *Hiding and Showing Sketches* (§4), read only in the 2022 help because the 2027 page renders only in JavaScript. It records behavior only: UI and visuals follow Fusion 360 (docs/ux/fusion.md). Colours appear only where they carry state (a sketch's definition, a feature's error), since M5.11 colours a sketch as SolidWorks does. Where the help says nothing, or two of its pages disagree, the line says so. A line marked *unofficial* cites a source outside Dassault's documentation, used only because the help is silent.

## 1. Selection and pre-selection

### Hover

- Select is the mode when no command is active, and most commands return to it when they end ([Select](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Select.htm)).
- Dynamic highlighting shows which entity a click would select as the pointer moves over faces, edges, and vertices; it's an option, *Dynamic highlight from graphics view* ([Display Options](https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_display_options.htm), [Select](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Select.htm)).
- A hovered edge highlights, and a hovered face highlights by its edges (SolidWorks' look, kept for reference only: thick and thin solid lines) ([Highlighting](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Highlighting.htm)).
- A selected edge and a selected face highlight (SolidWorks' look, for reference only: a thick solid line, and a solid colour when shaded with RealView off) ([Highlighting](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Highlighting.htm)).
- In a sketch, endpoints, midpoints, and vertices highlight as the pointer approaches, and change colour when a click would take them ([Highlighting](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Highlighting.htm)).

### Click, Ctrl, Shift

- A click selects the entity under the pointer, in the graphics area or the FeatureManager tree; holding Ctrl selects several ([Select](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Select.htm)).
- The help doesn't say that a click without Ctrl replaces the selection, or that a Ctrl-click on a selected item deselects it.
- In the graphics area, the selection pages give Shift on a click one role: it reverses *Enable selection through transparency* for that click ([Selection Options](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_OPTIONS_SELECTION.htm)).
- In the FeatureManager tree, Shift selects a consecutive run, Ctrl adds items one by one, and a drag from empty space in the panel box-selects ([Selecting from the FeatureManager Design Tree](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_selecting_from_featuremanager.htm)).
- An item selected in the tree highlights in the graphics area ([Selecting from the FeatureManager Design Tree](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_selecting_from_featuremanager.htm)); an item selected in the graphics area expands and scrolls the tree to it, and a click in the graphics area that selects nothing restores the tree's scroll (default on) ([FeatureManager Design Tree Response to Selections](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_featuremanager_design_tree_response_selections.htm)).

### Box, cross, and lasso

- A drag from empty space draws a box. Left to right selects only items completely inside it; right to left (cross select) also selects items crossing its boundary ([Box Selection](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Box_Selection.htm), [Cross Selection](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Cross_Selection.htm)).
- (SolidWorks' look, for reference only.) A box is drawn in solid lines, a cross selection in dotted lines ([Cross Selection](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Cross_Selection.htm)).
- In a part, a box selects edges by default; selection filters select other types ([Box Selection](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Box_Selection.htm)). In a sketch, a cross selection takes sketch entities and dimensions ([Cross Selection](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Cross_Selection.htm)).
- Shift with a box adds everything inside it, selected or not; Ctrl with a box inverts the selection inside it; either way, selections outside the box stay ([Box Selection](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Box_Selection.htm)).
- In drawings, hidden edges and faces aren't box-selected unless *Allow selection in wireframe and HLV modes* or *Allow selection in HLR and shaded modes* is on, each for its display modes ([Box Selection](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Box_Selection.htm), [Selection Options](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_OPTIONS_SELECTION.htm)).
- A drag that starts on geometry selects the geometry under the press and draws no box. In *Select over Geometry* mode (T) a drag draws a box anywhere: a drag replaces the selection, Shift with a drag adds to it ([Selecting Over Geometry](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_select_over_geometry.htm)).
- Lasso replaces the box when chosen as the default bulk selection method. A clockwise lasso selects only items inside the loop; a counterclockwise one also selects items crossing it, in sketches and drawings only. The loop needn't be closed ([Selecting Items by Lasso](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_lasso_selection.htm)).

### Clearing a selection

- The help doesn't say how a selection in the graphics area is cleared (a click on empty space, Esc). It names *Clear Selections* on the right-click menu in one procedure ([Manually Selecting Each Edge](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Manually_Selecting_Each_Edge.htm)).
- *Previous Selection* restores the last selection made and cleared in the current editing session; it's offered on the right-click toolbar when nothing is selected ([Previous Selection](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_previous_selection.htm)).
- In a PropertyManager's selection box, right-click *Delete* removes one item and *Clear Selections* removes them all ([PropertyManager Overview](https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_pm_overview.htm)).

### Selection filters

- A filter limits what can be selected to its types: with the face filter on, only faces ([Selection Filter Toolbar](https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_selection_filter.htm)).
- F5 shows or hides the filter toolbar; F6 turns the chosen filters on or off; *Clear All Filters* and *Select All Filters* do as named ([Selection Filter Toolbar](https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_selection_filter.htm)).
- Default shortcuts: E filters edges, X faces, V vertices ([Selection Filter Toolbar](https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_selection_filter.htm)).
- With a filter on, the pointer changes, and near an item the filter takes it shows that item's icon ([Selection Filter Toolbar](https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_selection_filter.htm)).
- *Invert Selection* selects every other item of the selected kinds and clears the original ones ([Selection Filter Toolbar](https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_selection_filter.htm)).
- The help doesn't say whether F6 or the E, X, V shortcuts add a filter to those on or replace them.

### Select Other

- Right-click the model and choose *Select Other*: a list of the entities under the pointer, in the order they're met, each with its type's icon ([Select Other](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_SelectOther_dlg.htm)).
- Hovering an item in the list highlights it in the model; Tab or the wheel steps through the list; a click in the list or the graphics area selects; right-clicking the model hides a face to reveal what's behind it ([Select Other](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_SelectOther_dlg.htm)).
- The right-click toolbar on empty space offers Box, Lasso, Selection Filters, Select Other, and the magnifier ([Selection Context Toolbar](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Selection_Tool_access.htm)).

## 2. Sketch inference and automatic relations

### Relations added while sketching

- As you sketch, the pointer shows which relations can be created; with *Automatic relations* on, they're added. The help's examples: horizontal, vertical, coincident, midpoint, perpendicular, tangent ([Using Automatic Relations](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Using_Automatic_Relations.htm)).
- A relation the pointer shows at the click that accepts the entity is added to that entity ([Inferencing](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_inferencing.htm)).
- Several relations can show at once, depending on the entities and the pointer's position ([Using Automatic Relations](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Using_Automatic_Relations.htm)).
- Holding Ctrl while sketching suppresses automatic relations for as long as it's held ([Using Automatic Relations](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Using_Automatic_Relations.htm)).
- *Automatic relations* is turned off at Tools > Sketch Settings or Options > Relations/Snaps ([Using Automatic Relations](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Using_Automatic_Relations.htm), [Relations/Snaps Options](https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_relations_snaps_options.htm)).
- Clearing *Enable snapping* also clears *Automatic relations*, and no relations are shown ([Inferencing](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_inferencing.htm)).
- A relation to a line is to the infinite line; a relation to an arc is to its full circle ([Description of Sketch Relations](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Description_of_Sketch_Relations.htm)).
- *On Edge* is the relation Convert Entities makes when it projects a solid's edges; Offset and Convert add relations automatically ([Description of Sketch Relations](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Description_of_Sketch_Relations.htm)). The help doesn't say that sketching onto a model edge adds one.
- A tangent arc started at a line's, arc's, ellipse's, or spline's endpoint is tangent or normal to it as the pointer's first motion says (four intent zones); returning to the endpoint and leaving in another direction switches ([Sketching Tangent Arcs](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_Tangent_Arcs.htm)).
- A line being edited that is nearly horizontal or vertical gets *Make Horizontal* or *Make Vertical* highlighted on its context toolbar: offered, not added ([Horizontal and Vertical Sketch Relations](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_horizontal_vertical_sketch_relations.htm)).
- A rectangle's PropertyManager lists "relations inferenced automatically during sketching" ([Rectangle PropertyManager](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_SKETCH_RECTANGLES.htm)). The help doesn't say which. *Unofficial:* a corner rectangle is four lines with a vertical relation on each side and a horizontal one on the top and bottom ([LinkedIn Learning, *SOLIDWORKS 2020 Essential Training*, "Using the Rectangle tool"](https://www.linkedin.com/learning/solidworks-2020-essential-training/using-the-rectangle-tool)).

### Relations added by hand

- Relations are added to a selection: "Ctrl + select two lines. On the context toolbar, click Make Parallel" ([Context Toolbar dimensions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Control_Dimension_Context_Toolbar.htm)); or from *Add Relations*, whose list "includes only relations that are possible for the selected entity" ([Line Properties PM](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_SKETCH_LINE.htm)).
- The context toolbar "stays visible to allow you to add multiple relations" to the same selection ([Sketch Relations Overview](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Relations_Overview.htm)).
- What each takes ([Description of Sketch Relations](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Description_of_Sketch_Relations.htm)): Horizontal or Vertical, "One or more lines or two or more points"; Coincident, "A point and a line, arc, or ellipse"; Merge Points, "Two sketch points or endpoints"; Tangent, "An arc, ellipse, or spline, and a line or arc"; Equal, "Two or more lines or two or more arcs"; Parallel, "Two or more lines"; Perpendicular, "Two lines"; Midpoint, "Two lines or a point and a line"; Symmetric, "A centerline and two points, lines, arcs, or ellipses"; Fix, "Any entity".
- A relation that conflicts is added, and the sketch reads over defined; a redundant one is added, and "The sketch is still fully defined" ([FAQ Sketch Relations](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_FAQ_Sketch_Relations.htm)). No page offers to make a relation driven.
- Silico's tools, and where they differ (Fix as two dimensions, Symmetric of points only), are M5.0's open item 34. The full study is docs/study/constraint-tools.md.

### Inference lines and snaps

- Inferencing lines are dotted lines, drawn while sketching, between the pointer and existing sketch entities or model geometry; they guide the pointer as it nears cues such as midpoints ([Inferencing](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_inferencing.htm)).
- The help doesn't say which inferencing lines add a relation and which only guide. *Unofficial:* blue lines are reference only, and lines in the *Temporary Graphics* colour capture a relation ([CATI, "What are Inference Lines?", 2014](https://www.cati.com/?p=120362)).
- Sketch snaps are all on by default but Grid: endpoints and sketch points, centre points, midpoints, quadrant points, intersections, nearest, tangent, perpendicular (line to line), parallel (lines), horizontal/vertical to a line (vertical to a horizontal line and the reverse), horizontal/vertical to points, length (with Shift held), grid, and angle ([Sketch Snaps](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Snaps.htm)).
- Only the entity being sketched shows snaps ([Sketch Snaps](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Snaps.htm)).
- With *Nearest* cleared, the pointer needn't be near a point to infer or snap to it; with it on, only near ([Sketch Snaps](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Snaps.htm)).
- The angle snap counts from the sketch's X and Y axes when there's no line, and from the existing line when you sketch from its end ([Relations/Snaps Options](https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_relations_snaps_options.htm)).
- *Snap to model geometry* lets sketch entities snap to model edges ([Relations/Snaps Options](https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_relations_snaps_options.htm)).
- The help doesn't say which snaps leave a relation behind (a parallel or perpendicular snap, a snap to a model edge) and which only place the point.

### Definition states

- A sketch is in one of five states, shown in the status bar: Fully Defined, Over Defined, Under Defined, No Solution Found, Invalid Solution Found. Each entity has a state too ([Sketch Status Conventions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Status_Conventions.htm)).
- Invalid Solution Found: solved, but to invalid geometry, such as a zero-length line, a zero-radius arc, or a self-intersecting spline ([Sketch Status Conventions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Status_Conventions.htm)).
- In a sketch the status bar shows the pointer's coordinates, the state (Over Defined, Under Defined, or Fully Defined), and "Editing Sketch" ([Working in a Sketch](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Working_in_a_Sketch.htm)).
- In the FeatureManager tree a sketch's name is prefixed `(+)` over defined, `(–)` under defined, `(?)` not solvable, and nothing when fully defined ([FeatureManager Design Tree Conventions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_FeatureManager_Design_Tree_Conventions.htm)).
- A feature can be built from an under-defined sketch, unless the option *Use fully defined sketches* is on ([Sketch Status Conventions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Status_Conventions.htm)).
- Colours by state: under defined blue, fully defined black, dangling olive, driven dimension gray, item conflicts yellow, invalid yellow, unsolvable red ([Sketch Geometry Status](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Geometry_Status.htm)).
- Entity endpoints, when shown: black fully defined, blue under defined, red over defined, green selected; over-defined and dangling points always show ([Sketch Options](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_OPTIONS_SKETCH.htm)).
- **Conflict, over defined:** red in Sketch Options and in Sketch Geometry Status's own note on driven dimensions ("changes from red (over defined) to gray"); yellow in the same page's *Item Conflicts* and in [Error Message - Item Conflicts](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDE_SKETCH_OVERDEFINED.htm).
- **Conflict, invalid and unsolvable:** Sketch Geometry Status says invalid is yellow and unsolvable red, and that unsolvable means the position can't be determined; [Error Message - No Solution Found](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDE_SKETCH_HAS_INVALID_GEOM.htm) swaps both the colours and the definitions.
- A dimension that over-defines a sketch, with the option set to prompt, asks: make it driven (the sketch is repaired, the dimension gray), leave it driving (the sketch stays over defined), or Cancel (it isn't placed) ([Relations/Snaps Options](https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_relations_snaps_options.htm), [Sketch Geometry Status](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Geometry_Status.htm)).
- The first dimension placed in a sketch scales the whole sketch uniformly, angular and scalar dimensions aside ([Maintaining Proportions in a Sketch Profile](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_maintain_proportion.htm)). Silico leaves this out: the help doesn't give the point the sketch scales about, or whether the behavior can be turned off, so a port would guess (M5.0, SD6).
- Over-defined error text: "The sketch conflicts. Consider deleting some over defining dimensions or relations." ([Error Message - Item Conflicts](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDE_SKETCH_OVERDEFINED.htm)).
- Dangling: a dimension or relation to geometry that no longer exists, drawn dashed in the dangling colour; text: "This sketch contains dimensions or relations to model geometry which no longer exists." ([Error Message - Dangling Geometry](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDE_SKETCH_EXTREFS_FAIL.htm)).
- *No Solution Found* message text (which state it names is the conflict above): "The sketch has not been updated because solving it would result in invalid geometry (such as a zero-length line)." ([Error Message - No Solution Found](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDE_SKETCH_HAS_INVALID_GEOM.htm)).

## 3. Dragging in a sketch

- Under-defined endpoints, lines, and curves can be dragged, changing the sketch's shape ([Sketch Status Conventions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Status_Conventions.htm)).
- A line's endpoint dragged changes its length or angle; the line dragged moves it; a horizontal or vertical relation must be deleted before its angle can change ([Modifying Lines by Dragging](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Modifying_Lines_By_Dragging.htm)).
- A circle's edge dragged changes its size; its centre dragged moves it ([Modifying Circles by Dragging](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Modifying_Circles_By_Dragging.htm)).
- Dimensions hold during a drag. With *Override Dims on Drag/Move* on, a drag can change dimensioned geometry, the dimensions update at the end of the drag, and they stay driving ([Override Dims on Drag/Move](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Override_Dims_on_Drag_Move.htm), [Sketch Options](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_OPTIONS_SKETCH.htm)).
- *Ghost image on drag* shows the entity's original position while it's dragged ([Sketch Options](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_OPTIONS_SKETCH.htm)).
- *Detach Segment on Drag* lets a dragged segment's endpoints come away from the entities they're attached to, where no dimension or relation prevents it; it deletes no relation ([Using Detach Segment on Drag](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Using_Detach_Segment_On_Drag.htm)).
- *No Solve Move* moves entities without solving; if they have dimensions or relations to other geometry it asks: Yes deletes those and moves, No leaves them and copies the entities to the new place ([Using No Solve Move](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Using_No_Solve_Move.htm)).
- With *Automatic Solve* off, the status bar says "Auto-Solve Mode Off"; past a set number of entities, Automatic Solve and Undo turn off ([Using Automatic Solve](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Using_Automatic_Solve.htm)).
- The help doesn't say what a release keeps beyond the dragged position: whether dropping an endpoint on another entity adds a relation, or what happens when the solver can't follow the pointer.
- The help doesn't say what dragging a fully defined entity does.

## 4. Sketch tools

### Line

- The first click decides the mode: press and drag is click-drag (one line, ended by the release); press and release is click-click ([Sketch Modes](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Modes.htm)).
- Click-click goes on sketching lines, a chain of segments, until a double-click ([Insert Line PropertyManager](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_SKETCH_LINE_CMD.htm)). A double-click or right-click *End chain* ends the chain and keeps the tool; Esc ends it and leaves the tool; choosing another tool ends it ([Sketch Modes](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Modes.htm)).
- A click-click line or arc ended on an existing endpoint keeps the tool but shows no preview from there ([Sketch Modes](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Modes.htm)).
- Orientation *As sketched*, *Horizontal*, *Vertical*, or *Angle*; the last three take a length (and angle), and *Add dimensions* places them ([Sketching Lines](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_Lines.htm)).
- From a line's end, moving back to the endpoint and away again previews a tangent arc instead (autotransition); A switches between line and arc ([Autotransitioning](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Autotransitioning.htm)).

### Rectangle, circle, arc, point

- Corner rectangle: press at the first corner, drag, release; Shift held before the release makes a square ([Sketching Corner Rectangles and Squares](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_Corner_Rectangles.htm)). The rectangle's page describes only that, but every 2D sketch tool takes click-drag or click-click, the first click deciding ([Sketch Modes](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Sketch_Modes.htm)), so two clicks draw one too.
- Other types: centre, 3-point corner, 3-point centre, parallelogram; centrelines can be added from the corners or the midpoints ([Rectangle PropertyManager](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_SKETCH_RECTANGLES.htm)).
- Circle: click the centre, then drag and click to set the radius ([Sketching Circles](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_Circles.htm)).
- Centrepoint arc: click the centre, drag to the radius, click the start, then click the end ([Sketching Centerpoint Arcs](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_Centerpoint_Arcs.htm)).
- Tangent arc: press on an endpoint of a line, arc, ellipse, or spline, drag, release ([Sketching Tangent Arcs](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_Tangent_Arcs.htm)).
- 3-point arc: click the start, click the end, drag to set the radius, click ([Sketching 3 Point Arcs](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_3_Point_Arcs.htm)).
- Point: each click places a point, and the tool stays active ([Sketching Points](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketching_Points.htm)).

### Trim, extend, split

- Power trim: drag across entities; each one crossed is trimmed to its nearest intersecting entity, and a trail marks the path ([Trimming with Power Trim](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Trimming_with_Power_Trim.htm), [Trim PropertyManager](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_TRIM_CHOICES.htm)).
- Power trim also extends: press on an entity and drag along its natural path; an arc reaching its maximum extension continues from its other end ([Extending with Power Trim](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Extending_with_Power_Trim.htm)).
- Trim to closest: a click removes the entity up to its closest intersection; a drag extends it in the drag's direction ([Trim PropertyManager](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_TRIM_CHOICES.htm)).
- *Keep trimmed entities as construction geometry* and *Ignore trimming of construction geometry* keep dimensional relations to the trimmed entities, and "might" keep geometric ones ([Trim PropertyManager](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_TRIM_CHOICES.htm)).
- The help doesn't say what a plain trim does to the trimmed entity's relations and dimensions, or whether the new end gets a relation to the entity it was trimmed at.
- Extend: hovering an entity previews it extended to the next entity; the half of the line or arc under the pointer picks the direction; a click accepts ([Extend Entities](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Extend_Entities.htm)).
- Split: a click on an entity splits it there into two, with a split point between them; a circle, a full ellipse, or a closed spline needs two split points; selecting the split point and pressing Delete joins the two again ([Split Entities](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_split_entities.htm)).

### Delete, and relations

- A relation is deleted by selecting its icon and pressing Delete, from the entity's PropertyManager, or in *Display/Delete Relations* ([Error Message - Item Conflicts](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDE_SKETCH_OVERDEFINED.htm)).
- *Display/Delete Relations* lists relations by filter (all, dangling, over defining/not solved, external, and others), deletes one or all, undoes its last change, and repairs all dangling relations in a 2D sketch ([Display/Delete Relations PropertyManager](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_SK_EDIT_RELATIONS.htm)).
- *Replace Entity* gives the new entity the old one's dimensions and relations; deleting an entity and drawing another usually means making the references again ([Replace Sketch Entities](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_replace_sketch_entities.htm)).
- The help doesn't say what deleting a sketch entity does to its relations and dimensions, or to the endpoint it shares with a neighbour.

### Sketch visibility

- A sketch a feature absorbs (an extrude's or a cut's) is hidden once the feature is made, and users report it can't be turned off; a sketch nothing absorbs shows. *(Unofficial: the help's *Hiding and Showing Sketches* page ([2022 help](https://help.solidworks.com/2022/english/SolidWorks/sldworks/t_hiding_and_showing_sketches.htm)) covers drawings only and doesn't state the rule. Reported in an Eng-Tips thread ([Eng-Tips, "How do I make sketch lines that are behind a solid not show?"](https://www.eng-tips.com/threads/how-do-i-make-sketch-lines-that-are-behind-a-solid-not-show.273813/)) and a Siemens community thread comparing NX with "how solidworks hides the sketch once u use it for an extrude or cut" ([Siemens Community, "Is there a way to make the sketches disappear after I create a feature?", 2024](https://community.sw.siemens.com/s/question/0D5Vb00000PePNGKA3/)).)*

## 5. Features

### The PropertyManager

- A feature's command opens its PropertyManager in the left panel. OK accepts, runs the command, and closes; Cancel ignores the selections and closes ([PropertyManager Overview](https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_pm_overview.htm)).
- Enter is OK and Esc is Cancel; right-click offers OK and Cancel; the Confirmation Corner in the graphics area holds OK and Cancel, and D moves them to the pointer and back ([Accepting Features](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_accepting_features.htm)).
- The active selection box is pink, and its items highlight in the graphics area ([PropertyManager Overview](https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_pm_overview.htm)).
- An extrude shows a shaded preview as soon as its PropertyManager opens; the view can be rotated, panned, and zoomed without losing it; dragging a handle shows a dynamic preview ([Shaded and Dynamic Previews](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_shaded_dynamic_previews.htm)).
- A slow preview of a supported command (combine, shell, patterns, fillets, chamfers) can be cancelled with Esc; the status bar says "Press <ESC> to cancel Preview" ([Interrupt Regeneration of Parts](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Interrupt_Regeneration_of_Parts.htm)).
- The help doesn't say what a PropertyManager shows when its preview fails (a fillet too large) before OK.
- The Fillet PropertyManager "remembers its last used state" ([Fillet PropertyManager, 2016](https://help.solidworks.com/2016/english/solidworks/sldworks/hidd_dve_fillet_dlg2.htm); [2019](https://help.solidworks.com/2019/English/SolidWorks/sldworks/c_fillet_propertymanager.htm); read 2026-11-04). The Chamfer PropertyManager's page says nothing of it ([Chamfer PropertyManager, 2021](https://help.solidworks.com/2021/English/SolidWorks/sldworks/r_chamfer_pm.htm)): Silico follows the fillet's rule for both, each opening with the values it was last OK'd with in the app's run (Cancel changes nothing; Edit Feature shows the feature's own). Neither page gives a first value: Silico's first radius and distance, 1 mm, are its own choice.

### Editing upstream

- *Edit Feature* on a feature, in the tree or the graphics area, opens its PropertyManager; OK applies the change ([Editing Features](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Editing_Features.htm)).
- *Edit Sketch* opens from the sketch, from a feature built on it, or from one of its entities ([Editing Sketches](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Editing_Sketches.htm)).
- *Exit Sketch* keeps the changes; the Confirmation Corner's Cancel or *Exit Sketch without Saving Changes* discards them ([Accepting Features](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_accepting_features.htm), [Exiting Sketches](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Exiting_Sketches.htm)).
- While a sketch is edited, a rebuild icon marks the part and the feature; exiting the sketch rebuilds the part automatically ([Status Bar](https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_status_bar.htm)).
- A child feature depends on its parent and can't be moved before it ([Parent and Child Relations](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Parent_and_Child_Relations.htm)).
- Suppressing a parent suppresses its children; unsuppressing the parent leaves the children suppressed; unsuppressing a child unsuppresses its parent ([Dependency Editing](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Dependency_Editing.htm)).
- The rollback bar shows the model as it stood at a feature (noted only; out of M5) ([Rollback Bar](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_rollback_bar.htm)).

### Rebuild

- Ctrl+B rebuilds the features changed since the last rebuild; Ctrl+Q (Force Rebuild) rebuilds every feature ([Rebuild](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_rebuild.htm), [Rebuild Tools](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_fundamentals_rebuild_tools_help.htm)).
- A rebuild icon in the tree marks features and parts a change has left needing a rebuild ([FeatureManager Design Tree Conventions](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_FeatureManager_Design_Tree_Conventions.htm)).
- Esc interrupts a rebuild: the current feature finishes and the rollback bar is placed after it ([Interrupt Regeneration of Parts](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_Interrupt_Regeneration_of_Parts.htm)).

### Errors and warnings

- Geometry is built when there are warnings, not when there are errors. Common errors: dangling dimensions or relations, and features that can't be built, such as a fillet too large ([What's Wrong?](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_NEW_WHATS_WRONG.htm)).
- Tree marks, four icons, each with where it appears ([What's Wrong?](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_NEW_WHATS_WRONG.htm)):
  - an error with the model: on the document's name, and on the feature that contains the error;
  - an error with a feature: on the feature's name;
  - a warning underneath the node: on the document's name and on the parent feature;
  - a warning with a feature or sketch: on that feature.
- So a mark on a node says either that the problem is its own or that it's passed up from beneath it. Items in error have red text and items with a warning green, and one combined icon shows both ([What's Wrong?](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_NEW_WHATS_WRONG.htm)).
- Selecting an error or warning in the tree or the graphics area shows a box describing it, with suggestions and links to *Edit Feature* or *Edit Sketch* (sometimes Preview and Help); a click on the tree's top node lists them all ([What's Wrong?](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_NEW_WHATS_WRONG.htm)).
- The *What's Wrong* dialog lists type, feature, preview, help, and description, sortable by column, with *Show errors* and *Show warnings* ([What's Wrong?](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_NEW_WHATS_WRONG.htm)).
- At the first rebuild error, by option: Stop (fix first; hovering a feature in the tree explains its error), Continue (rebuild the rest without asking), or Prompt ([Messages/Errors/Warnings](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_OPTIONS_ADVANCED.htm)).
- *What's Wrong* opens at every rebuild or only once, by option; saving a document with rebuild errors can open it first, to fix them before the save ([Messages/Errors/Warnings](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_OPTIONS_ADVANCED.htm)).
- Deleting a parent while keeping its children leaves their sketch planes dangling: a message offers to continue or to stop and repair; the part, the child, and its sketch show warnings; *Edit Sketch Plane* repairs it ([Sketch Plane References](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Sketch_Plane_References.htm)).
- A dangling dimension is repaired by dragging its handle onto an entity; a dangling relation by dragging the entity, by *Auto Repair*, or in *Display/Delete Relations*; the help advises repairing the first feature in error and working down ([Error Message - Dangling Geometry](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDE_SKETCH_EXTREFS_FAIL.htm)).
- The help doesn't say what a feature after a failed one is built on (the last good body, or nothing), or whether it fails too.

### Combine

Read from the 2027 help on 2026-11-08.

- In a multibody part, *Combine* (Features toolbar, or Insert > Features > Combine) opens the Combine PropertyManager, which adds, subtracts, or keeps the overlap of bodies of that one part ([Combine PropertyManager](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_FEAT_COMBINE_BODIES.htm), [Combine Bodies](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_combine_bodies.htm)).
- *Operation Type* is one of three ([Combine Bodies](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_combine_bodies.htm)):
  - *Add* combines the solids of all the selected bodies into a single body;
  - *Subtract* removes overlapping material from a selected main body;
  - *Common* removes all material except what overlaps.

  The PropertyManager's page names the third *Combine* where it lists them, and *Common* where it says which boxes show; the Combine Bodies and Combining Bodies - Common pages name it *Common*.
- *Add* and *Common* have one box, *Bodies to Combine* (*Solid Bodies*): the bodies to combine ([Combine PropertyManager](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_FEAT_COMBINE_BODIES.htm)).
- *Subtract* has two ([Combine PropertyManager](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_FEAT_COMBINE_BODIES.htm)):
  - *Main Body* (*Solid Body*): the one body to keep. *Make main body transparent* helps to select bodies to subtract that it hides.
  - *Bodies to Subtract* (*Solid Bodies*): the bodies whose material is removed.
- Each box takes bodies selected in the graphics area or in the *Solid Bodies* folder of the FeatureManager design tree ([Combine PropertyManager](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_FEAT_COMBINE_BODIES.htm)).
- The steps for each operation: choose the operation, fill its boxes (for Subtract, *Main Body* first), click *Show Preview*, then OK ([Combining Bodies - Add](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_combining_bodies_add.htm), [Combining Bodies - Subtract](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_combining_bodies_subtract.htm), [Combining Bodies - Common](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_combining_bodies_common.htm)).
- The help doesn't say:
  - whether bodies, or faces, selected before the command fill its boxes;
  - whether *Main Body* hands the focus on to *Bodies to Subtract* once it holds its body;
  - what a click on a body's face or edge puts in a box;
  - whether one body may be in both of Subtract's boxes;
  - what the PropertyManager shows when the result is empty or changes nothing.

Silico's booleans are binary: `fuse`, `cut`, and `common` each take one `base` and one `tool`, and consume both. So Silico's dialog has two boxes of one body each, its target and its tool, for all three operations, as Subtract's *Main Body* and one body to subtract. A pick in a box replaces its body, as a box of one entity does. The same body in both boxes is refused. Bodies selected in the browser fill the boxes when the dialog opens, the first the target, the second the tool; a selected face, edge, or vertex doesn't, since the help doesn't say it does. A click on a body's face, edge, or vertex takes that body. Once the target is taken, the tool's box becomes the active one if it's empty, as Fusion's Combine moves from its Target Body to its Tool Bodies (docs/ux/fusion.md, *Combine*). The help names none of these rules, so they're Silico's own.

## 6. Undo, saving, and changes on disk

- Ctrl+Z undoes the last action "when possible"; the Undo List undoes a chosen action and every one after it; Ctrl+Y redoes, when the undo can be reversed ([Basic Commands](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_basic_commands.htm)).
- In a large sketch, past the set number of entities, Undo turns off with Automatic Solve ([Using Automatic Solve](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_Using_Automatic_Solve.htm)).
- Undo on the Standard toolbar restores a sketch after SketchXpert's *Delete All*, which SketchXpert's own undo can't ([SketchXpert PropertyManager](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_DVE_SKETCH_XPERT.htm)).
- The help doesn't list what is undoable, or say whether undo reaches across a save, a rebuild, leaving a sketch, or an OK in a PropertyManager.
- An asterisk next to a document's name means it has changed since it was last saved ([Document Windows](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_document_windows.htm)).
- Ctrl+S saves; a new document asks for a name first ([Save](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_file_save.htm)).
- Ctrl+W closes; closing a new or changed document asks whether to save it ([Closing Documents](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_closing_documents.htm)).
- Closing several changed documents lists them to save all, save some, or discard all, marking those another user saved during the session ([Saving Modified Documents](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_saving_modified_documents.htm)).
- A reminder appears when a document hasn't been saved for a set number of minutes ([Messages/Errors/Warnings](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_OPTIONS_ADVANCED.htm)).
- Auto-recover writes a recovery file every set number of minutes, deleted when the document is saved; backup keeps the version before the last save ([Backup/Recover/Version Options](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_OPTIONS_BACKUPS.htm)).
- *Reload* loads documents again from disk, and can undo changes since the last save; its list shows *Needs Save?* and *Newer Version On Disk?*, and marks in red a reload that would lose unsaved changes ([Reload](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_FILE_RELOAD_MULTIPLE.htm)).
- With the multi-user environment on, files opened read-only are checked every set number of minutes; when another user has saved one, a tooltip points to a status-bar icon that opens *Reload* ([Collaboration Options](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_OPTIONS_COLLABORATION.htm), [Reload](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_FILE_RELOAD_MULTIPLE.htm)).
- Starting to change a read-only document warns that it's read-only ([Multi-User Environment](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_multi_user_environment.htm)).
- With the multi-user environment on, one user holds write access at a time: a document someone else has open for writing opens read-only, its tooltip naming that user, and *Get Write Access* or *Make Read-Only* changes it ([Multi-User Environment](https://help.solidworks.com/2027/english/SolidWorks/sldworks/c_multi_user_environment.htm)). When the writer gives up write access, *Reload* can take it ([Reload](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_FILE_RELOAD_MULTIPLE.htm)).
- Beyond its own multi-user locking, the help doesn't say what happens when a document open for writing is changed on disk by another program.

## 7. Measure

Read on 2026-11-08 for M5.7's selection view; the 2026 help says the same.

- Measure measures distance, angle, and radius, and the size of and between lines, points, surfaces, and planes, in parts, assemblies, sketches, and drawings ([Measure Tool](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_MEASURE.htm)).
- One entity selected is measured on its own; a vertex or sketch point gives its x, y, and z coordinates ([Measure Tool](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_MEASURE.htm), [Using the Measure Tool](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_using_the_measure_tool.htm)).
- Two entities selected give the smallest distance between them ([Using the Measure Tool](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_using_the_measure_tool.htm)). For two arcs or circles, *Arc/Circle Measurements* picks the distance shown: center to center, minimum, maximum, or a custom one ([Measure Tool](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_MEASURE.htm)). The help doesn't say which is the default.
- *Projected On* a face or plane adds the distance projected on it and the distance normal to it; *Show XYZ Measurements* adds dX, dY, and dZ between the entities ([Measure Tool](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_MEASURE.htm)).
- The dialog lists up to six selections. Clicking a selected item again in the graphics area removes it, and clicking a blank area clears them all ([Measure Tool](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_MEASURE.htm), [Using the Measure Tool](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_using_the_measure_tool.htm)).
- Measure updates as the selection changes, even across documents and while other tools run ([Using the Measure Tool](https://help.solidworks.com/2027/english/SolidWorks/sldworks/t_using_the_measure_tool.htm)).
- With Measure closed, the status bar shows common measurements of what's selected, such as an edge's length ([Measure Tool](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_MEASURE.htm), [Status Bar](https://help.solidworks.com/2027/english/SolidWorks/sldworks/r_status_bar.htm)).
- *Quick Copy* copies a value from the dialog, with or without its units ([Measure Tool](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_MEASURE.htm)).
- The help doesn't say which measures a pair of entities gets (when an angle is given, or a normal distance without *Projected On*), nor what Measure shows for a pair it can't relate.

## Not covered

What this note looked for and couldn't source from the help:

- Whether a plain click replaces the selection, whether a Ctrl-click deselects, and how a graphics-area selection is cleared (empty-space click, Esc).
- Whether the filter shortcuts (E, X, V) add to the active filters or replace them.
- Which inferencing lines and snaps leave a relation (parallel, perpendicular, a model edge) and which only guide; the inference lines' colours (only an unofficial source).
- Which relations each tool adds (the rectangle's, only from an unofficial source; a click-click chain's shared endpoints).
- What a drag's release keeps (a relation from dropping on an entity; a position the solver can't reach); dragging fully defined geometry.
- What trim does to relations and dimensions, and what deleting an entity does to its relations and to a shared endpoint.
- What a PropertyManager shows when its preview fails.
- Combine: whether a selection made before it fills its boxes, whether its boxes hand the focus on, and what a click on a body's face puts in one (§5, *Combine*).
- What a feature downstream of a failed one is built on.
- What is undoable, and whether undo crosses a save, a rebuild, leaving a sketch, or a feature's OK.
- What happens when a document open for writing changes on disk, beyond SolidWorks' own one-writer locking.
- Dimensioning: Smart Dimension's type chosen from the pick, the Modify box, Enter and Esc, a value driving the geometry. Not read for this note; read from the help for M5.11 in docs/study/smart-dimension.md, which the decisions SD1 to SD9 (M5.0, section 9) take.
- Starting a sketch beyond the view: which planes and faces may be picked. (*Auto-rotate view normal to sketch plane on sketch creation and sketch edit* is an option, and the view reverts if the changes are discarded: [Sketch Options](https://help.solidworks.com/2027/english/SolidWorks/sldworks/HIDD_OPTIONS_SKETCH.htm).)
- The export dialog (M5.12), what happens to a selection when a rebuild removes its entity (M5.7), and the pick tolerance.
- Whether a sketch a feature absorbs is hidden (only from unofficial sources: §4, *Sketch visibility*).
- Relations added by hand: which entity moves; which point a merge keeps; whether a relation already present is offered again; what Symmetric holds of two lines' ends or two arcs' radii; what Midpoint does with two lines; what the context toolbar offers for each selection.

The draft M5 plan's corner rectangle (M5.11) and suppression (M5.1) follow §4 and §5 above.
