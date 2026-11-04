//! A gesture's members, as a `sketch_add` takes them: each new member with a fresh ID, each
//! point a click shares named by its own.

use std::collections::BTreeSet;

use super::infer::{Placed, View};
use super::{NewIds, Xy, not_finite};
use crate::document::sketch_edit::Members;
use crate::document::{
    Coordinate, ElementId, Form, Reference, SketchArc, SketchCircle, SketchConstraint, SketchLine,
    SketchPoint,
};
use crate::{Error, ErrorCode};

pub(super) struct Builder<'i> {
    ids: &'i mut dyn NewIds,
    /// The IDs the sketch holds, and those given here.
    taken: BTreeSet<ElementId>,
    /// Whether each line, circle, and arc is construction (*For construction*); points are
    /// as each is asked for.
    construction: bool,
    pub(super) members: Members,
}

impl<'i> Builder<'i> {
    pub(super) fn new(ids: &'i mut dyn NewIds, view: &View<'_>, construction: bool) -> Self {
        Builder {
            ids,
            taken: view.ids(),
            construction,
            members: Members::default(),
        }
    }

    /// A fresh ID: none the sketch holds or this gesture has given.
    fn id(&mut self) -> Result<ElementId, Error> {
        let taken = &self.taken;
        let id = self.ids.next(&|id| taken.contains(id))?;
        if !self.taken.insert(id.clone()) {
            return Err(Error::new(
                ErrorCode::Internal,
                format!("the IDs' source gave {id}, which is taken"),
            )
            .with_suggestion(crate::worker::SILICO_BUG));
        }
        Ok(id)
    }

    /// The point `placed` puts: the one it shares, or a new one, with the relations it
    /// keeps there.
    pub(super) fn point(
        &mut self,
        placed: &Placed,
        construction: bool,
    ) -> Result<ElementId, Error> {
        if let Some(id) = placed.shared() {
            return Ok(id.clone());
        }
        let id = self.new_point(placed.at, construction)?;
        for form in placed.relations(&id) {
            self.constraint(form)?;
        }
        Ok(id)
    }

    /// A new point at `at`, with no relation.
    pub(super) fn new_point(&mut self, at: Xy, construction: bool) -> Result<ElementId, Error> {
        let (Some(x), Some(y)) = (Coordinate::new(at[0]), Coordinate::new(at[1])) else {
            return Err(not_finite("a point"));
        };
        let id = self.id()?;
        self.members.points.push(SketchPoint {
            id: id.clone(),
            x,
            y,
            construction,
        });
        Ok(id)
    }

    pub(super) fn line(&mut self, from: &ElementId, to: &ElementId) -> Result<ElementId, Error> {
        let id = self.id()?;
        self.members.lines.push(SketchLine {
            id: id.clone(),
            from: from.clone(),
            to: to.clone(),
            construction: self.construction,
        });
        Ok(id)
    }

    pub(super) fn circle(&mut self, center: &ElementId, radius: f64) -> Result<ElementId, Error> {
        let radius = Coordinate::new(radius).ok_or_else(|| not_finite("a radius"))?;
        let id = self.id()?;
        self.members.circles.push(SketchCircle {
            id: id.clone(),
            center: center.clone(),
            radius,
            construction: self.construction,
        });
        Ok(id)
    }

    /// An arc counter-clockwise from `start` to `end` about `center`.
    pub(super) fn arc(
        &mut self,
        center: &ElementId,
        start: &ElementId,
        end: &ElementId,
    ) -> Result<ElementId, Error> {
        let id = self.id()?;
        self.members.arcs.push(SketchArc {
            id: id.clone(),
            center: center.clone(),
            start: start.clone(),
            end: end.clone(),
            construction: self.construction,
        });
        Ok(id)
    }

    pub(super) fn constraint(&mut self, form: Form) -> Result<(), Error> {
        let id = self.id()?;
        self.members.constraints.push(SketchConstraint { id, form });
        Ok(())
    }

    /// A rectangle's four sides through its corners in turn, the first and third
    /// horizontal, the second and fourth vertical: the tool's own relations.
    pub(super) fn rectangle(&mut self, corners: [&ElementId; 4]) -> Result<(), Error> {
        for (i, from) in corners.iter().enumerate() {
            let to = corners[(i + 1) % 4];
            let line = Reference::Element(self.line(from, to)?);
            self.constraint(if i % 2 == 0 {
                Form::Horizontal { line }
            } else {
                Form::Vertical { line }
            })?;
        }
        Ok(())
    }
}
