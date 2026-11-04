//! The point (docs/design/m5.0-app.md, section 9, *Circle, arcs, and points*;
//! docs/ux/solidworks.md, §4): a point at each click, the tool staying. On a curve, `on`; at
//! a line's middle, `midpoint`; where two curves cross, two `on`s; at the origin, level with
//! it and above it (#278's A); level with or above another point, as the pointer shows it
//! (#278's E). On a point the sketch holds, nothing: the tool says so (#278's D).

use super::build::Builder;
use super::infer::{Align, Placed};
use super::{Event, Message, Outcome, Preview, Step, Xy, failed, within_reach};

#[derive(Debug, Clone, Default)]
pub(super) struct Point {
    pressed: bool,
}

impl Point {
    pub(super) fn busy(&self) -> bool {
        self.pressed
    }

    pub(super) fn handle(
        &mut self,
        step: &mut Step<'_, '_, '_>,
        event: Event,
        preview: &mut Preview,
    ) -> Outcome {
        let outcome = match event {
            Event::Escape => {
                self.pressed = false;
                Outcome::left()
            }
            Event::Press(_) => {
                self.pressed = true;
                Outcome::default()
            }
            Event::Release(at) if self.pressed => {
                self.pressed = false;
                place(step, at)
            }
            _ => Outcome::default(),
        };
        if let Event::Press(at) | Event::Move(at) | Event::Release(at) = event {
            let placed = placed_at(step, at);
            preview.pointer = Some(placed.at);
            preview.shown.extend(placed.shown());
            preview.guides.extend(placed.guides.iter().copied());
        }
        outcome
    }
}

fn placed_at(step: &Step<'_, '_, '_>, at: Xy) -> Placed {
    let mut placed = step.pointer.snap(at);
    step.pointer.align(&mut placed, Align::Relations, &[]);
    placed
}

fn place(step: &mut Step<'_, '_, '_>, at: Xy) -> Outcome {
    let placed = placed_at(step, at);
    if let Some(id) = placed.shared() {
        return Outcome::said(Message::PointThere(id.clone()));
    }
    if !within_reach(placed.at) {
        return Outcome::said(Message::TooFar);
    }
    let mut builder = Builder::new(&mut *step.ids, step.pointer.view, step.construction);
    match builder.point(&placed, false) {
        Ok(_) => Outcome::added(builder.members),
        Err(error) => failed(error),
    }
}
