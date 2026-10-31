//! The arcs (docs/design/m5.0-app.md, section 9, *Circle, arcs, and points*;
//! docs/ux/solidworks.md, §4). Silico's arc runs counter-clockwise from its start to its
//! end (M3.2's D2), so an arc drawn clockwise is written from its far end back.
//!
//! - **Centrepoint arc:** its centre, then its start (by a drag from the centre, or a
//!   click), then its end, a click: the end on the circle the start sets, in the direction
//!   the pointer swept from the start.
//! - **3-point arc:** its start, then its end (a drag or a click), then a click it passes
//!   through, which sets its radius.
//! - **Tangent arc:** pressed on the end of a line or an arc, dragged, and released (or a
//!   click, then a click): tangent there, a smooth join (`tangent_at`, `same`), its own
//!   relation, kept with Ctrl; tangent arcs only, with no normal zones (#278's B).
//!
//! Each arc's start and end take what a line's end takes, its alignments with other points
//! guides only, but a centrepoint arc's end, which its circle places on it; a centrepoint
//! arc's centre takes what a circle's does; a 3-point arc's centre is where its points
//! place it.

use super::build::Builder;
use super::geometry::{add, angle_of, circumcentre, distance, scale, sub, unit};
use super::infer::{Align, Placed, Target, counter_clockwise, one_place, tangent_arc};
use super::{
    Event, Message, Outcome, Preview, PreviewArc, Relation, Shown, Step, Xy, failed, within_reach,
};
use crate::document::{Direction, ElementId, Form, Reference};

/// An arc's points, its start and end counter-clockwise, and whether that's the gesture's
/// end to its start (an arc drawn clockwise).
struct Drawn {
    center: Xy,
    start: Xy,
    end: Xy,
    reversed: bool,
}

impl Drawn {
    /// The arc about `center` from the gesture's `from` to its `to`, counter-clockwise if
    /// `ccw`, otherwise written from `to` back to `from`.
    fn new(center: Xy, from: Xy, to: Xy, ccw: bool) -> Self {
        if ccw {
            Drawn {
                center,
                start: from,
                end: to,
                reversed: false,
            }
        } else {
            Drawn {
                center,
                start: to,
                end: from,
                reversed: true,
            }
        }
    }
}

fn draw_arc(preview: &mut Preview, drawn: &Drawn) {
    preview.arcs.push(PreviewArc {
        center: drawn.center,
        start: drawn.start,
        end: drawn.end,
    });
}

/// Where a point the pointer places: an arc's end, lined up as a guide (as a line's end
/// is), with the gesture's own points `besides`.
fn placed_at(step: &Step<'_, '_, '_>, at: Xy, besides: &[&ElementId]) -> Placed {
    let mut placed = step.pointer.snap(at);
    step.pointer.align(&mut placed, Align::Guides, besides);
    placed
}

/// Where a centre the pointer places, lined up as relations: a lone point, which shows no
/// direction of its own (#278's E).
fn centre_at(step: &Step<'_, '_, '_>, at: Xy) -> Placed {
    let mut placed = step.pointer.snap(at);
    step.pointer.align(&mut placed, Align::Relations, &[]);
    placed
}

/// Whether `end` is no point apart from `start`: the same point of the sketch, or within the
/// pick tolerance (a double-click's second click).
fn on_top(step: &Step<'_, '_, '_>, start: &Placed, end: &Placed) -> bool {
    (start.shared().is_some() && start.shared() == end.shared())
        || (end.shared().is_none() && distance(start.at, end.at) <= step.pointer.tolerance)
}

/// Each new point's place within reach.
fn reach(points: &[&Placed]) -> bool {
    points
        .iter()
        .all(|p| p.shared().is_some() || within_reach(p.at))
}

// --- Centrepoint ------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub(super) struct Centerpoint {
    state: CenterState,
}

#[derive(Debug, Clone, Default)]
enum CenterState {
    #[default]
    Idle,
    /// The centre pressed: a drag sets the start.
    Pressed {
        centre: Placed,
        press: Xy,
        dragged: bool,
    },
    /// The centre clicked: the next click sets the start.
    Centre { centre: Placed, pressed: bool },
    /// The start set: the pointer sweeps to the end, `swept` radians from the start,
    /// counter-clockwise positive, its last angle about the centre `last`.
    Sweep {
        centre: Placed,
        start: Placed,
        swept: f64,
        last: f64,
        pressed: bool,
    },
}

