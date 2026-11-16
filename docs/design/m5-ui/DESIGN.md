# Silico UI prototype: design notes

A clickable prototype of Silico's M5 app: open `index.html` directly (no build step, no network; three.js r149 and Inter are vendored). Use the **Prototype ▾** pill at the top right to jump between the states under review, or just use the app: almost every control does what it would do.

It follows docs/design/m5.0-app.md (§3, §4, §8, §9, §12) and the two UX studies (docs/ux/fusion.md, docs/ux/solidworks.md). Where the prototype goes past the approved design, this note says so under *Proposals beyond M5.0* and *Open questions*.

## The idea in one line

**The canvas is the part, and the timeline is the rebuild.** Everything else is quiet Rerun chrome around those two. A rebuild runs visibly *along the timeline*: each feature's icon spins while its step runs, dims while queued, and turns red if it fails. The status bar repeats the same thing in words, with Stop beside it. An agent's edit, which arrives through the file, gets its own colour (the info tone, re_ui's info blue) on the features it changed. A person can always tell three things at a glance: what is built, what is building, and who changed what.

## Screens and states

| # | State | How to reach it in the app |
|---|---|---|
| 1 | **Start page** (no document) | Launch; Home button at the far left of the app bar |
| 1b | **Blank canvas** (the alternative for no document) | Prototype ▾ › 1b |
| 2 | **Part open**: app bar, toolbar, browser, viewport with grid, view cube and triad, timeline, properties, status | Open `bracket.silico` from Recent |
| 2r | **Rebuilding**, with Stop | Any edit; Prototype ▾ › 2r sets a slow kernel so Stop (or Esc) can be tried |
| 3 | **Pad dialog** docked right, with the distance arrow in the canvas | Double-click *Legs* in the timeline |
| 3e | **Failed preview** inside the dialog | Type `8` in the dialog's Distance (both pockets then cut nothing); type `abc` for an unparseable value |
| 4 | **Selection**: hover, a selected face and edge, the filters, Select Other | Click faces and edges; Ctrl/⌘-click adds; drag on empty space for box select; right-click › Select Other |
| 5 | **Sketch mode**, a dimension being placed with its Modify box | Prototype ▾ › 5, or double-click *Bracket profile* then press D and click a line |
| 5n | **New sketch** with the line tool, inference glyphs, and live DOF | Create Sketch › pick a plane › L |
| 6a | **Failed feature** after an agent's edit | Prototype ▾ › 6a |
| 6b | **Broken reference** with candidates (hover one to see it in the canvas; *Use* re-points it) | Prototype ▾ › 6b |
| 6c | **File changed on disk**, no unsaved edits: reloaded, changes marked | Prototype ▾ › 6c |
| 6d | **File changed on disk** while you have unsaved edits: Reload / Keep mine / Show changes, and Save held | Prototype ▾ › 6d |
| 7 | **Shortcut sheet**, filterable | `?` anywhere, or *Shortcuts* in the status bar |

Also built: the command search (`S`), File menu, context menus on the canvas, timeline and browser, What's Wrong (click the red status or the document's root), Save As, close-with-unsaved-edits, the over-defining-dimension prompt (sketch palette › *Try: dimension a defined line*), suppress, rename (F2), the rollback bar, Undo/Redo with named steps in their tooltips, visibility eyes, view cube faces/edges/corners, orthographic/perspective, grid toggle, Look At.

## Start page or blank canvas? Recommendation: the start page

Both are built so Trevin can compare (1 and 1b). **Recommended: the start page**, because:

- With no document, no command in the toolbar can act. A canvas full of disabled tools reads as broken, which is today's problem with nicer paint.
- ~~New asks for the path first (open item 30), so a blank canvas can't really be "drawn on" before a file exists.~~ *Overturned by Trevin (2026-11-01):* New opens an untitled part at once, with nothing on disk until its first save asks for a name (open item 30, overturned). The start page still puts New, Open, and Recent first, the three things that can happen next.
- It earns its place later too: it's the **Home** tab (app bar, far left), so recent parts stay one click away with documents open, as Fusion's Home and Onshape's documents page are.
- It's honest about warm start (decision 10): the status bar says *Starting the kernel and sketch solver…* then *ready*.
- It tells a new user the one non-obvious thing about Silico: agents edit the same files through `silico serve`, and there's a *Copy MCP config* button.

