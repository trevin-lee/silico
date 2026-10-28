//! break-it: N8 and N9 (every name says its feature's type, the parser reads it from the
//! text, and `OpCode::bites` decides which side of an operation keeps an element's own name,
//! the other side written `I`). Hostile
//! histories built by hand and generated, names an agent could plausibly write wrong, and the
//! gate's and the rebuild's words for them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::model::WAIT;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use silico_core::ErrorCode;
use silico_core::document::{Checked, FeatureId, read};
use silico_core::fake::FakeKernel;
use silico_core::kernel::{
    ElementCounts, ElementFate, ElementKind, ElementRef, History, InputHistory, Primitive,
    Reported, Topology,
};
use silico_core::naming::{ElementMap, Maker, Name, OpCode, PrimitiveKind, freecad};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};

const HEAD: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": ["#;
const BLOCK: &str = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#;
const PIN: &str = r#"{"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": 2.5, "height": 12}"#;
const HOLE: &str = r#"{"id": "m3tq8v0z", "type": "cut", "v": 1, "name": "Hole", "base": "k7f2qx3m", "tool": "a91cw0de"}"#;

/// The block, pin, and hole, then a fillet of the hole's `edge`.
fn document(edge: &str) -> String {
    format!(
        r#"{HEAD}{BLOCK},
{PIN},
{HOLE},
{{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "m3tq8v0z", "edges": [{}], "radius": 0.5}}]}}"#,
        serde_json::to_string(edge).unwrap()
    )
}

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn published(engine: &Recompute<FakeKernel>, text: &str) -> Arc<Model> {
    let document = Checked::new(read(text.as_bytes()).unwrap()).unwrap();
    match engine.start(document).wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish in a minute"),
    }
}

fn element(kind: ElementKind, index: u32) -> ElementRef {
    ElementRef { kind, index }
}

fn block() -> ElementMap {
    ElementMap::primitive(
        id("pppppppp"),
        &Primitive::Box {
            dx: 10.0,
            dy: 10.0,
            dz: 10.0,
        },
    )
}

fn pin() -> ElementMap {
    ElementMap::primitive(
        id("tttttttt"),
        &Primitive::Cylinder {
            radius: 1.0,
            height: 6.0,
        },
    )
}

// --- Kept elements and the bites table -------------------------------------------------

