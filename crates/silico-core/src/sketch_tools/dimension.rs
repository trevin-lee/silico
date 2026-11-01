//! Smart Dimension, headless (docs/design/m5.0-app.md, section 9, *Smart Dimension*; the
//! decisions SD1 to SD9; docs/study/smart-dimension.md, §11 and *For M5.0 and the
//! maintainer*): a state machine from the pointer's and the Modify box's events to the
//! `sketch_add` of one driving dimension, with the preview the viewport draws, and the offer
//! the app makes when the dimension placed over-defines the sketch (SD7). Behaviour is
//! SolidWorks' as the decisions take it:
//!
//! - **Picks.** A click on a point (the origin among them, a circle's or an arc's centre too)
//!   or a curve (a line or an arc where it's drawn, a circle all round, an axis anywhere)
//!   picks it, up to two; a click on nothing, or once two are picked, places the dimension
//!   the preview shows there, and opens the Modify box. Esc undoes the last pick, and with
//!   none leaves the tool ([`Outcome::left`]). A right-click locks the kind the preview
//!   shows: the pointer then moves the dimension, not its kind, until a pick changes.
//! - **What is placed** (SD1), measured on the newest read's coordinates:
//!   - one line: `length`; one circle: `diameter`; one arc: `radius` (SD8: no toggle);
//!   - two points: `distance_x` with the pointer above or below the box the two span,
//!     `distance_y` with it left or right of the box, `distance` elsewhere (inside it, or
//!     off a corner); `distance_x` and `distance_y` ordered so the value starts positive,
//!     `a` the point to the left (below), `distance` in the order picked;
//!   - a point and a line: `point_line_distance`, its `side` the side of the line the point
//!     is on as drawn (facing from `from` to `to`; an axis faces +x or +y);
//!   - a circle or an arc with anything else: as its centre (to a centre by default);
//!   - two lines that aren't parallel: `angle`, `{first, second}` with the pointer in the
//!     sweep counter-clockwise from the first's direction to the second's about where they
//!     meet, `{second, first}` (the reflex) elsewhere; only those two (SD5);
//!   - two parallel lines ([`PARALLEL`], FreeCAD's test): `point_line_distance` from the
//!     second line's `from` to the first line (SD4), the first line's `from` to an axis
//!     picked second, which has none;
//!   - a point and a centreline (a construction line, or an axis): `point_line_distance`
//!     with the pointer on the point's side; across it, a diameter (a doubled distance),
//!     which isn't offered (SD3), nor are an arc's length and the Min and Max gaps.
//! - **The Modify box** opens on placing, holding the measured value rounded to the
//!   nanometre (or nanodegree), as the document writes it. It takes plain numbers only, read
//!   as the document reads a value (SD9): a JSON number, at most the quantity's decimals, no
//!   units, arithmetic, or expressions. Enter places the dimension, `driving: true`, with the
//!   value typed (SD2); one the reader or the gate refuses is said, and the box stays open.
//!   Esc skips the box: the dimension is placed with the measured value, rounded by the core
//!   (SD2); if the gate refuses that, nothing is placed, and the box closes saying why.
//! - **Over-defining** (SD7). The add is made whatever the solve will find. Once the app has
//!   the `sketch` read of the revision the add made, [`offer`] gives SolidWorks' three
//!   choices if the dimension placed reads conflicting or redundant: driven (`set_field` of
//!   its `driving` to false), leave it, or Cancel (`undo`).
//!
//! Adds are tracked as the sketch tools' are ([`super::SketchTool`]): each with a token the
//! viewport answers with the revision it made, or its refusal.

use super::adds::Adds;
use super::build::Builder;
use super::geometry::{cross, distance, dot, length, sub, unit};
use super::infer::{Pointer, View};
use super::{AddToken, Context, Message, NewIds, Outcome, Xy, geometry};
use crate::command::Revision;
use crate::command::sketch::SketchRead;
use crate::document::json::{self, At, Node, Value};
use crate::document::sketch_edit::{self, Geo};
use crate::document::{
    Angle, ElementId, FeatureId, Form, Length, LineSide, Reference, SketchConstraint,
};
use crate::sketch_states::{ConstraintState, SketchState};
use crate::{Error, ErrorCode};

