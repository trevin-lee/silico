//! References by name (M2.8, docs/ports/references.md): a fillet's or chamfer's edges, as
//! its feature names them, found in its base's element map, exactly. A name that finds no
//! element, finds one that isn't an edge, or finds one whose name is unstable (N3) is
//! broken: every broken one is said (the first hundred, and how many more), each a
//! `reference_broken` error with the edges it most likely meant as candidates (N4), and
//! the feature fails (N6). Nothing is ever taken in a broken name's place.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::document::{ElementName, FeatureId, PROBLEMS_LISTED};
use crate::kernel::{ElementKind, ElementRef};
use crate::naming::{ElementMap, Maker, Name, OpCode, TypeWord};
use crate::{Error, ErrorCode};

/// A feature's broken references: the first [`PROBLEMS_LISTED`], each its
/// `reference_broken` error, and how many there are, listed or not; or, in their place, a
/// bug in Silico's ([`Broken::bug`]), an `internal` error, which a command reports as its
/// own failure.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Broken {
    pub(crate) errors: Vec<Error>,
    pub(crate) total: u64,
}

impl Broken {
    /// One error, a bug in Silico's, in place of a feature's references.
    pub(crate) fn bug(error: Error) -> Broken {
        Broken {
            errors: vec![error],
            total: 1,
        }
    }
}

/// At most this many candidates for a broken name, or for a sketch's reference to an ID no
/// element holds.
pub(crate) const CANDIDATES: usize = 5;

/// At most this many characters of a reference in its error's `elements`.
const SHOWN: usize = 200;

