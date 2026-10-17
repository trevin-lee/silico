//! M2.7: element maps: their rules on histories written by hand (expected names worked from
//! FreeCAD's algorithm), and each built feature's map in a recompute, on the fake kernel.
//! FreeCAD's own names, on OCCT's histories, are checked in silico-kernel's tests/naming.rs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/example.rs"]
mod example;
#[path = "common/model.rs"]
mod model;

use std::sync::Arc;

use example::EXAMPLE;
use model::{checked, described, id, made, published};
use silico_core::ErrorCode;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    ElementCounts, ElementFate, ElementKind, ElementRef, History, InputHistory, Op, Primitive,
    Reported, Topology,
};
use silico_core::naming::{ElementMap, Name, OpCode, freecad};
use silico_core::recompute::Recompute;

/// A box's faces, edges, and vertices, as OCCT makes it.
const BOX: ElementCounts = ElementCounts {
    faces: 6,
    edges: 12,
    vertices: 8,
    solids: 1,
};

fn element(kind: ElementKind, index: u32) -> ElementRef {
    ElementRef { kind, index }
}

fn kept(index: u32) -> ElementFate {
    ElementFate {
        kept: Some(index),
        ..ElementFate::default()
    }
}

fn names(map: &ElementMap, kind: ElementKind, count: u32) -> Vec<String> {
    (1..=count)
        .map(|i| map.name(element(kind, i)).unwrap().to_string())
        .collect()
}

#[test]
fn two_elements_that_come_about_alike_are_numbered_and_marked() {
    // Two edges joining the same two vertices (a ruled surface's, in FreeCAD's case), named
    // from those vertices alike: the first in FreeCAD's order keeps the name, the second is
    // its `~1`, and the name is marked duplicated (N3).
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
        },
    };
    history.validate(1).unwrap();
    let input = ElementMap::primitive(id("aaaaaaaa"), BOX);
    let map = ElementMap::operation(id("rrrrrrrr"), OpCode::Fuse, &[&input], &history);
    assert_eq!(
        names(&map, ElementKind::Edge, 2),
        [
            "rrrrrrrr.Edge(L aaaaaaaa.Vertex1, aaaaaaaa.Vertex2)",
            "rrrrrrrr.Edge(L aaaaaaaa.Vertex1, aaaaaaaa.Vertex2)~1",
        ]
    );
    let first = map.name(element(ElementKind::Edge, 1)).unwrap();
    assert!(map.is_unstable(&first));
    assert!(!map.is_unstable(&map.name(element(ElementKind::Vertex, 1)).unwrap()));
    // FreeCAD's form: `;D1`, as its debug builds number them.
    assert_eq!(
        freecad::text(&map.name(element(ElementKind::Edge, 2)).unwrap()),
        "Vertex1;:L(Vertex2);FUS;D1"
    );
    // Each name finds its own element.
    let second = map.name(element(ElementKind::Edge, 2)).unwrap();
    assert_eq!(map.element(&first), Some(element(ElementKind::Edge, 1)));
    assert_eq!(map.element(&second), Some(element(ElementKind::Edge, 2)));
}

fn reported(kind: ElementKind, index: u32, position: u32) -> Reported {
    Reported {
        element: element(kind, index),
        position,
    }
}

/// A one-input history: what became of the box's faces and edges (the rest gone), the
/// result's counts, and its topology.
fn one_input(
    faces: Vec<ElementFate>,
    edges: Vec<ElementFate>,
    result: (u32, u32),
    face_edges: Vec<Vec<u32>>,
    outer_edges: Vec<Vec<u32>>,
) -> History {
    let history = History {
        inputs: vec![InputHistory {
            faces,
            edges,
            vertices: vec![],
        }],
        result: ElementCounts {
            faces: result.0,
            edges: result.1,
            vertices: 0,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges,
            outer_edges,
            edge_vertices: vec![vec![]; result.1 as usize],
        },
    };
    history.validate(1).unwrap();
    history
}