/// The sweep from `last` to the pointer's angle, the short way round.
fn turn(last: f64, now: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    (now - last + PI).rem_euclid(TAU) - PI
}

impl Centerpoint {
    pub(super) fn busy(&self) -> bool {
        !matches!(self.state, CenterState::Idle)
    }

    fn sweep_from(centre: Placed, start: Placed) -> CenterState {
        let last = angle_of(sub(start.at, centre.at));
        CenterState::Sweep {
            centre,
            start,
            swept: 0.0,
            last,
            pressed: false,
        }
    }

    pub(super) fn handle(
        &mut self,
        step: &mut Step<'_, '_, '_>,
        event: Event,
        preview: &mut Preview,
    ) -> Outcome {
        // The sweep follows every place the pointer is seen.
        if let (
            CenterState::Sweep {
                centre,
                swept,
                last,
                ..
            },
            Event::Press(at) | Event::Move(at) | Event::Release(at),
        ) = (&mut self.state, event)
            && distance(at, centre.at) > 0.0
        {
            let now = angle_of(sub(at, centre.at));
            *swept += turn(*last, now);
            *last = now;
        }
        let outcome = match (&mut self.state, event) {
            (_, Event::Escape) => {
                self.state = CenterState::Idle;
                Outcome::left()
            }
            (CenterState::Idle, Event::Press(at)) => {
                let centre = centre_at(step, at);
                if !reach(&[&centre]) {
                    Outcome::said(Message::TooFar)
                } else {
                    self.state = CenterState::Pressed {
                        centre,
                        press: at,
                        dragged: false,
                    };
                    Outcome::nothing()
                }
            }
            (CenterState::Pressed { press, dragged, .. }, Event::Move(at)) => {
                if distance(at, *press) > step.pointer.tolerance {
                    *dragged = true;
                }
                Outcome::nothing()
            }
            (
                CenterState::Pressed {
                    centre,
                    press,
                    dragged,
                },
                Event::Release(at),
            ) => {
                let centre = centre.clone();
                if *dragged || distance(at, *press) > step.pointer.tolerance {
                    self.start(step, centre, at)
                } else {
                    self.state = CenterState::Centre {
                        centre,
                        pressed: false,
                    };
                    Outcome::nothing()
                }
            }
            (
                CenterState::Centre { pressed, .. } | CenterState::Sweep { pressed, .. },
                Event::Press(_),
            ) => {
                *pressed = true;
                Outcome::nothing()
            }
            (CenterState::Centre { centre, pressed }, Event::Release(at)) => {
                if !*pressed {
                    Outcome::nothing()
                } else {
                    *pressed = false;
                    let centre = centre.clone();
                    self.start(step, centre, at)
                }
            }
            (
                CenterState::Sweep {
                    centre,
                    start,
                    swept,
                    pressed,
                    ..
                },
                Event::Release(at),
            ) => {
                if !*pressed {
                    Outcome::nothing()
                } else {
                    *pressed = false;
                    let (centre, start, swept) = (centre.clone(), start.clone(), *swept);
                    let outcome = finish_centerpoint(step, &centre, &start, swept, at);
                    if outcome.add.is_some() {
                        self.state = CenterState::Idle;
                    }
                    outcome
                }
            }
            _ => Outcome::nothing(),
        };
        self.draw(step, event, preview);
        outcome
    }

    /// The start set where the pointer places it, `at`: unless it's on the centre.
    fn start(&mut self, step: &Step<'_, '_, '_>, centre: Placed, at: Xy) -> Outcome {
        let besides: Vec<&ElementId> = centre.shared().into_iter().collect();
        let start = placed_at(step, at, &besides);
        if on_top(step, &centre, &start) {
            self.state = CenterState::Centre {
                centre,
                pressed: false,
            };
            return Outcome::nothing();
        }
        if !reach(&[&start]) {
            self.state = CenterState::Centre {
                centre,
                pressed: false,
            };
            return Outcome::said(Message::TooFar);
        }
        self.state = Self::sweep_from(centre, start);
        Outcome::nothing()
    }

