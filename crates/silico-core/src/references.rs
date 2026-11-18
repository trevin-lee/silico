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
use crate::{Error, ErrorCode, Nearest, Rank};

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

/// What a stored reference's name finds of an element of one kind in some solids' maps
/// ([`look_up`]): a fillet's edge in its base's, a face sketch's face in the solids live at
/// its place.
#[derive(Debug)]
pub(crate) enum Lookup {
    /// An element of the kind looked for, in the map at place `map` among those searched,
    /// and the name that found it.
    Found {
        map: usize,
        element: ElementRef,
        name: Name,
    },
    /// It isn't a name, or it finds an element no reference may take (by an unstable name,
    /// N3, or of another kind): why, and the name, if it's one, for its candidates.
    Refused { why: String, name: Option<Name> },
    /// It finds nothing in any of the maps: the name, and, if one of the features it names
    /// is of another type now, why in those terms (the caller words the rest, as its maps
    /// are).
    Missing { name: Name, retyped: Option<String> },
}

/// What the stored name `text` finds of an element of `kind` in `maps`, searched in order,
/// the first that holds it deciding: the one step every stored reference takes (a fillet's
/// or chamfer's edges, a face sketch's face), so they read a name, refuse an unstable one
/// or one of another kind, and say a retyped feature, alike. `makers` says how each feature
/// makes its elements, to read the name.
pub(crate) fn look_up(
    text: &str,
    kind: ElementKind,
    maps: &[&ElementMap],
    makers: &dyn Fn(&FeatureId) -> Option<Maker>,
) -> Lookup {
    let name = match Name::parse(text, makers) {
        Ok(name) => name,
        Err(error) => {
            return Lookup::Refused {
                why: format!("isn't a name: {error}"),
                name: None,
            };
        }
    };
    for (at, map) in maps.iter().enumerate() {
        let Some(element) = map.element(&name) else {
            continue;
        };
        let why = if map.is_unstable(&name) {
            unstable(element.kind)
        } else if element.kind != kind {
            format!("names {}, not {}", element.kind.a_word(), kind.a_word())
        } else {
            return Lookup::Found {
                map: at,
                element,
                name,
            };
        };
        return Lookup::Refused {
            why,
            name: Some(name),
        };
    }
    let retyped = retyped(&name, makers).map(|(feature, said, made)| {
        format!(
            "names an element a {said} made, but {feature} is a {made} now: its elements are \
             another's"
        )
    });
    Lookup::Missing { name, retyped }
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
    // The base's edges by the elements their names are made from, built once, the first
    // time a broken name needs candidates.
    let mut index: Option<Candidates> = None;
    let mut seen = BTreeSet::new();
    let mut names = EdgeNames::default();
    let mut broken = Vec::new();
    let mut more = 0u64;
    for (i, edge) in edges.iter().enumerate() {
        let (why, name) = match look_up(edge.as_str(), ElementKind::Edge, &[map], makers) {
            Lookup::Found { element, .. } => {
                if seen.insert(element.index) {
                    found.push(element.index);
                }
                names.insert(element.index, i, edge.as_str());
                continue;
            }
            Lookup::Refused { why, name } => (why, name),
            Lookup::Missing { name, retyped } => (
                retyped.unwrap_or_else(|| {
                    format!(
                        "names no element of {}'s solid: it isn't one of the elements that \
                         solid is made from, or an edit before this feature removed or remade it",
                        map.feature()
                    )
                }),
                Some(name),
            ),
        };
        if broken.len() >= PROBLEMS_LISTED {
            more += 1;
            continue;
        }
        let candidates = match &name {
            Some(name) => index
                .get_or_insert_with(|| Candidates::of(&[map], ElementKind::Edge))
                .of_name(name),
            None => Vec::new(),
        };
        broken.push(
            refused(
                &format!("edges.{i}"),
                edge.as_str(),
                &why,
                ElementKind::Edge,
            )
            .with_ranked(candidates)
            .with_feature(feature.as_str()),
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

/// Why a stored reference refuses a name that finds an element of `kind` but is unstable
/// (N3, `ElementMap::is_unstable`): a fillet's or chamfer's edge, and a selection's element
/// none of whose names is stable (`selection`), say it alike.
pub(crate) fn unstable(kind: ElementKind) -> String {
    format!(
        "names {}, whose name is unstable (it came about as another element did, or is its \
         number): an edit could make it another element",
        kind.a_word()
    )
}

/// A stored reference's refusal of the name `text`, at `parameter` (`edges.0`): a
/// `reference_broken` error saying `why`, and the advice to name the element, of `kind`, as
/// its feature now names it, none of the candidates taken. The caller gives the candidates:
/// ranked ([`Candidates::of_name`], [`Error::with_ranked`]), or by a rule of their own.
pub(crate) fn refused(parameter: &str, text: &str, why: &str, kind: ElementKind) -> Error {
    Error::new(
        ErrorCode::ReferenceBroken,
        format!("{parameter}, {}, {why}", crate::document::json::shown(text)),
    )
    .with_parameter(parameter)
    // As written, cut short as the message is: a name can be long, and the parameter says
    // where it is in full.
    .with_element(cut_short(text))
    .with_suggestion(format!(
        "name the {} as its feature now names it: the candidates are the likeliest, none \
         taken for it",
        kind.word()
    ))
}

/// Candidates for broken names, of the elements of one kind in some maps (a fillet's base,
/// or a measure's live solids): each map's index built once, as it's made, so each broken
/// name's candidates cost what its own elements' do, however many names are broken.
pub(crate) struct Candidates {
    indexes: Vec<Index>,
}

impl Candidates {
    pub(crate) fn of(maps: &[&ElementMap], kind: ElementKind) -> Candidates {
        Candidates {
            indexes: maps.iter().map(|map| Index::of(map, kind)).collect(),
        }
    }

    /// The elements a broken `name` most likely meant, as one list, each with why it's
    /// where it is ([`Rank`]): those whose names share the most of what it's made from,
    /// then those of `name`'s own feature, then by name; or, if none shares any, each map's
    /// nearest by number ([`Index::nearest`]), by name (those of `name`'s own feature
    /// first, as its elements' names sort first); at most [`CANDIDATES`].
    pub(crate) fn of_name(&self, name: &Name) -> Vec<(String, Rank)> {
        let own = leaves(name);
        let rank = |named: &Name, nearest: Option<Nearest>| {
            let theirs = leaves(named);
            let (shared, unshared): (Vec<&Name>, Vec<&Name>) =
                own.iter().partition(|leaf| theirs.contains(*leaf));
            Rank {
                shared: shared.into_iter().map(ToString::to_string).collect(),
                unshared: unshared.into_iter().map(ToString::to_string).collect(),
                same_feature: named.feature() == name.feature(),
                nearest,
            }
        };
        let mut scored: Vec<(usize, bool, &Arc<Name>)> = self
            .indexes
            .iter()
            .flat_map(|index| index.scored(name))
            .map(|(count, named)| (count, named.feature() != name.feature(), named))
            .collect();
        if scored.is_empty() {
            let mut nearest: Vec<(&Arc<Name>, &Name, u32)> =
                self.indexes.iter().flat_map(|i| i.nearest(name)).collect();
            nearest.sort();
            nearest.dedup_by(|a, b| a.0 == b.0);
            return nearest
                .into_iter()
                .take(CANDIDATES)
                .map(|(named, leaf, gap)| {
                    let nearest = Nearest {
                        element: leaf.to_string(),
                        gap: u64::from(gap),
                    };
                    (named.to_string(), rank(named, Some(nearest)))
                })
                .collect();
        }
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(b.2))
        });
        scored.dedup_by(|a, b| a.2 == b.2);
        scored
            .into_iter()
            .take(CANDIDATES)
            .map(|(_, _, named)| (named.to_string(), rank(named, None)))
            .collect()
    }
}