fn cut_of_box(history: &History) -> ElementMap {
    let input = ElementMap::primitive(id("aaaaaaaa"), BOX);
    ElementMap::operation(id("rrrrrrrr"), OpCode::Cut, &[&input], history)
}

#[test]
fn an_edge_named_from_above_counts_only_the_unnamed_edges_before_it() {
    // Face1 modified into the result's Face1, bounded by Edge1 (the box's Edge1, kept) and
    // Edge2 (named by nothing): Edge2 is Face1's first unnamed edge, `U`, not its second
    // edge, `U2`, as FreeCAD counts only unnamed ones (TopoShapeExpansion.cpp:1869-1880,
    // its counter past `if (getMappedName(indexedName)) continue;`). Found by the review.
    let history = one_input(
        vec![ElementFate {
            modified: vec![reported(ElementKind::Face, 1, 1)],
            ..ElementFate::default()
        }],
        vec![kept(1)],
        (1, 2),
        vec![vec![1, 2]],
        vec![vec![1, 2]],
    );
    let edge2 = cut_of_box(&history)
        .name(element(ElementKind::Edge, 2))
        .unwrap();
    assert_eq!(freecad::text(&edge2), "Face1;:M;CUT;:U;CUT");
    assert_eq!(edge2.to_string(), "rrrrrrrr.Edge(U aaaaaaaa.Face1)");
}

#[test]
fn each_sources_place_is_its_rank_in_that_sources_own_list() {
    // Face1 generates Edges 1 and 2, in that order; Face2 generates them the other way
    // round. The two come about alike (generated from both faces), so each holds its place
    // in each source's list: Edge1 is first in Face1's and second in Face2's. Found by the
    // review: a place copied from the first source to the others passed every test.
    let both = |first: u32, second: u32| ElementFate {
        generated: vec![
            reported(ElementKind::Edge, 1, first),
            reported(ElementKind::Edge, 2, second),
        ],
        ..ElementFate::default()
    };
    let history = one_input(vec![both(1, 2), both(2, 1)], vec![], (0, 2), vec![], vec![]);
    let map = cut_of_box(&history);
    assert_eq!(
        names(&map, ElementKind::Edge, 2),
        [
            "rrrrrrrr.Edge(G1/2 aaaaaaaa.Face1, G2/2 aaaaaaaa.Face2)",
            "rrrrrrrr.Edge(G2/2 aaaaaaaa.Face1, G1/2 aaaaaaaa.Face2)",
        ]
    );
    for index in [1, 2] {
        assert!(!map.is_unstable(&map.name(element(ElementKind::Edge, index)).unwrap()));
    }
}

#[test]
fn a_face_keeps_its_sources_name_only_as_its_first_and_only_result_of_its_kind() {
    // N7's one from one, its two other conditions (found untested by the review):
    // modified into the result's Face1, but second in OCCT's list, it's derived, as FreeCAD
    // writes it (`;:M2`; a sole result at position 1 is the source's own name)...
    let second = one_input(
        vec![ElementFate {
            modified: vec![reported(ElementKind::Face, 1, 2)],
            ..ElementFate::default()
        }],
        vec![],
        (1, 0),
        vec![vec![]],
        vec![vec![]],
    );
    let face = cut_of_box(&second)
        .name(element(ElementKind::Face, 1))
        .unwrap();
    assert_eq!(freecad::text(&face), "Face1;:M2;CUT");
    assert_eq!(face.to_string(), "rrrrrrrr.Face(M aaaaaaaa.Face1)");
    // ...and modified into Face1 while it generated Face2 too, it's one of two results of
    // its kind, so derived.
    let also_generated = one_input(
        vec![ElementFate {
            modified: vec![reported(ElementKind::Face, 1, 1)],
            generated: vec![reported(ElementKind::Face, 2, 1)],
            ..ElementFate::default()
        }],
        vec![],
        (2, 0),
        vec![vec![], vec![]],
        vec![vec![], vec![]],
    );
    let map = cut_of_box(&also_generated);
    assert_eq!(
        names(&map, ElementKind::Face, 2),
        [
            "rrrrrrrr.Face(M aaaaaaaa.Face1)",
            "rrrrrrrr.Face(G aaaaaaaa.Face1)",
        ]
    );
    // ...but an edge it generated is of another kind: the face is still its sole result
    // of its own, and keeps its name (found untested by the review).
    let and_an_edge = one_input(
        vec![ElementFate {
            modified: vec![reported(ElementKind::Face, 1, 1)],
            generated: vec![reported(ElementKind::Edge, 1, 1)],
            ..ElementFate::default()
        }],
        vec![],
        (1, 1),
        vec![vec![1]],
        vec![vec![1]],
    );
    let map = cut_of_box(&and_an_edge);
    assert_eq!(names(&map, ElementKind::Face, 1), ["aaaaaaaa.Face1"]);
    assert_eq!(
        names(&map, ElementKind::Edge, 1),
        ["rrrrrrrr.Edge(G aaaaaaaa.Face1)"]
    );
}

