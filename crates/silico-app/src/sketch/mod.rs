//! Sketch mode (docs/design/m5.0-app.md, section 9, *Sketching*; the M5.11 plan): a tab's
//! state while a sketch is open, never a pane of its own. The sketch open and the revision
//! sketch mode opened at; the newest `sketch` read, one in flight at a time, the one
//! source of what's drawn, the status, and the tools' context; the sketch's members
//! selected; the palette's settings; and the tool chosen ([`tools`]), with what it asks of
//! the core. Entering, Finish, Cancel, and the edits are the tab's (`tab.rs`), through its
//! queue, as a feature dialog's are.
//!
//! The viewport calls [`SketchMode::frame`] each frame (through `Tab::sketch_frame`, which
//! then sends what the tool asked) with its left button's events, a projection to the
//! screen, and the pointer's way back onto the sketch's plane; nothing here knows of a
//! camera. A tool on takes the left button before the selection does.

pub mod bar;
pub mod draw;
pub mod hit;
pub mod style;
pub mod tools;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use eframe::egui::{self, Painter, Pos2, Rect, Vec2};
use silico_core::Error;
use silico_core::command::sketch::SketchRead;
use silico_core::command::{ModelChoice, Problems, Revision, Running, SessionId, Sessions};
use silico_core::document::{ElementId, ElementKind, FeatureId, Reference};
use silico_core::kernel::{Clients, SketchPlane};
use silico_core::sketch_states::SketchState;
use silico_core::sketch_tools::{Event, Modifiers};

use crate::tab::{Row, said};
use bar::{PaletteView, Settings, StateShown, StatusLine};
use draw::{
    Drawn, RegionFill, Scale, Shown, View, bounds, drawing, fill_scale, preview, region_fills,
    to_shapes,
};
use hit::{Hit, hit};
use style::*;
use tools::{Input, Tools};

/// A sketch's state as sketch mode words it (SolidWorks' five states, in the prototype's
/// words: an empty sketch "Empty", an under-defined one with its degrees of freedom left),
/// in the canvas's state colours, as the prototype has them.
pub fn state_shown(read: &SketchRead) -> StateShown {
    let empty = read.members.elements().next().is_none();
    let (words, colour) = match read.states.state {
        _ if empty => ("Empty".to_owned(), TEXT_SUBDUED),
        SketchState::UnderDefined => (
            format!("Under defined · {} DOF", read.states.dofs),
            SKETCH_UNDER_DEFINED,
        ),
        SketchState::FullyDefined => ("Fully defined".to_owned(), SKETCH_FULLY_DEFINED),
        SketchState::OverDefined => ("Over defined".to_owned(), SKETCH_OVER_DEFINED),
        SketchState::NoSolution => ("No solution found".to_owned(), SKETCH_OVER_DEFINED),
        SketchState::Invalid => ("Invalid solution found".to_owned(), SKETCH_INVALID),
    };
    StateShown { words, colour }
}

/// A word on what the state means, as the palette says it.
fn hint(state: SketchState) -> Option<&'static str> {
    match state {
        SketchState::UnderDefined => {
            Some("Blue curves can still move. Add relations or dimensions.")
        }
        SketchState::OverDefined => Some(
            "A red constraint conflicts, or a red dimension repeats others: make it driven, or delete one. An orange relation repeats others too.",
        ),
        SketchState::NoSolution => {
            Some("No position meets every constraint. Delete or change one.")
        }
        SketchState::Invalid => {
            Some("It solves only to geometry that can't be built, such as a zero-length line.")
        }
        SketchState::FullyDefined => None,
    }
}

/// A plane's name, as the sketch bar says it, and its axes' letters, as the status bar's
/// coordinates are given.
pub fn plane_words(plane: Option<&SketchPlane>) -> (&'static str, [&'static str; 2]) {
    match plane {
        Some(SketchPlane::Xy) => ("XY", ["x", "y"]),
        Some(SketchPlane::Xz) => ("XZ", ["x", "z"]),
        Some(SketchPlane::Yz) => ("YZ", ["y", "z"]),
        Some(SketchPlane::Face(_)) | None => ("a face", ["u", "v"]),
    }
}

