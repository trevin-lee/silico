//! break-it: M2.7's element map (silico_core::naming) on hostile histories written by hand
//! or generated, every one passing `History::validate` (what the core checks before naming):
//! random histories against the map's own promises (every element named, every name
//! finding its element, nothing named twice, names that don't depend on how the result
//! happens to number its elements), FreeCAD's comparator against the rules of a total
//! order (Rust's sort may panic on one that isn't), ambiguous names carried into the next
//! feature, values the types hold that print alike, and long timelines.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use silico_core::document::FeatureId;
use silico_core::kernel::{
    ElementCounts, ElementFate, ElementKind, ElementRef, History, InputHistory, Reported, Topology,
};
use silico_core::naming::{
    Copied, ElementMap, Instance, Maker, Name, OpCode, Place, Raw, Relation, Source, TypeWord,
    freecad,
};

#[path = "common/within.rs"]
mod within;

use within::within;

/// A box's faces, edges, and vertices, as OCCT makes it.
const BOX: ElementCounts = ElementCounts {
    faces: 6,
    edges: 12,
    vertices: 8,
    solids: 1,
};

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn element(kind: ElementKind, index: u32) -> ElementRef {
    ElementRef { kind, index }
}

fn kept(index: u32) -> ElementFate {
    ElementFate {
        kept: Some(index),
        ..ElementFate::default()
    }
}

const KINDS: [ElementKind; 3] = [ElementKind::Face, ElementKind::Edge, ElementKind::Vertex];

// --- FreeCAD's comparator is a total order ---------------------------------------------

/// Every string up to `len` long over `alphabet`.
fn strings(alphabet: &[u8], len: usize) -> Vec<String> {
    let mut all = vec![String::new()];
    let mut layer = vec![String::new()];
    for _ in 0..len {
        let mut next = Vec::new();
        for s in &layer {
            for &c in alphabet {
                let mut t = s.clone();
                t.push(c as char);
                next.push(t);
            }
        }
        all.extend(next.iter().cloned());
        layer = next;
    }
    all
}

#[test]
fn freecads_comparator_is_a_total_order_over_every_short_name() {
    // Digits, letters, and the punctuation FreeCAD's names hold, both below '0' in ASCII
    // ('(' ',' and '|' is above) and above it (';' ':'), so the "digit first" rule and the
    // byte order after a run of digits meet. `sort_by` panics (Rust 1.81+) on an order that
    // isn't total, and two sorts of one set would disagree.
    let all = strings(b"01a(;:|", 4);
    let less = |a: &str, b: &str| freecad::compare(a, b) == Ordering::Less;
    // Reflexive and antisymmetric.
    for a in &all {
        assert_eq!(freecad::compare(a, a), Ordering::Equal, "{a:?}");
    }
    for a in &all {
        for b in &all {
            let (ab, ba) = (freecad::compare(a, b), freecad::compare(b, a));
            assert_eq!(ab, ba.reverse(), "{a:?} {b:?}");
            if a != b {
                assert_ne!(ab, Ordering::Equal, "{a:?} and {b:?} compare equal");
            }
        }
    }
    // Transitive, on the strings up to 3 long (every triple).
    let short: Vec<&String> = all.iter().filter(|s| s.len() <= 3).collect();
    for a in &short {
        for b in &short {
            if !less(a, b) {
                continue;
            }
            for c in &short {
                if less(b, c) {
                    assert!(less(a, c), "{a:?} < {b:?} < {c:?} but not {a:?} < {c:?}");
                }
            }
        }
    }
}

// --- Random histories against the map's promises --------------------------------------

/// A random history that passes `History::validate`: up to two inputs, each element kept,
/// modified, or generated at random into a small result, and a random topology.
fn histories() -> impl Strategy<Value = History> {
    let counts = (1u32..=5, 0u32..=8, 0u32..=6);
    counts
        .prop_flat_map(|(faces, edges, vertices)| {
            let result = ElementCounts {
                faces,
                edges,
                vertices,
                solids: 1,
            };
            let input = (0usize..=5, 0usize..=8, 0usize..=6).prop_flat_map(move |(f, e, v)| {
                (
                    proptest::collection::vec(fate(ElementKind::Face, result), f),
                    proptest::collection::vec(fate(ElementKind::Edge, result), e),
                    proptest::collection::vec(fate(ElementKind::Vertex, result), v),
                )
                    .prop_map(|(faces, edges, vertices)| InputHistory {
                        faces,
                        edges,
                        vertices,
                    })
            });
            (
                proptest::collection::vec(input, 1..=2),
                topology(result),
                Just(result),
            )
        })
        .prop_map(|(inputs, topology, result)| History {
            inputs,
            result,
            ends: Default::default(),
            topology,
        })
}

/// A sorted, repeat-free list of reported result elements of the kinds in `kinds`.
fn reports(kinds: Vec<ElementKind>, result: ElementCounts) -> impl Strategy<Value = Vec<Reported>> {
    let all: Vec<ElementRef> = kinds
        .into_iter()
        .flat_map(|k| (1..=result.of(k)).map(move |i| element(k, i)))
        .collect();
    let n = all.len();
    (
        proptest::sample::subsequence(all, 0..=n.min(3)),
        proptest::collection::vec(1u32..=3, 3),
    )
        .prop_map(|(mut elements, positions)| {
            elements.sort();
            elements
                .into_iter()
                .zip(positions)
                .map(|(element, position)| Reported { element, position })
                .collect()
        })
}

fn fate(kind: ElementKind, result: ElementCounts) -> impl Strategy<Value = ElementFate> {
    let count = result.of(kind);
    let keep = if count == 0 {
        Just(None).boxed()
    } else {
        prop_oneof![2 => Just(None), 1 => (1..=count).prop_map(Some)].boxed()
    };
    (
        keep,
        reports(vec![kind], result),
        reports(
            vec![
                ElementKind::Vertex,
                ElementKind::Edge,
                ElementKind::Face,
                ElementKind::Solid,
            ],
            result,
        ),
    )
        .prop_map(|(kept, modified, generated)| ElementFate {
            kept,
            modified,
            generated,
        })
}

fn shuffled_subset(n: u32, most: usize) -> impl Strategy<Value = Vec<u32>> {
    let all: Vec<u32> = (1..=n).collect();
    let len = all.len();
    proptest::sample::subsequence(all, 0..=len.min(most)).prop_shuffle()
}

