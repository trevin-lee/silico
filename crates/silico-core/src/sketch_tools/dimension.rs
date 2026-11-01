//! Smart Dimension, headless (docs/design/m5.0-app.md, section 9, *Smart Dimension*; the
//! decisions SD1 to SD9; docs/study/smart-dimension.md, §11 and *For M5.0 and the
//! maintainer*): a state machine from the pointer's and the Modify box's events to the
//! `sketch_add` of one driving dimension, with the preview the viewport draws, and the offer
//! the app makes when the dimension placed over-defines the sketch (SD7). Behaviour is
//! SolidWorks' as the decisions take it:
//!
//! - **Picks.** A click on a point (the origin among them, a circle's or an arc's centre too)
//!   or a curve (a line or an arc where it's drawn, a circle all round, an axis anywhere)
//!   picks it, up to two; a click on nothing, on what's picked, or once two are picked,
//!   places the dimension the preview shows there, and opens the Modify box. Esc undoes the
//!   last pick, and with none leaves the tool ([`Outcome::left`]). A right-click locks the
//!   kind the preview shows: the pointer then moves the dimension, not its kind, until a pick
//!   changes. After a distance to a centreline, the centreline stays picked for the next, as
//!   the help's "additional dimensions" have it.
//! - **What is placed** (SD1), measured on the newest read's coordinates:
//!   - one line: `length`; one circle: `diameter`; one arc: `radius` (SD8: no toggle);
//!   - two points: level within [`LEVEL`], `distance_x`; plumb within it, `distance_y`
//!     (FreeCAD's `76a84f63ab`); otherwise `distance_x` with the pointer above or below the
//!     box the two span, `distance_y` with it left or right of the box, `distance` elsewhere
//!     (inside it, or off a corner), FreeCAD's zones. `distance_x` and `distance_y` ordered
//!     so the value starts positive, `a` the point to the left (below), the order picked
//!     where they're level; `distance` in the order picked;
//!   - a point and a line: `point_line_distance`, its `side` the side of the line the point
//!     is on as drawn (facing from `from` to `to`; an axis faces +x or +y);
//!   - a circle or an arc with anything else: as its centre (to a centre by default);
//!   - two lines that aren't parallel: `angle`, by the quadrant the pointer is in about
//!     where they meet, among each line's along-ray (leaving the end nearer, FreeCAD's rule)
//!     and its opposite: between the along-rays, the angle θ; opposite, the reflex
//!     360 − θ; beside, 180 − θ, with `reversed` where one ray runs against its line
//!     (Fable's chain-angle decision, §3, superseding SD5). Lines that cross take θ in
//!     both vertical sectors (§4): opposite is θ too, as drawn. Structure decides first
//!     (Main's ruling): lines the sketch ties where they meet, an end of one a point of
//!     the other or held on it (`on`, `midpoint`, `symmetric` about it), never cross, so
//!     a T-junction keeps the quadrant rule however its end rounds; lines it doesn't tie
//!     cross where each is strictly inside the other, compared without tolerance; a line
//!     and an axis never;
//!   - two parallel lines ([`PARALLEL`], FreeCAD's test): `point_line_distance` from the
//!     second line's `from` to the first line (SD4); with an axis picked second, the first
//!     line's `from` to the axis, Silico's own choice (an axis has no `from`);
//!   - a point, or a parallel line, and a centreline (a construction line, or an axis):
//!     `point_line_distance` with the pointer on its side; across it, a diameter (a doubled
//!     distance), which isn't offered (SD3), nor are an arc's length and the Min and Max gaps.
//! - **The Modify box** opens on placing, holding the measured value rounded to the
//!   nanometre (or nanodegree), as the document writes it. It takes plain numbers only, read
//!   as the document reads a value (SD9): a JSON number, at most the quantity's decimals, no
//!   units, arithmetic, or expressions. Enter places the dimension, `driving: true`, with the
//!   value typed (SD2); one the reader or the gate refuses is said, and the box stays open.
//!   Esc skips the box: the dimension is placed with the measured value, rounded by the core
//!   (SD2); if the gate refuses that, nothing is placed, and the box closes saying why. While
//!   it's open, each newer read measures its dimension again, its kind held: an untouched box
//!   shows the new value, a typed one keeps its text, and one the read leaves no dimension
//!   for closes, saying why.
//! - **Over-defining** (SD7). The add is made whatever the solve will find. Once the app has
//!   the `sketch` read of the revision the add made, [`offer`] gives SolidWorks' three
//!   choices if the add flagged a constraint the read before it didn't, left a solved
//!   sketch's degrees of freedom where they were (a dependent dimension), or took the sketch
//!   to over defined or to no solution (when nothing came between the two reads; else if the
//!   dimension itself is flagged): driven (`set_field` of its `driving` to false), leave it,
//!   or Cancel (`undo`). SolidWorks prompts for each dimension, so the offer is modal: no
//!   other dimension is placed, and the tool is [`busy`](SmartDimension::busy), until that
//!   read has come with no offer, or, with one ([`open_offer`](SmartDimension::open_offer)),
//!   until it's released (see `open_offer`). The app gives the tool each read
//!   ([`settle`](SmartDimension::settle)), so it settles without a pointer event.
//!
//! Adds are tracked as the sketch tools' are ([`super::SketchTool`]): each with a token the
//! viewport answers with the revision it made, or its refusal.

use super::adds::Adds;
use super::build::Builder;
use super::geometry::{cross, distance, dot, length, scale, sub, unit};
use super::infer::{Pointer, View};
use super::{AddToken, Context, Message, NewIds, Outcome, Xy, geometry};
use crate::command::Revision;
use crate::command::sketch::SketchRead;
use crate::document::json::{self, At, Node, Value};
use crate::document::sketch_edit::{self, Geo};
use crate::document::{
    Angle, ElementId, FeatureId, Form, Length, LineSide, Reference, SketchConstraint,
};
use crate::sketch_states::{ConstraintState, SketchState, SketchStates};
use crate::{Error, ErrorCode};
use std::collections::BTreeSet;

/// How nearly two lines' directions as drawn (each its `to` less its `from`, an axis's a
/// unit vector) must cross to nothing for the lines to be parallel, in square millimetres:
/// FreeCAD 1.1.3's, `Precision::Intersection()` on the same product in its dimension tool
/// (`calculateAngle`, CommandConstraints.cpp, at the clone's `876271d660`), which then
/// writes the distance SD4 takes. Lines a relation holds parallel are solved so.
pub const PARALLEL: f64 = 1e-9;