/// A sketch's prefix in the browser (§2): `(+)` over defined, `(–)` under defined, `(?)`
/// not solvable (no solution, or one that can't be built); none fully defined.
pub fn prefix(state: SketchState) -> Option<&'static str> {
    match state {
        SketchState::OverDefined => Some("(+)"),
        SketchState::UnderDefined => Some("(–)"),
        SketchState::NoSolution | SketchState::Invalid => Some("(?)"),
        SketchState::FullyDefined => None,
    }
}

/// Whether a sketch's browser row, `row` (its name's response), asks for Edit Sketch: by a
/// double-click, or its context menu's Edit Sketch, on only while `can` (fusion.md, *Sketch
/// mode*, [esk]).
pub fn edit_asked(row: Option<&egui::Response>, can: bool) -> bool {
    let Some(row) = row else {
        return false;
    };
    let mut asked = can && row.double_clicked();
    row.context_menu(|ui| {
        if ui
            .add_enabled(can, egui::Button::new("Edit Sketch"))
            .clicked()
        {
            asked = true;
            ui.close();
        }
    });
    asked
}

/// A `sketch` read, answered: the revision of the model it read (none if it named none),
/// and its answer.
#[derive(Debug, Clone)]
pub struct Read {
    pub model: Option<Revision>,
    pub result: Result<SketchRead, Problems>,
}

/// What a left button did in the viewport, as the pane hands it over each frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerAction {
    Press,
    Move,
    Release,
    DoubleClick,
    /// A press that ends without a release (the pointer gone, focus lost, a navigation tool
    /// taking over, the tab hidden): every press ends in one release or one of these.
    Cancel,
}

/// One left-button event, at a place in the viewport (in the same space `to_plane` takes),
/// with the modifiers held.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerEvent {
    pub action: PointerAction,
    pub at: Pos2,
    pub shift: bool,
    pub ctrl: bool,
}

/// What the viewport gives sketch mode each frame: the left button's events, the pointer
/// (none off the viewport), a projection from the sketch's millimetres to screen points
/// (none for a place it can't show), and the way back, from screen points to the sketch's
/// plane (none for a ray that misses it).
pub struct FrameInput<'a> {
    pub events: &'a [PointerEvent],
    pub pointer: Option<Pos2>,
    pub project: &'a dyn Fn([f64; 2]) -> Option<Pos2>,
    pub to_plane: &'a dyn Fn(Pos2) -> Option<[f64; 2]>,
}

/// What Delete does with the selection: removes it (`sketch_remove`), or, for a lone point
/// two pieces share, joins them (`sketch_join`, §9, *Split*).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Deletion {
    Remove(Vec<Reference>),
    Join(ElementId),
}

/// What a frame's drawing was of: the read (by how many it has taken), the selection, what's
/// shown, and the view, by where it puts three places of the plane, its scale, its chord
/// tolerance, its
/// rectangle, and the part of the plane in it. A frame with the same key draws the same for
/// an affine view (the flat one, an orthographic camera), whose three places fix it; a
/// perspective one could move and keep them, so when the perspective viewport is wired, the
/// view is to be keyed on its camera.
#[derive(Debug, Clone, PartialEq)]
struct DrawnOf {
    read: u64,
    selected: BTreeSet<ElementId>,
    shown: Shown,
    probes: [Option<Pos2>; 3],
    scale: Scale,
    chord_tolerance: u32,
    clip: Rect,
    visible: Option<([f64; 2], [f64; 2])>,
}

