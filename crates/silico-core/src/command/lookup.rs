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
use crate::naming::{ElementMap, Name};
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
        let makers = |id: &FeatureId| model.maker(id);
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
        let Some((id, map, element)) = self.located(&name) else {
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

    /// The solid `name`'s element is in, its map, and the element, if the request's solids
    /// have it: the first in timeline order that does. Looked for in every body, it can be
    /// in one only, its feature's live body ([`Model::live_body`]), so it's looked for there,
    /// one map read, not one a body; and, not found there, in each, as before, so what's
    /// said of a name found nowhere doesn't change.
    fn located(&self, name: &Name) -> Option<(&'m FeatureId, &'m ElementMap, ElementRef)> {
        let model = self.model;
        let in_solid = |id: &'m FeatureId| {
            let map = model.element_map(id)?;
            map.element(name).map(|element| (id, map, element))
        };
        let live = if self.asked {
            None
        } else {
            model.live_body(name.feature())
        };
        live.and_then(in_solid)
            .or_else(|| self.solids.iter().find_map(|id| in_solid(id)))
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
    use std::collections::BTreeSet;
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

    fn published(checked: Checked) -> Arc<Model> {
        let engine = Recompute::new(FakeKernel::new(2).unwrap())
            .with_sketches(crate::fake::FakeSketches::new());
        match engine
            .start(Arc::new(checked))
            .wait_timeout(Duration::from_secs(120))
        {
            Ok(Ok(RunOutcome::Published(model))) => model,
            _ => panic!("not published"),
        }
    }

    fn checked(features: &[String]) -> Checked {
        let text = format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
            features.join(", ")
        );
        Checked::new(crate::document::read(text.as_bytes()).unwrap()).unwrap()
    }

    fn a_box(id: &str, x: usize) -> String {
        format!(
            r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Block", "position": [{x}, 0, 0], "length": 1, "width": 1, "height": 1}}"#
        )
    }

    /// This thread's map reads (features visited by `Model::element_map`) and lookups in
    /// indexes so far.
    fn counted() -> (u64, u64) {
        (crate::recompute::walked(), crate::document::looked_up())
    }

    #[test]
    fn a_name_in_every_body_reads_one_map_not_one_a_body_before_its_own() {
        // #187's architecture review: a name looked for in every body read each body's map
        // in timeline order until one had it, so B names, one of each of B bodies, read
        // 1 + 2 + ... + B = B (B + 1) / 2 maps: 500,500 for 1,000 bodies, 8,002,000 for
        // 4,000. Its element can be in no body but its feature's live body, so that's the
        // one read. B boxes, each its own body, and a face of each named: B map reads (a
        // box is made of nothing, so a read visits it alone, named yet or not). And 3 B
        // lookups by ID: one as the name is parsed (what its feature is), one for its
        // feature's live body, and one as that body's map is read. Each body's place is found
        // as the timeline is walked once, with no lookup. The scan's were 1 + (i + 1) for the
        // ith name, B + B (B + 1) / 2 in all. Counted, not timed, where maps are read and
        // IDs looked up, so the scan put back is counted as the live body's read is.
        for b in [1_000, 4_000] {
            let ids: Vec<String> = (0..b).map(|i| format!("b{i:07}")).collect();
            let features: Vec<String> =
                ids.iter().enumerate().map(|(i, id)| a_box(id, i)).collect();
            let model = published(checked(&features));
            assert_eq!(model.bodies().len(), b);
            let mut lookup = Lookup::new(&model, None, FACES_AND_EDGES);
            let (walked, looked_up) = counted();
            for (i, id) in ids.iter().enumerate() {
                let (found, element) = lookup
                    .find_element(&format!("highlight.{i}"), &format!("{id}.box.Face3"))
                    .unwrap();
                assert_eq!(found.as_str(), id);
                assert_eq!(element.kind, ElementKind::Face);
            }
            let (walked, looked_up) = (counted().0 - walked, counted().1 - looked_up);
            let b = b as u64;
            assert_eq!(walked, b, "{walked} map reads for {b} names");
            assert_eq!(looked_up, 3 * b, "{looked_up} lookups for {b} names");
            assert!(lookup.finish().is_ok());
        }
    }

    #[test]
    fn a_name_is_found_in_the_body_the_scan_of_every_body_finds_it_in_or_in_none() {
        // The live body is the scan's first match, whatever the name: every name of every
        // built feature's map (bodies', and those of features consumed, some of which no
        // body keeps), found by `located` and by the scan of every body in timeline order
        // that it replaced, in a model of several bodies: two pads of one sketch, a fuse of
        // two boxes filleted twice, a pad on a box, a cut of a pad, and a box alone. And a
        // sketch's own names, which no body holds, and a name of a feature that isn't one.
        const SKETCH: &str = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Outline", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 4, "y": 0, "construction": false}, {"id": "pa000003", "x": 4, "y": 2, "construction": false}, {"id": "pa000004", "x": 0, "y": 2, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#;
        let pad = |id: &str, base: &str, length: u32| {
            format!(
                r#"{{"id": "{id}", "type": "pad", "v": 1, "name": "Pad", {base}"sketch": "s0000001", "extent": {{"type": "one_side", "length": {length}, "reversed": false}}}}"#
            )
        };
        let operation = |id: &str, kind: &str, base: &str, tool: &str| {
            format!(
                r#"{{"id": "{id}", "type": "{kind}", "v": 1, "name": "Op", "base": "{base}", "tool": "{tool}"}}"#
            )
        };
        let fillet = |id: &str, base: &str, edge: &str| {
            format!(
                r#"{{"id": "{id}", "type": "fillet", "v": 1, "name": "Round", "base": "{base}", "edges": ["{edge}"], "radius": 0.1}}"#
            )
        };
        let features = vec![
            SKETCH.to_owned(),
            pad("p0000001", "", 10),
            pad("p0000002", "", 5),
            a_box("a0000001", 0),
            a_box("a0000002", 20),
            operation("v0000001", "fuse", "a0000001", "a0000002"),
            fillet("f0000001", "v0000001", "a0000001.box.Edge1"),
            fillet("f0000002", "f0000001", "a0000002.box.Edge2"),
            a_box("c0000001", 40),
            pad("p0000003", r#""base": "c0000001", "#, 3),
            a_box("d0000001", 60),
            operation("k0000001", "cut", "p0000001", "d0000001"),
            a_box("e0000001", 80),
        ];
        let model = published(checked(&features));
        assert!(
            model.errors().next().is_none(),
            "{:?}",
            model.errors().collect::<Vec<_>>()
        );
        let bodies: Vec<&str> = model.bodies().iter().map(FeatureId::as_str).collect();
        assert_eq!(
            bodies,
            ["p0000002", "f0000002", "p0000003", "k0000001", "e0000001"]
        );
        let lookup = Lookup::new(&model, None, FACES_AND_EDGES);
        // The scan of every body it replaced, as it was.
        let scanned = |name: &Name| {
            lookup.solids.iter().find_map(|id| {
                let map = model.element_map(id)?;
                map.element(name).map(|element| (*id, map, element))
            })
        };
        // Every map named first, so a read of one visits it alone.
        for (id, _) in model.features() {
            model.element_map(id);
        }
        let same = |name: &Name| {
            let before = counted().0;
            let found = lookup.located(name);
            let read = counted().0 - before;
            // Found, it was found in the first map read: its live body's, even for a name
            // of a feature three consumers back from its body (a box under a fuse and two
            // fillets).
            if found.is_some() {
                assert_eq!(read, 1, "{name}");
            }
            let scan = scanned(name);
            assert_eq!(
                found.map(|(id, map, element)| (id, map as *const ElementMap, element)),
                scan.map(|(id, map, element)| (id, map as *const ElementMap, element)),
                "{name}"
            );
            found.map(|(id, _, _)| (name.feature().as_str().to_owned(), id.as_str()))
        };
        let (mut found, mut not) = (BTreeSet::new(), 0);
        for (id, _) in model.features() {
            let Some(map) = model.element_map(id) else {
                continue;
            };
            for (_, names) in map.iter() {
                for name in names {
                    match same(name) {
                        Some(pair) => {
                            found.insert(pair);
                        }
                        None => not += 1,
                    }
                }
            }
        }
        // Names found in each body, of features down each chain: each box's three
        // consumers back (the fake's fillets name nothing of their own), a pad's base, the
        // cut's base; and of the consumed features' elements, some found nowhere.
        let pair = |feature: &str, body: &'static str| (feature.to_owned(), body);
        for expected in [
            pair("a0000001", "f0000002"),
            pair("a0000002", "f0000002"),
            pair("c0000001", "p0000003"),
            pair("p0000003", "p0000003"),
            pair("p0000001", "k0000001"),
            pair("k0000001", "k0000001"),
            pair("p0000002", "p0000002"),
            pair("e0000001", "e0000001"),
        ] {
            assert!(found.contains(&expected), "{expected:?} in {found:?}");
        }
        assert!(not > 0);
        // A sketch's names: in no body, and found in none.
        let sketch = FeatureId::new("s0000001").unwrap();
        for (kind, element) in [
            (ElementKind::Edge, "na000001"),
            (ElementKind::Vertex, "pa000001"),
        ] {
            let name = Name::Sketch {
                feature: sketch.clone(),
                kind,
                id: crate::document::ElementId::new(element).unwrap(),
            };
            assert_eq!(same(&name), None);
        }
        // A name of a feature the document doesn't have.
        let nowhere = Name::Element {
            feature: FeatureId::new("z0000001").unwrap(),
            made: crate::naming::TypeWord::Primitive(crate::naming::PrimitiveKind::Box),
            element: ElementRef::face(1),
        };
        assert_eq!(same(&nowhere), None);
        // Each body's own faces and edges, as text, found in it (vertices aren't
        // highlights).
        let mut lookup = Lookup::new(&model, None, FACES_AND_EDGES);
        for body in model.bodies() {
            let map = model.element_map(body).unwrap();
            for (element, names) in map.iter() {
                if element.kind == ElementKind::Vertex {
                    continue;
                }
                let text = names[0].to_string();
                assert_eq!(
                    lookup.find_element("highlight.0", &text),
                    Some((body, element)),
                    "{text}"
                );
            }
        }
        assert!(lookup.finish().is_ok());
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
