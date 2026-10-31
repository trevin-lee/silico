//! The corner and centre rectangles (docs/design/m5.0-app.md, section 9, *Corner
//! rectangle* and *Centre rectangle*; docs/ux/solidworks.md, §4). A corner rectangle is a
//! press at its first corner, a drag, and a release at the opposite one, or two clicks; a
//! centre rectangle's first is its centre, its second a corner. Shift held at the second
//! makes a square. Each is four lines sharing four points, the first and third sides
//! `horizontal`, the second and fourth `vertical`: the tool's own relations, kept with Ctrl
//! and with *Automatic relations* off (FreeCAD 1.1.3's, #278's F). A centre rectangle's
//! centre is one construction point, with a `symmetric` of two opposite corners about it, and
//! no diagonals (#278's C).

use super::build::Builder;
use super::geometry::{add, distance, scale, sub};
use super::infer::{Align, Placed};
use super::{Event, Message, Outcome, Preview, Step, Xy, failed, within_reach};
use crate::document::{Form, Reference};

#[derive(Debug, Clone)]
pub(super) struct Rectangle {
    /// A centre rectangle, whose first point is its centre.
    centred: bool,
    state: State,
}

#[derive(Debug, Clone, Default)]
enum State {
    #[default]
    Idle,
    /// The first point pressed: click-drag if the pointer leaves the press.
    Pressed {
        first: Placed,
        press: Xy,
        dragged: bool,
    },
    /// The first point clicked: click-click, the next click the corner.
    Clicked { first: Placed, pressed: bool },
}

impl Rectangle {
    pub(super) fn corner() -> Self {
        Rectangle {
            centred: false,
            state: State::Idle,
        }
    }

    pub(super) fn centre() -> Self {
        Rectangle {
            centred: true,
            state: State::Idle,
        }
    }