/// A tab's sketch mode.
pub struct SketchMode {
    /// The number of the opening it is (the tab's dialogs' count), so its add's answer and
    /// its opening's marker find it.
    pub serial: u64,
    /// The sketch open: an existing one's, or, for Create Sketch, its ID once its add has
    /// answered.
    pub sketch: Option<FeatureId>,
    /// The revision it opened at, once everything asked before it has answered (set by its
    /// own opening alone): its undo goes no further back, and is off till it's known.
    pub opened_at: Option<Revision>,
    /// The newest `sketch` read answered, and the one in flight, with the published
    /// revision it was asked at.
    pub read: Option<Read>,
    reading: Option<Running<SketchRead>>,
    asked: Option<Revision>,
    /// How many reads it has sent, and taken: what a test counts, and what names the read a
    /// drawing and a fill are of.
    pub reads: u64,
    taken: u64,
    /// The profile's fill, worked out once for each read and each power-of-two scale (the
    /// last two such kept, so a zoom back and forth across one doesn't work it out each
    /// time), and how many times it has been: what a test counts.
    fills: Vec<((u64, i32, u32), Vec<RegionFill>)>,
    pub fill_builds: u64,
    /// The last frame's drawing, and how many drawings it has worked out.
    drawn: Option<(DrawnOf, Vec<Drawn>)>,
    pub drawings: u64,
    /// The members selected, each with its kind; and a press under way, what's under it and
    /// whether Ctrl was held, to select on its release.
    pub selection: BTreeSet<Hit>,
    pressed: Option<(Option<Hit>, bool)>,
    pub settings: Settings,
    /// The tool chosen, if one is (M5.11b's): what it draws as it's used, what it says, and
    /// what it asks of the core, which the tab sends.
    pub tools: Tools,
    /// The pointer on the sketch's plane, in millimetres, as the viewport last gave it.
    pub pointer: Option<[f64; 2]>,
    /// How many of its own edits sketch mode has undone and not redone: what Redo may give
    /// back, never edits from before it opened.
    pub undone: u32,
    /// For Create Sketch, the revision its add came to: its undo goes no further back, so it
    /// never undoes the sketch it's in (Cancel still goes back to where it opened).
    pub undo_floor: Option<Revision>,
}

impl SketchMode {
    /// Sketch mode on the sketch `sketch` (Edit Sketch), or on one still to be added (Create
    /// Sketch), numbered `serial`.
    pub fn new(serial: u64, sketch: Option<FeatureId>) -> Self {
        SketchMode {
            serial,
            sketch,
            opened_at: None,
            read: None,
            reading: None,
            asked: None,
            reads: 0,
            taken: 0,
            fills: Vec::new(),
            fill_builds: 0,
            drawn: None,
            drawings: 0,
            selection: BTreeSet::new(),
            pressed: None,
            settings: Settings::default(),
            tools: Tools::default(),
            pointer: None,
            undone: 0,
            undo_floor: None,
        }
    }

    /// The newest read's sketch, if it read one.
    pub fn drawable(&self) -> Option<&SketchRead> {
        self.read
            .as_ref()
            .and_then(|read| read.result.as_ref().ok())
    }