    fn draw(&self, step: &Step<'_, '_, '_>, event: Event, preview: &mut Preview) {
        let at = match event {
            Event::Press(at) | Event::Move(at) | Event::Release(at) | Event::DoubleClick(at) => at,
            Event::EndChain | Event::Escape | Event::Enter => return,
        };
        match &self.state {
            CenterState::Idle => {
                let placed = centre_at(step, at);
                show(preview, &placed);
            }
            CenterState::Pressed { centre, .. } | CenterState::Centre { centre, .. } => {
                let besides: Vec<&ElementId> = centre.shared().into_iter().collect();
                let start = placed_at(step, at, &besides);
                preview.points.push(centre.at);
                preview.lines.push([centre.at, start.at]);
                preview.shown.extend(centre.shown());
                show(preview, &start);
            }
            CenterState::Sweep {
                centre,
                start,
                swept,
                ..
            } => {
                preview.points.push(centre.at);
                preview.shown.extend(centre.shown());
                preview.shown.extend(start.shown());
                if let Some(end) = end_on_circle(step, centre, start, at) {
                    let drawn = oriented(centre.at, start.at, end.at, *swept);
                    draw_arc(preview, &drawn);
                    show(preview, &end);
                }
            }
        }
    }
}

fn show(preview: &mut Preview, placed: &Placed) {
    preview.pointer = Some(placed.at);
    preview.shown.extend(placed.shown());
    preview.guides.extend(placed.guides.iter().copied());
}

/// The end of a centrepoint arc with the pointer at `at`: on the circle the start sets,
/// toward the pointer, where the pointer places it there (not lined up: the circle places
/// it). None with the pointer on the centre.
fn end_on_circle(
    step: &Step<'_, '_, '_>,
    centre: &Placed,
    start: &Placed,
    at: Xy,
) -> Option<Placed> {
    let radius = distance(start.at, centre.at);
    let toward = unit(sub(at, centre.at))?;
    let on_circle = add(centre.at, scale(toward, radius));
    Some(step.pointer.snap(on_circle))
}

/// The arc about `center` from `start` to `end`, swept `swept` radians from the start
/// (counter-clockwise positive), as Silico writes it: counter-clockwise.
fn oriented(center: Xy, start: Xy, end: Xy, swept: f64) -> Drawn {
    Drawn::new(center, start, end, swept >= 0.0)
}

fn finish_centerpoint(
    step: &mut Step<'_, '_, '_>,
    centre: &Placed,
    start: &Placed,
    swept: f64,
    at: Xy,
) -> Outcome {
    let radius = distance(start.at, centre.at);
    if radius <= silico_protocol::LENGTH_TOLERANCE {
        return Outcome::said(Message::TooSmall);
    }
    if radius > silico_protocol::MAX_LENGTH {
        return Outcome::said(Message::TooFar);
    }
    let Some(end) = end_on_circle(step, centre, start, at) else {
        return Outcome::nothing();
    };
    if on_top(step, start, &end) || swept == 0.0 {
        return Outcome::nothing();
    }
    if one_place(start.at, end.at) {
        return Outcome::said(Message::TooSmall);
    }
    if !reach(&[&end]) {
        return Outcome::said(Message::TooFar);
    }
    let mut builder = Builder::new(&mut *step.ids, step.pointer.view);
    let built = (|| -> Result<(), crate::Error> {
        let c = builder.point(centre, false)?;
        let s = builder.point(start, false)?;
        let e = builder.point(&end, false)?;
        if swept < 0.0 {
            builder.arc(&c, &e, &s)?;
        } else {
            builder.arc(&c, &s, &e)?;
        }
        Ok(())
    })();
    match built {
        Ok(()) => Outcome::added(builder.members),
        Err(error) => failed(error),
    }
}

// --- 3-point ----------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub(super) struct ThreePoint {
    state: ThreeState,
}

#[derive(Debug, Clone, Default)]
enum ThreeState {
    #[default]
    Idle,
    Pressed {
        start: Placed,
        press: Xy,
        dragged: bool,
    },
    /// The start clicked: the next click is the end.
    Start { start: Placed, pressed: bool },
    /// Both ends set: the next click is a point the arc passes through.
    Ends {
        start: Placed,
        end: Placed,
        pressed: bool,
    },
}

