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

use std::collections::{BTreeMap, BTreeSet};

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

/// What a lookup of one name finds.
pub(crate) enum Found<'m> {
    /// The solid whose element it names, its map, the element, and the name as read.
    Hit(&'m FeatureId, &'m ElementMap, ElementRef, Name),
    /// Nothing: why kept to say; and, a selection's ([`Lookup::selection`]), the feature
    /// whose failure is why, if one is (its failure kept once, [`Lookup::failures`]).
    Refused(Option<FeatureId>),
    /// A selection's: a map it needs isn't named yet, so it isn't answered now; asked
    /// again once the model's names are, it is.
    Pending,
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
    /// Whether it reads a selection's names ([`Lookup::selection`]): each name's reasons
    /// its own, a failed feature's failure kept once in `failures`, and no map named on the
    /// caller's thread.
    selection: bool,
    /// A selection's failed features' failures, once each, by feature.
    failures: BTreeMap<FeatureId, Problems>,
    /// Features whose failure is said already: once each.
    unbuilt: BTreeSet<FeatureId>,
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
            selection: false,
            failures: BTreeMap::new(),
            unbuilt: BTreeSet::new(),
            undescribed: Vec::new(),
            candidates: BTreeMap::new(),
        };
        lookup.solids = match feature {
            Some(id) => match built(model, id, true) {
                Ok(_) => model.feature(id).map(|f| &f.id).into_iter().collect(),
                Err(problems) => {
                    lookup.refused.absorb(problems);
                    // Said once: a name of its own elements adds nothing to it.
                    lookup.unbuilt.insert(id.clone());
                    Vec::new()
                }
            },
            None => model.bodies().iter().collect(),
        };
        lookup
    }

    /// A selection's lookup (`crate::selection`), in every body of `model`: each name's
    /// reasons its own, taken ([`take`](Self::take)) as it's looked up; a name of a feature
    /// that didn't build refused at its own place, pointing to that feature's failure, kept
    /// once ([`failures`](Self::failures)); a name read as of the model, not of a place in
    /// a document (`Name::parse_in_model`); and no element map named on the caller's
    /// thread, which may be the UI's: a name that needs a map not named yet is
    /// [`Found::Pending`].
    pub(crate) fn selection(model: &'m Model, kinds: Kinds) -> Self {
        Lookup {
            selection: true,
            ..Lookup::new(model, None, kinds)
        }
    }

    /// A selection's lookup ([`selection`](Self::selection)) in the feature `feature`'s solid
    /// alone, not in every body: a name is found only if that solid's map holds it, named
    /// so far ([`Found::Pending`] till it is), and one it doesn't hold is refused as of
    /// that solid, with candidates from it. The caller sees that `feature` has a solid
    /// first: one that hasn't is refused once, as [`new`](Self::new) refuses it.
    pub(crate) fn selection_in(model: &'m Model, feature: &FeatureId, kinds: Kinds) -> Self {
        Lookup {
            selection: true,
            ..Lookup::new(model, Some(feature), kinds)
        }
    }

    /// The element `text` names, the request's `parameter` (`elements.0`); or none, why
    /// kept to say.
    pub(crate) fn find(&mut self, parameter: &str, text: &str) -> Option<Hit<'m>> {
        let Found::Hit(id, map, element, _) = self.found(parameter, text) else {
            return None;
        };
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
        match self.found(parameter, text) {
            Found::Hit(id, _, element, _) => Some((id, element)),
            Found::Refused(_) | Found::Pending => None,
        }
    }

    /// The solid whose element `text` names, its map, the element, and the name as read; or
    /// none, why kept to say; or, a selection's, not yet.
    pub(crate) fn found(&mut self, parameter: &str, text: &str) -> Found<'m> {
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
        let parsed = if self.selection {
            Name::parse_in_model(text, &makers)
        } else {
            Name::parse(text, &makers)
        };
        let name = match parsed {
            Ok(name) => name,
            Err(error) => {
                self.refused
                    .push(broken(format!("isn't a name: {error}"), Vec::new()));
                return Found::Refused(None);
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
            return Found::Refused(None);
        }
        let located = if self.selection {
            match self.located_now(&name) {
                Located::Hit(id, map, element) => Some((id, map, element)),
                Located::Nowhere => None,
                Located::Pending => return Found::Pending,
                Located::Unnamed(id) => {
                    self.refused.push(unnamed(parameter, text, id));
                    return Found::Refused(None);
                }
            }
        } else {
            self.located(&name)
        };
        let Some((id, map, element)) = located else {
            // An element of a feature that failed, was broken, or was blocked is in no
            // solid: that feature's own failure is why, said once (as `elements` and the
            // measures answer one: M4.1 section 7, M4.5), never a
            // name that finds nothing with another solid's elements offered for it.
            // (A sketch's elements are refused as they're parsed: no name of one gets here.)
            let maker = name.feature();
            if self.selection {
                // Each name's own reason, at its place, pointing to the failure, kept once.
                if let Err(problems) = built(model, maker, false) {
                    self.refused
                        .push(failed_at(parameter, text, maker, &problems));
                    self.failures.entry(maker.clone()).or_insert(problems);
                    return Found::Refused(Some(maker.clone()));
                }
            } else {
                // Its failure said already: not worded again for each name of it.
                if self.unbuilt.contains(maker) {
                    return Found::Refused(None);
                }
                if let Err(problems) = built(model, maker, false) {
                    self.refused.absorb(problems);
                    self.unbuilt.insert(maker.clone());
                    return Found::Refused(None);
                }
            }
            // The request's `feature` was refused already: its solid has nothing to look in.
            if self.solids.is_empty() && self.asked {
                return Found::Refused(None);
            }
            let kind = name.kind();
            if !self.candidates.contains_key(&kind) {
                // A selection's candidates wait for every body's names: offered from some
                // bodies now and all of them later, they'd differ from call to call.
                let maps: Option<Vec<&ElementMap>> = if self.selection {
                    self.solids
                        .iter()
                        .map(|id| model.named_so_far(id))
                        .collect::<Option<Vec<_>>>()
                        .map(|maps| maps.into_iter().flatten().collect())
                } else {
                    Some(
                        self.solids
                            .iter()
                            .filter_map(|id| model.element_map(id))
                            .collect(),
                    )
                };
                let Some(maps) = maps else {
                    return Found::Pending;
                };
                self.candidates.insert(kind, Candidates::of(&maps, kind));
            }
            let candidates = self
                .candidates
                .get(&kind)
                .map(|c| c.of_name(&name))
                .unwrap_or_default();
            self.refused.push(broken(
                format!("names no element of {}", self.place),
                candidates,
            ));
            return Found::Refused(None);
        };
        Found::Hit(id, map, element, name)
    }

    /// As [`located`](Self::located) finds it, in the request's `feature`'s solid or in
    /// every body, but naming no map: a selection's, whose caller may be the UI's thread.
    fn located_now(&self, name: &Name) -> Located<'m> {
        let model = self.model;
        let solid = if self.asked {
            self.solids.first().copied()
        } else {
            model.live_body(name.feature())
        };
        let Some(id) = solid else {
            return Located::Nowhere;
        };
        match model.named_so_far(id) {
            None => Located::Pending,
            Some(None) => Located::Unnamed(id),
            Some(Some(map)) => match map.element(name) {
                Some(element) => Located::Hit(id, map, element),
                None => Located::Nowhere,
            },
        }
    }

    /// The solid `name`'s element is in, its map, and the element, if the request's solids
    /// have it. In the request's `feature`'s solid, its map read. In every body, its
    /// feature's live body's map alone ([`Model::live_body`]), one read, or none for a
    /// feature with no live body: every name in a body's map is of a feature whose live
    /// body that is, so a name not in its feature's live body's map is in no body's, and a
    /// name of a feature with no live body (one that didn't build, a sketch, or one the
    /// document hasn't, as [`Model::live_body`] says) is in none. What's said of a name
    /// found nowhere (its feature's failure, or `reference_broken` with candidates from
    /// every body) is `found`'s, never this read's.
    fn located(&self, name: &Name) -> Option<(&'m FeatureId, &'m ElementMap, ElementRef)> {
        let model = self.model;
        let in_solid = |id: &'m FeatureId| {
            let map = model.element_map(id)?;
            map.element(name).map(|element| (id, map, element))
        };
        if self.asked {
            self.solids.iter().find_map(|id| in_solid(id))
        } else {
            model.live_body(name.feature()).and_then(in_solid)
        }
    }

    /// A refusal of the request's own, kept with the lookup's to say in its order.
    pub(crate) fn refuse(&mut self, error: Error) {
        self.refused.push(error);
    }

    /// A selection's failed features' failures, once each, by feature.
    pub(crate) fn failures(self) -> BTreeMap<FeatureId, Problems> {
        self.failures
    }

    /// The refusals kept so far, in the order they came, taken: the lookup's since the last
    /// take, so a selection's lookup ([`selection`](Self::selection)) gives each name its
    /// reasons.
    pub(crate) fn take(&mut self) -> Problems {
        std::mem::replace(&mut self.refused, Problems::none())
    }

    /// Every refusal kept, in the order they came; none if every name was found.
    pub(crate) fn finish(self) -> Result<(), Problems> {
        self.refused.into_result()
    }
}

