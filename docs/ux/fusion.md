# Fusion's layout and look

A study note for M5's design (M5.0): how Autodesk Fusion lays out its window and how it looks, described in Silico's words so the app's layout can be built from it, never copied (no screenshots, no Autodesk icon art). It records layout and look only; behavior follows SolidWorks (docs/ux/solidworks.md), as CLAUDE.md says. It was read on 2026-10-27 from Autodesk's Fusion help (help.autodesk.com, unversioned and kept current; its latest release notes are for build v.2706.1.14, 2026-10-20 [rel26]) and Autodesk's own blog, support, and brand pages. Each line cites its source. Where the help is silent or sources disagree, the line says so. Blogs and forums by people outside Autodesk are marked *unofficial*. The last section, kept apart, lists what `re_ui` 0.38.1, egui 0.36.2, and `egui_tiles` 0.17.1 offer for each element; it says nothing about Fusion.

"Diagram" below means the annotated interface map on [iface] (a 2026 screenshot with numbered regions). Positions read from it are marked *(diagram)*.

## The window

- Regions: Data Panel, Application Bar, Toolbar, Browser, canvas, ViewCube, Marking Menu, Navigation Bar, Timeline, and the Autodesk Assistant panel [iface].
- Application Bar, left to right: Home, Data Panel toggle, File menu, Save, Undo/Redo, open design tabs, New Design, Assistant, Extensions, Job Status, Notification Center, Help, My Profile [iface]. It spans the top of the window *(diagram)*; My Profile is at its top right [prefs].
- Toolbar: directly under the Application Bar, full width *(diagram)*.
- Browser: a panel at the top left of the canvas, titled BROWSER, with a collapse control *(diagram)*. A COMMENTS bar sits at the canvas's bottom left *(diagram)*.
- Canvas: the rest of the window; objects are selected by clicking them there [iface].
- Timeline: a bar across the bottom of the window, under the canvas *(diagram)*.
- Data Panel (note only): opens on the left of the application, toggled from the Application Bar [basics] [iface]. Out of M5 (see *Not covered*).
- Assistant panel: on the right of the canvas *(diagram)*. Out of M5.
- Marking Menu: right-click in the canvas gives a wheel of frequent commands and an overflow menu [iface].
- Dialogs and toolbars dock by dragging them over a window edge until a vertical green line appears, and undock by dragging away [dock].

## The toolbar

- Workspace menu at its left end (DESIGN in the diagram); the workspace decides which commands and data types are available [iface] [ws]. Workspaces: Design, Electronics, Generative Design, Render, Animation, Simulation, Manufacture, Drawing [ws]. M5 needs Design only.
- Tabs group the workspace's tools [iface]. Design's tabs: SOLID, SURFACE, MESH, SHEET METAL, PLASTIC, MANAGE, UTILITIES *(diagram)*.
- Contextual tabs (Sketch) appear beside the others while their command is active; contextual environments (Form) replace the tabs until exited [iface].
- Solid tab panels: CREATE, MODIFY, ASSEMBLE, CONFIGURE, CONSTRUCT, INSPECT, INSERT, SELECT *(diagram)*. The help names the Create [sfs], Modify [modify], Construct [construct], Inspect [inspect], Insert [insert], and Select [sel] panels.
- A panel shows large icons in a row with its name under them and a drop-down arrow beside the name; labels are in capitals *(diagram)*. The help gives no sizes.

## The browser

