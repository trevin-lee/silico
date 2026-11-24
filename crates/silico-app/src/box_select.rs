//! Box and crossing selection in the canvas (M5.7; docs/design/m5.0-app.md, section 9's
//! *Selection*: "a drag from empty space draws a box; left to right takes only what's
//! wholly inside, and right to left also what it crosses; in a part, a box takes edges by
//! default; Shift with a box adds, and Ctrl with a box inverts inside it", and "a drag on
//! geometry selects what's under the press, and draws no box"; SolidWorks' Box Selection
//! and Cross Selection, docs/ux/solidworks.md §1; the approved prototype's `onBox`,
//! docs/design/m5-ui/js/interact.js).
//!
//! **The press.** A left press on the canvas records what the pointer was over there, as
//! the selection's hover answered for the press's pixel ([`Selection::hovered`]). At the
//! first move farther from the press than egui's click distance (the distance the
//! selection's click is judged by, so a press is either a click or a drag, never both),
//! the drag is decided: on geometry, what's under the press is picked as a click picks it
//! (Ctrl toggling), and no box is drawn; on empty space, a box. The answer is the hover's,
//! as the selection took it ([`Selection::heard`], each frame's, kept here until the view
//! changes): to the press's pixel, never a click's answer. If the hover hasn't answered
//! for the press's pixel yet, the decision is held until it has, the pick at the press
//! asked again each frame meanwhile (as the selection's click is), in place of the
//! hover's request only, never a waiting click's; a drag let go meanwhile keeps its
//! corners and modifiers and is decided when the answer comes, as a click waits. A held
//! decision is bounded by state, never by a count of frames: it's let go when the camera
//! moves, the view's bodies change, another model is published, or the filter changes,
//! since the press's pixel no longer shows what it did; a drag let go so selects nothing,
//! and a toast says why ([`PRESS_MOVED`]), as one does if the next press comes first
//! ([`PRESS_UNREAD`]: a renderer that never answers, backing off or not installed, ends
//! there, or, as the selection's click is after its release, once its pick has been asked
//! [`CLICK_FRAMES`] frames unanswered or [`CLICK_WAIT`] has passed; and it's let go
//! unsaid if the selection is changed elsewhere meanwhile, as a waiting click is). Hiding a
//! body lets a press go too. Nothing is picked from meshes of another model than the
//! selection's. A Cancel
//! ends the press and changes nothing.
//!
//! **The box.** Dragged left to right it's a window, drawn solid: it takes what's wholly
//! inside. Right to left it's a crossing, drawn dashed: it takes what's inside or crosses
//! its outline. The box is clamped to the canvas, so nothing off the canvas is inside it.
//! Let go, it replaces the selection; with Shift it adds ([`How::Add`]); with Ctrl it
//! inverts what's inside ([`How::Toggle`]); through [`Selection::pick_all`], one change.
//! Let go in the frame the view takes another table of bodies, it selects nothing and a
//! toast says so ([`BOX_REDRAWN`]): it was drawn over the bodies the frame before drew, and
//! what's inside it is decided only against what the person saw (#568).
//! While a dialog's box of names takes the canvas's picks, a box, like a drag on geometry,
//! toggles what it takes, whatever its modifiers, and the dialog's box hears of each as it
//! hears of a click ([`Selection::take_toggled`]).
//! As the prototype's `onBox`: a box in a part takes edges, always; with the edge filter
//! off (or a dialog's box taking other kinds: [`Selection::picks`]) it takes nothing and a
//! toast says so ([`EDGE_FILTER_OFF`]); else a toast says how many edges it took
//! ("Window selection: 4 edges").
//!
//! **What's inside** is decided against the displayed mesh, what the person sees: each
//! body drawn (not hidden), its edges' polylines (and, for a box that takes faces, its
//! faces' triangles) as the view's meshes hold them, cut where the drawing cuts them and
//! projected by the renderer's own projection ([`silico_render::render::project`]), tested
//! in f64. In perspective the drawing cuts away what's nearer the eye than
//! [`MIN_EYE_DISTANCE`] along the view (#353's pyramid): each segment and triangle is cut
//! there in 3D first, and what's left is tested, so a part of an edge in front of the eye
//! counts though its other end is behind it. An edge is inside when every point of what's
//! drawn of it projects inside the box, and crosses when a drawn segment meets the box; a
//! face, when every vertex of its drawn triangles is inside, or one of them overlaps the
//! box. Nothing drawn of it, it's never taken. A point that can't be read (not finite, an
//! index past the mesh) leaves its element not wholly inside, and a crossing tests the
//! points beside it that are read. This is the mesh, not the exact B-rep: a curved edge's
//! chord can lie inside a box its true arc bulges out of; the box takes it, as what's
//! drawn is inside. A face or an edge the mesh has nothing of (a sliver OCCT couldn't
//! mesh) is never taken. What's behind other faces is taken too: the box doesn't test
//! what's seen in front, as the prototype's doesn't.
//!
//! The visuals are behind [`draw_box`], which the UI port may replace.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, Painter, Pos2, Rect, Shape, Stroke};
use silico_core::document::FeatureId;
use silico_core::naming::Name;
use silico_protocol::vec3::{dot, sub};
use silico_protocol::{Camera, MIN_EYE_DISTANCE, Projection};
use silico_render::render::{project, project_segment};

use crate::renderer::Element;
use crate::selection::{
    CLICK_FRAMES, CLICK_WAIT, How, Selection, band_px, centre_of, distance, square,
};
use crate::theme;
use crate::viewport::{LeftEvent, LeftKind, View, ViewBody, ViewFrame};

/// What a box let go with the edge filter off says (the prototype's `onBox`), selecting
/// nothing.
pub const EDGE_FILTER_OFF: &str = "The edge filter is off, and a box selects edges in a part.";

/// What a drag let go before what's under its press was read says: it selects nothing.
pub const PRESS_UNREAD: &str = "The drag selected nothing: what's under its press wasn't read yet";

/// What a drag let go after the view changed under its undecided press says.
pub const PRESS_MOVED: &str =
    "The drag selected nothing: the view changed before what's under its press was read";

/// What a box let go in the frame the view took another table of bodies says: it was drawn
/// over the bodies the frame before drew, not these, so it selects nothing (#568).
pub const BOX_REDRAWN: &str =
    "The box selected nothing: the view's meshes changed as it was let go; drag again";

/// The canvas's box selection: the left press, while one is held (or let go and waiting
/// for what's under it), what the view was the last frame, and the newest hover answer of
/// that view.
#[derive(Debug, Default)]
pub struct BoxSelect {
    press: Option<Press>,
    /// The camera, the bodies table, the model the selection is read against, and its
    /// filter, as the last frame had them: once any changes, a press undecided is let go,
    /// and the hover's answers before it are forgotten.
    seen: Option<Seen>,
    /// The newest answer the hover took since the view last changed: the pixel, and what
    /// it found there.
    heard: Option<([u32; 2], Option<Arc<Name>>)>,
}

/// What a press's answer is true of: the view, the model, and the filter.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    camera: Camera,
    table: u64,
    model: Option<(u64, silico_core::command::Revision)>,
    filter: crate::selection::Filter,
    /// The features the browser hides: hiding a body (H) changes what's under a pixel,
    /// though the bodies table stays.
    hidden: BTreeSet<FeatureId>,
}

/// A left press on the canvas.
#[derive(Debug, Clone, PartialEq)]
struct Press {
    /// Where it was pressed and where the pointer is now (or was let go), in physical
    /// pixels from the canvas's top left.
    at_px: [f32; 2],
    corner: [f32; 2],
    /// The pixel it was pressed on, as the hover's pick centres on it.
    pixel: [u32; 2],
    /// What the hover answered for the press's pixel: an element by its name, or nothing;
    /// none until it has answered.
    under: Option<Option<Arc<Name>>>,
    phase: Phase,
    /// Whether the button has been let go: a drag let go undecided waits for its answer,
    /// and is decided then, with the modifiers it was let go with.
    released: bool,
    /// Whether the view changed while it was still a click's: past the click distance, it's
    /// let go (what's under its pixel isn't what it was pressed on); let go within it, it
    /// was a click, the selection's alone.
    stale: bool,
    /// The frames its pick has been asked in since it was let go unanswered.
    asked: u32,
    /// When it was let go, by the viewport's clock, and the selection's version then.
    let_go: Option<(Instant, u64)>,
    /// The modifiers of its newest event.
    ctrl: bool,
    shift: bool,
}

/// Where a press stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Within the click distance of where it was pressed: a click so far, the selection's.
    Pressed,
    /// Past it, the hover's answer for the press's pixel not come yet.
    Held,
    /// On geometry: what was under it is picked, and no box drawn.
    Geometry,
    /// On empty space: a box.
    Boxing,
    /// Let go undecided: the view, the model, or the filter changed under it. Nothing
    /// more until its end.
    LetGo,
}

/// What a frame's events and answers ask of the selection and the view.
#[derive(Debug, Clone, PartialEq)]
enum Effect {
    /// The element under the press picked, as a click picks it.
    Pick(Arc<Name>, How),
    /// A box let go: from one corner to the other, in physical pixels, a crossing or not.
    Box {
        from: [f32; 2],
        to: [f32; 2],
        crossing: bool,
        how: How,
    },
    /// A drag let go undecided: said, nothing selected.
    Said(&'static str),
}

/// What a frame tells [`BoxSelect::step`]: the left button's events; the newest answer
/// the hover took for a pixel (the pixel, and what it found there), if one stands now;
/// whether the view, the model, or the filter changed since the last frame; the click
/// distance, in physical pixels; and the canvas's size.
struct Told<'a> {
    left: &'a [LeftEvent],
    heard: Option<([u32; 2], Option<Arc<Name>>)>,
    changed: bool,
    click_px: f32,
    size_px: [u32; 2],
    /// The frame's time, by the viewport's clock, and the selection's version (one more
    /// each time its items change).
    now: Instant,
    version: u64,
}

impl BoxSelect {
    /// The canvas's left button this frame (`frame`, given to the tab's overlay after the
    /// selection has taken its clicks and hover): a drag on geometry picked, a box drawn on
    /// `painter` and, let go, selected; the pick at the press asked again of `view` while
    /// its answer is awaited. `current` is whether the view's bodies are of the model the
    /// selection is read against (nothing is decided or selected from other bodies);
    /// `hidden`, the features the browser hides (their bodies aren't drawn). What a toast is to say,
    /// if anything: how many edges a box took, or why a drag took none.
    pub(crate) fn overlay(
        &mut self,
        selection: &mut Selection,
        view: &mut View,
        frame: &ViewFrame,
        painter: &Painter,
        current: bool,
        hidden: &BTreeSet<FeatureId>,
    ) -> Option<String> {
        let ppp = frame.pixels_per_point;
        let click_px = painter
            .ctx()
            .options(|options| options.input_options.max_click_dist)
            * ppp;
        let now = Seen {
            camera: frame.camera.clone(),
            table: view.table(),
            model: selection.model_of(),
            filter: selection.picks(),
            hidden: hidden.clone(),
        };
        let changed = self.seen.as_ref().is_some_and(|seen| *seen != now);
        self.seen = Some(now);
        if changed {
            self.heard = None;
        }
        if let Some(heard) = selection.heard() {
            self.heard = Some(heard.clone());
        }
        let effects = self.step(&Told {
            left: &frame.left,
            heard: self.heard.clone().filter(|_| current),
            changed,
            click_px,
            size_px: frame.size_px,
            now: frame.now,
            version: selection.version(),
        });
        let mut toast = None;
        for effect in effects {
            match effect {
                Effect::Pick(name, how) if current => {
                    let how = selection.canvas_how(how);
                    selection.pick(&name.to_string(), how);
                }
                // Decided over meshes of another model than the selection's: the name was
                // read from what's drawn, not what it would be read against.
                Effect::Pick(..) => toast = Some(PRESS_MOVED.to_owned()),
                Effect::Said(said) => toast = Some(said.to_owned()),
                Effect::Box { .. } if !selection.picks().edges => {
                    toast = Some(EDGE_FILTER_OFF.to_owned());
                }
                // Let go in the frame the view took another table: drawn over the bodies the
                // frame before drew, never cast on these.
                Effect::Box { .. } if view.table_taken() => {
                    tracing::debug!(
                        "a box let go in the frame a new table was taken: nothing taken"
                    );
                    toast = Some(BOX_REDRAWN.to_owned());
                }
                Effect::Box {
                    from,
                    to,
                    crossing,
                    how,
                } if current => {
                    let area = ScreenBox::clamped(from, to, frame.size_px);
                    let names = taken(
                        view.bodies(),
                        hidden,
                        &frame.camera,
                        frame.size_px,
                        area,
                        crossing,
                    );
                    tracing::debug!(crossing, ?how, taken = names.len(), "box_select");
                    toast = Some(box_words(crossing, names.len()));
                    selection.pick_all(&names, how);
                }
                Effect::Box { .. } => {
                    tracing::debug!("a box let go over meshes of another model: nothing taken");
                    toast = Some(PRESS_MOVED.to_owned());
                }
            }
        }
        if current && let Some(said) = self.ask(view, frame) {
            toast = Some(said.to_owned());
        }
        if let Some(press) = &self.press
            && press.phase == Phase::Held
            && let Some((at, _)) = press.let_go
        {
            // A frame at the backstop, as the click asks one, if nothing else draws.
            let waited = frame.now.saturating_duration_since(at);
            painter
                .ctx()
                .request_repaint_after(CLICK_WAIT.saturating_sub(waited));
        }
        if let Some(press) = &self.press
            && press.phase == Phase::Boxing
        {
            draw_box(painter, frame, press.at_px, press.corner);
        }
        toast
    }

