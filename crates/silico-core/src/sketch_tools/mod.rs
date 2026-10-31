//! M5.11's sketch tools, headless (docs/design/m5.0-app.md, section 9, *Sketching*; #278's
//! decisions A to G; docs/study/sketch-tool-relations.md): the line, the corner and centre
//! rectangles, the circle, the centrepoint, 3-point, and tangent arcs, and the point, each a
//! state machine from the pointer's events, in the sketch's plane, to the `sketch_add` its
//! gesture makes, with the relations the pointer shows. No viewport, no thread, no session:
//! the viewport feeds a tool points in sketch millimetres, its pick tolerance, and the newest
//! `sketch` read; it draws the tool's [`Preview`], says its [`Message`], and sends its
//! [`Outcome::add`] as a `sketch_add` (`NewMembers::of`). Behaviour is SolidWorks', as the
//! design takes it (docs/ux/solidworks.md, §4):
//!
//! - **Modes.** The first press decides: a press, a drag, and a release draws one entity;
//!   a press and release goes on clicking. A line's clicks make a chain, its segments
//!   sharing their points (D2), each segment added as it's clicked. A double-click, *End
//!   chain*, or Enter ends a chain and keeps the tool; a click on a point the sketch holds
//!   ends it there; Esc ends it and leaves the tool ([`Outcome::left`]). A click within the
//!   pick tolerance of the gesture's last point (a double-click's second click) adds nothing;
//!   a drag released within it of its press stays a drag, and draws nothing (too small).
//! - **Points shared.** A click on a point of the sketch always shares it: structure, never a
//!   relation, whatever *Automatic relations* says and whether Ctrl is held (#278's F). The
//!   point tool clicked on a point adds nothing, and says so (#278's D).
//! - **Inferred relations** ([`infer`]'s order): the origin's `horizontal_points` and
//!   `vertical_points` (#278's A), `midpoint` (and `on` of a curve crossing there), `on` (two
//!   at a crossing); a line's `tangent_at` (`same`) to an arc it leaves, with its
//!   `horizontal` or `vertical` too if it's also that, else `horizontal` or `vertical`, else
//!   `perpendicular` or `parallel` to a line at its start, each within 2°. Every one the
//!   pointer shows at the accepting click is added, redundant or not (#278's G). Ctrl held,
//!   or *Automatic relations* off, suppresses them all, and only them (#278's F).
//! - **Points lined up** (#278's E, and the coordinator's decision of 2026-10-31, as
//!   SolidWorks wakes a point by hovering it): a point placed on nothing lines up only with
//!   a woken point, one the pointer has passed within the pick tolerance of while the tool's
//!   in use, or one the tool has added: the last [`WAKE`], the newest first, forgotten on
//!   Esc and with the tool. Lining up is a relation (`horizontal_points`, `vertical_points`)
//!   only for a lone point (the point tool's, and a circle's, an arc's, or a centre
//!   rectangle's centre), whose alignment is all it shows; a line's or an arc's end, or a
//!   rectangle's corner, only follows a dotted guide, as the help's horizontal line does.
//! - **A tool's own relations** stay with Ctrl and the setting: a rectangle's two
//!   `horizontal` and two `vertical`, a centre rectangle's `symmetric` about its one
//!   construction point (#278's C), and a tangent arc's `tangent_at` (#278's B: tangent arcs
//!   only, no normal zones).
//! - **Not built:** a tangency along an edge (`tangent`, or `tangent_on`: a line ended
//!   tangent on a circle, a circle drawn tangent to a curve), and `perpendicular` or
//!   `parallel` to a line the pointer only hovered (a line only shows them to a line ending
//!   at its start).
//!
//! **The sketch under a tool.** What a tool adds is taken to be in the sketch until a read
//! settles it: the viewport tells the tool the revision each add made
//! ([`SketchTool::added`]), and a read of that revision or later decides (one without it
//! was undone, rolled back, or removed by another client). A refused add is forgotten with
//! the gesture ([`SketchTool::forget_pending`]); another sketch starts the tool afresh. Each
//! event finds the gesture's points and curves in the newest read again: a chain's last
//! point where the solve put it, a chain whose last point is gone ended, and a point shared
//! that's gone a new point at its place.
//!
//! The pointer's positions are taken as given: one that isn't finite is refused
//! ([`Message::NotFinite`]); a click that would place a point past a kilometre, or draw
//! something the kernel can't (1e-7 mm or less), adds nothing and says why. The work an
//! event does is bounded: each curve's shape is found once, and crossings are tried among
//! the [`CROSSING_CURVES`] curves nearest the pointer only.

