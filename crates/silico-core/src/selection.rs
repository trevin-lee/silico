//! A selection's rules that are the model's (M5.7, docs/design/m5.0-app.md section 9's
//! *Selection*): a selection is a list of element names, each with its stability, and this
//! reads one against a model. Each name is found in the model's bodies as every command
//! that reads elements by name finds it (`command::lookup`), and, found, said stable or
//! not as `elements` says it (`elements::stable_name`: its own name or an alias stable),
//! and held by a stable name if it has one. One with none is held when picked
//! ([`resolve`]), but what a store would say of it is said with it, in the words a fillet's
//! or chamfer's reference refuses it with (`references`, N3; M4.1 section 2). A name not
//! found is gone, with every reason the lookup gives: a name that isn't one, one of no live
//! solid's element, or one of a feature that didn't build (pointing to that feature's
//! failure, kept once). A name whose body isn't named yet is pending: nothing here waits.
//!
//! After a rebuild, the selection is carried to the newly published model ([`refresh`]):
//! what's kept, each with its stability now, and what's gone, with a word, for the app to
//! clear and say (M5.7's *After a rebuild*); a name with no stable form is gone then, since
//! its text can outlive its element. Which names are selected, and how (clicks, Ctrl, a box,
//! filters, Esc), is the app's own state, not this.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::command::Problems;
use crate::command::lookup::{Found, Kinds, Lookup};
use crate::document::FeatureId;
use crate::elements::stable_name;
use crate::kernel::{ElementKind, ElementRef};
use crate::naming::Name;
use crate::recompute::Model;
use crate::references::{Candidates, refused, unstable};
use crate::{Error, ErrorCode};

/// The kinds a selection holds by name, and what it says of a name of another (a solid's).
const SELECTED: Kinds = Kinds {
    taken: &[ElementKind::Face, ElementKind::Edge, ElementKind::Vertex],
    why: "a selection is of faces, edges, and vertices",
    suggestion: "select a face, an edge, or a vertex of a body",
};

/// The most names one selection holds: ten times the most a render highlights, room for a
/// box selection across a large part. Each name's answer costs what its own text does, and
/// a failed feature's failure is kept once, so a selection this long answers in bounded
/// time and memory. A box selection over a large part can take more: the app clips it to
/// this, or warns, before calling, rather than lose the whole selection to the refusal.
pub const MAX_SELECTED: usize = 10_000;

/// A selection read against a model: the names found, those gone, and those not answered
/// yet, each list in the selection's order. A name given more than once (the same way, in
/// [`read_each`]) is read once, at its first place; a name of an element an earlier name
/// found already is dropped (an element is selected or not, however many of its names are
/// given).
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub kept: Vec<Selected>,
    pub gone: Vec<Gone>,
    /// Names whose body's elements aren't named yet: the background names a model's
    /// elements once it's published, so asked again on a later frame, they're answered.
    /// Meanwhile the app keeps its previous answer for each, so the selection doesn't
    /// flicker. A model the background never finishes naming (a newer one published, or
    /// the engine dropped, mid-naming) leaves them pending: the app asks again of the next
    /// published model.
    pub pending: Vec<Pending>,
    /// The failure of each feature that didn't build that a gone name is of, once each: its
    /// errors as `check` gives them.
    pub failures: BTreeMap<FeatureId, Problems>,
}

/// A selected element, found by its name.
#[derive(Debug, Clone, PartialEq)]
pub struct Selected {
    /// The name the selection holds for it: the name selected, if it's stable; or else
    /// the element's own or an alias that's stable (`elements::stable_name`), so a later
    /// read follows a name that can't come to name another element; or the name selected,
    /// if none of its names is stable.
    pub name: Arc<Name>,
    /// Its place in the selection given, from 0.
    pub place: usize,
    /// The feature whose solid it's of: the body it's in.
    pub feature: FeatureId,
    /// The element, in that solid's numbering.
    pub element: ElementRef,
    /// The name a store (a fillet's edge, a sketch's face) writes for it, the stable one
    /// `name` holds; or, if none of its names is stable, the store's refusal:
    /// `reference_broken`, saying why as a fillet's reference does, with the stable names
    /// it most likely meant as candidates, at the parameter `<parameter>.<place>`.
    pub stored: Result<Arc<Name>, Error>,
}

impl Selected {
    /// Whether a store can take it: one of its names is stable (its `stable`, as the app
    /// holds it).
    pub fn stable(&self) -> bool {
        self.stored.is_ok()
    }

    /// Which kind of element it is.
    pub fn kind(&self) -> ElementKind {
        self.element.kind
    }
}

/// A selected name the model doesn't find, or, carried to a new model ([`refresh`]), an
/// unstable one.
#[derive(Debug, Clone, PartialEq)]
pub struct Gone {
    /// The name as selected.
    pub name: String,
    /// Its place in the selection given, from 0.
    pub place: usize,
    /// Why, each reason an error at the parameter `<parameter>.<place>`: as the lookup says
    /// it (a name that isn't one, of no live solid's element, of a kind a selection doesn't
    /// hold), the store's refusal of an unstable name ([`refresh`]'s), or, for an element of
    /// a feature that didn't build, one naming that feature, with the first of its failure.
    pub why: Problems,
    /// The feature that didn't build, if that's why: its whole failure is in
    /// [`Resolved::failures`].
    pub failed: Option<FeatureId>,
}

/// A selected name not answered yet: its body's elements aren't named yet.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub name: String,
    /// Its place in the selection given, from 0.
    pub place: usize,
}

/// `names`, a pick on `model` (the app's clicks, an agent's names): each name found is
/// kept, an unstable one too, flagged (`Selected::stable`), with the refusal a store would
/// give. Each name is looked for in the model's bodies, as `distance`, `angle`, and
/// `render`'s highlights look ([`Lookup`]). Each error is at `<parameter>.<place>` (the
/// app's `selection.2`, a dialog's `edges.0`). Refused, at `parameter`, past
/// [`MAX_SELECTED`] names. [`read_each`] with every name [`Carry::Pick`].
///
/// Any thread may call it, the UI's too: it names no element map and waits for none, reading
/// only those the model's background has named ([`Resolved::pending`] for the rest), and asks
/// the kernel nothing.
pub fn resolve<S: AsRef<str>>(
    model: &Model,
    names: &[S],
    parameter: &str,
) -> Result<Resolved, Error> {
    let _span = tracing::info_span!("selection_resolve", names = names.len()).entered();
    read(
        model,
        names.iter().map(|name| (name.as_ref(), Carry::Pick)),
        parameter,
        None,
    )
}

/// `names`, a pick on `model` read as [`resolve`] reads it, but each name looked for in the
/// feature `feature`'s solid alone, not in every body: a pick read against the solid a
/// feature builds on, as SolidWorks reads a feature's picks on the model rolled back to
/// before it (a fillet's dialog, open on a fillet that later features build on, reads an
/// edge picked on the body as it's built now against the fillet's base). A name that solid's
/// map holds is kept, its body `feature`, held by a stable name of that map's, else flagged
/// with the store's refusal; one it doesn't hold is gone (`reference_broken`, naming no
/// element of `feature`'s solid, with the likeliest of its names as candidates); one whose
/// map isn't named yet is pending. Refused if `feature` has no solid (the first reason why,
/// as `elements` says it), and, at `parameter`, past [`MAX_SELECTED`] names. The same
/// threads may call it.
pub fn resolve_in<S: AsRef<str>>(
    model: &Model,
    names: &[S],
    parameter: &str,
    feature: &FeatureId,
) -> Result<Resolved, Error> {
    let _span = tracing::info_span!("selection_resolve_in", names = names.len()).entered();
    if let Err(problems) = crate::command::elements::built(model, feature, false) {
        return Err(problems.errors().first().cloned().unwrap_or_else(|| {
            Error::new(ErrorCode::Internal, format!("{feature} has no solid"))
                .with_feature(feature.as_str())
                .with_suggestion(crate::worker::SILICO_BUG)
        }));
    }
    read(
        model,
        names.iter().map(|name| (name.as_ref(), Carry::Pick)),
        parameter,
        Some(feature),
    )
}