/// `text` as it is, or its first [`SHOWN`] characters and `…`.
pub(crate) fn cut_short(text: &str) -> String {
    match text.char_indices().nth(SHOWN) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

/// A document's names for the edges a fillet or chamfer lists, by the number each finds in
/// its base: so an error names the edges at fault as the document does, the names as its
/// elements and their places in `edges` in its words, rather than by numbers no document
/// holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EdgeNames(std::collections::BTreeMap<u32, (usize, String)>);

impl EdgeNames {
    /// Edge `edge` named `name`, the `position`th of the feature's `edges`. An edge named
    /// more than once keeps its first name, as the fillet takes it once.
    pub(crate) fn insert(&mut self, edge: u32, position: usize, name: &str) {
        self.0
            .entry(edge)
            .or_insert_with(|| (position, name.to_owned()));
    }

    /// Edge `edge`'s place in `edges` and its name, if the document names it.
    pub(crate) fn get(&self, edge: u32) -> Option<(usize, &str)> {
        self.0
            .get(&edge)
            .map(|(position, name)| (*position, name.as_str()))
    }
}

/// The edge numbers `edges` (feature `feature`'s, in order) name in `map`, each once, with
/// the names that find them (for the kernel's errors to name them as the document does); or
/// the broken ones' errors, the first [`PROBLEMS_LISTED`], each with its candidates, and
/// how many are broken, listed or not (as the gate lists its problems and counts them). `makers` says
/// how each feature makes its elements, to read the names. An edge two names find is
/// filleted once, as FreeCAD's PartDesign takes it (each name still finds it: nothing
/// re-points).
pub(crate) fn resolve(
    feature: &FeatureId,
    edges: &[ElementName],
    map: &ElementMap,
    makers: &dyn Fn(&FeatureId) -> Option<Maker>,
) -> Result<(Vec<u32>, EdgeNames), Broken> {
    let mut found = Vec::with_capacity(edges.len());
    let mut seen = BTreeSet::new();
    let mut names = EdgeNames::default();
    let mut broken = Vec::new();
    let mut more = 0u64;
    // The map's edges by the elements their names are made from, built once, the first
    // time a broken name needs candidates.
    let mut index: Option<Index> = None;
    for (i, edge) in edges.iter().enumerate() {
        let name = Name::parse(edge.as_str(), makers);
        let why = match &name {
            Err(error) => format!("isn't a name: {error}"),
            Ok(name) => match map.element(name) {
                Some(element) if map.is_unstable(name) => format!(
                    "names {}, whose name is unstable (it came about as another element did, or \
                     is its number): an edit could make it another element",
                    element_word(element)
                ),
                Some(element) if element.kind == ElementKind::Edge => {
                    if seen.insert(element.index) {
                        found.push(element.index);
                    }
                    names.insert(element.index, i, edge.as_str());
                    continue;
                }
                Some(element) => format!("names {}, not an edge", element_word(element)),
                None => match retyped(name, makers) {
                    Some((feature, said, made)) => format!(
                        "names an element a {said} made, but {feature} is a {made} now: its \
                         elements are another's"
                    ),
                    None => format!(
                        "names no element of {}'s solid: it isn't one of the elements that \
                         solid is made from, or an edit before this feature removed or remade it",
                        map.feature()
                    ),
                },
            },
        };
        if broken.len() >= PROBLEMS_LISTED {
            more += 1;
            continue;
        }
        let candidates = match &name {
            Ok(name) => index
                .get_or_insert_with(|| Index::of(map, ElementKind::Edge))
                .candidates(name),
            Err(_) => Vec::new(),
        };
        broken.push(
            Error::new(
                ErrorCode::ReferenceBroken,
                format!(
                    "edges.{i}, {}, {why}",
                    crate::document::json::shown(edge.as_str())
                ),
            )
            .with_feature(feature.as_str())
            .with_parameter(format!("edges.{i}"))
            // As written, cut short as the message is: a name can be long, and the
            // parameter says where it is in full.
            .with_element(cut_short(edge.as_str()))
            .with_candidates(candidates)
            .with_suggestion(
                "name the edge as its feature now names it: the candidates are the likeliest, \
                 none taken for it",
            ),
        );
    }
    if broken.is_empty() {
        Ok((found, names))
    } else {
        let total = broken.len() as u64 + more;
        Err(Broken {
            errors: broken,
            total,
        })
    }
}

/// The elements of `kind` a broken `name` most likely meant, across `maps` (a read's
/// bodies): as a fillet's broken edge's candidates are found, in each map, then the most
/// shared first across them all, then by name; at most [`CANDIDATES`].
pub(crate) fn likeliest(maps: &[&ElementMap], name: &Name, kind: ElementKind) -> Vec<String> {
    let mut scored: Vec<(usize, Arc<Name>)> = maps
        .iter()
        .flat_map(|map| Index::of(map, kind).scored(name))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.dedup_by(|a, b| a.1 == b.1);
    scored
        .into_iter()
        .take(CANDIDATES)
        .map(|(_, name)| name.to_string())
        .collect()
}

/// A map's stable elements of a kind, each with the elements its name is made from, and
/// each such element's elements of the kind: so a broken name's candidates cost what its own
/// elements' do.
struct Index {
    edges: Vec<Arc<Name>>,
    by_leaf: std::collections::BTreeMap<Name, Vec<usize>>,
}

impl Index {
    fn of(map: &ElementMap, kind: ElementKind) -> Index {
        let mut index = Index {
            edges: Vec::new(),
            by_leaf: std::collections::BTreeMap::new(),
        };
        for (element, names) in map.iter() {
            let Some(name) = names.first() else { continue };
            if element.kind != kind || map.is_unstable(name) {
                continue;
            }
            let at = index.edges.len();
            index.edges.push(Arc::clone(name));
            for leaf in leaves(name) {
                index.by_leaf.entry(leaf).or_default().push(at);
            }
        }
        index
    }

    /// The edges `name` most likely meant: those whose names share the most of the
    /// elements `name` is made from (a hole moved: the rims generated from the same faces),
    /// most first, then by name; at most [`CANDIDATES`].
    fn candidates(&self, name: &Name) -> Vec<String> {
        let mut scored = self.scored(name);
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        scored
            .into_iter()
            .take(CANDIDATES)
            .map(|(_, edge)| edge.to_string())
            .collect()
    }

    /// Each element sharing any of the elements `name` is made from, with how many.
    fn scored(&self, name: &Name) -> Vec<(usize, Arc<Name>)> {
        let mut shared: std::collections::BTreeMap<usize, usize> =
            std::collections::BTreeMap::new();
        for leaf in leaves(name) {
            for &at in self.by_leaf.get(&leaf).into_iter().flatten() {
                *shared.entry(at).or_default() += 1;
            }
        }
        shared
            .into_iter()
            .filter_map(|(at, count)| self.edges.get(at).map(|edge| (count, Arc::clone(edge))))
            .collect()
    }
}

/// A feature `name` names an element of (itself, or one it's made from, the nearest first),
/// the type `name` says it is, and the type it is now, if they differ (a box made a
/// cylinder, a cut made a common, under its ID).
fn retyped<'a>(
    name: &'a Name,
    makers: &dyn Fn(&FeatureId) -> Option<Maker>,
) -> Option<(&'a FeatureId, &'static str, &'static str)> {
    let mut stack = vec![name];
    while let Some(name) = stack.pop() {
        let (feature, said) = match name {
            Name::Element { feature, made, .. } => (feature, *made),
            Name::Sketch { feature, .. } => (feature, TypeWord::Operation(OpCode::Sketch)),
            Name::Derived {
                feature, operation, ..
            }
            | Name::Upper {
                feature, operation, ..
            }
            | Name::Lower {
                feature, operation, ..
            } => (feature, TypeWord::Operation(*operation)),
            Name::Duplicate { name, .. } | Name::Modified { name, .. } => {
                stack.push(name);
                continue;
            }
        };
        match makers(feature).map(Maker::type_word) {
            Some(now) if now != said => return Some((feature, said.word(), now.word())),
            _ => {}
        }
        // The parts after, the first nearest the top.
        stack.extend(name.parts().into_iter().rev().map(Arc::as_ref));
    }
    None
}