#[test]
fn a_face_is_named_from_below_only_when_every_edge_of_its_outer_wire_is_named() {
    // A face nothing reports, its outer wire Edge1 (the box's, kept) and Edge2 (named by
    // nothing, with no named face above it): FreeCAD names a face from below only if every
    // lower element is named (TopoShapeExpansion.cpp:1933-1941), so it's named by its
    // number, unstable, not from Edge1 alone. Found by the review.
    let history = one_input(
        vec![],
        vec![kept(1)],
        (1, 2),
        vec![vec![1, 2]],
        vec![vec![1, 2]],
    );
    let map = cut_of_box(&history);
    let face = map.name(element(ElementKind::Face, 1)).unwrap();
    assert_eq!(face.to_string(), "rrrrrrrr.Face1");
    assert!(map.is_unstable(&face));
}

#[test]
fn a_face_kept_and_modified_too_is_a_duplicate_and_both_are_marked() {
    // A history (a broken worker's, or an operation OCCT reports oddly) that keeps the
    // input's Face1 as the result's Face1 and modifies it, as its sole result, into Face2:
    // the modified face is the source's own name (Trevin's decision), so the two coincide.
    // The kept face, named first, keeps the name; the other is its `~1`; and both are
    // unstable, so no reference resolves to either (N3).
    let history = History {
        inputs: vec![InputHistory {
            faces: vec![ElementFate {
                kept: Some(1),
                modified: vec![Reported {
                    element: element(ElementKind::Face, 2),
                    position: 1,
                }],
                ..ElementFate::default()
            }],
            edges: vec![],
            vertices: vec![],
        }],
        result: ElementCounts {
            faces: 2,
            edges: 0,
            vertices: 0,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![vec![], vec![]],
            outer_edges: vec![vec![], vec![]],
            edge_vertices: vec![],
        },
    };
    history.validate(1).unwrap();
    let input = ElementMap::primitive(id("aaaaaaaa"), BOX);
    let map = ElementMap::operation(id("rrrrrrrr"), OpCode::Cut, &[&input], &history);
    assert_eq!(
        names(&map, ElementKind::Face, 2),
        ["aaaaaaaa.Face1", "aaaaaaaa.Face1~1"]
    );
    for index in [1, 2] {
        let name = map.name(element(ElementKind::Face, index)).unwrap();
        assert!(map.is_unstable(&name), "Face{index}");
        assert_eq!(map.element(&name), Some(element(ElementKind::Face, index)));
    }
}

