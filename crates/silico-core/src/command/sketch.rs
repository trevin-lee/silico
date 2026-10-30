//! M5.10's sketch read, `sketch` (docs/design/m5.0-app.md, section 7, *`sketch` (M5.10)*): a
//! sketch as the model a read names solved it, for sketch mode to draw and colour, and for an
//! agent, which reads the same thing (invariant 3). Its plane's frame in 3D; its state and
//! degrees of freedom; each member in its document form, with its solved values in place of
//! the stored ones and its definition state ([`crate::sketch_states`]); its profile's regions
//! (M3.5), or why it makes none; and why it wasn't solved, if it wasn't.
//!
//! A model keeps each sketch's diagnosis beside its solve's outcome, both read from the
//! answer its rebuild's solve gave ([`Model::diagnosed`]), so the read gives exactly what
//! the rebuild found, and asks no worker anything: neither the sketch worker nor the kernel
//! (M5.0's A8: "`sketch` and `timeline` need none").

use super::elements::worded;
use super::{ModelChoice, Problems, ReadCommand, Running, SessionId, Sessions};
use crate::document::{ElementId, ElementKind, FeatureId, Operation, Plane, Sketch};
use crate::kernel::Clients;
use crate::profile::{self, Profile};
use crate::recompute::Model;
use crate::references::CANDIDATES;
use crate::sketch_states::{self, SketchStates};
use crate::worker::SILICO_BUG;
use crate::{Error, ErrorCode};

impl<P: Clients + 'static> Sessions<P> {
    /// `sketch` (M5.10): the sketch `sketch` of the model `model` names (decision 33), as
    /// its rebuild solved it ([`SketchRead`]). Refused, naming `sketch`, for a feature the
    /// document doesn't have, or one that isn't a sketch, the document's first sketches the
    /// candidates; and with the rebuild's own error for a sketch it couldn't solve or
    /// diagnose for a reason that isn't the sketch's (a worker's failure), as `check`
    /// reports it. Read as every model is ([`read_model`](Self::read_model)): the current one
    /// queued, waiting for the revision's rebuild, or the newest published at once, counted
    /// among the session's published reads.
    pub fn sketch(
        &self,
        session: &SessionId,
        model: ModelChoice,
        sketch: FeatureId,
    ) -> Running<SketchRead> {
        self.read_model(session, ReadCommand::Sketch, model, move |read| {
            let _span = tracing::info_span!(
                "sketch",
                feature = sketch.as_str(),
                model = read.model_read().of.as_str()
            )
            .entered();
            read_sketch(read.model(), &sketch)
        })
    }
}

/// What `sketch` answers: the sketch as the model's solve left it.
#[derive(Debug, Clone, PartialEq)]
pub struct SketchRead {
    /// The sketch's ID, as the request named it.
    pub sketch: FeatureId,
    /// The plane it's drawn on.
    pub plane: Plane,
    /// The sketch in its document form: its points, circles' radii, and reference
    /// dimensions' values as the solve gave them, written as a save writes them (M3.4); or,
    /// for a sketch that wasn't solved, as stored.
    pub members: Sketch,
    /// Its state, its degrees of freedom, each member's state, and why it wasn't solved
    /// (naming the sketch, as `check` reports it), if it wasn't.
    pub states: SketchStates,
    /// The profile's regions, by their outer loops' smallest curve IDs: none if it makes no
    /// profile, or wasn't solved.
    pub regions: Vec<Region>,
    /// Why the solved sketch makes no profile, M3.5's refusals in the order they're checked,
    /// the first [`PROBLEMS_LISTED`](crate::document::PROBLEMS_LISTED): none if it makes one,
    /// or wasn't solved (`solve_problem` says why).
    pub profile_problems: Vec<Error>,
    /// How many refusals there are, listed or not (M3.5's count, as `check` gives it for a
    /// feature that sweeps the sketch).
    pub profile_problems_total: u64,
}

/// One region of a sketch's profile, by its curves' IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    /// Its outer loop's curves, in the order the loop walks them, counter-clockwise.
    pub outer: Vec<ElementId>,
    /// Each hole's curves, in the order it walks them, clockwise; the holes by their
    /// smallest curve IDs.
    pub holes: Vec<Vec<ElementId>>,
}