fn topology(result: ElementCounts) -> impl Strategy<Value = Topology> {
    let face = shuffled_subset(result.edges, 5).prop_flat_map(|edges| {
        let len = edges.len();
        (
            Just(edges.clone()),
            proptest::sample::subsequence(edges, 0..=len),
        )
            .prop_map(|(edges, mut outer)| {
                outer.sort_unstable();
                (edges, outer)
            })
    });
    (
        proptest::collection::vec(face, result.faces as usize),
        proptest::collection::vec(shuffled_subset(result.vertices, 2), result.edges as usize),
    )
        .prop_map(move |(faces, edge_vertices)| {
            let (face_edges, outer_edges) = faces.into_iter().unzip();
            Topology {
                face_edges,
                outer_edges,
                edge_vertices,
                // A solid for each the result counts, every face the first's.
                solid_faces: (0..result.solids)
                    .map(|n| {
                        if n == 0 {
                            (1..=result.faces).collect()
                        } else {
                            Vec::new()
                        }
                    })
                    .collect(),
            }
        })
}

/// The inputs' maps: each a primitive's, by its own feature.
fn input_maps(history: &History) -> Vec<ElementMap> {
    (0..history.inputs.len())
        .map(|i| {
            let input = &history.inputs[i];
            let counts = ElementCounts {
                faces: input.faces.len() as u32,
                edges: input.edges.len() as u32,
                vertices: input.vertices.len() as u32,
                solids: 1,
            };
            ElementMap::by_number(id(&format!("abcd{i:04}")), OpCode::Fuse, counts)
        })
        .collect()
}

fn named(history: &History) -> ElementMap {
    let maps = input_maps(history);
    let refs: Vec<&ElementMap> = maps.iter().collect();
    ElementMap::operation(id("rrrrrrrr"), OpCode::Fuse, &refs, history)
}

fn every_element(result: ElementCounts) -> Vec<ElementRef> {
    KINDS
        .iter()
        .flat_map(|&k| (1..=result.of(k)).map(move |i| element(k, i)))
        .collect()
}

fn config(cases: u32) -> Config {
    Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    }
}

#[test]
fn every_valid_history_names_each_element_and_each_name_finds_it() {
    let mut runner = TestRunner::new(config(3000));
    runner
        .run(&histories(), |history| {
            prop_assert_eq!(history.validate(history.inputs.len()), Ok(()));
            let map = named(&history);
            let elements = every_element(history.result);
            let mut texts = BTreeSet::new();
            for &e in &elements {
                let names = map.names(e);
                prop_assert!(!names.is_empty(), "{} has no name", e);
                for name in &names {
                    prop_assert_eq!(map.element(name), Some(e), "{}'s name {}", e, name);
                    prop_assert!(texts.insert(name.to_string()), "two names written {}", name);
                }
            }
            // Nothing outside the result is named.
            for (e, _) in map.iter() {
                prop_assert!(elements.contains(&e), "names {}, not the result's", e);
            }
            // M2.8: every name reads back as itself, and finds its element. `named` names by
            // a fuse, and its inputs' maps are by number, as an operation's are.
            let maker = |_: &FeatureId| Some(Maker::By(OpCode::Fuse));
            for (e, names) in map.iter() {
                for name in names {
                    let text = name.to_string();
                    let read = Name::parse(&text, &maker);
                    prop_assert!(read.is_ok(), "{} doesn't read: {:?}", text, read);
                    let read = read.unwrap();
                    prop_assert_eq!(&read, &**name, "{}", text);
                    prop_assert_eq!(read.to_string(), text);
                    prop_assert_eq!(map.element(&read), Some(e));
                }
            }
            // N2: a kept element has every name its input element had.
            let maps = input_maps(&history);
            for (input, (fates, imap)) in history.inputs.iter().zip(&maps).enumerate() {
                for kind in KINDS {
                    let list = match kind {
                        ElementKind::Face => &fates.faces,
                        ElementKind::Edge => &fates.edges,
                        _ => &fates.vertices,
                    };
                    for (n, fate) in list.iter().enumerate() {
                        if let Some(k) = fate.kept {
                            let source = element(kind, n as u32 + 1);
                            for name in imap.names(source) {
                                prop_assert!(
                                    map.names(element(kind, k)).contains(&name),
                                    "input {} {} kept as {:?}{} lost its name {}",
                                    input,
                                    source,
                                    kind,
                                    k,
                                    name
                                );
                            }
                        }
                    }
                }
            }
            // The same history names alike.
            prop_assert_eq!(&named(&history), &map);
            Ok(())
        })
        .unwrap();
}

/// `history` with its result's elements renumbered by `order` (each kind's new number for
/// old number `i` is `order[kind][i - 1]`): the same result, as OCCT might number it after
/// an upstream edit that changed no element.
fn renumbered(history: &History, order: &BTreeMap<ElementKind, Vec<u32>>) -> History {
    let to = |e: ElementRef| -> ElementRef {
        match order.get(&e.kind) {
            Some(o) => element(e.kind, o[e.index as usize - 1]),
            None => e,
        }
    };
    let list = |l: &[Reported]| -> Vec<Reported> {
        let mut l: Vec<Reported> = l
            .iter()
            .map(|r| Reported {
                element: to(r.element),
                position: r.position,
            })
            .collect();
        l.sort();
        l
    };
    let fates = |kind: ElementKind, fates: &[ElementFate]| -> Vec<ElementFate> {
        fates
            .iter()
            .map(|f| ElementFate {
                kept: f.kept.map(|k| to(element(kind, k)).index),
                modified: list(&f.modified),
                generated: list(&f.generated),
            })
            .collect()
    };
    let inputs = history
        .inputs
        .iter()
        .map(|i| InputHistory {
            faces: fates(ElementKind::Face, &i.faces),
            edges: fates(ElementKind::Edge, &i.edges),
            vertices: fates(ElementKind::Vertex, &i.vertices),
        })
        .collect();
    // Each owner's list moves to its new number; its values are renumbered, its order
    // within the element kept (OCCT's order within one face doesn't follow the numbering).
    let relist = |lists: &[Vec<u32>], owner: ElementKind, of: ElementKind, sort: bool| {
        let mut out = vec![Vec::new(); lists.len()];
        for (i, l) in lists.iter().enumerate() {
            let new = to(element(owner, i as u32 + 1)).index as usize - 1;
            let mut l: Vec<u32> = l.iter().map(|&e| to(element(of, e)).index).collect();
            if sort {
                l.sort_unstable();
            }
            out[new] = l;
        }
        out
    };
    let t = &history.topology;
    History {
        inputs,
        result: history.result,
        ends: Default::default(),
        topology: Topology {
            face_edges: relist(&t.face_edges, ElementKind::Face, ElementKind::Edge, false),
            outer_edges: relist(&t.outer_edges, ElementKind::Face, ElementKind::Edge, true),
            edge_vertices: relist(
                &t.edge_vertices,
                ElementKind::Edge,
                ElementKind::Vertex,
                false,
            ),
            solid_faces: relist(&t.solid_faces, ElementKind::Solid, ElementKind::Face, true),
        },
    }
}