/// `names`, a selection carried to `model`, newly published after a rebuild: what's kept,
/// each with its stability now, and what's gone, for the app to clear and say. As
/// [`resolve`], but a name that's unstable in `model` is gone, its reason the store's
/// refusal, whatever the element it finds now and whatever that element's aliases: its text
/// can survive while its element doesn't, so it could find another element now. Nothing is
/// lost by it: [`resolve`] holds an element picked by an unstable name by its stable alias,
/// if it had one then. Several unstable names of one element are each gone, each reported
/// on its own: the check for an element already taken comes after. The same threads may
/// call it. [`read_each`] with every name [`Carry::Refresh`].
pub fn refresh<S: AsRef<str>>(
    model: &Model,
    names: &[S],
    parameter: &str,
) -> Result<Resolved, Error> {
    let _span = tracing::info_span!("selection_refresh", names = names.len()).entered();
    read(
        model,
        names.iter().map(|name| (name.as_ref(), Carry::Refresh)),
        parameter,
        None,
    )
}

/// `names`, a selection read against `model` in one read, each name as it came to the
/// model ([`Carry`]): one picked on it read as [`resolve`] reads it, one carried to it from
/// an older model as [`refresh`] does. One read, so an element is taken once, at the first
/// name that finds it, whichever way each came (a later name of it, an alias or the same
/// text, is in none of the lists); and a name given more than once the same way is read
/// once, at its first place. The same text given both ways is read each way: carried, it
/// may be gone (unstable), while a pick of it on this model is held. As [`resolve`], each
/// error at `<parameter>.<place>`, refused past [`MAX_SELECTED`] names, and the same threads
/// may call it.
pub fn read_each<S: AsRef<str>>(
    model: &Model,
    names: &[(S, Carry)],
    parameter: &str,
) -> Result<Resolved, Error> {
    let _span = tracing::info_span!("selection_read_each", names = names.len()).entered();
    read(
        model,
        names.iter().map(|(name, carry)| (name.as_ref(), *carry)),
        parameter,
        None,
    )
}

/// How a name came to the model a selection is read against, and so whether an element
/// it finds with no stable name is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Carry {
    /// Picked on this model ([`resolve`]): an element found with no stable name is held,
    /// by a stable alias if it has one, else flagged with the store's refusal.
    Pick,
    /// Carried to this model from an older one ([`refresh`]): unstable text is gone,
    /// whatever the element it finds now.
    Refresh,
}