/// The sketch `id` of `model`, as its rebuild solved it: its states from the diagnosis
/// the rebuild's solve read beside its outcome ([`Model::diagnosed`]), its members at their
/// solved values, and its profile, from the solution. One the rebuild couldn't diagnose is
/// refused with the rebuild's error, which says nothing of the sketch's state, as is one
/// whose outcome is no state ([`sketch_states::states`]).
fn read_sketch(model: &Model, id: &FeatureId) -> Result<SketchRead, Problems> {
    let sketch = sketch_of(model, id)?;
    let diagnosed = match model.diagnosed(id) {
        Some(Ok(diagnosed)) => diagnosed,
        // A worker's failure, as every read refuses with the fault the model kept.
        Some(Err(error)) => return Err(worded(error).into()),
        None => return Err(worded(&unsolved(model, id)).into()),
    };
    let named = |e: Error| Problems::from(e.with_feature(id.as_str()));
    let mut states = sketch_states::states(sketch, diagnosed).map_err(named)?;
    states.solve_problem = states
        .solve_problem
        .map(|problem| problem.with_feature(id.as_str()));
    let (members, regions, (profile_problems, profile_problems_total)) = match diagnosed.outcome() {
        Ok(solution) => {
            let members = solution.written(sketch);
            match profile::of_sketch(id, sketch, solution) {
                Ok(profile) => (members, regions(&profile), (Vec::new(), 0)),
                Err(refused) => (
                    members,
                    Vec::new(),
                    (refused.errors().to_vec(), refused.total()),
                ),
            }
        }
        Err(_) => (sketch.clone(), Vec::new(), (Vec::new(), 0)),
    };
    every_member_stated(id, &members, &states)?;
    Ok(SketchRead {
        sketch: id.clone(),
        plane: sketch.plane,
        members,
        states,
        regions,
        profile_problems,
        profile_problems_total,
    })
}

/// The state `states` gives the member `id` of `kind`, as its word: none only if it gives
/// that member none.
pub(super) fn state_word(
    states: &SketchStates,
    kind: ElementKind,
    id: &ElementId,
) -> Option<&'static str> {
    match kind {
        ElementKind::Point => states.points.get(id).map(|s| s.as_str()),
        ElementKind::Line => states.lines.get(id).map(|s| s.as_str()),
        ElementKind::Circle => states.circles.get(id).map(|s| s.as_str()),
        ElementKind::Arc => states.arcs.get(id).map(|s| s.as_str()),
        ElementKind::Constraint => states.constraints.get(id).map(|s| s.as_str()),
    }
}

/// Whether `states` gives every member of `members`, the sketch `id`'s, a state, as
/// [`sketch_states::states`] gives each one. The first in the format's order that has none is
/// a bug: an `internal` error naming it and the sketch, so the read is refused rather than
/// written with a member that has no state.
fn every_member_stated(
    id: &FeatureId,
    members: &Sketch,
    states: &SketchStates,
) -> Result<(), Error> {
    match members
        .elements()
        .find(|&(member, kind)| state_word(states, kind, member).is_none())
    {
        None => Ok(()),
        Some((member, kind)) => Err(Error::new(
            ErrorCode::Internal,
            format!("the sketch's {} {member} was given no state", kind.noun()),
        )
        .with_feature(id.as_str())
        .with_elements([member.as_str()])
        .with_suggestion(SILICO_BUG)),
    }
}

/// Why the model holds no solve of its sketch `id`: what became of it, as `check` reports
/// it, or, if that's nothing, a bug's (every sketch a model's document has is solved).
fn unsolved(model: &Model, id: &FeatureId) -> Error {
    match model.outcome(id).map(|outcome| outcome.errors()) {
        Some([first, ..]) => first.clone(),
        _ => Error::new(
            ErrorCode::Internal,
            format!("the model holds no solve of its sketch {id}"),
        )
        .with_feature(id.as_str())
        .with_suggestion(SILICO_BUG),
    }
}