/// A boolean of the block (base) and the pin (tool) whose result is one edge, the pin's
/// Edge3 (its top circle) there unchanged: the pin wholly inside the block, so a cut leaves a
/// void whose ceiling's rim is that circle, and a common leaves the pin, whose top rim is it.
fn pin_inside(op: OpCode) -> ElementMap {
    let mut edges = vec![ElementFate::default(); 3];
    edges[2].kept = Some(1);
    let history = History {
        inputs: vec![
            InputHistory {
                faces: vec![ElementFate::default(); 6],
                edges: vec![ElementFate::default(); 12],
                vertices: vec![ElementFate::default(); 8],
            },
            InputHistory {
                faces: vec![ElementFate::default(); 3],
                edges,
                vertices: vec![ElementFate::default(); 2],
            },
        ],
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
    ElementMap::operation(id("rrrrrrrr"), op, &[&block(), &pin()], &history)
}

#[test]
fn a_tool_edge_kept_whole_inside_the_base_is_the_cuts_own_and_the_commons_own() {
    // Found by break-it (round 2's R2B1), closed by N9: step 1 (kept) never asked `bites`,
    // so an element kept whole kept its input's bare name under any operation, and the
    // void's concave rim under the cut and the pin's convex rim under the common were both
    // `tttttttt.cylinder.Edge3`; a fillet of it followed the retype silently (the real
    // kernel's proof is silico-kernel's tests/references.rs, the void's). Now what each keeps
    // of an input it doesn't bite is its own, `I`, whole or trimmed; FreeCAD's form writes
    // a kept element as its source, as FreeCAD's `mapSubElement` does (FreeCAD 1.1.3:
    // `Edge3;:H5e9,E`, its tag aside).
    let cut = pin_inside(OpCode::Cut);
    let common = pin_inside(OpCode::Common);
    let rim = element(ElementKind::Edge, 1);
    let (under_cut, under_common) = (cut.name(rim).unwrap(), common.name(rim).unwrap());
    assert_eq!(
        under_cut.to_string(),
        "rrrrrrrr.cut.Edge(I tttttttt.cylinder.Edge3)"
    );
    assert_eq!(
        under_common.to_string(),
        "rrrrrrrr.common.Edge(I tttttttt.cylinder.Edge3)"
    );
    assert_eq!(freecad::text(&under_cut), "Edge3");
    assert_eq!(freecad::text(&under_common), "Edge3");
}

#[test]
fn a_base_edge_kept_whole_outside_the_tool_is_named_alike_by_a_cut_and_a_fuse() {
    // The other side of the table, which N8 means to share: a cut and a fuse both keep the
    // base's outside, so its edge kept whole there (the same edge, the same material on the
    // same side) is its own name under either.
    let mut edges = vec![ElementFate::default(); 12];
    edges[0].kept = Some(1);
    let history = History {
        inputs: vec![
            InputHistory {
                faces: vec![ElementFate::default(); 6],
                edges,
                vertices: vec![ElementFate::default(); 8],
            },
            InputHistory {
                faces: vec![ElementFate::default(); 3],
                edges: vec![ElementFate::default(); 3],
                vertices: vec![ElementFate::default(); 2],
            },
        ],
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
    for op in [OpCode::Cut, OpCode::Fuse] {
        let map = ElementMap::operation(id("rrrrrrrr"), op, &[&block(), &pin()], &history);
        assert_eq!(
            map.name(element(ElementKind::Edge, 1)).unwrap().to_string(),
            "pppppppp.box.Edge1",
            "{op:?}"
        );
    }
}

// --- What the parser and the gate say about type words ---------------------------------

#[test]
fn an_operations_name_without_its_type_word_is_told_a_form_the_gate_takes() {
    // Found by break-it: a scheme-2 name (before N8: `m3tq8v0z.Edge(G ...)`, every saved document's
    // reference to a cut's rim) is refused, and told to write it "as `m3tq8v0z.cut.Edge1`":
    // an operation's element by its number, which the gate then refuses as unstable. The
    // example an agent copies should be one the gate takes (`m3tq8v0z.cut.Edge(...)`).
    let old = "m3tq8v0z.Edge(G k7f2qx3m.box.Face6, G a91cw0de.cylinder.Face1)";
    let error = Name::parse(old, &|f: &FeatureId| match f.as_str() {
        "m3tq8v0z" => Some(Maker::By(OpCode::Cut)),
        "k7f2qx3m" => Some(Maker::Itself(PrimitiveKind::Box)),
        "a91cw0de" => Some(Maker::Itself(PrimitiveKind::Cylinder)),
        _ => None,
    })
    .unwrap_err();
    assert_eq!(error.column, 10, "{error}");
    // The example the message gives, between its last backticks.
    let example = error.why.rsplit('`').nth(1).unwrap().to_owned();
    let text = document(&example);
    let gate = Checked::new(read(text.as_bytes()).unwrap());
    assert!(
        gate.is_ok(),
        "the parser's example {example:?} is refused by the gate: {:?}",
        gate.unwrap_err().errors()
    );
}

#[test]
fn a_primitives_element_written_with_an_operations_word_is_refused_saying_the_word_is_wrong() {
    // Found by break-it (wording): `k7f2qx3m.cut.Edge1`, the block's Edge1 with the wrong word, is
    // refused by the gate as "a duplicate (~n) or an operation's element by its number", told
    // to "name the edge by how it came about": the block is a box, whose elements are named
    // by number, and the word is all that's wrong. With a primitive's wrong word
    // (`k7f2qx3m.cylinder.Edge1`) the gate passes it and the rebuild says "names an element a
    // cylinder made, but k7f2qx3m is a box now"; this one should say as much.
    // No number of the primitive's is suggested: none is the one the operation's was.
    for (edge, feature, kind) in [
        ("k7f2qx3m.cut.Edge1", "k7f2qx3m", "box"),
        ("a91cw0de.fuse.Edge2", "a91cw0de", "cylinder"),
    ] {
        let text = document(edge);
        let invalid = Checked::new(read(text.as_bytes()).unwrap()).unwrap_err();
        assert_eq!(invalid.errors().len(), 1, "{:?}", invalid.errors());
        let error = &invalid.errors()[0];
        let (word, number) = edge.split_once('.').unwrap().1.split_once('.').unwrap();
        assert_eq!(
            (error.code(), error.parameter(), error.message(), error.suggestion()),
            (
                ErrorCode::InvalidInput,
                Some("edges.0"),
                format!(
                    "edges.0, \"{edge}\", names {number} of a {word}, but {feature} is a {kind}: its elements are another's"
                )
                .as_str(),
                Some(
                    format!(
                        "name the edge as {feature}'s solid names it now (a {kind}'s elements are `{feature}.{kind}.` and their numbers)"
                    )
                    .as_str()
                ),
            ),
        );
    }
}

#[test]
fn an_operations_element_written_with_a_primitives_word_breaks_at_the_rebuild_never_resolves() {
    // `m3tq8v0z.box.Edge5`: the cut's own element by its number, written with a box's word.
    // Before N8 the gate refused it (an operation's element by its number); now it reads as a
    // primitive's, passes, and must break at the rebuild, never finding the cut's Edge5.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    let model = published(&engine, &document("m3tq8v0z.box.Edge5"));
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("f0000001")) else {
        panic!("{:?}", model.outcome(&id("f0000001")));
    };
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code(), silico_core::ErrorCode::ReferenceBroken);
    assert_eq!(errors[0].parameter(), Some("edges.0"));
    assert!(
        errors[0]
            .message()
            .contains("names an element a box made, but m3tq8v0z is a cut now"),
        "{}",
        errors[0]
    );
    // And the next valid edit still builds.
    let model = published(&engine, &document("k7f2qx3m.box.Edge1"));
    assert!(model.errors().next().is_none());
}

