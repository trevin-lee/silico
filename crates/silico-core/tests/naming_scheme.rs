//! The naming scheme's version (`naming::NAMING_SCHEME`), held to the names it gives: a
//! set of documents' every element name, on the fake kernel, and every name the map's
//! builder gives from histories that use each of its steps (the real kernel's, recorded in
//! `fixtures/histories/`, and some written by hand), digested and pinned with the version.
//! The fake keeps or moves every element, so its names alone are a primitive's: the review
//! found the builder's later steps free to change unnoticed. A change to how elements are
//! named (the grammar, the map's builder, a primitive's numbers) changes the digest and
//! fails this until the version is bumped too, since a fillet's result kept on disk is
//! keyed by it (a result made from the edges an older scheme's names found must not be
//! taken for today's). Bump NAMING_SCHEME (and NAMING_SCHEME_NUMBER, which documents
//! record, with it), then pin the new digest here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sha2::{Digest, Sha256};
use silico_core::document::{Checked, FeatureId, read};
use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    ElementCounts, ElementFate, ElementKind, ElementRef, History, InputHistory, Primitive,
    Reported, Topology,
};
use silico_core::naming::{ElementMap, NAMING_SCHEME, OpCode};
use silico_core::recompute::{Recompute, RunOutcome};

const DOCUMENTS: &[&str] = &[
    // The design's example: a block, a pin, a hole.
    r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [
{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10},
{"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 12},
{"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de"},
{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "m3tq8v0z", "edges": ["m3tq8v0z.cut.Edge(I a91cw0de.cylinder.Edge2)"], "radius": 0.5}]}"#,
    // Every primitive, a cone pointed at either end among them, fused in turn, and the
    // fuse in common with a sphere at its box's corner (so the common takes some of it).
    r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [
{"id": "b0000001", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 1, "width": 2, "height": 3},
{"id": "c0000001", "type": "cone", "v": 1, "name": "C", "position": [5, 0, 0], "radius1": 2, "radius2": 0, "height": 3},
{"id": "f0000001", "type": "fuse", "v": 1, "name": "F", "base": "b0000001", "tool": "c0000001"},
{"id": "c0000002", "type": "cone", "v": 1, "name": "C2", "position": [9, 0, 0], "radius1": 0, "radius2": 2, "height": 3},
{"id": "f0000002", "type": "fuse", "v": 1, "name": "F2", "base": "f0000001", "tool": "c0000002"},
{"id": "s0000001", "type": "sphere", "v": 1, "name": "S", "position": [0, 0, 0], "radius": 1},
{"id": "f0000003", "type": "common", "v": 1, "name": "F3", "base": "f0000002", "tool": "s0000001"},
{"id": "t0000001", "type": "torus", "v": 1, "name": "T", "position": [20, 0, 0], "radius1": 3, "radius2": 1},
{"id": "f0000004", "type": "fuse", "v": 1, "name": "F4", "base": "f0000003", "tool": "t0000001"},
{"id": "c0000003", "type": "chamfer", "v": 1, "name": "Ch", "base": "f0000004", "edges": ["f0000003.common.Edge(I b0000001.box.Edge1)"], "size": {"distance": 0.1}}]}"#,
    // M3.6's features: a square with a round hole padded, two circles pocketed up into it,
    // a ring revolved onto it whole, grooved a quarter turn, and a fillet of the pad's top
    // edge by name.
    r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": [
{"id": "s0000001", "type": "sketch", "v": 1, "name": "Plate", "plane": "xy", "points": [{"id": "pa000001", "x": 0, "y": 0, "construction": false}, {"id": "pa000002", "x": 10, "y": 0, "construction": false}, {"id": "pa000003", "x": 10, "y": 10, "construction": false}, {"id": "pa000004", "x": 0, "y": 10, "construction": false}, {"id": "pc000001", "x": 5, "y": 5, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}], "circles": [{"id": "ca000001", "center": "pc000001", "radius": 2, "construction": false}], "arcs": [], "constraints": []},
{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}},
{"id": "s0000002", "type": "sketch", "v": 1, "name": "Holes", "plane": "xy", "points": [{"id": "pc000001", "x": 2, "y": 2, "construction": false}, {"id": "pc000002", "x": 8, "y": 8, "construction": false}], "lines": [], "circles": [{"id": "ca000001", "center": "pc000001", "radius": 1, "construction": false}, {"id": "ca000002", "center": "pc000002", "radius": 1, "construction": false}], "arcs": [], "constraints": []},
{"id": "q0000001", "type": "pocket", "v": 1, "name": "Pocket", "base": "p0000001", "sketch": "s0000002", "extent": {"type": "one_side", "length": 2, "reversed": true}},
{"id": "s0000003", "type": "sketch", "v": 1, "name": "Ring", "plane": "xz", "points": [{"id": "pa000001", "x": 12, "y": 0, "construction": false}, {"id": "pa000002", "x": 14, "y": 0, "construction": false}, {"id": "pa000003", "x": 14, "y": 2, "construction": false}, {"id": "pa000004", "x": 12, "y": 2, "construction": false}], "lines": [{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}, {"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}, {"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}, {"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}], "circles": [], "arcs": [], "constraints": []},
{"id": "r0000001", "type": "revolve", "v": 1, "name": "Ring", "base": "q0000001", "sketch": "s0000003", "axis": "y_axis", "extent": {"type": "one_side", "angle": 360, "reversed": false}},
{"id": "g0000001", "type": "groove", "v": 1, "name": "Groove", "base": "r0000001", "sketch": "s0000003", "axis": "y_axis", "extent": {"type": "symmetric", "angle": 90}},
{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "g0000001", "edges": ["p0000001.pad.Edge(G s0000001.sketch.Edge.na000003)"], "radius": 0.5}]}"#,
];

