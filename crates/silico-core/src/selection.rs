//! A selection's rules that are the model's (M5.7, docs/design/m5.0-app.md section 9's
//! *Selection*): a selection is a list of element names, each with its stability, and this
//! reads one against a model. Each name is found in the model's bodies as every command
//! that reads elements by name finds it (`command::lookup`), and, found, said stable or
//! not as `elements` says it (`elements::stable_name`: its own name or an alias stable);
//! one that isn't is held, but what a store would say of it is said with it, in the words a
//! fillet's or chamfer's reference refuses it with (`references`, N3; M4.1 section 2). A
//! name not found is gone, with every reason the lookup gives: a name that isn't one, one
//! of no live solid's element, or one of a feature that didn't build (that feature's
//! failure).
//!
//! After a rebuild, the selection is read again against the newly published model: what's
//! kept, each with its stability now, and what's gone, with a word, for the app to clear and
//! say (M5.7's *After a rebuild*). Which names are selected, and how (clicks, Ctrl, a box,
//! filters, Esc), is the app's own state, not this.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::Error;
use crate::command::Problems;
use crate::command::lookup::{Kinds, Lookup};
use crate::document::FeatureId;
use crate::elements::stable_name;
use crate::kernel::{ElementKind, ElementRef};
use crate::naming::Name;
use crate::recompute::Model;
use crate::references::{Candidates, refused, unstable};

/// The kinds a selection holds by name, and what it says of a name of another (a solid's).
const SELECTED: Kinds = Kinds {
    taken: &[ElementKind::Face, ElementKind::Edge, ElementKind::Vertex],
    why: "a selection is of faces, edges, and vertices",
    suggestion: "select a face, an edge, or a vertex of a body",
};

/// A selection read against a model: the names found, in the selection's order, and those
/// gone, in its order too. A name given more than once is read once, at its first place;
/// a name of an element an earlier name found already is dropped (an element is selected
/// or not, however many of its names are given).
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub kept: Vec<Selected>,
    pub gone: Vec<Gone>,
}

/// A selected element, found by its name.
#[derive(Debug, Clone, PartialEq)]
pub struct Selected {
    /// The name as selected.
    pub name: Arc<Name>,
    /// The feature whose solid it's of: the body it's in.
    pub feature: FeatureId,
    /// The element, in that solid's numbering.
    pub element: ElementRef,
    /// The name a store (a fillet's edge, a sketch's face) writes for it: the name selected
    /// if it's stable, or else its own or an alias that is (`elements::stable_name`); or,
    /// if none of its names is stable, the store's refusal: `reference_broken`, saying why
    /// as a fillet's reference does, with the stable names it most likely meant as
    /// candidates, at the parameter `selection.<n>`, its place in the selection.
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

/// A selected name the model doesn't find, and why, as the lookup says it (each reason an
/// error at the parameter `selection.<n>`, its place in the selection).
#[derive(Debug, Clone, PartialEq)]
pub struct Gone {
    /// The name as selected.
    pub name: String,
    pub why: Problems,
}

/// `names`, a selection, read against `model`: on its own, or, after a rebuild, against the
/// newly published model, whose `kept` are what survives and whose `gone` are what the app
/// clears, saying why. Each name is looked for in the model's bodies, as `distance`,
/// `angle`, and `render`'s highlights look ([`Lookup`]).
pub fn resolve<S: AsRef<str>>(model: &Model, names: &[S]) -> Resolved {
    let _span = tracing::info_span!("selection_resolve", names = names.len()).entered();
    let mut lookup = Lookup::each_name(model, SELECTED);
    let mut resolved = Resolved {
        kept: Vec::new(),
        gone: Vec::new(),
    };
    let mut read: BTreeSet<&str> = BTreeSet::new();
    let mut found: BTreeSet<(&FeatureId, ElementRef)> = BTreeSet::new();
    // Each solid's candidates of each kind, built once, the first time a name of it needs
    // them, as a reference's are.
    let mut candidates: BTreeMap<(&FeatureId, ElementKind), Candidates> = BTreeMap::new();
    for (place, text) in names.iter().map(AsRef::as_ref).enumerate() {
        if !read.insert(text) {
            continue;
        }
        let parameter = format!("selection.{place}");
        let Some((feature, map, element, name)) = lookup.found(&parameter, text) else {
            resolved.gone.push(Gone {
                name: text.to_owned(),
                why: lookup.take(),
            });
            continue;
        };
        if !found.insert((feature, element)) {
            continue;
        }
        let name = Arc::new(name);
        let stored = if map.is_unstable(&name) {
            stable_name(map, element).ok_or_else(|| {
                let candidates = candidates
                    .entry((feature, element.kind))
                    .or_insert_with(|| Candidates::of(&[map], element.kind))
                    .of_name(&name);
                refused(
                    &parameter,
                    text,
                    &unstable(element.kind),
                    element.kind,
                    candidates,
                )
            })
        } else {
            Ok(Arc::clone(&name))
        };
        resolved.kept.push(Selected {
            name,
            feature: feature.clone(),
            element,
            stored,
        });
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorCode;
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

    /// `features` rebuilt on a fake kernel `setup` has prepared, and published.
    fn rebuilt(features: &[String], setup: impl FnOnce(&FakeKernel)) -> Arc<Model> {
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
        let resolved = resolve(&before, &names);
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
        let refreshed = resolve(&after, &names);
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
        let resolved = resolve(&model(&boxes), &names);
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
        let refreshed = resolve(&model(&cut), &names);
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
        let removed = resolve(&model(&boxes[..1]), &names);
        assert_eq!(kept(&removed).len(), 1);
        let why: Vec<Vec<(ErrorCode, &str)>> =
            gone(&removed).into_iter().map(|(_, why)| why).collect();
        assert_eq!(
            why,
            [
                vec![(
                    ErrorCode::ReferenceBroken,
                    r#"selection.1, "b0000001.box.Face2", isn't a name: no feature b0000001 before this one, at column 1"#
                )],
                vec![(
                    ErrorCode::ReferenceBroken,
                    r#"selection.2, "b0000001.box.Edge3", isn't a name: no feature b0000001 before this one, at column 1"#
                )],
            ]
        );
    }

    #[test]
    fn each_name_of_a_feature_that_failed_is_gone_with_that_failure() {
        // A box the kernel refuses (2 mm long) has no solid: each of its names selected is
        // gone, each with the box's failure, its own copy, not said once for the first.
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
        let resolved = resolve(&model, &names);
        assert_eq!(kept(&resolved).len(), 1);
        assert_eq!(resolved.kept[0].feature.as_str(), "a0000001");
        assert_eq!(resolved.gone.len(), 2);
        for (gone, name) in resolved.gone.iter().zip([names[0], names[2]]) {
            assert_eq!(gone.name, name);
            let failure: Vec<(ErrorCode, Option<&str>)> = gone
                .why
                .errors()
                .iter()
                .map(|e| (e.code(), e.feature()))
                .collect();
            assert_eq!(failure, [(ErrorCode::KernelFailed, Some("e0000001"))]);
        }
    }

    #[test]
    fn a_malformed_name_and_a_solids_name_are_gone_each_saying_why() {
        // Not a name: the lookup's `reference_broken`, "isn't a name", with nothing offered.
        // A solid's name parses, but a selection holds faces, edges, and vertices.
        let model = model(&[a_box("k7f2qx3m", 0, 1)]);
        let resolved = resolve(
            &model,
            &["k7f2qx3m.box", "k7f2qx3m.box.Face1", "k7f2qx3m.box.Solid1"],
        );
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
            },
        };
        history.validate(2).unwrap();
        history
    }