    /// Its read polled, without waiting: an answer taken (the selection let go of members it
    /// no longer has), and, once the sketch is known, a read of the newest published model
    /// sent if none is in flight and that model is newer than the last asked of. A published
    /// read answers at once, never queued behind the session's commands.
    pub fn poll<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        session: &SessionId,
        published: Option<Revision>,
    ) {
        if let Some(running) = self.reading.take() {
            match running.try_take() {
                Ok(response) => self.took(Read {
                    model: response.model.map(|model| model.revision),
                    result: response.result,
                }),
                Err(running) => {
                    self.reading = Some(running);
                    return;
                }
            }
        }
        let (Some(sketch), Some(published)) = (&self.sketch, published) else {
            return;
        };
        if self.asked == Some(published) {
            return;
        }
        self.asked = Some(published);
        self.reads += 1;
        self.reading = Some(sessions.sketch(session, ModelChoice::Published, sketch.clone()));
    }

    /// `read` kept as the newest, and the selection kept to what it still holds.
    pub(crate) fn took(&mut self, read: Read) {
        if let Ok(sketch) = &read.result {
            let held: BTreeSet<&ElementId> = sketch.members.elements().map(|(id, _)| id).collect();
            self.selection.retain(|hit| held.contains(&hit.id));
        }
        self.read = Some(read);
        self.taken += 1;
    }

    /// A click in the viewport, on `hit` (none: empty space), with Ctrl or not (open item
    /// 24, as SolidWorks' selection): a plain click selects only what it's on, a Ctrl-click
    /// adds it or takes it away, and a click on nothing clears the selection.
    pub fn click(&mut self, hit: Option<Hit>, ctrl: bool) {
        match (hit, ctrl) {
            (None, _) => self.selection.clear(),
            (Some(hit), false) => {
                self.selection.clear();
                self.selection.insert(hit);
            }
            (Some(hit), true) => {
                if !self.selection.remove(&hit) {
                    self.selection.insert(hit);
                }
            }
        }
    }

    /// What Delete would do with the selection: a lone point whose two curves end there,
    /// the pieces a split leaves, joined (whether `sketch_join` takes them is its own to
    /// say); anything else removed, as `sketch_remove` judges it. None with nothing
    /// selected.
    pub fn deletion(&self) -> Option<Deletion> {
        if self.selection.is_empty() {
            return None;
        }
        if let ([only], Some(sketch)) = (
            self.selection.iter().collect::<Vec<_>>().as_slice(),
            self.drawable(),
        ) && only.kind == ElementKind::Point
        {
            let id = &only.id;
            let members = &sketch.members;
            let ends = members
                .lines
                .iter()
                .filter(|l| &l.from == id || &l.to == id)
                .map(|l| &l.id)
                .chain(
                    members
                        .arcs
                        .iter()
                        .filter(|a| &a.start == id || &a.end == id)
                        .map(|a| &a.id),
                )
                .collect::<BTreeSet<_>>();
            if ends.len() == 2 {
                return Some(Deletion::Join(id.clone()));
            }
        }
        Some(Deletion::Remove(
            self.selection
                .iter()
                .map(|hit| Reference::Element(hit.id.clone()))
                .collect(),
        ))
    }

    /// One frame of the viewport's: the pointer's place on the plane kept for the status
    /// bar; each press noted with what's under it (within the pick tolerance at the view's
    /// scale there), and taken as a click when it's released, or let go if it's cancelled;
    /// and the sketch, and the tool's preview, drawn with `painter`, the grid over the part
    /// of the plane the painter's rectangle shows. The glyphs' and dimensions' sizes are
    /// worked out at the rectangle's centre, so they don't move with the pointer. What's
    /// drawn of the sketch is worked out again only when the read, the selection, what's
    /// shown, or the view changes, and its profile's fill only for a new read or a zoom past
    /// a power of two.
    pub fn frame(&mut self, painter: &Painter, input: &FrameInput<'_>) {
        self.pointer = input.pointer.and_then(|p| (input.to_plane)(p));
        let clip = painter.clip_rect();
        let scale = scale_at(input.to_plane, clip.center()).unwrap_or(Scale(1.0));
        // A tool on takes the left button, before the selection (the plan's precedence).
        let selecting: &[PointerEvent] = if self.tools.choice().is_some() {
            self.pressed = None;
            let held = painter.ctx().input(|i| Modifiers {
                shift: i.modifiers.shift,
                ctrl: i.modifiers.command,
            });
            self.tool_events(input, held);
            &[]
        } else {
            input.events
        };
        for event in selecting {
            match event.action {
                PointerAction::Press => {
                    let found = self.drawable().and_then(|read| {
                        let scale = scale_at(input.to_plane, event.at)?;
                        let at = (input.to_plane)(event.at)?;
                        hit(read, at, scale.mm(SKETCH_PICK_TOLERANCE), scale)
                    });
                    self.pressed = Some((found, event.ctrl));
                }
                PointerAction::Release => {
                    if let Some((found, ctrl)) = self.pressed.take() {
                        // What the press was on, if the read now still holds it: a read
                        // taken between the press and its release may have let it go.
                        let held = found.filter(|hit| {
                            self.drawable().is_some_and(|read| {
                                read.members.elements().any(|(id, _)| id == &hit.id)
                            })
                        });
                        self.click(held, ctrl);
                    }
                }
                PointerAction::Cancel => self.pressed = None,
                PointerAction::Move | PointerAction::DoubleClick => {}
            }
        }
        let corners = [
            clip.left_top(),
            clip.right_top(),
            clip.left_bottom(),
            clip.right_bottom(),
        ];
        let visible = corners
            .iter()
            .map(|&c| (input.to_plane)(c))
            .collect::<Option<Vec<_>>>()
            .and_then(|corners| bounds(corners.into_iter()));
        let view = View {
            project: input.project,
            scale,
            visible,
            chord_tolerance: SKETCH_CHORD_TOLERANCE,
        };
        let mut drawn = self.drawing(&view, clip);
        drawn.extend(preview(self.tools.preview(), &view));
        painter.extend(to_shapes(painter, &drawn));
    }

    /// The frame's left-button events given to the tool on, in the sketch's millimetres
    /// (through `to_plane`), each with the pick tolerance where it is; then the pointer, if
    /// it has moved since, as a hover, with the keys `held`. A press off the plane is none
    /// of the tool's; a release off it, or a press cancelled, ends the gesture with nothing
    /// added; a double-click off it ends a chain. With no read of the sketch to work
    /// against, only a release or a cancel reaches the tool, ending its gesture.
    fn tool_events(&mut self, input: &FrameInput<'_>, held: Modifiers) {
        let on_plane =
            |at: Pos2| (input.to_plane)(at).filter(|p| p[0].is_finite() && p[1].is_finite());
        let tolerance = |at: Pos2| {
            scale_at(input.to_plane, at).map_or(0.0, |scale| scale.mm(SKETCH_PICK_TOLERANCE))
        };
        for event in input.events {
            let Some(context) = tool_input(
                &self.read,
                self.sketch.as_ref(),
                &self.settings,
                tolerance(event.at),
            ) else {
                // No read to work against (none yet, or a failed one): a press's end still
                // ends it, so none outlives its release.
                if matches!(event.action, PointerAction::Release | PointerAction::Cancel) {
                    self.tools.cancel(None);
                }
                continue;
            };
            let keys = Modifiers {
                shift: event.shift,
                ctrl: event.ctrl,
            };
            let tools = &mut self.tools;
            match (event.action, on_plane(event.at)) {
                (PointerAction::Press, Some(at)) => tools.handle(&context, Event::Press(at), keys),
                (PointerAction::Move, Some(at)) => tools.handle(&context, Event::Move(at), keys),
                (PointerAction::Release, Some(at)) => {
                    tools.handle(&context, Event::Release(at), keys);
                }
                (PointerAction::DoubleClick, Some(at)) => {
                    tools.handle(&context, Event::DoubleClick(at), keys);
                }
                (PointerAction::DoubleClick, None) => tools.handle(&context, Event::EndChain, keys),
                (PointerAction::Release, None) | (PointerAction::Cancel, _) => {
                    tools.cancel(Some(&context));
                }
                (PointerAction::Press | PointerAction::Move, None) => {}
            }
        }
        let Some(pointer) = input.pointer else {
            return;
        };
        let Some(at) = on_plane(pointer) else {
            return;
        };
        if self.tools.choice().is_none() || self.tools.last() == Some(at) {
            return;
        }
        if let Some(context) = tool_input(
            &self.read,
            self.sketch.as_ref(),
            &self.settings,
            tolerance(pointer),
        ) {
            self.tools.handle(&context, Event::Move(at), held);
        }
    }

    /// What's drawn of the read through `view`, in `clip`: the last frame's, if nothing it's
    /// of has changed; otherwise worked out again, with the profile's fill worked out again
    /// only for a new read or another power-of-two scale.
    fn drawing(&mut self, view: &View<'_>, clip: Rect) -> Vec<Drawn> {
        let Some(read) = self
            .read
            .as_ref()
            .and_then(|read| read.result.as_ref().ok())
        else {
            return Vec::new();
        };
        let of = DrawnOf {
            read: self.taken,
            selected: self.selection.iter().map(|hit| hit.id.clone()).collect(),
            shown: self.shown(),
            probes: [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]].map(|p| (view.project)(p)),
            scale: view.scale,
            chord_tolerance: view.chord_tolerance.to_bits(),
            clip,
            visible: view.visible,
        };
        if let Some((last, drawn)) = &self.drawn
            && *last == of
        {
            return drawn.clone();
        }
        let (bucket, at) = fill_scale(view.scale);
        // Keyed on the outline's chord tolerance too: the fill is worked out at the one the
        // outline is drawn at, so the two share their vertices whatever it is.
        let key = (self.taken, bucket, view.chord_tolerance.to_bits());
        self.fills.retain(|(had, _)| had.0 == self.taken);
        if !self.fills.iter().any(|(had, _)| *had == key) {
            self.fill_builds += 1;
            self.fills
                .push((key, region_fills(read, at, view.chord_tolerance)));
            if self.fills.len() > 2 {
                self.fills.remove(0);
            }
        }
        let fills = self
            .fills
            .iter()
            .find(|(had, _)| *had == key)
            .map_or(&[][..], |(_, fills)| fills.as_slice());
        let drawn = drawing(read, view, of.shown, &of.selected, fills);
        self.drawings += 1;
        self.drawn = Some((of, drawn.clone()));
        drawn
    }

    /// The last frame's drawing, if one was worked out: what a test reads.
    pub fn last_drawn(&self) -> &[Drawn] {
        self.drawn
            .as_ref()
            .map_or(&[][..], |(_, drawn)| drawn.as_slice())
    }

    /// The fill worked out for the newest read at the power-of-two scale `bucket`, if it's
    /// kept: what a test reads.
    pub fn fill_at(&self, bucket: i32) -> Option<&[RegionFill]> {
        self.fills
            .iter()
            .find(|(had, _)| had.0 == self.taken && had.1 == bucket)
            .map(|(_, fills)| fills.as_slice())
    }

    /// The IDs selected.
    pub fn selected_ids(&self) -> BTreeSet<ElementId> {
        self.selection.iter().map(|hit| hit.id.clone()).collect()
    }

    /// What the palette's settings show.
    pub fn shown(&self) -> Shown {
        Shown {
            profile: self.settings.show_profile,
            points: self.settings.show_points,
            dimensions: self.settings.show_dimensions,
            constraints: self.settings.show_constraints,
        }
    }

    /// The sketch's state, as the newest read gives it.
    pub fn state(&self) -> Option<SketchState> {
        self.drawable().map(|read| read.states.state)
    }

    /// The plane's problem, as the newest read gives it: a face sketch whose face broke
    /// (#347).
    pub fn plane_problem(&self) -> Option<&Error> {
        self.drawable().and_then(|read| read.plane.as_ref().err())
    }

    /// What the palette shows, the tab at `revision`: the state's words; the read's refusal
    /// if it's of the session's own revision (one of an older model, from before the sketch
    /// was added, waits for the next); and the plane's problem.
    pub fn palette_view(&self, revision: Revision) -> PaletteView {
        let said = match &self.read {
            Some(Read {
                result: Err(problems),
                model,
            }) if model.is_none_or(|model| model >= revision) => Some(said(problems)),
            _ => None,
        };
        PaletteView {
            state: self.drawable().map(state_shown),
            hint: self.state().and_then(hint),
            tool: self.tools.choice(),
            tool_said: self.tools.said().map(str::to_owned),
            said,
            plane_problem: self.plane_problem().map(|error| {
                (
                    error.message().to_owned(),
                    error.suggestion().map(str::to_owned),
                )
            }),
        }
    }
}