The blank canvas (1b) is still the right look for a **new, empty part**: grid, axes, view cube, the origin planes on Create Sketch, and one hint ("Start with a sketch") that goes away with the first feature.

## Layout

```
┌ app bar 30 ─ ●●● ⌂ ≡File 💾 ↶ ↷ │ bracket.silico* × │ +                 Prototype▾  🔍 ? ┐
├ toolbar ─ DESIGN │ SOLID [SKETCH]                                                      ┤
│           CREATE ▾ │ MODIFY ▾ │ INSPECT ▾ │ SELECT ▾ (face/edge/vertex toggles)  [Finish Sketch] │
├ BROWSER 248 ┬──────────────── canvas (grey) ──────────────────┬ PROPERTIES 296 ┤
│ ▾ bracket   │ [banner: changed on disk …]          ⌂ ▾ [cube] │ selection /     │
│  › Origin   │                                                 │ feature /       │
│  ▾ Bodies   │                part                [palette]    │ document /      │
│  ▾ Sketches │                                                 │ docked dialog   │
│             │ triad  Orthographic · Iso     [nav bar]         │ [Cancel] [OK]   │
├ TIMELINE ─ ▱ ▣ ▱ ▣ ▱ ▣ ⌐ ┃rollback                         1 error · 7 features ⚙ ┤
└ status ─ ● Rebuilt · rev 12 · 84 ms  ■Stop ⟳Rebuild │ hover/selection/coords │ Unsaved · 2 edits │ mm │ ?┘
```

