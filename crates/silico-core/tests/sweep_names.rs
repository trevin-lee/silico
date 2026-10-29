//! M3.6, step v on the fake kernel (docs/design/m3.6-sketch-features.md, S4): every element
//! of a pad's, pocket's, and revolve's solid named from its sketch's elements in one level,
//! each row of the design's tables written out by hand for the case, and a sketch's map of
//! the faces its profile makes. The fake sweeps as the kernel reports (each element's start
//! modified, its end and sides generated), and its boolean keeps both inputs' elements, so
//! the names here are the sweep's, as a real sweep's are where the boolean keeps them
//! (tests/sketch_feature_names.rs in silico-kernel pins the real kernel's).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::model::WAIT;
use super::sweeps;

use std::collections::BTreeSet;
use std::sync::Arc;

use silico_core::document::{ElementId, read};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{
    ElementCounts, ElementFate, ElementKind, ElementRef, History, InputHistory, Topology,
};
use silico_core::naming::{ElementMap, Maker, Name};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use sweeps::*;

fn published(features: &[&str]) -> Arc<Model> {
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    match engine.start(checked_document(features)).wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        other => panic!("not published: {:?}", other.ok()),
    }
}

/// Every element's own name of `feature`'s solid.
fn names(model: &Model, feature: &str) -> BTreeSet<String> {
    assert!(
        matches!(
            model.outcome(&id(feature)),
            Some(FeatureOutcome::Built { .. })
        ),
        "{feature}: {:?}",
        model.outcome(&id(feature))
    );
    let map = model.element_map(&id(feature)).unwrap();
    map.iter()
        .map(|(_, names)| {
            let name = names.first().unwrap();
            assert!(!map.is_unstable(name), "{name}");
            name.to_string()
        })
        .collect()
}

/// The names a sweep by `feature` of type `word` gives the rectangle sketch's elements
/// (sides na000001 to na000004, corners pa000001 to pa000004), the design's S4 table: each
/// side's face G, its start edge M and its end edge G; each corner's side edge G, its start
/// vertex M and its end vertex G; the region's start face M and end face G.
fn rectangle_names(feature: &str, word: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for n in 1..=4 {
        let edge = format!("ssssssss.sketch.Edge.na00000{n}");
        let vertex = format!("ssssssss.sketch.Vertex.pa00000{n}");
        names.insert(format!("{feature}.{word}.Face(G {edge})"));
        names.insert(format!("{feature}.{word}.Edge(M {edge})"));
        names.insert(format!("{feature}.{word}.Edge(G {edge})"));
        names.insert(format!("{feature}.{word}.Edge(G {vertex})"));
        names.insert(format!("{feature}.{word}.Vertex(M {vertex})"));
        names.insert(format!("{feature}.{word}.Vertex(G {vertex})"));
    }
    names.insert(format!("{feature}.{word}.Face(M ssssssss.sketch.Face1)"));
    names.insert(format!("{feature}.{word}.Face(G ssssssss.sketch.Face1)"));
    names
}

#[test]
fn a_pads_elements_are_its_own_named_from_its_sketchs_in_one_level() {
    // testPartDesignElementMapPad's essence: a square padded, 26 names, one per element,
    // each the pad's own, none a sketch's (design S4).
    let model = published(&[
        &rectangle("ssssssss", "xy", [0.0, 0.0], 1.0, 1.0),
        &pad("pppppppp", None, "ssssssss", TEN),
    ]);
    let got = names(&model, "pppppppp");
    assert_eq!(got.len(), 26);
    assert_eq!(got, rectangle_names("pppppppp", "pad"));
}

#[test]
fn a_pad_and_a_pocket_onto_a_base_keep_the_bases_names_and_their_own() {
    // The fake's boolean keeps both inputs' elements: the base's names bare, the sweep's as
    // the sweep named them, which a sweep feature's fuse or cut bites (design S4): no `I`,
    // no level added.
    let base = box_at("bbbbbbbb", [-1.0, -1.0, -10.0], 10.0, 10.0, 10.0);
    let sketch = rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0);
    for (feature, word) in [
        (pad("pppppppp", Some("bbbbbbbb"), "ssssssss", TEN), "pad"),
        (pocket("pppppppp", "bbbbbbbb", "ssssssss", TEN), "pocket"),
    ] {
        let model = published(&[&base, &sketch, &feature]);
        let got = names(&model, "pppppppp");
        let mut want = rectangle_names("pppppppp", word);
        for (kind, count) in [("Face", 6), ("Edge", 12), ("Vertex", 8)] {
            for n in 1..=count {
                want.insert(format!("bbbbbbbb.box.{kind}{n}"));
            }
        }
        assert_eq!(got, want, "{word}");
        assert!(!got.iter().any(|n| n.contains("(I ")), "{got:#?}");
    }
}