/// How nearly two points' y (or x) must agree for them to be level (or plumb), in
/// millimetres: FreeCAD's `Precision::Confusion()`, with which its tool gives such points
/// their horizontal (or vertical) distance wherever the pointer is (`76a84f63ab`).
pub const LEVEL: f64 = 1e-7;

/// How much of a refused number's text a message repeats, in characters.
const SHOWN: usize = 20;

/// What Smart Dimension takes: the pointer's events in the sketch's plane, a right-click,
/// and the Modify box's keys and text.
#[derive(Debug, Clone, PartialEq)]
pub enum DimensionEvent {
    Press(Xy),
    Move(Xy),
    Release(Xy),
    /// A right-click: locks the kind the preview shows.
    RightClick(Xy),
    /// Undoes the last pick; with the Modify box open, skips it.
    Escape,
    /// With the Modify box open, places the dimension with its text's value.
    Enter,
    /// The Modify box's text, whole, as it now reads.
    Typed(String),
}

/// What the viewport draws for Smart Dimension.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DimensionPreview {
    /// Where the pointer is: where the dimension's text goes.
    pub pointer: Option<Xy>,
    /// What's picked, in the order picked.
    pub picks: Vec<Reference>,
    /// What a click here would pick.
    pub hover: Option<Reference>,
    /// The dimension a click here would place (or the Modify box's), as its form, with its
    /// measured value.
    pub dimension: Option<Form>,
    /// Whether a right-click has locked its kind.
    pub locked: bool,
    /// Why nothing is offered here, for what's picked.
    pub unavailable: Option<Message>,
    /// The Modify box's text, while it's open.
    pub modify: Option<String>,
}

/// A dimension Smart Dimension placed, once the core answered its add: the sketch, the
/// constraint's ID, the revision the add made, and, of the read it was placed on, its
/// revision, the sketch's state and the constraints it flagged (conflicting, redundant, or
/// partially redundant), and its degrees of freedom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub sketch: FeatureId,
    pub constraint: ElementId,
    pub revision: Revision,
    /// The revision of the read it was placed on.
    pub read: Revision,
    pub before: SketchState,
    pub flagged: BTreeSet<ElementId>,
    /// The read's degrees of freedom.
    pub dofs: i32,
}

/// Smart Dimension, between events. Not `Clone`, as a sketch tool isn't.
#[derive(Debug)]
pub struct SmartDimension {
    adds: Adds,
    picks: Vec<Reference>,
    lock: Option<Measure>,
    pressed: bool,
    pointer: Option<Xy>,
    modify: Option<Modify>,
    placed: Option<Placed>,
    preview: DimensionPreview,
}

/// The Modify box, open.
#[derive(Debug, Clone)]
struct Modify {
    /// The kind it measures, held while it's open.
    measure: Measure,
    /// Whether the person locked it: then a centreline's side doesn't govern it.
    locked: bool,
    /// Where it was placed.
    at: Xy,
    /// How many picks it measures.
    picks: usize,
    /// The dimension, measured on the newest read.
    form: Form,
    text: String,
    /// Whether the person has typed in it: then a newer read's value doesn't replace its
    /// text.
    touched: bool,
}

/// The last dimension placed: its add's token, its constraint, the revision the add made
/// once answered, the read it was placed on (its revision, the sketch's state, its flagged
/// constraints, its degrees of freedom), and whether its offer is settled (a read of its
/// revision has come with none) or open.
#[derive(Debug, Clone)]
struct Placed {
    token: AddToken,
    constraint: ElementId,
    revision: Option<Revision>,
    read: Revision,
    before: SketchState,
    flagged: BTreeSet<ElementId>,
    dofs: i32,
    /// Its read has come with no offer: another dimension may be placed.
    settled: bool,
    /// Its read has come with an offer, which holds the tool until it's released.
    offer: Option<Offer>,
}

impl Placed {
    /// As the app reads it, in `sketch`, once its add is answered.
    fn placement(&self, sketch: &FeatureId) -> Option<Placement> {
        Some(Placement {
            sketch: sketch.clone(),
            constraint: self.constraint.clone(),
            revision: self.revision?,
            read: self.read,
            before: self.before,
            flagged: self.flagged.clone(),
            dofs: self.dofs,
        })
    }
}

/// An ID source for a step that places nothing ([`SmartDimension::settle`]).
struct NoIds;

impl NewIds for NoIds {
    fn next(&mut self, _: &dyn Fn(&ElementId) -> bool) -> Result<ElementId, Error> {
        Err(Error::new(
            ErrorCode::Internal,
            "Smart Dimension asked for an ID while settling a read",
        )
        .with_suggestion(crate::worker::SILICO_BUG))
    }
}

/// Which dimension of those a pick offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Measure {
    Length,
    Radius,
    Diameter,
    Distance,
    DistanceX,
    DistanceY,
    PointLine,
    /// The quadrant about the lines' meeting the pointer is in ([`Quadrant`]).
    Angle(Quadrant),
}

/// Of the four rays from where two lines meet, each line's along-ray (leaving the end
/// nearer the meeting) and its opposite, the quadrant a dimension's arc is in, taking the
/// along-rays in counter-clockwise order, `r1` then `r2` (Fable's chain-angle decision, §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quadrant {
    /// Between the along-rays, `r1` to `r2`: the angle θ between them.
    Between,
    /// From `r2` to −`r1`: 180 − θ.
    After,
    /// From −`r1` to −`r2`, opposite: the reflex, 360 − θ, from `r2` round to `r1`.
    Opposite,
    /// From −`r2` to `r1`: 180 − θ.
    Before,
}

/// A picked thing, as the read has it.
#[derive(Debug, Clone)]
enum Item {
    /// A point, or the origin.
    Point(Reference, Xy),
    /// A line, or an axis: where it starts and ends (an axis's a unit from the origin), its
    /// direction (to less from; an axis's unit), its `from` point (none for an axis, which is
    /// infinite), and whether it's a centreline (construction, or an axis).
    Line {
        reference: Reference,
        at: Xy,
        end: Xy,
        direction: Xy,
        from: Option<ElementId>,
        /// Its `to` point (none for an axis).
        to: Option<ElementId>,
        centreline: bool,
    },
    /// A circle or an arc: its centre point and where it is, and its radius.
    Round {
        curve: ElementId,
        arc: bool,
        centre: ElementId,
        centre_at: Xy,
        radius: f64,
    },
}