#[test]
fn an_element_named_from_above_takes_the_first_named_by_freecads_order() {
    // One input face (Face1) modified into the result's Face1; the result's Face2 generated
    // from the input's Edge1. Edge3 bounds both faces, unnamed: named from the face whose
    // name sorts first by FreeCAD's comparator, `Edge1;:G;FLT` before `Face1;:M;FLT`, as
    // the 1st unnamed edge of it. Edge1 and Edge2 are each in one face only.
    let reported = |kind, index| Reported {
        element: element(kind, index),
        position: 1,
    };
    let history = History {
        inputs: vec![InputHistory {
            faces: vec![ElementFate {
                modified: vec![reported(ElementKind::Face, 1)],
                ..ElementFate::default()
            }],
            edges: vec![ElementFate {
                generated: vec![reported(ElementKind::Face, 2)],
                ..ElementFate::default()
            }],
            vertices: vec![],
        }],
        result: ElementCounts {
            faces: 2,
            edges: 3,
            vertices: 0,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![vec![1, 3], vec![3, 2]],
            outer_edges: vec![vec![1, 3], vec![2, 3]],
            edge_vertices: vec![vec![], vec![], vec![]],
        },
    };
    history.validate(1).unwrap();
    let input = ElementMap::primitive(id("aaaaaaaa"), BOX);
    let map = ElementMap::operation(id("ffffffff"), OpCode::Fillet, &[&input], &history);
    assert_eq!(
        names(&map, ElementKind::Face, 2),
        [
            // Modified from it alone, first in OCCT's list: the face as modified.
            "aaaaaaaa.Face1",
            "ffffffff.Face(G aaaaaaaa.Edge1)",
        ]
    );
    let generated = "ffffffff.Face(G aaaaaaaa.Edge1)";
    let modified = "aaaaaaaa.Face1";
    assert_eq!(
        names(&map, ElementKind::Edge, 3),
        [
            // Face1's 1st unnamed edge, Face2's 2nd (Edge3 its 1st, taken by its order).
            format!("ffffffff.Edge(U {modified})"),
            format!("ffffffff.Edge(U2 {generated})"),
            format!("ffffffff.Edge(U {generated})"),
        ]
    );
}

#[test]
fn an_element_with_no_name_from_history_or_neighbours_is_named_by_its_number() {
    // A face nothing reports, with no edges: named as FreeCAD's references would name it,
    // by its number in what the feature made.
    let history = History {
        inputs: vec![InputHistory::default()],
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
        },
    };
    let input = ElementMap::primitive(id("aaaaaaaa"), BOX);
    let map = ElementMap::operation(id("rrrrrrrr"), OpCode::Cut, &[&input], &history);
    assert_eq!(names(&map, ElementKind::Face, 1), ["rrrrrrrr.Face1"]);
}

#[test]
fn each_built_feature_has_its_solids_names_from_its_inputs() {
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    let model = published(&engine, checked(EXAMPLE));
    // The block and the pin: by their numbers (the pin moved, which keeps them).
    let block = model.element_map(&id("k7f2qx3m")).unwrap();
    assert_eq!(
        block
            .name(element(ElementKind::Face, 6))
            .unwrap()
            .to_string(),
        "k7f2qx3m.Face6"
    );
    let pin = model.element_map(&id("a91cw0de")).unwrap();
    assert_eq!(
        pin.name(element(ElementKind::Edge, 3)).unwrap().to_string(),
        "a91cw0de.Edge3"
    );
    // The fake's cut keeps both solids whole, the pin's elements after the block's: so each
    // keeps its name (N2), and the map has the block's 6 faces, 12 edges, 8 vertices, then
    // the pin's 3, 3, 2.
    let hole = model.element_map(&id("m3tq8v0z")).unwrap();
    assert_eq!(
        described(&model, "m3tq8v0z"),
        "cut(box(40, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))"
    );
    assert_eq!(hole.iter().count(), 26 + 8);
    for (kind, index, name) in [
        (ElementKind::Face, 1, "k7f2qx3m.Face1"),
        (ElementKind::Face, 7, "a91cw0de.Face1"),
        (ElementKind::Edge, 12, "k7f2qx3m.Edge12"),
        (ElementKind::Edge, 13, "a91cw0de.Edge1"),
        (ElementKind::Vertex, 10, "a91cw0de.Vertex2"),
    ] {
        let named = hole.name(element(kind, index)).unwrap();
        assert_eq!(named.to_string(), name);
        assert_eq!(hole.element(&named), Some(element(kind, index)));
    }
}