fn is_number(name: &Name) -> bool {
    matches!(name, Name::Element { feature, .. } if *feature == id("rrrrrrrr"))
}

#[test]
fn names_dont_depend_on_how_the_result_numbers_its_elements() {
    // The point of naming: the same result numbered another way (as OCCT numbers it after
    // an upstream edit that moved nothing) names each element alike. Leaves out histories
    // with `~n` duplicates (N3: their order is the numbering's, and they're refused), and
    // bare numbers (FreeCAD's unmapped names), which are numbers by definition.
    let strategy = histories().prop_flat_map(|h| {
        let r = h.result;
        let perm = |n: u32| Just((1..=n).collect::<Vec<u32>>()).prop_shuffle();
        (Just(h), perm(r.faces), perm(r.edges), perm(r.vertices))
    });
    let mut runner = TestRunner::new(config(3000));
    runner
        .run(&strategy, |(history, faces, edges, vertices)| {
            let order: BTreeMap<ElementKind, Vec<u32>> = [
                (ElementKind::Face, faces),
                (ElementKind::Edge, edges),
                (ElementKind::Vertex, vertices),
            ]
            .into_iter()
            .collect();
            let other = renumbered(&history, &order);
            prop_assert_eq!(other.validate(other.inputs.len()), Ok(()));
            let (a, b) = (named(&history), named(&other));
            let has_duplicates = |m: &ElementMap| {
                m.iter()
                    .any(|(_, ns)| ns.iter().any(|n| matches!(&**n, Name::Duplicate { .. })))
            };
            prop_assume!(!has_duplicates(&a));
            prop_assert!(
                !has_duplicates(&b),
                "duplicates appear only when renumbered"
            );
            for e in every_element(history.result) {
                let moved = element(e.kind, order[&e.kind][e.index as usize - 1]);
                let (na, nb) = (a.name(e).unwrap(), b.name(moved).unwrap());
                if is_number(&na) || is_number(&nb) {
                    prop_assert!(
                        is_number(&na) && is_number(&nb),
                        "{} named {} but, renumbered, {}",
                        e,
                        na,
                        nb
                    );
                    continue;
                }
                prop_assert_eq!(&na, &nb, "{} renumbered as {}", e, moved);
            }
            Ok(())
        })
        .unwrap();
}

// --- Ambiguous names carried on ---------------------------------------------------------

/// Two edges joining the same two vertices, named alike from them (the second `~1`).
fn ambiguous() -> (History, ElementMap) {
    let history = History {
        inputs: vec![InputHistory {
            faces: vec![],
            edges: vec![],
            vertices: vec![kept(1), kept(2)],
        }],
        result: ElementCounts {
            faces: 0,
            edges: 2,
            vertices: 2,
            solids: 0,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![],
            outer_edges: vec![],
            edge_vertices: vec![vec![1, 2], vec![2, 1]],
            solid_faces: Vec::new(),
        },
    };
    history.validate(1).unwrap();
    let input = ElementMap::by_number(id("aaaaaaaa"), OpCode::Fuse, BOX);
    let map = ElementMap::operation(id("pppppppp"), OpCode::Fuse, &[&input], &history);
    (history, map)
}

#[test]
fn an_ambiguous_name_kept_by_the_next_feature_is_still_marked_ambiguous() {
    // Found by /break-it: `duplicated` was filled only by a map's own collisions, so the
    // next feature, keeping both edges unchanged (N2), had `X` and `X~1` in a map that said
    // neither was duplicated, and a reference to `X` (refused one feature earlier, N3) would
    // have been accepted, though which edge `X` names still turns on the numbering.
    let (_, first) = ambiguous();
    let x = first.name(element(ElementKind::Edge, 1)).unwrap();
    assert!(first.is_unstable(&x));
    // A cut by a tool that misses: the base's edges and vertices kept as they were; the
    // tool's (none) nothing.
    let history = History {
        inputs: vec![
            InputHistory {
                faces: vec![],
                edges: vec![kept(1), kept(2)],
                vertices: vec![kept(1), kept(2)],
            },
            InputHistory::default(),
        ],
        result: ElementCounts {
            faces: 0,
            edges: 2,
            vertices: 2,
            solids: 0,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![],
            outer_edges: vec![],
            edge_vertices: vec![vec![1, 2], vec![2, 1]],
            solid_faces: Vec::new(),
        },
    };
    history.validate(2).unwrap();
    let tool = ElementMap::by_number(id("tttttttt"), OpCode::Fuse, BOX);
    let next = ElementMap::operation(id("nnnnnnnn"), OpCode::Cut, &[&first, &tool], &history);
    assert_eq!(
        next.name(element(ElementKind::Edge, 1)),
        Some(Arc::clone(&x))
    );
    assert!(
        next.is_unstable(&x),
        "{x} names one of two edges that came about alike, in the next feature too"
    );
}

// --- Values the types hold that print alike ---------------------------------------------