mod arc;
mod build;
mod circle;
mod geometry;
mod infer;
mod line;
mod point;
mod rectangle;
#[cfg(test)]
mod tests;

use crate::command::{FeatureIds, Revision};
use crate::document::sketch_edit::Members;
use crate::document::{ElementId, FeatureId, Reference, Sketch};
use crate::{Error, ErrorCode};

pub use infer::{ANGLE_TOLERANCE_DEGREES, CROSSING_CURVES, WAKE};

/// A point or a vector in the sketch's plane, `[x, y]`, in millimetres.
pub type Xy = [f64; 2];

/// Which tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Line,
    CornerRectangle,
    CenterRectangle,
    Circle,
    CenterpointArc,
    ThreePointArc,
    TangentArc,
    Point,
}

impl ToolKind {
    pub const ALL: [ToolKind; 8] = [
        ToolKind::Line,
        ToolKind::CornerRectangle,
        ToolKind::CenterRectangle,
        ToolKind::Circle,
        ToolKind::CenterpointArc,
        ToolKind::ThreePointArc,
        ToolKind::TangentArc,
        ToolKind::Point,
    ];
}

/// What a tool works against: the sketch, by its feature's ID; the newest `sketch` read's
/// members, as solved, and the revision of the model it read; the pick tolerance, in
/// millimetres, that the viewport gives (a few pixels at its zoom); and whether *Automatic
/// relations* is on.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    pub sketch_id: &'a FeatureId,
    pub revision: Revision,
    pub sketch: &'a Sketch,
    pub tolerance: f64,
    pub automatic_relations: bool,
}

/// The keys held with an event: Shift (a square), and Ctrl (no inferred relations).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
}

/// The pointer's and the keyboard's events, each pointer event at a place in the sketch's
/// plane, as the viewport sees them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    Press(Xy),
    Move(Xy),
    Release(Xy),
    /// The viewport's double-click, after the release of its second click.
    DoubleClick(Xy),
    /// Right-click > *End chain*.
    EndChain,
    Escape,
    Enter,
}

/// What an event did: the members to add, if it completed something (a `sketch_add`'s,
/// each list as the document writes a sketch's: `NewMembers::of`); what the tool says; and
/// whether the tool was left (Esc).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Outcome {
    pub add: Option<Members>,
    pub message: Option<Message>,
    pub left: bool,
}

impl Outcome {
    fn nothing() -> Self {
        Outcome::default()
    }

    fn added(members: Members) -> Self {
        Outcome {
            add: Some(members),
            ..Outcome::default()
        }
    }

    fn said(message: Message) -> Self {
        Outcome {
            message: Some(message),
            ..Outcome::default()
        }
    }

    fn left() -> Self {
        Outcome {
            left: true,
            ..Outcome::default()
        }
    }
}

/// What a tool says when a click adds nothing it might have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// The point tool clicked on a point the sketch has (#278's D).
    PointThere(ElementId),
    /// A tangent arc pressed where no line or arc ends.
    NoEndToFollow,
    /// A tangent arc pressed where more than one line or arc ends: which to follow is
    /// unclear.
    SeveralEnds(ElementId),
    /// Something the kernel can't draw: 1e-7 mm or less.
    TooSmall,
    /// An arc through three points in a line, or a tangent arc to a point straight ahead.
    InLine,
    /// A point past a kilometre from the origin, or a radius past a kilometre.
    TooFar,
    /// A position that isn't a number.
    NotFinite,
    /// No new ID could be made.
    NoId(Error),
}

impl Message {
    /// As the app shows it.
    pub fn text(&self) -> String {
        match self {
            Message::PointThere(id) => {
                format!("point {id} is already there: nothing was added")
            }
            Message::NoEndToFollow => {
                "a tangent arc starts at the end of a line or an arc: press on one".to_owned()
            }
            Message::SeveralEnds(id) => format!(
                "more than one line or arc ends at point {id}: start a tangent arc at an end only one has"
            ),
            Message::TooSmall => {
                "too small to draw: lines, arcs, and circles are larger than 1e-7 mm".to_owned()
            }
            Message::InLine => "the points are in a line: no arc passes through them".to_owned(),
            Message::TooFar => "a sketch is drawn within a kilometre of its origin".to_owned(),
            Message::NotFinite => "the pointer's position isn't a number".to_owned(),
            Message::NoId(error) => format!("no new ID could be made: {}", error.message()),
        }
    }
}