#[test]
fn every_type_word_glued_to_every_other_is_refused_or_read_as_written() {
    // Prefixes and near-misses of the ten words: `cone` and `common`, `cut` and `cutt`, a
    // word with its dot missing, doubled, or in capitals. Each is read as exactly the word
    // written, or refused saying where; never taken for another word.
    let words = [
        "box", "cylinder", "sphere", "cone", "torus", "cut", "fuse", "common", "fillet", "chamfer",
    ];
    let any = |_: &FeatureId| Some(Maker::By(OpCode::Cut));
    for word in words {
        for bad in [
            format!("{word}Edge1"),
            format!("{word}..Edge1"),
            format!("{}.Edge1", word.to_uppercase()),
            format!("{word}s.Edge1"),
            format!("{}.Edge1", &word[..word.len() - 1]),
            format!("{word}.{word}.Edge1"),
            format!(" {word}.Edge1"),
        ] {
            let text = format!("aaaaaaaa.{bad}");
            assert!(Name::parse(&text, &any).is_err(), "{text} reads");
        }
        let text = format!("aaaaaaaa.{word}.Edge1");
        let name = Name::parse(&text, &any).unwrap();
        assert_eq!(name.to_string(), text);
    }
}

// --- Generated histories, every operation -----------------------------------------------

