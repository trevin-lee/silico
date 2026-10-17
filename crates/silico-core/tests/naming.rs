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
use silico_core::naming::{ElementMap, Name, Operation, freecad};
use silico_core::recompute::Recompute;

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
    let input = ElementMap::primitive(id("aaaaaaaa"));
    let map = ElementMap::operation(id("rrrrrrrr"), Operation::Fuse, &[&input], &history);
    assert_eq!(
        names(&map, ElementKind::Edge, 2),
        [
            "rrrrrrrr.Edge(L aaaaaaaa.Vertex1, aaaaaaaa.Vertex2)",
            "rrrrrrrr.Edge(L aaaaaaaa.Vertex1, aaaaaaaa.Vertex2)~1",
        ]
    );
    let first = map.name(element(ElementKind::Edge, 1)).unwrap();
    assert!(map.is_duplicated(&first));
    assert!(!map.is_duplicated(&map.name(element(ElementKind::Vertex, 1)).unwrap()));
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
    let input = ElementMap::primitive(id("aaaaaaaa"));
    let map = ElementMap::operation(id("ffffffff"), Operation::Fillet, &[&input], &history);
    assert_eq!(
        names(&map, ElementKind::Face, 2),
        [
            "ffffffff.Face(M aaaaaaaa.Face1)",
            "ffffffff.Face(G aaaaaaaa.Edge1)",
        ]
    );
    let generated = "ffffffff.Face(G aaaaaaaa.Edge1)";
    let modified = "ffffffff.Face(M aaaaaaaa.Face1)";
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
    let input = ElementMap::primitive(id("aaaaaaaa"));
    let map = ElementMap::operation(id("rrrrrrrr"), Operation::Cut, &[&input], &history);
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
fn a_kept_name_is_its_inputs_and_a_primitive_names_any_element_of_its_own() {
    let map = ElementMap::primitive(id("aaaaaaaa"));
    let name = Arc::new(Name::Element {
        feature: id("aaaaaaaa"),
        element: element(ElementKind::Vertex, 99),
    });
    assert_eq!(map.element(&name), Some(element(ElementKind::Vertex, 99)));
    assert_eq!(map.names(element(ElementKind::Vertex, 99)), vec![name]);
    assert_eq!(map.feature(), &id("aaaaaaaa"));
    assert_eq!(map.iter().count(), 0);
}