/// Every element name of every feature of every document, in timeline and element order.
fn names() -> String {
    let engine = Recompute::new(FakeKernel::new(2).unwrap())
        .with_sketches(silico_core::fake::FakeSketches::new());
    let mut out = String::new();
    for text in DOCUMENTS {
        let document = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
        let Ok(RunOutcome::Published(model)) = engine.start(document).wait() else {
            panic!("not published");
        };
        assert!(
            model.errors().next().is_none(),
            "{:?}",
            model.errors().collect::<Vec<_>>()
        );
        for (feature, outcome) in model.features() {
            // A sketch makes no solid: its elements are named in what sweeps it.
            if matches!(outcome, silico_core::recompute::FeatureOutcome::Sketch(_)) {
                continue;
            }
            let map = model.element_map(feature).unwrap();
            for (element, names) in map.iter() {
                for name in names {
                    out.push_str(&format!("{feature} {element:?} {name}\n"));
                }
            }
        }
    }
    out
}

/// The real kernel's histories, recorded from OCCT 7.9.3 through a kernel worker by
/// silico-kernel's `tests/history_fixtures.rs`, which holds them to what the kernel reports
/// (each file: the operation, its inputs' IDs, primitives, and where each is moved to, and
/// the history): a chamfer of a 10 mm cube's Edge7 and Edge8 (the elements above several
/// named faces), a fillet of its Edge1, a ball (r 10) with a pin (r 2) through it (ranked
/// rims and seam points), a plate (40 × 20 × 5) with a slot across it (ranked pieces), and
/// two unit cubes fused, the second moved by (0.5, 0.5, 0.5) (ranked split edges).
const RECORDED: &[&str] = &[
    "chamfer-edge7-edge8",
    "fillet-edge1",
    "ball-through-hole",
    "plate-slot",
    "two-cubes-fuse",
];

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn written(map: &ElementMap, out: &mut String) {
    for (element, names) in map.iter() {
        for name in names {
            out.push_str(&format!("{} {element:?} {name}\n", map.feature()));
        }
    }
}

fn recorded(case: &str) -> (OpCode, Vec<ElementMap>, History) {
    let path = format!(
        "{}/tests/fixtures/histories/{case}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let op = match json["op"].as_str().unwrap() {
        "Fuse" => OpCode::Fuse,
        "Cut" => OpCode::Cut,
        "Common" => OpCode::Common,
        "Fillet" => OpCode::Fillet,
        "Chamfer" => OpCode::Chamfer,
        other => panic!("{other}"),
    };
    let inputs = json["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|input| {
            let primitive: Primitive = serde_json::from_value(input["primitive"].clone()).unwrap();
            ElementMap::primitive(id(input["id"].as_str().unwrap()), &primitive)
        })
        .collect();
    let history: History = serde_json::from_value(json["history"].clone()).unwrap();
    history
        .validate(json["inputs"].as_array().unwrap().len())
        .unwrap();
    (op, inputs, history)
}