/// A map's stable elements of one kind, each with the elements its name is made from, and
/// each such element's elements: so a broken name's candidates cost what its own elements'
/// do.
struct Index {
    names: Vec<Arc<Name>>,
    by_leaf: std::collections::BTreeMap<Name, Vec<usize>>,
}

impl Index {
    fn of(map: &ElementMap, kind: ElementKind) -> Index {
        let mut index = Index {
            names: Vec::new(),
            by_leaf: std::collections::BTreeMap::new(),
        };
        for (element, names) in map.iter() {
            let Some(name) = names.first() else { continue };
            if element.kind != kind || map.is_unstable(name) {
                continue;
            }
            let at = index.names.len();
            index.names.push(Arc::clone(name));
            for leaf in leaves(name) {
                index.by_leaf.entry(leaf).or_default().push(at);
            }
        }
        index
    }

    /// This map's names that share any of the elements `name` is made from, each with how
    /// many it shares, unsorted.
    fn scored(&self, name: &Name) -> Vec<(usize, &Arc<Name>)> {
        let mut shared: std::collections::BTreeMap<usize, usize> =
            std::collections::BTreeMap::new();
        for leaf in leaves(name) {
            for &at in self.by_leaf.get(&leaf).into_iter().flatten() {
                *shared.entry(at).or_default() += 1;
            }
        }
        shared
            .into_iter()
            .filter_map(|(at, count)| self.names.get(at).map(|named| (count, named)))
            .collect()
    }