/// A random two-input history that passes `History::validate`: each input element kept,
/// modified, or generated at random into a small result, with a random topology. Positions
/// are 1 to 3, so a sole result is sometimes not OCCT's first.
fn histories() -> impl Strategy<Value = History> {
    (1u32..=4, 0u32..=6, 0u32..=4)
        .prop_flat_map(|(faces, edges, vertices)| {
            let result = ElementCounts {
                faces,
                edges,
                vertices,
                solids: 1,
            };
            let input = (0usize..=4, 0usize..=6, 0usize..=4).prop_flat_map(move |(f, e, v)| {
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
                proptest::collection::vec(input, 2),
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
        .prop_filter("a history the core checks", |h| h.validate(2).is_ok())
}

fn reports(kinds: Vec<ElementKind>, result: ElementCounts) -> impl Strategy<Value = Vec<Reported>> {
    let all: Vec<ElementRef> = kinds
        .into_iter()
        .flat_map(|k| (1..=result.of(k)).map(move |i| element(k, i)))
        .collect();
    let n = all.len();
    (
        proptest::sample::subsequence(all, 0..=n.min(2)),
        proptest::collection::vec(prop_oneof![3 => Just(1u32), 1 => 2u32..=3], 2),
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
        prop_oneof![3 => Just(None), 1 => (1..=count).prop_map(Some)].boxed()
    };
    (
        keep,
        reports(vec![kind], result),
        reports(
            vec![ElementKind::Vertex, ElementKind::Edge, ElementKind::Face],
            result,
        ),
    )
        .prop_map(|(kept, modified, generated)| ElementFate {
            kept,
            modified,
            generated,
        })
}

fn subset(n: u32, most: usize) -> impl Strategy<Value = Vec<u32>> {
    let all: Vec<u32> = (1..=n).collect();
    let len = all.len();
    proptest::sample::subsequence(all, 0..=len.min(most)).prop_shuffle()
}

fn topology(result: ElementCounts) -> impl Strategy<Value = Topology> {
    let face = subset(result.edges, 4).prop_flat_map(|edges| {
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
        proptest::collection::vec(subset(result.vertices, 2), result.edges as usize),
    )
        .prop_map(|(faces, edge_vertices)| {
            let (face_edges, outer_edges) = faces.into_iter().unzip();
            Topology {
                face_edges,
                outer_edges,
                edge_vertices,
            }
        })
}

/// The inputs' maps: a box and a cylinder's, numbered by number for as many elements as the
/// history lists (more than a primitive has, so the builder's numbered names are reached).
fn inputs(history: &History) -> Vec<ElementMap> {
    ["pppppppp", "tttttttt"]
        .into_iter()
        .zip(&history.inputs)
        .zip([OpCode::Fuse, OpCode::Common])
        .map(|((feature, input), op)| {
            ElementMap::by_number(
                id(feature),
                op,
                ElementCounts {
                    faces: input.faces.len() as u32,
                    edges: input.edges.len() as u32,
                    vertices: input.vertices.len() as u32,
                    solids: 1,
                },
            )
        })
        .collect()
}

fn makers_for(op: OpCode) -> impl Fn(&FeatureId) -> Option<Maker> {
    move |feature: &FeatureId| match feature.as_str() {
        "pppppppp" => Some(Maker::By(OpCode::Fuse)),
        "tttttttt" => Some(Maker::By(OpCode::Common)),
        "rrrrrrrr" => Some(Maker::By(op)),
        _ => None,
    }
}

fn config(cases: u32) -> Config {
    Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    }
}

#[test]
fn every_name_any_operation_gives_reads_back_from_its_text_and_finds_its_element() {
    // Under every operation (common and chamfer included, which the grammar's own round trip
    // never builds, and the sweep features' last steps), each name a map holds is written
    // with its type words and read back as a name that finds the same element. (Not a
    // sketch's: its map is named by its IDs, `ElementMap::sketch`, and its whole names are
    // refused as references, its elements being in no solid.)
    TestRunner::new(config(1500))
        .run(&histories(), |history| {
            let maps = inputs(&history);
            let refs: Vec<&ElementMap> = maps.iter().collect();
            for op in OpCode::ALL.into_iter().filter(|op| *op != OpCode::Sketch) {
                let map = ElementMap::operation(id("rrrrrrrr"), op, &refs, &history);
                let makers = makers_for(op);
                for (element, names) in map.iter() {
                    for name in names {
                        let text = name.to_string();
                        let read = Name::parse(&text, &makers);
                        prop_assert!(read.is_ok(), "{op:?}: {text}: {read:?}");
                        let read = read.unwrap();
                        prop_assert_eq!(map.element(&read), Some(element), "{:?}: {}", op, text);
                        prop_assert_eq!(read.to_string(), text);
                    }
                }
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_name_two_operations_both_give_from_one_history_is_an_inputs_own_where_both_bite_it() {
    // Two operations on the same inputs and history (a boolean retyped, its history alike):
    // every name both give is an input's own, and only where both operations bite that
    // input, kept whole or modified alike (N9: what an operation keeps of an input it
    // doesn't bite, even whole, is its own, `I`). Anything the feature makes says the
    // operation, so it's never shared.
    TestRunner::new(config(1500))
        .run(&histories(), |history| {
            let maps = inputs(&history);
            let refs: Vec<&ElementMap> = maps.iter().collect();
            let booleans = [OpCode::Fuse, OpCode::Cut, OpCode::Common];
            let built: BTreeMap<OpCode, ElementMap> = booleans
                .into_iter()
                .map(|op| {
                    (
                        op,
                        ElementMap::operation(id("rrrrrrrr"), op, &refs, &history),
                    )
                })
                .collect();
            let stable = |map: &ElementMap| -> BTreeSet<String> {
                map.iter()
                    .flat_map(|(_, names)| names.iter())
                    .filter(|name| !map.is_unstable(name))
                    .map(|name| name.to_string())
                    .collect()
            };
            for (x, a) in &built {
                for (y, b) in &built {
                    if x >= y {
                        continue;
                    }
                    for shared in stable(a).intersection(&stable(b)) {
                        prop_assert!(
                            !shared.starts_with("rrrrrrrr."),
                            "{x:?} and {y:?} both name {shared}"
                        );
                        let input = usize::from(shared.starts_with("tttttttt."));
                        prop_assert!(
                            x.bites(input) && y.bites(input),
                            "{x:?} and {y:?} both name input {input}'s {shared}, \
                             though one keeps that input's other side"
                        );
                    }
                }
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_hundred_type_words_of_garbage_around_a_name_are_refused_promptly() {
    // A name that's all type words (`aaaaaaaa.box.box.box....`): every word is tried at
    // every place, so the reader must still stop at the first that isn't a kind.
    let text = format!("aaaaaaaa.{}Edge1", "chamfer.".repeat(100_000));
    let any = |_: &FeatureId| Some(Maker::By(OpCode::Chamfer));
    let started = std::time::Instant::now();
    let error = Name::parse(&text, &any).unwrap_err();
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(error.column, 18, "{error}");
}