#[test]
fn every_name_the_types_hold_prints_differently_from_every_other() {
    // Places and ordinals count from 1 by their type (`/break-it` found that a 0
    // printed as a 1 does), so the forms' leaving out a 1 can't make two names print alike:
    // 1 and 2 differ, and a place and none differ. OCCT's raw position isn't part of a
    // name: two names that differ only in it are one name, printed alike (Trevin's
    // decision: positions only within groups).
    let leaf = Arc::new(Name::Element {
        feature: id("aaaaaaaa"),
        made: TypeWord::Operation(OpCode::Fuse),
        element: element(ElementKind::Face, 1),
    });
    let n = |n: u32| NonZeroU32::new(n).unwrap();
    let derived = |place: Option<(u32, u32)>, position: u32| Name::Derived {
        feature: id("rrrrrrrr"),
        operation: OpCode::Cut,
        kind: ElementKind::Face,
        first: Source {
            relation: Relation::Modified,
            place: place.map(|(rank, size)| Place {
                rank: NonZeroU32::new(rank).unwrap(),
                size: NonZeroU32::new(size).unwrap(),
            }),
            position: Raw(Some(NonZeroU32::new(position).unwrap())),
            name: Arc::clone(&leaf),
        },
        more: vec![],
    };
    // Only OCCT's position differs: one name, written alike.
    assert_eq!(derived(None, 1), derived(None, 2));
    assert_eq!(derived(None, 1).to_string(), derived(None, 2).to_string());
    let upper = |place: Option<(u32, u32)>, nth| Name::Upper {
        feature: id("rrrrrrrr"),
        operation: OpCode::Cut,
        kind: ElementKind::Edge,
        upper: Arc::clone(&leaf),
        more: vec![],
        place: place.map(|(rank, size)| Place {
            rank: n(rank),
            size: n(size),
        }),
        nth: Raw(nth),
    };
    // Only FreeCAD's count differs: one name, written alike.
    assert_eq!(upper(None, n(1)), upper(None, n(2)));
    let duplicate = |ordinal| Name::Duplicate {
        name: Arc::clone(&leaf),
        ordinal,
    };
    let mut clashes = Vec::new();
    for (a, b) in [
        (derived(Some((1, 2)), 1), derived(Some((2, 2)), 2)),
        (derived(None, 1), derived(Some((1, 2)), 1)),
        (derived(Some((1, 2)), 1), derived(Some((1, 3)), 1)),
        (upper(Some((1, 2)), n(1)), upper(Some((2, 2)), n(1))),
        (upper(None, n(1)), upper(Some((1, 2)), n(1))),
        (duplicate(n(1)), duplicate(n(2))),
    ] {
        assert_ne!(a, b);
        if a.to_string() == b.to_string() {
            clashes.push(format!("{a:?} and {b:?} both print {a}"));
        }
    }
    assert!(clashes.is_empty(), "{clashes:#?}");
}

#[test]
fn a_derived_name_has_a_freecad_form() {
    // Every derived or lower name has a first source by its type (`/break-it` found an
    // empty list writing FreeCAD's form as "", which sorts before every other name), so
    // the shortest of each writes its source.
    let leaf = Arc::new(Name::Element {
        feature: id("aaaaaaaa"),
        made: TypeWord::Operation(OpCode::Fuse),
        element: element(ElementKind::Edge, 1),
    });
    let derived = Name::Derived {
        feature: id("rrrrrrrr"),
        operation: OpCode::Cut,
        kind: ElementKind::Face,
        first: Source {
            relation: Relation::Generated,
            place: None,
            position: Raw(Some(NonZeroU32::MIN)),
            name: Arc::clone(&leaf),
        },
        more: vec![],
    };
    let lower = Name::Lower {
        feature: id("rrrrrrrr"),
        operation: OpCode::Cut,
        kind: ElementKind::Face,
        first: Arc::clone(&leaf),
        more: vec![],
    };
    assert_eq!(freecad::text(&derived), "Edge1;:G;CUT");
    assert_eq!(freecad::text(&lower), "Edge1;:L;CUT");
    assert_eq!(
        derived.to_string(),
        "rrrrrrrr.cut.Face(G aaaaaaaa.fuse.Edge1)"
    );
    assert_eq!(
        lower.to_string(),
        "rrrrrrrr.cut.Face(L aaaaaaaa.fuse.Edge1)"
    );
}

#[test]
fn a_names_further_sources_are_written_apart_in_freecads_form() {
    // FreeCAD writes the sources after the first in parentheses, each after the first
    // behind a `|` (TopoShapeExpansion.cpp's makeShapeWithElementMap: generated or modified,
    // and named from below).
    let leaf = |n| {
        Arc::new(Name::Element {
            feature: id("aaaaaaaa"),
            made: TypeWord::Operation(OpCode::Fuse),
            element: element(ElementKind::Edge, n),
        })
    };
    let modified = |n| Source {
        relation: Relation::Modified,
        place: None,
        position: Raw(Some(NonZeroU32::MIN)),
        name: leaf(n),
    };
    let derived = Name::Derived {
        feature: id("rrrrrrrr"),
        operation: OpCode::Cut,
        kind: ElementKind::Face,
        first: modified(1),
        more: vec![modified(2), modified(3)],
    };
    let lower = Name::Lower {
        feature: id("rrrrrrrr"),
        operation: OpCode::Cut,
        kind: ElementKind::Face,
        first: leaf(1),
        more: vec![leaf(2), leaf(3)],
    };
    assert_eq!(freecad::text(&derived), "Edge1;:M(Edge2|Edge3);CUT");
    assert_eq!(freecad::text(&lower), "Edge1;:L(Edge2|Edge3);CUT");
}

#[test]
fn of_two_sources_freecad_writes_alike_the_bases_comes_first() {
    // A fuse of the base `zzzzzzzz` (input 0) and the tool `aaaaaaaa`, each with one face,
    // both kept, and an edge nothing reported, bounded by both: it's named from the faces
    // above it, the first by FreeCAD's order. Both are `Face1` in FreeCAD's form, so the
    // input decides, the base's first, not the IDs' order, which puts the tool's first.
    let one_face = ElementCounts {
        faces: 1,
        edges: 0,
        vertices: 0,
        solids: 1,
    };
    let base = ElementMap::by_number(id("zzzzzzzz"), OpCode::Fuse, one_face);
    let tool = ElementMap::by_number(id("aaaaaaaa"), OpCode::Fuse, one_face);
    let result = ElementCounts {
        faces: 2,
        edges: 1,
        vertices: 0,
        solids: 1,
    };
    let history = History {
        inputs: vec![
            InputHistory {
                faces: vec![kept(1)],
                edges: vec![],
                vertices: vec![],
            },
            InputHistory {
                faces: vec![kept(2)],
                edges: vec![],
                vertices: vec![],
            },
        ],
        result,
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![vec![1], vec![1]],
            outer_edges: vec![vec![1], vec![1]],
            edge_vertices: vec![vec![]],
            solid_faces: vec![vec![1, 2]],
        },
    };
    let map = ElementMap::operation(id("ffffffff"), OpCode::Fuse, &[&base, &tool], &history);
    let edge = map.name(element(ElementKind::Edge, 1)).unwrap();
    let Name::Upper { upper, more, .. } = &*edge else {
        panic!("{edge}");
    };
    assert_eq!(upper.to_string(), "zzzzzzzz.fuse.Face1");
    assert_eq!(
        more.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["aaaaaaaa.fuse.Face1"]
    );
}

// --- A primitive's map and element numbers ----------------------------------------------

