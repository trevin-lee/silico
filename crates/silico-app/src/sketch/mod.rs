//! Sketch mode (docs/design/m5.0-app.md, section 9, *Sketching*; the M5.11 plan): a tab's
//! state while a sketch is open, never a pane of its own. The sketch open and the revision
//! sketch mode opened at; the newest `sketch` read, one in flight at a time, the one
//! source of what's drawn, the status, and the tools' context; what's selected, the
//! sketch's members and its origin and axes, in pick order ([`selection`]); the palette's
//! settings; and the tool chosen ([`tools`]), with what it asks of the core. Entering,
//! Finish, Cancel, and the edits are the tab's (`tab.rs`), through its queue, as a feature
//! dialog's are.
//!
//! The viewport calls [`SketchMode::frame`] each frame (through `Tab::sketch_frame`, which
//! then sends what the tool asked) with its left button's events, a projection to the
//! screen, and the pointer's way back onto the sketch's plane; nothing here knows of a
//! camera. A drag under way takes the left button first; then a tool on; then, with none,
//! a press on an under-defined point or curve that moves begins a drag ([`drag`]), a press
//! on empty space that moves draws a box that selects what it takes ([`boxing`]), and
//! anything else is the selection's, a double-click on a dimension opening its Modify box
//! ([`dimension`]).

pub mod bar;
pub mod bounds;
pub mod boxing;
pub mod dimension;
pub mod drag;
pub mod draw;
pub mod hit;
pub mod lines;
pub mod screen;
pub mod selection;
pub mod style;
pub mod tools;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use eframe::egui::{self, Painter, Pos2, Rect, Vec2};
use silico_core::Error;
use silico_core::command::DragGrab;
use silico_core::command::sketch::SketchRead;
use silico_core::command::{ModelChoice, Problems, Revision, Running, SessionId, Sessions};
use silico_core::document::{ElementId, ElementKind, FeatureId, Reference};
use silico_core::kernel::{Clients, SketchPlane};
use silico_core::sketch_states::{MemberState, SketchState};
use silico_core::sketch_tools::{
    Axes, DimensionEvent, Event, Modifiers, Offer, OfferChoice, OfferRequest,
};

use crate::selection::How;
use crate::tab::{Row, Said, said};
use bar::{DimensionShown, PaletteView, Settings, StateShown, StatusLine};
use boxing::SketchBox;
use dimension::{Ask, EditBox, Edited, ModifyAction, ModifyView, OfferView, value_of};
use drag::Drag;
use draw::{
    Curve, Drawn, DrawnSelection, Geometry, RegionFill, Scale, Shown, View, add, angle, bounds,
    dimension_preview, direction, dot, drawing, fill_scale, ghost, length, mul, preview,
    region_fills, sub, to_shapes,
};
use hit::{Hit, SketchPick, hit};
use selection::SketchSelection;
use style::*;
use tools::{Input, Tools};