/// A relation the pointer shows, drawn as its glyph at `at`.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub at: Xy,
    pub relation: Relation,
}

/// The relations a pointer shows: each is added with the entity, but `Shared`, which is the
/// point itself, shared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Relation {
    /// On a point of the sketch, which the click shares (coincident, as structure).
    Shared(ElementId),
    /// At the origin (#278's A).
    Origin,
    Midpoint(ElementId),
    On(Reference),
    Horizontal,
    Vertical,
    /// Level with the point.
    HorizontalPoints(ElementId),
    /// Above or below the point.
    VerticalPoints(ElementId),
    Perpendicular(ElementId),
    Parallel(ElementId),
    /// Tangent to the curve (`tangent_at`, a smooth join).
    Tangent(ElementId),
}

/// An arc to draw: counter-clockwise from `start` to `end` about `center`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewArc {
    pub center: Xy,
    pub start: Xy,
    pub end: Xy,
}

/// What the viewport draws for the tool: where the pointer snapped; the entity under way;
/// the relations the pointer shows; and the dotted inference lines.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Preview {
    pub pointer: Option<Xy>,
    pub points: Vec<Xy>,
    pub lines: Vec<[Xy; 2]>,
    pub construction_points: Vec<Xy>,
    pub circles: Vec<(Xy, f64)>,
    pub arcs: Vec<PreviewArc>,
    pub shown: Vec<Shown>,
    pub guides: Vec<[Xy; 2]>,
}

/// Where a tool's new members' IDs come from: fresh ones, none `taken`, never one given
/// before (a `sketch_add`'s contract: a fresh random ID, never one a removal freed).
pub trait NewIds {
    fn next(&mut self, taken: &dyn Fn(&ElementId) -> bool) -> Result<ElementId, Error>;
}

/// IDs from the system's randomness, as the core draws a feature's and an edit's.
pub struct SystemIds(FeatureIds);

impl SystemIds {
    pub fn new() -> Self {
        SystemIds(FeatureIds::system())
    }
}

impl Default for SystemIds {
    fn default() -> Self {
        SystemIds::new()
    }
}

impl NewIds for SystemIds {
    fn next(&mut self, taken: &dyn Fn(&ElementId) -> bool) -> Result<ElementId, Error> {
        self.0.next_member(taken)
    }
}

/// A sketch tool, between events.
#[derive(Debug, Clone)]
pub struct SketchTool {
    state: State,
    preview: Preview,
    /// The sketch it works in: a context of another starts it afresh.
    sketch: Option<FeatureId>,
    /// What the tool has added that the newest read may not hold yet, for the next click to
    /// share or find (a chain's last point, a point clicked twice), each add in turn with
    /// the revision it made once the core has answered ([`SketchTool::added`]).
    pending: Vec<Pending>,
    /// The points the pointer has woken, the newest first, at most [`WAKE`] (#278's E).
    woken: Vec<ElementId>,
}

/// One add the tool made: its members, until a read holds them or a read of the revision
/// it made (or later) doesn't (undone, rolled back, or removed by another client).
#[derive(Debug, Clone)]
struct Pending {
    members: Members,
    revision: Option<Revision>,
}

#[derive(Debug, Clone)]
enum State {
    Line(line::Line),
    CornerRectangle(rectangle::Rectangle),
    CenterRectangle(rectangle::Rectangle),
    Circle(circle::Circle),
    CenterpointArc(arc::Centerpoint),
    ThreePointArc(arc::ThreePoint),
    TangentArc(arc::Tangent),
    Point(point::Point),
}

/// What a tool's step has: the sketch it snaps to, the pointer there, the keys held, and
/// where new IDs come from.
struct Step<'v, 's, 'i> {
    pointer: infer::Pointer<'v, 's>,
    shift: bool,
    ids: &'i mut dyn NewIds,
}