#[test]
fn a_primitives_map_doesnt_name_element_zero() {
    // Found by /break-it: elements count from 1 (`Face1`), so `Face0` is no solid's. A
    // primitive's map answers for any number, leaving the bound to the solid's counts, and
    // a check of `index <= count` let 0 through: a reference `k7f2qx3m.fuse.Face0` (made up, or
    // off by one) resolved to an element that can't exist. An operation's map says None.
    let map = ElementMap::by_number(id("k7f2qx3m"), OpCode::Fuse, BOX);
    let zero = Name::Element {
        feature: id("k7f2qx3m"),
        made: TypeWord::Operation(OpCode::Fuse),
        element: element(ElementKind::Face, 0),
    };
    assert_eq!(map.element(&zero), None);
    assert!(map.name(element(ElementKind::Face, 0)).is_none());
}

#[test]
fn a_primitives_map_and_an_operations_agree_about_solids() {
    // A map names faces, edges, and vertices. A primitive's says it names `Solid1` too,
    // an operation's doesn't, so a reference to a solid is found or not by what made it.
    let primitive = ElementMap::by_number(id("k7f2qx3m"), OpCode::Fuse, BOX);
    let (_, operation) = ambiguous();
    let solid = element(ElementKind::Solid, 1);
    // Found by /break-it: the primitive's map named the solid; the operation's didn't.
    assert_eq!(
        primitive.name(solid).is_some(),
        operation.name(solid).is_some(),
        "primitive {:?}, operation {:?}",
        primitive.name(solid),
        operation.name(solid)
    );
}

// --- Long timelines ---------------------------------------------------------------------

/// A box's history through an operation that modifies its top face (Face6) and keeps
/// everything else: a hole drilled from the top, without the hole's own elements.
fn drilled() -> History {
    let box_counts = ElementCounts {
        faces: 6,
        edges: 12,
        vertices: 8,
        solids: 1,
    };
    let mut faces: Vec<ElementFate> = (1..=6).map(kept).collect();
    faces[5] = ElementFate {
        modified: vec![Reported {
            element: element(ElementKind::Face, 6),
            position: 1,
        }],
        ..ElementFate::default()
    };
    History {
        inputs: vec![
            InputHistory {
                faces,
                edges: (1..=12).map(kept).collect(),
                vertices: (1..=8).map(kept).collect(),
            },
            InputHistory::default(),
        ],
        result: box_counts,
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![
                vec![1, 2, 3, 4],
                vec![5, 6, 7, 8],
                vec![9, 5, 10, 1],
                vec![11, 7, 12, 3],
                vec![4, 11, 8, 9],
                vec![2, 12, 6, 10],
            ],
            outer_edges: vec![
                vec![1, 2, 3, 4],
                vec![5, 6, 7, 8],
                vec![1, 5, 9, 10],
                vec![3, 7, 11, 12],
                vec![4, 8, 9, 11],
                vec![2, 6, 10, 12],
            ],
            edge_vertices: vec![
                vec![1, 2],
                vec![3, 1],
                vec![3, 4],
                vec![4, 2],
                vec![5, 6],
                vec![7, 5],
                vec![7, 8],
                vec![8, 6],
                vec![6, 2],
                vec![5, 1],
                vec![8, 4],
                vec![7, 3],
            ],
            solid_faces: vec![(1..=box_counts.faces).collect()],
        },
    }
}

#[test]
fn a_thousand_holes_drilled_one_by_one_leave_the_top_face_its_name_promptly() {
    // A perforated plate built as 1,000 cut features, each modifying the top face (and
    // nothing else) as OCCT's first result of it: the face is the plate's Face6 as later
    // features have modified it (N7: its own name), whatever the number of holes, stable.
    // Before N7 its name nested a level per hole, until the bound on a name's
    // depth made it a number, unreferenceable for good (the review found that cliff).
    let history = drilled();
    history.validate(2).unwrap();
    let tool = ElementMap::by_number(id("tttttttt"), OpCode::Fuse, BOX);
    let map = within(Duration::from_secs(10), move || {
        let mut map = ElementMap::by_number(id("pppppppp"), OpCode::Fuse, BOX);
        for n in 0..1000 {
            let feature = id(&format!("f{n:07}"));
            map = ElementMap::operation(feature, OpCode::Cut, &[&map, &tool], &history);
            let top = map.name(element(ElementKind::Face, 6)).unwrap();
            assert_eq!(top.to_string(), "pppppppp.fuse.Face6", "after {n}");
            assert!(!map.is_unstable(&top));
        }
        map
    });
    // FreeCAD's form, one level: FreeCAD would write a thousand.
    let top = map.name(element(ElementKind::Face, 6)).unwrap();
    assert_eq!(freecad::text(&top), "Face6;:M;CUT");
    // A bystander keeps its primitive name through all of it (N2).
    assert_eq!(
        map.name(element(ElementKind::Face, 1)).unwrap().to_string(),
        "pppppppp.fuse.Face1"
    );
}

/// `drilled()`, but the input's Face1 and Face2 each modified into both of the result's
/// Face1 and Face2: two names each made of the same two, so written out, a name doubles
/// with every feature (a history every check passes).
fn crossed() -> History {
    let mut h = drilled();
    let both = |p: u32| ElementFate {
        modified: vec![
            Reported {
                element: element(ElementKind::Face, 1),
                position: p,
            },
            Reported {
                element: element(ElementKind::Face, 2),
                position: p + 1,
            },
        ],
        ..ElementFate::default()
    };
    h.inputs[0].faces[0] = both(1);
    h.inputs[0].faces[1] = both(1);
    h.inputs[0].faces[5] = kept(6);
    h
}

#[test]
fn names_that_would_double_with_every_feature_are_bounded_promptly() {
    // Found by the review: 21 features of this made one name 67 MB in FreeCAD's form, and
    // each feature doubled the time and memory. Past the bounds, an element is named by
    // its number, and the name says it's unstable.
    let history = crossed();
    history.validate(2).unwrap();
    let tool = ElementMap::by_number(id("tttttttt"), OpCode::Fuse, BOX);
    let map = within(Duration::from_secs(5), move || {
        let mut map = ElementMap::by_number(id("pppppppp"), OpCode::Fuse, BOX);
        for n in 0..40 {
            map = ElementMap::operation(
                id(&format!("f{n:07}")),
                OpCode::Cut,
                &[&map, &tool],
                &history,
            );
        }
        map
    });
    let face1 = map.name(element(ElementKind::Face, 1)).unwrap();
    assert!(freecad::text(&face1).len() < 200_000);
    assert!(face1.to_string().len() < 200_000);
    // Named by its number where it passed the bounds (and modified from that since), so
    // unstable.
    assert!(map.is_unstable(&face1));
    // The kept faces aren't touched by it.
    let face3 = map.name(element(ElementKind::Face, 3)).unwrap();
    assert_eq!(face3.to_string(), "pppppppp.fuse.Face3");
    assert!(!map.is_unstable(&face3));
}