#[test]
fn renaming_a_feature_renames_its_elements_without_asking_the_kernel() {
    // Names hold IDs; recompute's keys don't, so a renamed feature costs no kernel call.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    published(&engine, checked(EXAMPLE));
    let before = engine.kernel().calls().len();
    let renamed = EXAMPLE.replace("a91cw0de", "pqrs2345");
    let model = published(&engine, checked(&renamed));
    assert!(made(&engine.kernel().calls()[before..]).is_empty());
    let hole = model.element_map(&id("m3tq8v0z")).unwrap();
    assert_eq!(
        hole.name(element(ElementKind::Face, 7))
            .unwrap()
            .to_string(),
        "pqrs2345.Face1"
    );
}

#[test]
fn a_feature_that_doesnt_build_has_no_names() {
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
            .then(|| silico_core::Error::new(ErrorCode::KernelFailed, "MakeCylinder failed"))
    });
    let model = published(&engine, checked(EXAMPLE));
    assert!(model.element_map(&id("k7f2qx3m")).is_some());
    assert!(model.element_map(&id("a91cw0de")).is_none());
    assert!(model.element_map(&id("m3tq8v0z")).is_none());
    assert!(model.element_map(&id("zzzzzzzz")).is_none());
}

#[test]
fn a_primitives_map_names_each_of_its_elements_and_no_others() {
    // A box: its 6 faces, 12 edges, and 8 vertices, each by its number, and nothing past
    // them (the review found a primitive's map answering for any number).
    let map = ElementMap::primitive(id("aaaaaaaa"), BOX);
    assert_eq!(map.iter().count(), 26);
    let vertex = |index| Name::Element {
        feature: id("aaaaaaaa"),
        element: element(ElementKind::Vertex, index),
    };
    assert_eq!(
        map.element(&vertex(8)),
        Some(element(ElementKind::Vertex, 8))
    );
    assert_eq!(map.element(&vertex(9)), None);
    assert!(map.names(element(ElementKind::Vertex, 9)).is_empty());
    assert_eq!(
        map.names(element(ElementKind::Vertex, 8)),
        vec![Arc::new(vertex(8))]
    );
    assert_eq!(map.feature(), &id("aaaaaaaa"));
}

#[test]
fn an_edit_names_only_the_features_it_changes() {
    // A plate and 60 bolts fused onto it one feature at a time. Moving the last bolt
    // renames only its fuse: every other feature's map is the one the model before had,
    // not built again (`/break-it` found every map rebuilt on every edit, quadratic in the
    // timeline). Moving the first bolt renames every fuse from its own on.
    let document = |first: u32, last: u32| {
        let mut features = vec![r#"{"id": "pppppppp", "type": "box", "v": 1, "name": "P", "position": [0, 0, 0], "length": 1000, "width": 10, "height": 1}"#.to_owned()];
        let mut base = "pppppppp".to_owned();
        for n in 0..60u32 {
            let x = match n {
                0 => first,
                59 => last,
                _ => 10 * n,
            };
            features.push(format!(r#"{{"id": "b{n:07}", "type": "box", "v": 1, "name": "B", "position": [{x}, 0, 1], "length": 1, "width": 1, "height": 1}}"#));
            let fuse = format!("f{n:07}");
            features.push(format!(r#"{{"id": "{fuse}", "type": "fuse", "v": 1, "name": "F", "base": "{base}", "tool": "b{n:07}"}}"#));
            base = fuse;
        }
        checked(&format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{}]}}"#,
            features.join(", ")
        ))
    };
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    let before = published(&engine, document(0, 590));
    let maps = |model: &silico_core::recompute::Model, n: u32| {
        model.element_map(&id(&format!("f{n:07}"))).unwrap() as *const ElementMap
    };
    let after = published(&engine, document(0, 595));
    for n in 0..59 {
        assert_eq!(maps(&before, n), maps(&after, n), "f{n:07} was named again");
    }
    assert_ne!(maps(&before, 59), maps(&after, 59));
    let moved = published(&engine, document(5, 595));
    for n in 0..60 {
        assert_ne!(
            maps(&after, n),
            maps(&moved, n),
            "f{n:07} wasn't named again"
        );
    }
    // Named alike either way: the bolts' names don't hold where they are.
    assert_eq!(
        before.element_map(&id("f0000059")).unwrap().iter().count(),
        after.element_map(&id("f0000059")).unwrap().iter().count()
    );
}

#[test]
fn a_name_lists_at_most_five_sources() {
    // Six input faces all modified into the result's one face: the first and four more, as
    // FreeCAD's names list them (TopoShapeExpansion.cpp:1774, `if (++count == 4) break;`).
    let modified = || ElementFate {
        modified: vec![Reported {
            element: element(ElementKind::Face, 1),
            position: 1,
        }],
        ..ElementFate::default()
    };
    let history = History {
        inputs: vec![InputHistory {
            faces: (0..6).map(|_| modified()).collect(),
            edges: vec![],
            vertices: vec![],
        }],
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
        },
    };
    history.validate(1).unwrap();
    let input = ElementMap::primitive(id("aaaaaaaa"), BOX);
    let map = ElementMap::operation(id("rrrrrrrr"), OpCode::Fuse, &[&input], &history);
    assert_eq!(
        map.name(element(ElementKind::Face, 1)).unwrap().to_string(),
        "rrrrrrrr.Face(M aaaaaaaa.Face1, M aaaaaaaa.Face2, M aaaaaaaa.Face3, M aaaaaaaa.Face4, M aaaaaaaa.Face5)"
    );
}