- **Fusion's regions** (§8), docked rather than floating: the browser and properties are side panels (egui `SidePanel`s or `egui_tiles` panes), not Fusion's floating browser over the canvas. Floating panels over a 3D view steal picks and need translucency egui doesn't do well. Both panels collapse.
- **Resizable side panes** (*approved by Trevin, 2026-11-09*: "I also wanna grab the side panel and resize it"): either side pane's inner border drags, with the resize cursor over it and 3 px either side of it (egui's grab radius). The column moves by the drag's distance and the canvas takes up the difference, between the pane's least width and what leaves the canvas its own. 248 and 296 above are the defaults; the least columns, each what its content needs rounded up to the 8 px grid the defaults are on, are the browser's 200 (its deepest row's indent, caret, eye, icon, sketch prefix and mark, `Sketch12` whole, and its border: an exact fit, already on the grid) and the properties' 232 (`--pad` 12 at each side, a row's 96 px key column, the 8 px gap after it, a value column as wide as the key's, and the border: 225, which also fits the dialog footer's Cancel and OK with their key caps), and the canvas keeps 340 (a banner at its least). A narrower window takes width from the panes first, each in proportion to how far it is above its least, then from the canvas, so the panes and the canvas need 772 (200 + 340 + 232); the window's least width is the toolbar's in sketch mode, 888 (its panels and Finish Sketch, 881.5, on the 8 px grid), so at it the panes keep their defaults. A drag in a window too narrow for both chosen widths changes only the dragged pane's: while it's held the other pane keeps the width it's shown at, and on release that pane takes back its share of what the drag freed (at 800 wide, the browser dragged from 212 to 202 settles at 201 and the properties grow from 248 to 259), and widening the window restores both chosen widths. It's a deliberate trade, for Trevin to approve knowingly: the dragged border moves exactly with the pointer, and neither pane's choice is lost to a narrow window. The widths dragged to are remembered across restarts in `pane-widths`, beside `recent-documents` in the platform's app-data folder. The prototype's borders drag too (`.sash`, interact.js), but its grid doesn't model a narrower window: there the canvas alone shrinks.
- **One right panel, four jobs**: properties of the selection, of the selected feature, of the document when nothing is selected, and the feature dialog when one is open (dialogs dock right, open item 27). It never shows an empty "nothing selected" state; with nothing selected it shows the document's revision, save state, file-on-disk state, and model measures.
- **The macOS title bar is the app bar** (eframe's `with_fullsize_content_view`, as Rerun does): today's app wastes a row on the system title.

## What came from where, and why

**Fusion 360 (layout and look)**
- Application bar with File, Save, Undo/Redo, document tabs; toolbar of icon panels with capital labels and a drop-down under each (CREATE, MODIFY, INSPECT, SELECT); the contextual **SKETCH** tab, highlighted blue, with a blue **Finish Sketch** at its right end.
- Browser grouped by type (Origin, Bodies, Sketches) with an eye per row; the timeline as icons along the bottom; the ViewCube top right (faces, edges, corners clickable, drag orbits, Home); the floating navigation bar bottom centre (Orbit, Pan, Zoom, Fit, Look At, Display settings, Grid).
- Feature dialogs: label left, control right, units in the field, OK disabled while a value doesn't parse, a distance arrow in the canvas that drags the value.
- The Sketch Palette floating in the canvas (status, tool options, Show toggles, Look At).
- Navigation: wheel zooms toward the pointer, middle-drag pans, Shift+middle orbits, F6 fits, Z up.

**SolidWorks (behaviour)**
- Selection: click replaces, Ctrl/⌘ toggles, Esc or a click on empty space clears; drag on empty space box-selects edges, left-to-right window (solid outline), right-to-left crossing (dashed); X/E/V filters as toggles; **Select Other** listing what's under the pointer in order, Tab or the wheel stepping through it with the canvas highlighting each.
- Errors: red marks on the feature in the timeline and the browser and on the document's tab; a click on the document's root opens **What's Wrong**; OK still allowed on a failed preview (saves the feature with its error).
- Sketches: definition-state colours; `(–)`/`(+)`/`(?)` prefixes in the browser; the status bar showing pointer coordinates and the state; the three-way prompt for an over-defining dimension (driven / leave driving / cancel); automatic relations with Ctrl held to suppress them; click-click line chains ended by double-click or Esc.
- Esc interrupts a rebuild; Ctrl+B rebuilds; Ctrl+S saves; asterisk on an unsaved tab; Close asks.
- Solids: Trevin's decision of 2026-11-09: solids are made sketch-first, as SolidWorks makes them; the app creates no primitives, and edits those a file holds. CREATE holds Create Sketch and the sweeps alone, and no command, menu, or key makes a box, cylinder, sphere, cone, or torus; one a file holds (an agent's, through `silico serve`, or written by hand) opens its dialog from Edit Feature (a double-click, or the feature's menu), as any feature does.

**Onshape (good ideas)**
- The **rollback bar**, here a blue handle in the timeline you drag between features (Fusion's marker is out of M5, so this is flagged).
- **Hover a feature, see its geometry**: hovering a timeline item tints the faces that feature made; hovering a face outlines its feature in the timeline. Silico's element map knows every face's maker, so this is cheap and honest.
- Clean, dense feature rows and a calm document list on the start page.

**Rerun (the look)**
- re_ui 0.38.1's dark palette and tokens, Inter Medium as the only face, 24 px list rows, 11 px uppercase section headers, 1 px Gray.250 panel borders, a dark grey gradient canvas (Fusion's, Trevin's change of 2026-11-09, below; it was a black viewport), the light **primary** button (Rerun's, not blue) for OK, blue reserved for selection and "active".

**Never FreeCAD**: no task panel combo views, no workbench switcher, no tree-as-history, no modal property editor.

## Agents as equal clients

- **Disk changed, nothing unsaved** (6c): the session reloads on its own (§4). An info banner (re_ui's info blue on the banners' dark card, the reload glyph) says so, lists what changed by feature and field, and the changed features carry an info-blue dot in the timeline and browser until looked at. *Changed (Trevin, 2026-11-09):* the purple and the ± glyph didn't fit the theme; they're the info tone and the reload glyph now, the dot's light blue kept apart from the selection's saturated blue. Nothing modal; Esc dismisses the banner.
- **Disk changed with unsaved edits** (6d): an amber banner explains who could have written the file (an agent through `silico serve`, git, an editor; the app can't know which) and offers **Show changes**, **Reload, discard my edits**, and **Keep my edits…**. Keeping them holds Save (a `conflict` per decision 2) and offers Save As; the status bar keeps a *Changed on disk* chip until resolved.
- **Errors carry the agent's words**: every error card shows the structured error's code, message, elements, parameter, and suggestion, and has *Copy as JSON*, the exact object an agent gets. Selected elements show their stable name with a copy button, so a person can hand an agent "this face" in its own terms.
- **Command search** (`S`) lists the same commands MCP exposes.

## Tokens

All colours are re_ui 0.38.1 dark-theme values (resolved through `color_table.ron`) unless marked *proposal*.

| Token | Value | re_ui source | Use |
|---|---|---|---|
| canvas | `#141517` at the top to `#2a2c30` at the bottom | Fusion's dark theme (*changed*: Trevin, 2026-11-09, then darkened by him the same day; was Gray.0 `#000000`, `viewport_background`) | the viewport behind the part |
| bar / panel | `#0d0d0d` | Gray.100, `top_bar_color`, `panel_bg_color` | app bar, toolbar, side panels |
| bottom bar | `#171717` | Gray.150, `bottom_bar_color` | timeline, status |
| floating / field | `#212121` | Gray.200, `floating_color`, `text_edit_bg_color` | menus, dialogs, inputs, nav bar |
| hover | `#272626` | Gray.250, `list_item_hovered_bg` | rows; also 1 px panel borders |
| widget | `#2c2b2b` | Gray.300, `widget_inactive_bg_fill` | buttons, toolbar hover |
| stroke strong | `#525151` | Gray.350 | key caps, glyph borders |
| text subdued | `#939090` | Gray.550, `text_subdued` | labels, hints (6.4:1 on bar) |
| text | `#cfcfcf` | Gray.750, `text_default` | body |
| text strong | `#ffffff` | Gray.1000, `text_strong` | titles, values that matter |
| selection | `#186add` | Blue.500, `selection_bg_fill` | selected rows, active tool, viewport selection tint |
| on selection | `#f0f2ff` | Blue.900 | text on selection (4.6:1) |
| focus ring | `#4676ff` | Blue.525 (*proposal*: re_ui's Blue.400 `#004bc2` is ~2.5:1 on black) | keyboard focus |
| error fill | `#ab0116` | `error_fg_color` | badges, error marks' fills |
| error text | `#ff3865` | Red.550 (*proposal*: `#ab0116` is too dark for text on black) | error text and icons |
| warning | `#ff7a0c` | `warn_fg_color` | blocked features, warnings |
| alert warning | `#df7700` @ 18%, text `#ffd28a` | `alert_warning` | conflict banner |
| success | `#00da7e` | Green.700 | "Rebuilt", ready dots |
| axes X/Y/Z | `#ff6981` `#00b266` `#7a95ff` | `axis_color_x/y/z` | grid axes, triad, cube stubs |
| changed outside | `#b780ff` | Purple.600 (*proposal*) | agent/other-program changes |
| primary button | fill `#e0dfdf`, text `#272626` | `button_primary` | OK, Create, New part |

**Sketch definition states** (SolidWorks' meaning, dark-theme values): under defined `#7a95ff` (Blue.600), fully defined `#eeeeed` (Gray.900, SolidWorks' "black" on a dark canvas), over defined and unsolvable `#ff3865`, invalid `#f2c94c` (*proposal*), dangling `#b5b84a` (*proposal*), driven dimension `#827f7f` (Gray.500). Construction is dashed in its state's colour.

**Viewport**: body `#b9c0ca` under `re_renderer`'s `simple_lighting` (ambient 0.2, lights at (1,2,3) and 0.5×(−1,−3,−5), taken in view space as §10's fixed camera implies); edges `#101010` 1.5 pt; hover: white tint 28% and 3 pt white edges; selection: Blue tint 55% and 3.5 pt `#5b95ff` edges; a feature's faces on hover: Blue.800 tint 40%; candidates: `#ff7a0c`. Grid Gray `#3a3939`/`#646363` at 10/50 mm, fading out by 200 mm.

**Type**: Inter Medium only (egui has no synthetic bold; emphasis is colour). Body 12/16, −0.15 tracking; section headers 11 uppercase; titles 13; monospace 11 only for IDs and element names (*proposal*: a second font, see questions).

**Spacing**: top bar 30 (28 + 2 for traffic lights); title rows 24; list rows 24; tree indent 18; toolbar tabs 24 + panels 52; timeline 40; status 24; panel padding 12; item spacing 8; small radius 4, normal 6; toolbar buttons 30 with 20 px icons; key caps 16 high.

## egui mapping

| Part | egui / egui_tiles / re_renderer |
|---|---|
| App bar | `TopBottomPanel::top`, height 30; macOS `ViewportBuilder::with_fullsize_content_view(true)` + `with_title_shown(false)`, left inset for the traffic lights (as Rerun's top bar). Tabs: `selectable_label`s in a horizontal `ScrollArea`; the close `×` shown on hover/active |
| Toolbar | `TopBottomPanel::top`; tabs row of `selectable_label`s; each panel a `ui.vertical` with a row of `ImageButton`s (SVG icons) and a small `menu_button` label ("CREATE ▾"). Finish Sketch: a `Button` with `fill(blue)` right-aligned |
| Browser | `SidePanel::left` (resizable, collapsible to a 28 px strip) or an `egui_tiles` pane labelled for accessibility; rows from `CollapsingState` + a custom row painter (caret, eye `ImageButton`, icon, label, right-aligned meta), 24 px, hover/selected fills from tokens |
| Viewport | `CentralPanel` with an `egui_wgpu::CallbackTrait` paint callback into `re_renderer` (docs/study/viewport.md); everything drawn over it (triad, Select Other highlight, sketch, dimensions, glyphs, the distance arrow, box select) uses `ui.painter()` in screen space, as the prototype's SVG overlay does |
| View cube | Either a second small `re_renderer` view in the corner, or painter-drawn: six projected quads with text, hit-tested in 2D. Home and ▾ are small buttons beside it |
| Navigation bar | A scope over the canvas at its bottom centre (`viewport::navigation_bar`), painted as `.nav-bar` is (floating fill, a Gray.250 border, 6 radius, the `0 4px 14px` shadow), its buttons the app bar's `.ab-btn`, 26 wide (at least) by 24 high (`bar::ButtonStyle::NAVIGATION_BAR`), and Display settings' menu an egui popup above it |
| Banners | A `Frame` at the top of the central panel's `ui` (an `Area` anchored top within the canvas rect, right margin past the cube); buttons inline |
| Properties panel | `SidePanel::right` (resizable) or an `egui_tiles` pane; rows as `re_ui`'s `PropertyContent` pattern: `egui::Grid` with two columns, label `weak`, value strong |
| Feature dialog | Same right panel; `DragValue` or `TextEdit::singleline` with a unit suffix and red stroke when `parse` fails; selection boxes as framed rows with a blue left stroke when active; a bottom row (`TopBottomPanel::bottom` inside the side panel) with Cancel and OK |
| Error card | `Frame` with `alert_error` fill, an icon, a code chip (`RichText::monospace`), a `Grid` for elements/parameter, and buttons; candidates as rows whose `hovered()` sets the viewport's highlight |
| Timeline | `TopBottomPanel::bottom` at 40; a horizontal `ScrollArea` of 30 px icon buttons; marks painted at the icon's corner; the spinner is a painted arc (`ctx.request_repaint` only while a step runs); the rollback bar a thin `Sense::drag` rect; drag-to-reorder via `dnd_drag_source`/`dnd_drop_zone` |
| Status bar | `TopBottomPanel::bottom` at 24; labels, a 70 px `ProgressBar`, small buttons |
| Menus | `Response::context_menu` and `menu_button`; Select Other as a context menu whose rows set the hover highlight; Tab/wheel handled by `ctx.input` while it's open |
| Modals | `egui::Modal` (backdrop `#000000CC`, `modal_backdrop_color`) for the typed Open and Save As (the native panels' fallbacks), Close, over-define, What's Wrong |
| Shortcut sheet | `egui::Modal` with a `TextEdit` filter and three `columns` |
| Command search | An `Area` anchored top centre with a `TextEdit` and a list; arrows/Enter via `ctx.input` |
| Tooltips | `on_hover_ui` with a title, key caps, one-line description (egui's 0.5 s delay) |
| Toasts | An `Area` anchored bottom-right of the canvas, faded by `ctx.animate_value_with_time` |
| Key caps | Painted `Frame`s (Gray.250 fill, Gray.350 stroke, 3 radius); `Modifiers::COMMAND` renders ⌘ on macOS and Ctrl elsewhere |
| Focus | egui's focus ring set to Blue.525 1 px plus a 2 px halo (`focus_halo_stroke`), focus order left to right, top to bottom: app bar, toolbar, browser, canvas, properties, timeline, status |

Nothing uses blur, gradients, or animation beyond a 200 ms view transition (egui `animate_value`), the step spinner, and toasts; `prefers-reduced-motion` turns the transitions off.

## Keyboard

The sheet (`?`) is generated from the same command registry as menus, tooltips and search, so a shortcut is never documented in one place and different in another. Highlights:

- File: ⌘N, ⌘O, ⌘S, ⇧⌘S, ⌘W. Edit: ⌘Z, ⇧⌘Z / Ctrl+Y, F2, Del, S (search).
- Rebuild: ⌘B (rebuild / resume), ⇧⌘B (everything), ⌘R (reload: the file read again, then everything rebuilt; it asks first if it would drop unsaved edits, drops nothing when the file is gone from disk, only rebuilds an untitled part, and is refused while a dialog is open (finish or cancel it first); Trevin's ask, 2026-11-09), Esc (stop).
- View: F6 fit, Home, ⌘1–⌘7 standard views (SolidWorks' numbering), ⌘8 Look At; Shift+middle orbit, middle pan, wheel zoom. Look At turns the view square to the sketch in sketch mode (its x across, its y up), and otherwise to the one flat face selected (from its outside, +z up, or +y for a face along z, as the standard views have it); with neither, it says "Look At: select a flat face first." In sketch mode, F6, Home, the standard views and Look At take the sketch's geometry in with the bodies, and fit them clear of the sketch bar and the palette (the M5.12 slice's finding, docs/walkthrough/m5.md, 2026-11-09).
- Selection (outside sketches): X faces, E edges, V vertices, H hide/show, Esc clears. A click on the timeline's empty space clears it too, and ⌘/Ctrl-click on a selected timeline item or browser row takes its feature out (Trevin's ask, 2026-11-09).
- Sketch: L line, R rectangle, C circle, A arc, P point, T trim, D Smart Dimension, X construction, hold ⌘/Ctrl for no relations, ⌘↵ Finish Sketch.
- Dialogs: ↵ OK, Esc Cancel.
- **Esc does the nearest thing first** (§3): ends a drag, then a sketch tool's chain, then the tool, then stops a dialog's preview, then cancels the dialog, then clears the selection, then stops the rebuild.

## Proposals beyond M5.0 (each needs a yes or no)

1. **Purple "changed outside" marks and the field-level summary** in the reload banner. The app can diff two `timeline` answers for added/removed features, but "radius 5 → 3 mm" needs either a per-feature content hash in `timeline` or a core diff command (the `textconv` driver already has the reading).
2. **Restore my version** after an automatic reload: keep the pre-reload document as one undoable step. Today a reload clears undo.
3. **Copy as JSON** on errors and **Copy name** on elements (agent hand-off).
4. **Hover-to-highlight between timeline and canvas** (Onshape), driven by the element map's makers.
5. **The rollback bar** (out of M5 per §8; built here because the brief asked for it).
6. **Command search** on `S` (Fusion's toolbox, SolidWorks' S key) listing the MCP commands.
7. **Home tab start page** with *Copy MCP config*.

## Open questions for Trevin

1. **Start page or blank canvas** for no document? Recommended: start page (above). The blank canvas stays as the empty-part look.
2. **F6 is claimed twice** in M5.0: §8 makes it Fit (Fusion), §9 makes it "filters on/off" (SolidWorks). The prototype gives F6 to Fit and leaves the filter toggle to the SELECT panel. Agree?
3. **V is claimed twice**: §8 "V toggles the selection's visibility" (Fusion) and §9 "V filters vertices" (SolidWorks). The prototype keeps V for vertices and moves hide/show to **H**. Agree?
4. **Ctrl+Q** (SolidWorks' forced rebuild) quits on macOS. The prototype uses ⇧⌘B / Ctrl+Shift+B on every platform. Agree?
5. **Error red and focus blue**: re_ui's `#ab0116` and Blue.400 fail contrast as text and focus rings on black. Use Red.550 for error text and Blue.525 for focus, keeping `#ab0116` for fills?
6. **Two colours re_ui lacks**: invalid yellow and dangling olive. Fine as constants in `theme.rs`? (The purple "changed outside" was a third, until Trevin's change of 2026-11-09 gave it the info tone.)
7. **Under-defined blue vs selection blue** in sketches: both blue. The prototype marks a picked sketch entity with a wider halo and white stroke. Or use SolidWorks' green for selected sketch entities?
8. **A monospace font** for IDs and element names (`p0000001.pad.Face(…)`): the toolkit study allows one "when something needs one". Which (JetBrains Mono, IBM Plex Mono, both OFL), or Inter with tabular figures only?
9. **Icons**: SVG through `egui_extras`' `svg` loader (resvg, offline) or painted by Silico's own code? The set here is 70 simple 20×20 strokes either way.
10. **Docked browser** instead of Fusion's floating one over the canvas. Agree?
11. **A permanent properties panel** (296 px) when no dialog is open. Fusion has none; it's where errors, measures and names live here. Keep it (collapsible)?
12. **Lighting** (§10's veto line): with `simple_lighting` fixed to the camera, faces turned away read as dark as 0.2 (see the upright's outer face from the back). Acceptable, or raise the ambient?
13. ~~**New part**: the prototype's own dialog (name + folder, path shown) or the system's save panel (`rfd`)? Both satisfy open item 30.~~ *Settled by Trevin's decision (2026-11-01), which overturns open item 30:* New asks nothing, opening an "Untitled" part at once; its first Save, and every Save As, opens the system's save panel (`rfd`). The prototype's New dialog is gone.
14. **Timeline labels**: icons only (Fusion), names on hover. Add an option to show names under the icons?

## Files

- `index.html`: the shell. `styles.css`: tokens and components.
- `js/app.js`: state, the stand-in model (health, problems, rebuild, undo), and every panel's render.
- `js/interact.js`: actions, keyboard and the Esc chain, viewport hooks, menus, Select Other, agents' edits, the prototype's states.
- `js/sketch.js`: sketch mode (states, glyphs, dimensions, Modify box, line tool with inference).
- `js/viewport.js`: three.js stand-in for `re_renderer` (lighting, edges in points, picking, view cube, triad, Fusion navigation).
- `js/geometry.js`: the bracket from §7, face by face, with element names. `js/icons.js`: the icon set.
- `vendor/three.min.js` (MIT), `fonts/Inter-Medium.otf` (OFL, copied from re_ui 0.38.1, `OFL.txt` beside it).

Prototype limits: only `bracket.silico` is modelled; element names are illustrative, in Silico's form; in a browser ⌘N, ⌘W and ⌘1–8 belong to the browser, so use the menus and the view cube for those; Alt-drag also orbits, for trackpads.

## Approval (2026-11-01)

Trevin: "looks good, build it". The open questions are taken as recommended above: a start page for no document; F6 Fit; H hide/show; ⇧⌘B full rebuild; brighter error red and focus blue for text; the halo for selected sketch entities; the three extra colours as theme.rs constants. The rest go to the port PRs' judgement under these defaults. The proposals that need core changes (a per-feature change summary, "Restore my version" across a reload) are designed separately before they're built.

## Changes since approval

**The canvas (Trevin, 2026-11-09).** "The background of the part shouldn't be pitch black, it looks like the void." The canvas is Fusion's dark-theme canvas instead of Gray.0: a vertical gradient, soft charcoal `#141517` along its top to a lighter `#2a2c30` along its bottom (styles.css's `--canvas-top` and `--canvas-bottom`), the dark UI around it and the grey part still standing out. Trevin darkened it the same day, after seeing it in the app: the first values, `#1e1f22` to `#3a3c41`, were "too light". The WebGL canvas is cleared transparent over it (viewport.js's `setClearColor(0x000000, 0)`, for the viewport and the start page's thumbnails), as the app paints the gradient with egui's painter under `re_renderer`'s frame, which has no background of its own there. The values are theme.rs's `VIEWPORT_BG_TOP` and `VIEWPORT_BG_BOTTOM`, one constant each, to be adjusted from a screenshot. Agents' renders (`render`'s PNGs) keep their own flat light grey: they're read for the part, not the app's look.

**The ground grid (Main, 2026-11-09).** The app paints the prototype's `buildGrid` (a line every 10 mm across the XY plane out to 200 mm, each 50 mm's stronger, each 20 mm segment's ends faded by their distance from the origin, and the X, Y, and Z axes) with egui's painter under `re_renderer`'s frame, between the canvas's gradient and the bodies, each line projected by the renderer's own projection. That's a deliberate difference from the prototype, which draws the grid in the scene, depth-tested: there its lines show over a body's part below z = 0, and over its bottom seen from below; in the app a body hides the grid wherever the body is drawn, below z = 0 or seen from below. The grid never covers a body, and is never in the picking layer, so it can't take a click meant for a face. Its offsets below the plane (the lines at z = −0.05, the axes at −0.04), which kept its depth-tested lines off the origin planes, have nothing to do there, and the app draws it at z = 0. Its fade runs along each segment on the screen, where the prototype's WebGL runs it along the line: the same seen square on, a little nearer the far end's alpha on a segment cut near the eye in a view grazing the plane. As in the prototype, it's on at first, the navigation bar's Grid shows and hides it, and a sketch open hides it (sketch.js). The display menu's *Ground grid* item, built with that menu (the prototype audit's item 11), shows and hides it too. In the app it's `crate::grid`.

**The origin planes (Worker 3, 2026-11-15; audit item 17).** The app draws the prototype's `buildPlanes` (`crates/silico-app/src/planes.rs`): XY, XZ, and YZ each the square from (−4, −4) to (46, 46) in its own (U, V), filled `#9f9f9f` at 0.08, outlined `#9f9f9f` at 0.6 one point wide, a plane hovered filled `#186add` at 0.32 and outlined `#4676ff` (theme.rs's `VIEWPORT_PLANE_*`). Unlike the ground grid, they're drawn by `re_renderer` among the bodies (`silico_render::render::Plane`, `Frame::planes`), depth-tested as the prototype's are:
- *The fill* is a mesh, unlit, its alpha in its material, so `re_renderer` draws it in its transparent phase: after every opaque face, tested against their depth and writing none, from both sides, as `MeshBasicMaterial({ transparent, side: DoubleSide, depthWrite: false })` is. A body in front of a plane hides it; a plane in front of a body tints it and hides nothing. One difference: `re_renderer` draws its lines (edges, sketch curves, the planes' outlines) before its transparent meshes and writes no depth for them, so the fill tints an edge behind it (as the prototype) but also the half of a silhouette edge's stroke beyond its faces in front of it, a sketch's curve clear of every face, and the inner half of its own outline. An edge in front of a plane lies on the faces it bounds, whose depth keeps the fill off it.
- *The outline* is a closed line strip, placed and moved toward the eye as a shown sketch's curves are (#449), alpha blended at its 0.6.
- *Neither is in the picking layer.* `re_renderer` draws every mesh and line into it with depth, whatever its ID, which would cover the faces and edges behind a plane from a pick (a sketch's curves cover what's under their stroke). So each is `re_renderer`'s own draw data wrapped so its renderer draws every phase but the picking layer's (`render/planes.rs`, `Unpicked`): a frame's picking is the same with planes as without, pixel for pixel, and a plane takes no click.

Which show is one rule for every way a plane shows. Each plane's eye shows it; they open hidden, as the prototype's `vis.planes` and Fusion's and SolidWorks' origin planes do (m5.0-app.md's "everything shows" otherwise). The Origin folder's eye is the planes', as the prototype's folder row's: open while any shows, and a click shows all three if none shows, else hides them all; its label is "Show origin planes" or "Hide origin planes", apart from the origin point's row, also named Origin. The app's origin point and axes keep their own eyes, and draw nothing yet. The origin's rows aren't faded with their eyes closed, as the prototype's plane rows aren't. A plane's browser row under the pointer draws it hovered, eye open or not: the prototype's `planeRow` lights the plane its row selects, but the app's origin rows don't select, so the pointer lights it, as a timeline item hovered lights its faces (#485). Create Sketch's planes (its menu, from the toolbar, CREATE's drop-down, or the canvas's menu) show all three, the one whose item the pointer is on lit: the prototype's `startPickPlane`, whose pick in the canvas is the menu's item in the app. The planes are the prototype's fixed 50 mm whatever the model's size; Fusion and SolidWorks size theirs to the model, a later change.