/// `names` read against `model`, each as it came to it, in the feature `solid`'s solid
/// alone, or, for none, in every body.
fn read<'a>(
    model: &Model,
    names: impl ExactSizeIterator<Item = (&'a str, Carry)>,
    parameter: &str,
    solid: Option<&FeatureId>,
) -> Result<Resolved, Error> {
    if names.len() > MAX_SELECTED {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "{} names selected, more than the {MAX_SELECTED} a selection holds",
                names.len()
            ),
        )
        .with_parameter(parameter)
        .with_suggestion(format!("select at most {MAX_SELECTED} elements")));
    }
    let mut lookup = match solid {
        Some(feature) => Lookup::selection_in(model, feature, SELECTED),
        None => Lookup::selection(model, SELECTED),
    };
    let mut resolved = Resolved {
        kept: Vec::new(),
        gone: Vec::new(),
        pending: Vec::new(),
        failures: BTreeMap::new(),
    };
    let mut read: BTreeSet<(&str, Carry)> = BTreeSet::new();
    let mut found: BTreeSet<(&FeatureId, ElementRef)> = BTreeSet::new();
    // Each solid's candidates of each kind, built once, the first time a name of it needs
    // them, as a reference's are.
    let mut candidates: BTreeMap<(&FeatureId, ElementKind), Candidates> = BTreeMap::new();
    for (place, (text, carry)) in names.enumerate() {
        if !read.insert((text, carry)) {
            continue;
        }
        let at = format!("{parameter}.{place}");
        let (feature, map, element, name) = match lookup.found(&at, text) {
            Found::Hit(feature, map, element, name) => (feature, map, element, name),
            Found::Refused(failed) => {
                resolved.gone.push(Gone {
                    name: text.to_owned(),
                    place,
                    why: lookup.take(),
                    failed,
                });
                continue;
            }
            Found::Pending => {
                resolved.pending.push(Pending {
                    name: text.to_owned(),
                    place,
                });
                continue;
            }
        };
        let name = Arc::new(name);
        // The store's refusal of the name as given, if it's unstable.
        let mut refusal = || {
            let candidates = candidates
                .entry((feature, element.kind))
                .or_insert_with(|| Candidates::of(&[map], element.kind))
                .of_name(&name);
            refused(&at, text, &unstable(element.kind), element.kind).with_ranked(candidates)
        };
        let stored = match (map.is_unstable(&name), carry) {
            (false, _) => Ok(Arc::clone(&name)),
            // Carried, unstable text is gone whatever the element it finds now: that may be
            // another element than the one picked, and an alias it has now would be held as
            // if it were the one picked. A pick holds the stable alias it had then.
            (true, Carry::Refresh) => {
                resolved.gone.push(Gone {
                    name: text.to_owned(),
                    place,
                    why: refusal().into(),
                    failed: None,
                });
                continue;
            }
            (true, Carry::Pick) => stable_name(map, element).ok_or_else(refusal),
        };
        // An element found again by a later name: taken once, at the first.
        if !found.insert((feature, element)) {
            continue;
        }
        resolved.kept.push(Selected {
            name: stored.as_ref().map_or(name, Arc::clone),
            place,
            feature: feature.clone(),
            element,
            stored,
        });
    }
    resolved.failures = lookup.failures();
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Checked;
    use crate::fake::FakeKernel;
    use crate::kernel::{
        BooleanOp, BuiltData, ElementCounts, ElementFate, History, InputHistory, Op, Reported,
        Topology,
    };
    use crate::recompute::{FeatureOutcome, Recompute, RunOutcome};
    use std::time::Duration;

    /// The words a reference refuses an unstable face's name with, by hand from
    /// references.rs's message (N3).
    const UNSTABLE_FACE: &str = "names a face, whose name is unstable (it came about as \
                                 another element did, or is its number): an edit could make \
                                 it another element";

    fn checked(features: &[String]) -> Checked {
        let text = format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
            features.join(", ")
        );
        Checked::new(crate::document::read(text.as_bytes()).unwrap()).unwrap()
    }

    /// `features` rebuilt on a fake kernel `setup` has prepared, and published, every
    /// feature's elements named (as the background names them, and a selection read on a
    /// later frame sees them).
    fn rebuilt(features: &[String], setup: impl FnOnce(&FakeKernel)) -> Arc<Model> {
        let model = published(features, setup);
        for (id, _) in model.features() {
            model.element_map(id);
        }
        model
    }

    /// `features` rebuilt on a fake kernel `setup` has prepared, and published, as it is.
    fn published(features: &[String], setup: impl FnOnce(&FakeKernel)) -> Arc<Model> {
        let engine = Recompute::new(FakeKernel::new(2).unwrap())
            .with_sketches(crate::fake::FakeSketches::new());
        setup(engine.kernel());
        match engine
            .start(Arc::new(checked(features)))
            .wait_timeout(Duration::from_secs(120))
        {
            Ok(Ok(RunOutcome::Published(model))) => model,
            _ => panic!("not published"),
        }
    }

    fn model(features: &[String]) -> Arc<Model> {
        rebuilt(features, |_| {})
    }

    fn a_box(id: &str, x: u32, length: u32) -> String {
        format!(
            r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Block", "position": [{x}, 0, 0], "length": {length}, "width": 1, "height": 1}}"#
        )
    }

    fn operation(id: &str, kind: &str, base: &str, tool: &str) -> String {
        format!(
            r#"{{"id": "{id}", "type": "{kind}", "v": 1, "name": "Op", "base": "{base}", "tool": "{tool}"}}"#
        )
    }

    fn fillet(id: &str, base: &str, edge: &str) -> String {
        format!(
            r#"{{"id": "{id}", "type": "fillet", "v": 1, "name": "Round", "base": "{base}", "edges": ["{edge}"], "radius": 0.1}}"#
        )
    }

    fn element(kind: ElementKind, index: u32) -> ElementRef {
        ElementRef { kind, index }
    }

    /// Each kept name, its body, its element, and the name a store writes (or `None`).
    fn kept(resolved: &Resolved) -> Vec<(String, &str, ElementRef, Option<String>)> {
        resolved
            .kept
            .iter()
            .map(|s| {
                (
                    s.name.to_string(),
                    s.feature.as_str(),
                    s.element,
                    s.stored.as_ref().ok().map(ToString::to_string),
                )
            })
            .collect()
    }

    /// Each gone name, and its reasons' codes and messages.
    fn gone(resolved: &Resolved) -> Vec<(&str, Vec<(ErrorCode, &str)>)> {
        resolved
            .gone
            .iter()
            .map(|g| {
                assert_eq!(g.why.total(), g.why.errors().len() as u64);
                (
                    g.name.as_str(),
                    g.why
                        .errors()
                        .iter()
                        .map(|e| (e.code(), e.message()))
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn a_boxs_faces_edges_and_vertex_are_stable_and_kept_by_a_rebuild_that_names_nothing_new() {
        // A box's map names each element by its type and OCCT's number for it (M2.7), every
        // name stable: each selected is found in the box, the body it is, and stored as
        // selected. Then the box made 50 long, and another box beside it: nothing selected
        // is renamed, so all four are kept, in order, still stable, and none is gone.
        let names = [
            "k7f2qx3m.box.Face6",
            "k7f2qx3m.box.Edge1",
            "k7f2qx3m.box.Vertex8",
            "k7f2qx3m.box.Face1",
        ];
        let want = |name: &str, kind, index| {
            (
                name.to_owned(),
                "k7f2qx3m",
                element(kind, index),
                Some(name.to_owned()),
            )
        };
        let expected = vec![
            want(names[0], ElementKind::Face, 6),
            want(names[1], ElementKind::Edge, 1),
            want(names[2], ElementKind::Vertex, 8),
            want(names[3], ElementKind::Face, 1),
        ];
        let before = model(&[a_box("k7f2qx3m", 0, 40)]);
        let resolved = resolve(&before, &names, "selection").unwrap();
        assert_eq!(kept(&resolved), expected);
        assert!(resolved.gone.is_empty());
        assert!(resolved.kept.iter().all(Selected::stable));
        let kinds: Vec<ElementKind> = resolved.kept.iter().map(Selected::kind).collect();
        assert_eq!(
            kinds,
            [
                ElementKind::Face,
                ElementKind::Edge,
                ElementKind::Vertex,
                ElementKind::Face
            ]
        );
        let after = model(&[a_box("k7f2qx3m", 0, 50), a_box("b0000001", 80, 1)]);
        let refreshed = refresh(&after, &names, "selection").unwrap();
        assert_eq!(kept(&refreshed), expected);
        assert!(refreshed.gone.is_empty());
    }

    #[test]
    fn a_rebuild_that_consumes_or_removes_a_selected_element_reports_it_gone_saying_why() {
        // Two boxes, a face of the first and a face and an edge of the second selected.
        // Then a cut of the first by the second: the cut is the body now, holding the
        // first's names (its base's, kept), so that face is kept, in the cut; the second's
        // elements the cut keeps as its own piece, under its own names, so the second's
        // names are in no body: each gone, `reference_broken`, in lookup's words, with
        // the cut's names from that element as candidates, none taken. Then the second box
        // removed: its names are of no feature, so they don't read as names.
        let names = [
            "a0000001.box.Face1",
            "b0000001.box.Face2",
            "b0000001.box.Edge3",
        ];
        let boxes = [a_box("a0000001", 0, 4), a_box("b0000001", 1, 1)];
        let resolved = resolve(&model(&boxes), &names, "selection").unwrap();
        let stored = |name: &str| Some(name.to_owned());
        assert_eq!(
            kept(&resolved),
            [
                (
                    names[0].to_owned(),
                    "a0000001",
                    element(ElementKind::Face, 1),
                    stored(names[0])
                ),
                (
                    names[1].to_owned(),
                    "b0000001",
                    element(ElementKind::Face, 2),
                    stored(names[1])
                ),
                (
                    names[2].to_owned(),
                    "b0000001",
                    element(ElementKind::Edge, 3),
                    stored(names[2])
                ),
            ]
        );
        let mut cut = boxes.to_vec();
        cut.push(operation("c0000001", "cut", "a0000001", "b0000001"));
        let refreshed = refresh(&model(&cut), &names, "selection").unwrap();
        assert_eq!(
            kept(&refreshed),
            [(
                names[0].to_owned(),
                "c0000001",
                element(ElementKind::Face, 1),
                stored(names[0])
            )]
        );
        assert_eq!(
            gone(&refreshed),
            [
                (
                    names[1],
                    vec![(
                        ErrorCode::ReferenceBroken,
                        r#"selection.1, "b0000001.box.Face2", names no element of the live solids"#
                    )]
                ),
                (
                    names[2],
                    vec![(
                        ErrorCode::ReferenceBroken,
                        r#"selection.2, "b0000001.box.Edge3", names no element of the live solids"#
                    )]
                ),
            ]
        );
        let face = &refreshed.gone[0].why.errors()[0];
        assert_eq!(face.parameter(), Some("selection.1"));
        assert_eq!(face.elements(), ["b0000001.box.Face2"]);
        // The cut keeps the tool's elements as its own piece of it (`I`): the one made from
        // each gone name's element is the one offered.
        assert_eq!(
            face.candidates(),
            ["c0000001.cut.Face(I b0000001.box.Face2)"]
        );
        assert_eq!(
            refreshed.gone[1].why.errors()[0].candidates(),
            ["c0000001.cut.Edge(I b0000001.box.Edge3)"]
        );
        // Each ranked by the names alone: the piece is made from the gone name's one
        // element, the whole of it shared, nothing unshared, and it's the cut's, not the
        // box's.
        let rank = |shared: &str| crate::Rank {
            shared: vec![shared.to_owned()],
            unshared: Vec::new(),
            same_feature: false,
            nearest: None,
        };
        assert_eq!(face.ranking(), [rank("b0000001.box.Face2")]);
        assert_eq!(
            refreshed.gone[1].why.errors()[0].ranking(),
            [rank("b0000001.box.Edge3")]
        );
        // Worded for a model, which has no place in a timeline as a document's reference
        // does ("before this one").
        let removed = refresh(&model(&boxes[..1]), &names, "selection").unwrap();
        assert_eq!(kept(&removed).len(), 1);
        let why: Vec<Vec<(ErrorCode, &str)>> =
            gone(&removed).into_iter().map(|(_, why)| why).collect();
        assert_eq!(
            why,
            [
                vec![(
                    ErrorCode::ReferenceBroken,
                    r#"selection.1, "b0000001.box.Face2", isn't a name: no feature b0000001 in the model, at column 1"#
                )],
                vec![(
                    ErrorCode::ReferenceBroken,
                    r#"selection.2, "b0000001.box.Edge3", isn't a name: no feature b0000001 in the model, at column 1"#
                )],
            ]
        );
    }

    #[test]
    fn each_name_of_a_feature_that_failed_is_gone_at_its_place_and_the_failure_kept_once() {
        // A box the kernel refuses (2 mm long) has no solid: each of its names selected is
        // gone, each with its own reason at its place, of the failure's code, naming the box
        // and the failure's words; the box's failure itself kept once for the selection.
        let refusing = |kernel: &FakeKernel| {
            kernel.refuse(|op| {
                matches!(op, Op::MakePrimitive(silico_protocol::Primitive::Box { dx, .. }) if *dx == 2.0)
                    .then(|| Error::new(ErrorCode::KernelFailed, "MakeBox failed"))
            })
        };
        let model = rebuilt(
            &[a_box("a0000001", 0, 1), a_box("e0000001", 5, 2)],
            refusing,
        );
        let names = [
            "e0000001.box.Face1",
            "a0000001.box.Face1",
            "e0000001.box.Edge2",
        ];
        let resolved = resolve(&model, &names, "selection").unwrap();
        assert_eq!(kept(&resolved).len(), 1);
        assert_eq!(resolved.kept[0].feature.as_str(), "a0000001");
        assert_eq!(resolved.gone.len(), 2);
        for (gone, place) in resolved.gone.iter().zip([0, 2]) {
            assert_eq!(gone.name, names[place]);
            assert_eq!(gone.place, place);
            assert_eq!(
                gone.failed.as_ref().map(FeatureId::as_str),
                Some("e0000001")
            );
            let [error] = gone.why.errors() else {
                panic!("{:?}", gone.why)
            };
            assert_eq!(error.code(), ErrorCode::KernelFailed);
            assert_eq!(error.feature(), Some("e0000001"));
            let at = format!("selection.{place}");
            assert_eq!(error.parameter(), Some(at.as_str()));
            assert_eq!(
                error.message(),
                format!(
                    r#"{at}, "{}", is of e0000001, which didn't build: MakeBox failed"#,
                    names[place]
                )
            );
        }
        let failures: Vec<(&str, Vec<(ErrorCode, &str)>)> = resolved
            .failures
            .iter()
            .map(|(id, p)| {
                (
                    id.as_str(),
                    p.errors().iter().map(|e| (e.code(), e.message())).collect(),
                )
            })
            .collect();
        assert_eq!(
            failures,
            [(
                "e0000001",
                vec![(ErrorCode::KernelFailed, "MakeBox failed")]
            )]
        );
    }

    #[test]
    fn a_malformed_name_and_a_solids_name_are_gone_each_saying_why() {
        // Not a name: the lookup's `reference_broken`, "isn't a name", with nothing offered.
        // A solid's name parses, but a selection holds faces, edges, and vertices.
        let model = model(&[a_box("k7f2qx3m", 0, 1)]);
        let resolved = resolve(
            &model,
            &["k7f2qx3m.box", "k7f2qx3m.box.Face1", "k7f2qx3m.box.Solid1"],
            "selection",
        )
        .unwrap();
        assert_eq!(resolved.kept.len(), 1);
        let [gone, solid] = resolved.gone.as_slice() else {
            panic!("{:?}", resolved.gone)
        };
        let [error] = solid.why.errors() else {
            panic!("{:?}", solid.why)
        };
        assert_eq!(error.code(), ErrorCode::InvalidInput);
        assert_eq!(
            error.message(),
            r#"selection.2, "k7f2qx3m.box.Solid1", names a solid: a selection is of faces, edges, and vertices"#
        );
        assert_eq!(
            error.suggestion(),
            Some("select a face, an edge, or a vertex of a body")
        );
        assert_eq!(gone.name, "k7f2qx3m.box");
        let [error] = gone.why.errors() else {
            panic!("{:?}", gone.why)
        };
        assert_eq!(error.code(), ErrorCode::ReferenceBroken);
        assert!(
            error
                .message()
                .starts_with(r#"selection.0, "k7f2qx3m.box", isn't a name: "#),
            "{}",
            error.message()
        );
        assert!(error.candidates().is_empty());
    }

    /// A fuse's history in which the base, a 1 mm cube, is the result, its Face1 and Edge1
    /// reported by nothing (Edge1 bounding Face1 alone), so both are named by their numbers,
    /// unstable (N3); the rest kept; the tool reported by nothing (as adversarial_entries'
    /// an_element_nothing_names_is_unstable_and_stable_filters_on_it).
    fn numbering_fuse() -> History {
        let kept = |n: u32| ElementFate {
            kept: Some(n),
            ..ElementFate::default()
        };
        let none = |n: usize| vec![ElementFate::default(); n];
        let face1 = |rest: Vec<Vec<u32>>| -> Vec<Vec<u32>> {
            std::iter::once(vec![1, 2]).chain(rest).collect()
        };
        let history = History {
            inputs: vec![
                InputHistory {
                    faces: none(1).into_iter().chain((2..=6).map(kept)).collect(),
                    edges: none(1).into_iter().chain((2..=12).map(kept)).collect(),
                    vertices: (1..=8).map(kept).collect(),
                },
                InputHistory {
                    faces: none(6),
                    edges: none(12),
                    vertices: none(8),
                },
            ],
            result: ElementCounts {
                faces: 6,
                edges: 12,
                vertices: 8,
                solids: 1,
            },
            ends: Default::default(),
            topology: Topology {
                face_edges: face1(vec![vec![]; 5]),
                outer_edges: face1(vec![vec![]; 5]),
                edge_vertices: vec![vec![]; 12],
                solid_faces: vec![(1..=6).collect()],
            },
        };
        history.validate(2).unwrap();
        history
    }

    /// A fake whose fuses of a tool `history` picks give a box a mm³ larger than the base
    /// back with that history.
    fn forging(
        history: impl Fn(&str) -> Option<History> + Send + Sync + 'static,
    ) -> impl FnOnce(&FakeKernel) {
        move |kernel: &FakeKernel| {
            kernel.answer_instead(move |op| match op {
                Op::Boolean {
                    op: BooleanOp::Fuse,
                    base,
                    tool,
                } => {
                    let shape = FakeKernel::resized(base, 1.0)?;
                    history(&FakeKernel::describe(tool)?)
                        .map(|history| silico_protocol::Reply::Built(BuiltData { shape, history }))
                }
                _ => None,
            })
        }
    }

    #[test]
    fn a_numbered_face_is_held_unstable_with_the_refusal_a_reference_gives() {
        // The fuse's Face1, named by its number, is found, in the fuse, and held, but a
        // store refuses it: `reference_broken`, in a reference's words for a face, its
        // place in the selection the parameter, and no candidate (no stable face shares
        // anything with a name by number of the fuse's, and none is the fuse's own by
        // number). Its kept Face2 is stable.
        let features = [
            a_box("aaaaaaaa", 0, 1),
            a_box("bbbbbbbb", 5, 1),
            operation("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
        ];
        let model = rebuilt(&features, forging(|_| Some(numbering_fuse())));
        let resolved = resolve(
            &model,
            &["aaaaaaaa.box.Face2", "rrrrrrrr.fuse.Face1"],
            "selection",
        )
        .unwrap();
        assert!(resolved.gone.is_empty());
        let [face2, face1] = resolved.kept.as_slice() else {
            panic!("{resolved:?}")
        };
        assert!(face2.stable());
        assert_eq!(face2.element, element(ElementKind::Face, 2));
        assert!(!face1.stable());
        assert_eq!(face1.feature.as_str(), "rrrrrrrr");
        assert_eq!(face1.element, element(ElementKind::Face, 1));
        let Err(refusal) = &face1.stored else {
            panic!("{:?}", face1.stored)
        };
        assert_eq!(refusal.code(), ErrorCode::ReferenceBroken);
        assert_eq!(
            refusal.message(),
            format!(r#"selection.1, "rrrrrrrr.fuse.Face1", {UNSTABLE_FACE}"#)
        );
        assert_eq!(refusal.parameter(), Some("selection.1"));
        assert_eq!(refusal.elements(), ["rrrrrrrr.fuse.Face1"]);
        assert!(refusal.candidates().is_empty());
        assert_eq!(
            refusal.suggestion(),
            Some(
                "name the face as its feature now names it: the candidates are the likeliest, \
                 none taken for it"
            )
        );
    }

    /// A fuse's history in which the base, a 1 mm cube, is the result, every element kept
    /// but its Edge1 and Edge2, which come about alike, each generated from Face1 and from
    /// Face6: two edges of one name, the second its `~1`, both unstable (N3), though the
    /// first's text names no number; and its Edge3, generated from Face1 and Face3, stable.
    /// The tool is reported by nothing.
    fn duplicating_fuse() -> History {
        let kept = |n: u32| ElementFate {
            kept: Some(n),
            ..ElementFate::default()
        };
        let edge = |index| Reported {
            element: element(ElementKind::Edge, index),
            position: 1,
        };
        let mut faces: Vec<ElementFate> = (1..=6).map(kept).collect();
        faces[0].generated = vec![edge(1), edge(2), edge(3)];
        faces[2].generated = vec![edge(3)];
        faces[5].generated = vec![edge(1), edge(2)];
        let history = History {
            inputs: vec![
                InputHistory {
                    faces,
                    edges: vec![ElementFate::default(); 3]
                        .into_iter()
                        .chain((4..=12).map(kept))
                        .collect(),
                    vertices: (1..=8).map(kept).collect(),
                },
                InputHistory {
                    faces: vec![ElementFate::default(); 6],
                    edges: vec![ElementFate::default(); 12],
                    vertices: vec![ElementFate::default(); 8],
                },
            ],
            result: ElementCounts {
                faces: 6,
                edges: 12,
                vertices: 8,
                solids: 1,
            },
            ends: Default::default(),
            topology: Topology {
                face_edges: vec![vec![]; 6],
                outer_edges: vec![vec![]; 6],
                edge_vertices: vec![vec![]; 12],
                solid_faces: vec![(1..=6).collect()],
            },
        };
        history.validate(2).unwrap();
        history
    }

    #[test]
    fn an_unstable_edge_is_refused_as_a_fillet_naming_it_is_refused() {
        // M5.7's test: a selection of an unstable name is held, and a fillet fed it refuses
        // it, saying why. The duplicated edge's first name has no number in it, so the gate
        // takes it and the fillet's rebuild refuses it (references.rs): the selection's
        // refusal is that one, word for word, but for where it is (`selection.0` for
        // `edges.0`): its code, its reason, its candidates, and its advice. Its candidate is
        // the stable edge sharing Face1 with it, Edge3.
        let edge1 = "rrrrrrrr.fuse.Edge(G1/2 aaaaaaaa.box.Face1, G1/2 aaaaaaaa.box.Face6)";
        let mut features = vec![
            a_box("aaaaaaaa", 0, 1),
            a_box("bbbbbbbb", 5, 1),
            operation("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
        ];
        let before = rebuilt(&features, forging(|_| Some(duplicating_fuse())));
        let map = before
            .element_map(&FeatureId::new("rrrrrrrr").unwrap())
            .unwrap();
        let first = map.name(element(ElementKind::Edge, 1)).unwrap();
        assert_eq!(first.to_string(), edge1);
        assert!(map.is_unstable(&first));
        let resolved = resolve(&before, &[edge1], "selection").unwrap();
        let [selected] = resolved.kept.as_slice() else {
            panic!("{resolved:?}")
        };
        assert!(!selected.stable());
        let Err(refusal) = &selected.stored else {
            panic!("{:?}", selected.stored)
        };
        features.push(fillet("ffffffff", "rrrrrrrr", edge1));
        let filleted = rebuilt(&features, forging(|_| Some(duplicating_fuse())));
        let Some(FeatureOutcome::Broken { errors, total: 1 }) =
            filleted.outcome(&FeatureId::new("ffffffff").unwrap())
        else {
            panic!("{:?}", filleted.features())
        };
        let [fillets] = errors.as_slice() else {
            panic!("{errors:?}")
        };
        // The name as a message shows it: its first 40 characters, quoted, then `…`.
        let why = r#""rrrrrrrr.fuse.Edge(G1/2 aaaaaaaa.box.Fac"…, names an edge, whose name is unstable (it came about as another element did, or is its number): an edit could make it another element"#;
        assert_eq!(fillets.message(), format!("edges.0, {why}"));
        assert_eq!(refusal.message(), format!("selection.0, {why}"));
        assert_eq!(refusal.code(), fillets.code());
        assert_eq!(refusal.code(), ErrorCode::ReferenceBroken);
        assert_eq!(
            refusal.candidates(),
            ["rrrrrrrr.fuse.Edge(G aaaaaaaa.box.Face1, G aaaaaaaa.box.Face3)"]
        );
        assert_eq!(refusal.candidates(), fillets.candidates());
        assert_eq!(refusal.suggestion(), fillets.suggestion());
        assert_eq!(refusal.elements(), fillets.elements());
        // The fillet's names its feature; a selection's has none.
        assert_eq!(fillets.feature(), Some("ffffffff"));
        assert_eq!(refusal.feature(), None);
        // A dialog's, at its own parameter (`edges`): the fillet's message, word for word.
        let dialog = resolve(&before, &[edge1], "edges").unwrap();
        let Err(refusal) = &dialog.kept[0].stored else {
            panic!("{dialog:?}")
        };
        assert_eq!(refusal.message(), fillets.message());
        assert_eq!(refusal.parameter(), Some("edges.0"));
    }

    /// A second fuse's history, of the numbering fuse (base) and a cube (tool): the base
    /// kept whole, and the tool's Face1 kept as the base's Face1 too, so that face has two
    /// names, the base's first, by its number, unstable, and the tool's, stable. The rest
    /// of the tool reported by nothing.
    fn aliasing_fuse() -> History {
        let kept = |n: u32| ElementFate {
            kept: Some(n),
            ..ElementFate::default()
        };
        let mut tool_faces = vec![ElementFate::default(); 6];
        tool_faces[0] = kept(1);
        let history = History {
            inputs: vec![
                InputHistory {
                    faces: (1..=6).map(kept).collect(),
                    edges: (1..=12).map(kept).collect(),
                    vertices: (1..=8).map(kept).collect(),
                },
                InputHistory {
                    faces: tool_faces,
                    edges: vec![ElementFate::default(); 12],
                    vertices: vec![ElementFate::default(); 8],
                },
            ],
            result: ElementCounts {
                faces: 6,
                edges: 12,
                vertices: 8,
                solids: 1,
            },
            ends: Default::default(),
            topology: Topology {
                face_edges: vec![vec![]; 6],
                outer_edges: vec![vec![]; 6],
                edge_vertices: vec![vec![]; 12],
                solid_faces: vec![(1..=6).collect()],
            },
        };
        history.validate(2).unwrap();
        history
    }

    #[test]
    fn an_element_unstable_by_its_own_name_with_a_stable_alias_is_stable_and_stored_by_it() {
        // The numbering fuse's Face1, `rrrrrrrr.fuse.Face1`, unstable, fused again with a
        // cube (cccccccc, 2 mm) whose Face1 becomes the same face: its own name is still
        // the first fuse's, and `cccccccc.box.Face1` its alias, stable. Selected by its own
        // name, it's stable, held by the alias (so a later read follows a name that can't
        // come to name another element), and a store writes the alias; selected by the
        // alias, the alias. Both names selected are one element: taken once, at the first.
        let features = [
            a_box("aaaaaaaa", 0, 1),
            a_box("bbbbbbbb", 5, 1),
            operation("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
            a_box("cccccccc", 9, 2),
            operation("ssssssss", "fuse", "rrrrrrrr", "cccccccc"),
        ];
        let model = rebuilt(
            &features,
            forging(|tool| {
                if tool.contains("box(2,") {
                    Some(aliasing_fuse())
                } else {
                    Some(numbering_fuse())
                }
            }),
        );
        let map = model
            .element_map(&FeatureId::new("ssssssss").unwrap())
            .unwrap();
        let names: Vec<String> = map
            .names(element(ElementKind::Face, 1))
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(names, ["rrrrrrrr.fuse.Face1", "cccccccc.box.Face1"]);
        let own = "rrrrrrrr.fuse.Face1";
        let alias = "cccccccc.box.Face1";
        let face1 = element(ElementKind::Face, 1);
        let by_own = resolve(&model, &[own, alias], "selection").unwrap();
        assert_eq!(
            kept(&by_own),
            [(alias.to_owned(), "ssssssss", face1, Some(alias.to_owned()))]
        );
        assert_eq!(by_own.kept[0].place, 0);
        assert!(by_own.kept[0].stable());
        assert!(by_own.gone.is_empty());
        let by_alias = resolve(&model, &[alias, own], "selection").unwrap();
        assert_eq!(
            kept(&by_alias),
            [(alias.to_owned(), "ssssssss", face1, Some(alias.to_owned()))]
        );
        // Its Edge1, by its number in the first fuse and no other name: unstable still,
        // held as selected when picked, and gone when carried to a model.
        let edge = resolve(&model, &["rrrrrrrr.fuse.Edge1"], "selection").unwrap();
        assert!(!edge.kept[0].stable());
        assert_eq!(edge.kept[0].name.to_string(), "rrrrrrrr.fuse.Edge1");
        // Carried, unstable text is gone, the face's own name too, though the face has a
        // stable alias now: the pick held it by the alias, which is carried and kept.
        let carried = refresh(&model, &[own, "rrrrrrrr.fuse.Edge1"], "selection").unwrap();
        assert!(carried.kept.is_empty(), "{carried:?}");
        let places: Vec<usize> = carried.gone.iter().map(|g| g.place).collect();
        assert_eq!(places, [0, 1]);
        assert_eq!(
            carried.gone[0].why.errors()[0].message(),
            format!(r#"selection.0, "{own}", {UNSTABLE_FACE}"#)
        );
        let held: Vec<String> = by_own.kept.iter().map(|s| s.name.to_string()).collect();
        let carried = refresh(&model, &held, "selection").unwrap();
        assert_eq!(
            kept(&carried),
            [(alias.to_owned(), "ssssssss", face1, Some(alias.to_owned()))]
        );
        // Unstable text gone, the element's stable name after it is still kept: the gone
        // name holds no place for its element.
        let carried = refresh(&model, &[own, alias], "selection").unwrap();
        assert_eq!(
            kept(&carried),
            [(alias.to_owned(), "ssssssss", face1, Some(alias.to_owned()))]
        );
        assert_eq!(carried.kept[0].place, 1);
    }

    #[test]
    fn a_read_of_names_picked_and_carried_reads_each_its_own_way_taking_each_element_once() {
        // The face of two names, its own `rrrrrrrr.fuse.Face1` (unstable) and the alias
        // `cccccccc.box.Face1` (stable), as the alias test builds it. One read, each name as
        // it came: carried, the unstable text is gone, in the store's words, and holds no
        // place for its element, so a pick after it of the alias, or of the same text (held
        // by the alias), is kept at its own place. Read the other way round, the pick takes
        // the face first, and the carried text is gone all the same; a stable alias carried
        // takes it first, and a pick of the face's own name after it is in no list.
        let features = [
            a_box("aaaaaaaa", 0, 1),
            a_box("bbbbbbbb", 5, 1),
            operation("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
            a_box("cccccccc", 9, 2),
            operation("ssssssss", "fuse", "rrrrrrrr", "cccccccc"),
        ];
        let model = rebuilt(
            &features,
            forging(|tool| {
                if tool.contains("box(2,") {
                    Some(aliasing_fuse())
                } else {
                    Some(numbering_fuse())
                }
            }),
        );
        let own = "rrrrrrrr.fuse.Face1";
        let alias = "cccccccc.box.Face1";
        let face1 = element(ElementKind::Face, 1);
        let held = (alias.to_owned(), "ssssssss", face1, Some(alias.to_owned()));
        let gone_at = |resolved: &Resolved| -> Vec<(String, usize, String)> {
            resolved
                .gone
                .iter()
                .map(|g| {
                    (
                        g.name.clone(),
                        g.place,
                        g.why.errors()[0].message().to_owned(),
                    )
                })
                .collect()
        };
        let unstable_at = |place: usize| format!(r#"selection.{place}, "{own}", {UNSTABLE_FACE}"#);
        let places =
            |resolved: &Resolved| -> Vec<usize> { resolved.kept.iter().map(|s| s.place).collect() };
        for picked in [alias, own] {
            let read = read_each(
                &model,
                &[(own, Carry::Refresh), (picked, Carry::Pick)],
                "selection",
            )
            .unwrap();
            assert_eq!(kept(&read), std::slice::from_ref(&held), "{picked}");
            assert_eq!(places(&read), [1], "{picked}");
            assert_eq!(gone_at(&read), [(own.to_owned(), 0, unstable_at(0))]);
            assert!(read.pending.is_empty());
        }
        let read = read_each(
            &model,
            &[(own, Carry::Pick), (own, Carry::Refresh)],
            "selection",
        )
        .unwrap();
        assert_eq!(kept(&read), std::slice::from_ref(&held));
        assert_eq!(places(&read), [0]);
        assert_eq!(gone_at(&read), [(own.to_owned(), 1, unstable_at(1))]);
        let read = read_each(
            &model,
            &[(alias, Carry::Refresh), (own, Carry::Pick)],
            "selection",
        )
        .unwrap();
        assert_eq!(kept(&read), std::slice::from_ref(&held));
        assert_eq!(places(&read), [0]);
        assert!(read.gone.is_empty() && read.pending.is_empty(), "{read:?}");
        // The same text the same way twice: read once.
        let read = read_each(
            &model,
            &[(own, Carry::Refresh), (own, Carry::Refresh)],
            "selection",
        )
        .unwrap();
        assert_eq!(gone_at(&read), [(own.to_owned(), 0, unstable_at(0))]);
        assert!(read.kept.is_empty());
        // Every name one way: as `resolve` and `refresh` read them.
        let names = [own, alias];
        let each = |carry| -> Vec<(&str, Carry)> { names.iter().map(|n| (*n, carry)).collect() };
        assert_eq!(
            read_each(&model, &each(Carry::Pick), "selection").unwrap(),
            resolve(&model, &names, "selection").unwrap()
        );
        assert_eq!(
            read_each(&model, &each(Carry::Refresh), "selection").unwrap(),
            refresh(&model, &names, "selection").unwrap()
        );
    }

    /// A second fuse's history, of the numbering fuse (base) and a cube (tool): the base
    /// kept whole, and nothing of the tool reported, so it names nothing new.
    fn plain_fuse() -> History {
        let mut history = aliasing_fuse();
        history.inputs[1].faces[0] = ElementFate::default();
        history.validate(2).unwrap();
        history
    }

    #[test]
    fn carried_unstable_text_is_gone_though_the_element_it_finds_now_has_a_stable_alias() {
        // The format reviewer's proof. Before: the numbering fuse's `rrrrrrrr.fuse.Face1`,
        // fused again plainly with a 2 mm cube, names the second fuse's Face1, which has no
        // other name: picked, unstable, held as given. Then the first fuse's tool edited (3
        // mm: the renumbering fuse, so the text names another face) and the cube made 4 mm
        // (the aliasing fuse: its Face1 joins the face the text names now). The text finds
        // a face it never picked, which has a stable alias now; carried, it's gone, its word
        // the store's refusal, never kept as `cccccccc.box.Face1`.
        let name = "rrrrrrrr.fuse.Face1";
        let features = |tool: u32, cube: u32| {
            [
                a_box("aaaaaaaa", 0, 1),
                a_box("bbbbbbbb", 5, tool),
                operation("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
                a_box("cccccccc", 9, cube),
                operation("ssssssss", "fuse", "rrrrrrrr", "cccccccc"),
            ]
        };
        let fusing = || {
            forging(|tool| {
                Some(if tool.contains("box(3,") {
                    renumbering_fuse()
                } else if tool.contains("box(2,") {
                    plain_fuse()
                } else if tool.contains("box(4,") {
                    aliasing_fuse()
                } else {
                    numbering_fuse()
                })
            })
        };
        let face1 = element(ElementKind::Face, 1);
        let before = rebuilt(&features(1, 2), fusing());
        let names_of = |model: &Model| -> Vec<String> {
            model
                .element_map(&FeatureId::new("ssssssss").unwrap())
                .unwrap()
                .names(face1)
                .iter()
                .map(ToString::to_string)
                .collect()
        };
        assert_eq!(names_of(&before), [name]);
        let picked = resolve(&before, &[name], "selection").unwrap();
        assert_eq!(kept(&picked), [(name.to_owned(), "ssssssss", face1, None)]);
        let after = rebuilt(&features(3, 4), fusing());
        assert_eq!(names_of(&after), [name, "cccccccc.box.Face1"]);
        let held: Vec<String> = picked.kept.iter().map(|s| s.name.to_string()).collect();
        let carried = refresh(&after, &held, "selection").unwrap();
        assert!(carried.kept.is_empty(), "{carried:?}");
        let [gone] = carried.gone.as_slice() else {
            panic!("{carried:?}")
        };
        let [error] = gone.why.errors() else {
            panic!("{:?}", gone.why)
        };
        assert_eq!(error.code(), ErrorCode::ReferenceBroken);
        assert_eq!(
            error.message(),
            format!(r#"selection.0, "{name}", {UNSTABLE_FACE}"#)
        );
        // A new pick of the text on the edited model is another matter: held by the alias
        // the face has now.
        let repicked = resolve(&after, &[name], "selection").unwrap();
        assert_eq!(
            kept(&repicked),
            [(
                "cccccccc.box.Face1".to_owned(),
                "ssssssss",
                face1,
                Some("cccccccc.box.Face1".to_owned())
            )]
        );
    }

    /// The numbering fuse after an edit of its tool: the base's Face1 kept, as the result's
    /// Face2, and its Face2 reported by nothing, so the result's Face1 is the base's Face2's
    /// place's face, named by its number, `rrrrrrrr.fuse.Face1` again: the same text, another
    /// element.
    fn renumbering_fuse() -> History {
        let mut history = numbering_fuse();
        let kept = |n: u32| ElementFate {
            kept: Some(n),
            ..ElementFate::default()
        };
        history.inputs[0].faces = vec![
            kept(2),
            ElementFate::default(),
            kept(3),
            kept(4),
            kept(5),
            kept(6),
        ];
        history.validate(2).unwrap();
        history
    }

    #[test]
    fn a_numbered_name_a_rebuild_gives_another_element_is_gone_when_carried_not_kept() {
        // The format reviewer's proof. Before: `rrrrrrrr.fuse.Face1` is the face where the
        // base's Face1 was (nothing reported it), held unstable when picked. Then the tool
        // edited (a 3 mm box): the base's Face1 is kept as the result's Face2, and the
        // result's Face1 is another face, named by its number as before. A pick of the text
        // finds that other face, flagged unstable; carried across the rebuild, it's gone,
        // its word the store's refusal, never silently kept on another element.
        let features = |tool: u32| {
            [
                a_box("aaaaaaaa", 0, 1),
                a_box("bbbbbbbb", 5, tool),
                operation("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
            ]
        };
        let fusing = || {
            forging(|tool| {
                if tool.contains("box(3,") {
                    Some(renumbering_fuse())
                } else {
                    Some(numbering_fuse())
                }
            })
        };
        let name = "rrrrrrrr.fuse.Face1";
        let before = rebuilt(&features(1), fusing());
        let picked = resolve(&before, &[name], "selection").unwrap();
        assert!(!picked.kept[0].stable());
        let after = rebuilt(&features(3), fusing());
        let map = after
            .element_map(&FeatureId::new("rrrrrrrr").unwrap())
            .unwrap();
        // The base's Face1 is the result's Face2 now.
        let base_face1 = Name::parse("aaaaaaaa.box.Face1", &|id| after.maker(id)).unwrap();
        assert_eq!(
            map.element(&base_face1),
            Some(element(ElementKind::Face, 2))
        );
        let carried = refresh(&after, &[name], "selection").unwrap();
        assert!(carried.kept.is_empty(), "{carried:?}");
        let [gone] = carried.gone.as_slice() else {
            panic!("{carried:?}")
        };
        assert_eq!(
            (gone.name.as_str(), gone.place, &gone.failed),
            (name, 0, &None)
        );
        let [error] = gone.why.errors() else {
            panic!("{:?}", gone.why)
        };
        assert_eq!(error.code(), ErrorCode::ReferenceBroken);
        assert_eq!(
            error.message(),
            format!(r#"selection.0, "{name}", {UNSTABLE_FACE}"#)
        );
        // Picked on the new model, the text is held, flagged, on the other face.
        let repicked = resolve(&after, &[name], "selection").unwrap();
        assert_eq!(repicked.kept[0].element, element(ElementKind::Face, 1));
        assert!(!repicked.kept[0].stable());
    }

    #[test]
    fn a_selection_read_before_the_models_names_are_named_answers_pending_and_names_nothing() {
        // A model as published, before the background has named any element: a name of a
        // body is pending, never named on the caller's thread (which may be the UI's), so
        // no map is named after it either; a name of a feature that failed, or one that
        // isn't a name, needs no map and is answered. Once one body is named, its names are
        // kept, and a name its map hasn't stays pending, since its candidates come from
        // every body's map. Once all are, every name is answered.
        let refusing = |kernel: &FakeKernel| {
            kernel.refuse(|op| {
                matches!(op, Op::MakePrimitive(silico_protocol::Primitive::Box { dx, .. }) if *dx == 2.0)
                    .then(|| Error::new(ErrorCode::KernelFailed, "MakeBox failed"))
            })
        };
        let features = [
            a_box("a0000001", 0, 1),
            a_box("b0000001", 5, 1),
            a_box("e0000001", 9, 2),
        ];
        let model = published(&features, refusing).unnamed();
        let names = [
            "a0000001.box.Face1",
            "e0000001.box.Face1",
            "b0000001.box.Edge4",
            "nonsense",
            "a0000001.box.Face7",
        ];
        let pending = |resolved: &Resolved| -> Vec<(String, usize)> {
            resolved
                .pending
                .iter()
                .map(|p| (p.name.clone(), p.place))
                .collect()
        };
        let at = |place: usize| (names[place].to_owned(), place);
        let gone =
            |resolved: &Resolved| -> Vec<usize> { resolved.gone.iter().map(|g| g.place).collect() };
        let resolved = resolve(&model, &names, "selection").unwrap();
        assert!(resolved.kept.is_empty());
        assert_eq!(pending(&resolved), [at(0), at(2), at(4)]);
        assert_eq!(gone(&resolved), [1, 3]);
        for (id, _) in model.features() {
            assert!(model.named_so_far(id).is_none(), "{id} named");
        }
        // One body named.
        assert!(
            model
                .element_map(&FeatureId::new("a0000001").unwrap())
                .is_some()
        );
        let resolved = resolve(&model, &names, "selection").unwrap();
        assert_eq!(kept(&resolved).len(), 1);
        assert_eq!(resolved.kept[0].place, 0);
        assert_eq!(pending(&resolved), [at(2), at(4)]);
        assert!(
            model
                .named_so_far(&FeatureId::new("b0000001").unwrap())
                .is_none()
        );
        // Every body named.
        for (id, _) in model.features() {
            model.element_map(id);
        }
        let resolved = resolve(&model, &names, "selection").unwrap();
        let places: Vec<usize> = resolved.kept.iter().map(|s| s.place).collect();
        assert_eq!(places, [0, 2]);
        assert!(resolved.pending.is_empty());
        assert_eq!(gone(&resolved), [1, 3, 4]);
    }

    #[test]
    fn a_stable_name_selected_is_stored_as_selected_though_the_elements_own_is_another() {
        // The aliasing fuse of a plain cube and a 2 mm one: their two Face1s one face, its
        // names the base's, `aaaaaaaa.box.Face1`, then the tool's, both stable. Selected by
        // the tool's, it's stored by the tool's, as selected, not by the face's own.
        let features = [
            a_box("aaaaaaaa", 0, 1),
            a_box("cccccccc", 9, 2),
            operation("ssssssss", "fuse", "aaaaaaaa", "cccccccc"),
        ];
        let model = rebuilt(&features, forging(|_| Some(aliasing_fuse())));
        let alias = "cccccccc.box.Face1";
        assert_eq!(
            kept(&resolve(&model, &[alias], "selection").unwrap()),
            [(
                alias.to_owned(),
                "ssssssss",
                element(ElementKind::Face, 1),
                Some(alias.to_owned())
            )]
        );
        let own = "aaaaaaaa.box.Face1";
        assert_eq!(
            kept(&resolve(&model, &[own], "selection").unwrap())[0]
                .3
                .as_deref(),
            Some(own)
        );
    }

    #[test]
    fn a_selection_keeps_its_order_and_reads_each_name_once() {
        // Faces, edges, a vertex, and two names that aren't, out of the map's order and
        // repeated: kept in the selection's order, each at its first place, and the bad
        // name gone once, its place its first.
        let model = model(&[a_box("k7f2qx3m", 0, 1)]);
        let names = [
            "k7f2qx3m.box.Face3",
            "k7f2qx3m.box.Edge12",
            "nonsense",
            "k7f2qx3m.box.Face3",
            "k7f2qx3m.box.Vertex2",
            "nonsense",
            "k7f2qx3m.box.Edge12",
            "k7f2qx3m.box.Face1",
        ];
        let resolved = resolve(&model, &names, "selection").unwrap();
        let kept: Vec<String> = resolved.kept.iter().map(|s| s.name.to_string()).collect();
        assert_eq!(
            kept,
            [
                "k7f2qx3m.box.Face3",
                "k7f2qx3m.box.Edge12",
                "k7f2qx3m.box.Vertex2",
                "k7f2qx3m.box.Face1"
            ]
        );
        let [gone] = resolved.gone.as_slice() else {
            panic!("{:?}", resolved.gone)
        };
        assert_eq!(gone.name, "nonsense");
        assert_eq!(gone.why.errors()[0].parameter(), Some("selection.2"));
        // Again, the same answer.
        assert_eq!(resolve(&model, &names, "selection").unwrap(), resolved);
    }

    #[test]
    fn the_first_stable_alias_is_stored() {
        // The tests reviewer's proof. Three fuses: the numbering fuse's Face1, by its number,
        // unstable; then the aliasing fuse with a 2 mm cube, whose Face1 joins it; then the
        // aliasing fuse again with a 3 mm cube, whose Face1 joins it too. The face's names:
        // its own, unstable, then the two cubes' Face1s, in the map's order, both stable.
        // Picked by its own name, it's held and stored by the first stable alias, the 2 mm
        // cube's, not the last; carried, that name is kept as it is.
        let features = [
            a_box("aaaaaaaa", 0, 1),
            a_box("bbbbbbbb", 5, 1),
            operation("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
            a_box("cccccccc", 9, 2),
            operation("ssssssss", "fuse", "rrrrrrrr", "cccccccc"),
            a_box("dddddddd", 20, 3),
            operation("tttttttt", "fuse", "ssssssss", "dddddddd"),
        ];
        let model = rebuilt(
            &features,
            forging(|tool| {
                if tool.contains("box(1,") {
                    Some(numbering_fuse())
                } else {
                    Some(aliasing_fuse())
                }
            }),
        );
        let map = model
            .element_map(&FeatureId::new("tttttttt").unwrap())
            .unwrap();
        let names: Vec<String> = map
            .names(element(ElementKind::Face, 1))
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            names,
            [
                "rrrrrrrr.fuse.Face1",
                "cccccccc.box.Face1",
                "dddddddd.box.Face1"
            ]
        );
        let first = (
            "cccccccc.box.Face1".to_owned(),
            "tttttttt",
            element(ElementKind::Face, 1),
            Some("cccccccc.box.Face1".to_owned()),
        );
        let resolved = resolve(&model, &["rrrrrrrr.fuse.Face1"], "selection").unwrap();
        assert_eq!(kept(&resolved), std::slice::from_ref(&first));
        let held: Vec<String> = resolved.kept.iter().map(|s| s.name.to_string()).collect();
        assert_eq!(kept(&refresh(&model, &held, "selection").unwrap()), [first]);
    }

    #[test]
    fn each_bodys_unstable_edge_gets_its_own_bodys_candidates() {
        // The tests reviewer's proof. Two bodies, each the duplicating fuse of two cubes:
        // each has an unstable Edge1 (two edges came about alike), and a stable Edge3 sharing
        // its Face1. Read in one call, each edge's refusal offers its own body's Edge3, never
        // the other body's (the candidates are built once a body and kind, not once a kind).
        // Carried, both are gone, each with the same refusal and candidates.
        let features = [
            a_box("aaaaaaaa", 0, 1),
            a_box("bbbbbbbb", 5, 1),
            operation("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
            a_box("xxxxxxxx", 20, 1),
            a_box("yyyyyyyy", 25, 1),
            operation("qqqqqqqq", "fuse", "xxxxxxxx", "yyyyyyyy"),
        ];
        let model = rebuilt(&features, forging(|_| Some(duplicating_fuse())));
        let names = [
            "rrrrrrrr.fuse.Edge(G1/2 aaaaaaaa.box.Face1, G1/2 aaaaaaaa.box.Face6)",
            "qqqqqqqq.fuse.Edge(G1/2 xxxxxxxx.box.Face1, G1/2 xxxxxxxx.box.Face6)",
        ];
        let want = [
            vec!["rrrrrrrr.fuse.Edge(G aaaaaaaa.box.Face1, G aaaaaaaa.box.Face3)".to_owned()],
            vec!["qqqqqqqq.fuse.Edge(G xxxxxxxx.box.Face1, G xxxxxxxx.box.Face3)".to_owned()],
        ];
        let resolved = resolve(&model, &names, "selection").unwrap();
        let candidates: Vec<Vec<String>> = resolved
            .kept
            .iter()
            .map(|s| s.stored.as_ref().unwrap_err().candidates().to_vec())
            .collect();
        assert_eq!(candidates, want);
        let carried = refresh(&model, &names, "selection").unwrap();
        assert!(carried.kept.is_empty(), "{carried:?}");
        let candidates: Vec<Vec<String>> = carried
            .gone
            .iter()
            .map(|g| g.why.errors()[0].candidates().to_vec())
            .collect();
        assert_eq!(candidates, want);
    }

    #[test]
    fn several_unstable_names_of_one_element_are_each_gone_when_carried() {
        // Two numbering fuses, each with a Face1 by its number, unstable, fused by the
        // aliasing fuse so both are one face: its names are both, neither stable. Picked,
        // the face is held once, by the first, flagged; carried, each name is gone, at its
        // own place, neither taken as the other's element.
        let features = [
            a_box("aaaaaaaa", 0, 1),
            a_box("bbbbbbbb", 5, 1),
            operation("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
            a_box("xxxxxxxx", 20, 2),
            a_box("yyyyyyyy", 25, 3),
            operation("qqqqqqqq", "fuse", "xxxxxxxx", "yyyyyyyy"),
            operation("ssssssss", "fuse", "rrrrrrrr", "qqqqqqqq"),
        ];
        // The third fuse's tool is what `qqqqqqqq` gives back: the forged box, made where it
        // is (`FakeKernel::resized`), where every box the document makes but the first is
        // moved into place.
        let model = rebuilt(
            &features,
            forging(|tool| {
                Some(if tool.starts_with("box(") {
                    aliasing_fuse()
                } else {
                    numbering_fuse()
                })
            }),
        );
        let names = ["rrrrrrrr.fuse.Face1", "qqqqqqqq.fuse.Face1"];
        let map = model
            .element_map(&FeatureId::new("ssssssss").unwrap())
            .unwrap();
        let held: Vec<String> = map
            .names(element(ElementKind::Face, 1))
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(held, names);
        let picked = resolve(&model, &names, "selection").unwrap();
        assert_eq!(
            kept(&picked),
            [(
                names[0].to_owned(),
                "ssssssss",
                element(ElementKind::Face, 1),
                None
            )]
        );
        let carried = refresh(&model, &names, "selection").unwrap();
        assert!(carried.kept.is_empty(), "{carried:?}");
        let gone: Vec<(&str, usize)> = carried
            .gone
            .iter()
            .map(|g| (g.name.as_str(), g.place))
            .collect();
        assert_eq!(gone, [(names[0], 0), (names[1], 1)]);
    }

    #[test]
    fn a_pick_read_in_one_features_solid_finds_only_what_that_solid_holds() {
        // A box, a fillet of its Edge3, a second box, and a fuse of the fillet with it: the
        // fuse is the body, and holds the first box's Edge5 (kept by the fillet and the
        // fuse) and the second box's Edge1, by those names. Read in every body, both are in
        // the fuse. Read in the first box's solid, Edge5 is there, the box its body, stored
        // as picked; the second box's edge isn't, `reference_broken` as of that solid; in the
        // fillet's, Edge5 is the fillet's. A repeat is read once.
        let features = [
            a_box("aaaaaaaa", 0, 1),
            fillet("ffffffff", "aaaaaaaa", "aaaaaaaa.box.Edge3"),
            a_box("bbbbbbbb", 5, 1),
            operation("rrrrrrrr", "fuse", "ffffffff", "bbbbbbbb"),
        ];
        let names = [
            "aaaaaaaa.box.Edge5",
            "bbbbbbbb.box.Edge1",
            "aaaaaaaa.box.Edge5",
        ];
        let id = |id: &str| FeatureId::new(id).unwrap();
        let edge = |index| element(ElementKind::Edge, index);
        let stored = |name: &str| Some(name.to_owned());
        let model = model(&features);
        let everywhere = resolve(&model, &names, "selection").unwrap();
        assert_eq!(
            kept(&everywhere),
            [
                (names[0].to_owned(), "rrrrrrrr", edge(5), stored(names[0])),
                // The tool's edges come after the base's 12 in the fake's fuse.
                (names[1].to_owned(), "rrrrrrrr", edge(13), stored(names[1])),
            ]
        );
        let in_box = resolve_in(&model, &names, "selection", &id("aaaaaaaa")).unwrap();
        assert_eq!(
            kept(&in_box),
            [(names[0].to_owned(), "aaaaaaaa", edge(5), stored(names[0]))]
        );
        assert_eq!(
            gone(&in_box),
            [(
                names[1],
                vec![(
                    ErrorCode::ReferenceBroken,
                    "selection.1, \"bbbbbbbb.box.Edge1\", names no element of aaaaaaaa's solid"
                )]
            )]
        );
        assert_eq!(in_box.gone[0].place, 1);
        assert!(in_box.pending.is_empty());
        let in_fillet = resolve_in(&model, &names[..1], "selection", &id("ffffffff")).unwrap();
        assert_eq!(
            kept(&in_fillet),
            [(names[0].to_owned(), "ffffffff", edge(5), stored(names[0]))]
        );
        // Published, its names not named yet: pending, and no map named for it.
        let unnamed = model.unnamed();
        let waiting = resolve_in(&unnamed, &names, "selection", &id("aaaaaaaa")).unwrap();
        assert!(waiting.kept.is_empty() && waiting.gone.is_empty());
        let pending: Vec<(&str, usize)> = waiting
            .pending
            .iter()
            .map(|p| (p.name.as_str(), p.place))
            .collect();
        assert_eq!(pending, [(names[0], 0), (names[1], 1)]);
        for (feature, _) in unnamed.features() {
            assert!(unnamed.named_so_far(feature).is_none(), "{feature} named");
        }
        // A feature with no solid: refused, as `elements` refuses it.
        let refused = resolve_in(&model, &names, "selection", &id("zzzzzzzz")).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::InvalidInput);
        assert_eq!(refused.message(), "the document has no feature zzzzzzzz");
    }
}