/// The sketch `id` of `model`'s document, or why there's none: a feature the document
/// doesn't have, or one that isn't a sketch, each `invalid_input` naming `sketch`, the
/// document's first [`CANDIDATES`] sketches, in timeline order, the candidates (as a chain's
/// edit offers its bases).
fn sketch_of<'m>(model: &'m Model, id: &FeatureId) -> Result<&'m Sketch, Problems> {
    let document = model.document();
    let refused = |message: String| {
        let sketches: Vec<String> = document
            .features
            .iter()
            .filter(|f| matches!(f.operation, Operation::Sketch(_)))
            .take(CANDIDATES)
            .map(|f| f.id.as_str().to_owned())
            .collect();
        let suggestion = if sketches.is_empty() {
            "the document has no sketch: add one, and read it by its ID"
        } else {
            "name a sketch of the document, by its ID"
        };
        Problems::from(
            Error::new(ErrorCode::InvalidInput, message)
                .with_parameter("sketch")
                .with_suggestion(suggestion)
                .with_candidates(sketches),
        )
    };
    match document.features.iter().find(|f| &f.id == id) {
        None => Err(refused(format!("the document has no feature {id}"))),
        Some(feature) => match &feature.operation {
            Operation::Sketch(sketch) => Ok(sketch),
            other => Err(refused(format!(
                "{id} is a {}, not a sketch",
                other.type_name()
            ))),
        },
    }
}

/// `profile`'s regions, each loop by its curves' IDs in walking order.
fn regions(profile: &Profile) -> Vec<Region> {
    let curves = |l: &profile::Loop| l.curves.iter().map(|e| e.curve.clone()).collect();
    profile
        .regions
        .iter()
        .map(|r| Region {
            outer: curves(&r.outer),
            holes: r.holes.iter().map(curves).collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Coordinate, SketchLine, SketchPoint};
    use crate::sketch_states::{MemberState, SketchState};

    fn point(id: &str) -> SketchPoint {
        SketchPoint {
            id: ElementId::new(id).unwrap(),
            x: Coordinate::new(0.0).unwrap(),
            y: Coordinate::new(1.0).unwrap(),
            construction: false,
        }
    }

    #[test]
    fn a_member_given_no_state_refuses_the_read_naming_the_first_in_the_formats_order() {
        // Two points and a line; the states give the first point one and nothing else.
        // The line comes after every point in the format's order, so the second point is
        // named first; with it stated, the line; with that, nothing.
        let sketch = Sketch {
            plane: Plane::Xy,
            points: vec![point("p0000001"), point("p0000002")],
            lines: vec![SketchLine {
                id: ElementId::new("n0000001").unwrap(),
                from: ElementId::new("p0000001").unwrap(),
                to: ElementId::new("p0000002").unwrap(),
                construction: false,
            }],
            circles: Vec::new(),
            arcs: Vec::new(),
            constraints: Vec::new(),
        };
        let id = FeatureId::new("k7f2qx3m").unwrap();
        let p1 = ElementId::new("p0000001").unwrap();
        let p2 = ElementId::new("p0000002").unwrap();
        let mut states = SketchStates {
            state: SketchState::UnderDefined,
            dofs: 4,
            points: [(p1, MemberState::UnderDefined)].into(),
            lines: Default::default(),
            circles: Default::default(),
            arcs: Default::default(),
            constraints: Default::default(),
            solve_problem: None,
        };
        let refused = |states: &SketchStates| {
            every_member_stated(&id, &sketch, states).map_err(|e| {
                (
                    e.code(),
                    e.message().to_owned(),
                    e.feature().map(str::to_owned),
                    e.elements().to_vec(),
                    e.suggestion().map(str::to_owned),
                )
            })
        };
        let bug = Some(SILICO_BUG.to_owned());
        assert_eq!(
            refused(&states),
            Err((
                ErrorCode::Internal,
                "the sketch's point p0000002 was given no state".to_owned(),
                Some("k7f2qx3m".to_owned()),
                vec!["p0000002".to_owned()],
                bug.clone(),
            ))
        );
        states.points.insert(p2, MemberState::UnderDefined);
        assert_eq!(
            refused(&states),
            Err((
                ErrorCode::Internal,
                "the sketch's line n0000001 was given no state".to_owned(),
                Some("k7f2qx3m".to_owned()),
                vec!["n0000001".to_owned()],
                bug,
            ))
        );
        states.lines.insert(
            ElementId::new("n0000001").unwrap(),
            MemberState::UnderDefined,
        );
        assert_eq!(refused(&states), Ok(()));
    }
}
