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
use silico_core::naming::{ElementMap, Name, OpCode, TypeWord, freecad};
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
    let input = ElementMap::by_number(id("aaaaaaaa"), OpCode::Fuse, BOX);
    let map = ElementMap::operation(id("rrrrrrrr"), OpCode::Fuse, &[&input], &history);
    assert_eq!(
        names(&map, ElementKind::Edge, 2),
        [
            "rrrrrrrr.fuse.Edge(L aaaaaaaa.fuse.Vertex1, aaaaaaaa.fuse.Vertex2)",
            "rrrrrrrr.fuse.Edge(L aaaaaaaa.fuse.Vertex1, aaaaaaaa.fuse.Vertex2)~1",
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
    let input = ElementMap::by_number(id("aaaaaaaa"), OpCode::Fuse, BOX);
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
    assert_eq!(
        edge2.to_string(),
        "rrrrrrrr.cut.Edge(U aaaaaaaa.fuse.Face1)"
    );
}

#[test]
fn a_source_that_both_modified_and_generated_an_element_named_it_by_generating() {
    // Found by the milestone's tests review: no history had one source both modify and
    // generate one element, so which relation counts was untested. Generated, as FreeCAD's
    // map takes the last it's told (TopoShapeExpansion.cpp:1508-1555, generated after
    // modified): Face1 modified into the result's Face1 first in its list, and generated
    // it second.
    let history = one_input(
        vec![ElementFate {
            modified: vec![reported(ElementKind::Face, 1, 1)],
            generated: vec![reported(ElementKind::Face, 1, 2)],
            ..ElementFate::default()
        }],
        vec![],
        (1, 0),
        vec![vec![]],
        vec![vec![]],
    );
    let face1 = cut_of_box(&history)
        .name(element(ElementKind::Face, 1))
        .unwrap();
    assert_eq!(freecad::text(&face1), "Face1;:G2;CUT");
    assert_eq!(
        face1.to_string(),
        "rrrrrrrr.cut.Face(G aaaaaaaa.fuse.Face1)"
    );
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
            "rrrrrrrr.cut.Edge(G1/2 aaaaaaaa.fuse.Face1, G2/2 aaaaaaaa.fuse.Face2)",
            "rrrrrrrr.cut.Edge(G2/2 aaaaaaaa.fuse.Face1, G1/2 aaaaaaaa.fuse.Face2)",
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
    assert_eq!(face.to_string(), "rrrrrrrr.cut.Face(M aaaaaaaa.fuse.Face1)");
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
            "rrrrrrrr.cut.Face(M aaaaaaaa.fuse.Face1)",
            "rrrrrrrr.cut.Face(G aaaaaaaa.fuse.Face1)",
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
    assert_eq!(names(&map, ElementKind::Face, 1), ["aaaaaaaa.fuse.Face1"]);
    assert_eq!(
        names(&map, ElementKind::Edge, 1),
        ["rrrrrrrr.cut.Edge(G aaaaaaaa.fuse.Face1)"]
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
    assert_eq!(face.to_string(), "rrrrrrrr.cut.Face1");
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
    let input = ElementMap::by_number(id("aaaaaaaa"), OpCode::Fuse, BOX);
    let map = ElementMap::operation(id("rrrrrrrr"), OpCode::Cut, &[&input], &history);
    assert_eq!(
        names(&map, ElementKind::Face, 2),
        ["aaaaaaaa.fuse.Face1", "aaaaaaaa.fuse.Face1~1"]
    );
    for index in [1, 2] {
        let name = map.name(element(ElementKind::Face, index)).unwrap();
        assert!(map.is_unstable(&name), "Face{index}");
        assert_eq!(map.element(&name), Some(element(ElementKind::Face, index)));
    }
}

#[test]
fn an_element_named_from_above_is_named_by_every_named_element_it_bounds() {
    // One input face (Face1) modified into the result's Face1; the result's Face2 generated
    // from the input's Edge1. The edges are unnamed: each is named from every named face it
    // bounds (Trevin's, 2026-10-17, departing from FreeCAD's first face only), those in
    // FreeCAD's order, `Edge1;:G;FLT` before `Face1;:M;FLT`. Edge1 is in Face1 only, Edge2
    // in Face2 only, Edge3 in both: each the only edge of its set, so none has a place.
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
    let input = ElementMap::by_number(id("aaaaaaaa"), OpCode::Fuse, BOX);
    let map = ElementMap::operation(id("ffffffff"), OpCode::Fillet, &[&input], &history);
    assert_eq!(
        names(&map, ElementKind::Face, 2),
        [
            // Modified from it alone, first in OCCT's list: the face as modified.
            "aaaaaaaa.fuse.Face1",
            "ffffffff.fillet.Face(G aaaaaaaa.fuse.Edge1)",
        ]
    );
    let generated = "ffffffff.fillet.Face(G aaaaaaaa.fuse.Edge1)";
    let modified = "aaaaaaaa.fuse.Face1";
    assert_eq!(
        names(&map, ElementKind::Edge, 3),
        [
            format!("ffffffff.fillet.Edge(U {modified})"),
            format!("ffffffff.fillet.Edge(U {generated})"),
            format!("ffffffff.fillet.Edge(U {generated}, {modified})"),
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
    let input = ElementMap::by_number(id("aaaaaaaa"), OpCode::Fuse, BOX);
    let map = ElementMap::operation(id("rrrrrrrr"), OpCode::Cut, &[&input], &history);
    assert_eq!(names(&map, ElementKind::Face, 1), ["rrrrrrrr.cut.Face1"]);
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
        "k7f2qx3m.box.Face6"
    );
    let pin = model.element_map(&id("a91cw0de")).unwrap();
    assert_eq!(
        pin.name(element(ElementKind::Edge, 3)).unwrap().to_string(),
        "a91cw0de.cylinder.Edge3"
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
        (ElementKind::Face, 1, "k7f2qx3m.box.Face1"),
        (
            ElementKind::Face,
            7,
            "m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face1)",
        ),
        (ElementKind::Edge, 12, "k7f2qx3m.box.Edge12"),
        (
            ElementKind::Edge,
            13,
            "m3tq8v0z.cut.Edge(I a91cw0de.cylinder.Edge1)",
        ),
        (
            ElementKind::Vertex,
            10,
            "m3tq8v0z.cut.Vertex(I a91cw0de.cylinder.Vertex2)",
        ),
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
        "m3tq8v0z.cut.Face(I pqrs2345.cylinder.Face1)"
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
    let map = ElementMap::by_number(id("aaaaaaaa"), OpCode::Fuse, BOX);
    assert_eq!(map.iter().count(), 26);
    let vertex = |index| Name::Element {
        feature: id("aaaaaaaa"),
        made: TypeWord::Operation(OpCode::Fuse),
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
    let input = ElementMap::by_number(id("aaaaaaaa"), OpCode::Fuse, BOX);
    let map = ElementMap::operation(id("rrrrrrrr"), OpCode::Fuse, &[&input], &history);
    assert_eq!(
        map.name(element(ElementKind::Face, 1)).unwrap().to_string(),
        "rrrrrrrr.fuse.Face(M aaaaaaaa.fuse.Face1, M aaaaaaaa.fuse.Face2, M aaaaaaaa.fuse.Face3, M aaaaaaaa.fuse.Face4, M aaaaaaaa.fuse.Face5)"
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
    let input = ElementMap::by_number(id("aaaaaaaa"), OpCode::Fuse, BOX);
    let map = ElementMap::operation(id("rrrrrrrr"), OpCode::Fuse, &[&input], &history);
    assert_eq!(
        map.name(element(ElementKind::Face, 1)).unwrap().to_string(),
        "rrrrrrrr.fuse.Face(L aaaaaaaa.fuse.Edge1, aaaaaaaa.fuse.Edge2, aaaaaaaa.fuse.Edge3, aaaaaaaa.fuse.Edge4, aaaaaaaa.fuse.Edge5)"
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
    let input = ElementMap::by_number(id("aaaaaaaa"), OpCode::Fuse, BOX);
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
            "rrrrrrrr.cut.Edge(G1/2 aaaaaaaa.fuse.Face1, G2/2 aaaaaaaa.fuse.Face2)",
            "rrrrrrrr.cut.Edge(G2/2 aaaaaaaa.fuse.Face1, G1/2 aaaaaaaa.fuse.Face2)",
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
            "rrrrrrrr.cut.Edge(G1/2 aaaaaaaa.fuse.Face1, G2/2 aaaaaaaa.fuse.Face2)",
            "rrrrrrrr.cut.Edge(G2/2 aaaaaaaa.fuse.Face1, G1/2 aaaaaaaa.fuse.Face2)",
        ]
    );
}

#[test]
fn a_non_member_before_a_middle_member_doesnt_move_its_rank() {
    // A group of three: Edges 2, 3, and 4, generated alike from both faces, with Face1's
    // Edge1 (not a member) listed between the first and the middle member. The middle
    // member's rank is 2/3, counted among the members alone (found by the review: a rank
    // counted from the first member's position, held to the group's size, passed with
    // groups of two).
    let face1 = ElementFate {
        generated: vec![
            reported(ElementKind::Edge, 1, 2),
            reported(ElementKind::Edge, 2, 1),
            reported(ElementKind::Edge, 3, 3),
            reported(ElementKind::Edge, 4, 4),
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
        got[1..4],
        [
            "rrrrrrrr.cut.Edge(G1/3 aaaaaaaa.fuse.Face1, G3/3 aaaaaaaa.fuse.Face2)",
            "rrrrrrrr.cut.Edge(G2/3 aaaaaaaa.fuse.Face1, G2/3 aaaaaaaa.fuse.Face2)",
            "rrrrrrrr.cut.Edge(G3/3 aaaaaaaa.fuse.Face1, G1/3 aaaaaaaa.fuse.Face2)",
        ]
    );
}

#[test]
fn a_sketch_in_the_timeline_has_no_map_and_changes_no_other_features_names() {
    // A sketch makes no solid (M3.2; solved in M3.3), so it's named nothing, and the
    // features around it are named as without it.
    let sketch = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Sketch", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 10, "y": 0, "construction": false}], "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}], "circles": [], "arcs": [], "constraints": []},"#;
    let with = EXAMPLE.replacen(
        "    {\"id\": \"a91cw0de\"",
        &format!("    {sketch}\n    {{\"id\": \"a91cw0de\""),
        1,
    );
    assert_ne!(with, EXAMPLE);
    // Each on its own engine, so neither borrows the other's names (found by the review:
    // on one engine the second model took the first's map, so this compared a map with
    // itself).
    let model = |text: &str| {
        let engine = Recompute::new(FakeKernel::new(2).unwrap())
            .with_sketches(silico_core::fake::FakeSketches::new());
        published(&engine, checked(text))
    };
    let (plain, sketched) = (model(EXAMPLE), model(&with));
    assert!(sketched.errors().next().is_none());
    assert!(sketched.element_map(&id("s0000001")).is_none());
    let hole = |model: &silico_core::recompute::Model| {
        let map = model.element_map(&id("m3tq8v0z")).unwrap();
        map.iter()
            .map(|(e, names)| (e, names[0].to_string()))
            .collect::<Vec<_>>()
    };
    assert_eq!(hole(&sketched), hole(&plain));
    // And as pinned: the block's elements first, then the pin's.
    let map = sketched.element_map(&id("m3tq8v0z")).unwrap();
    assert_eq!(map.iter().count(), 26 + 8);
    for (kind, index, name) in [
        (ElementKind::Face, 1, "k7f2qx3m.box.Face1"),
        (
            ElementKind::Face,
            7,
            "m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face1)",
        ),
        (
            ElementKind::Edge,
            13,
            "m3tq8v0z.cut.Edge(I a91cw0de.cylinder.Edge1)",
        ),
        (
            ElementKind::Vertex,
            10,
            "m3tq8v0z.cut.Vertex(I a91cw0de.cylinder.Vertex2)",
        ),
    ] {
        assert_eq!(map.name(element(kind, index)).unwrap().to_string(), name);
    }
}

#[test]
fn edges_or_vertices_bounding_the_same_named_elements_are_ranked_by_freecads_count() {
    // Found by the tests review: no map gave an element from above a place. Face1 modified
    // from the cube's Face1; Face2 generated from its Edge1. Edge1 and Edge2 bound both
    // faces, Edge3 Face2 alone; Vertex1 and Vertex2 each end both Edge1 and Edge2. Named
    // from the same faces, Edge1 and Edge2 are ranked by their count in the first face's
    // edges, Face2's (it sorts first, `Edge1;:G;FLT`), where Edge2 comes first: 2/2 and
    // 1/2. Their vertices likewise, by the first edge's vertices. Each stable, so each a
    // reference of its own.
    let history = History {
        inputs: vec![InputHistory {
            faces: vec![ElementFate {
                modified: vec![reported(ElementKind::Face, 1, 1)],
                ..ElementFate::default()
            }],
            edges: vec![ElementFate {
                generated: vec![reported(ElementKind::Face, 2, 1)],
                ..ElementFate::default()
            }],
            vertices: vec![],
        }],
        result: ElementCounts {
            faces: 2,
            edges: 3,
            vertices: 2,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![vec![1, 2], vec![2, 1, 3]],
            outer_edges: vec![vec![1, 2], vec![1, 2, 3]],
            edge_vertices: vec![vec![1, 2], vec![1, 2], vec![]],
        },
    };
    history.validate(1).unwrap();
    let cube = ElementMap::primitive(
        id("aaaaaaaa"),
        &Primitive::Box {
            dx: 1.0,
            dy: 1.0,
            dz: 1.0,
        },
    );
    let map = ElementMap::operation(id("ffffffff"), OpCode::Fillet, &[&cube], &history);
    let (g, m) = (
        "ffffffff.fillet.Face(G aaaaaaaa.box.Edge1)",
        "aaaaaaaa.box.Face1",
    );
    assert_eq!(
        names(&map, ElementKind::Edge, 3),
        [
            format!("ffffffff.fillet.Edge(U2/2 {g}, {m})"),
            format!("ffffffff.fillet.Edge(U1/2 {g}, {m})"),
            format!("ffffffff.fillet.Edge(U {g})"),
        ]
    );
    let vertices = names(&map, ElementKind::Vertex, 2);
    for (vertex, rank) in vertices.iter().zip(["1/2", "2/2"]) {
        assert!(
            vertex.starts_with(&format!(
                "ffffffff.fillet.Vertex(U{rank} ffffffff.fillet.Edge(U"
            )),
            "{vertices:?}"
        );
        // From both edges.
        assert_eq!(
            vertex.matches("ffffffff.fillet.Edge(U").count(),
            2,
            "{vertex}"
        );
    }
    for kind in [ElementKind::Edge, ElementKind::Vertex] {
        for (element, names) in map.iter().filter(|(e, _)| e.kind == kind) {
            assert!(!map.is_unstable(&names[0]), "{element} {}", names[0]);
        }
    }
}

#[test]
fn a_vertex_ending_six_named_edges_is_named_by_the_first_five() {
    // Found by the tests review: the bound of five elements above was untested. Six edges,
    // each generated from one of the cube's faces, all ending Vertex1: its name lists five
    // of them, the first five in FreeCAD's order (Face1's to Face5's), and reads back.
    let history = History {
        inputs: vec![InputHistory {
            faces: (1..=6)
                .map(|e| ElementFate {
                    generated: vec![reported(ElementKind::Edge, e, 1)],
                    ..ElementFate::default()
                })
                .collect(),
            edges: vec![],
            vertices: vec![],
        }],
        result: ElementCounts {
            faces: 0,
            edges: 6,
            vertices: 1,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![],
            outer_edges: vec![],
            edge_vertices: vec![vec![1]; 6],
        },
    };
    history.validate(1).unwrap();
    let cube = ElementMap::primitive(
        id("aaaaaaaa"),
        &Primitive::Box {
            dx: 1.0,
            dy: 1.0,
            dz: 1.0,
        },
    );
    let map = ElementMap::operation(id("ffffffff"), OpCode::Fillet, &[&cube], &history);
    let edges: Vec<String> = (1..=5)
        .map(|f| format!("ffffffff.fillet.Edge(G aaaaaaaa.box.Face{f})"))
        .collect();
    let vertex = map.name(element(ElementKind::Vertex, 1)).unwrap();
    assert_eq!(
        vertex.to_string(),
        format!("ffffffff.fillet.Vertex(U {})", edges.join(", "))
    );
    let maker = |f: &silico_core::document::FeatureId| {
        Some(match f.as_str() {
            "aaaaaaaa" => {
                silico_core::naming::Maker::Itself(silico_core::naming::PrimitiveKind::Box)
            }
            _ => silico_core::naming::Maker::By(OpCode::Fillet),
        })
    };
    let read = Name::parse(&vertex.to_string(), &maker).unwrap();
    assert_eq!(map.element(&read), Some(element(ElementKind::Vertex, 1)));
}

#[test]
fn a_names_hash_is_its_identitys() {
    // Found by mutation testing: nothing held `Hash` to `Eq`, which leaves OCCT's raw
    // positions out. A name built from history and the same name read from its text (which
    // holds no positions) are equal and hash alike; names that differ hash apart.
    use std::hash::{BuildHasher, BuildHasherDefault};
    let hash = |name: &Name| {
        BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default().hash_one(name)
    };
    let history = History {
        inputs: vec![InputHistory {
            faces: vec![ElementFate {
                generated: vec![reported(ElementKind::Edge, 1, 7)],
                ..ElementFate::default()
            }],
            edges: vec![],
            vertices: vec![],
        }],
        result: ElementCounts {
            faces: 0,
            edges: 2,
            vertices: 0,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![],
            outer_edges: vec![],
            edge_vertices: vec![vec![], vec![]],
        },
    };
    history.validate(1).unwrap();
    let cube = ElementMap::primitive(
        id("aaaaaaaa"),
        &Primitive::Box {
            dx: 1.0,
            dy: 1.0,
            dz: 1.0,
        },
    );
    let map = ElementMap::operation(id("ffffffff"), OpCode::Fillet, &[&cube], &history);
    let built = map.name(element(ElementKind::Edge, 1)).unwrap();
    let maker = |f: &silico_core::document::FeatureId| {
        Some(match f.as_str() {
            "aaaaaaaa" => {
                silico_core::naming::Maker::Itself(silico_core::naming::PrimitiveKind::Box)
            }
            _ => silico_core::naming::Maker::By(OpCode::Fillet),
        })
    };
    let read = Name::parse(&built.to_string(), &maker).unwrap();
    assert_eq!(read, *built);
    assert_eq!(hash(&read), hash(&built));
    let other = map.name(element(ElementKind::Edge, 2)).unwrap();
    assert_ne!(*other, *built);
    assert_ne!(hash(&other), hash(&built));
}

#[test]
fn a_modified_names_hash_is_its_sources() {
    // Found by the review: a face modified one from one is its source's name, equal to it
    // through `identity`, so it hashes as its source does.
    use std::hash::{BuildHasher, BuildHasherDefault};
    let hash = |name: &Name| {
        BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default().hash_one(name)
    };
    let history = History {
        inputs: vec![InputHistory {
            faces: vec![ElementFate {
                modified: vec![reported(ElementKind::Face, 1, 1)],
                ..ElementFate::default()
            }],
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
    let cube = ElementMap::primitive(
        id("aaaaaaaa"),
        &Primitive::Box {
            dx: 1.0,
            dy: 1.0,
            dz: 1.0,
        },
    );
    let map = ElementMap::operation(id("ffffffff"), OpCode::Fillet, &[&cube], &history);
    let built = map.name(element(ElementKind::Face, 1)).unwrap();
    let source = cube.name(element(ElementKind::Face, 1)).unwrap();
    assert_eq!(built.to_string(), "aaaaaaaa.box.Face1");
    assert_eq!(*built, *source);
    assert_eq!(hash(&built), hash(&source));
}

#[test]
fn which_operations_keep_an_inputs_outside_is_a_fixed_table() {
    // N8, by the booleans' definitions: a cut keeps its base's outside and its tool's
    // inside; a common both insides; a fuse both outsides; a fillet or chamfer its input's
    // outside.
    for (op, base, tool) in [
        (OpCode::Cut, true, false),
        (OpCode::Common, false, false),
        (OpCode::Fuse, true, true),
    ] {
        assert_eq!((op.bites(0), op.bites(1)), (base, tool), "{op:?}");
    }
    for op in [OpCode::Fillet, OpCode::Chamfer] {
        assert!(op.bites(0), "{op:?}");
    }
    let words: Vec<&str> = OpCode::ALL.iter().map(|op| op.word()).collect();
    assert_eq!(words, ["fuse", "cut", "common", "fillet", "chamfer"]);
}

#[test]
fn an_element_modified_one_from_one_keeps_its_name_only_where_it_is_bitten() {
    // A two-input operation whose result's Face1 is input `from`'s Face1 modified, its sole
    // result: the base's (`pppppppp`) and the tool's (`tttttttt`) maps by number, as a
    // fuse's. Where the operation keeps that input's outside (bitten), the face is its
    // source's name; where it keeps its inside (a piece), the feature's own, `I` (N9),
    // written in FreeCAD's form alike (`Face1;:M;CUT`).
    let named = |op: OpCode, from: usize| {
        let modified = ElementFate {
            modified: vec![reported(ElementKind::Face, 1, 1)],
            ..ElementFate::default()
        };
        let fates = |i: usize| InputHistory {
            faces: if i == from {
                vec![modified.clone()]
            } else {
                vec![]
            },
            edges: vec![],
            vertices: vec![],
        };
        let history = History {
            inputs: vec![fates(0), fates(1)],
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
        history.validate(2).unwrap();
        let base = ElementMap::by_number(id("pppppppp"), OpCode::Fuse, BOX);
        let tool = ElementMap::by_number(id("tttttttt"), OpCode::Fuse, BOX);
        let map = ElementMap::operation(id("rrrrrrrr"), op, &[&base, &tool], &history);
        let face = map.name(element(ElementKind::Face, 1)).unwrap();
        (face.to_string(), freecad::text(&face))
    };
    let bare = |input: &str| format!("{input}.fuse.Face1");
    let piece = |op: &str, input: &str| format!("rrrrrrrr.{op}.Face(I {input}.fuse.Face1)");
    for (op, from, name) in [
        (OpCode::Cut, 0, bare("pppppppp")),
        (OpCode::Cut, 1, piece("cut", "tttttttt")),
        (OpCode::Common, 0, piece("common", "pppppppp")),
        (OpCode::Common, 1, piece("common", "tttttttt")),
        (OpCode::Fuse, 0, bare("pppppppp")),
        (OpCode::Fuse, 1, bare("tttttttt")),
    ] {
        let (written, freecad) = named(op, from);
        assert_eq!(written, name, "{op:?} of input {from}");
        assert_eq!(
            freecad,
            format!("Face1;:M;{}", op.code()),
            "{op:?} of input {from}"
        );
    }
}

#[test]
fn an_element_kept_whole_keeps_its_name_only_where_it_is_bitten() {
    // N9: step 1 asks `bites` as step 2 does. A two-input operation whose result's Face1 is
    // input `from`'s Face1 kept whole: where the operation keeps that input's outside, the
    // face keeps its name (N2); where it keeps its inside, it's the feature's own, `I`, with
    // no OCCT position, so FreeCAD's form writes it as the source, as FreeCAD's
    // `mapSubElement` does. Read back, the `I` name is the name built.
    let named = |op: OpCode, from: usize| {
        let fates = |i: usize| InputHistory {
            faces: if i == from {
                vec![ElementFate {
                    kept: Some(1),
                    ..ElementFate::default()
                }]
            } else {
                vec![]
            },
            edges: vec![],
            vertices: vec![],
        };
        let history = History {
            inputs: vec![fates(0), fates(1)],
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
        history.validate(2).unwrap();
        let base = ElementMap::by_number(id("pppppppp"), OpCode::Fuse, BOX);
        let tool = ElementMap::by_number(id("tttttttt"), OpCode::Fuse, BOX);
        let map = ElementMap::operation(id("rrrrrrrr"), op, &[&base, &tool], &history);
        map.name(element(ElementKind::Face, 1)).unwrap()
    };
    let bare = |input: &str| format!("{input}.fuse.Face1");
    let inside = |op: &str, input: &str| format!("rrrrrrrr.{op}.Face(I {input}.fuse.Face1)");
    for (op, from, name) in [
        (OpCode::Cut, 0, bare("pppppppp")),
        (OpCode::Cut, 1, inside("cut", "tttttttt")),
        (OpCode::Common, 0, inside("common", "pppppppp")),
        (OpCode::Common, 1, inside("common", "tttttttt")),
        (OpCode::Fuse, 0, bare("pppppppp")),
        (OpCode::Fuse, 1, bare("tttttttt")),
    ] {
        let face = named(op, from);
        assert_eq!(face.to_string(), name, "{op:?} of input {from}");
        assert_eq!(freecad::text(&face), "Face1", "{op:?} of input {from}");
        let any = |_: &silico_core::document::FeatureId| Some(silico_core::naming::Maker::By(op));
        assert_eq!(Name::parse(&name, &any).unwrap(), *face, "{name}");
    }
}

#[test]
fn a_source_the_operation_keeps_inside_is_written_i_beside_one_it_generated_from() {
    // N9: an edge modified from the tool's Edge1 and generated from the base's Face1 (an edge
    // lying in a face). Under a cut, the tool's source is what lies inside the base, `I`, the
    // base's `G`; under a fuse it bites, `M`. Sources by kind, edges before faces; FreeCAD's
    // form writes `I` as `M`, both relations, `MG`.
    let named = |op: OpCode| {
        let base = InputHistory {
            faces: vec![ElementFate {
                generated: vec![reported(ElementKind::Edge, 1, 1)],
                ..ElementFate::default()
            }],
            edges: vec![],
            vertices: vec![],
        };
        let tool = InputHistory {
            faces: vec![],
            edges: vec![ElementFate {
                modified: vec![reported(ElementKind::Edge, 1, 1)],
                ..ElementFate::default()
            }],
            vertices: vec![],
        };
        let history = History {
            inputs: vec![base, tool],
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
        let base = ElementMap::by_number(id("pppppppp"), OpCode::Fuse, BOX);
        let tool = ElementMap::by_number(id("tttttttt"), OpCode::Fuse, BOX);
        let map = ElementMap::operation(id("rrrrrrrr"), op, &[&base, &tool], &history);
        map.name(element(ElementKind::Edge, 1)).unwrap()
    };
    let cut = named(OpCode::Cut);
    assert_eq!(
        cut.to_string(),
        "rrrrrrrr.cut.Edge(I tttttttt.fuse.Edge1, G pppppppp.fuse.Face1)"
    );
    assert_eq!(freecad::text(&cut), "Edge1;:MG(Face1;K-1);CUT");
    assert_eq!(
        named(OpCode::Fuse).to_string(),
        "rrrrrrrr.fuse.Edge(M tttttttt.fuse.Edge1, G pppppppp.fuse.Face1)"
    );
}

#[test]
fn an_edge_modified_from_both_operands_says_m_for_a_bitten_one_and_i_for_the_other() {
    // Found by the milestone's third round: coincident edges of the two operands made one,
    // modified from each. Each source's letter is its own input's side: a cut bites its base
    // (`M`) and not its tool (`I`); a fuse both; a common neither. FreeCAD's form writes `I`
    // as `M`, so both sources are modified, `;:M`, never `;:MG`.
    let named = |op: OpCode| {
        let fate = || ElementFate {
            modified: vec![reported(ElementKind::Edge, 1, 1)],
            ..ElementFate::default()
        };
        let input = || InputHistory {
            faces: vec![],
            edges: vec![fate()],
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
        let base = ElementMap::by_number(id("pppppppp"), OpCode::Fuse, BOX);
        let tool = ElementMap::by_number(id("tttttttt"), OpCode::Fuse, BOX);
        let map = ElementMap::operation(id("rrrrrrrr"), op, &[&base, &tool], &history);
        map.name(element(ElementKind::Edge, 1)).unwrap()
    };
    for (op, base, tool) in [
        (OpCode::Cut, "M", "I"),
        (OpCode::Fuse, "M", "M"),
        (OpCode::Common, "I", "I"),
    ] {
        let name = named(op);
        assert_eq!(
            name.to_string(),
            format!(
                "rrrrrrrr.{}.Edge({base} pppppppp.fuse.Edge1, {tool} tttttttt.fuse.Edge1)",
                op.word()
            )
        );
        assert_eq!(
            freecad::text(&name),
            format!("Edge1;:M(Edge1);{}", op.code()),
            "{op:?}"
        );
    }
}