impl SketchMode {
    /// Esc, with a tool on: the gesture ended and the tool left, the tool told if the sketch
    /// is read (section 9, *Line*). Whether a tool was on to take it.
    pub fn escape_tool(&mut self) -> bool {
        let input = tool_input(&self.read, self.sketch.as_ref(), &self.settings, 0.0);
        self.tools.escape(input.as_ref())
    }

    /// Enter, or the right-click menu's *End chain* (`event`), given to the tool on: a
    /// chain ended, the tool kept. Nothing before the sketch is read.
    pub fn end_chain(&mut self, event: Event) {
        if let Some(input) = tool_input(&self.read, self.sketch.as_ref(), &self.settings, 0.0) {
            self.tools.handle(&input, event, Modifiers::default());
        }
    }

    /// What the status bar says of the sketch, named `name`: that it's being edited, the
    /// pointer's coordinates on its plane, and its state.
    pub fn status(&self, name: &str) -> StatusLine {
        let plane = self.drawable().and_then(|read| read.plane.as_ref().ok());
        let [u, v] = plane_words(plane).1;
        StatusLine {
            editing: format!("Editing {name}"),
            coordinates: self
                .pointer
                .map(|[x, y]| format!("{u} {x:.3} · {v} {y:.3} mm")),
            state: self.drawable().map(state_shown),
        }
    }