    /// For `name`, an element by its number that names nothing (`box.Face7`), the elements
    /// made from one of the same feature's own elements of its kind numbered nearest it
    /// (`box.Face6`): each as its own name, with that element (the first by name, if it's
    /// made from more than one so near) and how far it's numbered from `name`'s, by name,
    /// every one (its caller cuts the list). None for any other name.
    fn nearest(&self, name: &Name) -> Vec<(&Arc<Name>, &Name, u32)> {
        let Name::Element {
            feature,
            made,
            element,
        } = name
        else {
            return Vec::new();
        };
        let gap = |leaf: &Name| match leaf {
            Name::Element {
                feature: f,
                made: m,
                element: e,
            } if f == feature && m == made && e.kind == element.kind => {
                Some(e.index.abs_diff(element.index))
            }
            _ => None,
        };
        let Some(least) = self.by_leaf.keys().filter_map(gap).min() else {
            return Vec::new();
        };
        let mut found: Vec<(&Arc<Name>, &Name, u32)> = self
            .by_leaf
            .iter()
            .filter(|(leaf, _)| gap(leaf) == Some(least))
            .flat_map(|(leaf, at)| {
                at.iter()
                    .filter_map(|&at| self.names.get(at))
                    .map(move |named| (named, leaf, least))
            })
            .collect();
        found.sort();
        found.dedup_by(|a, b| a.0 == b.0);
        found
    }
}