#[test]
fn a_face_named_from_below_lists_at_most_five_edges() {
    // A hexagonal face nothing reports, its six edges kept: named from the first and four
    // more (TopoShapeExpansion.cpp:1988).
    let history = History {
        inputs: vec![InputHistory {
            faces: vec![],
            edges: (1..=6).map(kept).collect(),
            vertices: vec![],
        }],
        result: ElementCounts {
            faces: 1,
            edges: 6,
            vertices: 0,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![vec![1, 2, 3, 4, 5, 6]],
            outer_edges: vec![vec![1, 2, 3, 4, 5, 6]],
            edge_vertices: vec![vec![]; 6],
        },
    };
    history.validate(1).unwrap();
    let input = ElementMap::primitive(id("aaaaaaaa"), BOX);
    let map = ElementMap::operation(id("rrrrrrrr"), OpCode::Fuse, &[&input], &history);
    assert_eq!(
        map.name(element(ElementKind::Face, 1)).unwrap().to_string(),
        "rrrrrrrr.Face(L aaaaaaaa.Edge1, aaaaaaaa.Edge2, aaaaaaaa.Edge3, aaaaaaaa.Edge4, aaaaaaaa.Edge5)"
    );
}

#[test]
fn a_name_derived_from_an_ambiguous_name_is_ambiguous() {
    // Two edges alike (`X`, `X~1`); the next feature modifies `X`: its new name is built on
    // `X`, so which edge it names still depends on how they were numbered (N3).
    let topology = Topology {
        face_edges: vec![],
        outer_edges: vec![],
        edge_vertices: vec![vec![1, 2], vec![2, 1]],
    };
    let counts = ElementCounts {
        faces: 0,
        edges: 2,
        vertices: 2,
        solids: 0,
    };
    let first_history = History {
        inputs: vec![InputHistory {
            faces: vec![],
            edges: vec![],
            vertices: vec![kept(1), kept(2)],
        }],
        result: counts,
        ends: Default::default(),
        topology: topology.clone(),
    };
    let input = ElementMap::primitive(id("aaaaaaaa"), BOX);
    let first = ElementMap::operation(id("pppppppp"), OpCode::Fuse, &[&input], &first_history);
    let x = first.name(element(ElementKind::Edge, 1)).unwrap();
    assert!(first.is_unstable(&x));
    let next_history = History {
        inputs: vec![InputHistory {
            faces: vec![],
            edges: vec![
                ElementFate {
                    modified: vec![Reported {
                        element: element(ElementKind::Edge, 1),
                        position: 1,
                    }],
                    ..ElementFate::default()
                },
                kept(2),
            ],
            vertices: vec![kept(1), kept(2)],
        }],
        result: counts,
        ends: Default::default(),
        topology,
    };
    next_history.validate(1).unwrap();
    let next = ElementMap::operation(id("nnnnnnnn"), OpCode::Fillet, &[&first], &next_history);
    let derived = next.name(element(ElementKind::Edge, 1)).unwrap();
    assert!(matches!(&*derived, Name::Modified { .. }), "{derived}");
    assert!(
        next.is_unstable(&derived),
        "{derived} derives from ambiguous {x}"
    );
    // A plain kept vertex isn't.
    assert!(!next.is_unstable(&next.name(element(ElementKind::Vertex, 1)).unwrap()));
}

