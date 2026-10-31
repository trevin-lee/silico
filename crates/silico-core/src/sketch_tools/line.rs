//! The line (docs/design/m5.0-app.md, section 9, *Line*; docs/ux/solidworks.md, §4): the
//! first press decides the mode. A press, a drag, and a release draws one line. A press and
//! release begins a chain: each click adds a segment from the last point, sharing it (D2),
//! until a double-click, *End chain*, or Enter ends the chain and keeps the tool, a click on
//! a point the sketch holds ends it there (its own start too: the chain closes), or Esc ends
//! it and leaves the tool.

use super::build::Builder;
use super::geometry::distance;
use super::infer::{Align, Along, Placed, View, along, one_place, onto};
use super::{Event, Message, Outcome, Preview, Shown, Step, Xy, within_reach};
use crate::document::ElementId;
use crate::document::sketch_edit::Members;

#[derive(Debug, Clone, Default)]
pub(super) struct Line {
    state: State,
}

#[derive(Debug, Clone, Default)]
enum State {
    #[default]
    Idle,
    /// The first press: where the line starts, where it was pressed, and whether the
    /// pointer has left the press (click-drag).
    Pressed {
        start: Placed,
        press: Xy,
        dragged: bool,
    },
    /// A chain: its last point, and whether a click's press is down.
    Chain { last: Start, pressed: bool },
}

/// Where the next segment starts: a point not yet added, or one the sketch (or the chain)
/// holds.
#[derive(Debug, Clone)]
enum Start {
    New(Placed),
    Point(ElementId, Xy),
}

impl Start {
    fn of(placed: Placed) -> Start {
        match placed.shared() {
            Some(id) => Start::Point(id.clone(), placed.at),
            None => Start::New(placed),
        }
    }

    fn at(&self) -> Xy {
        match self {
            Start::New(placed) => placed.at,
            Start::Point(_, at) => *at,
        }
    }

    fn id(&self) -> Option<&ElementId> {
        match self {
            Start::New(_) => None,
            Start::Point(id, _) => Some(id),
        }
    }
}

/// A segment's end, as the pointer shows it: the point, and the directions it shows.
struct End {
    placed: Placed,
    along: Vec<Along>,
}

/// What a segment's click does.
enum Click {
    /// Too near the last point to be a segment: a double-click's second click.
    Ignored,
    Refused(Message),
    Added(Members, ElementId, Xy, bool),
}