- The tree's root is the design's default component; it holds Document Settings (units), Named Views (standard and custom views), Origin (origin point, X/Y/Z axes, XY/XZ/YZ planes), Analysis, Joint Origins, Relationships, Bodies, Sketches, and Construction folders [browser].
- Each row: an expand arrow, a visibility icon, the object's type icon, its name; the root also has an activation radio button [browser] *(diagram)*.
- Visibility icon: an eye, and an eye struck through when hidden (the help's own 16 px icons, `visible.png` and `hidden.png`) [vis]. Clicking it toggles the object in browser and canvas; `V` toggles the selection's visibility [vis]. The API docs still call it a "light bulb" [apivis]: older wording.
- Hidden categories set in the Navigation Bar's Object Visibility override the browser's eyes [vis].
- A fully constrained sketch shows a red lock on its icon [fully].
- *Auto hide sketch on feature creation*, Preferences > General > Design, on by default: a sketch is hidden when a feature is created from it, and stays visible with the setting off. *(Autodesk support [autohide] [sketchinvis]; the help itself wasn't found to state it.)*
- Ordering: the browser groups objects by type in folders. It isn't the feature history: in Parametric Modeling mode the history is the Timeline; in Direct Modeling mode features such as Extrude, Fillet, and Hole are listed in the browser instead [modes]. CLAUDE.md's rule settles this for Silico, since it's layout: the browser groups by type as Fusion's does, and the timeline holds the features (M5.4, docs/roadmap/m5-app.md). A failed feature's mark is error feedback, so it behaves as SolidWorks' does (docs/ux/solidworks.md), drawn with Fusion's look on its timeline entry and its browser entry.

## The timeline

- Records each feature, in order, in Parametric Modeling mode only [modes]. One icon per feature, left to right *(diagram)*. The help has no table of the icons; forum users say so too (*unofficial*, [fticons]).
- Playback buttons at its left: Move to Beginning, Previous Step, Play, Next Step, Move to End [tl]. They move the Timeline Marker, which is out of M5 (the plan's Study section).
- Options (a settings control) at its right end [tl] *(diagram)*.
- Double-click a feature, or right-click and Edit Feature, to edit it [tl]. Right-click also gives Rename, Suppress/Unsuppress, Create Group, Find in Browser, Create Selection Set [tl].
- Suppressed features are dimmed [tl]. Drag reorders features [tl] [iface].
- A small mark above a feature's icon means geometry it made is selected; the browser marks a node the same way (Autodesk's Engineering Director in the forum, not the help: [fticons]).
- Health: each item is Healthy, Warning, Error, Suppressed, Rolled Back, or Unknown, with a message for Warning and Error [health]. An Autodesk blog post speaks of "red or yellow problems": errors fail to compute, warnings still show but have lost references [errblog]. The help doesn't say how a mark is drawn (fill, outline, or badge).

## Feature dialogs

- A command dialog is a vertical stack of inputs with OK and CANCEL at the bottom [cmds].
- Position and width are shared by all commands: the next dialog opens where the user left the last one; height follows the inputs [cmds] (also an Autodesk API expert, *unofficial*: [fpos]). Per session only: a moved dialog goes back to the right when Fusion restarts (*unofficial*, [fside]).
- Default side: the help doesn't say. Forum users say dialogs open on the right; a reply from Autodesk staff in the same thread says the left (forum, conflicting: [fside]).
- OK stays disabled until the inputs are valid; an unparseable value turns red and disables OK [cmds] [inputs].
- Preview: most commands preview their result; each input change discards the last preview and builds it again [cmds].
- Field types [inputs]: value inputs with units (length, angle), with a drop-down of previous values and a Measure option; selection boxes; drop-downs (checkbox list, labelled icons, text list; Extrude's Direction, Operation, and Extents are labelled-icon drop-downs); checkboxes and toggle buttons; button rows; radio groups; sliders; spinners; collapsible groups; tabs; tables (Loft's profile list); text boxes.
- Labels sit on the left, controls on the right; an input may take the full width [inputs].
- Example, Extrude: Type, Profiles, Start, Direction, Extent Type, Distance, Taper Angle, Operation, Objects To Cut [extref]. A sketch with one visible profile is selected at once [extref].
- Example, Fillet: Type, then one row per selection set (edges, faces, or features) with its Radius Type and radius; `+` adds a set [fillet].
- In-canvas manipulators: a distance input shows an arrow in the canvas that drags its value; an angle input shows a draggable widget; a direction input shows an arrow that flips; a triad moves, rotates, and scales [inputs]. Fillet shows radius handles [fillet]. A manipulator hides while its input is hidden or disabled [inputs].
- In-canvas value boxes beside a manipulator: not described in Fusion's help. (Inventor's help describes such a box; that isn't Fusion.)

## Combine

Read on 2026-11-08 from [combine] and [combref].

- Design > Solid > Modify > Combine opens the Combine dialog. The steps: select the Target Body in the canvas, then the Tool Bodies, then choose the Operation, then OK [combine].
- Its inputs, in order [combref]:
  - **Target Body**: the body being acted on.
  - **Tool Bodies**: the bodies that change it.
  - **Operation**:
    - Join: one solid body of them;
    - Cut: the Tool Bodies' volume removed from the Target Body;
    - Intersect: the overlapping volume kept, as one body [combine].
  - **Objects To Cut**: offered when a cut is edited, to cut the bodies chosen when it was made or the bodies visible now.
  - **New Component**.
  - **Keep Tools**: keeps the Tool Bodies after the combine [combine].
- If the result is hidden behind the tool bodies in the canvas, hide those in the browser [combine].
- The help doesn't say whether a body selected before the command fills the Target Body, nor whether Tool Bodies becomes the active box once the Target Body is chosen. Its steps take them in that order.

Silico's app follows its words: the box labels Target Body and Tool Body, the second singular since each of Silico's booleans takes one tool (docs/ux/solidworks.md, *Combine*). Its commands are the prototype's Union, Subtract, and Intersect, one button each, in place of an Operation choice. It has no Keep Tools, since Silico's booleans always consume both bodies, and no New Component.

## ViewCube and Navigation Bar

- ViewCube: a cube in a corner of the canvas [cube], the top right *(diagram)* [pzo].
- ViewCube clicks: drag to orbit; a corner gives an isometric view; a named face (Front, Top, Right) an orthographic view; the Home icon the home view [cube].
- ViewCube context menu: Go Home; Camera (Orthographic, Perspective, Perspective with Ortho Faces); Set current view as Home (Fit to View, Fixed Distance); Reset Home; Set current view as [Face]; Reset [Face]; ViewCube Size (Automatic, Large, Normal, Small, Tiny) [cube].
- Navigation Bar: a floating toolbar at the bottom of the canvas [nav], centred *(diagram)*.
- Its controls, in the help's order [nav]: Orbit (Free, Constrained; shows the last used), Look At (selected face, edge, or line parallel to the screen), Pan, Zoom, Fit / Zoom Window (shows the last used), Display Settings, Grid and Snaps, Viewports.
- Display Settings [nav]: Visual Style (Shaded; Shaded with Hidden Edges; Shaded with Visible Edges Only; Wireframe; Wireframe with Hidden Edges; Wireframe with Visible Edges Only), Environment, Graphics Preset, Effects, Object Visibility, Camera (the three above), Ground Plane Offset, full screen.
- Projection default (orthographic or perspective): not documented. Forum users report no setting for it and the camera reverting to Perspective (*unofficial*: [camforum]).

## Navigation defaults

- Preset: Preferences, General, "Mouse setup (pan, zoom, orbit)": Fusion (default), Alias, Inventor, PowerMill, SolidWorks, Tinkercad [prefs]. A 2024 Autodesk blog post calls it "Pan, Zoom, Orbit Shortcuts" [pzo].
- Fusion preset: zoom rolls the wheel (or Ctrl+Shift+middle drag); pan holds the middle button; orbit holds Shift and the middle button [prefs] [keys]. Orbit about a point: Shift+click+middle button [keys].
- SolidWorks preset as Fusion lists it: zoom Shift+wheel, pan Ctrl+middle (Cmd on macOS), orbit middle [prefs]. Navigation follows Fusion (M5.6); SolidWorks' own defaults aren't recorded.
- F6 fits the view [prefs]; double-clicking the wheel zooms to fit [pzo].
- Options named in the blog: reverse zoom direction, camera pivot (window centre or the part's focal point), trackpad gestures [pzo]. Whether zoom goes toward the cursor is not documented.
- Default modeling orientation: Z up; Y up is an option needing a restart [prefs].
- "Animate view transitions" animates named-view and sketch-view changes [prefs].

## Sketch mode

- Enter: Solid tab, Create Sketch, then pick an origin plane, a work plane, or a planar face [csk]. Edit: right-click the sketch in the browser or timeline, Edit Sketch [esk].
- The Sketch contextual tab and its Finish Sketch button are highlighted blue while the sketch is open [esk]. Its panels include Create (Sketch Dimension) [dims] and Constraints [cons]; constraints are visible by default [esk].
- Finish: Finish Sketch on the Sketch tab, or Marking Menu, Sketch, Finish Sketch [fin].
- Sketch Palette: a floating palette in the canvas [pal]. Top: Status: Under-Constrained or Status: Constrained [under]. Then the active tool's options, then: Linetype (Construction, Centerline), Look At, Sketch Grid, Snap, Slice, Show Profile, Show Points, Show Dimensions, Show Constraints, Show Construction Geometries, Show Projected Geometries, 3D Sketch [pal]; Find Under-Constrained, Compute Sketch, AutoConstrain [under].
- Colours: new geometry is blue, centerlines orange; fully constrained geometry turns black [fully] [dims]. Closed profiles have a blue shading (Show Profile) [pal].
- Points: an unconstrained end point is a white dot; spline fit points are grey; a point-to-point coincidence shows nothing; a point-to-curve coincidence shows the coincident glyph [coinc].
- Constraint glyphs: small icons beside the constrained curves (Horizontal/Vertical, Coincident, Tangent, Equal, Parallel, Perpendicular, Fix, Midpoint, Concentric, Collinear, Symmetry, Curvature) [cons] [fully] *(the example image)*. Their art isn't copied here.
- Dimensions: drawn in the canvas, with arrows and extension lines *(the example image)*; double-click to edit; Enter accepts [dims]. Driven dimensions are toggled from the right-click menu [dims].
- Over-constrained colour: not described in Fusion's help. (M5.11 colours by SolidWorks' states anyway.)

## Selection look

- Pre-highlight: entities valid for selection pre-highlight under the cursor [cmds]. Colours: not published. The style is a setting: "Selection display style" (Preferences, Graphics) and Selection Effect: Normal, Fast, Simplified, None (Display Settings) [prefs] [nav].
Lines below marked *behavior* are Fusion's, kept for reference only: selection behaves as SolidWorks' does (docs/ux/solidworks.md, §1), and where they differ, SolidWorks' governs.

- *Behavior.* Window selection is the default mode; others are Freeform (2) and Paint (3) [sel].
- *Behavior.* Window: drag left to right takes objects wholly inside; right to left takes objects inside or crossed. The area inside fills with a translucent colour [sel].
- *Behavior.* Freeform: [sel] says clockwise is window and counterclockwise crossing; [selmodes] says left-to-right and right-to-left, as for the rectangle. Conflict, unresolved.
- *Behavior.* Ctrl (Cmd) adds or removes; Shift only adds [sel]. Click and hold opens a depth list of the objects under the cursor [selobj].
- *Behavior.* Filters: Select, Selection Filters, a checklist (Bodies, Body Edges, Body Faces, Body Vertices, Sketch Curves, Sketch Points, Sketch Profiles, Work Geometry, and more) with Select Through [selfilt]. Priorities: Body, Face, Edge, Component, one at a time [selprio].
- The Select icon on the toolbar shows the current mode [selmodes].

## The look, as Autodesk publishes it

- Themes: Dark Blue, Light Gray, Fusion Classic, and Match Device Theme (September 2025) [rel25]; "Match OS" became the default in November 2025 [rel25] [themes]. Classic is to retire once the new themes match it [themes].
- Font: Autodesk's brand hub says its proprietary Artifakt "should also be used inside our product interfaces" [type]. It's licensed to Autodesk's employees and suppliers only [type], so Silico can't ship it. Fusion's help doesn't name its UI font.
- Colours, sizes, spacing, and density: not published. No help page, release note, or blog post read gives a hex value, panel width, row height, or font size.
- Autodesk's design system, Weave, publishes theme data under Apache-2.0 [weave]. No Autodesk page says Fusion's themes are that data, so this note takes no values from it.

## What `re_ui` 0.38.1 and egui 0.36.2 offer

Read from Rerun's source at tag `0.38.1` (`crates/viewer_support/re_ui`: `src/design_tokens.rs`, `src/lib.rs`, `src/list_item/list_item.rs`, `data/{dark,light}_theme.ron`, `data/color_table.ron`), egui 0.36.2 (`src/style.rs`, `src/containers/panel.rs`, `src/containers/frame.rs`, `src/memory/theme.rs`), and `egui_tiles` 0.17.1 (`src/behavior.rs`). Colours are the RON aliases resolved through the colour table, dark / light. Rows marked *re_viewer* are Rerun's viewer's own choices, not `re_ui`'s. A setting set by `re_ui` lands through `re_ui::apply_style_and_install_loaders`, which writes both themes' `egui::Style`.

| Fusion element | Setting | Value |
|---|---|---|
| Theme, Match OS default | `egui::Options::theme_preference` | `ThemePreference::System` by default; `re_ui` sets `fallback_theme = Dark` |
| UI font | `DesignTokens::set_fonts` (the family) and `set_text_styles` (the sizes), both private, applied by `apply_style_and_install_loaders` | Inter Medium (bundled, OFL-1.1); Body, Button, Monospace 12 px; Heading 16 px; token line height 16 px |
| Application bar | `DesignTokens::top_bar_height()`, `top_bar_margin()`, `top_panel_frame()`; `top_bar_color` | 28; 8 x 0; `#0d0d0d` / `#e0dfdf` |
| Toolbar buttons | `large_button_size`, `large_button_icon_size`, `small_icon_size` | 22, 12, 14 (no ribbon widget in `re_ui`) |
| Browser panel | `egui::Panel::left(..)`: `default_size`, `min_size`, `resizable` | egui: 200, 96, resizable; *re_viewer* blueprint panel: `min(0.35 w, 200)`, min 120 |
| Browser rows | `re_ui::list_item::ListItem`; `DesignTokens::list_item_height()` | 24 |
| Tree indent | `ListItem` children: `small_icon_size.x + text_to_icon_padding()`; `spacing.indent` | 14 + 4 = 18; `re_ui` sets `indent` 14 (egui 18) |
| Row hover / selected | `list_item_hovered_bg`; `visuals.selection.bg_fill` | `#272626` / `#eeeeed`; `#186ADD` both |
| Panel title (BROWSER) | `title_bar_height()`, `list_header_font_size()` | 24, 11 |
| Visibility eye | `re_ui::icons::VISIBLE`, `INVISIBLE` | Rerun's own SVGs |
| Timeline bar | `egui::Panel::bottom(..)`; `bottom_panel_frame()`, `bottom_bar_color` | egui: not resizable, min 20; margin 8 x 0; `#171717` / `#ffffff`; *re_viewer* time panel: collapsed 29, expanded `clamp(0.25 h, 150, 250)` |
| Error / warning marks | `visuals.error_fg_color`, `warn_fg_color`; `icons::ERROR`, `WARNING` | `#AB0116`, `#FF7A0C` both |
| Docked dialog | `egui::Panel::right(..)` | egui: 200; *re_viewer* selection panel: `min(0.45 w, 300)`, min 120, max 0.65 w |
| Floating dialog | `egui::Window` (`anchor`, `resizable`, `collapsible`); `window_fill`, `window_corner_radius`, `window_shadow` | `floating_color` `#212121` / `#ffffff`; 6; offset 0,15, blur 50 |
| Dockable panes | `egui_tiles::Behavior`: `tab_bar_height`, `gap_width`, `min_size` | 24, 1, 32 |
| Dialog rows | `re_ui::list_item::PropertyContent` (label left, value right); `re_ui::re_form` | as list items, 24 high |
| Value fields | `spacing.interact_size.y`; `visuals.text_edit_bg_color` | 15 (egui 18); `#212121` / `#eeeeed` |
| Invalid value | `visuals.error_fg_color` | `#AB0116` |
| OK / Cancel | `DesignTokens::button_primary`, `button_secondary`; `modal_button_width` | 50 |
| Item spacing | `spacing.item_spacing`; `button_padding`; `menu_spacing` | 8 x 8 (egui 8 x 3); 1 x 0 (egui 4 x 1); 1 (egui 2) |
| Margins | `view_padding()`, `panel_margin()`; egui `Frame::side_top_panel` | 12; 12 x 0; 8 x 2 |
| Corners | `small_corner_radius()`, `normal_corner_radius()` | 4, 6 |
| UI selection colour | `visuals.selection.bg_fill`; `selection.stroke` | `#186ADD`; `#f0f2ff`, width 2 (both themes) |
| Viewport hover / selection outline | `re_view::outline_config`: hover from `widgets.active.fg_stroke`, selection from `selection.stroke` | `text_strong` (`#ffffff` / `#000000`), width 2; `#f0f2ff`, width 2 |
| Window-select fill | nearest token `highlight_color` (no direct match) | `#186ADD` at alpha 120 / `#c2ccff` |
| Canvas background | `viewport_background` | `#000000` / `#ffffff` |
| Sketch axes (red X, green Y in [fully]'s image) | `axis_color_x`, `axis_color_y`, `axis_color_z` | `#ff6981`, `#00b266`, `#7a95ff` / `#ff3865`, `#009e5a`, `#5a81ff` |
| Scroll bars, tooltips | `spacing.scroll.bar_width`; `tooltip_width` | 6; 600 |

## Conflicts and gaps

- Freeform selection direction: clockwise/counterclockwise [sel] against left/right [selmodes].
- Shift on a click: Fusion's adds to the selection [sel]; SolidWorks' reverses selection through transparency, and Shift with a box adds (docs/ux/solidworks.md, §1). SolidWorks governs; M5.7 takes it from there. Fusion's Ctrl-click removing an item fills a gap SolidWorks' help leaves, for M5.0 to decide.
- Dialogs' default side: right (forum users) against left (one forum reply) [fside]; the help is silent.
- The visibility icon: an eye in the help [vis], a "light bulb" in the API docs [apivis].
- Not documented: projection default, zoom toward the cursor, selection and pre-highlight colours, the timeline's error and warning drawing, an over-constrained sketch colour, in-canvas value boxes, and every size, colour, and spacing value.

## Not covered

- The Timeline Marker and rolling back (out of M5, per the plan).
- The Data Panel, Fusion Home, the web client, hubs, projects, comments, and collaboration badges.
- The Autodesk Assistant panel and the Marking Menu's contents.
- Workspaces other than Design, and tabs other than Solid and Sketch.
- Assemblies: components, joints, activation, Edit In Place.
- Render environments, visual effects, multiple viewports, the grid's look.
- Keyboard shortcuts beyond navigation.
- Behavior of any tool (docs/ux/solidworks.md).

[iface]: https://help.autodesk.com/cloudhelp/ENU/Fusion-GetStarted/files/GS-THE-FUSION-INTERFACE.htm
[basics]: https://help.autodesk.com/cloudhelp/ENU/Fusion-GetStarted/files/GS-BASIC-OPERATIONS.htm
[ws]: https://help.autodesk.com/cloudhelp/ENU/Fusion-GetStarted/files/GS-WORKSPACES.htm
[prefs]: https://help.autodesk.com/cloudhelp/ENU/Fusion-GetStarted/files/GUID-878489CD-3A23-4303-8450-C2F4F8E410B1.htm
[keys]: https://help.autodesk.com/cloudhelp/ENU/Fusion-GetStarted/files/GUID-F0491540-0324-470A-B651-2238D0EFAC30.htm
[nav]: https://help.autodesk.com/cloudhelp/ENU/Fusion-GetStarted/files/GS-NAVIGATION-BAR.htm
[cube]: https://help.autodesk.com/cloudhelp/ENU/Fusion-GetStarted/files/GS-VIEWCUBE.htm
[browser]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Designs/files/GUID-38F2A3B4-CE01-4375-A21D-4CC9A743B2A1.htm
[vis]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Designs/files/ASM-VISIBILITY.htm
[modes]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Designs/files/ASM-DESIGN-MODELING-MODES.htm
[tl]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Designs/files/ASM-USE-TIMELINE.htm
[sfs]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Model/files/SLD-CREATE-SOLID-FROM-SKETCH.htm
[modify]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Model/files/SLD-MODIFY-SOLID-BODY.htm
[construct]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Model/files/SLD-CONSTRUCT-TOOLS.htm
[inspect]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Model/files/SLD-INSPECT-TOOLS.htm
[insert]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Model/files/SLD-INSERT-TOOLS.htm
[extref]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Model/files/SLD-REF-EXTRUDE.htm
[fillet]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Model/files/SLD-FILLET-SOLID.htm
[combine]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Model/files/SLD-COMBINE.htm
[combref]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Model/files/SLD-REF-COMBINE.htm
[sel]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Model/files/SLD-SELECTION.htm
[selobj]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Model/files/SLD-SELECT-OBJECTS.htm
[selmodes]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Model/files/SLD-SELECTION-MODES.htm
[selfilt]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Model/files/SLD-SELECTION-FILTERS.htm
[selprio]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Model/files/SLD-SELECTION-PRIORITY-FILTERS.htm
[csk]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-CREATE-3D-SKETCH.htm
[esk]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/GUID-0EEF7073-6CDE-4E31-AF1A-0811F969F031.htm
[fin]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-FINISH-SKETCH.htm
[pal]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/GUID-4183A4B7-E002-4396-AD5A-7FF3C8B2F33A.htm
[under]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-FIND-UNDERCONSTRAINED.htm
[fully]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-FULLY-DEFINE-CONSTRAIN-SKETCH.htm
[coinc]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/GUID-685B9D79-E3BC-4EB8-AA4E-096A34E94249.htm
[cons]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-CONSTRAINTS.htm
[dims]: https://help.autodesk.com/cloudhelp/ENU/Fusion-Sketch/files/SKT-CREATE-DIMENSIONS.htm
[cmds]: https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/Commands_UM.htm
[inputs]: https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/CommandInputs_UM.htm
[health]: https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_FeatureHealthStates.htm
[apivis]: https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/fusion_Sketch_isVisible.htm
[rel26]: https://help.autodesk.com/cloudhelp/ENU/Fusion-ReleaseNotes/files/FIC-REL-NOTES-INC.htm
[rel25]: https://help.autodesk.com/cloudhelp/ENU/Fusion-ReleaseNotes/files/FIC-REL-NOTES-2025.htm
[dock]: https://www.autodesk.com/support/technical/article/caas/sfdcarticles/sfdcarticles/How-to-dock-undock-toolbars-and-dialog-boxes-in-Fusion-360.html
[autohide]: https://www.autodesk.com/support/technical/article/caas/sfdcarticles/sfdcarticles/How-to-adjust-automatic-visibility-of-sketch-in-Fusion-360.html
[sketchinvis]: https://www.autodesk.com/support/technical/article/caas/sfdcarticles/sfdcarticles/Sketch-becomes-invisible-after-extruding-it-in-Fusion-3560.html
[pzo]: https://www.autodesk.com/products/fusion-360/blog/quick-tip-pan-zoom-orbit-preferences/
[errblog]: https://www.autodesk.com/products/fusion-360/blog/quick-tip-fixing-errors-in-your-design/
[themes]: https://www.autodesk.com/products/fusion-360/blog/a-fresh-look-for-fusion-dark-and-light-mode-are-now-here/
[type]: https://brand.autodesk.com/visual-system/typography/
[weave]: https://github.com/Autodesk/hig
[fticons]: https://forums.autodesk.com/t5/fusion-support-forum/timeline-icons/td-p/10938599
[fpos]: https://forums.autodesk.com/t5/fusion-api-and-scripts-forum/the-way-to-control-command-dialog-positioning/m-p/12956032
[fside]: https://forums.autodesk.com/t5/fusion-design-validate-document/how-to-pin-menus-to-a-new-location/td-p/13152904
[camforum]: https://forums.autodesk.com/t5/fusion-design-validate-document/default-camera-view-keeps-switching-to-quot-perspective-quot/m-p/12740420
