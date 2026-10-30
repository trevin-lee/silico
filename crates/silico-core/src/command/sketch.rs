//! M5.10's sketch read, `sketch` (docs/design/m5.0-app.md, section 7, *`sketch` (M5.10)*): a
//! sketch as the model a read names solved it, for sketch mode to draw and colour, and for an
//! agent, which reads the same thing (invariant 3). Its plane's frame in 3D; its state and
//! degrees of freedom; each member in its document form, with its solved values in place of
//! the stored ones and its definition state ([`crate::sketch_states`]); its profile's regions
//! (M3.5), or why it makes none; and why it wasn't solved, if it wasn't.
//!
//! A model keeps each sketch's solution, not the diagnosis its states are read from, so a
//! read solves the model's sketch again, from the coordinates the model's document holds, on
//! a sketch worker of the sessions' shared solvers, as the rebuild solved it. A solve is a
//! function of the sketch, so the read gives what the rebuild found: one solve a read (and,
//! as for any solve, the loads that check it, M3.4), none of the kernel.

use super::{ModelChoice, Problems, ReadCommand, Running, SessionId, Sessions};
use crate::document::{ElementId, FeatureId, Operation, Plane, Sketch};
use crate::kernel::Clients;
use crate::profile::{self, Profile};
use crate::recompute::{Model, SketchSolvers};
use crate::sketch::{Diagnosed, SketchSolver, SolveStopper, Translation};
use crate::sketch_states::{self, SketchStates};
use crate::{Error, ErrorCode};

impl<P: Clients + 'static> Sessions<P> {
    /// `sketch` (M5.10): the sketch `sketch` of the model `model` names (decision 33), solved
    /// again as the rebuild solved it, from the coordinates the model's document holds, on a
    /// solver of the sessions' own ([`SketchRead`]). Refused, naming `sketch`, for a feature
    /// the document doesn't have, or one that isn't a sketch, the document's sketches the
    /// candidates. Read as every model is ([`read_model`](Self::read_model)): the current one
    /// queued, waiting for the revision's rebuild, or the newest published at once, counted
    /// among the session's published reads. A cancel, or the session closing, stops its
    /// solve.
    pub fn sketch(
        &self,
        session: &SessionId,
        model: ModelChoice,
        sketch: FeatureId,
    ) -> Running<SketchRead> {
        let solvers = self.sketch_solvers();
        self.read_model(session, ReadCommand::Sketch, model, move |read| {
            let _span = tracing::info_span!(
                "sketch",
                feature = sketch.as_str(),
                model = read.model_read().of.as_str()
            )
            .entered();
            read_sketch(read.model(), &sketch, &solvers, |stopper, solve| {
                read.solving(stopper, solve)
            })
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

/// The sketch `id` of `model`, solved as the rebuild solved it, from the coordinates its
/// document holds, by a solver taken from `solvers` and given back: the solve run by
/// `solving`, given the solver's stopper for it, so a session's read stops it when the read
/// is cancelled, or its session closes.
///
/// Refused, naming `sketch`, for a feature the document doesn't have, or one that isn't a
/// sketch, the document's sketches the candidates. A solver that can't be had, fails, or
/// answers what can't be read, refuses the read with its error, which says nothing of the
/// sketch's state ([`sketch_states::states`]).
fn read_sketch(
    model: &Model,
    id: &FeatureId,
    solvers: &SketchSolvers,
    solving: impl FnOnce(
        Option<SolveStopper>,
        &mut dyn FnMut() -> Result<Diagnosed, Error>,
    ) -> Result<Diagnosed, Error>,
) -> Result<SketchRead, Problems> {
    let sketch = sketch_of(model, id)?;
    let named = |e: Error| Problems::from(e.with_feature(id.as_str()));
    let mut solver = solvers.take().map_err(named)?;
    let solved = match solver.stopper() {
        Ok(stopper) => solving(stopper, &mut || diagnosed(sketch, solver.as_mut())),
        Err(e) => Err(e),
    };
    // Given back once its solve is over, and its stopper with it: a stop that comes later
    // stops nothing of the next solve's (`Read::solving`).
    solvers.give_back(solver);
    let diagnosed = solved.map_err(named)?;
    let mut states = sketch_states::states(sketch, &diagnosed).map_err(named)?;
    states.solve_problem = states
        .solve_problem
        .map(|problem| problem.with_feature(id.as_str()));
    let (members, regions, profile_problems) = match diagnosed.outcome() {
        Ok(solution) => {
            let members = solution.written(sketch);
            match profile::of_sketch(id, sketch, solution) {
                Ok(profile) => (members, regions(&profile), Vec::new()),
                Err(refused) => (members, Vec::new(), refused.errors().to_vec()),
            }
        }
        Err(_) => (sketch.clone(), Vec::new(), Vec::new()),
    };
    Ok(SketchRead {
        sketch: id.clone(),
        plane: sketch.plane,
        members,
        states,
        regions,
        profile_problems,
    })
}

/// The sketch `id` of `model`'s document, or why there's none: a feature the document
/// doesn't have, or one that isn't a sketch, each `invalid_input` naming `sketch`, the
/// document's sketches the candidates.
fn sketch_of<'m>(model: &'m Model, id: &FeatureId) -> Result<&'m Sketch, Problems> {
    let document = model.document();
    let refused = |message: String| {
        let sketches = document
            .features
            .iter()
            .filter(|f| matches!(f.operation, Operation::Sketch(_)))
            .map(|f| f.id.as_str().to_owned());
        Problems::from(
            Error::new(ErrorCode::InvalidInput, message)
                .with_parameter("sketch")
                .with_suggestion("name a sketch of the document, by its ID")
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

/// `sketch` solved by `solver` as the rebuild solves it (`crate::sketch::solve`), its
/// diagnosis and outcome paired: the answer's, or, for a sketch the translation refuses, the
/// diagnosis of its geometry beside that refusal ([`Diagnosed::untranslated`]). A solver that
/// fails, or a reply that can't be read, is the error.
fn diagnosed(sketch: &Sketch, solver: &mut dyn SketchSolver) -> Result<Diagnosed, Error> {
    if let Some(refused) = Diagnosed::untranslated(sketch, solver)? {
        return Ok(refused);
    }
    let answer = Translation::new(sketch)?.solve(solver)?;
    answer.diagnosed().cloned().map_err(Clone::clone)
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