impl SketchTool {
    pub fn new(kind: ToolKind) -> Self {
        let state = match kind {
            ToolKind::Line => State::Line(line::Line::default()),
            ToolKind::CornerRectangle => State::CornerRectangle(rectangle::Rectangle::corner()),
            ToolKind::CenterRectangle => State::CenterRectangle(rectangle::Rectangle::centre()),
            ToolKind::Circle => State::Circle(circle::Circle::default()),
            ToolKind::CenterpointArc => State::CenterpointArc(arc::Centerpoint::default()),
            ToolKind::ThreePointArc => State::ThreePointArc(arc::ThreePoint::default()),
            ToolKind::TangentArc => State::TangentArc(arc::Tangent::default()),
            ToolKind::Point => State::Point(point::Point::default()),
        };
        SketchTool {
            state,
            preview: Preview::default(),
            sketch: None,
            pending: Vec::new(),
            woken: Vec::new(),
        }
    }

    pub fn kind(&self) -> ToolKind {
        match &self.state {
            State::Line(_) => ToolKind::Line,
            State::CornerRectangle(_) => ToolKind::CornerRectangle,
            State::CenterRectangle(_) => ToolKind::CenterRectangle,
            State::Circle(_) => ToolKind::Circle,
            State::CenterpointArc(_) => ToolKind::CenterpointArc,
            State::ThreePointArc(_) => ToolKind::ThreePointArc,
            State::TangentArc(_) => ToolKind::TangentArc,
            State::Point(_) => ToolKind::Point,
        }
    }

    /// What to draw now.
    pub fn preview(&self) -> &Preview {
        &self.preview
    }

    /// Whether a gesture is under way: a press held, or a chain or an entity begun.
    pub fn busy(&self) -> bool {
        match &self.state {
            State::Line(t) => t.busy(),
            State::CornerRectangle(t) | State::CenterRectangle(t) => t.busy(),
            State::Circle(t) => t.busy(),
            State::CenterpointArc(t) => t.busy(),
            State::ThreePointArc(t) => t.busy(),
            State::TangentArc(t) => t.busy(),
            State::Point(t) => t.busy(),
        }
    }

    /// The tool's gesture begun afresh, its kind kept: nothing under way.
    fn restart(&mut self) {
        let fresh = SketchTool::new(self.kind());
        self.state = fresh.state;
    }

    /// Forgets what it added that no read has shown, and the gesture under way, which may
    /// name it: for an add the core refused, so no click finds a member the sketch hasn't.
    pub fn forget_pending(&mut self) {
        self.pending.clear();
        self.restart();
    }

    /// The core took the oldest of the tool's adds it hadn't answered, at `revision`: once
    /// a read of that revision or later is given, the read says whether the sketch holds it.
    pub fn added(&mut self, revision: Revision) {
        if let Some(pending) = self.pending.iter_mut().find(|p| p.revision.is_none()) {
            pending.revision = Some(revision);
        }
    }

    /// The tool brought up to `context`: started afresh in another sketch; its adds a read
    /// at or past their revision has settled forgotten, and what the read holds of the rest
    /// left to the read; and the gesture's points and curves found in the sketch again.
    fn settle(&mut self, context: &Context<'_>) {
        if self.sketch.as_ref() != Some(context.sketch_id) {
            if self.sketch.is_some() {
                self.pending.clear();
                self.woken.clear();
                self.restart();
            }
            self.sketch = Some(context.sketch_id.clone());
        }
        self.pending
            .retain(|p| p.revision.is_none_or(|made| made > context.revision));
        if !self.pending.is_empty() {
            let held: std::collections::BTreeSet<&ElementId> =
                context.sketch.elements().map(|(id, _)| id).collect();
            for p in &mut self.pending {
                let m = &mut p.members;
                m.points.retain(|x| !held.contains(&x.id));
                m.lines.retain(|x| !held.contains(&x.id));
                m.circles.retain(|x| !held.contains(&x.id));
                m.arcs.retain(|x| !held.contains(&x.id));
                m.constraints.retain(|x| !held.contains(&x.id));
            }
        }
    }