/// The arc from `start` to `end` through `through`: none for three points in a line.
fn through_three(start: Xy, end: Xy, through: Xy) -> Option<Drawn> {
    let center = circumcentre(start, end, through)?;
    Some(Drawn::new(
        center,
        start,
        end,
        counter_clockwise(start, through, end),
    ))
}

impl ThreePoint {
    pub(super) fn busy(&self) -> bool {
        !matches!(self.state, ThreeState::Idle)
    }

    pub(super) fn handle(
        &mut self,
        step: &mut Step<'_, '_, '_>,
        event: Event,
        preview: &mut Preview,
    ) -> Outcome {
        let outcome = match (&mut self.state, event) {
            (_, Event::Escape) => {
                self.state = ThreeState::Idle;
                Outcome::left()
            }
            (ThreeState::Idle, Event::Press(at)) => {
                let start = placed_at(step, at, &[]);
                if !reach(&[&start]) {
                    Outcome::said(Message::TooFar)
                } else {
                    self.state = ThreeState::Pressed {
                        start,
                        press: at,
                        dragged: false,
                    };
                    Outcome::nothing()
                }
            }
            (ThreeState::Pressed { press, dragged, .. }, Event::Move(at)) => {
                if distance(at, *press) > step.pointer.tolerance {
                    *dragged = true;
                }
                Outcome::nothing()
            }
            (
                ThreeState::Pressed {
                    start,
                    press,
                    dragged,
                },
                Event::Release(at),
            ) => {
                let start = start.clone();
                if *dragged || distance(at, *press) > step.pointer.tolerance {
                    self.end(step, start, at)
                } else {
                    self.state = ThreeState::Start {
                        start,
                        pressed: false,
                    };
                    Outcome::nothing()
                }
            }
            (
                ThreeState::Start { pressed, .. } | ThreeState::Ends { pressed, .. },
                Event::Press(_),
            ) => {
                *pressed = true;
                Outcome::nothing()
            }
            (ThreeState::Start { start, pressed }, Event::Release(at)) => {
                if !*pressed {
                    Outcome::nothing()
                } else {
                    *pressed = false;
                    let start = start.clone();
                    self.end(step, start, at)
                }
            }
            (
                ThreeState::Ends {
                    start,
                    end,
                    pressed,
                },
                Event::Release(at),
            ) => {
                if !*pressed {
                    Outcome::nothing()
                } else {
                    *pressed = false;
                    let tolerance = step.pointer.tolerance;
                    if distance(at, start.at) <= tolerance || distance(at, end.at) <= tolerance {
                        // On an end: a double-click's second click.
                        Outcome::nothing()
                    } else {
                        let (start, end) = (start.clone(), end.clone());
                        let outcome = finish_three(step, &start, &end, at);
                        if outcome.add.is_some() {
                            self.state = ThreeState::Idle;
                        }
                        outcome
                    }
                }
            }
            _ => Outcome::nothing(),
        };
        self.draw(step, event, preview);
        outcome
    }

    /// The end set where the pointer places it: unless it's on the start.
    fn end(&mut self, step: &Step<'_, '_, '_>, start: Placed, at: Xy) -> Outcome {
        let besides: Vec<&ElementId> = start.shared().into_iter().collect();
        let end = placed_at(step, at, &besides);
        if on_top(step, &start, &end) {
            self.state = ThreeState::Start {
                start,
                pressed: false,
            };
            return Outcome::nothing();
        }
        if one_place(start.at, end.at) || !reach(&[&end]) {
            let message = if one_place(start.at, end.at) {
                Message::TooSmall
            } else {
                Message::TooFar
            };
            self.state = ThreeState::Start {
                start,
                pressed: false,
            };
            return Outcome::said(message);
        }
        self.state = ThreeState::Ends {
            start,
            end,
            pressed: false,
        };
        Outcome::nothing()
    }