#[test]
fn a_cancel_after_publishing_doesnt_stop_the_published_models_names() {
    // Found by the review: a cancel (with nothing running) moved the generation on, and the
    // published model's background naming stopped for good, so `named_element_map` said
    // `None` for its later features forever. Naming stops only for a newer model.
    let (text, base) = plate(300);
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    let model = published(&engine, checked(&text));
    engine.cancel();
    let last = id(&base);
    let deadline = std::time::Instant::now() + 6 * model::WAIT;
    while model.named_element_map(&last).is_none() {
        assert!(std::time::Instant::now() < deadline, "never named");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(engine.published().as_ref(), Some(&model));
}

#[test]
fn a_published_models_names_are_made_in_the_background() {
    // Nothing asks for them, yet they come: a caller that mustn't wait (the UI's thread)
    // finds them with `named_element_map`, which never names anything itself.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    let model = published(&engine, checked(EXAMPLE));
    let deadline = std::time::Instant::now() + model::WAIT;
    for feature in ["k7f2qx3m", "a91cw0de", "m3tq8v0z"] {
        while model.named_element_map(&id(feature)).is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "{feature} wasn't named"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            model.named_element_map(&id(feature)),
            model.element_map(&id(feature))
        );
    }
    assert!(model.named_element_map(&id("zzzzzzzz")).is_none());
}