    /// The press's pick asked again, while its answer is awaited, unless a click waiting
    /// for its own pick asked this frame: what the selection asked is replaced only if it's
    /// the hover's (its square about the pointer's pixel), or nothing was.
    ///
    /// Let go and still unanswered, it's asked as the selection's click is after its
    /// release: in at most [`CLICK_FRAMES`] frames (a renderer that answers none, backing
    /// off or not installed, would otherwise have a frame asked for without end), and then
    /// let go, which is said ([`PRESS_UNREAD`]): what's returned, once.
    fn ask(&mut self, view: &mut View, frame: &ViewFrame) -> Option<&'static str> {
        let press = self.press.as_mut()?;
        if press.under.is_some()
            || press.stale
            || !matches!(press.phase, Phase::Pressed | Phase::Held)
        {
            return None;
        }
        if press.released {
            press.asked += 1;
            if press.asked > CLICK_FRAMES {
                tracing::debug!("a drag let go, its press never answered: let go");
                self.press = None;
                return Some(PRESS_UNREAD);
            }
        }
        let band = band_px(frame.pixels_per_point);
        let hover = frame
            .pointer_px
            .map(|pointer| pixel_of(pointer, frame.size_px));
        let ours = square(press.at_px, frame.size_px, band);
        match view.asking() {
            Some(asked) if asked == ours => {}
            Some(asked) if Some(centre_of(asked)) != hover => {}
            _ => {
                view.pick(ours);
            }
        }
        None
    }

    /// The frame `told` taken: each left event in turn, the press's answer learnt once it's
    /// heard, and what that decides.
    fn step(&mut self, told: &Told<'_>) -> Vec<Effect> {
        let mut effects = Vec::new();
        // Let go and waiting, as the selection's click waits after its release: let go at
        // [`CLICK_WAIT`] by the clock (said), or once the selection is changed elsewhere
        // (Esc, a row of the browser: as a click waiting is), so a late answer never
        // replaces it.
        if let Some(press) = &self.press
            && press.phase == Phase::Held
            && let Some((at, version)) = press.let_go
        {
            if told.now.saturating_duration_since(at) >= CLICK_WAIT {
                tracing::debug!("a drag let go waited its backstop: let go");
                effects.push(Effect::Said(PRESS_UNREAD));
                self.press = None;
            } else if told.version != version {
                tracing::debug!("the selection changed under a drag let go: let go");
                self.press = None;
            }
        }
        if told.changed
            && let Some(press) = &mut self.press
        {
            match press.phase {
                Phase::Pressed => press.stale = true,
                Phase::Held => {
                    tracing::debug!("the view changed under a drag undecided: let go");
                    press.phase = Phase::LetGo;
                    if press.released {
                        effects.push(Effect::Said(PRESS_MOVED));
                        self.press = None;
                    }
                }
                Phase::Geometry | Phase::Boxing | Phase::LetGo => {}
            }
        }
        for event in told.left {
            match event.kind {
                LeftKind::Press => {
                    if self
                        .press
                        .as_ref()
                        .is_some_and(|press| press.released && press.phase == Phase::Held)
                    {
                        // A drag let go and still waiting, superseded.
                        effects.push(Effect::Said(PRESS_UNREAD));
                    }
                    self.press = Some(Press {
                        at_px: event.at_px,
                        corner: event.at_px,
                        pixel: pixel_of(event.at_px, told.size_px),
                        under: None,
                        phase: Phase::Pressed,
                        released: false,
                        stale: false,
                        asked: 0,
                        let_go: None,
                        ctrl: event.ctrl,
                        shift: event.shift,
                    });
                    self.learn(told, &mut effects);
                }
                LeftKind::Move => {
                    if let Some(press) = &mut self.press
                        && !press.released
                    {
                        press.corner = event.at_px;
                        press.ctrl = event.ctrl;
                        press.shift = event.shift;
                        if press.phase == Phase::Pressed
                            && distance(press.at_px, event.at_px) > told.click_px
                        {
                            press.phase = Phase::Held;
                        }
                    }
                    self.learn(told, &mut effects);
                }
                LeftKind::Release => {
                    let Some(press) = &mut self.press else {
                        continue;
                    };
                    if press.released {
                        continue;
                    }
                    press.corner = event.at_px;
                    press.ctrl = event.ctrl;
                    press.shift = event.shift;
                    press.released = true;
                    press.let_go = Some((told.now, told.version));
                    // Let go past the click distance with no move there between: decided
                    // now, as a move there would have.
                    if press.phase == Phase::Pressed
                        && distance(press.at_px, event.at_px) > told.click_px
                    {
                        press.phase = Phase::Held;
                    }
                    self.learn(told, &mut effects);
                }
                LeftKind::Cancel => self.press = None,
                LeftKind::DoubleClick => {}
            }
        }
        self.learn(told, &mut effects);
        effects
    }

    /// The press's answer taken if it's heard now (the newest answer the hover took is to
    /// its pixel), and, if the press is past the click distance, the drag decided by it: on
    /// geometry, its element picked as a click picks it (Ctrl toggling); on empty space, a
    /// box. A press let go is ended: a box selected as its modifiers say, or a drag let go
    /// undecided said; one let go undecided and unanswered waits.
    fn learn(&mut self, told: &Told<'_>, effects: &mut Vec<Effect>) {
        let Some(press) = &mut self.press else {
            return;
        };
        if press.stale && press.phase == Phase::Held {
            press.phase = Phase::LetGo;
        }
        if press.under.is_none()
            && !press.stale
            && let Some((pixel, under)) = &told.heard
            && *pixel == press.pixel
            && matches!(press.phase, Phase::Pressed | Phase::Held)
        {
            press.under = Some(under.clone());
        }
        if press.phase == Phase::Held {
            match &press.under {
                None => {}
                Some(Some(name)) => {
                    let how = if press.ctrl {
                        How::Toggle
                    } else {
                        How::Replace
                    };
                    effects.push(Effect::Pick(Arc::clone(name), how));
                    press.phase = Phase::Geometry;
                }
                Some(None) => press.phase = Phase::Boxing,
            }
        }
        if !press.released {
            return;
        }
        match press.phase {
            Phase::Boxing => {
                let how = How::of_box(press.shift, press.ctrl);
                effects.push(Effect::Box {
                    from: press.at_px,
                    to: press.corner,
                    crossing: press.corner[0] < press.at_px[0],
                    how,
                });
            }
            Phase::LetGo => effects.push(Effect::Said(PRESS_MOVED)),
            // Let go and still unanswered: it waits.
            Phase::Held => return,
            Phase::Pressed | Phase::Geometry => {}
        }
        self.press = None;
    }
}

/// What the toast says of a box that took `count` edges (the prototype's `onBox`).
fn box_words(crossing: bool, count: usize) -> String {
    let kind = if crossing { "Crossing" } else { "Window" };
    let plural = if count == 1 { "" } else { "s" };
    format!("{kind} selection: {count} edge{plural}")
}

/// A box on the screen, `[x0, x1] × [y0, y1]`, its sides counted inside: the canvas's in
/// physical pixels from its top left ([`ScreenBox::clamped`]), sketch mode's in screen
/// points ([`ScreenBox::in_rect`]); what it's tested against is in its units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScreenBox {
    min: [f64; 2],
    max: [f64; 2],
}

impl ScreenBox {
    /// The box with corners `a` and `b`, in screen points, clamped to `clip`, as
    /// [`ScreenBox::clamped`] takes its corners: sketch mode's box.
    pub(crate) fn in_rect(a: Pos2, b: Pos2, clip: Rect) -> ScreenBox {
        let f = f64::from;
        let (x0, x1) = side(f(a.x), f(b.x), f(clip.left()), f(clip.right()));
        let (y0, y1) = side(f(a.y), f(b.y), f(clip.top()), f(clip.bottom()));
        ScreenBox {
            min: [x0, y0],
            max: [x1, y1],
        }
    }

    /// The box with corners `a` and `b`, in physical pixels, clamped to a canvas `size`
    /// pixels (what's off the canvas isn't seen, so it's never inside). A corner that isn't
    /// finite is taken as the other's.
    fn clamped(a: [f32; 2], b: [f32; 2], size: [u32; 2]) -> ScreenBox {
        let f = f64::from;
        let (x0, x1) = side(f(a[0]), f(b[0]), 0.0, f64::from(size[0]));
        let (y0, y1) = side(f(a[1]), f(b[1]), 0.0, f64::from(size[1]));
        ScreenBox {
            min: [x0, y0],
            max: [x1, y1],
        }
    }

    /// Whether it takes an element drawn as `pieces`: wholly inside (something of it drawn,
    /// every piece read and every point inside), or, for a `crossing`, meeting it (a path's
    /// segment meeting it, or another piece's point inside).
    pub(crate) fn takes(&self, pieces: impl IntoIterator<Item = Piece>, crossing: bool) -> bool {
        let mut any = false;
        for Piece { points, read, path } in pieces {
            if points.is_empty() {
                continue;
            }
            if crossing {
                let meets = if read && path && points.len() > 1 {
                    points
                        .windows(2)
                        .any(|pair| self.meets_segment(pair[0], pair[1]))
                } else {
                    points.iter().any(|p| self.holds(*p))
                };
                if meets {
                    return true;
                }
            } else {
                if !read || !points.iter().all(|p| self.holds(*p)) {
                    return false;
                }
                any = true;
            }
        }
        !crossing && any
    }

    fn holds(&self, p: [f64; 2]) -> bool {
        self.min[0] <= p[0] && p[0] <= self.max[0] && self.min[1] <= p[1] && p[1] <= self.max[1]
    }

    fn corners(&self) -> [[f64; 2]; 4] {
        [
            self.min,
            [self.max[0], self.min[1]],
            self.max,
            [self.min[0], self.max[1]],
        ]
    }

    /// Whether the box and the box spanned by `points` share a point.
    fn overlaps_bounds(&self, points: &[[f64; 2]]) -> bool {
        (0..2).all(|k| {
            let low = points.iter().map(|p| p[k]).fold(f64::INFINITY, f64::min);
            let high = points
                .iter()
                .map(|p| p[k])
                .fold(f64::NEG_INFINITY, f64::max);
            low <= self.max[k] && self.min[k] <= high
        })
    }