    fn draw(&self, step: &Step<'_, '_, '_>, event: Event, preview: &mut Preview) {
        let at = match event {
            Event::Press(at) | Event::Move(at) | Event::Release(at) | Event::DoubleClick(at) => at,
            Event::EndChain | Event::Escape | Event::Enter => return,
        };
        match &self.state {
            ThreeState::Idle => show(preview, &placed_at(step, at, &[])),
            ThreeState::Pressed { start, .. } | ThreeState::Start { start, .. } => {
                let besides: Vec<&ElementId> = start.shared().into_iter().collect();
                let end = placed_at(step, at, &besides);
                preview.lines.push([start.at, end.at]);
                preview.shown.extend(start.shown());
                show(preview, &end);
            }
            ThreeState::Ends { start, end, .. } => {
                preview.shown.extend(start.shown());
                preview.shown.extend(end.shown());
                preview.pointer = Some(at);
                match through_three(start.at, end.at, at) {
                    Some(drawn) => draw_arc(preview, &drawn),
                    None => preview.lines.push([start.at, end.at]),
                }
            }
        }
    }
}

fn finish_three(step: &mut Step<'_, '_, '_>, start: &Placed, end: &Placed, at: Xy) -> Outcome {
    let Some(drawn) = through_three(start.at, end.at, at) else {
        return Outcome::said(Message::InLine);
    };
    let radius = distance(drawn.center, start.at);
    if radius > silico_protocol::MAX_LENGTH || !within_reach(drawn.center) {
        return Outcome::said(Message::TooFar);
    }
    let mut builder = Builder::new(&mut *step.ids, step.pointer.view);
    let built = (|| -> Result<(), crate::Error> {
        let c = builder.new_point(drawn.center, false)?;
        let s = builder.point(start, false)?;
        let e = builder.point(end, false)?;
        if drawn.reversed {
            builder.arc(&c, &e, &s)?;
        } else {
            builder.arc(&c, &s, &e)?;
        }
        Ok(())
    })();
    match built {
        Ok(()) => Outcome::added(builder.members),
        Err(error) => failed(error),
    }
}

// --- Tangent ----------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub(super) struct Tangent {
    state: TangentState,
}

/// The end a tangent arc leaves from: the point, where it is, the curve that ends there,
/// and the direction that curve leaves it along.
#[derive(Debug, Clone)]
struct Base {
    point: ElementId,
    at: Xy,
    curve: ElementId,
    out: Xy,
}

#[derive(Debug, Clone, Default)]
enum TangentState {
    #[default]
    Idle,
    Pressed {
        base: Base,
        press: Xy,
        dragged: bool,
    },
    Clicked {
        base: Base,
        pressed: bool,
    },
}

/// The tangent arc from `base` to `end`: none where `end` lies straight ahead or behind.
fn tangent_from(base: &Base, end: Xy) -> Option<Drawn> {
    let (center, ccw) = tangent_arc(base.at, base.out, end)?;
    Some(Drawn::new(center, base.at, end, ccw))
}

impl Tangent {
    pub(super) fn busy(&self) -> bool {
        !matches!(self.state, TangentState::Idle)
    }

    pub(super) fn handle(
        &mut self,
        step: &mut Step<'_, '_, '_>,
        event: Event,
        preview: &mut Preview,
    ) -> Outcome {
        let outcome = match (&mut self.state, event) {
            (_, Event::Escape) => {
                self.state = TangentState::Idle;
                Outcome::left()
            }
            (TangentState::Idle, Event::Press(at)) => match base_at(step, at) {
                Ok(base) => {
                    self.state = TangentState::Pressed {
                        base,
                        press: at,
                        dragged: false,
                    };
                    Outcome::nothing()
                }
                Err(message) => Outcome::said(message),
            },
            (TangentState::Pressed { press, dragged, .. }, Event::Move(at)) => {
                if distance(at, *press) > step.pointer.tolerance {
                    *dragged = true;
                }
                Outcome::nothing()
            }
            (
                TangentState::Pressed {
                    base,
                    press,
                    dragged,
                },
                Event::Release(at),
            ) => {
                let base = base.clone();
                if *dragged || distance(at, *press) > step.pointer.tolerance {
                    self.state = TangentState::Idle;
                    match finish_tangent(step, &base, at) {
                        Some(outcome) => outcome,
                        None => Outcome::said(Message::TooSmall),
                    }
                } else {
                    self.state = TangentState::Clicked {
                        base,
                        pressed: false,
                    };
                    Outcome::nothing()
                }
            }
            (TangentState::Clicked { pressed, .. }, Event::Press(_)) => {
                *pressed = true;
                Outcome::nothing()
            }
            (TangentState::Clicked { base, pressed }, Event::Release(at)) => {
                if !*pressed {
                    Outcome::nothing()
                } else {
                    *pressed = false;
                    let base = base.clone();
                    match finish_tangent(step, &base, at) {
                        Some(outcome) => {
                            if outcome.add.is_some() {
                                self.state = TangentState::Idle;
                            }
                            outcome
                        }
                        None => Outcome::nothing(),
                    }
                }
            }
            _ => Outcome::nothing(),
        };
        self.draw(step, event, preview);
        outcome
    }