fn reported(kind: ElementKind, index: u32, position: u32) -> Reported {
    Reported {
        element: ElementRef { kind, index },
        position,
    }
}

/// Histories written by hand for the builder's steps no recorded one takes: elements
/// above with places (two edges bounding the same faces), five above of six (a vertex
/// ending six named edges), elements below, a duplicate (`~1`), and an element nothing
/// names (by its number). The first two are tests/naming.rs's, whose tests say what each
/// names: a copy, since this digest must hold them whatever those tests become.
fn by_hand() -> Vec<History> {
    let counts = |faces, edges, vertices| ElementCounts {
        faces,
        edges,
        vertices,
        solids: 1,
    };
    let modified = |kind, index| ElementFate {
        modified: vec![reported(kind, index, 1)],
        ..ElementFate::default()
    };
    let generated = |list: Vec<Reported>| ElementFate {
        generated: list,
        ..ElementFate::default()
    };
    vec![
        // Face1 modified from Face1, Face2 generated from Edge1; Edge1 and Edge2 bound both,
        // Edge3 Face2 alone.
        History {
            inputs: vec![InputHistory {
                faces: vec![modified(ElementKind::Face, 1)],
                edges: vec![generated(vec![reported(ElementKind::Face, 2, 1)])],
                vertices: vec![],
            }],
            result: counts(2, 3, 0),
            ends: Default::default(),
            topology: Topology {
                face_edges: vec![vec![1, 2], vec![2, 1, 3]],
                outer_edges: vec![vec![1, 2], vec![1, 2, 3]],
                edge_vertices: vec![vec![], vec![], vec![]],
                solid_faces: vec![vec![1, 2]],
            },
        },
        // Six edges, each generated from one of the input's faces, all ending Vertex1.
        History {
            inputs: vec![InputHistory {
                faces: (1..=6)
                    .map(|e| generated(vec![reported(ElementKind::Edge, e, 1)]))
                    .collect(),
                edges: vec![],
                vertices: vec![],
            }],
            result: counts(0, 6, 1),
            ends: Default::default(),
            topology: Topology {
                face_edges: vec![],
                outer_edges: vec![],
                edge_vertices: vec![vec![1]; 6],
                solid_faces: vec![vec![]],
            },
        },
        // Two edges joining the same two kept vertices: named from below alike, `~1`; and
        // a face nothing names, by its number.
        History {
            inputs: vec![InputHistory {
                faces: vec![],
                edges: vec![],
                vertices: vec![
                    ElementFate {
                        kept: Some(1),
                        ..ElementFate::default()
                    },
                    ElementFate {
                        kept: Some(2),
                        ..ElementFate::default()
                    },
                ],
            }],
            result: counts(1, 2, 2),
            ends: Default::default(),
            topology: Topology {
                face_edges: vec![vec![]],
                outer_edges: vec![vec![]],
                edge_vertices: vec![vec![1, 2], vec![2, 1]],
                solid_faces: vec![vec![1]],
            },
        },
    ]
}

/// Every name the builder gives from the recorded and the hand-written histories.
fn built() -> String {
    let mut out = String::new();
    for case in RECORDED {
        let (op, inputs, history) = recorded(case);
        let inputs: Vec<&ElementMap> = inputs.iter().collect();
        written(
            &ElementMap::operation(id("rrrrrrrr"), op, &inputs, &history),
            &mut out,
        );
    }
    let cube = ElementMap::primitive(
        id("aaaaaaaa"),
        &Primitive::Box {
            dx: 1.0,
            dy: 1.0,
            dz: 1.0,
        },
    );
    for history in by_hand() {
        history.validate(1).unwrap();
        written(
            &ElementMap::operation(id("ffffffff"), OpCode::Fillet, &[&cube], &history),
            &mut out,
        );
    }
    out
}