/// `drilled()`, but the plate's Face1 and Face2 each modified into both the result's Face1
/// and Face2, and the result's Face1 made from the first `p` of the tool's faces too, its
/// Face2 from the first `q` (`p` and `q` differ, so the two are named apart).
fn crossed_with_tool(p: u32, q: u32) -> History {
    let mut h = drilled();
    let into = |faces: &[u32]| ElementFate {
        modified: (1..)
            .zip(faces)
            .map(|(position, &face)| Reported {
                element: element(ElementKind::Face, face),
                position,
            })
            .collect(),
        ..ElementFate::default()
    };
    h.inputs[0].faces[0] = into(&[1, 2]);
    h.inputs[0].faces[1] = into(&[1, 2]);
    h.inputs[0].faces[5] = kept(6);
    h.inputs[1].faces = (1..=p.max(q))
        .map(|j| match (j <= p, j <= q) {
            (true, true) => into(&[1, 2]),
            (true, false) => into(&[1]),
            _ => into(&[2]),
        })
        .collect();
    h
}

/// How many names `name` is made of, written out (a name used twice counts twice): the
/// bound's measure, counted here from the name's own parts.
fn written_out(name: &Name) -> u64 {
    let parts: Vec<&Name> = match name {
        Name::Element { .. } | Name::Sketch { .. } => vec![],
        Name::Derived { first, more, .. } => std::iter::once(first)
            .chain(more)
            .map(|source| source.name.as_ref())
            .collect(),
        Name::Upper { upper, more, .. } => std::iter::once(upper)
            .chain(more)
            .map(Arc::as_ref)
            .collect(),
        Name::Lower { first, more, .. } => std::iter::once(first)
            .chain(more)
            .map(Arc::as_ref)
            .collect(),
        Name::Duplicate { name, .. }
        | Name::Modified { name, .. }
        | Name::Copy { of: name, .. } => vec![name.as_ref()],
    };
    1 + parts.into_iter().map(written_out).sum::<u64>()
}

/// The plate cut by the tool once for each `(p, q)`, in turn, each history checked: its
/// Face1 and Face2 grow as the tests below work them by hand.
fn cut_chain(steps: &[(u32, u32)]) -> ElementMap {
    let tool = ElementMap::by_number(id("tttttttt"), OpCode::Fuse, BOX);
    let mut map = ElementMap::by_number(id("pppppppp"), OpCode::Fuse, BOX);
    for (n, &(p, q)) in steps.iter().enumerate() {
        let history = crossed_with_tool(p, q);
        history.validate(2).unwrap();
        map = ElementMap::operation(
            id(&format!("f{n:07}")),
            OpCode::Cut,
            &[&map, &tool],
            &history,
        );
    }
    map
}

#[test]
fn a_name_of_exactly_the_most_names_is_kept_and_one_more_is_numbered() {
    // Found by the milestone's mutation run: a name exactly at the bound (4,096 names
    // written out) was never made, so a bound of `>=` passed every test. The result's
    // Face1 and Face2 are each made of the plate's two and some of the tool's faces (each
    // one name): with a, b the plate's two and p, q the tool's faces each takes, the next
    // are 1 + a + b + p and 1 + a + b + q. Worked by hand from 1 and 1, with (p, q) of
    // (1, 3) six times, then (2, 3), (1, 3), (2, 3): 4 and 6, 12 and 14, 28 and 30, 60 and
    // 62, 124 and 126, 252 and 254, 509 and 510, 1021 and 1023, 2047 and 2048. Then (0, 1):
    // Face1 is 1 + 4095 = 4096 names, at the bound, so it's kept; Face2 is 4097, past it,
    // so it's named by its number, unstable.
    let map = cut_chain(&[
        (1, 3),
        (1, 3),
        (1, 3),
        (1, 3),
        (1, 3),
        (1, 3),
        (2, 3),
        (1, 3),
        (2, 3),
        (0, 1),
    ]);
    let face1 = map.name(element(ElementKind::Face, 1)).unwrap();
    assert_eq!(written_out(&face1), 4096);
    assert!(matches!(*face1, Name::Derived { .. }), "{face1}");
    assert!(!map.is_unstable(&face1));
    let face2 = map.name(element(ElementKind::Face, 2)).unwrap();
    assert_eq!(face2.to_string(), "f0000009.cut.Face2");
    assert!(map.is_unstable(&face2));
}

#[test]
fn a_copy_of_a_name_at_the_most_names_is_left_out_and_a_lighter_one_is_copied() {
    // N10: a copy is a name more, so the copy of a name already 4,096 names written out
    // would be 4,097, past the bound the parser reads to: the copy's map leaves it out
    // (never numbering it by the tool's number), and copies the rest. The map is the one
    // the test above works by hand, its Face1 at the bound.
    let map = cut_chain(&[
        (1, 3),
        (1, 3),
        (1, 3),
        (1, 3),
        (1, 3),
        (1, 3),
        (2, 3),
        (1, 3),
        (2, 3),
        (0, 1),
    ]);
    let face1 = map.name(element(ElementKind::Face, 1)).unwrap();
    assert_eq!(written_out(&face1), 4096);
    let one = Instance::new(&[1]).unwrap();
    let copied =
        ElementMap::copy(&map, id("m0000001"), OpCode::Mirror, Copied::Solid, &one).unwrap();
    assert_eq!(copied.name(element(ElementKind::Face, 1)), None);
    // Every other element is copied, each name wrapped once and under the bound.
    let mut kept = 0;
    for (element, names) in map.iter() {
        if element == self::element(ElementKind::Face, 1) {
            continue;
        }
        let own = &names[0];
        let copy = copied.name(element).unwrap();
        assert_eq!(
            copy.to_string(),
            format!("m0000001.mirror.{}(S1 {own})", kind_word(element.kind))
        );
        assert!(written_out(&copy) <= 4096, "{copy}");
        kept += 1;
    }
    assert!(kept > 0);
    assert_eq!(copied.iter().count(), map.iter().count() - 1);
}

