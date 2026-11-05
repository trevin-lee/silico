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
//! As the prototype's `onBox`: a box in a part takes edges, always; with the edge filter
//! off it takes nothing and a toast says so ([`EDGE_FILTER_OFF`]); else a toast says how
//! many edges it took ("Window selection: 4 edges").
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

use eframe::egui::{self, Painter, Rect, Shape, Stroke};
use silico_core::document::FeatureId;
use silico_core::naming::Name;
use silico_protocol::vec3::{dot, sub};
use silico_protocol::{Camera, MIN_EYE_DISTANCE, Projection};
use silico_render::render::project;

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
    /// The bodies the browser hides: hiding one (H) changes what's under a pixel, though
    /// the bodies table stays.
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
    /// `shown`, whether a body is drawn (the browser hides some). What a toast is to say,
    /// if anything: how many edges a box took, or why a drag took none.
    pub(crate) fn overlay(
        &mut self,
        selection: &mut Selection,
        view: &mut View,
        frame: &ViewFrame,
        painter: &Painter,
        current: bool,
        shown: &dyn Fn(&FeatureId) -> bool,
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
            filter: selection.filter,
            hidden: view
                .bodies()
                .values()
                .filter(|body| !shown(&body.feature))
                .map(|body| body.feature.clone())
                .collect(),
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
                Effect::Pick(name, how) if current => selection.pick(&name.to_string(), how),
                // Decided over meshes of another model than the selection's: the name was
                // read from what's drawn, not what it would be read against.
                Effect::Pick(..) => toast = Some(PRESS_MOVED.to_owned()),
                Effect::Said(said) => toast = Some(said.to_owned()),
                Effect::Box { .. } if !selection.filter.edges => {
                    toast = Some(EDGE_FILTER_OFF.to_owned());
                }
                Effect::Box {
                    from,
                    to,
                    crossing,
                    how,
                } if current => {
                    let area = PxBox::clamped(from, to, frame.size_px);
                    let names = taken(
                        view.bodies(),
                        shown,
                        EDGES,
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
                let how = if press.shift {
                    How::Add
                } else if press.ctrl {
                    How::Toggle
                } else {
                    How::Replace
                };
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

/// What a box takes in a part, as the prototype's `onBox`: edges alone, whatever the
/// filter (with the edge filter off, nothing, and that's said).
const EDGES: Kinds = Kinds {
    faces: false,
    edges: true,
};

/// What the toast says of a box that took `count` edges (the prototype's `onBox`).
fn box_words(crossing: bool, count: usize) -> String {
    let kind = if crossing { "Crossing" } else { "Window" };
    let plural = if count == 1 { "" } else { "s" };
    format!("{kind} selection: {count} edge{plural}")
}

/// Which kinds of element a box takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Kinds {
    faces: bool,
    edges: bool,
}

/// A box on the canvas, in physical pixels from its top left: `[x0, x1] × [y0, y1]`, its
/// sides counted inside.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PxBox {
    min: [f64; 2],
    max: [f64; 2],
}

impl PxBox {
    /// The box with corners `a` and `b`, clamped to a canvas `size` pixels (what's off the
    /// canvas isn't seen, so it's never inside). A corner that isn't finite is taken as the
    /// other's.
    fn clamped(a: [f32; 2], b: [f32; 2], size: [u32; 2]) -> PxBox {
        let side = |k: usize| {
            let limit = f64::from(size[k]);
            let first = f64::from(a[k]);
            let second = f64::from(b[k]);
            let (first, second) = match (first.is_finite(), second.is_finite()) {
                (true, true) => (first, second),
                (true, false) => (first, first),
                (false, true) => (second, second),
                (false, false) => (0.0, 0.0),
            };
            let low = first.min(second).clamp(0.0, limit);
            let high = first.max(second).clamp(0.0, limit);
            (low, high)
        };
        let (x0, x1) = side(0);
        let (y0, y1) = side(1);
        PxBox {
            min: [x0, y0],
            max: [x1, y1],
        }
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
    /// (the separating axes of a segment and a box).
    fn meets_segment(&self, a: [f64; 2], b: [f64; 2]) -> bool {
        if self.holds(a) || self.holds(b) {
            return true;
        }
        if !self.overlaps_bounds(&[a, b]) {
            return false;
        }
        let sides = self.corners().map(|c| orient(a, b, c));
        !(sides.iter().all(|s| *s > 0.0) || sides.iter().all(|s| *s < 0.0))
    }

    /// Whether the triangle `t` overlaps the box (shares a point with it): their boxes
    /// overlap, and no side of the triangle has every corner of the box strictly beyond it
    /// (the separating axes of two convex shapes). A triangle with no area is its sides.
    fn overlaps_triangle(&self, t: [[f64; 2]; 3]) -> bool {
        if !self.overlaps_bounds(&t) {
            return false;
        }
        let area = orient(t[0], t[1], t[2]);
        if area == 0.0 || !area.is_finite() {
            return (0..3).any(|k| self.meets_segment(t[k], t[(k + 1) % 3]));
        }
        (0..3).all(|k| {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            // The third vertex's side of this side's line is the inside.
            !self.corners().iter().all(|c| orient(a, b, *c) * area < 0.0)
        })
    }
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

    /// Where the segment from `inside` (kept) to `outside` (cut) crosses the cut, nudged a
    /// hair toward `inside` if rounding leaves it short, so it projects; `inside` itself
    /// if it can't be.
    fn crossing(&self, inside: [f64; 3], outside: [f64; 3]) -> [f64; 3] {
        let (Some(near), Some(far)) = (self.depth(inside), self.depth(outside)) else {
            return inside;
        };
        let t = (near - MIN_EYE_DISTANCE) / (near - far);
        for nudge in [0.0, 1e-12, 1e-9, 1e-6] {
            let s = t * (1.0 - nudge);
            let p: [f64; 3] = std::array::from_fn(|k| inside[k] + s * (outside[k] - inside[k]));
            if self.kept(p) && project(self.camera, self.size, p).is_some() {
                return p;
            }
        }
        inside
    }

    /// `points`, a segment or a triangle, as drawn: cut to its part in front of the cut (a
    /// polygon clipped by the plane), then projected.
    fn drawn(&self, points: &[Option<[f64; 3]>]) -> Drawn {
        let read: Vec<[f64; 3]> = points
            .iter()
            .flatten()
            .copied()
            .filter(|p| p.iter().all(|c| c.is_finite()))
            .collect();
        if read.len() < points.len() {
            let beside = read
                .iter()
                .filter(|p| self.kept(**p))
                .filter_map(|p| project(self.camera, self.size, *p))
                .collect();
            return Drawn::Unread(beside);
        }
        let mut left = Vec::with_capacity(read.len() + 1);
        if read.iter().all(|p| self.kept(*p)) {
            left = read;
        } else {
            let closed = read.len() > 2;
            let sides = if closed { read.len() } else { read.len() - 1 };
            for k in 0..sides {
                let (a, b) = (read[k], read[(k + 1) % read.len()]);
                match (self.kept(a), self.kept(b)) {
                    (true, true) => left.push(a),
                    (true, false) => {
                        left.push(a);
                        left.push(self.crossing(a, b));
                    }
                    (false, true) => left.push(self.crossing(b, a)),
                    (false, false) => {}
                }
                if !closed && self.kept(b) {
                    left.push(b);
                }
            }
        }
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
/// crossing, whether it meets it.
fn decide(pieces: impl Iterator<Item = Drawn>, area: &PxBox, crossing: bool) -> bool {
    let mut any = false;
    for piece in pieces {
        let (points, read) = match piece {
            Drawn::Cut => continue,
            Drawn::Shown(points) => (points, true),
            Drawn::Unread(points) => (points, false),
        };
        if crossing {
            let meets = if read {
                match points.as_slice() {
                    [p] => area.holds(*p),
                    [a, b] => area.meets_segment(*a, *b),
                    [first, rest @ ..] => rest
                        .windows(2)
                        .any(|pair| area.overlaps_triangle([*first, pair[0], pair[1]])),
                    [] => false,
                }
            } else {
                points.iter().any(|p| area.holds(*p))
            };
            if meets {
                return true;
            }
        } else {
            if !read || !points.iter().all(|p| area.holds(*p)) {
                return false;
            }
            any = true;
        }
    }
    !crossing && any
}

/// The names of what a box `area` takes of `bodies`, those `shown` alone, of the `kinds`
/// it takes, seen through `camera` on a canvas `size` pixels: wholly inside, or also
/// crossing it if `crossing`. By body in its number's order, then faces, then edges, each
/// by its number: as the element map names them.
fn taken(
    bodies: &BTreeMap<u32, ViewBody>,
    shown: &dyn Fn(&FeatureId) -> bool,
    kinds: Kinds,
    camera: &Camera,
    size: [u32; 2],
    area: PxBox,
    crossing: bool,
) -> Vec<String> {
    let sight = Sight::new(camera, size);
    let mut names = Vec::new();
    for body in bodies.values().filter(|body| shown(&body.feature)) {
        let mesh = &body.mesh;
        if kinds.faces {
            let position = |index: u32| -> Option<[f64; 3]> {
                usize::try_from(index)
                    .ok()
                    .and_then(|at| mesh.positions.get(at).copied())
            };
            let mut first = 0usize;
            for (count, number) in mesh.face_triangles.iter().zip(1u32..) {
                let count = usize::try_from(*count).unwrap_or(usize::MAX);
                let end = first.saturating_add(count);
                let triangles = mesh.triangles.get(first..end);
                first = end;
                let Some(triangles) = triangles.filter(|t| !t.is_empty()) else {
                    continue;
                };
                let pieces = triangles.iter().map(|t| sight.drawn(&t.map(position)));
                if decide(pieces, &area, crossing)
                    && let Some(named) = body.name_of(Element::Face(number))
                {
                    names.push(named.name.to_string());
                }
            }
        }
        if kinds.edges {
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
                    decide(
                        std::iter::once(sight.drawn(&[Some(*point)])),
                        &area,
                        crossing,
                    )
                } else {
                    let pieces = points
                        .windows(2)
                        .map(|pair| sight.drawn(&[Some(pair[0]), Some(pair[1])]));
                    decide(pieces, &area, crossing)
                };
                if take && let Some(named) = body.name_of(Element::Edge(number)) {
                    names.push(named.name.to_string());
                }
            }
        }
    }
    names
}

/// The box from `from` to `to` (physical pixels from the canvas's top left) drawn on
/// `painter` over `frame`'s canvas: [`theme::BOX_SELECT_FILL`], outlined in
/// [`theme::BOX_SELECT_STROKE`], solid for a window (dragged left to right) and dashed for a
/// crossing (right to left). The painter clips it to the canvas.
pub(crate) fn draw_box(painter: &Painter, frame: &ViewFrame, from: [f32; 2], to: [f32; 2]) {
    let ppp = frame.pixels_per_point;
    if !(ppp.is_finite() && ppp > 0.0) {
        return;
    }
    let at = |p: [f32; 2]| frame.rect.min + egui::vec2(p[0] / ppp, p[1] / ppp);
    let (a, b) = (at(from), at(to));
    if !(a.x.is_finite() && a.y.is_finite() && b.x.is_finite() && b.y.is_finite()) {
        return;
    }
    let rect = Rect::from_two_pos(a, b);
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
    if to[0] < from[0] {
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

    /// A body of `edges` (polylines, by their pixels on [`front`]) and `faces` (each a list
    /// of triangles, by their corners' pixels), each named `k7f2qx3m.box.Edge<n>` and
    /// `Face<n>` in order.
    fn body(edges: &[&[[f64; 2]]], faces: &[&[[[f64; 2]; 3]]]) -> ViewBody {
        let mut mesh = Mesh {
            positions: Vec::new(),
            normals: Vec::new(),
            triangles: Vec::new(),
            face_triangles: Vec::new(),
            valid: None,
            edge_points: Vec::new(),
            edge_point_counts: Vec::new(),
        };
        for edge in edges {
            mesh.edge_points
                .extend(edge.iter().map(|[x, y]| at(*x, *y, 0.0)));
            mesh.edge_point_counts.push(edge.len() as u32);
        }
        for face in faces {
            for triangle in *face {
                let base = mesh.positions.len() as u32;
                mesh.positions
                    .extend(triangle.iter().map(|[x, y]| at(*x, *y, 5.0)));
                mesh.normals.extend([[0.0, -1.0, 0.0]; 3]);
                mesh.triangles.push([base, base + 1, base + 2]);
            }
            mesh.face_triangles.push(face.len() as u32);
        }
        ViewBody {
            feature: FeatureId::new(BODY).unwrap(),
            key: Key::parse(&"01".repeat(32)).unwrap(),
            level: 0.01,
            deflection: 0.01,
            bounds: None,
            serves: [0.0, 1.0],
            held: false,
            faces: (1..=faces.len() as u32)
                .map(|n| named(ElementKind::Face, n))
                .collect(),
            edges: (1..=edges.len() as u32)
                .map(|n| named(ElementKind::Edge, n))
                .collect(),
            mesh: Arc::new(mesh),
        }
    }

    fn bodies(body: ViewBody) -> BTreeMap<u32, ViewBody> {
        BTreeMap::from([(1, body)])
    }

    const EDGES: Kinds = Kinds {
        faces: false,
        edges: true,
    };
    const FACES: Kinds = Kinds {
        faces: true,
        edges: false,
    };

    fn every(_: &FeatureId) -> bool {
        true
    }

    /// What a box from `a` to `b` (pixels) takes of `bodies` on [`front`], window or
    /// crossing as dragged (right to left a crossing), as short names (`Edge1`).
    fn boxed(
        bodies: &BTreeMap<u32, ViewBody>,
        kinds: Kinds,
        a: [f32; 2],
        b: [f32; 2],
    ) -> Vec<String> {
        let area = PxBox::clamped(a, b, SIZE);
        taken(bodies, &every, kinds, &front(), SIZE, area, b[0] < a[0])
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
        let held = bodies(body(&edges, &[]));
        // Left to right: a window.
        assert_eq!(boxed(&held, EDGES, [105.0, 25.0], [140.0, 45.0]), ["Edge1"]);
        // The same box right to left: a crossing, also Edge2, which is half in, and Edge4,
        // which runs through it (none of its points is inside).
        assert_eq!(
            boxed(&held, EDGES, [140.0, 25.0], [105.0, 45.0]),
            ["Edge1", "Edge2", "Edge4"]
        );
        // Up or down doesn't matter, only left or right.
        assert_eq!(boxed(&held, EDGES, [105.0, 45.0], [140.0, 25.0]), ["Edge1"]);
    }

    #[test]
    fn an_edge_on_the_boxs_outline_is_inside_and_one_a_hair_out_isnt() {
        // The box's sides count as inside: Edge1 along its top, (110, 25)–(130, 25); Edge2
        // a thousandth of a pixel above it.
        let edges: [&[[f64; 2]]; 2] = [
            &[[110.0, 25.0], [130.0, 25.0]],
            &[[110.0, 24.999], [130.0, 24.999]],
        ];
        let held = bodies(body(&edges, &[]));
        assert_eq!(boxed(&held, EDGES, [105.0, 25.0], [140.0, 45.0]), ["Edge1"]);
        // A crossing doesn't reach it either: it's outside, if barely.
        assert_eq!(boxed(&held, EDGES, [140.0, 25.0], [105.0, 45.0]), ["Edge1"]);
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
        let held = bodies(body(&edges, &[]));
        assert_eq!(boxed(&held, EDGES, [100.0, 40.0], [109.5, 60.0]), ["Edge1"]);
        assert!(boxed(&held, EDGES, [100.0, 40.0], [108.5, 60.0]).is_empty());
    }

    #[test]
    fn a_faces_triangles_decide_it_a_crossing_takes_one_the_box_lies_wholly_inside() {
        // Face1: the square (110, 20)–(150, 40) as two triangles. Face2: a triangle
        // (160, 10), (190, 10), (160, 40).
        let square: &[[[f64; 2]; 3]] = &[
            [[110.0, 20.0], [150.0, 20.0], [150.0, 40.0]],
            [[110.0, 20.0], [150.0, 40.0], [110.0, 40.0]],
        ];
        let corner: &[[[f64; 2]; 3]] = &[[[160.0, 10.0], [190.0, 10.0], [160.0, 40.0]]];
        let held = bodies(body(&[], &[square, corner]));
        // A window round the square alone, then round both.
        assert_eq!(boxed(&held, FACES, [105.0, 15.0], [155.0, 45.0]), ["Face1"]);
        assert_eq!(
            boxed(&held, FACES, [105.0, 5.0], [195.0, 45.0]),
            ["Face1", "Face2"]
        );
        // A crossing inside the square, no vertex of it inside the box: the box overlaps
        // its triangles, so it's taken.
        assert_eq!(boxed(&held, FACES, [140.0, 25.0], [120.0, 35.0]), ["Face1"]);
        // A crossing in the corner's empty half, beyond its long side from (190, 10) to
        // (160, 40) (x + y = 200): (185, 30)–(195, 38) has every corner past it. Taken by
        // its box's overlap alone, it would be.
        assert!(boxed(&held, FACES, [195.0, 30.0], [185.0, 38.0]).is_empty());
        // Moved to touch the long side at (180, 20): taken.
        assert_eq!(boxed(&held, FACES, [190.0, 20.0], [180.0, 30.0]), ["Face2"]);
    }

    #[test]
    fn a_box_takes_edges_alone_and_its_toast_counts_them_as_the_prototype_words_it() {
        // The prototype's `onBox`: edges, whatever the filter (its edge filter off, the
        // overlay takes nothing, `EDGE_FILTER_OFF`); and its toast's words.
        assert_eq!(
            super::EDGES,
            Kinds {
                faces: false,
                edges: true
            }
        );
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
        let held = bodies(body(&edges, &[]));
        assert_eq!(
            boxed(&held, EDGES, [140.0, 20.0], [100.0, 40.0]),
            ["Edge2", "Edge3"]
        );
    }

    #[test]
    fn a_hidden_body_and_an_unmeshed_face_or_edge_are_never_taken() {
        let square: &[[[f64; 2]; 3]] = &[[[110.0, 20.0], [150.0, 20.0], [150.0, 40.0]]];
        let edges: [&[[f64; 2]]; 2] = [&[], &[[110.0, 30.0], [130.0, 30.0]]];
        let held = bodies(body(&edges, &[&[], square]));
        // Edge1 and Face1 have nothing in the mesh: neither is taken, by a window or a
        // crossing; Edge2 and Face2 are.
        assert_eq!(boxed(&held, EDGES, [100.0, 10.0], [160.0, 50.0]), ["Edge2"]);
        assert_eq!(boxed(&held, FACES, [160.0, 10.0], [100.0, 50.0]), ["Face2"]);
        let area = PxBox::clamped([100.0, 10.0], [160.0, 50.0], SIZE);
        let hidden = |_: &FeatureId| false;
        assert!(taken(&held, &hidden, EDGES, &front(), SIZE, area, false).is_empty());
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

    /// A body of `edges` and `faces` in the document's coordinates, named in order.
    fn body_at(edges: &[&[[f64; 3]]], faces: &[[[f64; 3]; 3]]) -> BTreeMap<u32, ViewBody> {
        let mut held = body(&[], &[]);
        let mut mesh = (*held.mesh).clone();
        for edge in edges {
            mesh.edge_points.extend(edge.iter().copied());
            mesh.edge_point_counts.push(edge.len() as u32);
        }
        for face in faces {
            let base = mesh.positions.len() as u32;
            mesh.positions.extend(face.iter().copied());
            mesh.normals.extend([[0.0, -1.0, 0.0]; 3]);
            mesh.triangles.push([base, base + 1, base + 2]);
            mesh.face_triangles.push(1);
        }
        held.mesh = Arc::new(mesh);
        held.edges = (1..=edges.len() as u32)
            .map(|n| named(ElementKind::Edge, n))
            .collect();
        held.faces = (1..=faces.len() as u32)
            .map(|n| named(ElementKind::Face, n))
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
        let held = body_at(&edges, &[]);
        let names = |area: PxBox, crossing| -> Vec<String> {
            taken(&held, &every, EDGES, &camera, SIZE, area, crossing)
                .into_iter()
                .map(|name| name.trim_start_matches("k7f2qx3m.box.").to_owned())
                .collect()
        };
        // The reviewer's box, (190, 60) to (150, 68), right to left: Edge1's drawn part
        // crosses it, though its far end doesn't project.
        let right = PxBox::clamped([190.0, 60.0], [150.0, 68.0], SIZE);
        assert_eq!(names(right, true), ["Edge1"]);
        // A window over the whole canvas: Edge3's drawn part is inside; Edge1's runs off
        // the canvas to its cut; Edge2 has nothing drawn.
        let whole = PxBox::clamped([0.0, 0.0], [256.0, 128.0], SIZE);
        assert_eq!(names(whole, false), ["Edge3"]);
        assert_eq!(names(whole, true), ["Edge1", "Edge3"]);
        // Off the middle, left of x 50: nothing drawn there.
        let aside = PxBox::clamped([0.0, 0.0], [40.0, 128.0], SIZE);
        assert!(names(aside, true).is_empty());
    }

    #[test]
    fn a_face_partly_behind_the_eye_is_tested_by_what_the_drawing_keeps_of_it() {
        // Face1: (−50, 0, ∓10), 100 in front, and (50, −150, 0), 50 behind: drawn as the
        // quadrilateral left of the cut, which reaches far to the right along the middle
        // row. Face2 lies wholly behind the eye.
        let camera = from_near();
        let faces = [
            [[-50.0, 0.0, -10.0], [-50.0, 0.0, 10.0], [50.0, -150.0, 0.0]],
            [
                [0.0, -200.0, -10.0],
                [0.0, -200.0, 10.0],
                [10.0, -150.0, 0.0],
            ],
        ];
        let held = body_at(&[], &faces);
        let right = PxBox::clamped([190.0, 60.0], [150.0, 68.0], SIZE);
        assert_eq!(
            taken(&held, &every, FACES, &camera, SIZE, right, true),
            ["k7f2qx3m.box.Face1"]
        );
        let whole = PxBox::clamped([0.0, 0.0], [256.0, 128.0], SIZE);
        assert!(taken(&held, &every, FACES, &camera, SIZE, whole, false).is_empty());
        // Above the middle row, right of the near side (x ≈ 51, y 48.5 to 79.5): the quad's
        // cut corners fan out far up and down (its top side falls about 0.19 a pixel, to
        // y ≈ 20 at x 200), so it covers (200, 25)–(250, 35) too.
        let above = PxBox::clamped([250.0, 25.0], [200.0, 35.0], SIZE);
        assert_eq!(
            taken(&held, &every, FACES, &camera, SIZE, above, true),
            ["k7f2qx3m.box.Face1"]
        );
    }

    #[test]
    fn a_box_is_clamped_to_the_canvas_and_a_corner_that_isnt_finite_is_the_others() {
        assert_eq!(
            PxBox::clamped([150.0, 50.0], [500.0, -20.0], SIZE),
            PxBox {
                min: [150.0, 0.0],
                max: [256.0, 50.0]
            }
        );
        assert_eq!(
            PxBox::clamped([150.0, 50.0], [f32::NAN, f32::INFINITY], SIZE),
            PxBox {
                min: [150.0, 50.0],
                max: [150.0, 50.0]
            }
        );
        // Wholly off the canvas: a line along its side, which holds only what's on it.
        assert_eq!(
            PxBox::clamped([-50.0, 10.0], [-10.0, 20.0], SIZE),
            PxBox {
                min: [0.0, 10.0],
                max: [0.0, 20.0]
            }
        );
        // An edge off the canvas, past its right side, isn't inside a box dragged past it.
        let edges: [&[[f64; 2]]; 2] = [
            &[[246.0, 30.0], [266.0, 30.0]],
            &[[150.0, 30.0], [255.0, 30.0]],
        ];
        let held = bodies(body(&edges, &[]));
        assert_eq!(boxed(&held, EDGES, [140.0, 10.0], [400.0, 50.0]), ["Edge2"]);
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

    /// `boxes`' overlay over a fresh view (whose pick asked is the frame's alone) of
    /// `left`, the view's bodies `current`: the toast, and the pick asked.
    fn overlaid(
        boxes: &mut BoxSelect,
        selection: &mut Selection,
        left: Vec<LeftEvent>,
        current: bool,
    ) -> (Option<String>, Option<crate::renderer::PixelRect>) {
        let ctx = egui::Context::default();
        let painter = Painter::new(
            ctx,
            egui::LayerId::background(),
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(256.0, 128.0)),
        );
        let mut view = View::default();
        let mut frame = view_frame(None);
        frame.left = left;
        frame.now = start();
        let toast = boxes.overlay(selection, &mut view, &frame, &painter, current, &every);
        (toast, view.asking())
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
        let ours = square([50.0, 40.0], SIZE, 3);
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
        let ours = square([50.0, 40.0], SIZE, 3);
        let at = |pixel: [f32; 2]| square(pixel, SIZE, 3);
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
        // Counts past the arrays, an index past the positions, a point that isn't finite: a
        // face or an edge with a point that can't be read isn't wholly inside; a crossing
        // tests what it can read.
        let mut held = body(&[], &[]);
        held.mesh = Arc::new(Mesh {
            positions: vec![
                at(110.0, 20.0, 0.0),
                at(150.0, 20.0, 0.0),
                at(150.0, 40.0, 0.0),
            ],
            normals: vec![[0.0, -1.0, 0.0]; 3],
            triangles: vec![[0, 1, 2], [0, 1, 99]],
            face_triangles: vec![1, 1, u32::MAX],
            valid: None,
            edge_points: vec![
                at(110.0, 30.0, 0.0),
                [f64::NAN, 0.0, 0.0],
                at(120.0, 30.0, 0.0),
                at(130.0, 30.0, 0.0),
            ],
            edge_point_counts: vec![2, 2, u32::MAX],
        });
        held.faces = (1..=3).map(|n| named(ElementKind::Face, n)).collect();
        held.edges = (1..=3).map(|n| named(ElementKind::Edge, n)).collect();
        let held = bodies(held);
        let all = Kinds {
            faces: true,
            edges: true,
        };
        // Face1 whole; Face2's third corner is no position; Face3 runs past the triangles.
        // Edge1 has a point that isn't finite; Edge2 is whole; Edge3 runs past the points.
        assert_eq!(
            boxed(&held, all, [100.0, 10.0], [160.0, 50.0]),
            ["Face1", "Edge2"]
        );
        // A crossing takes Face2 by its corners that are read, and Edge1 by its point that
        // projects.
        assert_eq!(
            boxed(&held, all, [160.0, 10.0], [100.0, 50.0]),
            ["Face1", "Face2", "Edge1", "Edge2"]
        );
        // More names than elements, or fewer: what has no name isn't taken.
        let mut fewer = held[&1].clone();
        fewer.faces.truncate(1);
        fewer.edges.clear();
        assert_eq!(
            boxed(&bodies(fewer), all, [160.0, 10.0], [100.0, 50.0]),
            ["Face1"]
        );
    }

    #[test]
    fn an_empty_scene_a_box_of_no_size_and_a_camera_that_cant_draw_take_nothing() {
        let edges: [&[[f64; 2]]; 1] = [&[[110.0, 30.0], [130.0, 30.0]]];
        let held = bodies(body(&edges, &[]));
        assert!(boxed(&BTreeMap::new(), EDGES, [0.0, 0.0], [256.0, 128.0]).is_empty());
        // A box of no size takes only what's exactly on its point: an edge of one point
        // there, not one through it.
        assert!(boxed(&held, EDGES, [120.0, 30.0], [120.0, 30.0]).is_empty());
        let single: [&[[f64; 2]]; 1] = [&[[120.0, 31.0]]];
        assert_eq!(
            boxed(
                &bodies(body(&single, &[])),
                EDGES,
                [120.0, 31.0],
                [120.0, 31.0]
            ),
            ["Edge1"]
        );
        // A camera the renderer refuses, or a canvas of no size: nothing projects.
        let mut camera = front();
        camera.eye = camera.target;
        let area = PxBox::clamped([0.0, 0.0], [256.0, 128.0], SIZE);
        assert!(taken(&held, &every, EDGES, &camera, SIZE, area, true).is_empty());
        assert!(taken(&held, &every, EDGES, &front(), [0, 0], area, true).is_empty());
    }

    #[test]
    fn a_box_over_a_large_model_takes_every_edge_inside_once_in_order() {
        // 100,000 edges, each a two-point polyline half a pixel long, in a 200 by 100 grid
        // of pixels a quarter in: a window to x = 99.9 takes those wholly in it, in their
        // order.
        let mut held = body(&[], &[]);
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
        let names = boxed(&held, EDGES, [0.0, 0.0], [99.9, 100.0]);
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
}