/// A plate with `holes` holes drilled one by one, a cut a hole: its text, and the last
/// cut's ID, the last feature named.
fn plate(holes: u32) -> (String, String) {
    let mut features = vec![
        r#"{"id": "b0000000", "type": "box", "v": 1, "name": "b", "position": [0, 0, 0], "length": 400, "width": 20, "height": 5}"#.to_owned(),
    ];
    let mut base = "b0000000".to_owned();
    for k in 1..=holes {
        features.push(format!(r#"{{"id": "y{k:07}", "type": "cylinder", "v": 1, "name": "p", "position": [{k}, 10, -1], "radius": 0.25, "height": 7}}"#));
        features.push(format!(r#"{{"id": "c{k:07}", "type": "cut", "v": 1, "name": "h", "base": "{base}", "tool": "y{k:07}"}}"#));
        base = format!("c{k:07}");
    }
    (
        format!(
            r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{}]}}"#,
            features.join(", ")
        ),
        base,
    )
}

#[test]
fn a_newer_recompute_that_publishes_nothing_doesnt_stop_the_published_models_names() {
    // Found by the review: only a cancel was tested; naming stopped by a newer start
    // would pass it.
    let (text, last) = plate(300);
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    let model = published(&engine, checked(&text));
    // A newer recompute, cancelled: it publishes nothing.
    let run = engine.start(checked(EXAMPLE));
    engine.cancel();
    let _ = run.wait_timeout(model::WAIT);
    let last = id(&last);
    let deadline = std::time::Instant::now() + 6 * model::WAIT;
    while model.named_element_map(&last).is_none() {
        assert!(std::time::Instant::now() < deadline, "never named");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(engine.published().as_ref(), Some(&model));
}

/// Waits until the background naming thread has let `model` go (only the test holds it).
fn released(model: &Arc<silico_core::recompute::Model>) {
    let deadline = std::time::Instant::now() + 30 * model::WAIT;
    while Arc::strong_count(model) > 1 {
        assert!(std::time::Instant::now() < deadline, "never released");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn a_newer_published_model_stops_the_older_ones_names() {
    // Found by the review: that naming stops, and when, was untested. A newer model is
    // the one to name; the older is let go, its last features unnamed.
    let (text, last) = plate(300);
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    let model = published(&engine, checked(&text));
    let _newer = published(&engine, checked(EXAMPLE));
    released(&model);
    assert!(
        model.named_element_map(&id(&last)).is_none(),
        "the old model was named on"
    );
}

#[test]
fn dropping_the_engine_stops_the_published_models_names() {
    let (text, last) = plate(300);
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    let model = published(&engine, checked(&text));
    drop(engine);
    released(&model);
    assert!(
        model.named_element_map(&id(&last)).is_none(),
        "named on after the drop"
    );
}

#[test]
fn a_place_is_a_rank_among_the_members_not_occts_position() {
    // Edges 2 and 3 come about alike, generated from both faces, at positions 2 and 3 in
    // Face1's list and 3 and 2 in Face2's, each list with an edge of its own first (Edge1
    // in Face1's, Edge4 in Face2's). They're the group's only members, so their places are
    // their ranks among themselves, 1/2 and 2/2, not OCCT's positions (found by the
    // review: a place written as the raw position passed every test).
    let face1 = ElementFate {
        generated: vec![
            reported(ElementKind::Edge, 1, 1),
            reported(ElementKind::Edge, 2, 2),
            reported(ElementKind::Edge, 3, 3),
        ],
        ..ElementFate::default()
    };
    let face2 = ElementFate {
        generated: vec![
            reported(ElementKind::Edge, 2, 3),
            reported(ElementKind::Edge, 3, 2),
            reported(ElementKind::Edge, 4, 1),
        ],
        ..ElementFate::default()
    };
    let history = one_input(vec![face1, face2], vec![], (0, 4), vec![], vec![]);
    let map = cut_of_box(&history);
    let got = names(&map, ElementKind::Edge, 4);
    assert_eq!(
        got[1..3],
        [
            "rrrrrrrr.Edge(G1/2 aaaaaaaa.Face1, G2/2 aaaaaaaa.Face2)",
            "rrrrrrrr.Edge(G2/2 aaaaaaaa.Face1, G1/2 aaaaaaaa.Face2)",
        ]
    );
}

#[test]
fn a_place_skips_the_non_members_between_members() {
    // Face1 lists Edge2, then Edge1 (generated from Face1 alone, not a member), then
    // Edge3; Face2 lists Edge3, then Edge2. Edge3's place in Face1's list is 2/2, not 3/2:
    // a rank among the members, which an element between them doesn't move (found by the
    // review: a place counted from the first member's position passed every test).
    let face1 = ElementFate {
        generated: vec![
            reported(ElementKind::Edge, 1, 2),
            reported(ElementKind::Edge, 2, 1),
            reported(ElementKind::Edge, 3, 3),
        ],
        ..ElementFate::default()
    };
    let face2 = ElementFate {
        generated: vec![
            reported(ElementKind::Edge, 2, 2),
            reported(ElementKind::Edge, 3, 1),
        ],
        ..ElementFate::default()
    };
    let history = one_input(vec![face1, face2], vec![], (0, 3), vec![], vec![]);
    let map = cut_of_box(&history);
    let got = names(&map, ElementKind::Edge, 3);
    assert_eq!(
        got[1..3],
        [
            "rrrrrrrr.Edge(G1/2 aaaaaaaa.Face1, G2/2 aaaaaaaa.Face2)",
            "rrrrrrrr.Edge(G2/2 aaaaaaaa.Face1, G1/2 aaaaaaaa.Face2)",
        ]
    );
}