/// A sketch's state as sketch mode words it (SolidWorks' five states, in the prototype's
/// words: an empty sketch "Empty", an under-defined one with its degrees of freedom left),
/// in the canvas's state colours, as the prototype has them.
pub fn state_shown(read: &SketchRead) -> StateShown {
    let empty = read.members.elements().next().is_none();
    let state = read.states.state;
    let words = match state {
        _ if empty => "Empty".to_owned(),
        SketchState::UnderDefined => format!("Under defined · {} DOF", read.states.dofs),
        SketchState::FullyDefined => "Fully defined".to_owned(),
        SketchState::OverDefined => "Over defined".to_owned(),
        SketchState::NoSolution => "No solution found".to_owned(),
        SketchState::Invalid => "Invalid solution found".to_owned(),
    };
    let colour = if empty {
        TEXT_SUBDUED
    } else {
        crate::theme::sketch_state_colour(state)
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

/// The error card sketch mode's property panel shows for the sketch open, `name`, from its
/// read (sketch.js's `renderProps`, its card for an over-defined sketch): the core's error
/// for a solve it refused (a conflict, `over_constrained`, or no solution, or one that
/// can't be built), as every error card gives the core's words, its code, and its JSON;
/// else, for a sketch solved but over defined (a driving dimension repeating others and
/// agreeing, which the core reports as a state, not an error), the prototype's card:
/// "`name` is over defined", the dimensions that repeat the others, by value and ID, and its
/// fix, "Make it driven, or delete it." None for a sketch in any other state.
pub fn problem_card(read: &SketchRead, name: &str) -> Option<Said> {
    if let Some(problem) = &read.states.solve_problem {
        return Some(Said::of(problem));
    }
    if read.states.state != SketchState::OverDefined {
        return None;
    }
    // What over-defines it, as the core decides it: on a solved sketch, its flagged
    // dimensions, of which the card names the driving ones (a reference dimension is never
    // sent to the solver, so never flagged).
    let repeating: Vec<&silico_core::document::SketchConstraint> = read
        .states
        .over_defining(&read.members)
        .filter(|c| c.form.driving() == Some(true))
        .collect();
    let cause = match repeating.as_slice() {
        // Never, as the core states it: a solved sketch is over defined only by a dimension
        // it flags. Said without a count all the same.
        [] => "A dimension repeats what the others already give.".to_owned(),
        [one] => match crate::parameters::dimension_value(&one.form) {
            Some(value) => format!("The {value} dimension repeats what the others already give."),
            None => "A dimension repeats what the others already give.".to_owned(),
        },
        many => format!(
            "{} dimensions repeat what the others already give.",
            many.len()
        ),
    };
    let suggestion = if repeating.len() > 1 {
        "Make one driven, or delete one."
    } else {
        "Make it driven, or delete it."
    };
    Some(Said {
        warning: false,
        message: format!("{name} is over defined"),
        cause: Some(cause),
        elements: repeating.iter().map(|c| c.id.as_str().to_owned()).collect(),
        elements_total: repeating.len() as u64,
        suggestion: Some(suggestion.to_owned()),
        source: None,
    })
}

/// Why the card's Make it driven is off: several driving dimensions over-define the sketch,
/// none of them left driving at its prompt here, and none of them selected.
pub const SELECT_ONE_TO_MAKE_DRIVEN: &str =
    "Several dimensions over-define the sketch: select the one to make driven";

/// What the card's Make it driven would make driven ([`SketchMode::make_driven`]): the
/// dimension, or why it's off; and its value, as the property panel writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakeDriven {
    pub target: Result<ElementId, &'static str>,
    pub value: Option<String>,
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

/// Whether a sketch's browser row, `row` (its name's response), asks for Edit Sketch by a
/// double-click, on only while `can` (fusion.md, *Sketch mode*, [esk]). The double-click is
/// the row's own (`crate::clicks`), so a click on an eye just before it doesn't turn it into
/// a triple click (#452); its clicks are counted whether or not `can`. Its context menu's
/// Edit Sketch is the feature's menu's (`crate::feature_menu`), as every feature row's is.
pub fn edit_asked(row: Option<&egui::Response>, can: bool) -> bool {
    let double = row.is_some_and(crate::clicks::double_clicked);
    can && double
}

/// A `sketch` read, answered: the revision of the model it read (none if it named none),
/// and its answer.
#[derive(Debug, Clone)]
pub struct Read {
    pub model: Option<Revision>,
    pub result: Result<SketchRead, Problems>,
}

/// Sketch mode's panels over the canvas, each's rectangle on the screen as a frame drew it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Panels {
    pub bar: egui::Rect,
    pub palette: egui::Rect,
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
    /// A right-click (the secondary button's), outside the left button's presses: Smart
    /// Dimension's, which locks the kind its preview shows (section 9); otherwise the
    /// canvas's menu's, which it opens at the click.
    RightClick,
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

/// The pane's left-button events and right clicks of one frame (`ViewFrame::left` and
/// `ViewFrame::secondary`), as sketch mode takes them: one stream in the pane's order, by
/// each event's `seq` (which puts a Cancel a navigation tool forces first, a Cancel just
/// before the Press that forced it, and a DoubleClick just after its Release); each place
/// from physical pixels to points, `at_px / pixels_per_point`, from the viewport's top left
/// at `viewport_origin`, never clamped (a drag past the viewport's edge keeps going); each
/// left kind as its action, a right click as [`PointerAction::RightClick`], with Shift and
/// Ctrl as held.
pub fn frame_events(
    left: &[crate::viewport::LeftEvent],
    secondary: &[crate::viewport::SecondaryClick],
    pixels_per_point: f32,
    viewport_origin: Pos2,
) -> Vec<PointerEvent> {
    use crate::viewport::LeftKind;
    let point = |at_px: [f32; 2]| at_points(at_px, pixels_per_point, viewport_origin);
    let mut events: Vec<(u32, PointerEvent)> = left
        .iter()
        .map(|event| {
            let action = match event.kind {
                LeftKind::Press => PointerAction::Press,
                LeftKind::Move => PointerAction::Move,
                LeftKind::Release => PointerAction::Release,
                LeftKind::DoubleClick => PointerAction::DoubleClick,
                LeftKind::Cancel => PointerAction::Cancel,
            };
            let at = point(event.at_px);
            (
                event.seq,
                PointerEvent {
                    action,
                    at,
                    shift: event.shift,
                    ctrl: event.ctrl,
                },
            )
        })
        .chain(secondary.iter().map(|click| {
            let at = point(click.at_px);
            let action = PointerAction::RightClick;
            (
                click.seq,
                PointerEvent {
                    action,
                    at,
                    shift: click.shift,
                    ctrl: click.ctrl,
                },
            )
        }))
        .collect();
    events.sort_by_key(|(seq, _)| *seq);
    events.into_iter().map(|(_, event)| event).collect()
}

/// A place the pane measures in physical pixels from the viewport's top left, `at_px`, on
/// the screen in points, the viewport's top left at `viewport_origin`: as
/// [`frame_events`] takes each event's, and the pointer's.
pub fn at_points(at_px: [f32; 2], pixels_per_point: f32, viewport_origin: Pos2) -> Pos2 {
    viewport_origin + Vec2::new(at_px[0], at_px[1]) / pixels_per_point
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

/// What a Delete of `selected` does, judged on `read`: a lone point two curves (lines or
/// arcs) end at there joins them (`sketch_join`, §9, *Split*); anything else is removed
/// (`sketch_remove`), in ID order. None with nothing selected.
pub fn deletion_of(selected: &[Hit], read: Option<&SketchRead>) -> Option<Deletion> {
    if selected.is_empty() {
        return None;
    }
    if let ([only], Some(sketch)) = (selected, read)
        && only.kind == ElementKind::Point
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
    let mut selected: Vec<&Hit> = selected.iter().collect();
    selected.sort();
    Some(Deletion::Remove(
        selected
            .into_iter()
            .map(|hit| Reference::Element(hit.id.clone()))
            .collect(),
    ))
}

/// What Delete does with the selection: removes it (`sketch_remove`), or, for a lone point
/// two pieces share, joins them (`sketch_join`, §9, *Split*).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Deletion {
    Remove(Vec<Reference>),
    Join(ElementId),
}

/// What a frame's drawing was of: the read (by how many it has taken), the selection, what's
/// shown, and the view, by where it puts four places of the plane, its scale, its chord
/// tolerance, its rectangle, and the part of the plane in it. A frame with the same key
/// draws the same: a view of the plane, flat, orthographic, or in perspective, is a
/// projective map of it, which four places, no three in a line, fix.
#[derive(Debug, Clone, PartialEq)]
struct DrawnOf {
    read: u64,
    /// The drag's solved frames drawn, while a drag's frame is drawn in the read's place.
    drag: Option<u64>,
    selected: DrawnSelection,
    shown: Shown,
    probes: [Option<Pos2>; 4],
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
    /// For Create Sketch, the name its add gives the sketch: what its Finish's step adds
    /// ("Add Sketch1"), and what its steps call it till the timeline lists it. None for Edit
    /// Sketch.
    pub created: Option<String>,
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
    /// What's selected, in pick order: members, each with its kind, and the origin and the
    /// axes; and a press under way, what's under it and whether Ctrl was held, to select on
    /// its release.
    pub selection: SketchSelection,
    pressed: Option<(Option<SketchPick>, bool)>,
    /// A press on empty space with no tool on, which a move makes a box (`boxing`).
    boxing: Option<SketchBox>,
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
    /// A placed dimension's Modify box, open; how many have opened; its edit sent, to be
    /// judged on the read of the revision it made; and the offer that judgement made (SD7),
    /// with that revision, open until it's answered or a later read comes.
    pub edit_box: Option<EditBox>,
    edit_boxes: u64,
    pub edited: Option<Edited>,
    edit_offer: Option<(Offer, Revision)>,
    /// The dimensions whose over-define prompt was answered "Leave it driving", the newest
    /// last: which one the card's Make it driven means (SolidWorks' prompt is about "this
    /// dimension", the one just placed or edited).
    left_driving: Vec<ElementId>,
    /// The revision of the read the card's Make it driven was last asked on: a second click
    /// before the read of its edit comes asks nothing (it would be refused as overtaken).
    driven_at: Option<Revision>,
    /// What it asks of the core besides its tools' requests, until the tab takes it.
    asks: Vec<Ask>,
    /// Where the canvas's right-click menu (End chain, Finish Sketch, Cancel sketch) is open,
    /// on the screen, if it is: opened by a right-click with no tool on, or a tool other
    /// than Smart Dimension, whose right-click locks its kind instead.
    pub menu_at: Option<Pos2>,
    /// The sketch bar's and the palette's rectangles on the screen, as the last frame drew
    /// them: what the bands a fit keeps its box out of are measured from
    /// (`app::sketch_inset`).
    pub panels: Option<Panels>,
    /// Where the Modify box open stands on the screen, as the last frame projected it.
    modify_screen: Option<Pos2>,
    /// The pick tolerance, in millimetres, at the last frame's view's middle: what Smart
    /// Dimension is given with a read or the Modify box, between pointer events.
    tolerance: f64,
    /// Which axes the pointer may take, as the last pointer event on the plane given to a
    /// tool found them there ([`axes_at`]): what a tool is given with a read, the Modify
    /// box, or a key, between pointer events, so Smart Dimension's hover worked out again
    /// on a read takes no axis the pointer couldn't. Both before any.
    axes: Axes,
}

impl SketchMode {
    /// Sketch mode on the sketch `sketch` (Edit Sketch), or on one still to be added (Create
    /// Sketch), numbered `serial`.
    pub fn new(serial: u64, sketch: Option<FeatureId>) -> Self {
        SketchMode {
            serial,
            sketch,
            created: None,
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
            selection: SketchSelection::default(),
            pressed: None,
            boxing: None,
            settings: Settings::default(),
            tools: Tools::default(),
            pointer: None,
            undone: 0,
            undo_floor: None,
            edit_box: None,
            edit_boxes: 0,
            edited: None,
            edit_offer: None,
            left_driving: Vec::new(),
            driven_at: None,
            asks: Vec::new(),
            modify_screen: None,
            tolerance: 0.0,
            axes: Axes::BOTH,
            menu_at: None,
            panels: None,
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

    /// `read` kept as the newest, and the selection kept to what it still holds, in its
    /// order (the origin and the axes always); Smart Dimension given it (`settle`); and an
    /// edit of a dimension judged on it, if it's of the revision the edit made, or its offer
    /// let go, if it's of a later one.
    pub(crate) fn took(&mut self, read: Read) {
        if let Ok(sketch) = &read.result {
            self.selection.retain_held(sketch);
            if let Some(edit) = &self.edit_box
                && !sketch
                    .members
                    .elements()
                    .any(|(id, _)| *id == edit.constraint)
            {
                self.edit_box = None;
            }
        }
        self.read = Some(read);
        self.taken += 1;
        if let Some(input) = tool_input(
            &self.read,
            self.sketch.as_ref(),
            &self.settings,
            self.tolerance,
            self.axes,
        ) {
            self.tools.settle(&input);
        }
        self.judge();
    }

    /// An edit of a dimension judged on the newest read, if it's of the revision the edit
    /// made (the offer the core gives there opened, if it gives one), or let go if it's of a
    /// later one; an edit's offer open let go on a read past the revision it's for.
    fn judge(&mut self) {
        let model = self.read.as_ref().and_then(|read| read.model);
        let (Some(model), Some(sketch)) = (model, self.drawable()) else {
            return;
        };
        if let Some(judged) = self.edited.as_ref().and_then(|e| e.judged(sketch, model)) {
            let made = self.edited.as_ref().and_then(|e| e.made);
            self.edited = None;
            self.edit_offer = judged.zip(made);
        }
        if self
            .edit_offer
            .as_ref()
            .is_some_and(|(_, made)| model > *made)
        {
            self.edit_offer = None;
        }
    }

    /// What Delete would do with the selection's members, the origin and the axes ignored
    /// (C9), judged on the read drawn ([`deletion_of`]): a lone point whose two curves end
    /// there, the pieces a split leaves, joined (whether `sketch_join` takes them is its own
    /// to say); anything else removed, by ID, as `sketch_remove` judges it. None with no
    /// member selected. A Delete pressed is judged again as it comes up (`Tab::judge_delete`):
    /// a lone point this read doesn't hold, on the read of the revision it's sent against.
    pub fn deletion(&self) -> Option<Deletion> {
        let selected: Vec<Hit> = self.selection.members().cloned().collect();
        deletion_of(&selected, self.drawable())
    }

    /// One frame of the viewport's: the pointer's place on the plane kept for the status
    /// bar; each press noted with what's under it (within the pick tolerance at the view's
    /// scale there), and taken as a click when it's released, or let go if it's cancelled;
    /// one on empty space moved past the drag threshold a box instead ([`boxing`]), drawn
    /// while it's held and let go as what it takes; and the sketch, and the tool's preview,
    /// drawn with `painter`, the grid over the part
    /// of the plane the painter's rectangle shows. The glyphs' and dimensions' sizes are
    /// worked out at the rectangle's centre, so they don't move with the pointer. What's
    /// drawn of the sketch is worked out again only when the read, the selection, what's
    /// shown, or the view changes, and its profile's fill only for a new read or a zoom past
    /// a power of two.
    /// The tab's drag, `dragged`, is given it: begun by a press here, moved and ended by the
    /// frame's events, and drawn in the read's place while it lasts.
    pub fn frame(&mut self, painter: &Painter, input: &FrameInput<'_>, dragged: &mut Option<Drag>) {
        if dragged.as_ref().is_some_and(Drag::over) {
            *dragged = None;
        }
        self.pointer = input.pointer.and_then(|p| (input.to_plane)(p));
        let clip = painter.clip_rect();
        let scale = scale_at(input.to_plane, clip.center()).unwrap_or(Scale(1.0));
        self.tolerance = scale.mm(SKETCH_PICK_TOLERANCE);
        // A drag's press takes the left button till it ends; then a tool on, before the
        // selection (the plan's precedence).
        let dragging = dragged.as_ref().is_some_and(Drag::held);
        let selecting: &[PointerEvent] = if self.tools.choice().is_some() && !dragging {
            self.pressed = None;
            self.boxing = None;
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
                    let found = self.found(input, event.at, dragged.as_ref());
                    if dragged.is_none() {
                        *dragged = found
                            .as_ref()
                            .and_then(SketchPick::member)
                            .and_then(|hit| self.grab(hit, input, event.at));
                    }
                    // On empty space, with no drag, maybe a box (`boxing`).
                    self.boxing = (found.is_none() && dragged.is_none())
                        .then(|| SketchBox::pressed(event.at));
                    self.pressed = Some((found, event.ctrl));
                }
                PointerAction::Move => {
                    if let Some(boxing) = self.boxing.as_mut()
                        && boxing.moved(event.at)
                    {
                        // A box, not a click: its release selects what's in it.
                        self.pressed = None;
                    }
                    if let Some(drag) = dragged.as_mut().filter(|drag| drag.held()) {
                        if !drag.moved && (event.at - drag.press).length() > SKETCH_DRAG_THRESHOLD {
                            // A drag, not a click: its release selects nothing.
                            drag.moved = true;
                            self.pressed = None;
                        }
                        if let Some(at) = (input.to_plane)(event.at) {
                            drag.cursor(at);
                        }
                    }
                }
                PointerAction::Release if dragged.as_ref().is_some_and(|d| d.held() && d.moved) => {
                    if let Some(drag) = dragged.as_mut() {
                        drag.release((input.to_plane)(event.at));
                    }
                }
                PointerAction::Release => {
                    if let Some(mut boxing) = self.boxing.take() {
                        boxing.moved(event.at);
                        if boxing.open {
                            self.pressed = None;
                            self.box_released(&boxing, event);
                        }
                    }
                    // A press that never moved is a click: no drag.
                    if dragged.as_ref().is_some_and(Drag::held) {
                        *dragged = None;
                    }
                    if let Some((found, ctrl)) = self.pressed.take() {
                        // What the press was on, if the read now still holds it: a read
                        // taken between the press and its release may have let it go.
                        let held = found.filter(|picked| {
                            picked.held_by(|id| {
                                self.drawable().is_some_and(|read| {
                                    read.members.elements().any(|(held, _)| held == id)
                                })
                            })
                        });
                        self.selection.click(held, ctrl);
                    }
                }
                PointerAction::Cancel => {
                    self.pressed = None;
                    self.boxing = None;
                    match dragged.as_mut() {
                        Some(drag) if drag.held() && drag.moved => {
                            drag.cancel();
                        }
                        Some(drag) if drag.held() => *dragged = None,
                        _ => {}
                    }
                }
                PointerAction::DoubleClick => {
                    self.edit_dimension_at(input, event.at, dragged.as_ref());
                }
                // With no tool on, the canvas's menu, at the click.
                PointerAction::RightClick => self.menu_at = Some(event.at),
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
        let mut drawn = self.drawing(&view, clip, dragged.as_ref());
        drawn.extend(preview(self.tools.preview(), &view));
        if let (Some(dimension), Some(read)) = (self.tools.dimension_preview(), self.drawable()) {
            drawn.extend(dimension_preview(
                dimension,
                read,
                &view,
                self.tools.modify_at(),
            ));
        }
        self.modify_screen = match (&self.edit_box, self.tools.modify_at()) {
            (Some(edit), _) => (input.project)(edit.at),
            (None, Some(at)) => (input.project)(at),
            (None, None) => None,
        };
        painter.extend(to_shapes(painter, &drawn));
        if let Some(boxing) = &self.boxing {
            boxing.draw(painter);
        }
    }

    /// A box let go at `event`: the selection changed by what it takes of the frame
    /// before's drawing, what the person saw, on the canvas that drawing was made on (so a
    /// release in a frame the tab isn't shown in, with nothing to clip to, is judged as it
    /// was seen), as the modifiers held at its release say (`boxing`). Nothing with no read
    /// or no drawing.
    fn box_released(&mut self, boxing: &SketchBox, event: &PointerEvent) {
        let taken = match (self.drawable(), self.drawn.as_ref()) {
            (Some(read), Some((of, drawn))) => boxing.taken(drawn, read, of.clip),
            _ => return,
        };
        self.selection
            .boxed(taken, How::of_box(event.shift, event.ctrl));
    }

    /// What's under `at` on the screen, within the pick tolerance at the view's scale there:
    /// of what's drawn, the drag's last frame while it lasts (once it has ended, until the
    /// read of its edit comes), or else the newest read. An axis may be taken only where it's
    /// drawn within the pick tolerance on the screen too ([`axis_near`]): one that isn't is
    /// no candidate, and the nearest of the rest is taken.
    fn found(
        &self,
        input: &FrameInput<'_>,
        at: Pos2,
        dragged: Option<&Drag>,
    ) -> Option<SketchPick> {
        let read = dragged.and_then(Drag::shown).or_else(|| self.drawable())?;
        let (on, scale, axes) = axes_at(input, at)?;
        hit(read, on, scale.mm(SKETCH_PICK_TOLERANCE), scale, axes)
    }

    /// A drag of `hit`, pressed at `at` on the screen: a point, or a line's, a circle's, or
    /// an arc's edge, if the newest read, of a model it names, has it under defined (section
    /// 9, *Dragging*); its press put on the exact curve (section 7: the core refuses a press
    /// more than 1e-7 mm off it, and a pick lies within the tolerance of it). None for
    /// anything else, which a press selects.
    fn grab(&self, hit: &Hit, input: &FrameInput<'_>, at: Pos2) -> Option<Drag> {
        let read = self.drawable()?;
        let sketch = self
            .sketch
            .clone()
            .filter(|sketch| *sketch == read.sketch)?;
        let model = self.read.as_ref()?.model?;
        let states = &read.states;
        let free = |state: Option<&MemberState>| state == Some(&MemberState::UnderDefined);
        let grab = match hit.kind {
            ElementKind::Point => {
                free(states.points.get(&hit.id)).then(|| DragGrab::Point(hit.id.clone()))?
            }
            ElementKind::Line | ElementKind::Circle | ElementKind::Arc => {
                let state = match hit.kind {
                    ElementKind::Line => states.lines.get(&hit.id),
                    ElementKind::Circle => states.circles.get(&hit.id),
                    _ => states.arcs.get(&hit.id),
                };
                if !free(state) {
                    return None;
                }
                let curve = Geometry::of(read).curve(&Reference::Element(hit.id.clone()))?;
                let press = on_curve(curve, (input.to_plane)(at)?)?;
                DragGrab::Edge {
                    curve: hit.id.clone(),
                    press,
                }
            }
            ElementKind::Constraint => return None,
        };
        Some(Drag::pressed(sketch, grab, read.clone(), model, at))
    }

    /// A double-click at `at` on the screen: on a driving dimension, its Modify box opened
    /// with its value (section 9: a placed dimension edited), unless an offer is open or an
    /// edit's answer is awaited. A driven one's value is measured, never typed, so it opens
    /// none, as SolidWorks edits none.
    fn edit_dimension_at(&mut self, input: &FrameInput<'_>, at: Pos2, dragged: Option<&Drag>) {
        if self.offer().is_some() || self.edit_box.as_ref().is_some_and(|edit| edit.sent) {
            return;
        }
        let Some(SketchPick::Member(found)) = self.found(input, at, dragged) else {
            return;
        };
        let Some(read) = self.drawable() else {
            return;
        };
        let Some((text, unit)) = read
            .members
            .constraints
            .iter()
            .find(|c| c.id == found.id)
            .filter(|c| c.form.driving() != Some(false))
            .and_then(|c| value_of(&c.form))
        else {
            return;
        };
        let Some(on) = (input.to_plane)(at) else {
            return;
        };
        self.edit_boxes += 1;
        self.edit_box = Some(EditBox {
            constraint: found.id,
            text,
            unit,
            at: on,
            opened: self.edit_boxes,
            sent: false,
            said: None,
        });
    }

    /// The frame's left-button events given to the tool on, in the sketch's millimetres
    /// (through `to_plane`), each with the pick tolerance where it is and the axes it may
    /// take there, as the selection finds them ([`axes_at`], kept for the events with no
    /// place on the plane and for a read or a key between them); then the pointer, if it
    /// has moved since, as a hover, with the keys `held`. A press off the plane is none
    /// of the tool's; a release off it, or a press cancelled, ends the gesture with nothing
    /// added; a double-click off it ends a chain. With no read of the sketch to work
    /// against, only a release or a cancel reaches the tool, ending its gesture.
    fn tool_events(&mut self, input: &FrameInput<'_>, held: Modifiers) {
        let on_plane =
            |at: Pos2| (input.to_plane)(at).filter(|p| p[0].is_finite() && p[1].is_finite());
        let tolerance = |at: Pos2| {
            scale_at(input.to_plane, at).map_or(0.0, |scale| scale.mm(SKETCH_PICK_TOLERANCE))
        };
        let axes = |at: Pos2, kept: Axes| axes_at(input, at).map_or(kept, |(.., axes)| axes);
        for event in input.events {
            // A right-click is Smart Dimension's lock; with any other tool, the menu's.
            if event.action == PointerAction::RightClick
                && self.tools.choice() != Some(tools::Choice::Dimension)
            {
                self.menu_at = Some(event.at);
                continue;
            }
            self.axes = axes(event.at, self.axes);
            let Some(context) = tool_input(
                &self.read,
                self.sketch.as_ref(),
                &self.settings,
                tolerance(event.at),
                self.axes,
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
                (PointerAction::RightClick, Some(at)) => {
                    tools.dimension(&context, DimensionEvent::RightClick(at));
                }
                (PointerAction::Press | PointerAction::Move | PointerAction::RightClick, None) => {}
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
        self.axes = axes(pointer, self.axes);
        if let Some(context) = tool_input(
            &self.read,
            self.sketch.as_ref(),
            &self.settings,
            tolerance(pointer),
            self.axes,
        ) {
            self.tools.handle(&context, Event::Move(at), held);
        }
    }

    /// What's drawn of the read through `view`, in `clip`: the last frame's, if nothing it's
    /// of has changed; otherwise worked out again, with the profile's fill worked out again
    /// only for a new read or another power-of-two scale.
    fn drawing(&mut self, view: &View<'_>, clip: Rect, drag: Option<&Drag>) -> Vec<Drawn> {
        // A drag's frame is drawn in the read's place, over its begin state's ghost.
        let dragged = drag.and_then(|drag| Some((drag.shown()?, drag.begin(), drag.frames)));
        let Some(read) = dragged.map(|(shown, ..)| shown).or_else(|| {
            self.read
                .as_ref()
                .and_then(|read| read.result.as_ref().ok())
        }) else {
            return Vec::new();
        };
        let of = DrawnOf {
            read: self.taken,
            drag: dragged.map(|(.., frames)| frames),
            selected: DrawnSelection::of(&self.selection),
            shown: self.shown(),
            probes: [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]].map(|p| (view.project)(p)),
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
        if dragged.is_none() && !self.fills.iter().any(|(had, _)| *had == key) {
            self.fill_builds += 1;
            self.fills
                .push((key, region_fills(read, at, view.chord_tolerance)));
            if self.fills.len() > 2 {
                self.fills.remove(0);
            }
        }
        // A drag's frame has no fill: the read's is of where it began.
        let fills = self
            .fills
            .iter()
            .find(|(had, _)| *had == key && dragged.is_none())
            .map_or(&[][..], |(_, fills)| fills.as_slice());
        let mut drawn = drawing(read, view, of.shown, &of.selected, fills);
        if let Some((shown, begin, _)) = dragged {
            let begun = drawing(begin, view, of.shown, &DrawnSelection::default(), &[]);
            let mut ghosted = ghost(&begun, begin, shown);
            ghosted.append(&mut drawn);
            drawn = ghosted;
        }
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

    /// The members' IDs selected.
    pub fn selected_ids(&self) -> BTreeSet<ElementId> {
        self.selection.members().map(|hit| hit.id.clone()).collect()
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
    pub fn palette_view(&self, revision: Revision, drag: Option<&Drag>) -> PaletteView {
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
            dimension: self
                .tools
                .dimension_preview()
                .map(|preview| DimensionShown {
                    picks: preview.picks.len(),
                    locked: preview.locked,
                    unavailable: preview.unavailable.as_ref().map(|m| m.text()),
                }),
            dragging: drag.is_some_and(|drag| drag.moved && !drag.over()),
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
    /// The offer open for a dimension that over-defines (SD7): an edit's, or Smart
    /// Dimension's for the last it placed. One at a time: neither places nor edits another
    /// while one is open.
    pub fn offer(&self) -> Option<&Offer> {
        self.edit_offer
            .as_ref()
            .map(|(offer, _)| offer)
            .or_else(|| self.tools.open_offer())
    }

    /// What the over-define prompt shows of the offer open, the sketch named `name`: the
    /// dimension's value, and the constraints the read flags, by ID.
    pub fn offer_view(&self, name: &str) -> Option<OfferView> {
        let offer = self.offer()?;
        let read = self.drawable();
        let value = read
            .and_then(|read| {
                read.members
                    .constraints
                    .iter()
                    .find(|c| c.id == offer.constraint)
            })
            .and_then(|c| value_of(&c.form))
            .map(|(text, unit)| format!("{text} {unit}"));
        let flagged = read.map_or_else(Vec::new, |read| {
            read.states
                .constraints
                .iter()
                .filter(|(_, state)| state.flagged())
                .map(|(id, _)| id.to_string())
                .collect()
        });
        Some(OfferView {
            sketch: name.to_owned(),
            value,
            flagged,
        })
    }

    /// The offer open answered with `choice` (SD7): what it sends asked, made against the
    /// revision its add or edit made (Driven's `set_field`, Cancel's `undo`; Leave's
    /// nothing), and the offer let go, so the next dimension may be placed. Whether one was
    /// open to answer.
    pub fn answer_offer(&mut self, choice: OfferChoice) -> bool {
        let (request, constraint) = if let Some((offer, _)) = self.edit_offer.take() {
            (offer.request(choice), offer.constraint)
        } else if let Some(offer) = self.tools.open_offer() {
            let answered = (offer.request(choice), offer.constraint.clone());
            self.tools.dismiss_offer();
            answered
        } else {
            return false;
        };
        if choice == OfferChoice::Leave {
            // The newest last, once: the card's Make it driven means it while it over-defines.
            self.left_driving.retain(|left| *left != constraint);
            self.left_driving.push(constraint);
        }
        self.asks.extend(request.map(Ask::Offer));
        true
    }

    /// What the card's Make it driven (sketch.js's `skMakeDriven`) would make driven, on the
    /// newest read, if the card offers it: only on a read of a published revision, for a
    /// sketch over defined or with no solution (SolidWorks' prompt repairs "the over
    /// defined or unsolvable state"), and only for a driving dimension. Which one, as
    /// SolidWorks has it (docs/ux/solidworks.md, §2; its prompt is about the dimension just
    /// placed, its right-click Driven about the one selected):
    /// - the newest dimension whose prompt was answered "Leave it driving" that still drives
    ///   and still over-defines (or the sketch has no solution, which flags nothing);
    /// - else the one driving dimension that over-defines, if there's one;
    /// - else the one of those selected, if one is;
    /// - else none: off, saying to select one.
    ///
    /// None where there's no driving dimension to offer (a conflict of relations alone).
    ///
    /// SEAM: a driven dimension made driving again (SolidWorks' right-click Driven toggle,
    /// Fusion's right-click) has no control in the approved prototype; waiting on Trevin.
    pub fn make_driven(&self) -> Option<MakeDriven> {
        let read = self.drawable()?;
        self.read.as_ref()?.model?;
        let state = read.states.state;
        if !matches!(state, SketchState::OverDefined | SketchState::NoSolution) {
            return None;
        }
        let constraints = &read.members.constraints;
        let over: Vec<&ElementId> = read
            .states
            .over_defining(&read.members)
            .filter(|c| c.form.driving() == Some(true))
            .map(|c| &c.id)
            .collect();
        let driving = |id: &ElementId| {
            constraints
                .iter()
                .any(|c| &c.id == id && c.form.driving() == Some(true))
        };
        let left = self.left_driving.iter().rev().find(|left| {
            driving(left) && (over.contains(left) || state == SketchState::NoSolution)
        });
        if left.is_none() && over.is_empty() {
            return None;
        }
        let selected: Vec<&ElementId> = self
            .selection
            .members()
            .map(|hit| &hit.id)
            .filter(|id| over.contains(id))
            .collect();
        let target = match (left, over.as_slice(), selected.as_slice()) {
            (Some(left), _, _) => Ok(left.clone()),
            (None, [one], _) | (None, _, [one]) => Ok((*one).clone()),
            (None, _, _) => Err(SELECT_ONE_TO_MAKE_DRIVEN),
        };
        let value = target.as_ref().ok().and_then(|id| {
            constraints
                .iter()
                .find(|c| &c.id == id)
                .and_then(|c| crate::parameters::dimension_value(&c.form))
        });
        Some(MakeDriven { target, value })
    }

    /// The card's Make it driven asked for `constraint`: a `set_field` of its `driving` to
    /// false asked, against the revision of the read the card was drawn from, if the newest
    /// read still offers it for that dimension and none was asked on that read already.
    /// Whether it was asked.
    pub fn ask_driven(&mut self, constraint: &ElementId) -> bool {
        let offered = self
            .make_driven()
            .is_some_and(|driven| driven.target.as_ref() == Ok(constraint));
        let (Some(sketch), Some(model)) = (
            self.sketch.clone(),
            self.read.as_ref().and_then(|read| read.model),
        ) else {
            return false;
        };
        if !offered || self.driven_at == Some(model) {
            return false;
        }
        self.driven_at = Some(model);
        self.asks
            .push(Ask::Offer(OfferRequest::driven(model, sketch, constraint)));
        true
    }

    /// What the Modify box open shows, if one is and the last frame placed it: a placed
    /// dimension's, or Smart Dimension's.
    pub fn modify_view(&self) -> Option<ModifyView> {
        let at = self.modify_screen?;
        if let Some(edit) = &self.edit_box {
            return Some(ModifyView {
                text: edit.text.clone(),
                unit: edit.unit,
                at,
                opened: edit.opened,
                said: edit.said.clone(),
                waiting: edit.sent,
                placing: false,
            });
        }
        let preview = self.tools.dimension_preview()?;
        let text = preview.modify.clone()?;
        let unit = match &preview.dimension {
            Some(silico_core::document::Form::Angle { .. }) => "°",
            _ => "mm",
        };
        Some(ModifyView {
            text,
            unit,
            at,
            // Kept apart from the placed dimensions' boxes' numbers.
            opened: u64::MAX - self.tools.modify_opened,
            said: self.tools.said().map(str::to_owned),
            waiting: false,
            placing: true,
        })
    }

    /// The Modify box open was asked `action`. A placed dimension's: its text kept as
    /// typed; accepted, read whole by the core's `read_typed`, as Smart Dimension's box is
    /// (SD9), and asked as a `set_field` of its value, its judgement to come on the read of
    /// the revision it makes (a refusal said in the box, nothing sent); skipped, closed. Smart
    /// Dimension's: given to it (`Typed`, `Enter`, or `Escape`, which places the measured
    /// value, SD2).
    pub fn modify(&mut self, action: ModifyAction) {
        if let Some(edit) = self.edit_box.as_mut() {
            match action {
                ModifyAction::Typed(text) if !edit.sent => {
                    edit.text = text;
                    edit.said = None;
                }
                ModifyAction::Typed(_) => {}
                ModifyAction::Skip => self.edit_box = None,
                ModifyAction::Accept if edit.sent => {}
                ModifyAction::Accept => {
                    let (Some(read), Some(model), Some(sketch)) = (
                        self.read.as_ref().and_then(|r| r.result.as_ref().ok()),
                        self.read.as_ref().and_then(|r| r.model),
                        self.sketch.clone(),
                    ) else {
                        return;
                    };
                    let Some(form) = read
                        .members
                        .constraints
                        .iter()
                        .find(|c| c.id == edit.constraint)
                        .map(|c| &c.form)
                    else {
                        return;
                    };
                    match dimension::edit_value(form, &edit.text) {
                        Ok(value) => {
                            edit.sent = true;
                            self.edited = Some(Edited::of(read, model, edit.constraint.clone()));
                            self.asks.push(Ask::Value {
                                sketch,
                                constraint: edit.constraint.clone(),
                                value,
                            });
                        }
                        Err(said) => edit.said = Some(said),
                    }
                }
            }
            return;
        }
        let Some(input) = tool_input(
            &self.read,
            self.sketch.as_ref(),
            &self.settings,
            self.tolerance,
            self.axes,
        ) else {
            return;
        };
        let event = match action {
            ModifyAction::Typed(text) => DimensionEvent::Typed(text),
            ModifyAction::Accept => DimensionEvent::Enter,
            ModifyAction::Skip => DimensionEvent::Escape,
        };
        self.tools.dimension(&input, event);
    }

    /// The core's answer to a placed dimension's `set_field`, the session at `made`: taken
    /// (the box closed; its judgement to come on the read of `made`, if it changed the
    /// document), or refused, said in the box, which stays open.
    pub fn value_answered(&mut self, made: Revision, result: Result<bool, String>) {
        match result {
            Ok(changed) => {
                self.edit_box = None;
                match self.edited.as_mut() {
                    Some(edited) if changed => edited.made = Some(made),
                    _ => self.edited = None,
                }
                // A read of `made` may have come already.
                self.judge();
            }
            Err(said) => {
                self.edited = None;
                if let Some(edit) = self.edit_box.as_mut() {
                    edit.sent = false;
                    edit.said = Some(said);
                }
            }
        }
    }

    /// What it asks of the core besides its tools, since this was last taken, in order.
    pub fn take_asks(&mut self) -> Vec<Ask> {
        std::mem::take(&mut self.asks)
    }

    /// The press under way let go, so its release clicks nothing: what Esc does to a press
    /// on something that would have been dragged, as it ends.
    pub fn let_go_press(&mut self) {
        self.pressed = None;
        self.boxing = None;
    }

    /// Esc, with a tool on: the gesture ended and the tool left, the tool told if the sketch
    /// is read (section 9, *Line*). Whether a tool was on to take it.
    pub fn escape_tool(&mut self) -> bool {
        let input = tool_input(
            &self.read,
            self.sketch.as_ref(),
            &self.settings,
            0.0,
            self.axes,
        );
        self.tools.escape(input.as_ref())
    }

    /// Enter, or the right-click menu's *End chain* (`event`), given to the tool on: a
    /// chain ended, the tool kept. Nothing before the sketch is read.
    pub fn end_chain(&mut self, event: Event) {
        if let Some(input) = tool_input(
            &self.read,
            self.sketch.as_ref(),
            &self.settings,
            0.0,
            self.axes,
        ) {
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

    /// The newest read's geometry as a box in model space, on its plane
    /// ([`bounds::model_box`]): what a fit takes in with the bodies in sketch mode. None
    /// before a read has placed it, or for a sketch with nothing in it.
    pub fn model_box(&self) -> Option<[[f64; 3]; 2]> {
        bounds::model_box(self.drawable()?)
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
/// it names; the pick `tolerance`, in millimetres, and the `axes` the pointer may take; and
/// the palette's settings.
fn tool_input<'a>(
    read: &'a Option<Read>,
    sketch: Option<&'a FeatureId>,
    settings: &Settings,
    tolerance: f64,
    axes: Axes,
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
        axes,
        automatic_relations: settings.automatic_relations,
        construction: settings.construction,
        trim: settings.trim,
    })
}

/// `at` put on `curve`, exactly: a line's nearest place on its segment, an arc's on its
/// sweep (past it, its nearer end), a circle's on its round; none at a round's centre, which
/// is as near every place of it, or for a place that isn't one.
pub(crate) fn on_curve(curve: Curve, at: [f64; 2]) -> Option<[f64; 2]> {
    let on = match curve {
        Curve::Line { from, to } => {
            let d = sub(to, from);
            let dd = dot(d, d);
            let t = if dd > 0.0 {
                (dot(sub(at, from), d) / dd).clamp(0.0, 1.0)
            } else {
                0.0
            };
            add(from, mul(d, t))
        }
        Curve::Round {
            centre,
            radius,
            start,
            sweep,
        } => {
            let off = sub(at, centre);
            if !(length(off) > 0.0 && length(off).is_finite()) {
                return None;
            }
            let along = (angle(off) - start).rem_euclid(std::f64::consts::TAU);
            let along = if sweep >= std::f64::consts::TAU || along <= sweep {
                along
            } else if along - sweep < std::f64::consts::TAU - along {
                sweep
            } else {
                0.0
            };
            add(centre, mul(direction(start + along), radius))
        }
    };
    (on[0].is_finite() && on[1].is_finite()).then_some(on)
}

/// Whether the axis along `along` (a unit vector through the origin) is drawn within the
/// pick tolerance of the screen's point `at`, whose place on the plane is `on`: the
/// screen's distance from `at` to the line the view draws the axis along, through its
/// place nearest `on` and a point's step (`scale`) on from there (a view draws a line of
/// the plane as a line). None near where the view draws neither. The pick's millimetres
/// are the mean of a point's step across and down ([`scale_at`]): in a view oblique to the
/// plane, they reach farther than the pick's points one way on the screen, and an axis,
/// which runs the whole view, would be taken there far off it (R3).
fn axis_near(
    project: &dyn Fn([f64; 2]) -> Option<Pos2>,
    along: [f64; 2],
    on: [f64; 2],
    at: Pos2,
    scale: Scale,
) -> bool {
    let foot = mul(along, dot(on, along));
    let (Some(a), Some(b)) = (project(foot), project(add(foot, mul(along, scale.0)))) else {
        return false;
    };
    let (ab, off) = (b - a, at - a);
    let across = if ab.length() > 0.0 {
        (ab.x * off.y - ab.y * off.x).abs() / ab.length()
    } else {
        off.length()
    };
    across <= SKETCH_PICK_TOLERANCE
}

/// Where the screen's point `at` is on the plane, the millimetres a point there
/// ([`scale_at`]), and which axes a pick there may take: each only where the view draws it
/// within the pick tolerance of `at` on the screen ([`axis_near`]). Sketch mode's selection
/// and its tools work it out alike, so for the same pointer they allow the same axes. None
/// where `at`, or a point's step from it, misses the plane.
fn axes_at(input: &FrameInput<'_>, at: Pos2) -> Option<([f64; 2], Scale, Axes)> {
    let scale = scale_at(input.to_plane, at)?;
    let on = (input.to_plane)(at)?;
    let near = |along| axis_near(input.project, along, on, at, scale);
    let axes = Axes {
        x: near([1.0, 0.0]),
        y: near([0.0, 1.0]),
    };
    Some((on, scale, axes))
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

/// Each sketch's read, kept whole: for the browser's prefixes (its state), and for what the
/// viewport draws of a shown sketch (its plane and members). Read one sketch at a time, of
/// the newest published model, each once for each revision of it, the shown sketches first,
/// so a burst of revisions refreshes what's drawn before what isn't. Each answer is kept as
/// the newest: while the next revision's read is in flight, the last is held, so nothing
/// drawn of it ever lapses between them, as every answer is drawn as of its own revision
/// (docs/design/m4.1-command-api.md, decision 33; docs/design/m5.0-app.md, section 3). An
/// answer equal to the one held keeps the held one (the same `Arc`), so what's made of it
/// needn't be made again. A read refused once it ran (a suppressed sketch, one no longer
/// there, a worker's failure) holds none. One refused before it reached a model (too many
/// published reads running at once, the session closing, no thread to be had), which names
/// none, keeps what's held, and is sent again by a later frame.
///
/// The sketch open in sketch mode isn't read here. A sketch's state is kept once, in its
/// held read, but for the open sketch's: sketch mode's read of it is newer, so its state
/// stands in for the held one's while it's open, and after Finish until it's read again
/// ([`state`](Self::state), the browser's prefix). What's drawn of a sketch in the viewport
/// is its held read's, its state with its geometry, both of one revision.
#[derive(Default)]
pub struct SketchReads {
    held: BTreeMap<FeatureId, Held>,
    /// The sketch open in sketch mode, or last open, until it's read again here: its state
    /// as sketch mode's read gives it.
    open_state: Option<(FeatureId, SketchState)>,
    asked: BTreeMap<FeatureId, Revision>,
    reading: Option<(FeatureId, Revision, Running<SketchRead>)>,
    /// How many reads it has sent: what a test counts.
    pub reads: u64,
}

/// A sketch's newest read, and the published revision it's of.
#[derive(Debug, Clone)]
pub struct Held {
    pub read: Arc<SketchRead>,
    pub revision: Revision,
}

impl SketchReads {
    /// The read in flight polled, without waiting; and, if none is, the first of `rows`
    /// (the browser's sketches) not read of the model `published`, read: the first that's
    /// `shown`, else the first of the rest. The sketch open in sketch mode, `open`, isn't
    /// read here: its state is its read's.
    pub fn poll<P: Clients + 'static>(
        &mut self,
        sessions: &Arc<Sessions<P>>,
        session: &SessionId,
        rows: &[Row],
        published: Option<Revision>,
        open: Option<&SketchMode>,
        shown: &dyn Fn(&FeatureId) -> bool,
    ) {
        let open_id = open.and_then(|mode| mode.sketch.as_ref());
        if let (Some(id), Some(read)) = (open_id, open.and_then(SketchMode::drawable)) {
            self.open_state = Some((id.clone(), read.states.state));
        }
        if let Some((id, asked, running)) = self.reading.take() {
            match running.try_take() {
                Ok(response) => {
                    // Of the model the answer says it read, or the one it was asked of.
                    let revision = response
                        .model
                        .as_ref()
                        .map_or(asked, |model| model.revision);
                    let ran = response.model.is_some();
                    // Read again here: sketch mode's state no longer stands in for it,
                    // unless it's open.
                    if ran
                        && open_id != Some(&id)
                        && self
                            .open_state
                            .as_ref()
                            .is_some_and(|(last, _)| *last == id)
                    {
                        self.open_state = None;
                    }
                    match response.result {
                        Ok(read) => {
                            let kept = self
                                .held
                                .get(&id)
                                .map(|held| &held.read)
                                .filter(|held| ***held == read)
                                .map(Arc::clone);
                            let read = kept.unwrap_or_else(|| Arc::new(read));
                            self.held.insert(id, Held { read, revision });
                        }
                        // Refused before it reached a model (too many published reads
                        // running at once, the session closing, no thread to be had): what's
                        // held stays, and it's asked again.
                        Err(_) if !ran => {
                            self.asked.remove(&id);
                        }
                        // A suppressed sketch, one no longer there, or one a worker's failure
                        // left unread: no prefix, and nothing to draw.
                        Err(_) => {
                            self.held.remove(&id);
                        }
                    }
                }
                Err(running) => {
                    self.reading = Some((id, asked, running));
                    return;
                }
            }
        }
        let listed = |id: &FeatureId| rows.iter().any(|row| &row.id == id);
        if self.open_state.as_ref().is_some_and(|(id, _)| !listed(id)) {
            self.open_state = None;
        }
        self.held.retain(|id, _| listed(id));
        self.asked.retain(|id, _| listed(id));
        let Some(published) = published else {
            return;
        };
        let unread =
            |row: &&Row| Some(&row.id) != open_id && self.asked.get(&row.id) != Some(&published);
        let next = rows
            .iter()
            .filter(unread)
            .find(|row| shown(&row.id))
            .or_else(|| rows.iter().find(unread));
        if let Some(row) = next {
            self.asked.insert(row.id.clone(), published);
            self.reads += 1;
            let running = sessions.sketch(session, ModelChoice::Published, row.id.clone());
            self.reading = Some((row.id.clone(), published, running));
        }
    }

    /// The sketch `id`'s prefix, if its state is read and has one.
    pub fn prefix(&self, id: &FeatureId) -> Option<&'static str> {
        self.state(id).and_then(prefix)
    }

    /// The sketch `id`'s state, if it's read: sketch mode's, for the sketch open there or
    /// last open until it's read again here; otherwise its held read's.
    pub fn state(&self, id: &FeatureId) -> Option<SketchState> {
        match &self.open_state {
            Some((open, state)) if open == id => Some(*state),
            _ => self.held.get(id).map(|held| held.read.states.state),
        }
    }

    /// The sketch `id`'s newest read held, if one is: its read whole, and the revision it's
    /// of.
    pub fn held(&self, id: &FeatureId) -> Option<&Held> {
        self.held.get(id)
    }

    /// The sketch whose read is in flight, if one is.
    pub fn reading(&self) -> Option<&FeatureId> {
        self.reading.as_ref().map(|(id, ..)| id)
    }
}

#[cfg(test)]
pub(crate) mod tests;
