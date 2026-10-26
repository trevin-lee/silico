//! Elements found by name in a model's solids, as a request names them (`elements.0`): the
//! one lookup every command that reads elements by name shares (`distance` and `angle`,
//! M4.5; `render`'s highlights, M4.6), so each refuses a name alike. A name is looked for in
//! the solids the request reads (the feature `feature`'s, or, for none, every body, in
//! timeline order). Every reason one isn't found is said, once each: a name that isn't one,
//! or that names no element of those solids (`reference_broken`, with the names it most
//! likely meant, none taken for it), or names a kind the command doesn't take; an element
//! of a feature that didn't build, that feature's failure; a solid that couldn't be
//! described, why. An unstable name is taken: what's found is answered once and stored
//! nowhere.

use std::collections::BTreeMap;

use super::Problems;
use super::elements::built;
use crate::document::FeatureId;
use crate::kernel::{Description, ElementKind, ElementRef};
use crate::naming::{ElementMap, Maker, Name};
use crate::recompute::{Built, Model};
use crate::references::Candidates;
use crate::{Error, ErrorCode};

/// The kinds of element a command takes by name, and what it says of a name of another.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Kinds {
    /// Those it takes.
    pub(crate) taken: &'static [ElementKind],
    /// Why another isn't, after "names <a kind>: " (a measure's, after "names a solid: ",
    /// is `a measure is between faces, edges, and vertices`).
    pub(crate) why: &'static str,
    /// What to do instead.
    pub(crate) suggestion: &'static str,
}

/// An element found by its name.
pub(crate) struct Hit<'m> {
    /// The feature whose solid it's of.
    pub(crate) feature: &'m FeatureId,
    /// The element, in its solid's numbering.
    pub(crate) element: ElementRef,
    /// Its solid's element map.
    pub(crate) map: &'m ElementMap,
    /// Its solid's description: each element's geometry.
    pub(crate) description: &'m Description,
    /// Its solid's build, whose shape the kernel takes.
    pub(crate) built: &'m Built,
}

/// Names looked up in one request's solids, every refusal kept to say at the end
/// ([`finish`](Self::finish)).
pub(crate) struct Lookup<'m> {
    model: &'m Model,
    /// Whether the request named its `feature`: a solid that can't be read names it then.
    asked: bool,
    solids: Vec<&'m FeatureId>,
    /// Where names are looked for, as a refusal says it.
    place: String,
    kinds: Kinds,
    refused: Problems,
    /// Features whose failure is said already: once each.
    unbuilt: Vec<FeatureId>,
    /// Solids whose missing description is said already: once each.
    undescribed: Vec<&'m FeatureId>,
    /// Each kind's candidates, built once a request, the first time a name of it breaks.
    candidates: BTreeMap<ElementKind, Candidates>,
}

impl<'m> Lookup<'m> {
    /// A lookup in `model`'s solids that the request reads: the feature `feature`'s, or,
    /// for none, every body. A `feature` that has no solid is refused now, naming `feature`.
    pub(crate) fn new(model: &'m Model, feature: Option<&FeatureId>, kinds: Kinds) -> Self {
        let mut lookup = Lookup {
            model,
            asked: feature.is_some(),
            solids: Vec::new(),
            place: match feature {
                Some(id) => format!("{id}'s solid"),
                None => "the live solids".to_owned(),
            },
            kinds,
            refused: Problems::none(),
            unbuilt: Vec::new(),
            undescribed: Vec::new(),
            candidates: BTreeMap::new(),
        };
        lookup.solids = match feature {
            Some(id) => match built(model, id, true) {
                Ok(_) => model.feature(id).map(|f| &f.id).into_iter().collect(),
                Err(problems) => {
                    lookup.refused.absorb(problems);
                    // Said once: a name of its own elements adds nothing to it.
                    lookup.unbuilt.push(id.clone());
                    Vec::new()
                }
            },
            None => model.bodies().iter().collect(),
        };
        lookup
    }