    pub(super) fn busy(&self) -> bool {
        !matches!(self.state, State::Idle)
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
                let first = self.first(step, at);
                if first.shared().is_none() && !within_reach(first.at) {
                    Outcome::said(Message::TooFar)
                } else {
                    self.state = State::Pressed {
                        first,
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
                    first,
                    press,
                    dragged,
                },
                Event::Release(at),
            ) => {
                let first = first.clone();
                if *dragged || distance(at, *press) > step.pointer.tolerance {
                    self.state = State::Idle;
                    self.finish(step, &first, at)
                } else {
                    self.state = State::Clicked {
                        first,
                        pressed: false,
                    };
                    Outcome::nothing()
                }
            }
            (State::Clicked { pressed, .. }, Event::Press(_)) => {
                *pressed = true;
                Outcome::nothing()
            }
            (State::Clicked { first, pressed }, Event::Release(at)) => {
                if !*pressed {
                    Outcome::nothing()
                } else if distance(at, first.at) <= step.pointer.tolerance {
                    // A double-click's second click, at the first point.
                    *pressed = false;
                    Outcome::nothing()
                } else {
                    let first = first.clone();
                    let outcome = self.finish(step, &first, at);
                    self.state = if outcome.add.is_some() {
                        State::Idle
                    } else {
                        State::Clicked {
                            first,
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

    /// The first point, where the pointer places it: a corner lines up as a guide (its
    /// sides are the rectangle's own horizontal and vertical); a centre, with nothing of
    /// its own, as relations.
    fn first(&self, step: &Step<'_, '_, '_>, at: Xy) -> Placed {
        let mut first = step.pointer.snap(at);
        let align = if self.centred {
            Align::Relations
        } else {
            Align::Guides
        };
        step.pointer.align(&mut first, align, &[]);
        first
    }

    /// The corner opposite the first (a corner rectangle's) or the one the pointer drags (a
    /// centre rectangle's), with the pointer at `at`: with Shift, a square's, placed there
    /// and shared only with a point of the sketch there; otherwise where the pointer places
    /// it, lined up as a guide.
    fn corner_at(&self, step: &Step<'_, '_, '_>, first: &Placed, at: Xy) -> Placed {
        if step.shift {
            let d = sub(at, first.at);
            let side = d[0].abs().max(d[1].abs());
            let sign = |v: f64| if v < 0.0 { -1.0 } else { 1.0 };
            // From a corner, the opposite one; from a centre, a corner: either way the
            // pointer's farther offset, both ways.
            let square = add(first.at, [sign(d[0]) * side, sign(d[1]) * side]);
            return step.pointer.shared_only(square);
        }
        let mut corner = step.pointer.snap(at);
        let besides: Vec<_> = first.shared().into_iter().collect();
        step.pointer.align(&mut corner, Align::Guides, &besides);
        corner
    }

    /// The four corners in turn, from the first point and the corner the pointer places.
    fn corners(&self, first: Xy, corner: Xy) -> [Xy; 4] {
        let opposite = if self.centred {
            sub(scale(first, 2.0), corner)
        } else {
            first
        };
        // c0 and c2 diagonal; c1 level with c0 above c2; c3 above c0 level with c2.
        [
            opposite,
            [corner[0], opposite[1]],
            corner,
            [opposite[0], corner[1]],
        ]
    }

    fn finish(&self, step: &mut Step<'_, '_, '_>, first: &Placed, at: Xy) -> Outcome {
        let corner = self.corner_at(step, first, at);
        let [c0, c1, c2, c3] = self.corners(first.at, corner.at);
        let same_point = first.shared().is_some() && first.shared() == corner.shared();
        let flat = !((c2[0] - c0[0]).abs() > silico_protocol::LENGTH_TOLERANCE
            && (c2[1] - c0[1]).abs() > silico_protocol::LENGTH_TOLERANCE);
        if same_point || flat {
            return Outcome::said(Message::TooSmall);
        }
        if ![c0, c1, c2, c3].into_iter().all(within_reach) {
            return Outcome::said(Message::TooFar);
        }
        let mut builder = Builder::new(&mut *step.ids, step.pointer.view);
        let built = (|| -> Result<(), crate::Error> {
            if self.centred {
                let centre = builder.point(first, true)?;
                let p0 = builder.new_point(c0, false)?;
                let p1 = builder.new_point(c1, false)?;
                let p2 = builder.point(&corner, false)?;
                let p3 = builder.new_point(c3, false)?;
                builder.rectangle([&p0, &p1, &p2, &p3])?;
                builder.constraint(Form::Symmetric {
                    a: Reference::Element(p0),
                    b: Reference::Element(p2),
                    about: Reference::Element(centre),
                })
            } else {
                let p0 = builder.point(first, false)?;
                let p1 = builder.new_point(c1, false)?;
                let p2 = builder.point(&corner, false)?;
                let p3 = builder.new_point(c3, false)?;
                builder.rectangle([&p0, &p1, &p2, &p3])
            }
        })();
        match built {
            Ok(()) => Outcome::added(builder.members),
            Err(error) => failed(error),
        }
    }

    fn draw(&self, step: &Step<'_, '_, '_>, event: Event, preview: &mut Preview) {
        let at = match event {
            Event::Press(at) | Event::Move(at) | Event::Release(at) | Event::DoubleClick(at) => at,
            Event::EndChain | Event::Escape | Event::Enter => return,
        };
        let first = match &self.state {
            State::Idle => {
                let placed = self.first(step, at);
                preview.pointer = Some(placed.at);
                preview.shown.extend(placed.shown());
                preview.guides.extend(placed.guides.iter().copied());
                return;
            }
            State::Pressed { first, .. } | State::Clicked { first, .. } => first,
        };
        let corner = self.corner_at(step, first, at);
        let corners = self.corners(first.at, corner.at);
        for (i, from) in corners.iter().enumerate() {
            preview.lines.push([*from, corners[(i + 1) % 4]]);
        }
        if self.centred {
            preview.construction_points.push(first.at);
        }
        preview.shown.extend(first.shown());
        preview.pointer = Some(corner.at);
        preview.shown.extend(corner.shown());
        preview.guides.extend(corner.guides.iter().copied());
    }
}