#[test]
fn a_revolves_elements_are_named_alike_and_a_whole_turns_have_no_ends() {
    // A turn short of a whole one, as a prism; a whole turn has no ends: each side's face
    // G from its curve, each curve's start, the seam, M, each corner's circle G and its
    // vertex M (docs/ports/history.md's whole turn, as the fake reports it).
    let sketch = rectangle("ssssssss", "xz", [1.0, 0.0], 4.0, 2.0);
    let model = published(&[
        &sketch,
        &turned(
            "rrrrrrrr",
            "revolve",
            None,
            "ssssssss",
            "y_axis",
            &angle("90"),
        ),
    ]);
    assert_eq!(
        names(&model, "rrrrrrrr"),
        rectangle_names("rrrrrrrr", "revolve")
    );
    let model = published(&[
        &sketch,
        &turned("rrrrrrrr", "revolve", None, "ssssssss", "y_axis", TURN),
    ]);
    let mut want = BTreeSet::new();
    for n in 1..=4 {
        let edge = format!("ssssssss.sketch.Edge.na00000{n}");
        let vertex = format!("ssssssss.sketch.Vertex.pa00000{n}");
        want.insert(format!("rrrrrrrr.revolve.Face(G {edge})"));
        want.insert(format!("rrrrrrrr.revolve.Edge(M {edge})"));
        want.insert(format!("rrrrrrrr.revolve.Edge(G {vertex})"));
        want.insert(format!("rrrrrrrr.revolve.Vertex(M {vertex})"));
    }
    assert_eq!(names(&model, "rrrrrrrr"), want);
}

#[test]
fn a_pocket_of_two_circles_names_each_region_by_its_curve_and_each_circles_vertex_from_above() {
    // Design S4: several regions, each by its outer loop's smallest curve, from below; a
    // circle's one vertex, which no point is, from the edge it bounds.
    let base = box_at("bbbbbbbb", [-1.0, -1.0, -10.0], 10.0, 10.0, 10.0);
    let two = circles("ssssssss", "xy", &[([2.0, 2.0], 1.0), ([6.0, 2.0], 1.0)]);
    let model = published(&[
        &base,
        &two,
        &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN),
    ]);
    let got: BTreeSet<String> = names(&model, "qqqqqqqq")
        .into_iter()
        .filter(|n| n.starts_with("qqqqqqqq."))
        .collect();
    let mut want = BTreeSet::new();
    for n in 1..=2 {
        let edge = format!("ssssssss.sketch.Edge.ca00000{n}");
        let vertex = format!("ssssssss.sketch.Vertex(U {edge})");
        let region = format!("ssssssss.sketch.Face(L {edge})");
        want.insert(format!("qqqqqqqq.pocket.Face(G {edge})"));
        want.insert(format!("qqqqqqqq.pocket.Edge(M {edge})"));
        want.insert(format!("qqqqqqqq.pocket.Edge(G {edge})"));
        want.insert(format!("qqqqqqqq.pocket.Edge(G {vertex})"));
        want.insert(format!("qqqqqqqq.pocket.Vertex(M {vertex})"));
        want.insert(format!("qqqqqqqq.pocket.Vertex(G {vertex})"));
        want.insert(format!("qqqqqqqq.pocket.Face(M {region})"));
        want.insert(format!("qqqqqqqq.pocket.Face(G {region})"));
    }
    assert_eq!(got, want);
}

#[test]
fn a_pads_names_dont_change_when_its_sketchs_points_are_dragged_or_its_lists_reordered() {
    // Names come from IDs, which never move (design S4): the rectangle dragged to another
    // size, and its points and lines listed in another order, name alike.
    let pad = pad("pppppppp", None, "ssssssss", TEN);
    let first = names(
        &published(&[&rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0), &pad]),
        "pppppppp",
    );
    let dragged = names(
        &published(&[&rectangle("ssssssss", "xy", [-3.0, 1.0], 7.5, 0.25), &pad]),
        "pppppppp",
    );
    assert_eq!(dragged, first);
    let sketch = rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0);
    let document = read(document(&[&sketch, &pad]).as_bytes()).unwrap();
    let mut reordered = document.clone();
    if let silico_core::document::Operation::Sketch(s) = &mut reordered.features[0].operation {
        s.points.reverse();
        s.lines.reverse();
    }
    assert_ne!(reordered, document);
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    let Ok(RunOutcome::Published(model)) = engine
        .start(silico_core::document::Checked::new(reordered).unwrap())
        .wait()
    else {
        panic!("not published");
    };
    assert_eq!(names(&model, "pppppppp"), first);
}

