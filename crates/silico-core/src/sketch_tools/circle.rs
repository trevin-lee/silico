//! The circle (docs/design/m5.0-app.md, section 9, *Circle, arcs, and points*;
//! docs/ux/solidworks.md, §4): its centre, then its radius, by a press, a drag, and a
//! release, or by two clicks. The centre takes what a line's end takes; a centre on a centre
//! is one point, concentric. The radius set on a point of the sketch (or the origin) passes
//! through it: `on` (the study's *Recommended for M5.11*, 6), an inferred relation.

use super::build::Builder;
use super::geometry::distance;
use super::infer::{Align, Placed, Target, View};
use super::{Event, Message, Outcome, Preview, Relation, Shown, Step, Xy, failed, within_reach};
use crate::document::{Form, Reference};

#[derive(Debug, Clone, Default)]
pub(super) struct Circle {
    state: State,
}

#[derive(Debug, Clone, Default)]
enum State {
    #[default]
    Idle,
    Pressed {
        centre: Placed,
        press: Xy,
        dragged: bool,
    },
    Clicked {
        centre: Placed,
        pressed: bool,
    },
}

/// Where the radius is set, and what the circle passes through there, if the pointer shows
/// it: a point of the sketch, or the origin.
fn rim(step: &Step<'_, '_, '_>, centre: &Placed, at: Xy) -> (Xy, Option<Reference>) {
    if !step.pointer.infer {
        return (at, None);
    }
    let placed = step.pointer.snap(at);
    match placed.target {
        Target::Shared(id) if centre.shared() != Some(&id) => {
            (placed.at, Some(Reference::Element(id)))
        }
        Target::Origin if centre.target != Target::Origin => (placed.at, Some(Reference::Origin)),
        _ => (at, None),
    }
}

impl Circle {
    pub(super) fn busy(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// The gesture against `view`, a newer sketch ([`Placed::revalidated`]).
    pub(super) fn revalidate(&mut self, view: &View<'_>) {
        if let State::Pressed { centre, .. } | State::Clicked { centre, .. } = &mut self.state {
            *centre = centre.revalidated(view);
        }
    }

    pub(super) fn handle(
        &mut self,
        step: &mut Step<'_, '_, '_>,
        event: Event,
        preview: &mut Preview,
    ) -> Outcome {
        let outcome = match (&mut self.state, event) {
            (_, Event::Escape) => {
                self.state = State::Idle;
                Outcome::left()
            }
            (State::Idle, Event::Press(at)) => {
                let centre = centre_at(step, at);
                if centre.shared().is_none() && !within_reach(centre.at) {
                    Outcome::said(Message::TooFar)
                } else {
                    self.state = State::Pressed {
                        centre,
                        press: at,
                        dragged: false,
                    };
                    Outcome::nothing()
                }
            }
            (State::Pressed { press, dragged, .. }, Event::Move(at)) => {
                if distance(at, *press) > step.pointer.tolerance {
                    *dragged = true;
                }
                Outcome::nothing()
            }
            (
                State::Pressed {
                    centre,
                    press,
                    dragged,
                },
                Event::Release(at),
            ) => {
                let centre = centre.clone();
                if *dragged && distance(at, *press) <= step.pointer.tolerance {
                    // A drag back to where it began.
                    self.state = State::Idle;
                    Outcome::said(Message::TooSmall)
                } else if *dragged || distance(at, *press) > step.pointer.tolerance {
                    self.state = State::Idle;
                    finish(step, &centre, at)
                } else {
                    self.state = State::Clicked {
                        centre,
                        pressed: false,
                    };
                    Outcome::nothing()
                }
            }
            (State::Clicked { pressed, .. }, Event::Press(_)) => {
                *pressed = true;
                Outcome::nothing()
            }
            (State::Clicked { centre, pressed }, Event::Release(at)) => {
                if !*pressed {
                    Outcome::nothing()
                } else if distance(at, centre.at) <= step.pointer.tolerance {
                    *pressed = false;
                    Outcome::nothing()
                } else {
                    let centre = centre.clone();
                    let outcome = finish(step, &centre, at);
                    self.state = if outcome.add.is_some() {
                        State::Idle
                    } else {
                        State::Clicked {
                            centre,
                            pressed: false,
                        }
                    };
                    outcome
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
            State::Idle => {
                let placed = centre_at(step, at);
                preview.pointer = Some(placed.at);
                preview.shown.extend(placed.shown());
                preview.guides.extend(placed.guides.iter().copied());
            }
            State::Pressed { centre, .. } | State::Clicked { centre, .. } => {
                let (rim, through) = rim(step, centre, at);
                preview.points.push(centre.at);
                preview.circles.push((centre.at, distance(rim, centre.at)));
                preview.shown.extend(centre.shown());
                preview.pointer = Some(rim);
                if let Some(through) = through {
                    preview.shown.push(Shown {
                        at: rim,
                        relation: match through {
                            Reference::Element(id) => Relation::Shared(id),
                            _ => Relation::Origin,
                        },
                    });
                }
            }
        }
    }
}

fn centre_at(step: &Step<'_, '_, '_>, at: Xy) -> Placed {
    let mut centre = step.pointer.snap(at);
    step.pointer.align(&mut centre, Align::Relations, &[]);
    centre
}

fn finish(step: &mut Step<'_, '_, '_>, centre: &Placed, at: Xy) -> Outcome {
    let (rim, through) = rim(step, centre, at);
    let radius = distance(rim, centre.at);
    if radius <= silico_protocol::LENGTH_TOLERANCE {
        return Outcome::said(Message::TooSmall);
    }
    if radius > silico_protocol::MAX_LENGTH {
        return Outcome::said(Message::TooFar);
    }
    let mut builder = Builder::new(&mut *step.ids, step.pointer.view);
    let built = (|| -> Result<(), crate::Error> {
        let c = builder.point(centre, false)?;
        let circle = builder.circle(&c, radius)?;
        if let Some(point) = through {
            builder.constraint(Form::On {
                point,
                curve: Reference::Element(circle),
            })?;
        }
        Ok(())
    })();
    match built {
        Ok(()) => Outcome::added(builder.members),
        Err(error) => failed(error),
    }
}