    /// The members of every add the read may not hold yet.
    fn pending_members(&self) -> Members {
        let mut all = Members::default();
        for p in &self.pending {
            let m = &p.members;
            all.points.extend(m.points.iter().cloned());
            all.lines.extend(m.lines.iter().cloned());
            all.circles.extend(m.circles.iter().cloned());
            all.arcs.extend(m.arcs.iter().cloned());
            all.constraints.extend(m.constraints.iter().cloned());
        }
        all
    }

    /// `ids` woken, the first the newest, the oldest past [`WAKE`] forgotten.
    fn wake(&mut self, ids: impl DoubleEndedIterator<Item = ElementId>) {
        for id in ids.rev() {
            self.woken.retain(|w| *w != id);
            self.woken.insert(0, id);
        }
        self.woken.truncate(WAKE);
    }

    /// `event`, with `keys` held, against `context`: what it added, said, or ended; the
    /// preview updated. What it adds is taken to be in the sketch, for the events after it,
    /// until a read settles it ([`added`](Self::added), [`forget_pending`](Self::forget_pending)).
    pub fn handle(
        &mut self,
        context: &Context<'_>,
        ids: &mut dyn NewIds,
        event: Event,
        keys: Modifiers,
    ) -> Outcome {
        let _span = tracing::debug_span!("sketch_tool", tool = ?self.kind()).entered();
        if let Event::Press(at) | Event::Move(at) | Event::Release(at) | Event::DoubleClick(at) =
            event
            && !geometry::finite(at)
        {
            return Outcome::said(Message::NotFinite);
        }
        self.settle(context);
        let pending = self.pending_members();
        let view = infer::View::of(context.sketch, &pending);
        let tolerance = if context.tolerance.is_finite() && context.tolerance > 0.0 {
            context.tolerance
        } else {
            0.0
        };
        match &mut self.state {
            State::Line(t) => t.revalidate(&view),
            State::CornerRectangle(t) | State::CenterRectangle(t) => t.revalidate(&view),
            State::Circle(t) => t.revalidate(&view),
            State::CenterpointArc(t) => t.revalidate(&view),
            State::ThreePointArc(t) => t.revalidate(&view),
            State::TangentArc(t) => t.revalidate(&view),
            State::Point(_) => {}
        }
        if let Event::Press(at) | Event::Move(at) | Event::Release(at) = event {
            self.wake(view.points_near(at, tolerance).into_iter());
        }
        let woken = self.woken.clone();
        let mut step = Step {
            pointer: infer::Pointer {
                view: &view,
                tolerance,
                infer: context.automatic_relations && !keys.ctrl,
                woken: &woken,
            },
            shift: keys.shift,
            ids,
        };
        let mut preview = Preview::default();
        let outcome = match &mut self.state {
            State::Line(t) => t.handle(&mut step, event, &mut preview),
            State::CornerRectangle(t) | State::CenterRectangle(t) => {
                t.handle(&mut step, event, &mut preview)
            }
            State::Circle(t) => t.handle(&mut step, event, &mut preview),
            State::CenterpointArc(t) => t.handle(&mut step, event, &mut preview),
            State::ThreePointArc(t) => t.handle(&mut step, event, &mut preview),
            State::TangentArc(t) => t.handle(&mut step, event, &mut preview),
            State::Point(t) => t.handle(&mut step, event, &mut preview),
        };
        self.preview = preview;
        if let Some(added) = &outcome.add {
            // The pointer was at each point the tool placed: they're woken too.
            self.wake(added.points.iter().rev().map(|p| p.id.clone()));
            self.pending.push(Pending {
                members: added.clone(),
                revision: None,
            });
        }
        if outcome.left {
            self.woken.clear();
        }
        outcome
    }
}

/// A builder's failure as what the tool says: an ID the source couldn't make, or (a bug)
/// a coordinate that isn't finite.
fn failed(error: Error) -> Outcome {
    Outcome::said(Message::NoId(error))
}

/// Whether `at` is within a kilometre of the origin, as the gate's points are.
fn within_reach(at: Xy) -> bool {
    at[0].abs() <= silico_protocol::MAX_LENGTH && at[1].abs() <= silico_protocol::MAX_LENGTH
}

/// An internal error for a value the tools made that isn't finite.
fn not_finite(what: &str) -> Error {
    Error::new(
        ErrorCode::Internal,
        format!("a sketch tool made {what} that isn't a finite number"),
    )
    .with_suggestion(crate::worker::SILICO_BUG)
}