impl Item {
    fn of(view: &View<'_>, reference: &Reference) -> Option<Item> {
        let id = match reference {
            Reference::Origin => return Some(Item::Point(Reference::Origin, [0.0, 0.0])),
            Reference::XAxis | Reference::YAxis => {
                let direction = if *reference == Reference::XAxis {
                    [1.0, 0.0]
                } else {
                    [0.0, 1.0]
                };
                return Some(Item::Line {
                    reference: reference.clone(),
                    at: [0.0, 0.0],
                    end: direction,
                    direction,
                    from: None,
                    to: None,
                    centreline: true,
                });
            }
            Reference::Element(id) => id,
        };
        if let Some(at) = view.point(id) {
            return Some(Item::Point(reference.clone(), at));
        }
        let sketch = &view.sketch;
        match view.geo(reference)? {
            Geo::Line { from, to } => {
                let line = sketch.lines.iter().find(|l| &l.id == id)?;
                Some(Item::Line {
                    reference: reference.clone(),
                    at: from,
                    end: to,
                    direction: sub(to, from),
                    from: Some(line.from.clone()),
                    to: Some(line.to.clone()),
                    centreline: line.construction,
                })
            }
            Geo::Circle { center, radius } => {
                let circle = sketch.circles.iter().find(|c| &c.id == id)?;
                Some(Item::Round {
                    curve: id.clone(),
                    arc: false,
                    centre: circle.center.clone(),
                    centre_at: center,
                    radius,
                })
            }
            Geo::Arc { center, radius, .. } => {
                let arc = sketch.arcs.iter().find(|a| &a.id == id)?;
                Some(Item::Round {
                    curve: id.clone(),
                    arc: true,
                    centre: arc.center.clone(),
                    centre_at: center,
                    radius,
                })
            }
        }
    }

    /// As a second pick sees it: a circle or an arc as its centre (to a centre by default).
    fn centred(self) -> Item {
        match self {
            Item::Round {
                centre, centre_at, ..
            } => Item::Point(Reference::Element(centre), centre_at),
            other => other,
        }
    }

    fn is_centreline(&self) -> bool {
        matches!(
            self,
            Item::Line {
                centreline: true,
                ..
            }
        )
    }
}

/// A dimension offered: which, and its form with the measured value.
struct Offered {
    measure: Measure,
    form: Form,
}

/// Why no dimension is offered: something to say, or nothing (too little picked).
type NotOffered = Option<Message>;

impl Default for SmartDimension {
    fn default() -> Self {
        SmartDimension::new()
    }
}

impl SmartDimension {
    pub fn new() -> Self {
        SmartDimension {
            adds: Adds::new(),
            picks: Vec::new(),
            lock: None,
            pressed: false,
            pointer: None,
            modify: None,
            placed: None,
            preview: DimensionPreview::default(),
        }
    }

    /// What to draw now.
    pub fn preview(&self) -> &DimensionPreview {
        &self.preview
    }

    /// Whether something is under way: a pick held, a press down, the Modify box open, or a
    /// dimension placed that isn't settled: no read of its add's revision yet, or an offer
    /// open ([`open_offer`](Self::open_offer)). While that lasts no other dimension is
    /// placed: the offer is modal, as SolidWorks' prompt is, so each dimension's offer is
    /// judged against the read it was placed on, and each is answered before the next.
    pub fn busy(&self) -> bool {
        !self.picks.is_empty()
            || self.pressed
            || self.modify.is_some()
            || self.placed.as_ref().is_some_and(|p| !p.settled)
    }

    /// The core took the add `token` names, at `revision`.
    pub fn added(&mut self, token: AddToken, revision: Revision) {
        self.adds.added(token, revision);
        if let Some(placed) = &mut self.placed
            && placed.token == token
        {
            placed.revision = Some(revision);
        }
    }

    /// The core refused the add `token` names: it's forgotten.
    pub fn refused(&mut self, token: AddToken) {
        self.adds.refused(token);
        if self.placed.as_ref().is_some_and(|p| p.token == token) {
            self.placed = None;
        }
    }

    /// Forgets every add no read has shown, the last placement, and what's under way, as
    /// [`SketchTool::forget_pending`](super::SketchTool::forget_pending) does.
    pub fn forget_pending(&mut self) {
        self.adds.clear();
        self.placed = None;
        self.restart();
    }

    /// The person has answered the last placement's offer (the app calls this once Driven's
    /// or Cancel's request is sent, or on Leave), or the app dismisses it: it's forgotten,
    /// and the next dimension may be placed.
    pub fn dismiss_offer(&mut self) {
        self.placed = None;
    }

    /// The offer open for the last dimension placed (SD7), which the app shows and which
    /// holds the tool: one state for both. Made when the tool is given the read of the
    /// revision the add made, if [`offer`] gives one there. Released by
    /// [`dismiss_offer`](Self::dismiss_offer), by [`refused`](Self::refused) or
    /// [`forget_pending`](Self::forget_pending), by a context of another sketch, and by any
    /// read past the add's revision: the moment has passed, as such a read can't open one.
    /// Its requests are made against the add's revision, so once the session has moved on
    /// the core would refuse Driven and Cancel as a conflict, and Cancel sent again at the
    /// newer revision would undo whatever came since. A sketch still over defined shows so
    /// in its state.
    pub fn open_offer(&self) -> Option<&Offer> {
        self.placed.as_ref()?.offer.as_ref()
    }

    /// The last dimension placed, once the core has answered its add: for [`offer`].
    pub fn placement(&self) -> Option<Placement> {
        self.placed.as_ref()?.placement(self.adds.sketch()?)
    }

    /// The tool afresh: nothing picked, no box, no lock.
    fn restart(&mut self) {
        self.picks.clear();
        self.lock = None;
        self.pressed = false;
        self.modify = None;
    }

    /// `event` against `context`, whose read gives the sketch the states `states`: what it
    /// added, said, or ended; the preview updated.
    pub fn handle(
        &mut self,
        context: &Context<'_>,
        states: &SketchStates,
        ids: &mut dyn NewIds,
        event: DimensionEvent,
    ) -> Outcome {
        self.step(context, states, ids, Some(event))
    }

    /// A newer read, given with no event: what [`handle`](Self::handle) does with each
    /// read, so the app calls it as each read comes and the tool settles the last
    /// placement's offer (and [`busy`](Self::busy)) and measures its open box again without
    /// waiting for the pointer. What it says is the box's closing, if the read closes it.
    pub fn settle(&mut self, context: &Context<'_>, states: &SketchStates) -> Outcome {
        self.step(context, states, &mut NoIds, None)
    }