    /// A fake whose fuses of a tool `history` picks give the base back with that history.
    fn forging(
        history: impl Fn(&str) -> Option<History> + Send + Sync + 'static,
    ) -> impl FnOnce(&FakeKernel) {
        move |kernel: &FakeKernel| {
            kernel.answer_instead(move |op| match op {
                Op::Boolean {
                    op: BooleanOp::Fuse,
                    base,
                    tool,
                } => history(&FakeKernel::describe(tool)?).map(|history| {
                    silico_protocol::Reply::Built(BuiltData {
                        shape: base.clone(),
                        history,
                    })
                }),
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
        let resolved = resolve(&model, &["aaaaaaaa.box.Face2", "rrrrrrrr.fuse.Face1"]);
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
        let resolved = resolve(&before, &[edge1]);
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
        // name, it's stable, and a store writes the alias; selected by the alias, the alias.
        // Both names selected are one element: taken once, at the first.
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
        let by_own = resolve(&model, &[own, alias]);
        assert_eq!(
            kept(&by_own),
            [(own.to_owned(), "ssssssss", face1, Some(alias.to_owned()))]
        );
        assert!(by_own.kept[0].stable());
        assert!(by_own.gone.is_empty());
        let by_alias = resolve(&model, &[alias, own]);
        assert_eq!(
            kept(&by_alias),
            [(alias.to_owned(), "ssssssss", face1, Some(alias.to_owned()))]
        );
        // Its Edge1, by its number in the first fuse and no other name: unstable still.
        let edge = resolve(&model, &["rrrrrrrr.fuse.Edge1"]);
        assert!(!edge.kept[0].stable());
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
            kept(&resolve(&model, &[alias])),
            [(
                alias.to_owned(),
                "ssssssss",
                element(ElementKind::Face, 1),
                Some(alias.to_owned())
            )]
        );
        let own = "aaaaaaaa.box.Face1";
        assert_eq!(kept(&resolve(&model, &[own]))[0].3.as_deref(), Some(own));
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
        let resolved = resolve(&model, &names);
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
        assert_eq!(resolve(&model, &names), resolved);
    }
}