    /// Whether the segment from `a` to `b` meets the box (its outline or inside): their
    /// boxes overlap, and the box's corners aren't all strictly on one side of its line
    /// (the separating axes of a segment and a box). A segment with an end inside meets it
    /// so, as does one of no length on it (every corner then on its line).
    fn meets_segment(&self, a: [f64; 2], b: [f64; 2]) -> bool {
        if !self.overlaps_bounds(&[a, b]) {
            return false;
        }
        let sides = self.corners().map(|c| orient(a, b, c));
        !(sides.iter().all(|s| *s > 0.0) || sides.iter().all(|s| *s < 0.0))
    }
}

/// A box's side from the corners' `first` and `second` along it, clamped to `low` to
/// `high` (`high` below `low`, as an empty rectangle's are, taken as `low`): a corner that
/// isn't finite taken as the other's, and with neither, `low`.
fn side(first: f64, second: f64, low: f64, high: f64) -> (f64, f64) {
    let (first, second) = match (first.is_finite(), second.is_finite()) {
        (true, true) => (first, second),
        (true, false) => (first, first),
        (false, true) => (second, second),
        (false, false) => (low, low),
    };
    let clamp = |v: f64| v.clamp(low, high.max(low));
    (clamp(first.min(second)), clamp(first.max(second)))
}

/// A piece of what's drawn of an element, as a box decides it ([`ScreenBox::takes`]): its
/// points; whether they're all read (a point that can't be read leaves the element not
/// wholly inside, and a crossing tests the points beside it); and whether they're a path,
/// a polyline whose segments a crossing tests, or places (a triangle's corners, single
/// points) it tests one by one.
pub(crate) struct Piece {
    pub(crate) points: Vec<[f64; 2]>,
    pub(crate) read: bool,
    pub(crate) path: bool,
}

/// Twice the signed area of `a`, `b`, `c`: positive if they turn one way, negative the
/// other, 0 in a line.
fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// How `camera` draws on a canvas `size` pixels: where it cuts a perspective view (its eye
/// and its view's back, depth along the view being `−back · (p − eye)`, as
/// [`project`] takes it), and where a point lands.
struct Sight<'a> {
    camera: &'a Camera,
    size: [u32; 2],
    cut: Option<([f64; 3], [f64; 3])>,
}

/// A segment or a triangle as it's drawn: wholly cut away; drawn, the projected points of
/// what's left of it after the cut; or with a point that can't be read, the projected
/// points beside it that are.
enum Drawn {
    Cut,
    Shown(Vec<[f64; 2]>),
    Unread(Vec<[f64; 2]>),
}

impl<'a> Sight<'a> {
    fn new(camera: &'a Camera, size: [u32; 2]) -> Self {
        let cut = match camera.projection {
            Projection::Perspective { .. } => camera
                .check(size[0], size[1])
                .ok()
                .map(|frame| (camera.eye, frame.back)),
            Projection::Orthographic { .. } => None,
        };
        Sight { camera, size, cut }
    }

    /// `p`'s depth along a perspective view; none in an orthographic one, which cuts
    /// nothing.
    fn depth(&self, p: [f64; 3]) -> Option<f64> {
        self.cut.map(|(eye, back)| -dot(back, sub(p, eye)))
    }

    /// Whether the drawing keeps `p` (a finite point): in front of the cut, or in a view
    /// that cuts nothing.
    fn kept(&self, p: [f64; 3]) -> bool {
        self.depth(p).is_none_or(|depth| depth >= MIN_EYE_DISTANCE)
    }

    /// `points`, a segment or one point of an edge, as drawn: cut to its part in front of
    /// the cut, then projected. A segment is cut and projected by the renderer's own rule
    /// ([`project_segment`], as Select Other's cast and the drawing cut it: about the eye, so
    /// where it's cut is as precise as the eye is, wherever the eye is).
    fn drawn(&self, points: &[[f64; 3]]) -> Drawn {
        let read: Vec<[f64; 3]> = points
            .iter()
            .copied()
            .filter(|p| p.iter().all(|c| c.is_finite()))
            .collect();
        if read.len() < points.len() {
            let beside = read
                .iter()
                .filter_map(|p| project(self.camera, self.size, *p))
                .collect();
            return Drawn::Unread(beside);
        }
        let left: Vec<[f64; 3]> = match read.as_slice() {
            [a, b] => {
                return match project_segment(self.camera, self.size, *a, *b) {
                    None => Drawn::Cut,
                    Some([Some(a), Some(b)]) => Drawn::Shown(vec![a, b]),
                    Some(ends) => Drawn::Unread(ends.into_iter().flatten().collect()),
                };
            }
            points => points.iter().copied().filter(|p| self.kept(*p)).collect(),
        };
        if left.is_empty() {
            return Drawn::Cut;
        }
        let projected: Vec<Option<[f64; 2]>> = left
            .iter()
            .map(|p| project(self.camera, self.size, *p))
            .collect();
        if projected.iter().any(Option::is_none) {
            return Drawn::Unread(projected.into_iter().flatten().collect());
        }
        Drawn::Shown(projected.into_iter().flatten().collect())
    }
}

/// What's decided of an element from its drawn pieces (segments or triangles): whether
/// it's wholly inside `area` (something of it drawn, all of it read and inside), or, for a
/// crossing, whether it meets it ([`ScreenBox::takes`]).
fn decide(pieces: impl Iterator<Item = Drawn>, area: &ScreenBox, crossing: bool) -> bool {
    let pieces = pieces.filter_map(|piece| match piece {
        Drawn::Cut => None,
        Drawn::Shown(points) => {
            let path = points.len() == 2;
            Some(Piece {
                points,
                read: true,
                path,
            })
        }
        Drawn::Unread(points) => Some(Piece {
            points,
            read: false,
            path: false,
        }),
    });
    area.takes(pieces, crossing)
}

/// The names of the edges a box `area` takes of `bodies`, but those `hidden`, seen through
/// `camera` on a canvas `size` pixels: wholly inside, or also crossing it if `crossing`. By
/// body in its number's order, then each edge by its number: as the element map names
/// them. A box in a part takes edges alone (the prototype's `onBox`); a box that took faces
/// (a filter for them, M5.7's part 3 or later) would add their triangles here, cut and
/// tested as the edges' segments are.
fn taken(
    bodies: &BTreeMap<u32, ViewBody>,
    hidden: &BTreeSet<FeatureId>,
    camera: &Camera,
    size: [u32; 2],
    area: ScreenBox,
    crossing: bool,
) -> Vec<String> {
    let sight = Sight::new(camera, size);
    let mut names = Vec::new();
    for body in bodies
        .values()
        .filter(|body| !hidden.contains(&body.feature))
    {
        let mesh = &body.mesh;
        let mut first = 0usize;
        for (count, number) in mesh.edge_point_counts.iter().zip(1u32..) {
            let count = usize::try_from(*count).unwrap_or(usize::MAX);
            let end = first.saturating_add(count);
            let points = mesh.edge_points.get(first..end);
            first = end;
            let Some(points) = points.filter(|p| !p.is_empty()) else {
                continue;
            };
            let take = if let [point] = points {
                decide(std::iter::once(sight.drawn(&[*point])), &area, crossing)
            } else {
                let pieces = points.windows(2).map(|pair| sight.drawn(pair));
                decide(pieces, &area, crossing)
            };
            if take && let Some(named) = body.name_of(Element::Edge(number)) {
                names.push(named.name.to_string());
            }
        }
    }
    names
}

/// The box from `from` to `to` (physical pixels from the canvas's top left) drawn on
/// `painter` over `frame`'s canvas: [`theme::BOX_SELECT_FILL`], outlined in
/// [`theme::BOX_SELECT_STROKE`], solid for a window (dragged left to right) and dashed for a
/// crossing (right to left). The painter clips it to the canvas. A box of no size, or with
/// a corner that isn't finite, draws nothing.
pub(crate) fn draw_box(painter: &Painter, frame: &ViewFrame, from: [f32; 2], to: [f32; 2]) {
    // egui's pixels per point are above 0; one that isn't leaves a corner that isn't
    // finite, which draws nothing.
    let ppp = frame.pixels_per_point;
    let at = |p: [f32; 2]| frame.rect.min + egui::vec2(p[0] / ppp, p[1] / ppp);
    draw_rect(painter, at(from), at(to));
}

/// The box from `from` to `to`, in screen points: filled, and outlined solid as a window
/// (dragged left to right) or dashed as a crossing (right to left). Nothing for corners
/// that aren't finite, or one box of no size.
pub(crate) fn draw_rect(painter: &Painter, from: Pos2, to: Pos2) {
    if !(from.x.is_finite() && from.y.is_finite() && to.x.is_finite() && to.y.is_finite())
        || from == to
    {
        return;
    }
    let rect = Rect::from_two_pos(from, to);
    let stroke = Stroke::new(
        theme::BOX_SELECT_STROKE_WIDTH,
        theme::color32(theme::BOX_SELECT_STROKE),
    );
    painter.rect_filled(rect, 0.0, theme::color32(theme::BOX_SELECT_FILL));
    let outline = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    if to.x < from.x {
        let [dash, gap] = theme::BOX_SELECT_CROSSING_DASH;
        painter.extend(Shape::dashed_line(&outline, stroke, dash, gap));
    } else {
        painter.add(Shape::line(outline.to_vec(), stroke));
    }
}