    fn step(
        &mut self,
        context: &Context<'_>,
        states: &SketchStates,
        ids: &mut dyn NewIds,
        event: Option<DimensionEvent>,
    ) -> Outcome {
        let _span = tracing::debug_span!("smart_dimension").entered();
        if let Some(
            DimensionEvent::Press(at)
            | DimensionEvent::Move(at)
            | DimensionEvent::Release(at)
            | DimensionEvent::RightClick(at),
        ) = event
            && !geometry::finite(at)
        {
            return Outcome::said(Message::NotFinite);
        }
        if self.adds.settle(context) {
            self.placed = None;
            self.restart();
        }
        // An open offer is past its moment once a later read comes: its requests are against
        // the add's revision.
        if self.placed.as_ref().is_some_and(|p| {
            p.offer.is_some() && p.revision.is_some_and(|made| context.revision > made)
        }) {
            self.placed = None;
        }
        if let Some(placed) = &mut self.placed
            && placed.offer.is_none()
            && let Some(made) = placed.revision
            && context.revision >= made
        {
            // The add's own read decides whether an offer holds the tool; a later one, which
            // can't offer (the moment's past), settles it.
            let offer = if context.revision == made {
                placed.placement(context.sketch_id).and_then(|placement| {
                    offer_of(context.sketch_id, context.sketch, states, made, &placement)
                })
            } else {
                None
            };
            placed.settled = offer.is_none();
            placed.offer = offer;
        }
        let pending = self.adds.members();
        let view = View::of(context.sketch, &pending);
        let closed = self.revalidate(&view);
        let tolerance = if context.tolerance.is_finite() && context.tolerance > 0.0 {
            context.tolerance
        } else {
            0.0
        };
        let pointer = Pointer {
            view: &view,
            tolerance,
            infer: true,
            woken: &[],
        };
        if let Some(
            DimensionEvent::Press(at)
            | DimensionEvent::Move(at)
            | DimensionEvent::Release(at)
            | DimensionEvent::RightClick(at),
        ) = event
        {
            self.pointer = Some(at);
        }
        let Some(event) = event else {
            self.preview = self.drawn(&view, &pointer);
            return Outcome {
                message: closed,
                ..Outcome::default()
            };
        };
        let mut outcome = match event {
            DimensionEvent::Typed(text) => {
                if let Some(modify) = &mut self.modify {
                    modify.text = text;
                    modify.touched = true;
                }
                Outcome::default()
            }
            DimensionEvent::Enter => match self.modify.take() {
                Some(modify) => {
                    let placed = typed(&modify.form, &modify.text)
                        .and_then(|form| self.place(&view, ids, form, context.revision, states));
                    placed.unwrap_or_else(|message| {
                        self.modify = Some(modify);
                        Outcome::said(message)
                    })
                }
                None => Outcome::default(),
            },
            DimensionEvent::Escape => match self.modify.take() {
                Some(modify) => self
                    .place(&view, ids, modify.form, context.revision, states)
                    .unwrap_or_else(Outcome::said),
                None if self.picks.pop().is_some() => {
                    self.lock = None;
                    self.pressed = false;
                    Outcome::default()
                }
                None => {
                    self.restart();
                    Outcome::left()
                }
            },
            _ if self.modify.is_some() => Outcome::default(),
            DimensionEvent::Press(_) => {
                self.pressed = true;
                Outcome::default()
            }
            DimensionEvent::Move(_) => Outcome::default(),
            DimensionEvent::Release(at) if self.pressed => {
                self.pressed = false;
                self.click(&view, &pointer, at)
            }
            DimensionEvent::Release(_) => Outcome::default(),
            DimensionEvent::RightClick(at) => {
                if let Ok(offered) = offered(&view, &self.picks, at, None, false) {
                    self.lock = Some(offered.measure);
                }
                Outcome::default()
            }
        };
        if outcome.message.is_none() {
            outcome.message = closed;
        }
        self.preview = self.drawn(&view, &pointer);
        outcome
    }

    /// The picks and the box against `view`, a newer read: a pick it no longer has dropped
    /// (and the lock with it); the box's dimension measured again, its kind held, an
    /// untouched box's text with it, a typed one's kept. The box closes, saying why, if the
    /// read leaves it no dimension, or one of another kind (a pick dropped, lines made
    /// parallel), which a value typed for the first mustn't be carried onto.
    fn revalidate(&mut self, view: &View<'_>) -> Option<Message> {
        let before = self.picks.len();
        self.picks.retain(|r| Item::of(view, r).is_some());
        if self.picks.len() != before {
            self.lock = None;
        }
        let modify = self.modify.as_mut()?;
        if self.picks.len() != modify.picks {
            self.modify = None;
            return Some(Message::DimensionChanged);
        }
        match offered(
            view,
            &self.picks,
            modify.at,
            Some(modify.measure),
            modify.locked,
        ) {
            Ok(offered)
                if std::mem::discriminant(&offered.measure)
                    != std::mem::discriminant(&modify.measure) =>
            {
                self.modify = None;
                Some(Message::DimensionChanged)
            }
            Ok(offered) => {
                modify.measure = offered.measure;
                modify.form = offered.form;
                if !modify.touched {
                    modify.text = value_text(&modify.form).unwrap_or_default();
                }
                None
            }
            Err(said) => {
                self.modify = None;
                said
            }
        }
    }

    /// A click at `at`: a pick, while fewer than two are picked and it's on something not
    /// picked; else the dimension placed, its Modify box opened, unless the last placement's
    /// offer isn't settled (the offer is modal).
    fn click(&mut self, view: &View<'_>, pointer: &Pointer<'_, '_>, at: Xy) -> Outcome {
        if self.picks.len() < 2
            && let Some(hit) = pointer.pick(at)
            && !self.picks.contains(&hit)
        {
            self.picks.push(hit);
            self.lock = None;
            return Outcome::default();
        }
        let locked = self.lock == Some(Measure::PointLine);
        match offered(view, &self.picks, at, self.lock, locked) {
            Ok(_) if self.placed.as_ref().is_some_and(|p| p.offer.is_some()) => {
                Outcome::said(Message::OfferOpen)
            }
            Ok(_) if self.placed.as_ref().is_some_and(|p| !p.settled) => {
                Outcome::said(Message::Solving)
            }
            Ok(offered) => {
                self.modify = Some(Modify {
                    measure: offered.measure,
                    locked,
                    at,
                    picks: self.picks.len(),
                    text: value_text(&offered.form).unwrap_or_default(),
                    form: offered.form,
                    touched: false,
                });
                Outcome::default()
            }
            Err(Some(message)) => Outcome::said(message),
            Err(None) => Outcome::default(),
        }
    }