/// A feature `name` names an element of (itself, or one it's made from, the nearest first),
/// the type `name` says it is, and the type it is now, if they differ (a box made a
/// cylinder, a cut made a common, under its ID).
pub(crate) fn retyped<'a>(
    name: &'a Name,
    makers: &dyn Fn(&FeatureId) -> Option<Maker>,
) -> Option<(&'a FeatureId, &'static str, &'static str)> {
    let mut stack = vec![name];
    while let Some(name) = stack.pop() {
        // A duplicate or a modified element is its name's: it says no type of its own.
        if let Name::Duplicate { name, .. } | Name::Modified { name, .. } = name {
            stack.push(name);
            continue;
        }
        let (feature, said) = (name.feature(), name.type_word());
        match makers(feature).map(Maker::type_word) {
            Some(now) if now != said => return Some((feature, said.word(), now.word())),
            _ => {}
        }
        // The parts after, the first nearest the top.
        stack.extend(name.parts().into_iter().rev().map(Arc::as_ref));
    }
    None
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
        ElementCounts, ElementFate, ElementRef, History, InputHistory, Primitive, Reported,
        Topology,
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
                solid_faces: vec![vec![1]],
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

    /// The cube's faces `faces`, by name.
    fn faces(faces: &[u32]) -> Vec<String> {
        faces
            .iter()
            .map(|f| format!("aaaaaaaa.box.Face{f}"))
            .collect()
    }

    /// A rank by rule (a): the cube's faces `shared` and `unshared`, and whether it's the
    /// broken name's own feature's.
    fn alike(shared: &[u32], unshared: &[u32], same_feature: bool) -> Rank {
        Rank {
            shared: faces(shared),
            unshared: faces(unshared),
            same_feature,
            nearest: None,
        }
    }

    /// A rank by rule (b): nothing shared, `broken` the broken name's one element, and the
    /// candidate's element `element` numbered `gap` from it.
    fn near(broken: &str, element: &str, gap: u64) -> Rank {
        Rank {
            shared: Vec::new(),
            unshared: vec![broken.to_owned()],
            same_feature: true,
            nearest: Some(Nearest {
                element: element.to_owned(),
                gap,
            }),
        }
    }

    /// Each candidate's name and rank, as `of_name` gives them.
    fn ranked(candidates: Vec<(String, Rank)>) -> (Vec<String>, Vec<Rank>) {
        candidates.into_iter().unzip()
    }

    #[test]
    fn candidates_of_several_maps_rank_the_most_alike_first_whichever_map_holds_them() {
        // The broken name is made from Face1, Face2, and Face3. The first map's edges share
        // one of them each (Face1); the second's shares two (Face2 and Face3): it comes
        // first, though its map is the second.
        let broken = Name::parse(&edge(&[1, 2, 3]), &makers).unwrap();
        // Each map lists its edges against name order, so only the ranking's last rule, by
        // name, puts them in it.
        let first = generated(&[&[1, 5], &[1, 4]]);
        let second = generated(&[&[2, 3]]);
        // Each rank read off the names by hand: the broken name's Face1, Face2, and Face3,
        // those the candidate is made from shared, the rest not; every candidate the fuse's,
        // as the broken name is.
        assert_eq!(
            ranked(Candidates::of(&[&first, &second], ElementKind::Edge).of_name(&broken)),
            (
                vec![edge(&[2, 3]), edge(&[1, 4]), edge(&[1, 5])],
                vec![
                    alike(&[2, 3], &[1], true),
                    alike(&[1], &[2, 3], true),
                    alike(&[1], &[2, 3], true),
                ]
            )
        );
        // Seven alike: the first five by name, no more.
        let many = generated(&[
            &[3, 4],
            &[2, 6],
            &[2, 5],
            &[2, 4],
            &[1, 6],
            &[1, 5],
            &[1, 4],
        ]);
        assert_eq!(
            ranked(Candidates::of(&[&many], ElementKind::Edge).of_name(&broken)),
            (
                vec![
                    edge(&[1, 4]),
                    edge(&[1, 5]),
                    edge(&[1, 6]),
                    edge(&[2, 4]),
                    edge(&[2, 5]),
                ],
                vec![
                    alike(&[1], &[2, 3], true),
                    alike(&[1], &[2, 3], true),
                    alike(&[1], &[2, 3], true),
                    alike(&[2], &[1, 3], true),
                    alike(&[2], &[1, 3], true),
                ]
            )
        );
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
    fn an_edge_by_a_number_its_box_hasnt_has_the_nearest_edge_by_number_as_its_candidate() {
        // A cube has Edge1 to Edge12: Edge13 shares nothing with any of them, so its
        // candidate is the one numbered nearest it, Edge12; Edge40 too. A face's name, its
        // kind another's, finds no edge near it.
        let map = cube("aaaaaaaa");
        // Edge12 is 1 from Edge13 and 28 from Edge40, by hand; the broken name its one
        // element, shared with nothing; Edge12 the box's own, as the broken name is.
        for (broken, gap) in [("aaaaaaaa.box.Edge13", 1), ("aaaaaaaa.box.Edge40", 28)] {
            let errors = resolved(&map, &[broken]).unwrap_err();
            assert_eq!(errors[0].candidates(), ["aaaaaaaa.box.Edge12"], "{broken}");
            assert_eq!(
                errors[0].ranking(),
                [near(broken, "aaaaaaaa.box.Edge12", gap)],
                "{broken}"
            );
        }
        // A box has six faces: Face7 is 1 from Face6, Face9 3.
        for (broken, gap) in [("aaaaaaaa.box.Face7", 1), ("aaaaaaaa.box.Face9", 3)] {
            let face = Name::parse(broken, &makers).unwrap();
            assert_eq!(
                ranked(Candidates::of(&[&map], ElementKind::Face).of_name(&face)),
                (
                    vec!["aaaaaaaa.box.Face6".to_owned()],
                    vec![near(broken, "aaaaaaaa.box.Face6", gap)]
                ),
                "{broken}"
            );
        }
        let face = Name::parse("aaaaaaaa.box.Face7", &makers).unwrap();
        assert!(
            Candidates::of(&[&map], ElementKind::Edge)
                .of_name(&face)
                .is_empty()
        );
        // Another feature's element by its number: none of this one's.
        let other = Name::parse("bbbbbbbb.box.Edge13", &makers).unwrap();
        assert!(
            Candidates::of(&[&map], ElementKind::Edge)
                .of_name(&other)
                .is_empty()
        );
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
        // Each read off the names: the first made from both of the broken name's faces, the
        // rest from one, the other not.
        assert_eq!(
            errors[0].ranking(),
            [
                alike(&[1, 2], &[], true),
                alike(&[1], &[2], true),
                alike(&[1], &[2], true),
                alike(&[1], &[2], true),
                alike(&[2], &[1], true),
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
        assert_eq!(
            errors[0].ranking(),
            [alike(&[5, 6], &[], true), alike(&[5], &[6], true)]
        );
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
        // Its elements all the way down, Face1, Face2, and Face3, never the face between.
        assert_eq!(
            errors[0].ranking(),
            [alike(&[1, 3], &[2], true), alike(&[2], &[1, 3], true)]
        );
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
                solid_faces: vec![vec![]],
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