#[test]
fn the_histories_take_every_step_of_the_builder() {
    // So the digest holds each step's names: places among derived and upper names, upper
    // and lower names, five above of six, a duplicate, an element by its number, and a
    // piece.
    let names = built();
    for (what, needle) in [
        ("a derived name's place", "(G1/2 "),
        ("a split's place", "(M1/2 "),
        ("an element above", "Edge(U "),
        ("an element above with a place", "Edge(U1/2 "),
        (
            "several above",
            "Edge(U rrrrrrrr.chamfer.Face(G aaaaaaaa.box.Edge7), ",
        ),
        ("elements below", "(L "),
        ("a duplicate", ")~1"),
        ("an element by its number", "ffffffff.fillet.Face1\n"),
        // N9: what of a cut's tool it keeps is a piece, the cut's own, `I`.
        (
            "a piece of a cut's tool",
            "rrrrrrrr.cut.Face(I pppppppp.cylinder.Face1)\n",
        ),
    ] {
        assert!(names.contains(needle), "no {what} ({needle}) in:\n{names}");
    }
    // Five above of the six edges the vertex ends.
    let five = names
        .lines()
        .find(|line| line.contains("ffffffff.fillet.Vertex(U "))
        .unwrap();
    assert_eq!(five.matches("aaaaaaaa.box.Face").count(), 5, "{five}");
}

#[test]
fn the_documents_take_every_form_of_a_sweep_features_names() {
    // So the digest holds M3.6's forms (design S4): a sketch's curve, point, circle's vertex
    // (from above), only region, and one of several (from below), each as a source of a
    // sweep feature's element, M uncollapsed and G.
    let names = names();
    for needle in [
        "p0000001.pad.Face(G s0000001.sketch.Edge.na000001)",
        "p0000001.pad.Edge(M s0000001.sketch.Edge.ca000001)",
        "p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000002)",
        "p0000001.pad.Face(G s0000001.sketch.Face1)",
        "p0000001.pad.Edge(G s0000001.sketch.Vertex(U s0000001.sketch.Edge.ca000001))",
        "q0000001.pocket.Face(G s0000002.sketch.Face(L s0000002.sketch.Edge.ca000002))",
        "r0000001.revolve.Edge(M s0000003.sketch.Edge.na000002)",
        "g0000001.groove.Face(G s0000003.sketch.Face1)",
        // The fillet, built, its base's names kept (the fake's fillet keeps every element).
        "f0000001 ElementRef { kind: Edge",
    ] {
        assert!(names.contains(needle), "no {needle} in:\n{names}");
    }
}

#[test]
fn the_names_a_scheme_gives_are_pinned_with_its_version() {
    let digest: String = Sha256::digest((names() + &built()).as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(
        (std::str::from_utf8(NAMING_SCHEME).unwrap(), digest.as_str()),
        (
            "silico naming 5\n",
            "3a8e05eda1c6037fbf62de53747ad102f8fdfde25c4defe95e4a2049f9de80a8"
        ),
        "the names changed: bump NAMING_SCHEME, then pin the new digest"
    );
}

#[test]
fn every_name_the_scheme_gives_reads_back_as_itself() {
    // N8: each name says its features' types, which the parser reads from the text (the
    // makers only say which features there are), so every name the documents and the
    // histories get reads back, written alike; and with its type word changed, reads as
    // another name. (Names by number are unstable, refused as references by the gate, but
    // they read.)
    let all = names() + &built();
    let any = |_: &FeatureId| Some(silico_core::naming::Maker::By(OpCode::Fuse));
    let mut read = 0;
    for line in all.lines() {
        let Some((_, text)) = line.split_once("} ") else {
            panic!("{line}");
        };
        let name =
            silico_core::naming::Name::parse(text, &any).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(name.to_string(), text);
        read += 1;
    }
    assert!(read > 500, "{read}");
    let rim = "m3tq8v0z.cut.Edge(G k7f2qx3m.box.Face6, G a91cw0de.cylinder.Face1)";
    let as_common = rim.replacen(".cut.", ".common.", 1);
    assert_ne!(
        silico_core::naming::Name::parse(rim, &any).unwrap(),
        silico_core::naming::Name::parse(&as_common, &any).unwrap()
    );
}