impl Line {
    pub(super) fn busy(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    fn end_chain(&mut self) {
        self.state = State::Idle;
    }

    /// The gesture against `view`, a newer sketch: a chain's last point where the sketch now
    /// puts it, or the chain ended if the sketch no longer has it; a first point given up as
    /// [`Placed::revalidated`] gives it up.
    pub(super) fn revalidate(&mut self, view: &View<'_>) {
        self.state = match std::mem::take(&mut self.state) {
            State::Idle => State::Idle,
            State::Pressed {
                start,
                press,
                dragged,
            } => State::Pressed {
                start: start.revalidated(view),
                press,
                dragged,
            },
            State::Chain {
                last: Start::Point(id, _),
                pressed,
            } => match view.point(&id) {
                Some(at) => State::Chain {
                    last: Start::Point(id, at),
                    pressed,
                },
                None => State::Idle,
            },
            State::Chain {
                last: Start::New(placed),
                pressed,
            } => State::Chain {
                last: Start::New(placed.revalidated(view)),
                pressed,
            },
        };
    }

    pub(super) fn handle(
        &mut self,
        step: &mut Step<'_, '_, '_>,
        event: Event,
        preview: &mut Preview,
    ) -> Outcome {
        let outcome = match (&mut self.state, event) {
            (_, Event::Escape) => {
                self.end_chain();
                Outcome::left()
            }
            (State::Idle, Event::Press(at)) => {
                let mut start = step.pointer.snap(at);
                step.pointer.align(&mut start, Align::Guides, &[]);
                if start.shared().is_none() && !within_reach(start.at) {
                    Outcome::said(Message::TooFar)
                } else {
                    self.state = State::Pressed {
                        start,
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
                    start,
                    press,
                    dragged,
                },
                Event::Release(at),
            ) => {
                if *dragged || distance(at, *press) > step.pointer.tolerance {
                    // Click-drag: one line, ended by the release.
                    let from = Start::of(start.clone());
                    let outcome = match segment(step, &from, at) {
                        Click::Ignored => Outcome::said(Message::TooSmall),
                        Click::Refused(message) => Outcome::said(message),
                        Click::Added(members, ..) => Outcome::added(members),
                    };
                    self.end_chain();
                    outcome
                } else {
                    self.state = State::Chain {
                        last: Start::of(start.clone()),
                        pressed: false,
                    };
                    Outcome::nothing()
                }
            }
            (State::Chain { pressed, .. }, Event::Press(_)) => {
                *pressed = true;
                Outcome::nothing()
            }
            (State::Chain { last, pressed }, Event::Release(at)) => {
                if !*pressed {
                    // A release whose press this tool didn't see.
                    Outcome::nothing()
                } else {
                    *pressed = false;
                    match segment(step, last, at) {
                        Click::Ignored => Outcome::nothing(),
                        Click::Refused(message) => Outcome::said(message),
                        Click::Added(members, end, end_at, closes) => {
                            if closes {
                                self.end_chain();
                            } else {
                                self.state = State::Chain {
                                    last: Start::Point(end, end_at),
                                    pressed: false,
                                };
                            }
                            Outcome::added(members)
                        }
                    }
                }
            }
            (
                State::Chain { .. } | State::Pressed { .. },
                Event::DoubleClick(_) | Event::EndChain | Event::Enter,
            ) => {
                self.end_chain();
                Outcome::nothing()
            }
            _ => Outcome::nothing(),
        };
        self.draw(step, event, preview);
        outcome
    }

    /// The preview after the event: the segment under way to the pointer, or the pointer
    /// alone.
    fn draw(&self, step: &Step<'_, '_, '_>, event: Event, preview: &mut Preview) {
        let at = match event {
            Event::Press(at) | Event::Move(at) | Event::Release(at) | Event::DoubleClick(at) => at,
            Event::EndChain | Event::Escape | Event::Enter => return,
        };
        let from = match &self.state {
            State::Idle => {
                let mut placed = step.pointer.snap(at);
                step.pointer.align(&mut placed, Align::Guides, &[]);
                show_point(preview, &placed);
                return;
            }
            State::Pressed { start, .. } => Start::of(start.clone()),
            State::Chain { last, .. } => last.clone(),
        };
        let end = end_of(step, &from, at);
        preview.lines.push([from.at(), end.placed.at]);
        if let Start::New(placed) = &from {
            preview.shown.extend(placed.shown());
        }
        show_point(preview, &end.placed);
        for along in &end.along {
            preview.shown.push(Shown {
                at: end.placed.at,
                relation: along.relation(),
            });
        }
    }
}

fn show_point(preview: &mut Preview, placed: &Placed) {
    preview.pointer = Some(placed.at);
    preview.shown.extend(placed.shown());
    preview.guides.extend(placed.guides.iter().copied());
}

/// Where a segment from `start` ends with the pointer at `at`, and the direction it shows:
/// the point the pointer places; then, for a point placed on nothing, moved onto the
/// direction it shows (horizontal, vertical, tangent, perpendicular, parallel), and lined up
/// with the points it's level with or above as guides only, which keep the direction it
/// shows (#278's E: a horizontal line's end above a point is the help's own example of a
/// dotted line that adds nothing). A point the pointer fixes (shared, on a curve) isn't
/// moved; the direction it shows from there is added all the same (#278's G).
fn end_of(step: &Step<'_, '_, '_>, start: &Start, at: Xy) -> End {
    let pointer = &step.pointer;
    let mut placed = pointer.snap(at);
    if !pointer.infer {
        return End {
            placed,
            along: Vec::new(),
        };
    }
    let from = start.at();
    let shows = along(pointer.view, from, start.id(), placed.at);
    let free = placed.target == super::infer::Target::Free;
    if free && let Some((_, direction)) = &shows {
        placed.at = onto(from, *direction, placed.at);
    }
    let besides: Vec<&ElementId> = start.id().into_iter().collect();
    let along = shows.map(|(a, _)| a).unwrap_or_default();
    let align = match along.as_slice() {
        [] => Align::Guides,
        [Along::Horizontal] => Align::GuideX,
        [Along::Vertical] => Align::GuideY,
        _ => Align::None,
    };
    pointer.align(&mut placed, align, &besides);
    End { placed, along }
}

/// The segment from `start` to the pointer at `at`: its members, its end's ID and place,
/// and whether that end is a point the sketch held (which ends a chain); or why there's
/// none.
fn segment(step: &mut Step<'_, '_, '_>, start: &Start, at: Xy) -> Click {
    let end = end_of(step, start, at);
    let from = start.at();
    match (start.id(), end.placed.shared()) {
        (Some(a), Some(b)) if a == b => return Click::Ignored,
        (_, None) if distance(end.placed.at, from) <= step.pointer.tolerance => {
            return Click::Ignored;
        }
        _ => {}
    }
    if one_place(end.placed.at, from) {
        return Click::Refused(Message::TooSmall);
    }
    if end.placed.shared().is_none() && !within_reach(end.placed.at) {
        return Click::Refused(Message::TooFar);
    }
    let mut builder = Builder::new(&mut *step.ids, step.pointer.view);
    let built = (|| -> Result<ElementId, crate::Error> {
        let a = match start {
            Start::New(placed) => builder.point(placed, false)?,
            Start::Point(id, _) => id.clone(),
        };
        let b = builder.point(&end.placed, false)?;
        let line = builder.line(&a, &b)?;
        for along in &end.along {
            builder.constraint(along.form(&line, &a))?;
        }
        Ok(b)
    })();
    match built {
        Ok(b) => Click::Added(
            builder.members,
            b,
            end.placed.at,
            end.placed.shared().is_some(),
        ),
        Err(error) => Click::Refused(Message::NoId(error)),
    }
}