    /// The element `text` names, the request's `parameter` (`elements.0`); or none, why
    /// kept to say.
    pub(crate) fn find(&mut self, parameter: &str, text: &str) -> Option<Hit<'m>> {
        let (id, map, element) = self.found(parameter, text)?;
        let model = self.model;
        let (Some(description), Ok(built)) = (model.description(id), built(model, id, false))
        else {
            // Said once a solid, naming the request's `feature` only if it named one.
            if !self.undescribed.contains(&id) {
                self.undescribed.push(id);
                match super::elements::solid(model, id, self.asked) {
                    Err(problems) => self.refused.absorb(problems),
                    Ok(_) => self.refused.push(
                        Error::new(
                            ErrorCode::Internal,
                            format!("{id}'s solid has entries but no description"),
                        )
                        .with_suggestion(crate::worker::SILICO_BUG),
                    ),
                }
            }
            return None;
        };
        Some(Hit {
            feature: id,
            element,
            map,
            description,
            built,
        })
    }

    /// The element `text` names, as [`find`](Self::find) finds it, and the feature whose
    /// solid it's of, whether or not that solid could be described: for a command that
    /// needs which element, not its geometry (`render`'s highlights, of a body drawn though
    /// its measure failed).
    pub(crate) fn find_element(
        &mut self,
        parameter: &str,
        text: &str,
    ) -> Option<(&'m FeatureId, ElementRef)> {
        self.found(parameter, text)
            .map(|(id, _, element)| (id, element))
    }

    /// The solid whose element `text` names, its map, and the element; or none, why kept to
    /// say.
    fn found(
        &mut self,
        parameter: &str,
        text: &str,
    ) -> Option<(&'m FeatureId, &'m ElementMap, ElementRef)> {
        let model = self.model;
        let makers = |id: &FeatureId| model.feature(id).map(|f| Maker::of(&f.operation));
        let broken = |why: String, candidates: Vec<String>| {
            let suggestion = if candidates.is_empty() {
                "name the element as the model now names it: `elements` lists every one"
            } else {
                "name the element as the model now names it: the candidates are the \
                 likeliest, none taken for it"
            };
            Error::new(
                ErrorCode::ReferenceBroken,
                format!("{parameter}, {}, {why}", crate::document::json::shown(text)),
            )
            .with_parameter(parameter)
            .with_element(crate::references::cut_short(text))
            .with_candidates(candidates)
            .with_suggestion(suggestion)
        };
        let name = match Name::parse(text, &makers) {
            Ok(name) => name,
            Err(error) => {
                self.refused
                    .push(broken(format!("isn't a name: {error}"), Vec::new()));
                return None;
            }
        };
        if !self.kinds.taken.contains(&name.kind()) {
            self.refused.push(
                Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "{parameter}, {}, names {}: {}",
                        crate::document::json::shown(text),
                        name.kind().a_word(),
                        self.kinds.why
                    ),
                )
                .with_parameter(parameter)
                .with_element(crate::references::cut_short(text))
                .with_suggestion(self.kinds.suggestion),
            );
            return None;
        }
        let hit = self.solids.iter().find_map(|id| {
            let map = model.element_map(id)?;
            map.element(&name).map(|element| (*id, map, element))
        });
        let Some((id, map, element)) = hit else {
            // An element of a feature that failed, was broken, or was blocked is in no
            // solid: that feature's own failure is why, said once (as `elements` and the
            // measures answer one: M4.1 section 7, M4.5), never a
            // name that finds nothing with another solid's elements offered for it.
            // (A sketch's elements are refused as they're parsed: no name of one gets here.)
            let maker = name.feature();
            if let Err(problems) = built(model, maker, false) {
                if !self.unbuilt.contains(maker) {
                    self.refused.absorb(problems);
                    self.unbuilt.push(maker.clone());
                }
                return None;
            }
            // The request's `feature` was refused already: its solid has nothing to look in.
            if self.solids.is_empty() && self.asked {
                return None;
            }
            let solids = &self.solids;
            let candidates = self
                .candidates
                .entry(name.kind())
                .or_insert_with(|| {
                    let maps: Vec<&ElementMap> = solids
                        .iter()
                        .filter_map(|id| model.element_map(id))
                        .collect();
                    Candidates::of(&maps, name.kind())
                })
                .of_name(&name);
            self.refused.push(broken(
                format!("names no element of {}", self.place),
                candidates,
            ));
            return None;
        };
        Some((id, map, element))
    }

    /// A refusal of the request's own, kept with the lookup's to say in its order.
    pub(crate) fn refuse(&mut self, error: Error) {
        self.refused.push(error);
    }

    /// Every refusal kept, in the order they came; none if every name was found.
    pub(crate) fn finish(self) -> Result<(), Problems> {
        self.refused.into_result()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Checked;
    use crate::fake::FakeKernel;
    use crate::recompute::{Recompute, RunOutcome};
    use std::sync::Arc;
    use std::time::Duration;

    const BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

    /// Render's kinds: a face or an edge, as M4.6 highlights them.
    const FACES_AND_EDGES: Kinds = Kinds {
        taken: &[ElementKind::Face, ElementKind::Edge],
        why: "a highlight is of a face or an edge",
        suggestion: "name a face or an edge of the solid",
    };

    fn model() -> Arc<Model> {
        let checked =
            Arc::new(Checked::new(crate::document::read(BOX.as_bytes()).unwrap()).unwrap());
        let engine = Recompute::new(FakeKernel::new(1).unwrap());
        match engine.start(checked).wait_timeout(Duration::from_secs(60)) {
            Ok(Ok(RunOutcome::Published(model))) => model,
            _ => panic!("not published"),
        }
    }

    #[test]
    fn a_kind_the_command_doesnt_take_is_refused_in_its_words_and_one_it_does_is_found() {
        // A command that takes faces and edges (render's highlights) is given a vertex: the
        // refusal names the parameter, the name, its kind, and the command's own reason and
        // advice, as a measure's says a solid. Its face is found, of the box's solid.
        let model = model();
        let mut lookup = Lookup::new(&model, None, FACES_AND_EDGES);
        assert!(lookup.find("highlight.0", "k7f2qx3m.box.Vertex1").is_none());
        let hit = lookup.find("highlight.1", "k7f2qx3m.box.Face6").unwrap();
        assert_eq!(hit.feature.as_str(), "k7f2qx3m");
        assert_eq!(hit.element.kind, ElementKind::Face);
        let refused = lookup.finish().unwrap_err();
        let error = &refused.errors()[0];
        assert_eq!(refused.total(), 1);
        assert_eq!(error.code(), ErrorCode::InvalidInput);
        assert_eq!(
            error.message(),
            r#"highlight.0, "k7f2qx3m.box.Vertex1", names a vertex: a highlight is of a face or an edge"#
        );
        assert_eq!(error.parameter(), Some("highlight.0"));
        assert_eq!(
            error.suggestion(),
            Some("name a face or an edge of the solid")
        );
    }

    #[test]
    fn an_error_the_command_refuses_with_is_said_at_the_finish_after_the_lookups_own() {
        // Render's: a name found in a solid it doesn't draw, refused by the command as it
        // reads the hit. Said with the lookup's own refusals, in the order they came.
        let model = model();
        let mut lookup = Lookup::new(&model, None, FACES_AND_EDGES);
        lookup.refuse(Error::new(ErrorCode::Internal, "the command's own"));
        let refused = lookup.finish().unwrap_err();
        assert_eq!(refused.total(), 1);
        assert_eq!(refused.errors()[0].message(), "the command's own");
        let mut lookup = Lookup::new(&model, None, FACES_AND_EDGES);
        assert!(lookup.find("highlight.0", "k7f2qx3m.box.Vertex1").is_none());
        lookup.refuse(Error::new(ErrorCode::Internal, "the command's own"));
        let refused = lookup.finish().unwrap_err();
        assert_eq!(refused.total(), 2);
        assert_eq!(refused.errors()[0].parameter(), Some("highlight.0"));
        assert_eq!(refused.errors()[1].code(), ErrorCode::Internal);
    }
}