    /// `form` added as a new constraint, unless the gate, given the sketch with it, refuses
    /// the constraint itself (its value out of range, its `side`, what it names): the add,
    /// its token, and the tool afresh for the next dimension, a centreline it measured from
    /// kept picked; else the gate's words. A problem the gate finds elsewhere (a limit,
    /// another member) is the core's to answer, so the add is made and its refusal comes
    /// back ([`refused`](Self::refused)). `read` is the revision of the read it's placed on,
    /// and `states` that read's.
    fn place(
        &mut self,
        view: &View<'_>,
        ids: &mut dyn NewIds,
        form: Form,
        read: Revision,
        states: &SketchStates,
    ) -> Result<Outcome, Message> {
        let mut builder = Builder::new(ids, view);
        builder.constraint(form).map_err(Message::NoId)?;
        let members = builder.members;
        let Some(constraint) = members.constraints.first() else {
            return Err(Message::NoId(
                Error::new(
                    ErrorCode::Internal,
                    "a dimension was built with no constraint",
                )
                .with_suggestion(crate::worker::SILICO_BUG),
            ));
        };
        let id = constraint.id.clone();
        if let Err(refused) = sketch_edit::add(&view.sketch, &members) {
            let own = format!("constraints.{id}");
            let about = |e: &Error| {
                e.parameter()
                    .is_some_and(|p| p == own || p.starts_with(&format!("{own}.")))
            };
            if let Some(error) = refused.errors.into_iter().find(about) {
                return Err(Message::ValueRefused(error));
            }
        }
        // The help's centreline dimensions: "Select other lines or points in the sketch to
        // create additional dimensions."
        let kept = match &constraint.form {
            Form::PointLineDistance { .. } => self
                .picks
                .iter()
                .find(|r| Item::of(view, r).is_some_and(|i| i.is_centreline()))
                .cloned(),
            _ => None,
        };
        let token = self.adds.issue(&members);
        self.placed = Some(Placed {
            token,
            constraint: id,
            revision: None,
            read,
            before: states.state,
            flagged: flagged(states),
            dofs: states.dofs,
            settled: false,
            offer: None,
        });
        self.restart();
        self.picks.extend(kept);
        Ok(Outcome {
            add: Some(members),
            token: Some(token),
            ..Outcome::default()
        })
    }

    /// The preview, at the pointer.
    fn drawn(&self, view: &View<'_>, pointer: &Pointer<'_, '_>) -> DimensionPreview {
        let mut preview = DimensionPreview {
            pointer: self.pointer,
            picks: self.picks.clone(),
            locked: self.lock.is_some(),
            ..DimensionPreview::default()
        };
        if let Some(modify) = &self.modify {
            preview.dimension = Some(modify.form.clone());
            preview.modify = Some(modify.text.clone());
            return preview;
        }
        let Some(at) = self.pointer else {
            return preview;
        };
        if self.picks.len() < 2 {
            preview.hover = pointer.pick(at).filter(|r| !self.picks.contains(r));
        }
        let locked = self.lock == Some(Measure::PointLine);
        match offered(view, &self.picks, at, self.lock, locked) {
            Ok(offered) => preview.dimension = Some(offered.form),
            Err(message) => preview.unavailable = message,
        }
        preview
    }
}

/// A dimension's value as the document writes it, and the Modify box shows it: none for a
/// form with none.
fn value_text(form: &Form) -> Option<String> {
    match form {
        Form::Angle { value, .. } => Some(value.to_string()),
        Form::Length { value, .. }
        | Form::Distance { value, .. }
        | Form::DistanceX { value, .. }
        | Form::DistanceY { value, .. }
        | Form::PointLineDistance { value, .. }
        | Form::Radius { value, .. }
        | Form::Diameter { value, .. } => Some(value.to_string()),
        _ => None,
    }
}

/// The dimension `picks` offer with the pointer at `at`, `lock` the kind locked, if any and
/// if it's one of theirs; or why none. `held`: a lock holds a distance across a centreline
/// too, the pointer then moving only the dimension.
fn offered(
    view: &View<'_>,
    picks: &[Reference],
    at: Xy,
    lock: Option<Measure>,
    held: bool,
) -> Result<Offered, NotOffered> {
    let items: Vec<Item> = picks.iter().filter_map(|r| Item::of(view, r)).collect();
    match items.as_slice() {
        [one] => single(one),
        [first, second] => {
            let (first, second) = (first.clone().centred(), second.clone().centred());
            if picks.iter().all(|r| r.element().is_none()) {
                return Err(Some(Message::FixedOnly));
            }
            let side_at = (!held).then_some(at);
            match (&first, &second) {
                (Item::Point(..), Item::Point(..)) => points(&first, &second, at, lock),
                (Item::Point(..), Item::Line { .. }) => point_line(&first, &second, side_at),
                (Item::Line { .. }, Item::Point(..)) => point_line(&second, &first, side_at),
                (Item::Line { .. }, Item::Line { .. }) => {
                    let tied = tied(view, &first, &second);
                    lines(&first, &second, at, side_at, lock, tied)
                }
                _ => Err(None),
            }
        }
        _ => Err(None),
    }
}

/// One pick: a line's length, a circle's diameter, an arc's radius (SD1, SD8); nothing for
/// a point or an axis.
fn single(item: &Item) -> Result<Offered, NotOffered> {
    let element = |r: &Reference| r.element().is_some();
    match item {
        Item::Line {
            reference,
            direction,
            ..
        } if element(reference) => Ok(Offered {
            measure: Measure::Length,
            form: Form::Length {
                line: reference.clone(),
                value: rounded(length(*direction))?,
                driving: true,
            },
        }),
        Item::Round {
            curve, arc, radius, ..
        } => {
            let curve = Reference::Element(curve.clone());
            Ok(if *arc {
                Offered {
                    measure: Measure::Radius,
                    form: Form::Radius {
                        curve,
                        value: rounded(*radius)?,
                        driving: true,
                    },
                }
            } else {
                Offered {
                    measure: Measure::Diameter,
                    form: Form::Diameter {
                        curve,
                        value: rounded(2.0 * *radius)?,
                        driving: true,
                    },
                }
            })
        }
        _ => Err(None),
    }
}