/// Where a selection's name is, as named so far.
enum Located<'m> {
    Hit(&'m FeatureId, &'m ElementMap, ElementRef),
    /// In no body.
    Nowhere,
    /// Its live body's map isn't named yet.
    Pending,
    /// Its live body built, but its elements couldn't be named.
    Unnamed(&'m FeatureId),
}

/// A selection's refusal of `text`, at `parameter`, a name of an element of `feature`, which
/// didn't build (`problems`, its failure, kept once for the selection): the failure's
/// code, and its first words, naming the feature.
fn failed_at(parameter: &str, text: &str, feature: &FeatureId, problems: &Problems) -> Error {
    let (code, first) = match problems.errors().first() {
        Some(error) => (error.code(), error.message()),
        None => (ErrorCode::Internal, "no reason was kept"),
    };
    let more = match problems.total() {
        0 | 1 => String::new(),
        total => format!(" (the first of {total} problems)"),
    };
    Error::new(
        code,
        format!(
            "{parameter}, {}, is of {feature}, which didn't build: {first}{more}",
            crate::document::json::shown(text)
        ),
    )
    .with_feature(feature.as_str())
    .with_parameter(parameter)
    .with_element(crate::references::cut_short(text))
    .with_suggestion(format!(
        "mend {feature} first: its failure says why, and its elements are in no solid until \
         it builds"
    ))
}