#[test]
fn every_name_a_sweep_feature_gives_reads_back_as_itself_and_finds_its_element() {
    // N8 with M3.6's words: each name, written, reads back (the type words from the text)
    // as the name that finds the same element.
    let base = box_at("bbbbbbbb", [-1.0, -1.0, -10.0], 10.0, 10.0, 10.0);
    let two = circles("ssssssss", "xy", &[([2.0, 2.0], 1.0), ([6.0, 2.0], 1.0)]);
    let model = published(&[
        &base,
        &two,
        &pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN),
    ]);
    let makers = |feature: &silico_core::document::FeatureId| {
        model
            .document()
            .features
            .iter()
            .find(|f| &f.id == feature)
            .map(|f| Maker::of(&f.operation))
    };
    let map = model.element_map(&id("qqqqqqqq")).unwrap();
    for (element, names) in map.iter() {
        for name in names {
            let text = name.to_string();
            let read = Name::parse(&text, &makers).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(map.element(&read), Some(element), "{text}");
        }
    }
}

/// A faces step's history, as the kernel reports one (silico-protocol's `FaceRegions`):
/// each curve kept as an edge, each point as a vertex, in the order sent.
fn faces_history(
    curves: u32,
    points: u32,
    extra_vertices: u32,
    regions: Vec<Vec<u32>>,
    edge_vertices: Vec<Vec<u32>>,
) -> History {
    let kept = |n: u32| ElementFate {
        kept: Some(n),
        ..ElementFate::default()
    };
    History {
        inputs: vec![InputHistory {
            faces: Vec::new(),
            edges: (1..=curves).map(kept).collect(),
            vertices: (1..=points).map(kept).collect(),
        }],
        result: ElementCounts {
            faces: regions.len() as u32,
            edges: curves,
            vertices: points + extra_vertices,
            solids: 0,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: regions.clone(),
            outer_edges: regions,
            edge_vertices,
        },
    }
}

#[test]
fn a_sketchs_map_names_its_curves_points_circles_vertices_and_regions_by_ids() {
    // Design S4's first table, row by row, on histories written by hand: a square (four
    // curves, four points, one region: Face1) and two circles (two curves, no points, two
    // vertices of their own, two regions, each by its curve, from below).
    let ids = |list: &[&str]| -> Vec<ElementId> {
        list.iter().map(|i| ElementId::new(i).unwrap()).collect()
    };
    let square = faces_history(
        4,
        4,
        0,
        vec![vec![1, 2, 3, 4]],
        vec![vec![1, 2], vec![2, 3], vec![3, 4], vec![4, 1]],
    );
    let map = ElementMap::sketch(
        id("ssssssss"),
        &ids(&["na000001", "na000002", "na000003", "na000004"]),
        &ids(&["pa000001", "pa000002", "pa000003", "pa000004"]),
        &ids(&["na000001"]),
        &square,
    );
    let name = |kind, index| map.name(ElementRef { kind, index }).unwrap().to_string();
    assert_eq!(name(ElementKind::Face, 1), "ssssssss.sketch.Face1");
    assert_eq!(name(ElementKind::Edge, 3), "ssssssss.sketch.Edge.na000003");
    assert_eq!(
        name(ElementKind::Vertex, 2),
        "ssssssss.sketch.Vertex.pa000002"
    );
    assert_eq!(map.iter().count(), 9);
    assert_eq!(map.unstable().count(), 0);
    let circles = faces_history(2, 0, 2, vec![vec![1], vec![2]], vec![vec![1], vec![2]]);
    let map = ElementMap::sketch(
        id("ssssssss"),
        &ids(&["ca000001", "ca000002"]),
        &[],
        &ids(&["ca000001", "ca000002"]),
        &circles,
    );
    let name = |kind, index| map.name(ElementRef { kind, index }).unwrap().to_string();
    assert_eq!(
        name(ElementKind::Face, 2),
        "ssssssss.sketch.Face(L ssssssss.sketch.Edge.ca000002)"
    );
    assert_eq!(
        name(ElementKind::Vertex, 1),
        "ssssssss.sketch.Vertex(U ssssssss.sketch.Edge.ca000001)"
    );
    assert_eq!(name(ElementKind::Edge, 2), "ssssssss.sketch.Edge.ca000002");
    assert_eq!(map.unstable().count(), 0);
}