/// Two points: the locked kind; else level within [`LEVEL`], `distance_x`, plumb within it,
/// `distance_y` (FreeCAD's `76a84f63ab`, level first); else `distance_x` with the pointer
/// above or below the box they span, `distance_y` with it left or right of it, `distance`
/// elsewhere. `distance_x` and `distance_y` with `a` the point to the left (below), so the
/// value starts positive, the order picked where they're level with each other; `distance`
/// in the order picked, refused where the two are at one place (no direction to hold them
/// apart in).
fn points(
    first: &Item,
    second: &Item,
    at: Xy,
    lock: Option<Measure>,
) -> Result<Offered, NotOffered> {
    let (Item::Point(a, p), Item::Point(b, q)) = (first, second) else {
        return Err(None);
    };
    let within = |v: f64, i: usize| p[i].min(q[i]) <= v && v <= p[i].max(q[i]);
    let measure = match lock {
        Some(m @ (Measure::Distance | Measure::DistanceX | Measure::DistanceY)) => m,
        _ if (p[1] - q[1]).abs() < LEVEL => Measure::DistanceX,
        _ if (p[0] - q[0]).abs() < LEVEL => Measure::DistanceY,
        _ if within(at[0], 0) && !within(at[1], 1) => Measure::DistanceX,
        _ if within(at[1], 1) && !within(at[0], 0) => Measure::DistanceY,
        _ => Measure::Distance,
    };
    let ordered = |i: usize| {
        if q[i] < p[i] {
            (b.clone(), a.clone(), p[i] - q[i])
        } else {
            (a.clone(), b.clone(), q[i] - p[i])
        }
    };
    let form = match measure {
        Measure::DistanceX => {
            let (a, b, value) = ordered(0);
            Form::DistanceX {
                a,
                b,
                value: rounded(value)?,
                driving: true,
            }
        }
        Measure::DistanceY => {
            let (a, b, value) = ordered(1);
            Form::DistanceY {
                a,
                b,
                value: rounded(value)?,
                driving: true,
            }
        }
        _ => {
            let value = rounded(distance(*p, *q))?;
            if value.nm() == 0 {
                return Err(Some(Message::NoDistance));
            }
            Form::Distance {
                a: a.clone(),
                b: b.clone(),
                value,
                driving: true,
            }
        }
    };
    Ok(Offered { measure, form })
}

/// Whether the pointer `at` is across `line` from `off`, a signed distance from it (to its
/// left, facing along it, positive).
fn across(line_at: Xy, along: Xy, off: f64, at: Xy) -> bool {
    cross(along, sub(at, line_at)) * off < 0.0
}

/// A point and a line: `point_line_distance`, the side the point is on as drawn; refused
/// for a point on the line (at the nanometre: no side to hold it on). With the pointer `at`
/// across a centreline from the point, a diameter, which isn't offered (SD3); `at` none
/// where the side doesn't govern (a lock, or SD4's lines, which check their own).
fn point_line(point: &Item, line: &Item, at: Option<Xy>) -> Result<Offered, NotOffered> {
    let (
        Item::Point(p_ref, p),
        Item::Line {
            reference,
            at: start,
            direction,
            centreline,
            ..
        },
    ) = (point, line)
    else {
        return Err(None);
    };
    let along = unit(*direction).ok_or(None)?;
    let off = cross(along, sub(*p, *start));
    let value = rounded(off.abs())?;
    if value.nm() == 0 {
        return Err(Some(Message::NoDistance));
    }
    if let Some(at) = at
        && *centreline
        && across(*start, along, off, at)
    {
        return Err(Some(Message::AcrossCentreline));
    }
    Ok(Offered {
        measure: Measure::PointLine,
        form: Form::PointLineDistance {
            point: p_ref.clone(),
            line: reference.clone(),
            value,
            side: if off > 0.0 {
                LineSide::Left
            } else {
                LineSide::Right
            },
            driving: true,
        },
    })
}

/// Whether the place `over / under` along `line`, from its start (0) to its end (1), is
/// strictly inside it, as the two crosses stand, with no division and without tolerance
/// (Main's decision: 1 nm inside an end crosses, 1 nm outside doesn't). Never for an axis,
/// which runs through every vertex.
fn inside(line: &Item, over: f64, under: f64) -> bool {
    let Item::Line { from, .. } = line else {
        return false;
    };
    from.is_some() && over != 0.0 && (over > 0.0) == (under > 0.0) && over.abs() < under.abs()
}

/// Whether the sketch ties two lines where they meet (Main's ruling, structure before
/// geometry): an end of one is held on the other by `on` or `midpoint`, as a snap or the
/// line tool leaves it, or by a `symmetric` of the other's two ends about it (its middle,
/// as `midpoint` holds it). Such lines never cross, whatever their coordinates' last bits
/// say. A hold counts as the document writes it, met by the coordinates or not: structure
/// first.
/// An end that's a point of the other line is tied too, but needs no look at the document:
/// a point has one pair of coordinates, so the lines meet exactly there, at a shared end,
/// which [`lines`] finds without arithmetic and never counts as crossing.
fn tied(view: &View<'_>, first: &Item, second: &Item) -> bool {
    let line = |item: &Item| match item {
        Item::Line {
            reference,
            from,
            to,
            ..
        } => Some((reference.clone(), [from.clone(), to.clone()])),
        _ => None,
    };
    let (Some((a, a_ends)), Some((b, b_ends))) = (line(first), line(second)) else {
        return false;
    };
    let ends = |e: &[Option<ElementId>; 2]| e.iter().flatten().cloned().collect::<Vec<_>>();
    let (a_ends, b_ends) = (ends(&a_ends), ends(&b_ends));
    // The end `end` held on the line `on`, whose ends are `on_ends`.
    let held = |end: &ElementId, on: &Reference, on_ends: &[ElementId]| {
        let end = Reference::Element(end.clone());
        let an_end = |r: &Reference| r.element().is_some_and(|id| on_ends.contains(id));
        view.sketch.constraints.iter().any(|c| match &c.form {
            Form::On { point, curve } => *point == end && curve == on,
            Form::Midpoint { point, line } => *point == end && line == on,
            Form::Symmetric { a, b, about } => *about == end && a != b && an_end(a) && an_end(b),
            _ => false,
        })
    };
    a_ends.iter().any(|p| held(p, &b, &b_ends)) || b_ends.iter().any(|p| held(p, &a, &a_ends))
}