/// A selection's refusal of `text`, at `parameter`, a name whose body, `feature`, built but
/// whose elements couldn't be named: a fault in Silico, not the name's.
fn unnamed(parameter: &str, text: &str, feature: &FeatureId) -> Error {
    Error::new(
        ErrorCode::Internal,
        format!(
            "{parameter}, {}, is of {feature}'s solid, which built, but whose elements couldn't \
             be named",
            crate::document::json::shown(text)
        ),
    )
    .with_feature(feature.as_str())
    .with_parameter(parameter)
    .with_element(crate::references::cut_short(text))
    .with_suggestion(crate::worker::SILICO_BUG)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Checked;
    use crate::fake::FakeKernel;
    use crate::recompute::{FeatureOutcome, Recompute, RunOutcome};
    use std::collections::{BTreeMap, BTreeSet};
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
        published(Checked::new(crate::document::read(BOX.as_bytes()).unwrap()).unwrap())
    }

    fn published(checked: Checked) -> Arc<Model> {
        published_refusing(checked, |_| None)
    }

    /// `checked` rebuilt on a fake kernel that refuses each request `refuses` gives an
    /// error for.
    fn published_refusing(
        checked: Checked,
        refuses: impl Fn(&silico_protocol::Op) -> Option<Error> + Send + Sync + 'static,
    ) -> Arc<Model> {
        let engine = Recompute::new(FakeKernel::new(2).unwrap())
            .with_sketches(crate::fake::FakeSketches::new());
        engine.kernel().refuse(refuses);
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
        sized_box(id, x, 1)
    }

    fn sized_box(id: &str, x: usize, length: u32) -> String {
        format!(
            r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Block", "position": [{x}, 0, 0], "length": {length}, "width": 1, "height": 1}}"#
        )
    }

    fn operation(id: &str, kind: &str, base: &str, tool: &str) -> String {
        format!(
            r#"{{"id": "{id}", "type": "{kind}", "v": 1, "name": "Op", "base": "{base}", "tool": "{tool}"}}"#
        )
    }

    /// Whether `op` makes a box `length` long.
    fn a_box_of(op: &silico_protocol::Op, length: f64) -> bool {
        matches!(
            op,
            silico_protocol::Op::MakePrimitive(silico_protocol::Primitive::Box { dx, .. })
                if *dx == length
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
        //
        // And misses, which the scan, kept for them after the live body's read, made B
        // reads each (#193's review: none of them could find anything). Beside the bodies,
        // a box the kernel refuses (2 mm long), so no body: B names of it read no map (it
        // has no live body), and look up 2 B + 1 IDs: the parse's and the live body's for
        // each, and its failure's once (`built`), said once, not looked up again for each.
        // The scan read B maps each, B²; and the failure looked up again each, 3 B. Then a
        // seventh face of each box, which a box hasn't: one read each, of its own body, and
        // the face candidates from every body's map once, at the first, B more: 2 B. Each
        // name's lookups are its parse's, its live body's, its map read's, and its
        // feature's outcome (built, so not refused for its failure): 4 B, and the
        // candidates' B map reads' B: 5 B. The scan read B + 1 maps a name, B (B + 2) in
        // all. The misses at B = 1,000 only: their bounds are exact, and the scan's B² reads
        // of them fail them as surely at 1,000 as at 4,000, at a sixteenth of the time.
        for b in [1_000, 4_000] {
            let ids: Vec<String> = (0..b).map(|i| format!("b{i:07}")).collect();
            let mut features: Vec<String> =
                ids.iter().enumerate().map(|(i, id)| a_box(id, i)).collect();
            features.push(sized_box("x0000000", b, 2));
            let model = published_refusing(checked(&features), |op| {
                a_box_of(op, 2.0).then(|| Error::new(ErrorCode::KernelFailed, "MakeBox failed"))
            });
            assert_eq!(model.bodies().len(), b);
            let mut lookup = Lookup::new(&model, None, FACES_AND_EDGES);
            let b = b as u64;
            let (walked, looked_up) = counted();
            for (i, id) in ids.iter().enumerate() {
                let (found, element) = lookup
                    .find_element(&format!("highlight.{i}"), &format!("{id}.box.Face3"))
                    .unwrap();
                assert_eq!(found.as_str(), id);
                assert_eq!(element.kind, ElementKind::Face);
            }
            let (walked, looked_up) = (counted().0 - walked, counted().1 - looked_up);
            assert_eq!(walked, b, "{walked} map reads for {b} names");
            assert_eq!(looked_up, 3 * b, "{looked_up} lookups for {b} names");
            if b > 1_000 {
                assert!(lookup.finish().is_ok());
                continue;
            }
            let (walked, looked_up) = counted();
            for i in 0..b {
                let text = format!("x0000000.box.Face{}", i % 6 + 1);
                assert!(lookup.find_element("highlight.x", &text).is_none());
            }
            let (walked, looked_up) = (counted().0 - walked, counted().1 - looked_up);
            assert_eq!(walked, 0, "{walked} map reads for {b} names of no body");
            assert_eq!(
                looked_up,
                2 * b + 1,
                "{looked_up} lookups for {b} names of no body"
            );
            let (walked, looked_up) = counted();
            for id in &ids {
                let text = format!("{id}.box.Face7");
                assert!(lookup.find_element("highlight.y", &text).is_none());
            }
            let (walked, looked_up) = (counted().0 - walked, counted().1 - looked_up);
            assert_eq!(walked, 2 * b, "{walked} map reads for {b} broken names");
            assert_eq!(looked_up, 5 * b, "{looked_up} lookups for {b} broken names");
            // The refused box's failure, once, and each broken name.
            let refused = lookup.finish().unwrap_err();
            assert_eq!(refused.total(), 1 + b);
            assert_eq!(refused.errors()[0].feature(), Some("x0000000"));
            assert_eq!(refused.errors()[0].code(), ErrorCode::KernelFailed);
            assert!(
                refused.errors()[1..]
                    .iter()
                    .all(|error| error.code() == ErrorCode::ReferenceBroken)
            );
        }
    }

    #[test]
    fn each_features_live_body_is_found_by_one_visit_of_it_and_one_of_each_solid_it_uses() {
        // `Model::live_body`'s table, built once a model: each feature with a live body
        // visited once, and each solid it uses once (`live_visits`), N + Σ uses at most. B
        // boxes, and a fuse of each two, in that order: N = 3 B / 2 features, every one
        // with a live body (a fuse's its own, a box's its fuse), and Σ uses = B: 5 B / 2
        // visits, 2,500 for B = 1,000. Finding each feature's consumer by a scan forward
        // from it visits each feature after a box up to its fuse, with each fuse's two
        // uses: B + 2 + i for box 2i, B + 1 + i for box 2i + 1, 5 B² / 4 + B in all:
        // 1,251,000. Built once: asked again, no visit.
        //
        // And the first pass, which `live_visits` doesn't count: the timeline walked beside
        // the bodies, each feature's ID compared with the next body's while one is left (N
        // comparisons: the last body, w0000499, is the last feature, so one's left at every
        // feature), and none in the second pass, which goes by place. With the lookup of
        // `b0000000`'s place in the model's index (at most ⌈log₂ N⌉ + 1 = 12 comparisons:
        // 1,500 is between 2¹⁰ and 2¹¹), the first `live_body` compares at most N + 12 =
        // 1,512 IDs (`document::compared`). Asking each feature whether it's a body, by a
        // scan of the bodies, compares B / 2 for each box (none is one) and i + 1 for fuse
        // i: 500,000 + 125,250.
        let b = 1_000;
        let mut features: Vec<String> = (0..b).map(|i| a_box(&format!("b{i:07}"), 3 * i)).collect();
        for i in 0..b / 2 {
            let (base, tool) = (format!("b{:07}", 2 * i), format!("b{:07}", 2 * i + 1));
            features.push(operation(&format!("w{i:07}"), "fuse", &base, &tool));
        }
        let model = published(checked(&features));
        assert!(model.errors().next().is_none());
        assert_eq!(model.bodies().len(), b / 2);
        let first = FeatureId::new("b0000000").unwrap();
        let (before, compared) = (crate::recompute::live_visits(), crate::document::compared());
        assert_eq!(
            model.live_body(&first).map(FeatureId::as_str),
            Some("w0000000")
        );
        assert_eq!(crate::recompute::live_visits() - before, 5 * b as u64 / 2);
        let compared = crate::document::compared() - compared;
        assert!(
            compared <= 1_512,
            "{compared} IDs compared for 1,500 features"
        );
        let before = crate::recompute::live_visits();
        for i in 0..b {
            let id = FeatureId::new(&format!("b{i:07}")).unwrap();
            let fuse = format!("w{:07}", i / 2);
            assert_eq!(
                model.live_body(&id).map(FeatureId::as_str),
                Some(fuse.as_str())
            );
        }
        for i in 0..b / 2 {
            let id = FeatureId::new(&format!("w{i:07}")).unwrap();
            assert_eq!(model.live_body(&id), Some(&id));
        }
        assert_eq!(crate::recompute::live_visits(), before);
    }

    #[test]
    fn a_name_is_found_in_the_body_the_scan_of_every_body_finds_it_in_or_in_none() {
        // The live body is the scan's first match, whatever the name: every name of every
        // built feature's map (bodies', and those of features consumed, some of which no
        // body keeps), found by `located` and by the scan of every body in timeline order
        // that it replaced, in a model of several bodies: two pads of one sketch, a fuse of
        // two boxes filleted twice, a pad on a box, a cut of a pad, a box alone, a fillet
        // of it the kernel refuses, and a fillet of that fillet, blocked. And a sketch's
        // own names, which no body holds, names of the failed and blocked fillets, a face a
        // box hasn't, and a name of a feature that isn't one.
        const SKETCH: &str = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Outline", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 4, "y": 0, "construction": false}, {"id": "pa000003", "x": 4, "y": 2, "construction": false}, {"id": "pa000004", "x": 0, "y": 2, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": []}"#;
        let pad = |id: &str, base: &str, length: u32| {
            format!(
                r#"{{"id": "{id}", "type": "pad", "v": 1, "name": "Pad", {base}"sketch": "s0000001", "extent": {{"type": "one_side", "length": {length}, "reversed": false}}}}"#
            )
        };
        let fillet = |id: &str, base: &str, edge: &str, radius: f64| {
            format!(
                r#"{{"id": "{id}", "type": "fillet", "v": 1, "name": "Round", "base": "{base}", "edges": ["{edge}"], "radius": {radius}}}"#
            )
        };
        let features = vec![
            SKETCH.to_owned(),
            pad("p0000001", "", 10),
            pad("p0000002", "", 5),
            a_box("a0000001", 0),
            a_box("a0000002", 20),
            operation("v0000001", "fuse", "a0000001", "a0000002"),
            fillet("f0000001", "v0000001", "a0000001.box.Edge1", 0.1),
            fillet("f0000002", "f0000001", "a0000002.box.Edge2", 0.1),
            a_box("c0000001", 40),
            pad("p0000003", r#""base": "c0000001", "#, 3),
            // Inside the pad's corner, so the cut removes some of it (S7's rule refuses one
            // that removes nothing).
            a_box("d0000001", 0),
            operation("k0000001", "cut", "p0000001", "d0000001"),
            a_box("e0000001", 80),
            fillet("g0000001", "e0000001", "e0000001.box.Edge1", 0.25),
            fillet("h0000001", "g0000001", "e0000001.box.Edge2", 0.1),
        ];
        let model = published_refusing(checked(&features), |op| {
            matches!(op, silico_protocol::Op::Fillet { radius, .. } if *radius == 0.25)
                .then(|| Error::new(ErrorCode::KernelFailed, "the fillet failed"))
        });
        let id = |text: &str| FeatureId::new(text).unwrap();
        assert!(matches!(
            model.outcome(&id("g0000001")),
            Some(FeatureOutcome::Failed(_))
        ));
        assert!(matches!(
            model.outcome(&id("h0000001")),
            Some(FeatureOutcome::Blocked { on }) if on.as_str() == "g0000001"
        ));
        let bodies: Vec<&str> = model.bodies().iter().map(FeatureId::as_str).collect();
        // The failed fillet consumes nothing, so the box stays a body.
        assert_eq!(
            bodies,
            ["p0000002", "f0000002", "p0000003", "k0000001", "e0000001"]
        );
        // Each feature's live body, by hand: a body its own; a pad's base, the fuse's
        // operands and the fillets of it, and the cut's base and tool, the body at the end
        // of their chain; none for the sketch, the failed fillet, and the blocked one.
        let live: BTreeMap<&str, Option<&str>> = [
            ("s0000001", None),
            ("p0000001", Some("k0000001")),
            ("p0000002", Some("p0000002")),
            ("a0000001", Some("f0000002")),
            ("a0000002", Some("f0000002")),
            ("v0000001", Some("f0000002")),
            ("f0000001", Some("f0000002")),
            ("f0000002", Some("f0000002")),
            ("c0000001", Some("p0000003")),
            ("p0000003", Some("p0000003")),
            ("d0000001", Some("k0000001")),
            ("k0000001", Some("k0000001")),
            ("e0000001", Some("e0000001")),
            ("g0000001", None),
            ("h0000001", None),
        ]
        .into_iter()
        .collect();
        assert_eq!(live.len(), model.features().len());
        for (feature, _) in model.features() {
            assert_eq!(
                model.live_body(feature).map(FeatureId::as_str),
                live[feature.as_str()],
                "{feature}"
            );
        }
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
            // One map read, its feature's live body's, found or not (even for a name of a
            // feature three consumers back from its body: a box under a fuse and two
            // fillets); none for a feature with no live body.
            let expected = live
                .get(name.feature().as_str())
                .copied()
                .flatten()
                .map_or(0, |_| 1);
            assert_eq!(read, expected, "{name}");
            let scan = scanned(name);
            assert_eq!(
                found.map(|(id, map, element)| (id, map as *const ElementMap, element)),
                scan.map(|(id, map, element)| (id, map as *const ElementMap, element)),
                "{name}"
            );
            found.map(|(id, _, _)| (name.feature().as_str().to_owned(), id.as_str()))
        };
        let (mut found, mut not) = (BTreeSet::new(), BTreeMap::new());
        let mut found_count = 0;
        for (id, _) in model.features() {
            let Some(map) = model.element_map(id) else {
                continue;
            };
            for (_, names) in map.iter() {
                for name in names {
                    match same(name) {
                        Some(pair) => {
                            found.insert(pair);
                            found_count += 1;
                        }
                        None => *not.entry(name.feature().as_str().to_owned()).or_insert(0) += 1,
                    }
                }
            }
        }
        // Every name of every map, one each element: a box's 26 (6 faces, 12 edges, 8
        // vertices), and a pad's of a rectangle (26, as `sweep_names` counts them), seven
        // of them (four boxes and two pads alone, and the cut's tool); 52 for the fuse of
        // two boxes, each fillet of it (the fake's keeps every element), the pad on a box,
        // and the cut, which keep both inputs' elements: 7 × 26 + 5 × 52 = 442. Found, all
        // but the cut's tool's: what of the tool a cut keeps is its own piece (`I`), so the
        // tool's 26 names are in no body. 416 found.
        assert_eq!(found_count, 416);
        assert_eq!(not, BTreeMap::from([("d0000001".to_owned(), 26)]));
        // Names found in each body, of features down each chain.
        let pair = |feature: &str, body: &'static str| (feature.to_owned(), body);
        assert_eq!(
            found,
            BTreeSet::from([
                pair("a0000001", "f0000002"),
                pair("a0000002", "f0000002"),
                pair("c0000001", "p0000003"),
                pair("p0000003", "p0000003"),
                pair("p0000001", "k0000001"),
                pair("k0000001", "k0000001"),
                pair("p0000002", "p0000002"),
                pair("e0000001", "e0000001"),
            ])
        );
        // A sketch's names: in no body, and found in none.
        for (kind, element) in [
            (ElementKind::Edge, "na000001"),
            (ElementKind::Vertex, "pa000001"),
        ] {
            let name = Name::Sketch {
                feature: id("s0000001"),
                kind,
                id: crate::document::ElementId::new(element).unwrap(),
            };
            assert_eq!(same(&name), None);
        }
        // Names of the failed and the blocked fillet, of a feature the document doesn't
        // have, and a seventh face of the box alone, which has six.
        let named = |feature: &str, made, element| Name::Element {
            feature: id(feature),
            made,
            element,
        };
        let fillet_word = crate::naming::TypeWord::Operation(crate::naming::OpCode::Fillet);
        let box_word = crate::naming::TypeWord::Primitive(crate::naming::PrimitiveKind::Box);
        for name in [
            named("g0000001", fillet_word, ElementRef::face(1)),
            named("h0000001", fillet_word, ElementRef::face(1)),
            named("z0000001", box_word, ElementRef::face(1)),
            named("e0000001", box_word, ElementRef::face(7)),
        ] {
            assert_eq!(same(&name), None, "{name}");
        }
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