    /// What the sketch bar shows of the sketch, named `name`.
    pub fn bar_view(&self, name: &str, enabled: bool) -> bar::BarView {
        let plane = self.drawable().and_then(|read| read.plane.as_ref().ok());
        bar::BarView {
            name: name.to_owned(),
            plane: plane_words(plane).0.to_owned(),
            state: self.drawable().map(state_shown),
            enabled,
        }
    }
}

/// What the tools work against: the newest read, if it read the sketch `sketch` of a model
/// it names; the pick `tolerance`, in millimetres; and the palette's settings.
fn tool_input<'a>(
    read: &'a Option<Read>,
    sketch: Option<&'a FeatureId>,
    settings: &Settings,
    tolerance: f64,
) -> Option<Input<'a>> {
    let read = read.as_ref()?;
    let revision = read.model?;
    let drawn = read.result.as_ref().ok()?;
    let sketch = sketch.filter(|sketch| **sketch == drawn.sketch)?;
    Some(Input {
        read: drawn,
        sketch,
        revision,
        tolerance,
        automatic_relations: settings.automatic_relations,
        construction: settings.construction,
        trim: settings.trim,
    })
}

/// Millimetres per screen point on the plane at `at`: the mean of how far one point across
/// and one point down move there; none where either misses the plane, or reads as nothing.
pub fn scale_at(to_plane: &dyn Fn(Pos2) -> Option<[f64; 2]>, at: Pos2) -> Option<Scale> {
    let here = to_plane(at)?;
    let across = to_plane(at + Vec2::new(1.0, 0.0))?;
    let down = to_plane(at + Vec2::new(0.0, 1.0))?;
    let d = |p: [f64; 2]| libm::hypot(p[0] - here[0], p[1] - here[1]);
    let mm = (d(across) + d(down)) / 2.0;
    (mm > 0.0 && mm.is_finite()).then_some(Scale(mm))
}