#[test]
fn a_copy_of_exactly_the_most_names_is_kept_and_one_more_is_left_out() {
    // The bound's edge: worked by hand with the rule above (next 1 + a + b + p, or + q),
    // (1, 3) six times gives 252 and 254, then (2, 3) 509 and 510, (1, 3) 1021 and 1023,
    // (1, 2) 2046 and 2047, (1, 2) 4095 and 4096. Face1's copy is 4,096 names, at the
    // bound, so it's kept; Face2's would be 4,097, so it's left out.
    let map = cut_chain(&[
        (1, 3),
        (1, 3),
        (1, 3),
        (1, 3),
        (1, 3),
        (1, 3),
        (2, 3),
        (1, 3),
        (1, 2),
        (1, 2),
    ]);
    let face1 = map.name(element(ElementKind::Face, 1)).unwrap();
    let face2 = map.name(element(ElementKind::Face, 2)).unwrap();
    assert_eq!(written_out(&face1), 4095);
    assert_eq!(written_out(&face2), 4096);
    let one = Instance::new(&[1]).unwrap();
    let copied =
        ElementMap::copy(&map, id("m0000001"), OpCode::Mirror, Copied::Solid, &one).unwrap();
    let kept = copied.name(element(ElementKind::Face, 1)).unwrap();
    assert_eq!(written_out(&kept), 4096);
    assert_eq!(
        kept.to_string(),
        format!("m0000001.mirror.Face(S1 {face1})")
    );
    assert!(!copied.is_unstable(&kept));
    assert_eq!(copied.name(element(ElementKind::Face, 2)), None);
    // The parser agrees at the edge: the copy kept reads back as itself, and the one left
    // out, 4,097 names, is refused.
    let any = |_: &FeatureId| Some(Maker::By(OpCode::Fuse));
    assert_eq!(Name::parse(&kept.to_string(), &any).unwrap(), *kept);
    let past = format!("m0000001.mirror.Face(S1 {face2})");
    let error = Name::parse(&past, &any).unwrap_err();
    assert!(error.why.contains("more than 4096 names"), "{error}");
}

fn kind_word(kind: ElementKind) -> &'static str {
    match kind {
        ElementKind::Vertex => "Vertex",
        ElementKind::Edge => "Edge",
        ElementKind::Face => "Face",
        ElementKind::Solid => "Solid",
    }
}

#[test]
fn a_deep_timeline_names_and_writes_its_names_on_a_default_thread() {
    // Found by the review: a plate drilled 6,000 times, a feature a hole, nested its top
    // face's name 6,000 deep, and writing it overflowed a 2 MB stack (an abort). Bounded,
    // and with N7 (a face only ever modified keeps its name), every walk is short.
    let joined = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            let history = drilled();
            let tool = ElementMap::by_number(id("tttttttt"), OpCode::Fuse, BOX);
            let mut map = ElementMap::by_number(id("pppppppp"), OpCode::Fuse, BOX);
            for n in 0..6000 {
                map = ElementMap::operation(
                    id(&format!("f{n:07}")),
                    OpCode::Cut,
                    &[&map, &tool],
                    &history,
                );
            }
            let top = map.name(element(ElementKind::Face, 6)).unwrap();
            (
                freecad::text(&top).len(),
                top.to_string(),
                map.is_unstable(&top),
            )
        })
        .unwrap()
        .join();
    let (text, top, unstable) = joined.unwrap();
    assert!(text < 200_000);
    // With N7, the plate's top face, modified 6,000 times, is still its Face6.
    assert_eq!(top, "pppppppp.fuse.Face6");
    assert!(!unstable);
}

#[test]
fn a_name_reported_for_many_elements_is_written_once_promptly() {
    // Found by the review: a source's name, near the bound in size (32 KB written out),
    // reported for 100,000 result faces made the core copy its text for each, 5 GB from a
    // history of about a megabyte. Written once and shared, it's the history's size.
    let history = crossed();
    let tool = ElementMap::by_number(id("tttttttt"), OpCode::Fuse, BOX);
    let mut map = ElementMap::by_number(id("pppppppp"), OpCode::Fuse, BOX);
    for n in 0..11 {
        map = ElementMap::operation(
            id(&format!("f{n:07}")),
            OpCode::Cut,
            &[&map, &tool],
            &history,
        );
    }
    let face1 = map.name(element(ElementKind::Face, 1)).unwrap();
    assert!(freecad::text(&face1).len() > 30_000);
    let r: u32 = 100_000;
    let all = |p: u32| ElementFate {
        modified: (1..=r)
            .map(|i| Reported {
                element: element(ElementKind::Face, i),
                position: p,
            })
            .collect(),
        ..ElementFate::default()
    };
    let mut faces: Vec<ElementFate> = (0..6).map(|_| ElementFate::default()).collect();
    faces[0] = all(1);
    faces[1] = all(2);
    let many = History {
        inputs: vec![
            InputHistory {
                faces,
                edges: vec![ElementFate::default(); 12],
                vertices: vec![ElementFate::default(); 8],
            },
            InputHistory::default(),
        ],
        result: ElementCounts {
            faces: r,
            edges: 0,
            vertices: 0,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![vec![]; r as usize],
            outer_edges: vec![vec![]; r as usize],
            edge_vertices: vec![],
            solid_faces: vec![(1..=r).collect()],
        },
    };
    many.validate(2).unwrap();
    let named = within(Duration::from_secs(10), move || {
        ElementMap::operation(id("zzzzzzzz"), OpCode::Cut, &[&map, &tool], &many)
    });
    assert_eq!(named.iter().count(), r as usize);
}

#[test]
fn a_history_naming_an_element_its_input_doesnt_have_gives_an_unstable_name() {
    // A box has 6 faces; a history that keeps a 7th (a broken worker's) names it by number,
    // as FreeCAD's unmapped names are, and that name is unstable.
    let mut faces: Vec<ElementFate> = (1..=6).map(kept).collect();
    faces.push(kept(7));
    let history = History {
        inputs: vec![
            InputHistory {
                faces,
                edges: (1..=12).map(kept).collect(),
                vertices: (1..=8).map(kept).collect(),
            },
            InputHistory::default(),
        ],
        result: ElementCounts {
            faces: 7,
            edges: 12,
            vertices: 8,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![vec![]; 7],
            outer_edges: vec![vec![]; 7],
            edge_vertices: vec![vec![]; 12],
            solid_faces: vec![(1..=7).collect()],
        },
    };
    history.validate(2).unwrap();
    let base = ElementMap::by_number(id("pppppppp"), OpCode::Fuse, BOX);
    let tool = ElementMap::by_number(id("tttttttt"), OpCode::Fuse, BOX);
    let map = ElementMap::operation(id("cccccccc"), OpCode::Cut, &[&base, &tool], &history);
    let seventh = map.name(element(ElementKind::Face, 7)).unwrap();
    assert_eq!(seventh.to_string(), "pppppppp.fuse.Face7");
    assert!(map.is_unstable(&seventh));
    let sixth = map.name(element(ElementKind::Face, 6)).unwrap();
    assert!(!map.is_unstable(&sixth));
}