/// How nearly two lines' directions as drawn (each its `to` less its `from`, an axis's a
/// unit vector) must cross to nothing for the lines to be parallel, in square millimetres:
/// FreeCAD 1.1.3's, `Precision::Intersection()` on the same product in its dimension tool
/// (`calculateAngle`, CommandConstraints.cpp, at the clone's `876271d660`), which then
/// writes the distance SD4 takes. Lines a relation holds parallel are solved so.
pub const PARALLEL: f64 = 1e-9;

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
/// constraint's ID, and the revision the add made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub sketch: FeatureId,
    pub constraint: ElementId,
    pub revision: Revision,
}

/// Smart Dimension, between events. Not `Clone`, as a sketch tool isn't.
#[derive(Debug)]
pub struct SmartDimension {
    adds: Adds,
    /// The sketch it works in: a context of another starts it afresh.
    sketch: Option<FeatureId>,
    picks: Vec<Reference>,
    lock: Option<Measure>,
    pressed: bool,
    pointer: Option<Xy>,
    /// The Modify box, open: the dimension measured, and the box's text.
    modify: Option<(Form, String)>,
    /// The last dimension placed: its token, its constraint, and the revision its add made
    /// once answered.
    placed: Option<(AddToken, ElementId, Option<Revision>)>,
    preview: DimensionPreview,
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
    /// `forward`: from the first pick's direction to the second's.
    Angle {
        forward: bool,
    },
}