/// The pixel `at_px` (physical pixels from the canvas's top left, maybe outside it) is in,
/// clamped into a canvas `size` pixels, as the selection's pick centres its square on it.
fn pixel_of(at_px: [f32; 2], size: [u32; 2]) -> [u32; 2] {
    centre_of(square(at_px, size, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use silico_core::command::meshes::MeshElement;
    use silico_core::naming::{PrimitiveKind, TypeWord};
    use silico_core::recompute::Key;
    use silico_protocol::{ElementKind, ElementRef, Mesh, Projection};

    const BODY: &str = "k7f2qx3m";

    /// From −y onto the origin, z up, orthographic, 128 mm high, on a canvas 256 by 128
    /// pixels: 1 pixel a millimetre, x right from the canvas's middle (128) and z up from it
    /// (64), so `(x, y, z)` lands at `(128 + x, 64 − z)` whatever its y. Its scales are
    /// powers of two, so a pixel given in quarters lands exactly there, in f32 and f64.
    fn front() -> Camera {
        Camera {
            projection: Projection::Orthographic { view_height: 128.0 },
            eye: [0.0, -100.0, 0.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
        }
    }

    const SIZE: [u32; 2] = [256, 128];

    /// The edge band about a press, in pixels at one pixel a point: the prototype's 6
    /// points (viewport.js, `d < 6`).
    const BAND: u32 = 6;

    /// The document's point that [`front`] lands on pixel `(px, py)`, at depth `y`.
    fn at(px: f64, py: f64, y: f64) -> [f64; 3] {
        [px - 128.0, y, 64.0 - py]
    }

    fn named(kind: ElementKind, index: u32) -> MeshElement {
        MeshElement {
            name: Arc::new(Name::Element {
                feature: FeatureId::new(BODY).unwrap(),
                made: TypeWord::Primitive(PrimitiveKind::Box),
                element: ElementRef { kind, index },
            }),
            stable: true,
            count: None,
        }
    }

    /// A body of `edges` (polylines, by their pixels on [`front`]), each named
    /// `k7f2qx3m.box.Edge<n>` in order.
    fn body(edges: &[&[[f64; 2]]]) -> ViewBody {
        let mut mesh = Mesh {
            positions: Vec::new(),
            normals: Vec::new(),
            triangles: Vec::new(),
            face_triangles: Vec::new(),
            valid: None,
            edge_points: Vec::new(),
            edge_point_counts: Vec::new(),
            vertex_points: vec![],
        };
        for edge in edges {
            mesh.edge_points
                .extend(edge.iter().map(|[x, y]| at(*x, *y, 0.0)));
            mesh.edge_point_counts.push(edge.len() as u32);
        }
        ViewBody {
            feature: FeatureId::new(BODY).unwrap(),
            key: Key::parse(&"01".repeat(32)).unwrap(),
            level: 0.01,
            deflection: 0.01,
            bounds: None,
            serves: [0.0, 1.0],
            held: false,
            faces: Vec::new(),
            edges: (1..=edges.len() as u32)
                .map(|n| named(ElementKind::Edge, n))
                .collect(),
            mesh: Arc::new(mesh),
            vertices: Vec::new(),
        }
    }

    fn bodies(body: ViewBody) -> BTreeMap<u32, ViewBody> {
        BTreeMap::from([(1, body)])
    }

    /// No body hidden.
    fn none() -> BTreeSet<FeatureId> {
        BTreeSet::new()
    }

    /// What a box from `a` to `b` (pixels) takes of `bodies` on [`front`], window or
    /// crossing as dragged (right to left a crossing), as short names (`Edge1`).
    fn boxed(bodies: &BTreeMap<u32, ViewBody>, a: [f32; 2], b: [f32; 2]) -> Vec<String> {
        let area = ScreenBox::clamped(a, b, SIZE);
        taken(bodies, &none(), &front(), SIZE, area, b[0] < a[0])
            .into_iter()
            .map(|name| name.trim_start_matches("k7f2qx3m.box.").to_owned())
            .collect()
    }

    #[test]
    fn the_front_camera_lands_points_where_the_tests_say() {
        // The tests' pixels by hand rest on this: (128 + x, 64 − z), exactly.
        for (point, pixel) in [
            ([10.0, 0.0, 5.0], [138.0, 59.0]),
            ([-128.0, 30.0, 64.0], [0.0, 0.0]),
            ([25.5, -3.0, -12.25], [153.5, 76.25]),
            ([-18.0, 0.0, 39.0], [110.0, 25.0]),
        ] {
            assert_eq!(project(&front(), SIZE, point), Some(pixel), "{point:?}");
        }
    }

    #[test]
    fn a_window_takes_the_edge_wholly_inside_and_a_crossing_also_the_one_it_touches() {
        // Edge1 from (110, 40) to (130, 40), wholly in the box (105, 25)–(140, 45); Edge2
        // from (125, 30) to (160, 30), half out; Edge3 from (100, 90) to (120, 90), below;
        // Edge4 from (100, 35) to (150, 35), through the box with no point in it.
        let edges: [&[[f64; 2]]; 4] = [
            &[[110.0, 40.0], [130.0, 40.0]],
            &[[125.0, 30.0], [160.0, 30.0]],
            &[[100.0, 90.0], [120.0, 90.0]],
            &[[100.0, 35.0], [150.0, 35.0]],
        ];
        let held = bodies(body(&edges));
        // Left to right: a window.
        assert_eq!(boxed(&held, [105.0, 25.0], [140.0, 45.0]), ["Edge1"]);
        // The same box right to left: a crossing, also Edge2, which is half in, and Edge4,
        // which runs through it (none of its points is inside).
        assert_eq!(
            boxed(&held, [140.0, 25.0], [105.0, 45.0]),
            ["Edge1", "Edge2", "Edge4"]
        );
        // Up or down doesn't matter, only left or right.
        assert_eq!(boxed(&held, [105.0, 45.0], [140.0, 25.0]), ["Edge1"]);
    }

    #[test]
    fn an_edge_on_the_boxs_outline_is_inside_and_one_a_hair_out_isnt() {
        // The box's sides count as inside: Edge1 along its top, (110, 25)–(130, 25); Edge2
        // a thousandth of a pixel above it.
        let edges: [&[[f64; 2]]; 2] = [
            &[[110.0, 25.0], [130.0, 25.0]],
            &[[110.0, 24.999], [130.0, 24.999]],
        ];
        let held = bodies(body(&edges));
        assert_eq!(boxed(&held, [105.0, 25.0], [140.0, 45.0]), ["Edge1"]);
        // A crossing doesn't reach it either: it's outside, if barely.
        assert_eq!(boxed(&held, [140.0, 25.0], [105.0, 45.0]), ["Edge1"]);
    }

    #[test]
    fn a_window_tests_the_displayed_polyline_not_the_exact_arc_near_its_side() {
        // An arc of radius 10 about pixel (100, 50), from −30° to 30°, displayed as its
        // chord: (100 + 10 cos 30°, 50 ∓ 5), x = 100 + 5√3 = 108.660…. The arc bulges to x = 110
        // at 0°. A window to x = 109.5 holds the chord, though the arc runs 0.5 past it:
        // what's drawn is inside, so the box takes it (a test of the exact B-rep wouldn't).
        // To 108.5 it holds neither.
        let x = 100.0 + 5.0 * 3f64.sqrt();
        let edges: [&[[f64; 2]]; 1] = [&[[x, 45.0], [x, 55.0]]];
        let held = bodies(body(&edges));
        assert_eq!(boxed(&held, [100.0, 40.0], [109.5, 60.0]), ["Edge1"]);
        assert!(boxed(&held, [100.0, 40.0], [108.5, 60.0]).is_empty());
    }

    #[test]
    fn a_boxs_toast_counts_its_edges_as_the_prototype_words_it() {
        // The prototype's `onBox`.
        assert_eq!(box_words(false, 4), "Window selection: 4 edges");
        assert_eq!(box_words(true, 1), "Crossing selection: 1 edge");
        assert_eq!(box_words(false, 0), "Window selection: 0 edges");
    }

    #[test]
    fn a_segment_crosses_a_box_by_its_line_not_by_its_bounds_or_its_ends() {
        // The box (100, 20)–(140, 40), dragged right to left. Edge1, from (135, 10) to
        // (150, 25) (x − y = 125), passes outside its corner (140, 20) (x − y = 120): their
        // bounds overlap, but every corner is on one side of its line, so it isn't taken.
        // Edge2 and Edge3 run along its top and its foot, both ends outside: on its outline,
        // so taken, whichever side of them the other corners lie.
        let edges: [&[[f64; 2]]; 3] = [
            &[[135.0, 10.0], [150.0, 25.0]],
            &[[90.0, 20.0], [150.0, 20.0]],
            &[[90.0, 40.0], [150.0, 40.0]],
        ];
        let held = bodies(body(&edges));
        assert_eq!(
            boxed(&held, [140.0, 20.0], [100.0, 40.0]),
            ["Edge2", "Edge3"]
        );
    }

    #[test]
    fn a_hidden_body_and_an_unmeshed_edge_are_never_taken() {
        let edges: [&[[f64; 2]]; 2] = [&[], &[[110.0, 30.0], [130.0, 30.0]]];
        let held = bodies(body(&edges));
        // Edge1 has nothing in the mesh: not taken, by a window or a crossing; Edge2 is.
        assert_eq!(boxed(&held, [100.0, 10.0], [160.0, 50.0]), ["Edge2"]);
        assert_eq!(boxed(&held, [160.0, 10.0], [100.0, 50.0]), ["Edge2"]);
        // Its body hidden: nothing; another feature hidden: as before.
        let area = ScreenBox::clamped([100.0, 10.0], [160.0, 50.0], SIZE);
        let hidden = BTreeSet::from([FeatureId::new(BODY).unwrap()]);
        assert!(taken(&held, &hidden, &front(), SIZE, area, false).is_empty());
        let other = BTreeSet::from([FeatureId::new("a91cw0de").unwrap()]);
        assert_eq!(
            taken(&held, &other, &front(), SIZE, area, false),
            ["k7f2qx3m.box.Edge2"]
        );
    }

    /// In perspective from (0, −100, 0) onto the origin, z up, 45° high: depth along the
    /// view is y + 100, and the drawing cuts what's nearer than `MIN_EYE_DISTANCE`.
    fn from_near() -> Camera {
        Camera {
            projection: Projection::Perspective {
                field_of_view: 45.0,
            },
            eye: [0.0, -100.0, 0.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
        }
    }

    /// A body of `edges` in the document's coordinates, named in order.
    fn body_at(edges: &[&[[f64; 3]]]) -> BTreeMap<u32, ViewBody> {
        let mut held = body(&[]);
        let mut mesh = (*held.mesh).clone();
        for edge in edges {
            mesh.edge_points.extend(edge.iter().copied());
            mesh.edge_point_counts.push(edge.len() as u32);
        }
        held.mesh = Arc::new(mesh);
        held.edges = (1..=edges.len() as u32)
            .map(|n| named(ElementKind::Edge, n))
            .collect();
        bodies(held)
    }

    #[test]
    fn what_the_drawing_keeps_in_front_of_the_eye_is_tested_and_whats_behind_isnt() {
        // Edge1, from (−50, 0, 0), 100 in front of the eye, to (50, −150, 0), 50 behind
        // it: drawn from its start to where it meets the cut, two thirds along, which lands
        // far off to the right; it runs along the canvas's middle row (z = 0, y 64) through
        // x 128 halfway (the origin's x at depth 25 is 0). Edge2 lies wholly behind the eye.
        // Edge3 runs along the view, from behind the eye to the origin: what's drawn of it
        // lands on the canvas's middle, (128, 64).
        let camera = from_near();
        assert_eq!(project(&camera, SIZE, [50.0, -150.0, 0.0]), None);
        let edges: [&[[f64; 3]]; 3] = [
            &[[-50.0, 0.0, 0.0], [50.0, -150.0, 0.0]],
            &[[0.0, -200.0, 0.0], [10.0, -150.0, 5.0]],
            &[[0.0, -200.0, 0.0], [0.0, 0.0, 0.0]],
        ];
        let held = body_at(&edges);
        let names = |area: ScreenBox, crossing| -> Vec<String> {
            taken(&held, &none(), &camera, SIZE, area, crossing)
                .into_iter()
                .map(|name| name.trim_start_matches("k7f2qx3m.box.").to_owned())
                .collect()
        };
        // The reviewer's box, (190, 60) to (150, 68), right to left: Edge1's drawn part
        // crosses it, though its far end doesn't project.
        let right = ScreenBox::clamped([190.0, 60.0], [150.0, 68.0], SIZE);
        assert_eq!(names(right, true), ["Edge1"]);
        // A window over the whole canvas: Edge3's drawn part is inside; Edge1's runs off
        // the canvas to its cut; Edge2 has nothing drawn.
        let whole = ScreenBox::clamped([0.0, 0.0], [256.0, 128.0], SIZE);
        assert_eq!(names(whole, false), ["Edge3"]);
        assert_eq!(names(whole, true), ["Edge1", "Edge3"]);
        // Off the middle, left of x 50: nothing drawn there.
        let aside = ScreenBox::clamped([0.0, 0.0], [40.0, 128.0], SIZE);
        assert!(names(aside, true).is_empty());
    }

    #[test]
    fn a_box_is_clamped_to_the_canvas_and_a_corner_that_isnt_finite_is_the_others() {
        assert_eq!(
            ScreenBox::clamped([150.0, 50.0], [500.0, -20.0], SIZE),
            ScreenBox {
                min: [150.0, 0.0],
                max: [256.0, 50.0]
            }
        );
        assert_eq!(
            ScreenBox::clamped([150.0, 50.0], [f32::NAN, f32::INFINITY], SIZE),
            ScreenBox {
                min: [150.0, 50.0],
                max: [150.0, 50.0]
            }
        );
        // Wholly off the canvas: a line along its side, which holds only what's on it.
        assert_eq!(
            ScreenBox::clamped([-50.0, 10.0], [-10.0, 20.0], SIZE),
            ScreenBox {
                min: [0.0, 10.0],
                max: [0.0, 20.0]
            }
        );
        // An edge off the canvas, past its right side, isn't inside a box dragged past it.
        let edges: [&[[f64; 2]]; 2] = [
            &[[246.0, 30.0], [266.0, 30.0]],
            &[[150.0, 30.0], [255.0, 30.0]],
        ];
        let held = bodies(body(&edges));
        assert_eq!(boxed(&held, [140.0, 10.0], [400.0, 50.0]), ["Edge2"]);
    }

    // ---- The press, decided ----

    /// Box `k7f2qx3m`'s element `kind` `index` by its name (`k7f2qx3m.box.Face3`).
    fn name(kind: ElementKind, index: u32) -> Arc<Name> {
        Arc::clone(&named(kind, index).name)
    }

    fn event(kind: LeftKind, at_px: [f32; 2]) -> LeftEvent {
        LeftEvent {
            kind,
            at_px,
            shift: false,
            ctrl: false,
            frame: 0,
            time: 0.0,
            seq: 0,
        }
    }

    fn with(mut event: LeftEvent, shift: bool, ctrl: bool) -> LeftEvent {
        event.shift = shift;
        event.ctrl = ctrl;
        event
    }

    /// One frame of `left`, the answer for `heard` in, at 6 pixels' click distance.
    fn frame(
        boxes: &mut BoxSelect,
        left: &[LeftEvent],
        heard: Option<([u32; 2], Option<Arc<Name>>)>,
        changed: bool,
    ) -> Vec<Effect> {
        boxes.step(&Told {
            left,
            heard,
            changed,
            click_px: 6.0,
            size_px: SIZE,
            now: start(),
            version: 0,
        })
    }

    fn phase(boxes: &BoxSelect) -> Option<Phase> {
        boxes.press.as_ref().map(|press| press.phase)
    }

    /// The tests' clock's start: one instant, so every frame of a test is at it unless
    /// it says otherwise.
    fn start() -> Instant {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *START.get_or_init(Instant::now)
    }

    /// [`frame`] at `now`, the selection at `version`.
    fn frame_at(
        boxes: &mut BoxSelect,
        left: &[LeftEvent],
        heard: Option<([u32; 2], Option<Arc<Name>>)>,
        now: Instant,
        version: u64,
    ) -> Vec<Effect> {
        boxes.step(&Told {
            left,
            heard,
            changed: false,
            click_px: 6.0,
            size_px: SIZE,
            now,
            version,
        })
    }

    #[test]
    fn a_drag_let_go_waits_at_most_click_wait_and_not_past_a_selection_set_elsewhere() {
        let left = [
            event(Press, [50.0, 40.0]),
            event(Move, [70.0, 40.0]),
            event(Release, [90.0, 70.0]),
        ];
        let mut boxes = BoxSelect::default();
        assert!(frame_at(&mut boxes, &left, None, start(), 0).is_empty());
        // Just short of the click's backstop: still waiting.
        let short = start() + CLICK_WAIT - std::time::Duration::from_millis(1);
        assert!(frame_at(&mut boxes, &[], None, short, 0).is_empty());
        assert_eq!(phase(&boxes), Some(Phase::Held));
        // At it: let go, and said.
        assert_eq!(
            frame_at(&mut boxes, &[], None, start() + CLICK_WAIT, 0),
            [Effect::Said(PRESS_UNREAD)]
        );
        assert_eq!(phase(&boxes), None);
        // The selection changed elsewhere meanwhile (Esc, a row of the browser): let go,
        // unsaid, and its answer after decides nothing.
        let mut boxes = BoxSelect::default();
        frame_at(&mut boxes, &left, None, start(), 7);
        assert!(frame_at(&mut boxes, &[], None, start(), 8).is_empty());
        assert_eq!(phase(&boxes), None);
        assert!(frame_at(&mut boxes, &[], Some(([50, 40], None)), start(), 8).is_empty());
        // Unchanged, the answer decides it.
        let mut boxes = BoxSelect::default();
        frame_at(&mut boxes, &left, None, start(), 7);
        assert_eq!(
            frame_at(&mut boxes, &[], Some(([50, 40], None)), start(), 7).len(),
            1
        );
    }

    /// What a frame of the overlay did: its toast, the pick it asked, the shapes it
    /// painted, and the frame it asked for.
    struct Overlaid {
        toast: Option<String>,
        asked: Option<crate::renderer::PixelRect>,
        shapes: Vec<Shape>,
        repaint: std::time::Duration,
    }

    /// `boxes`' overlay, in a pass of egui's, over a fresh view (whose pick asked is the
    /// frame's alone) of `left`, at `ppp` pixels a point, the view's bodies `current`.
    fn overlaid_at(
        boxes: &mut BoxSelect,
        selection: &mut Selection,
        left: Vec<LeftEvent>,
        current: bool,
        ppp: f32,
    ) -> Overlaid {
        overlaid_in(boxes, selection, left, current, ppp, false)
    }

    /// [`overlaid_at`], the frame `taken` another table of bodies or not.
    fn overlaid_in(
        boxes: &mut BoxSelect,
        selection: &mut Selection,
        left: Vec<LeftEvent>,
        current: bool,
        ppp: f32,
        taken: bool,
    ) -> Overlaid {
        let ctx = egui::Context::default();
        // Passes first until egui asks for no frame of its own (its first frames do), so the
        // one measured asks for the overlay's alone.
        for _ in 0..8 {
            let output = pass(&ctx, |_| {});
            if output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .is_none_or(|viewport| viewport.repaint_delay == std::time::Duration::MAX)
            {
                break;
            }
        }
        let mut view = View::default();
        if taken {
            view.begin_frame();
            view.tabled_again();
        }
        let mut frame = view_frame(None);
        frame.left = left;
        frame.now = start();
        frame.pixels_per_point = ppp;
        let mut toast = None;
        let output = pass(&ctx, |ui| {
            let painter = ui.ctx().layer_painter(egui::LayerId::background());
            toast = boxes.overlay(selection, &mut view, &frame, &painter, current, &none());
        });
        Overlaid {
            toast,
            asked: view.asking(),
            shapes: flat(output.shapes.into_iter().map(|clipped| clipped.shape)),
            repaint: output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .map_or(std::time::Duration::MAX, |viewport| viewport.repaint_delay),
        }
    }

    /// A pass of egui's running `run`, its textures' changes let go (no renderer takes them).
    fn pass(ctx: &egui::Context, run: impl FnMut(&mut egui::Ui)) -> egui::FullOutput {
        let mut output = ctx.run_ui(egui::RawInput::default(), run);
        output.textures_delta.clear();
        output
    }

    fn flat(shapes: impl IntoIterator<Item = Shape>) -> Vec<Shape> {
        let mut out = Vec::new();
        for shape in shapes {
            match shape {
                Shape::Vec(inner) => out.extend(flat(inner)),
                shape => out.push(shape),
            }
        }
        out
    }

    /// [`overlaid_at`] at one pixel a point: its toast and the pick it asked.
    fn overlaid(
        boxes: &mut BoxSelect,
        selection: &mut Selection,
        left: Vec<LeftEvent>,
        current: bool,
    ) -> (Option<String>, Option<crate::renderer::PixelRect>) {
        let done = overlaid_at(boxes, selection, left, current, 1.0);
        (done.toast, done.asked)
    }

    #[test]
    fn through_the_overlay_a_drag_let_go_unanswered_stops_asking_and_says_so_once() {
        // A renderer that answers none: after its release, the press's pick is asked in
        // CLICK_FRAMES frames (each asked frame is one the viewport asks to draw), then
        // never again, and the toast says so once.
        let mut boxes = BoxSelect::default();
        let mut selection = Selection::default();
        let (toast, asked) = overlaid(
            &mut boxes,
            &mut selection,
            vec![
                event(Press, [50.0, 40.0]),
                event(Move, [70.0, 40.0]),
                event(Release, [90.0, 70.0]),
            ],
            true,
        );
        assert_eq!((toast, asked.is_some()), (None, true));
        let mut asking = 1;
        let mut toasts = Vec::new();
        for _ in 0..3 * CLICK_FRAMES {
            let (toast, asked) = overlaid(&mut boxes, &mut selection, Vec::new(), true);
            asking += u32::from(asked.is_some());
            toasts.extend(toast);
        }
        assert_eq!(asking, CLICK_FRAMES);
        assert_eq!(toasts, [PRESS_UNREAD]);
    }

    #[test]
    fn a_drag_let_go_waiting_keeps_its_answer_while_a_name_waits_on_the_model() {
        // Issue #489. The box's model published before its names are, its Edge1 selected
        // and waiting: each frame the tab carries the selection to the model (asking Edge1
        // again, `Tab::poll`) before the overlay (`Tab::overlay`). A drag let go before its press is
        // answered waits for that answer across those frames, as it does with nothing
        // waiting (the selection is the same); answered (nothing under the press), it's a
        // window, and its toast says so.
        let model = Arc::new(crate::selection::tests::box_model().unnamed());
        let revision = silico_core::command::Revision::from_number(1).unwrap();
        let carry = |selection: &mut Selection| {
            selection.published(Some((Arc::clone(&model), revision)), 1);
        };
        let mut selection = Selection::default();
        carry(&mut selection);
        selection.pick("aaaaaaaa.box.Edge1", How::Replace);
        assert_eq!(selection.names(), ["aaaaaaaa.box.Edge1"]);
        let mut boxes = BoxSelect::default();
        carry(&mut selection);
        let left = vec![
            event(Press, [50.0, 40.0]),
            event(Move, [70.0, 40.0]),
            event(Release, [90.0, 70.0]),
        ];
        let (toast, _) = overlaid(&mut boxes, &mut selection, left, true);
        assert_eq!(toast, None);
        assert_eq!(phase(&boxes), Some(Phase::Held));
        for frame in 0..3 {
            carry(&mut selection);
            let (toast, asked) = overlaid(&mut boxes, &mut selection, Vec::new(), true);
            assert_eq!(toast, None, "frame {frame}");
            assert_eq!(phase(&boxes), Some(Phase::Held), "frame {frame}: let go");
            assert!(asked.is_some(), "frame {frame}: its pick not asked");
        }
        // Its answer comes: nothing under the press.
        boxes.heard = Some(([50, 40], None));
        carry(&mut selection);
        let (toast, _) = overlaid(&mut boxes, &mut selection, Vec::new(), true);
        assert_eq!(toast.as_deref(), Some("Window selection: 0 edges"));
        assert_eq!(phase(&boxes), None);
    }

    #[test]
    fn a_drag_decided_on_geometry_over_another_models_meshes_picks_nothing() {
        // A press that knew Face3 under it, decided in a frame whose bodies aren't of the
        // selection's model: nothing picked (the selection, with no model, would say why
        // it couldn't), and the toast says the view changed.
        let mut boxes = BoxSelect::default();
        let mut selection = Selection::default();
        overlaid(&mut boxes, &mut selection, Vec::new(), false);
        boxes.press = Some(super::Press {
            at_px: [50.0, 40.0],
            corner: [50.0, 40.0],
            pixel: [50, 40],
            under: Some(Some(name(ElementKind::Face, 3))),
            phase: Phase::Pressed,
            released: false,
            stale: false,
            asked: 0,
            let_go: None,
            ctrl: false,
            shift: false,
        });
        let (toast, _) = overlaid(
            &mut boxes,
            &mut selection,
            vec![event(Move, [70.0, 40.0])],
            false,
        );
        assert_eq!(toast.as_deref(), Some(PRESS_MOVED));
        assert_eq!(selection.said(), None);
        assert!(selection.is_empty());
    }

    #[test]
    fn a_box_let_go_in_the_frame_a_new_table_is_taken_selects_nothing_and_says_so() {
        // #568: a box dragged over the bodies one frame drew, let go in the frame the view
        // took another table of them: what's inside it is never decided against those it
        // wasn't drawn over. Let go a frame later, over the bodies it was, it's a window.
        let boxing = || super::Press {
            at_px: [50.0, 40.0],
            corner: [90.0, 70.0],
            pixel: [50, 40],
            under: Some(None),
            phase: Phase::Boxing,
            released: false,
            stale: false,
            asked: 0,
            let_go: None,
            ctrl: false,
            shift: false,
        };
        for taken in [true, false] {
            let mut boxes = BoxSelect::default();
            let mut selection = Selection::default();
            overlaid(&mut boxes, &mut selection, Vec::new(), true);
            boxes.press = Some(boxing());
            let done = overlaid_in(
                &mut boxes,
                &mut selection,
                vec![event(Release, [90.0, 70.0])],
                true,
                1.0,
                taken,
            );
            let said = if taken {
                BOX_REDRAWN
            } else {
                "Window selection: 0 edges"
            };
            assert_eq!(done.toast.as_deref(), Some(said), "taken: {taken}");
            assert_eq!(phase(&boxes), None, "taken: {taken}");
            assert!(selection.is_empty());
            assert_eq!(selection.said(), None);
        }
    }

    use LeftKind::{Cancel, Move, Press, Release};

    #[test]
    fn a_drag_on_geometry_picks_what_was_under_the_press_and_draws_no_box() {
        let face = name(ElementKind::Face, 3);
        let mut boxes = BoxSelect::default();
        // Pressed at (50.5, 40.2), pixel (50, 40), whose hover found Face3.
        let heard = Some(([50, 40], Some(Arc::clone(&face))));
        assert!(
            frame(
                &mut boxes,
                &[event(Press, [50.5, 40.2])],
                heard.clone(),
                false
            )
            .is_empty()
        );
        // Within 6 pixels: still a click's.
        assert!(frame(&mut boxes, &[event(Move, [56.0, 40.2])], None, false).is_empty());
        assert_eq!(phase(&boxes), Some(Phase::Pressed));
        // Past it: Face3 picked, replacing, and no box.
        assert_eq!(
            frame(&mut boxes, &[event(Move, [56.6, 40.2])], None, false),
            [Effect::Pick(Arc::clone(&face), How::Replace)]
        );
        assert_eq!(phase(&boxes), Some(Phase::Geometry));
        assert!(frame(&mut boxes, &[event(Move, [90.0, 70.0])], None, false).is_empty());
        assert!(frame(&mut boxes, &[event(Release, [90.0, 70.0])], None, false).is_empty());
        assert_eq!(phase(&boxes), None);
        // With Ctrl, as a Ctrl-click: toggled.
        let left = [
            event(Press, [50.5, 40.2]),
            with(event(Move, [70.0, 40.0]), false, true),
        ];
        assert_eq!(
            frame(&mut boxes, &left, heard, false),
            [Effect::Pick(face, How::Toggle)]
        );
    }

    #[test]
    fn a_drag_from_empty_space_boxes_and_its_release_selects_by_its_modifiers() {
        let mut boxes = BoxSelect::default();
        let empty = Some(([50, 40], None));
        let drag = |boxes: &mut BoxSelect, to: [f32; 2], shift, ctrl| {
            frame(
                boxes,
                &[event(Press, [50.0, 40.0]), event(Move, to)],
                empty.clone(),
                false,
            );
            assert_eq!(phase(boxes), Some(Phase::Boxing));
            frame(boxes, &[with(event(Release, to), shift, ctrl)], None, false)
        };
        let effect = |to: [f32; 2], crossing, how| Effect::Box {
            from: [50.0, 40.0],
            to,
            crossing,
            how,
        };
        assert_eq!(
            drag(&mut boxes, [90.0, 70.0], false, false),
            [effect([90.0, 70.0], false, How::Replace)]
        );
        assert_eq!(
            drag(&mut boxes, [10.0, 70.0], false, false),
            [effect([10.0, 70.0], true, How::Replace)]
        );
        assert_eq!(
            drag(&mut boxes, [90.0, 10.0], true, false),
            [effect([90.0, 10.0], false, How::Add)]
        );
        assert_eq!(
            drag(&mut boxes, [10.0, 10.0], false, true),
            [effect([10.0, 10.0], true, How::Toggle)]
        );
        // Both: Shift's, as the prototype's onBox takes them.
        assert_eq!(
            drag(&mut boxes, [90.0, 70.0], true, true),
            [effect([90.0, 70.0], false, How::Add)]
        );
        // Straight down, x unchanged: a window.
        assert_eq!(
            drag(&mut boxes, [50.0, 90.0], false, false),
            [effect([50.0, 90.0], false, How::Replace)]
        );
    }

    #[test]
    fn a_drag_whose_press_isnt_answered_yet_is_held_until_it_is() {
        let mut boxes = BoxSelect::default();
        // Pressed with no answer for its pixel (the newest is another's), and dragged past
        // the click distance: held, nothing decided or drawn.
        let other = Some(([3, 3], Some(name(ElementKind::Face, 1))));
        let left = [event(Press, [50.0, 40.0]), event(Move, [70.0, 40.0])];
        assert!(frame(&mut boxes, &left, other.clone(), false).is_empty());
        assert_eq!(phase(&boxes), Some(Phase::Held));
        for _ in 0..100 {
            assert!(
                frame(
                    &mut boxes,
                    &[event(Move, [80.0, 50.0])],
                    other.clone(),
                    false
                )
                .is_empty()
            );
        }
        assert_eq!(phase(&boxes), Some(Phase::Held));
        // Answered, nothing there: a box, from the press to where the pointer is.
        assert!(frame(&mut boxes, &[], Some(([50, 40], None)), false).is_empty());
        assert_eq!(phase(&boxes), Some(Phase::Boxing));
        assert_eq!(
            frame(&mut boxes, &[event(Release, [85.0, 55.0])], None, false),
            [Effect::Box {
                from: [50.0, 40.0],
                to: [85.0, 55.0],
                crossing: false,
                how: How::Replace,
            }]
        );
        // Again, answered with an edge there: picked.
        let edge = name(ElementKind::Edge, 5);
        frame(&mut boxes, &left, None, false);
        assert_eq!(
            frame(
                &mut boxes,
                &[],
                Some(([50, 40], Some(Arc::clone(&edge)))),
                false
            ),
            [Effect::Pick(edge, How::Replace)]
        );
        // The answer can come in the frame it's let go: decided first, then let go.
        frame(&mut boxes, &left, None, false);
        assert_eq!(
            frame(
                &mut boxes,
                &[event(Release, [20.0, 40.0])],
                Some(([50, 40], None)),
                false
            ),
            [Effect::Box {
                from: [50.0, 40.0],
                to: [20.0, 40.0],
                crossing: true,
                how: How::Replace,
            }]
        );
    }

    #[test]
    fn a_drag_let_go_unanswered_waits_with_its_corners_and_is_decided_when_answered() {
        let mut boxes = BoxSelect::default();
        let left = [event(Press, [50.0, 40.0]), event(Move, [70.0, 40.0])];
        // Let go with Shift before its press is answered: nothing yet, and it waits.
        frame(&mut boxes, &left, None, false);
        let release = with(event(Release, [90.0, 70.0]), true, false);
        assert!(frame(&mut boxes, &[release], None, false).is_empty());
        for _ in 0..50 {
            assert!(frame(&mut boxes, &[], None, false).is_empty());
        }
        assert_eq!(phase(&boxes), Some(Phase::Held));
        // Answered, nothing there: the box it was let go as, Shift's.
        assert_eq!(
            frame(&mut boxes, &[], Some(([50, 40], None)), false),
            [Effect::Box {
                from: [50.0, 40.0],
                to: [90.0, 70.0],
                crossing: false,
                how: How::Add,
            }]
        );
        assert_eq!(phase(&boxes), None);
        // Answered with a face there: picked, as a drag on geometry.
        let face = name(ElementKind::Face, 2);
        frame(&mut boxes, &left, None, false);
        frame(&mut boxes, &[event(Release, [90.0, 70.0])], None, false);
        assert_eq!(
            frame(
                &mut boxes,
                &[],
                Some(([50, 40], Some(Arc::clone(&face)))),
                false
            ),
            [Effect::Pick(face, How::Replace)]
        );
        assert_eq!(phase(&boxes), None);
    }

    #[test]
    fn a_drag_undecided_is_let_go_when_the_view_changes_and_says_so() {
        let mut boxes = BoxSelect::default();
        let left = [event(Press, [50.0, 40.0]), event(Move, [70.0, 40.0])];
        // Held, then the camera moves (or the bodies, the model, or the filter change): let
        // go, an answer after it decides nothing, and its release says why.
        frame(&mut boxes, &left, None, false);
        assert!(frame(&mut boxes, &[], None, true).is_empty());
        assert_eq!(phase(&boxes), Some(Phase::LetGo));
        assert!(frame(&mut boxes, &[], Some(([50, 40], None)), false).is_empty());
        assert_eq!(
            frame(&mut boxes, &[event(Release, [90.0, 70.0])], None, false),
            [Effect::Said(PRESS_MOVED)]
        );
        assert_eq!(phase(&boxes), None);
        // Let go and waiting, then the change: said at once.
        frame(&mut boxes, &left, None, false);
        frame(&mut boxes, &[event(Release, [90.0, 70.0])], None, false);
        assert_eq!(
            frame(&mut boxes, &[], None, true),
            [Effect::Said(PRESS_MOVED)]
        );
        assert_eq!(phase(&boxes), None);
        // Let go and waiting, then the next press: said, and the new press begins.
        frame(&mut boxes, &left, None, false);
        frame(&mut boxes, &[event(Release, [90.0, 70.0])], None, false);
        assert_eq!(
            frame(&mut boxes, &[event(Press, [10.0, 10.0])], None, false),
            [Effect::Said(PRESS_UNREAD)]
        );
        assert_eq!(phase(&boxes), Some(Phase::Pressed));
        frame(&mut boxes, &[event(Cancel, [10.0, 10.0])], None, false);
        // A press still a click's when the view changes, its answer known before: past the
        // click distance it's let go (its pixel's answer isn't what it was pressed on), and
        // an answer of the new view for its pixel isn't taken.
        let empty = Some(([50, 40], None));
        frame(
            &mut boxes,
            &[event(Press, [50.0, 40.0])],
            empty.clone(),
            false,
        );
        frame(&mut boxes, &[], None, true);
        assert!(
            frame(
                &mut boxes,
                &[event(Move, [90.0, 40.0])],
                empty.clone(),
                false
            )
            .is_empty()
        );
        assert_eq!(phase(&boxes), Some(Phase::LetGo));
        frame(&mut boxes, &[event(Release, [90.0, 40.0])], None, false);
        // Let go within the click distance, it was a click: nothing said, the selection's.
        frame(
            &mut boxes,
            &[event(Press, [50.0, 40.0])],
            empty.clone(),
            false,
        );
        frame(&mut boxes, &[], None, true);
        assert!(
            frame(
                &mut boxes,
                &[event(Release, [51.0, 40.0])],
                empty.clone(),
                false
            )
            .is_empty()
        );
        // A box already drawn isn't let go: it's of the canvas, and selects at its release.
        frame(&mut boxes, &left, empty, false);
        frame(&mut boxes, &[], None, true);
        assert_eq!(phase(&boxes), Some(Phase::Boxing));
    }

    /// A frame of the view: 256 by 128 pixels, at one a point, the pointer at `pointer`.
    fn view_frame(pointer: Option<[f32; 2]>) -> ViewFrame {
        ViewFrame {
            session: silico_core::command::SessionId::parse("s1").unwrap(),
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(256.0, 128.0)),
            camera: front(),
            size_px: SIZE,
            pixels_per_point: 1.0,
            pointer_px: pointer,
            left: Vec::new(),
            secondary: Vec::new(),
            drawn: Vec::new(),
            now: std::time::Instant::now(),
        }
    }

    #[test]
    fn a_drag_let_go_unanswered_is_asked_as_a_click_is_then_let_go_and_said_once() {
        // Let go before its press is answered, with a renderer that answers none: asked in
        // CLICK_FRAMES frames, as the selection's click is after its release, so frames
        // aren't asked for without end; then let go, said once, and nothing more asked.
        let mut boxes = BoxSelect::default();
        let left = [
            event(Press, [50.0, 40.0]),
            event(Move, [70.0, 40.0]),
            event(Release, [90.0, 70.0]),
        ];
        assert!(frame(&mut boxes, &left, None, false).is_empty());
        let ours = square([50.0, 40.0], SIZE, BAND);
        for n in 1..=CLICK_FRAMES {
            let mut view = View::default();
            assert_eq!(boxes.ask(&mut view, &view_frame(None)), None, "frame {n}");
            assert_eq!(view.asking(), Some(ours), "frame {n}");
        }
        let mut view = View::default();
        assert_eq!(boxes.ask(&mut view, &view_frame(None)), Some(PRESS_UNREAD));
        assert_eq!(view.asking(), None);
        assert_eq!(phase(&boxes), None);
        for _ in 0..10 {
            assert_eq!(boxes.ask(&mut view, &view_frame(None)), None);
            assert_eq!(view.asking(), None);
        }
        // Held, not let go: asked for as long as the button is (the person holds it).
        let mut boxes = BoxSelect::default();
        frame(&mut boxes, &left[..2], None, false);
        for _ in 0..2 * CLICK_FRAMES {
            let mut view = View::default();
            assert_eq!(boxes.ask(&mut view, &view_frame(None)), None);
            assert_eq!(view.asking(), Some(ours));
        }
    }

    #[test]
    fn the_presss_pick_takes_the_hovers_place_never_a_waiting_clicks() {
        let mut boxes = BoxSelect::default();
        frame(&mut boxes, &[event(Press, [50.0, 40.0])], None, false);
        let ours = square([50.0, 40.0], SIZE, BAND);
        let at = |pixel: [f32; 2]| square(pixel, SIZE, BAND);
        let pointer = [80.5, 70.5];
        // A click's pick asked this frame, about another pixel than the pointer's: kept.
        let mut view = View::default();
        view.pick(at([20.0, 20.0]));
        boxes.ask(&mut view, &view_frame(Some(pointer)));
        assert_eq!(view.asking(), Some(at([20.0, 20.0])));
        // So is one with the pointer off the canvas, which the hover never asks.
        boxes.ask(&mut view, &view_frame(None));
        assert_eq!(view.asking(), Some(at([20.0, 20.0])));
        // The hover's, about the pointer's pixel: replaced by the press's.
        let mut view = View::default();
        view.pick(at(pointer));
        boxes.ask(&mut view, &view_frame(Some(pointer)));
        assert_eq!(view.asking(), Some(ours));
        // Nothing asked: the press's.
        let mut view = View::default();
        boxes.ask(&mut view, &view_frame(Some(pointer)));
        assert_eq!(view.asking(), Some(ours));
        // Answered: nothing more asked.
        frame(&mut boxes, &[], Some(([50, 40], None)), false);
        let mut view = View::default();
        boxes.ask(&mut view, &view_frame(Some(pointer)));
        assert_eq!(view.asking(), None);
    }

    #[test]
    fn a_cancel_ends_the_press_and_selects_nothing() {
        let mut boxes = BoxSelect::default();
        let empty = Some(([50, 40], None));
        let left = [
            event(Press, [50.0, 40.0]),
            event(Move, [90.0, 70.0]),
            event(Cancel, [90.0, 70.0]),
        ];
        assert!(frame(&mut boxes, &left, empty.clone(), false).is_empty());
        assert_eq!(phase(&boxes), None);
        // A stray release after it is nothing.
        assert!(frame(&mut boxes, &[event(Release, [90.0, 70.0])], empty, false).is_empty());
    }

    #[test]
    fn a_release_past_the_click_distance_with_no_move_between_is_a_drag() {
        let mut boxes = BoxSelect::default();
        let empty = Some(([50, 40], None));
        let left = [event(Press, [50.0, 40.0]), event(Release, [50.0, 46.5])];
        assert_eq!(
            frame(&mut boxes, &left, empty.clone(), false),
            [Effect::Box {
                from: [50.0, 40.0],
                to: [50.0, 46.5],
                crossing: false,
                how: How::Replace,
            }]
        );
        // Within it: a click, the selection's alone.
        let left = [event(Press, [50.0, 40.0]), event(Release, [50.0, 46.0])];
        assert!(frame(&mut boxes, &left, empty, false).is_empty());
    }

    // ---- Hostile input (break-it) ----

    #[test]
    fn a_malformed_mesh_takes_nothing_it_cant_read_and_never_panics() {
        // A count past the points, a point that isn't finite: an edge with a point that
        // can't be read isn't wholly inside; a crossing tests what it can read.
        let mut held = body(&[]);
        held.mesh = Arc::new(Mesh {
            edge_points: vec![
                at(110.0, 30.0, 0.0),
                [f64::NAN, 0.0, 0.0],
                at(120.0, 30.0, 0.0),
                at(130.0, 30.0, 0.0),
            ],
            edge_point_counts: vec![2, 2, u32::MAX],
            ..(*held.mesh).clone()
        });
        held.edges = (1..=3).map(|n| named(ElementKind::Edge, n)).collect();
        let held = bodies(held);
        // Edge1 has a point that isn't finite; Edge2 is whole; Edge3 runs past the points.
        assert_eq!(boxed(&held, [100.0, 10.0], [160.0, 50.0]), ["Edge2"]);
        // A crossing takes Edge1 by its point that projects.
        assert_eq!(
            boxed(&held, [160.0, 10.0], [100.0, 50.0]),
            ["Edge1", "Edge2"]
        );
        // Fewer names than edges: what has no name isn't taken.
        let mut fewer = held[&1].clone();
        fewer.edges.truncate(1);
        assert_eq!(
            boxed(&bodies(fewer), [160.0, 10.0], [100.0, 50.0]),
            ["Edge1"]
        );
    }

    #[test]
    fn an_empty_scene_a_box_of_no_size_and_a_camera_that_cant_draw_take_nothing() {
        let edges: [&[[f64; 2]]; 1] = [&[[110.0, 30.0], [130.0, 30.0]]];
        let held = bodies(body(&edges));
        assert!(boxed(&BTreeMap::new(), [0.0, 0.0], [256.0, 128.0]).is_empty());
        // A box of no size takes only what's exactly on its point: an edge of one point
        // there, not one through it.
        assert!(boxed(&held, [120.0, 30.0], [120.0, 30.0]).is_empty());
        let single: [&[[f64; 2]]; 1] = [&[[120.0, 31.0]]];
        assert_eq!(
            boxed(&bodies(body(&single)), [120.0, 31.0], [120.0, 31.0]),
            ["Edge1"]
        );
        // A camera the renderer refuses, or a canvas of no size: nothing projects.
        let mut camera = front();
        camera.eye = camera.target;
        let area = ScreenBox::clamped([0.0, 0.0], [256.0, 128.0], SIZE);
        assert!(taken(&held, &none(), &camera, SIZE, area, true).is_empty());
        assert!(taken(&held, &none(), &front(), [0, 0], area, true).is_empty());
    }

    #[test]
    fn a_box_over_a_large_model_takes_every_edge_inside_once_in_order() {
        // 100,000 edges, each a two-point polyline half a pixel long, in a 200 by 100 grid
        // of pixels a quarter in: a window to x = 99.9 takes those wholly in it, in their
        // order.
        let mut held = body(&[]);
        let count = 100_000u32;
        let mut points = Vec::with_capacity(2 * count as usize);
        for n in 0..count {
            let x = f64::from(n % 200) + 0.25;
            let y = f64::from(n / 200 % 100) + 0.25;
            points.push(at(x, y, 0.0));
            points.push(at(x + 0.5, y, 0.0));
        }
        held.mesh = Arc::new(Mesh {
            edge_points: points,
            edge_point_counts: vec![2; count as usize],
            ..(*held.mesh).clone()
        });
        held.edges = (1..=count).map(|n| named(ElementKind::Edge, n)).collect();
        let held = bodies(held);
        let names = boxed(&held, [0.0, 0.0], [99.9, 100.0]);
        // By hand: x from 0 to 99 (the edge's end at x + 0.75 ≤ 99.9), each of the 100 rows,
        // over the 5 times the grid repeats: 100 × 100 × 5.
        assert_eq!(names.len(), 50_000);
        assert_eq!(names[0], "Edge1");
        assert_eq!(names[1], "Edge2");
        assert_eq!(names[100], "Edge201");
    }

    #[test]
    fn a_press_held_without_a_release_and_a_second_press_starts_again() {
        // A press whose release never came (the window lost one): the next press begins
        // afresh, the first's decision dropped.
        let mut boxes = BoxSelect::default();
        let empty = Some(([50, 40], None));
        frame(
            &mut boxes,
            &[event(Press, [50.0, 40.0]), event(Move, [90.0, 70.0])],
            empty,
            false,
        );
        assert_eq!(phase(&boxes), Some(Phase::Boxing));
        frame(&mut boxes, &[event(Press, [10.0, 10.0])], None, false);
        assert_eq!(phase(&boxes), Some(Phase::Pressed));
        // Positions that aren't finite never pass the click distance, nor decide.
        assert!(
            frame(
                &mut boxes,
                &[event(Move, [f32::NAN, f32::INFINITY])],
                None,
                false
            )
            .is_empty()
        );
        assert_eq!(phase(&boxes), Some(Phase::Pressed));
        assert!(frame(&mut boxes, &[event(Release, [f32::NAN, 0.0])], None, false).is_empty());
        assert_eq!(phase(&boxes), None);
    }

    // ---- Each operand, decided (the mutants run's survivors) ----

    #[test]
    fn a_segment_wholly_on_either_side_of_a_corner_or_beside_a_side_isnt_taken() {
        // The box (100, 20)–(140, 40), right to left. Edge1, (150, 25) to (135, 10), runs
        // the other way along x − y = 125 (every corner on its other side). Edge2, (130, 10)
        // to (150, 30), on x − y = 120, touches the corner (140, 20) exactly: taken. Edge3,
        // (90, 10) to (150, 10), parallel to its top 10 above it.
        let edges: [&[[f64; 2]]; 3] = [
            &[[150.0, 25.0], [135.0, 10.0]],
            &[[130.0, 10.0], [150.0, 30.0]],
            &[[90.0, 10.0], [150.0, 10.0]],
        ];
        let held = bodies(body(&edges));
        assert_eq!(boxed(&held, [140.0, 20.0], [100.0, 40.0]), ["Edge2"]);
    }

    #[test]
    fn a_segment_is_cut_where_the_eye_cuts_at_its_hand_worked_point() {
        // From the eye at (0, −100, 0), depth is y + 100. The segment from (−50, 0, 0),
        // depth 100, to (50, −200, 0), depth −100, meets the cut, depth 1e-6, at t =
        // (100 − 1e-6) / 200: (−5e-7, −99.999999, 0), x half its depth left: a pixel of the
        // 128 rows is 2 tan 22.5° / 128 of the depth, so column 128 − 64 · 0.5 / tan 22.5° =
        // 50.75, row 64.
        let camera = from_near();
        let sight = Sight::new(&camera, SIZE);
        // (The point written in decimals rounds a hair nearer the eye than the cut, so
        // `project` itself refuses it: the pixel is worked here.)
        let landed = [128.0 - 64.0 * 0.5 / libm::tan(22.5f64.to_radians()), 64.0];
        assert!((landed[0] - 50.745).abs() < 1e-3);
        // Drawn, the segment is its start to that point; the other way round, the same.
        for (from, to) in [
            ([-50.0, 0.0, 0.0], [50.0, -200.0, 0.0]),
            ([50.0, -200.0, 0.0], [-50.0, 0.0, 0.0]),
        ] {
            let Drawn::Shown(points) = sight.drawn(&[from, to]) else {
                panic!("not drawn")
            };
            assert_eq!(points.len(), 2);
            let (kept, cut) = if from[1] == 0.0 {
                (points[0], points[1])
            } else {
                (points[1], points[0])
            };
            assert_eq!(Some(kept), project(&camera, SIZE, [-50.0, 0.0, 0.0]));
            assert!(
                (cut[0] - landed[0]).abs() < 1e-3 && (cut[1] - landed[1]).abs() < 1e-6,
                "{cut:?}, not {landed:?}"
            );
        }
    }

    #[test]
    fn a_segment_cut_near_an_eye_far_from_the_origin_lands_where_the_drawing_cuts_it() {
        // The eye at (2^36, 0, 0), about 6.9e10 mm out (well inside what the renderer
        // draws, where f64's step is 1.5e-5 mm), looking along −z, 90° high.
        // The segment from (2^36 − 0.25, 0, 1), behind the eye, to (2^36 + 0.25, 0, −1), in
        // front of it, runs through the eye: seen end on, every point of what's drawn of it
        // lands on one pixel, x a quarter of its depth, column 128 + 64 · 0.25 = 144, row 64.
        // It meets the cut, depth 1e-6, 2.5e-7 mm right of the eye: about the eye that's
        // exact, but at 2^36 it rounds to the eye's own x, which would land the cut at the
        // middle, column 128, and draw it as a line from 128 to 144.
        let x = 68_719_476_736.0_f64;
        let camera = Camera {
            projection: Projection::Perspective {
                field_of_view: 90.0,
            },
            eye: [x, 0.0, 0.0],
            target: [x, 0.0, -10.0],
            up: [0.0, 1.0, 0.0],
        };
        let sight = Sight::new(&camera, SIZE);
        let Drawn::Shown(points) = sight.drawn(&[[x - 0.25, 0.0, 1.0], [x + 0.25, 0.0, -1.0]])
        else {
            panic!("not drawn")
        };
        for point in &points {
            assert!(
                (point[0] - 144.0).abs() < 1e-3 && (point[1] - 64.0).abs() < 1e-3,
                "{points:?}"
            );
        }
        // So a crossing box from column 132 to 140 about its row doesn't take it, and one
        // from 140 to 148 does.
        let edges: [&[[f64; 3]]; 1] = [&[[x - 0.25, 0.0, 1.0], [x + 0.25, 0.0, -1.0]]];
        let held = body_at(&edges);
        let crossing = |from: f32, to: f32| -> Vec<String> {
            let area = ScreenBox::clamped([to, 60.0], [from, 68.0], SIZE);
            taken(&held, &none(), &camera, SIZE, area, true)
        };
        assert!(crossing(132.0, 140.0).is_empty());
        assert_eq!(crossing(140.0, 148.0), ["k7f2qx3m.box.Edge1"]);
    }

    #[test]
    fn a_point_exactly_at_the_cut_is_drawn_and_one_a_hair_nearer_isnt() {
        // From the eye at the origin along +y, depth is y exactly.
        let camera = Camera {
            projection: Projection::Perspective {
                field_of_view: 45.0,
            },
            eye: [0.0, 0.0, 0.0],
            target: [0.0, 1.0, 0.0],
            up: [0.0, 0.0, 1.0],
        };
        let sight = Sight::new(&camera, SIZE);
        let at_cut = [0.0, MIN_EYE_DISTANCE, 0.0];
        let short = [0.0, MIN_EYE_DISTANCE * (1.0 - 1e-9), 0.0];
        assert!(matches!(sight.drawn(&[at_cut]), Drawn::Shown(_)));
        assert!(matches!(sight.drawn(&[short]), Drawn::Cut));
        // A segment from the short point to one far in front: drawn from the cut on.
        let Drawn::Shown(points) = sight.drawn(&[short, [0.0, 10.0, 0.0]]) else {
            panic!("not drawn")
        };
        assert_eq!(points.len(), 2);
        // Wholly short: cut.
        assert!(matches!(sight.drawn(&[short, short]), Drawn::Cut));
    }

    #[test]
    fn the_click_distance_is_egui_s_in_points_scaled_to_pixels() {
        // egui's 6 points: at 2 pixels a point, 12 pixels. A move of 10 is still a click's;
        // one of 12.5 decides, the answer known empty: a box.
        let empty = Some(([50, 40], None));
        for (moved, decided) in [(10.0, false), (12.5, true)] {
            let mut boxes = BoxSelect {
                heard: empty.clone(),
                ..BoxSelect::default()
            };
            let mut selection = Selection::default();
            overlaid_at(
                &mut boxes,
                &mut selection,
                vec![event(Press, [50.0, 40.0])],
                true,
                2.0,
            );
            overlaid_at(
                &mut boxes,
                &mut selection,
                vec![event(Move, [50.0 + moved, 40.0])],
                true,
                2.0,
            );
            assert_eq!(phase(&boxes) == Some(Phase::Boxing), decided, "{moved}");
        }
    }

    #[test]
    fn a_box_let_go_over_another_models_meshes_takes_nothing_and_says_why() {
        // Drawn over the selection's model's meshes; let go in a frame whose meshes are of
        // another: nothing taken (the selection, with no model, would say why it couldn't),
        // and the toast says the view changed.
        let mut boxes = BoxSelect {
            heard: Some(([50, 40], None)),
            ..BoxSelect::default()
        };
        let mut selection = Selection::default();
        let left = vec![event(Press, [50.0, 40.0]), event(Move, [90.0, 70.0])];
        overlaid_at(&mut boxes, &mut selection, left, true, 1.0);
        assert_eq!(phase(&boxes), Some(Phase::Boxing));
        let release = vec![event(Release, [90.0, 70.0])];
        let done = overlaid_at(&mut boxes, &mut selection, release, false, 1.0);
        assert_eq!(done.toast.as_deref(), Some(PRESS_MOVED));
        assert_eq!(selection.said(), None);
    }

    #[test]
    fn a_frame_is_asked_for_at_the_backstop_only_while_a_drag_let_go_waits() {
        let mut selection = Selection::default();
        let left = vec![
            event(Press, [50.0, 40.0]),
            event(Move, [70.0, 40.0]),
            event(Release, [90.0, 70.0]),
        ];
        // Waiting, its view's bodies of another model (nothing asked): a frame at the
        // backstop, CLICK_WAIT from its release.
        let mut boxes = BoxSelect::default();
        let done = overlaid_at(&mut boxes, &mut selection, left.clone(), false, 1.0);
        assert_eq!(phase(&boxes), Some(Phase::Held));
        assert!(done.repaint <= CLICK_WAIT, "{:?}", done.repaint);
        // Held, not let go: none asked for.
        let mut boxes = BoxSelect::default();
        let done = overlaid_at(&mut boxes, &mut selection, left[..2].to_vec(), false, 1.0);
        assert_eq!(done.repaint, std::time::Duration::MAX);
    }

    fn fill() -> egui::Color32 {
        theme::color32(theme::BOX_SELECT_FILL)
    }

    #[test]
    fn a_box_is_drawn_while_boxing_and_nothing_before() {
        let mut selection = Selection::default();
        let mut boxes = BoxSelect {
            heard: Some(([50, 40], None)),
            ..BoxSelect::default()
        };
        let pressed = overlaid_at(
            &mut boxes,
            &mut selection,
            vec![event(Press, [50.0, 40.0])],
            true,
            1.0,
        );
        assert!(pressed.shapes.is_empty(), "{:?}", pressed.shapes);
        let boxing = overlaid_at(
            &mut boxes,
            &mut selection,
            vec![event(Move, [90.0, 70.0])],
            true,
            1.0,
        );
        assert!(
            boxing
                .shapes
                .iter()
                .any(|shape| matches!(shape, Shape::Rect(rect) if rect.fill == fill())),
            "{:?}",
            boxing.shapes
        );
    }

    /// What [`draw_box`] paints from `from` to `to`, at `ppp`, the canvas at (10, 20).
    fn drawn_box(from: [f32; 2], to: [f32; 2], ppp: f32) -> Vec<Shape> {
        let ctx = egui::Context::default();
        let mut frame = view_frame(None);
        frame.rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(128.0, 64.0));
        frame.pixels_per_point = ppp;
        let output = pass(&ctx, |ui| {
            draw_box(
                &ui.ctx().layer_painter(egui::LayerId::background()),
                &frame,
                from,
                to,
            );
        });
        flat(output.shapes.into_iter().map(|clipped| clipped.shape))
    }

    #[test]
    fn a_window_is_drawn_solid_a_crossing_dashed_and_a_box_of_no_size_not_at_all() {
        let stroke = Stroke::new(
            theme::BOX_SELECT_STROKE_WIDTH,
            theme::color32(theme::BOX_SELECT_STROKE),
        );
        // Left to right, at 2 pixels a point: (40, 30)–(100, 90) pixels is (20, 15)–(50, 45)
        // points from the canvas's corner (10, 20): (30, 35)–(60, 65), filled, its outline
        // one solid line round it.
        let window = drawn_box([40.0, 30.0], [100.0, 90.0], 2.0);
        let want = egui::Rect::from_min_max(egui::pos2(30.0, 35.0), egui::pos2(60.0, 65.0));
        assert!(
            matches!(
                &window[..],
                [Shape::Rect(rect), Shape::Path(path)]
                    if rect.rect == want && rect.fill == fill()
                        && path.points == [want.left_top(), want.right_top(), want.right_bottom(),
                            want.left_bottom(), want.left_top()]
                        && path.stroke.width == stroke.width
            ),
            "{window:?}"
        );
        // Straight down (x unchanged): a window too.
        let down = drawn_box([40.0, 30.0], [40.0, 90.0], 2.0);
        assert!(
            matches!(&down[..], [Shape::Rect(_), Shape::Path(_)]),
            "{down:?}"
        );
        // Right to left: dashed, 4 on and 3 off, so many segments.
        let crossing = drawn_box([100.0, 30.0], [40.0, 90.0], 2.0);
        let dashes = crossing
            .iter()
            .filter(|shape| matches!(shape, Shape::LineSegment { .. }))
            .count();
        assert!(matches!(crossing.first(), Some(Shape::Rect(rect)) if rect.rect == want));
        assert!(dashes > 4, "{crossing:?}");
        assert!(!crossing.iter().any(|shape| matches!(shape, Shape::Path(_))));
        // No size, or a corner that isn't finite, each of the four: nothing.
        assert!(drawn_box([40.0, 30.0], [40.0, 30.0], 2.0).is_empty());
        for (from, to) in [
            ([f32::NAN, 30.0], [100.0, 90.0]),
            ([40.0, f32::NAN], [100.0, 90.0]),
            ([40.0, 30.0], [f32::INFINITY, 90.0]),
            ([40.0, 30.0], [100.0, f32::NAN]),
        ] {
            assert!(drawn_box(from, to, 2.0).is_empty(), "{from:?} {to:?}");
        }
        // Pixels per point of 0: corners that aren't finite, nothing.
        assert!(drawn_box([40.0, 30.0], [100.0, 90.0], 0.0).is_empty());
    }

    #[test]
    fn a_press_let_go_long_ago_isnt_asked_and_an_answer_in_isnt_asked_over() {
        // Let go by a view change, unanswered: never asked again.
        let mut boxes = BoxSelect::default();
        frame(
            &mut boxes,
            &[event(Press, [50.0, 40.0]), event(Move, [70.0, 40.0])],
            None,
            false,
        );
        frame(&mut boxes, &[], None, true);
        assert_eq!(phase(&boxes), Some(Phase::LetGo));
        let mut view = View::default();
        assert_eq!(boxes.ask(&mut view, &view_frame(None)), None);
        assert_eq!(view.asking(), None);
        // The press's own pick asked already, its answer in: not asked again, so the
        // answer stands (asking again lets it go).
        let mut boxes = BoxSelect::default();
        frame(&mut boxes, &[event(Press, [50.0, 40.0])], None, false);
        let ours = square([50.0, 40.0], SIZE, BAND);
        let mut view = View::default();
        view.pick(ours);
        view.answered_as(
            (1, silico_core::command::Revision::from_number(1).unwrap()),
            crate::renderer::Picking {
                frame: 1,
                rect: ours,
                hit: None,
                edge: None,
            },
        );
        let before = view.picked();
        assert!(before.is_some());
        assert_eq!(view.asking(), Some(ours));
        assert_eq!(boxes.ask(&mut view, &view_frame(Some([50.5, 40.5]))), None);
        assert_eq!(view.picked(), before);
    }

    #[test]
    fn a_second_press_over_a_drag_still_held_says_nothing() {
        // The window lost a release: a press over a drag held (not let go) undecided starts
        // afresh, unsaid; only one let go and waiting is said superseded.
        let mut boxes = BoxSelect::default();
        frame(
            &mut boxes,
            &[event(Press, [50.0, 40.0]), event(Move, [70.0, 40.0])],
            None,
            false,
        );
        assert_eq!(phase(&boxes), Some(Phase::Held));
        assert!(frame(&mut boxes, &[event(Press, [10.0, 10.0])], None, false).is_empty());
    }

    #[test]
    fn a_move_of_exactly_the_click_distance_is_still_a_clicks() {
        let empty = Some(([50, 40], None));
        let mut boxes = BoxSelect::default();
        frame(
            &mut boxes,
            &[event(Press, [50.0, 40.0])],
            empty.clone(),
            false,
        );
        frame(&mut boxes, &[event(Move, [56.0, 40.0])], empty, false);
        assert_eq!(phase(&boxes), Some(Phase::Pressed));
    }
}