    fn draw(&self, step: &Step<'_, '_, '_>, event: Event, preview: &mut Preview) {
        let at = match event {
            Event::Press(at) | Event::Move(at) | Event::Release(at) | Event::DoubleClick(at) => at,
            Event::EndChain | Event::Escape | Event::Enter => return,
        };
        match &self.state {
            TangentState::Idle => {
                let placed = step.pointer.shared_only(at);
                preview.pointer = Some(placed.at);
                preview.shown.extend(placed.shown());
            }
            TangentState::Pressed { base, .. } | TangentState::Clicked { base, .. } => {
                let end = placed_at(step, at, &[&base.point]);
                preview.shown.push(Shown {
                    at: base.at,
                    relation: Relation::Tangent(base.curve.clone()),
                });
                match tangent_from(base, end.at) {
                    Some(drawn) => draw_arc(preview, &drawn),
                    None => preview.lines.push([base.at, end.at]),
                }
                show(preview, &end);
            }
        }
    }
}

/// The end a tangent arc pressed at `at` leaves from: a point of the sketch where exactly
/// one line or arc ends; or what the tool says.
fn base_at(step: &Step<'_, '_, '_>, at: Xy) -> Result<Base, Message> {
    let placed = step.pointer.shared_only(at);
    let Target::Shared(point) = placed.target else {
        return Err(Message::NoEndToFollow);
    };
    let mut ends = step.pointer.view.ends_at(&point).into_iter();
    match (ends.next(), ends.next()) {
        (None, _) => Err(Message::NoEndToFollow),
        (Some((curve, out, _)), None) => Ok(Base {
            point,
            at: placed.at,
            curve,
            out,
        }),
        (Some(_), Some(_)) => Err(Message::SeveralEnds(point)),
    }
}

/// The tangent arc from `base` to where the pointer places its end, `at`: none if the end is
/// on the base (nothing drawn yet).
fn finish_tangent(step: &mut Step<'_, '_, '_>, base: &Base, at: Xy) -> Option<Outcome> {
    let end = placed_at(step, at, &[&base.point]);
    if end.shared() == Some(&base.point)
        || (end.shared().is_none() && distance(end.at, base.at) <= step.pointer.tolerance)
    {
        return None;
    }
    if one_place(end.at, base.at) {
        return Some(Outcome::said(Message::TooSmall));
    }
    let Some(drawn) = tangent_from(base, end.at) else {
        return Some(Outcome::said(Message::InLine));
    };
    let radius = distance(drawn.center, base.at);
    if radius > silico_protocol::MAX_LENGTH || !within_reach(drawn.center) || !reach(&[&end]) {
        return Some(Outcome::said(Message::TooFar));
    }
    let mut builder = Builder::new(&mut *step.ids, step.pointer.view);
    let built = (|| -> Result<(), crate::Error> {
        let c = builder.new_point(drawn.center, false)?;
        let e = builder.point(&end, false)?;
        let arc = if drawn.reversed {
            builder.arc(&c, &e, &base.point)?
        } else {
            builder.arc(&c, &base.point, &e)?
        };
        builder.constraint(Form::TangentAt {
            a: Reference::Element(base.curve.clone()),
            b: Reference::Element(arc),
            at: Reference::Element(base.point.clone()),
            direction: Direction::Same,
        })
    })();
    Some(match built {
        Ok(()) => Outcome::added(builder.members),
        Err(error) => failed(error),
    })
}