/// A picked thing, as the read has it.
#[derive(Debug, Clone)]
enum Item {
    /// A point, or the origin.
    Point(Reference, Xy),
    /// A line, or an axis: where it starts, its direction (to less from; an axis's unit),
    /// its `from` point (none for an axis), and whether it's a centreline (construction, or
    /// an axis).
    Line {
        reference: Reference,
        at: Xy,
        direction: Xy,
        from: Option<ElementId>,
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
                    direction,
                    from: None,
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
                    direction: sub(to, from),
                    from: Some(line.from.clone()),
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
            sketch: None,
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

    /// Whether something is under way: a pick held, a press down, or the Modify box open.
    pub fn busy(&self) -> bool {
        !self.picks.is_empty() || self.pressed || self.modify.is_some()
    }

    /// The core took the add `token` names, at `revision`.
    pub fn added(&mut self, token: AddToken, revision: Revision) {
        self.adds.added(token, revision);
        if let Some((placed, _, made)) = &mut self.placed
            && *placed == token
        {
            *made = Some(revision);
        }
    }

    /// The core refused the add `token` names: it's forgotten.
    pub fn refused(&mut self, token: AddToken) {
        self.adds.refused(token);
        if self.placed.as_ref().is_some_and(|(t, _, _)| *t == token) {
            self.placed = None;
        }
    }

    /// The last dimension placed, once the core has answered its add: for [`offer`].
    pub fn placement(&self) -> Option<Placement> {
        let sketch = self.sketch.clone()?;
        match &self.placed {
            Some((_, constraint, Some(revision))) => Some(Placement {
                sketch,
                constraint: constraint.clone(),
                revision: *revision,
            }),
            _ => None,
        }
    }

    /// The tool afresh: nothing picked, no box, no lock.
    fn restart(&mut self) {
        self.picks.clear();
        self.lock = None;
        self.pressed = false;
        self.modify = None;
    }

    /// `event` against `context`: what it added, said, or ended; the preview updated.
    pub fn handle(
        &mut self,
        context: &Context<'_>,
        ids: &mut dyn NewIds,
        event: DimensionEvent,
    ) -> Outcome {
        let _span = tracing::debug_span!("smart_dimension").entered();
        if let DimensionEvent::Press(at)
        | DimensionEvent::Move(at)
        | DimensionEvent::Release(at)
        | DimensionEvent::RightClick(at) = event
            && !geometry::finite(at)
        {
            return Outcome::said(Message::NotFinite);
        }
        if self.sketch.as_ref() != Some(context.sketch_id) {
            if self.sketch.is_some() {
                self.adds.clear();
                self.placed = None;
                self.restart();
            }
            self.sketch = Some(context.sketch_id.clone());
        }
        self.adds.settle(context);
        let pending = self.adds.members();
        let view = View::of(context.sketch, &pending);
        self.revalidate(&view);
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
        if let DimensionEvent::Press(at)
        | DimensionEvent::Move(at)
        | DimensionEvent::Release(at)
        | DimensionEvent::RightClick(at) = event
        {
            self.pointer = Some(at);
        }
        let outcome = match event {
            DimensionEvent::Typed(text) => {
                if let Some((_, box_text)) = &mut self.modify {
                    *box_text = text;
                }
                Outcome::nothing()
            }
            DimensionEvent::Enter => match self.modify.take() {
                Some((measured, text)) => match typed(&measured, &text) {
                    Ok(form) => match self.place(&view, ids, form) {
                        Ok(outcome) => outcome,
                        Err(message) => {
                            self.modify = Some((measured, text));
                            Outcome::said(message)
                        }
                    },
                    Err(message) => {
                        self.modify = Some((measured, text));
                        Outcome::said(message)
                    }
                },
                None => Outcome::nothing(),
            },
            DimensionEvent::Escape => match self.modify.take() {
                Some((measured, _)) => self
                    .place(&view, ids, measured)
                    .unwrap_or_else(Outcome::said),
                None if self.picks.pop().is_some() => {
                    self.lock = None;
                    self.pressed = false;
                    Outcome::nothing()
                }
                None => {
                    self.restart();
                    Outcome::left()
                }
            },
            _ if self.modify.is_some() => Outcome::nothing(),
            DimensionEvent::Press(_) => {
                self.pressed = true;
                Outcome::nothing()
            }
            DimensionEvent::Move(_) => Outcome::nothing(),
            DimensionEvent::Release(at) if self.pressed => {
                self.pressed = false;
                self.click(&view, &pointer, at)
            }
            DimensionEvent::Release(_) => Outcome::nothing(),
            DimensionEvent::RightClick(at) => {
                if let Ok(offered) = offered(&view, &self.picks, at, None) {
                    self.lock = Some(offered.measure);
                }
                Outcome::nothing()
            }
        };
        self.preview = self.drawn(&view, &pointer);
        outcome
    }

    /// The picks and the box against `view`, a newer read: a pick it no longer has dropped
    /// (and the lock with it), and the box closed if what it measures is gone.
    fn revalidate(&mut self, view: &View<'_>) {
        let before = self.picks.len();
        self.picks.retain(|r| Item::of(view, r).is_some());
        if self.picks.len() != before {
            self.lock = None;
        }
        if let Some((form, _)) = &self.modify
            && !form
                .references()
                .iter()
                .all(|(_, r, _)| Item::of(view, r).is_some())
        {
            self.modify = None;
        }
    }

    /// A click at `at`: a pick, while fewer than two are picked and it's on something not
    /// picked; else the dimension placed, its Modify box opened.
    fn click(&mut self, view: &View<'_>, pointer: &Pointer<'_, '_>, at: Xy) -> Outcome {
        if self.picks.len() < 2
            && let Some(hit) = pointer.pick(at)
            && !self.picks.contains(&hit)
        {
            self.picks.push(hit);
            self.lock = None;
            return Outcome::nothing();
        }
        match offered(view, &self.picks, at, self.lock) {
            Ok(offered) => {
                let text = offered.form.value_text().unwrap_or_default();
                self.modify = Some((offered.form, text));
                Outcome::nothing()
            }
            Err(Some(message)) => Outcome::said(message),
            Err(None) => Outcome::nothing(),
        }
    }

    /// `form` added as a new constraint, unless the gate, given the sketch with it, refuses
    /// the constraint itself (its value out of range, its `side`, what it names): the add,
    /// its token, and the tool afresh for the next dimension; else the gate's words. A
    /// problem the gate finds elsewhere (a limit, another member) is the core's to answer,
    /// so the add is made and its refusal comes back ([`refused`](Self::refused)).
    fn place(
        &mut self,
        view: &View<'_>,
        ids: &mut dyn NewIds,
        form: Form,
    ) -> Result<Outcome, Message> {
        let mut builder = Builder::new(ids, view);
        builder.constraint(form).map_err(Message::NoId)?;
        let members = builder.members;
        let Some(constraint) = members.constraints.first().map(|c| c.id.clone()) else {
            return Err(Message::NoId(
                Error::new(
                    ErrorCode::Internal,
                    "a dimension was built with no constraint",
                )
                .with_suggestion(crate::worker::SILICO_BUG),
            ));
        };
        if let Err(refused) = sketch_edit::add(&view.sketch, &members) {
            let own = format!("constraints.{constraint}");
            let about = |e: &Error| {
                e.parameter()
                    .is_some_and(|p| p == own || p.starts_with(&format!("{own}.")))
            };
            if let Some(error) = refused.errors.into_iter().find(about) {
                return Err(Message::ValueRefused(error));
            }
        }
        let token = self.adds.issue(&members);
        self.placed = Some((token, constraint, None));
        self.restart();
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
        if let Some((form, text)) = &self.modify {
            preview.dimension = Some(form.clone());
            preview.modify = Some(text.clone());
            return preview;
        }
        let Some(at) = self.pointer else {
            return preview;
        };
        if self.picks.len() < 2 {
            preview.hover = pointer.pick(at).filter(|r| !self.picks.contains(r));
        }
        match offered(view, &self.picks, at, self.lock) {
            Ok(offered) => preview.dimension = Some(offered.form),
            Err(message) => preview.unavailable = message,
        }
        preview
    }
}

/// The dimension `picks` offer with the pointer at `at`, `lock` the kind locked, if any and
/// if it's one of theirs; or why none.
fn offered(
    view: &View<'_>,
    picks: &[Reference],
    at: Xy,
    lock: Option<Measure>,
) -> Result<Offered, NotOffered> {
    let items: Vec<Item> = picks.iter().filter_map(|r| Item::of(view, r)).collect();
    match items.as_slice() {
        [one] => single(one),
        [first, second] => {
            let (first, second) = (first.clone().centred(), second.clone().centred());
            if picks.iter().all(|r| r.element().is_none()) {
                return Err(Some(Message::FixedOnly));
            }
            let side_at = (lock != Some(Measure::PointLine)).then_some(at);
            match (&first, &second) {
                (Item::Point(..), Item::Point(..)) => points(&first, &second, at, lock),
                // A lock holds the distance across a centreline too: the pointer then
                // moves only the dimension.
                (Item::Point(..), Item::Line { .. }) => point_line(&first, &second, side_at),
                (Item::Line { .. }, Item::Point(..)) => point_line(&second, &first, side_at),
                (Item::Line { .. }, Item::Line { .. }) => lines(&first, &second, at, lock),
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

/// Two points: `distance_x` with the pointer above or below the box they span, `distance_y`
/// with it left or right of it, `distance` elsewhere; or the locked one. `distance_x` and
/// `distance_y` with `a` the point to the left (below), so the value starts positive, the
/// order picked where they're level with each other; `distance` in the order picked, refused
/// where the two are at one place (no direction to hold them apart in).
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

/// A point and a line: `point_line_distance`, the side the point is on as drawn; refused
/// for a point on the line (at the nanometre: no side to hold it on). With the pointer `at`
/// across a centreline from the point, a diameter, which isn't offered (SD3); `at` none for
/// two parallel lines (SD4), which a centreline's side doesn't govern.
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
        && cross(along, sub(at, *start)) * off < 0.0
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

/// Two lines: parallel ([`PARALLEL`], FreeCAD's test), the second's `from` to the first
/// (SD4), or the first's `from` to an axis picked second;
/// otherwise the angle, `{first, second}` with the pointer in the sweep counter-clockwise
/// from the first's direction to the second's, about where they meet, `{second, first}` (the
/// reflex) elsewhere, or as locked (SD5).
fn lines(
    first: &Item,
    second: &Item,
    at: Xy,
    lock: Option<Measure>,
) -> Result<Offered, NotOffered> {
    let (
        Item::Line {
            reference: ra,
            at: pa,
            direction: da,
            from: fa,
            ..
        },
        Item::Line {
            reference: rb,
            at: pb,
            direction: db,
            from: fb,
            ..
        },
    ) = (first, second)
    else {
        return Err(None);
    };
    let across = cross(*da, *db);
    if across.abs() < PARALLEL {
        // SD4: a point of one, the other's line; an axis has no `from`.
        let (from, at_from, line) = match (fb, fa) {
            (Some(from), _) => (from, *pb, first),
            (None, Some(from)) => (from, *pa, second),
            (None, None) => return Err(None),
        };
        let point = Item::Point(Reference::Element(from.clone()), at_from);
        return point_line(&point, line, None);
    }
    let degrees = |v: Xy| {
        let d = libm::atan2(cross(*da, v), dot(*da, v)).to_degrees();
        if d < 0.0 { d + 360.0 } else { d }
    };
    let sweep = degrees(*db);
    let forward = match lock {
        Some(Measure::Angle { forward }) => forward,
        _ => {
            // Where the lines meet: pa + t·da on b's line.
            let t = cross(sub(*pb, *pa), *db) / across;
            let meet = [pa[0] + t * da[0], pa[1] + t * da[1]];
            let toward = sub(at, meet);
            length(toward) == 0.0 || degrees(toward) <= sweep
        }
    };
    let (a, b, value) = if forward {
        (ra, rb, sweep)
    } else {
        (rb, ra, 360.0 - sweep)
    };
    let value = Angle::rounded(value).ok_or(Some(Message::TooFar))?;
    Ok(Offered {
        measure: Measure::Angle { forward },
        form: Form::Angle {
            a: a.clone(),
            b: b.clone(),
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
fn typed(measured: &Form, text: &str) -> Result<Form, Message> {
    let node = json::read(text.as_bytes()).map_err(|_| Message::NotANumber)?;
    let Value::Number(number) = node.value else {
        return Err(Message::NotANumber);
    };
    let refused = |e: crate::document::NumberError, unit: &str| {
        Message::ValueRefused(
            Error::new(ErrorCode::InvalidInput, format!("{number} {e}")).with_suggestion(format!(
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
/// how the read flags it, the sketch's state, and what each choice sends.
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

/// The offer for `placement`, given `read`, the `sketch` read of the model at `revision`:
/// only for a read of the model the add made (an older one doesn't hold it, and a newer one
/// is past the moment), of its sketch, where the dimension is there, still driving, and
/// flagged, conflicting or redundant (SD7). Which constraint of a group the solve flags is
/// PlaneGCS's: where it flags another, the read shows that one, and no offer is made.
pub fn offer(read: &SketchRead, revision: Revision, placement: &Placement) -> Option<Offer> {
    if revision != placement.revision || read.sketch != placement.sketch {
        return None;
    }
    let driving = read
        .members
        .constraints
        .iter()
        .find(|SketchConstraint { id, .. }| *id == placement.constraint)
        .and_then(|c| c.form.driving())?;
    let state = *read.states.constraints.get(&placement.constraint)?;
    (driving && state.flagged()).then(|| Offer {
        constraint: placement.constraint.clone(),
        state,
        sketch_state: read.states.state,
        placement: placement.clone(),
    })
}