/// Two lines: parallel ([`PARALLEL`], FreeCAD's test), the second's `from` to the first
/// (SD4), or the first's `from` to an axis picked second, Silico's own choice (an axis has
/// no `from`); refused, as a point is, with the pointer `side_at` across a centreline from
/// the other line. Otherwise the angle, by the quadrant the pointer is in, or the one
/// locked (Fable's chain-angle decision, §3, superseding SD5):
/// - the vertex is where the lines meet: a point they share exactly, else their crossing;
/// - each line's along-ray leaves the end nearer the vertex (FreeCAD's `calculateAngle`:
///   its start where that's strictly nearer, else its end), an axis's along it;
/// - the along-rays in counter-clockwise order, `r1` then `r2`, θ the sweep between them:
///   between them, θ; opposite, the reflex 360 − θ, or θ where the lines cross (both
///   vertical sectors are drawn, Fable's §4): not `tied` ([`tied`]), and each strictly
///   inside the other; either quadrant beside, 180 − θ;
/// - written so the value is the sweep counter-clockwise from the first ray to the second,
///   `a` the first ray's line and `b` the second's, `reversed` where exactly one of the two
///   rays runs against its line (`b` taken the other way gives the same angle as `a`).
fn lines(
    first: &Item,
    second: &Item,
    at: Xy,
    side_at: Option<Xy>,
    lock: Option<Measure>,
    tied: bool,
) -> Result<Offered, NotOffered> {
    let (
        Item::Line {
            reference: ra,
            at: pa,
            end: ea,
            direction: da,
            from: fa,
            ..
        },
        Item::Line {
            reference: rb,
            at: pb,
            end: eb,
            direction: db,
            from: fb,
            ..
        },
    ) = (first, second)
    else {
        return Err(None);
    };
    let crossed = cross(*da, *db);
    if crossed.abs() < PARALLEL {
        // A centreline and a line on its other side: a diameter, not offered (SD3).
        let centreline = match (first.is_centreline(), second.is_centreline()) {
            (true, false) => Some((*pa, *da, *pb)),
            (false, true) => Some((*pb, *db, *pa)),
            _ => None,
        };
        if let (Some(at), Some((c_at, c_dir, other))) = (side_at, centreline)
            && let Some(along) = unit(c_dir)
            && across(c_at, along, cross(along, sub(other, c_at)), at)
        {
            return Err(Some(Message::AcrossCentreline));
        }
        // SD4: a point of one, the other's line; an axis has no `from`.
        let (from, at_from, line) = match (fb, fa) {
            (Some(from), _) => (from, *pb, first),
            (None, Some(from)) => (from, *pa, second),
            (None, None) => return Err(None),
        };
        let point = Item::Point(Reference::Element(from.clone()), at_from);
        return point_line(&point, line, None);
    }
    // The vertex: an end the two share exactly, else where the lines cross, pa + t·da.
    let shared = [*pa, *ea]
        .into_iter()
        .find(|p| fa.is_some() && fb.is_some() && (*p == *pb || *p == *eb));
    // Where the lines meet along each, pa + t·da and pb + s·db: t and s as the exact ratios
    // of their crosses to `crossed`.
    let between = sub(*pb, *pa);
    let (t_over, s_over) = (cross(between, *db), cross(between, *da));
    let t = t_over / crossed;
    let vertex = shared.unwrap_or([pa[0] + t * da[0], pa[1] + t * da[1]]);
    // Crossing: lines the sketch doesn't tie, the vertex strictly inside both segments (an
    // axis, or an end they share where they are, isn't).
    let crossing = !tied
        && shared.is_none()
        && inside(first, t_over, crossed)
        && inside(second, s_over, crossed);
    // Each along-ray's sign: +1 leaving `from` (strictly nearer the vertex), −1 leaving `to`;
    // an axis's runs along it.
    let sign = |from: &Option<ElementId>, start: Xy, end: Xy| {
        if from.is_none() || distance(vertex, start) < distance(vertex, end) {
            1.0
        } else {
            -1.0
        }
    };
    let (sa, sb) = (sign(fa, *pa, *ea), sign(fb, *pb, *eb));
    let (ray_a, ray_b) = (scale(*da, sa), scale(*db, sb));
    // The along-rays in counter-clockwise order: (line, ray's sign, ray).
    let (r1, r2) = if cross(ray_a, ray_b) > 0.0 {
        ((ra, sa, ray_a), (rb, sb, ray_b))
    } else {
        ((rb, sb, ray_b), (ra, sa, ray_a))
    };
    let from_r1 = |v: Xy| {
        let d = libm::atan2(cross(r1.2, v), dot(r1.2, v)).to_degrees();
        if d < 0.0 { d + 360.0 } else { d }
    };
    let theta = from_r1(r2.2);
    let quadrant = match lock {
        Some(Measure::Angle(quadrant)) => quadrant,
        _ => {
            let toward = sub(at, vertex);
            let turned = from_r1(toward);
            if length(toward) == 0.0 || turned <= theta {
                Quadrant::Between
            } else if turned <= 180.0 {
                Quadrant::After
            } else if turned <= 180.0 + theta {
                Quadrant::Opposite
            } else {
                Quadrant::Before
            }
        }
    };
    // Exactly one ray against its line: `reversed`.
    let odd = r1.1 != r2.1;
    let (a, b, reversed, value) = match quadrant {
        Quadrant::Between => (r1.0, r2.0, odd, theta),
        // Crossing lines draw both vertical sectors: the one opposite is θ too (§4).
        Quadrant::Opposite if crossing => (r1.0, r2.0, odd, theta),
        Quadrant::Opposite => (r2.0, r1.0, odd, 360.0 - theta),
        Quadrant::After | Quadrant::Before => (r2.0, r1.0, !odd, 180.0 - theta),
    };
    let value = Angle::rounded(value).ok_or(Some(Message::TooFar))?;
    Ok(Offered {
        measure: Measure::Angle(quadrant),
        form: Form::Angle {
            a: a.clone(),
            b: b.clone(),
            reversed,
            value,
            driving: true,
        },
    })
}

/// `mm`, rounded to the nanometre, as the core rounds a measured value (SD2).
fn rounded(mm: f64) -> Result<Length, NotOffered> {
    Length::rounded(mm).ok_or(Some(Message::TooFar))
}