/// `a face`, `an edge`, `a vertex`, `a solid`.
fn element_word(element: ElementRef) -> &'static str {
    match element.kind {
        ElementKind::Face => "a face",
        ElementKind::Edge => "an edge",
        ElementKind::Vertex => "a vertex",
        ElementKind::Solid => "a solid",
    }
}

/// The elements a name is made from, all the way down (itself, if it's one): each named by
/// its number, or a sketch's by its ID.
fn leaves(name: &Name) -> BTreeSet<Name> {
    let mut found = BTreeSet::new();
    let mut stack: Vec<&Name> = vec![name];
    while let Some(name) = stack.pop() {
        match name {
            Name::Element { .. } | Name::Sketch { .. } => {
                found.insert(name.clone());
            }
            _ => stack.extend(name.parts().into_iter().map(Arc::as_ref)),
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{
        ElementCounts, ElementFate, History, InputHistory, Primitive, Reported, Topology,
    };
    use crate::naming::{OpCode, PrimitiveKind};

    fn id(text: &str) -> FeatureId {
        FeatureId::new(text).unwrap()
    }

    fn makers(feature: &FeatureId) -> Option<Maker> {
        match feature.as_str() {
            "aaaaaaaa" | "bbbbbbbb" => Some(Maker::Itself(PrimitiveKind::Box)),
            "rrrrrrrr" => Some(Maker::By(OpCode::Fuse)),
            _ => None,
        }
    }

    fn cube(feature: &str) -> ElementMap {
        ElementMap::primitive(
            id(feature),
            &Primitive::Box {
                dx: 1.0,
                dy: 1.0,
                dz: 1.0,
            },
        )
    }

    fn resolved(map: &ElementMap, edges: &[&str]) -> Result<Vec<u32>, Vec<Error>> {
        let edges: Vec<ElementName> = edges.iter().map(|e| ElementName::new(e).unwrap()).collect();
        resolve(&id("ffffffff"), &edges, map, &makers)
            .map(|(found, _)| found)
            .map_err(|broken| {
                assert_eq!(broken.total, broken.errors.len() as u64);
                broken.errors
            })
    }

    fn reported(kind: ElementKind, index: u32) -> Reported {
        Reported {
            element: ElementRef { kind, index },
            position: 1,
        }
    }

    /// A fuse (`rrrrrrrr`) of a cube (`aaaaaaaa`) whose result has one face, generated from
    /// the cube's Face1, and edges each generated from the cube's faces `edges[n]`, each at
    /// the first place in its source's list.
    fn generated(edges: &[&[u32]]) -> ElementMap {
        let mut faces = vec![ElementFate::default(); 6];
        faces[0].generated.push(reported(ElementKind::Face, 1));
        for (n, from) in (1..).zip(edges) {
            for &face in *from {
                faces[face as usize - 1]
                    .generated
                    .push(reported(ElementKind::Edge, n));
            }
        }
        for fate in &mut faces {
            fate.generated.sort_by_key(|r| r.element);
        }
        let history = History {
            inputs: vec![InputHistory {
                faces,
                edges: vec![ElementFate::default(); 12],
                vertices: vec![ElementFate::default(); 8],
            }],
            result: ElementCounts {
                faces: 1,
                edges: edges.len() as u32,
                vertices: 0,
                solids: 1,
            },
            ends: Default::default(),
            topology: Topology {
                face_edges: vec![vec![]],
                outer_edges: vec![vec![]],
                edge_vertices: vec![vec![]; edges.len()],
            },
        };
        history.validate(1).unwrap();
        ElementMap::operation(id("rrrrrrrr"), OpCode::Fuse, &[&cube("aaaaaaaa")], &history)
    }

    fn edge(faces: &[u32]) -> String {
        let sources: Vec<String> = faces
            .iter()
            .map(|f| format!("G aaaaaaaa.box.Face{f}"))
            .collect();
        format!("rrrrrrrr.fuse.Edge({})", sources.join(", "))
    }

    #[test]
    fn a_name_two_elements_came_about_by_is_never_taken() {
        // Found by the tests review: the two edges generated alike from Face1 and Face6, at
        // one place in each list, share a rank and so a name, the second `~1` (N3). Neither
        // name is taken, though each finds an edge in the map: the next rebuild could give
        // either to the other.
        let map = generated(&[&[1, 3], &[1, 6], &[1, 6]]);
        let duplicated =
            "rrrrrrrr.fuse.Edge(G1/2 aaaaaaaa.box.Face1, G1/2 aaaaaaaa.box.Face6)".to_owned();
        let second = format!("{duplicated}~1");
        for name in [&duplicated, &second] {
            assert!(map.element(&Name::parse(name, &makers).unwrap()).is_some());
            let errors = resolved(&map, &[name]).unwrap_err();
            assert_eq!(errors.len(), 1, "{name}");
            assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
            assert!(errors[0].message().contains("unstable"), "{}", errors[0]);
            // Nor offered: the stable edge shares Face1 with it.
            assert_eq!(errors[0].candidates(), [edge(&[1, 3])], "{name}");
        }
        assert_eq!(resolved(&map, &[&edge(&[1, 3])]), Ok(vec![1]));
    }

    #[test]
    fn a_broken_names_candidates_are_stable_edges_sharing_the_most_then_by_name_five_at_most() {
        // Found by the tests review: nothing pinned the candidates. The broken name is made
        // from Face1 and Face2. The edge from Face1, Face2, and Face3 shares both: first.
        // Then those sharing one, in name order, to five; not the face generated from Face1,
        // nor the two edges made alike (unstable), though they share Face1 too.
        let map = generated(&[
            &[1, 3],
            &[1, 4],
            &[1, 5],
            &[2, 3],
            &[2, 4],
            &[1, 2, 3],
            &[1, 6],
            &[1, 6],
        ]);
        let errors = resolved(&map, &[&edge(&[1, 2])]).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].candidates(),
            [
                edge(&[1, 2, 3]),
                edge(&[1, 3]),
                edge(&[1, 4]),
                edge(&[1, 5]),
                edge(&[2, 3]),
            ]
        );
    }

    #[test]
    fn a_candidate_sharing_more_comes_first_though_its_name_sorts_after() {
        // Found by mutation testing: the candidates test's order was name order too, so a
        // count of shared elements that stayed 0 passed. The broken name is made from Face5
        // and Face6; the edge from Face1, Face5, and Face6 shares both, the edge from Face1
        // and Face5 one, though it sorts first by name.
        let map = generated(&[&[1, 5], &[1, 5, 6]]);
        let errors = resolved(&map, &[&edge(&[5, 6])]).unwrap_err();
        assert!(edge(&[1, 5]) < edge(&[1, 5, 6]));
        assert_eq!(errors[0].candidates(), [edge(&[1, 5, 6]), edge(&[1, 5])]);
    }

    #[test]
    fn a_nested_broken_names_candidates_share_the_elements_it_is_made_from_all_the_way_down() {
        // Found by the milestone's tests review: every broken name tested was made of plain
        // elements, so candidates by a name's direct parts passed. This one is made from a
        // face made from Face1 and Face2, and from Face3: the edge from Face1 and Face3
        // shares two of those, the one from Face2 and Face4 one, the one from Face5 and
        // Face6 none.
        let map = generated(&[&[1, 3], &[2, 4], &[5, 6]]);
        let nested = "rrrrrrrr.fuse.Edge(G rrrrrrrr.fuse.Face(G aaaaaaaa.box.Face1, G aaaaaaaa.box.Face2), \
                      G aaaaaaaa.box.Face3)";
        let errors = resolved(&map, &[nested]).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].candidates(), [edge(&[1, 3]), edge(&[2, 4])]);
    }

    #[test]
    fn each_edge_found_is_named_by_its_place_among_the_names_not_among_the_edges() {
        // Found by the tests review: an edge named twice is taken once, so the edges found
        // are fewer than the names; an error about the edge after must say its own place in
        // `edges` (2), not its place among the edges found (1).
        let map = generated(&[&[1, 3], &[1, 4]]);
        let (a, b) = (edge(&[1, 3]), edge(&[1, 4]));
        let edges: Vec<ElementName> = [&a, &a, &b]
            .iter()
            .map(|e| ElementName::new(e).unwrap())
            .collect();
        let Ok((found, names)) = resolve(&id("ffffffff"), &edges, &map, &makers) else {
            panic!("broken");
        };
        assert_eq!(found, [1, 2]);
        let mut expected = EdgeNames::default();
        expected.insert(1, 0, &a);
        expected.insert(2, 2, &b);
        assert_eq!(names, expected);
    }

    #[test]
    fn a_broken_reference_says_what_it_found() {
        // Found by mutation testing: the element a broken name found wasn't said in a
        // tested message. A face's name: "names a face, not an edge"; a duplicated edge's:
        // "names an edge, whose name is unstable".
        let map = generated(&[&[1, 6], &[1, 6]]);
        let face = "rrrrrrrr.fuse.Face(G aaaaaaaa.box.Face1)";
        let errors = resolved(&map, &[face]).unwrap_err();
        assert!(
            errors[0].message().contains("names a face, not an edge"),
            "{}",
            errors[0]
        );
        let duplicated = "rrrrrrrr.fuse.Edge(G1/2 aaaaaaaa.box.Face1, G1/2 aaaaaaaa.box.Face6)";
        let errors = resolved(&map, &[duplicated]).unwrap_err();
        assert!(
            errors[0]
                .message()
                .contains("names an edge, whose name is unstable"),
            "{}",
            errors[0]
        );
    }

    #[test]
    fn an_edge_two_names_find_is_filleted_once() {
        // Found by the tests review: two cubes fused where one edge of each is the same edge
        // of the result, so it has both names (N2). Named both ways, it's one edge in the
        // request, as FreeCAD's PartDesign takes it, and either name alone finds it.
        let kept = |index| ElementFate {
            kept: Some(index),
            ..ElementFate::default()
        };
        let input = || InputHistory {
            faces: vec![ElementFate::default(); 6],
            edges: vec![kept(1)],
            vertices: vec![],
        };
        let history = History {
            inputs: vec![input(), input()],
            result: ElementCounts {
                faces: 0,
                edges: 1,
                vertices: 0,
                solids: 1,
            },
            ends: Default::default(),
            topology: Topology {
                face_edges: vec![],
                outer_edges: vec![],
                edge_vertices: vec![vec![]],
            },
        };
        history.validate(2).unwrap();
        let map = ElementMap::operation(
            id("rrrrrrrr"),
            OpCode::Fuse,
            &[&cube("aaaaaaaa"), &cube("bbbbbbbb")],
            &history,
        );
        let (a, b) = ("aaaaaaaa.box.Edge1", "bbbbbbbb.box.Edge1");
        assert_eq!(resolved(&map, &[a, b]), Ok(vec![1]));
        assert_eq!(resolved(&map, &[b]), Ok(vec![1]));
        assert_eq!(resolved(&map, &[a]), Ok(vec![1]));
    }
}
