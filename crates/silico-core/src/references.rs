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
use crate::naming::{ElementMap, Maker, Name};
use crate::{Error, ErrorCode};

/// At most this many candidates for a broken name.
const CANDIDATES: usize = 5;

/// At most this many characters of a reference in its error's `elements`.
const SHOWN: usize = 200;

/// `text` as it is, or its first [`SHOWN`] characters and `…`.
fn cut_short(text: &str) -> String {
    match text.char_indices().nth(SHOWN) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

/// The edge numbers `edges` (feature `feature`'s, in order) name in `map`, each once; or
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
) -> Result<Vec<u32>, (Vec<Error>, u64)> {
    let mut found = Vec::with_capacity(edges.len());
    let mut seen = BTreeSet::new();
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
                    continue;
                }
                Some(element) => format!("names {}, not an edge", element_word(element)),
                None => match retyped(name, makers) {
                    Some((feature, said, made)) => format!(
                        "names an element of a {said}, but {feature} is a {made} now: its \
                         elements are another primitive's"
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
            Ok(name) => index.get_or_insert_with(|| Index::of(map)).candidates(name),
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
        Ok(found)
    } else {
        let total = broken.len() as u64 + more;
        Err((broken, total))
    }
}

/// A map's stable edges, each with the elements its name is made from, and each such
/// element's edges: so a broken name's candidates cost what its own elements' edges do.
struct Index {
    edges: Vec<Arc<Name>>,
    by_leaf: std::collections::BTreeMap<Name, Vec<usize>>,
}

impl Index {
    fn of(map: &ElementMap) -> Index {
        let mut index = Index {
            edges: Vec::new(),
            by_leaf: std::collections::BTreeMap::new(),
        };
        for (element, names) in map.iter() {
            let Some(name) = names.first() else { continue };
            if element.kind != ElementKind::Edge || map.is_unstable(name) {
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
        let mut shared: std::collections::BTreeMap<usize, usize> =
            std::collections::BTreeMap::new();
        for leaf in leaves(name) {
            for &at in self.by_leaf.get(&leaf).into_iter().flatten() {
                *shared.entry(at).or_default() += 1;
            }
        }
        let mut scored: Vec<(usize, &Arc<Name>)> = shared
            .into_iter()
            .filter_map(|(at, count)| self.edges.get(at).map(|edge| (count, edge)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        scored
            .into_iter()
            .take(CANDIDATES)
            .map(|(_, edge)| edge.to_string())
            .collect()
    }
}

/// The primitive `name` names an element of, the type `name` says it is, and the type it
/// is now, if they differ (a box made a cylinder under its ID).
fn retyped<'a>(
    name: &'a Name,
    makers: &dyn Fn(&FeatureId) -> Option<Maker>,
) -> Option<(&'a FeatureId, &'static str, &'static str)> {
    match name {
        Name::Element {
            feature,
            primitive: Some(said),
            ..
        } => match makers(feature) {
            Some(Maker::Itself(made)) if made != *said => Some((feature, said.word(), made.word())),
            _ => None,
        },
        _ => None,
    }
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
/// its number.
fn leaves(name: &Name) -> BTreeSet<Name> {
    let mut found = BTreeSet::new();
    let mut stack: Vec<&Name> = vec![name];
    while let Some(name) = stack.pop() {
        match name {
            Name::Element { .. } => {
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
        resolve(&id("ffffffff"), &edges, map, &makers).map_err(|(errors, total)| {
            assert_eq!(total, errors.len() as u64);
            errors
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
        format!("rrrrrrrr.Edge({})", sources.join(", "))
    }

    #[test]
    fn a_name_two_elements_came_about_by_is_never_taken() {
        // Found by the tests review: the two edges generated alike from Face1 and Face6, at
        // one place in each list, share a rank and so a name, the second `~1` (N3). Neither
        // name is taken, though each finds an edge in the map: the next rebuild could give
        // either to the other.
        let map = generated(&[&[1, 3], &[1, 6], &[1, 6]]);
        let duplicated =
            "rrrrrrrr.Edge(G1/2 aaaaaaaa.box.Face1, G1/2 aaaaaaaa.box.Face6)".to_owned();
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