#[test]
fn a_maps_unstable_names_are_its_own_however_long_the_timeline() {
    // Found by the review: each map carried every earlier map's names by number, so a
    // chain of results nothing names grew with the square of its length (3.7 GB for 160
    // features of 20,000 faces). A map holds only its own unstable names.
    let (r, n) = (1_000u32, 50usize);
    let history = History {
        inputs: vec![InputHistory::default(), InputHistory::default()],
        result: ElementCounts {
            faces: r,
            edges: 0,
            vertices: 0,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![vec![]; r as usize],
            outer_edges: vec![vec![]; r as usize],
            edge_vertices: vec![],
            solid_faces: vec![(1..=r).collect()],
        },
    };
    history.validate(2).unwrap();
    let tool = ElementMap::by_number(id("tttttttt"), OpCode::Fuse, BOX);
    let mut map = ElementMap::by_number(id("pppppppp"), OpCode::Fuse, BOX);
    for k in 0..n {
        map = ElementMap::operation(
            id(&format!("f{k:07}")),
            OpCode::Cut,
            &[&map, &tool],
            &history,
        );
    }
    assert_eq!(map.unstable().count(), r as usize);
    // Each is this feature's element by number.
    let face = map.name(element(ElementKind::Face, 1)).unwrap();
    assert_eq!(face.to_string(), "f0000049.cut.Face1");
    assert!(map.is_unstable(&face));
}

/// A fuse of disjoint solids, as OCCT reports one: the base's elements (`a` of each kind)
/// kept, the tool's (`b`) kept after them.
fn kept_fuse(a: ElementCounts, b: ElementCounts) -> History {
    let kept_all = |n: u32, after: u32| (1..=n).map(|i| kept(after + i)).collect::<Vec<_>>();
    let result = ElementCounts {
        faces: a.faces + b.faces,
        edges: a.edges + b.edges,
        vertices: a.vertices + b.vertices,
        solids: 1,
    };
    History {
        inputs: vec![
            InputHistory {
                faces: kept_all(a.faces, 0),
                edges: kept_all(a.edges, 0),
                vertices: kept_all(a.vertices, 0),
            },
            InputHistory {
                faces: kept_all(b.faces, a.faces),
                edges: kept_all(b.edges, a.edges),
                vertices: kept_all(b.vertices, a.vertices),
            },
        ],
        result,
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![vec![]; result.faces as usize],
            outer_edges: vec![vec![]; result.faces as usize],
            edge_vertices: vec![vec![]; result.edges as usize],
            solid_faces: vec![(1..=result.faces).collect()],
        },
    }
}

/// The CPU time of naming a fuse of a solid of `n` boxes' elements (6 n faces, 12 n
/// edges, 8 n vertices) and one more box, every element kept: a map of 26 (n + 1) names,
/// the least of five.
fn naming_cost(n: u32) -> Duration {
    let counts = ElementCounts {
        faces: 6 * n,
        edges: 12 * n,
        vertices: 8 * n,
        solids: 1,
    };
    let base = ElementMap::by_number(id("pppppppp"), OpCode::Fuse, counts);
    let history = kept_fuse(counts, BOX);
    let tool = ElementMap::by_number(id("tttttttt"), OpCode::Fuse, BOX);
    (0..5)
        .map(|_| {
            let start = cpu_time::ThreadTime::now();
            let map =
                ElementMap::operation(id("zzzzzzzz"), OpCode::Fuse, &[&base, &tool], &history);
            let spent = start.elapsed();
            assert_eq!(map.iter().count() as u32, 26 * (n + 1));
            spent
        })
        .min()
        .unwrap()
}

#[test]
fn naming_a_solid_costs_as_its_elements_do() {
    // One feature's names cost in proportion to its solid's elements: four times the
    // elements, about four times the CPU (twice as many, at eight, would be the square).
    // Found by the review (via #26): a long chain's recompute took seven times as long as
    // without naming; the cost was the fake kernel's (fixed), and this keeps naming's own
    // cost from growing past linear. In thread CPU time, which load doesn't change but a
    // busy other hyperthread on the core does, so nextest runs it alone (.config/nextest.toml).
    let small = naming_cost(1_000);
    let large = naming_cost(4_000);
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
    assert!(
        ratio < 8.0,
        "1000: {small:?}, 4000: {large:?}, {ratio:.1} times"
    );
}

#[test]
fn a_chain_of_commons_keeping_a_face_whole_is_bounded() {
    // Found by the milestone's third round: what a common keeps whole of its base is its own
    // piece, `I` (N9), a level a common, which no collapse takes away; past the bound (256
    // deep), an element is named by its number, unstable, as every name's depth is bounded
    // (a reference can't be deeper). 300 commons in a row, each keeping the base's Face1.
    let history = History {
        inputs: vec![
            InputHistory {
                faces: vec![ElementFate {
                    kept: Some(1),
                    ..ElementFate::default()
                }],
                edges: vec![],
                vertices: vec![],
            },
            InputHistory {
                faces: vec![],
                edges: vec![],
                vertices: vec![],
            },
        ],
        result: ElementCounts {
            faces: 1,
            edges: 0,
            vertices: 0,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![vec![]],
            outer_edges: vec![vec![]],
            edge_vertices: vec![],
            solid_faces: vec![vec![1]],
        },
    };
    history.validate(2).unwrap();
    let tool = ElementMap::by_number(id("tttttttt"), OpCode::Fuse, BOX);
    let mut map = ElementMap::by_number(id("pppppppp"), OpCode::Fuse, BOX);
    let face = element(ElementKind::Face, 1);
    for n in 0..300 {
        map = ElementMap::operation(
            id(&format!("f{n:07}")),
            OpCode::Common,
            &[&map, &tool],
            &history,
        );
        let name = map.name(face).unwrap();
        let depth = name.to_string().matches("(I ").count();
        assert!(depth <= 256, "common {n}: {depth} deep");
        // Within the bound, a level a common, stable.
        if n < 255 {
            assert_eq!(depth, n + 1, "common {n}");
            assert!(!map.is_unstable(&name), "common {n}");
        }
    }
    assert!(map.is_unstable(&map.name(face).unwrap()));
}