/// `measured` with the value the Modify box's `text` gives it, read as the document reads a
/// value (SD9): one JSON number, at most the quantity's decimals, in range of what it holds.
/// A refused number is repeated to its first [`SHOWN`] characters.
fn typed(measured: &Form, text: &str) -> Result<Form, Message> {
    let node = json::read(text.as_bytes()).map_err(|_| Message::NotANumber)?;
    let Value::Number(number) = node.value else {
        return Err(Message::NotANumber);
    };
    let shown = if number.chars().count() > SHOWN {
        format!("{}…", number.chars().take(SHOWN).collect::<String>())
    } else {
        number.clone()
    };
    let refused = |e: crate::document::NumberError, unit: &str| {
        Message::ValueRefused(
            Error::new(ErrorCode::InvalidInput, format!("{shown} {e}")).with_suggestion(format!(
                "type a value in {unit}, as the document writes one"
            )),
        )
    };
    let mut form = measured.clone();
    match &mut form {
        Form::Angle { value, .. } => {
            *value = Angle::parse(&number).map_err(|e| refused(e, "degrees, to 9 decimals"))?;
        }
        Form::Length { value, .. }
        | Form::Distance { value, .. }
        | Form::DistanceX { value, .. }
        | Form::DistanceY { value, .. }
        | Form::PointLineDistance { value, .. }
        | Form::Radius { value, .. }
        | Form::Diameter { value, .. } => {
            *value =
                Length::parse(&number).map_err(|e| refused(e, "millimetres, to 6 decimals"))?;
        }
        _ => return Err(Message::NotANumber),
    }
    Ok(form)
}

/// SolidWorks' choices for a dimension that over-defines (SD7; docs/ux/solidworks.md, §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferChoice {
    /// Make it driven: `set_field` of its `driving` to false.
    Driven,
    /// Leave it driving: nothing is sent.
    Leave,
    /// Cancel: `undo` of its add.
    Cancel,
}

impl OfferChoice {
    /// The three, in SolidWorks' order.
    pub const ALL: [OfferChoice; 3] =
        [OfferChoice::Driven, OfferChoice::Leave, OfferChoice::Cancel];
}

/// What a choice sends, each made against the revision the add made (decision 12), so a
/// session that has moved on since refuses it as a `conflict` rather than change something
/// else.
#[derive(Debug, Clone, PartialEq)]
pub enum OfferRequest {
    /// `set_field`: the sketch's `constraints.ID.driving` set to `false`.
    SetField {
        revision: Revision,
        feature: FeatureId,
        field: String,
        value: Node,
    },
    /// `undo` of one step: the add.
    Undo {
        revision: Revision,
        to: Option<Revision>,
    },
}

/// The offer SD7 makes for a dimension placed that over-defines the sketch: the dimension,
/// its state in the read, the sketch's state, and what each choice sends.
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub constraint: ElementId,
    pub state: ConstraintState,
    pub sketch_state: SketchState,
    placement: Placement,
}

impl Offer {
    /// What `choice` sends: none for leaving it.
    pub fn request(&self, choice: OfferChoice) -> Option<OfferRequest> {
        let revision = self.placement.revision;
        match choice {
            OfferChoice::Driven => Some(OfferRequest::SetField {
                revision,
                feature: self.placement.sketch.clone(),
                field: format!("constraints.{}.driving", self.placement.constraint),
                value: Node {
                    at: At::UNPLACED,
                    value: Value::Bool(false),
                },
            }),
            OfferChoice::Leave => None,
            OfferChoice::Cancel => Some(OfferRequest::Undo { revision, to: None }),
        }
    }
}

/// Whether a sketch in `state` is one the over-defining prompt is for: over defined (a
/// conflict, or a driving dimension repeating others) or with no solution, as SolidWorks'
/// prompt repairs "the over defined or unsolvable state".
fn prompted(state: SketchState) -> bool {
    matches!(state, SketchState::OverDefined | SketchState::NoSolution)
}

/// The constraints `states` flag: conflicting, redundant, or partially redundant.
fn flagged(states: &SketchStates) -> BTreeSet<ElementId> {
    states
        .constraints
        .iter()
        .filter(|(_, s)| s.flagged())
        .map(|(id, _)| id.clone())
        .collect()
}

/// The offer for `placement`, given `read`, the `sketch` read of the model at `revision`:
/// only for a read of the model the add made (an older one doesn't hold it, and a newer one
/// is past the moment), of its sketch, where the dimension is there and still driving, and
/// the add (SD7):
/// - flagged a constraint the read it was placed on didn't (any, not only itself: which of
///   a group PlaneGCS flags follows the IDs' order);
/// - or, the sketch solved, left its degrees of freedom where they were, or higher: a
///   driving dimension adds an equation, so one that doesn't lower the rank is dependent,
///   whatever PlaneGCS's greedy contest flagged (a constraint flagged already can cover the
///   new group: four of one horizontal distance flag no more than three);
/// - or took the sketch to over defined or no solution from a state that was neither (a
///   refused solve, whose degrees of freedom say nothing of the add).
///
/// So a sketch already over defined, left so, offers each dimension that adds to it, as
/// SolidWorks prompts for each.
///
/// Those compare the read the dimension was placed on with the add's, so they hold only
/// where the add is the one change between them: its revision the next after that read's.
/// Where another came between (another client's edit, an agent's, landing first), its
/// flags, freedoms, and state would be taken for the dimension's (an added line raising the
/// freedoms, an added constraint lowering them), so the dimension is judged by its own state
/// alone: offered if it's flagged (conflicting, redundant, or partially redundant). The
/// tool places nothing while its last offer is unsettled, so its own adds never come
/// between.
///
/// One case is taken for the dimension's doing that isn't: a value typed that makes the
/// geometry degenerate (circles tangent through a point, for one) can newly flag a relation
/// that doesn't depend on the dimension, and the offer appears, though Driven doesn't repair
/// the sketch.
pub fn offer(read: &SketchRead, revision: Revision, placement: &Placement) -> Option<Offer> {
    offer_of(
        &read.sketch,
        &read.members,
        &read.states,
        revision,
        placement,
    )
}

/// [`offer`], from a read's parts: the sketch's ID, its members, and its states.
fn offer_of(
    sketch: &FeatureId,
    members: &crate::document::Sketch,
    states: &SketchStates,
    revision: Revision,
    placement: &Placement,
) -> Option<Offer> {
    if revision != placement.revision || *sketch != placement.sketch {
        return None;
    }
    let driving = members
        .constraints
        .iter()
        .find(|SketchConstraint { id, .. }| *id == placement.constraint)
        .and_then(|c| c.form.driving())?;
    let state = *states.constraints.get(&placement.constraint)?;
    let sketch_state = states.state;
    let over_defines = if placement.read.next() == Some(placement.revision) {
        let newly_flagged = flagged(states)
            .iter()
            .any(|id| !placement.flagged.contains(id));
        let dependent = states.solve_problem.is_none() && states.dofs >= placement.dofs;
        let newly_prompted = prompted(sketch_state) && !prompted(placement.before);
        newly_flagged || dependent || newly_prompted
    } else {
        state.flagged()
    };
    (driving && over_defines).then(|| Offer {
        constraint: placement.constraint.clone(),
        state,
        sketch_state,
        placement: placement.clone(),
    })
}