/// Each sketch's state, for the browser's prefixes: read one sketch at a time, of the
/// newest published model, each once for each revision of it.
#[derive(Default)]
pub struct Prefixes {
    states: BTreeMap<FeatureId, SketchState>,
    asked: BTreeMap<FeatureId, Revision>,
    reading: Option<(FeatureId, Running<SketchRead>)>,
    /// How many reads it has sent: what a test counts.
    pub reads: u64,
}

impl Prefixes {
    /// The read in flight polled, without waiting; and, if none is, the first of `rows`
    /// (the browser's sketches) not read of the model `published`, read. The sketch open in
    /// sketch mode, `open`, isn't read here: its state is its read's.
    pub fn poll<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        session: &SessionId,
        rows: &[Row],
        published: Option<Revision>,
        open: Option<&SketchMode>,
    ) {
        let open_id = open.and_then(|mode| mode.sketch.as_ref());
        if let (Some(id), Some(read)) = (open_id, open.and_then(SketchMode::drawable)) {
            self.states.insert(id.clone(), read.states.state);
        }
        if let Some((id, running)) = self.reading.take() {
            match running.try_take() {
                Ok(response) => match response.result {
                    Ok(read) => {
                        self.states.insert(id, read.states.state);
                    }
                    // A suppressed sketch, or one a worker's failure left unread: no prefix.
                    Err(_) => {
                        self.states.remove(&id);
                    }
                },
                Err(running) => {
                    self.reading = Some((id, running));
                    return;
                }
            }
        }
        self.states
            .retain(|id, _| rows.iter().any(|row| &row.id == id));
        self.asked
            .retain(|id, _| rows.iter().any(|row| &row.id == id));
        let Some(published) = published else {
            return;
        };
        if let Some(row) = rows
            .iter()
            .filter(|row| Some(&row.id) != open_id)
            .find(|row| self.asked.get(&row.id) != Some(&published))
        {
            self.asked.insert(row.id.clone(), published);
            self.reads += 1;
            let running = sessions.sketch(session, ModelChoice::Published, row.id.clone());
            self.reading = Some((row.id.clone(), running));
        }
    }

    /// The sketch `id`'s prefix, if its state is read and has one.
    pub fn prefix(&self, id: &FeatureId) -> Option<&'static str> {
        self.states.get(id).copied().and_then(prefix)
    }

    /// The sketch `id`'s state, if it's read.
    pub fn state(&self, id: &FeatureId) -> Option<SketchState> {
        self.states.get(id).copied()
    }
}

#[cfg(test)]
mod tests;
