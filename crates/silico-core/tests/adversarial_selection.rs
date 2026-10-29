//! break-it: M5.7's selection read against a model (`silico_core::selection::resolve`).
//! Names a confused agent or a stale app would hold: empty, padded, misspelled, by number 0
//! and past `u32`, `~0`, another type's word, a sketch's, a solid's, NUL, bidi and
//! zero-width characters, multibyte text astride the 40-character cut, a megabyte, nesting
//! past the parser's bound; a hundred thousand names; every name of every element of a
//! model with aliases and duplicates, each kept as selected and stored by a name the gate
//! and a fillet then take (or refused as the fillet refuses it); names of features that
//! failed, were blocked, or a hostile selection asks after thousands of times; and any
//! selection drawn from these, against invariants of order, deduplication, and
//! determinism. Every expected value is by hand or from another part of the core than the
//! selection's (the gate, a fillet's rebuild).

use super::model;
use super::sweeps;

use std::collections::BTreeSet;
use std::sync::Arc;

use proptest::prelude::*;
use silico_core::document::{Checked, FeatureId};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{
    BooleanOp, BuiltData, ElementCounts, ElementFate, ElementKind, ElementRef, History,
    InputHistory, Op, Reported, Topology,
};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::selection::{Resolved, resolve};
use silico_core::{Error, ErrorCode};

const HEADER: &str =
    r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": ["#;

fn text(features: &[String]) -> String {
    format!("{HEADER}{}]}}", features.join(", "))
}

fn checked(features: &[String]) -> Checked {
    model::checked(&text(features))
}

fn rebuilt(features: &[String], setup: impl FnOnce(&FakeKernel)) -> Arc<Model> {
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    setup(engine.kernel());
    match engine
        .start(Arc::new(checked(features)))
        .wait_timeout(model::WAIT)
    {
        Ok(Ok(RunOutcome::Published(model))) => model,
        _ => panic!("not published"),
    }
}

fn plain(features: &[String]) -> Arc<Model> {
    rebuilt(features, |_| {})
}

fn a_box(id: &str, x: u32, length: u32) -> String {
    format!(
        r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Block", "position": [{x}, 0, 0], "length": {length}, "width": 1, "height": 1}}"#
    )
}

fn operation(id: &str, kind: &str, base: &str, tool: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "{kind}", "v": 1, "name": "Op", "base": "{base}", "tool": "{tool}"}}"#
    )
}

fn fillet(id: &str, base: &str, edges: &[&str]) -> String {
    let edges: Vec<String> = edges
        .iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect();
    format!(
        r#"{{"id": "{id}", "type": "fillet", "v": 1, "name": "Round", "base": "{base}", "edges": [{}], "radius": 0.1}}"#,
        edges.join(", ")
    )
}

fn element(kind: ElementKind, index: u32) -> ElementRef {
    ElementRef { kind, index }
}

/// A kernel that refuses every box `dx` long.
fn refusing_length(dx: f64, code: ErrorCode) -> impl FnOnce(&FakeKernel) {
    move |kernel: &FakeKernel| {
        kernel.refuse(move |op| {
            matches!(op, Op::MakePrimitive(silico_protocol::Primitive::Box { dx: x, .. }) if *x == dx)
                .then(|| Error::new(code, "MakeBox failed"))
        })
    }
}

/// A fuse's history in which the base, a 1 mm cube, is the result, every element kept but
/// its Edge1 and Edge2, which come about alike (each generated from Face1 and Face6: two
/// edges of one name, the second its `~1`, both unstable) and its Edge3 (from Face1 and
/// Face3, stable). The tool reported by nothing. (As selection.rs's unit tests forge it.)
fn duplicating_fuse() -> History {
    let kept = |n: u32| ElementFate {
        kept: Some(n),
        ..ElementFate::default()
    };
    let edge = |index| Reported {
        element: element(ElementKind::Edge, index),
        position: 1,
    };
    let mut faces: Vec<ElementFate> = (1..=6).map(kept).collect();
    faces[0].generated = vec![edge(1), edge(2), edge(3)];
    faces[2].generated = vec![edge(3)];
    faces[5].generated = vec![edge(1), edge(2)];
    let history = History {
        inputs: vec![
            InputHistory {
                faces,
                edges: vec![ElementFate::default(); 3]
                    .into_iter()
                    .chain((4..=12).map(kept))
                    .collect(),
                vertices: (1..=8).map(kept).collect(),
            },
            InputHistory {
                faces: vec![ElementFate::default(); 6],
                edges: vec![ElementFate::default(); 12],
                vertices: vec![ElementFate::default(); 8],
            },
        ],
        result: ElementCounts {
            faces: 6,
            edges: 12,
            vertices: 8,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![vec![]; 6],
            outer_edges: vec![vec![]; 6],
            edge_vertices: vec![vec![]; 12],
        },
    };
    history.validate(2).unwrap();
    history
}

/// A fuse's history: the base whole, and the tool's Face1 kept as the base's Face1 too
/// (one face, two names); the rest of the tool reported by nothing.
fn aliasing_fuse() -> History {
    let kept = |n: u32| ElementFate {
        kept: Some(n),
        ..ElementFate::default()
    };
    let mut tool_faces = vec![ElementFate::default(); 6];
    tool_faces[0] = kept(1);
    let history = History {
        inputs: vec![
            InputHistory {
                faces: (1..=6).map(kept).collect(),
                edges: (1..=12).map(kept).collect(),
                vertices: (1..=8).map(kept).collect(),
            },
            InputHistory {
                faces: tool_faces,
                edges: vec![ElementFate::default(); 12],
                vertices: vec![ElementFate::default(); 8],
            },
        ],
        result: ElementCounts {
            faces: 6,
            edges: 12,
            vertices: 8,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: vec![vec![]; 6],
            outer_edges: vec![vec![]; 6],
            edge_vertices: vec![vec![]; 12],
        },
    };
    history.validate(2).unwrap();
    history
}

/// A fake whose fuses give the base back with the history `history` picks for the tool.
fn forging(
    history: impl Fn(&str) -> Option<History> + Send + Sync + 'static,
) -> impl FnOnce(&FakeKernel) {
    move |kernel: &FakeKernel| {
        kernel.answer_instead(move |op| match op {
            Op::Boolean {
                op: BooleanOp::Fuse,
                base,
                tool,
            } => history(&FakeKernel::describe(tool)?).map(|history| {
                silico_protocol::Reply::Built(BuiltData {
                    shape: base.clone(),
                    history,
                })
            }),
            _ => None,
        })
    }
}

/// The features of a model with a duplicated (unstable) edge, an aliased face, a cut's
/// kept and inside elements, and a plain box beside them.
fn rich_features() -> Vec<String> {
    vec![
        a_box("aaaaaaaa", 0, 1),
        a_box("bbbbbbbb", 5, 1),
        operation("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
        a_box("cccccccc", 9, 2),
        operation("ssssssss", "fuse", "rrrrrrrr", "cccccccc"),
        a_box("dddddddd", 20, 4),
        a_box("eeeeeeee", 21, 1),
        operation("tttttttt", "cut", "dddddddd", "eeeeeeee"),
    ]
}

fn rich_setup() -> impl FnOnce(&FakeKernel) {
    forging(|tool| {
        if tool.contains("box(2,") {
            Some(aliasing_fuse())
        } else {
            Some(duplicating_fuse())
        }
    })
}

fn rich() -> Arc<Model> {
    rebuilt(&rich_features(), rich_setup())
}

/// Every name of every element of every body: (body, element, name's text).
fn every_name(model: &Model) -> Vec<(FeatureId, ElementRef, String)> {
    let mut all = Vec::new();
    for body in model.bodies() {
        let map = model.element_map(body).unwrap();
        for (element, names) in map.iter() {
            for name in names {
                all.push((body.clone(), element, name.to_string()));
            }
        }
    }
    all
}

/// The one reason `text`, selected alone, is gone for.
#[track_caller]
fn sole_reason(model: &Model, text: &str) -> Error {
    let resolved = resolve(model, &[text]);
    assert!(resolved.kept.is_empty(), "{text:?} kept: {resolved:?}");
    let [gone] = resolved.gone.as_slice() else {
        panic!("{text:?}: {resolved:?}")
    };
    assert_eq!(gone.name, text);
    let [error] = gone.why.errors() else {
        panic!("{text:?}: {:?}", gone.why)
    };
    assert_eq!(gone.why.total(), 1);
    error.clone()
}

/// A reason any agent can act on: says where (`selection.0`), what (the name, cut short to
/// 200 characters and `…` in `elements`, to 40 in the message), and what to do.
#[track_caller]
fn assert_actionable(error: &Error, text: &str) {
    assert_eq!(error.parameter(), Some("selection.0"), "{error:?}");
    assert!(
        error.message().starts_with("selection.0, \""),
        "{}",
        error.message()
    );
    let [element] = error.elements() else {
        panic!("{error:?}")
    };
    assert!(element.chars().count() <= 201, "{element:?}");
    assert!(
        text.starts_with(element.trim_end_matches('…')),
        "{element:?}"
    );
    assert!(
        error.suggestion().is_some_and(|s| !s.is_empty()),
        "{error:?}"
    );
    // The message is short however long the name: the name in it is cut short.
    assert!(error.message().len() < 600, "{}", error.message().len());
}

#[test]
fn an_empty_selection_keeps_nothing_and_loses_nothing() {
    let model = plain(&[a_box("k7f2qx3m", 0, 1)]);
    let none: [&str; 0] = [];
    assert_eq!(
        resolve(&model, &none),
        Resolved {
            kept: Vec::new(),
            gone: Vec::new()
        }
    );
}

#[test]
fn texts_that_are_no_name_are_each_gone_with_one_actionable_reason() {
    // Each is no name of the box's: padded, cased otherwise, numbered 0, with a leading
    // zero, past u32, negative, `~0`, a `~1` of an element nothing duplicated, an empty
    // string, whitespace, NUL, U+200B, U+202E, a newline, and the right name with another
    // type's word. None may be found: the box's Face1 is `k7f2qx3m.box.Face1` exactly.
    let model = plain(&[a_box("k7f2qx3m", 0, 1)]);
    let not_names = [
        "",
        " ",
        "k7f2qx3m.box.Face1 ",
        " k7f2qx3m.box.Face1",
        "k7f2qx3m.box.Face1\n",
        "K7F2QX3M.box.Face1",
        "k7f2qx3m.Box.Face1",
        "k7f2qx3m.box.face1",
        "k7f2qx3m.box.Face0",
        "k7f2qx3m.box.Face01",
        "k7f2qx3m.box.Face4294967296",
        "k7f2qx3m.box.Face99999999999999999999999",
        "k7f2qx3m.box.Face-1",
        "k7f2qx3m.box.Face1~0",
        "k7f2qx3m.box.Face\u{0}1",
        "k7f2qx3m.box.Face\u{200b}1",
        "\u{202e}k7f2qx3m.box.Face1",
        "k7f2qx3m..Face1",
        "k7f2qx3m.box.",
        "k7f2qx3m.box.Face",
        ".box.Face1",
        "k7f2qx3m.box.Face1.",
        "k7f2qx3m.box.Face(G k7f2qx3m.box.Face1",
        "k7f2qx3m.box.Face()",
    ];
    for text in not_names {
        let error = sole_reason(&model, text);
        assert_eq!(
            error.code(),
            ErrorCode::ReferenceBroken,
            "{text:?}: {error:?}"
        );
        assert!(
            error.message().contains(", isn't a name: "),
            "{text:?}: {}",
            error.message()
        );
        assert_actionable(&error, text);
        // An invisible or control character is named by its code point, not printed raw.
        for c in ['\u{0}', '\u{200b}', '\u{202e}', '\n'] {
            assert!(
                !error.message().contains(c),
                "{text:?}: {:?}",
                error.message()
            );
        }
    }
}

#[test]
fn names_that_parse_but_find_nothing_are_gone_with_the_box_s_likeliest_names() {
    // Face7 (a box has six), Face 2^32 - 1, an edge past twelve, a `~1` of a face nothing
    // duplicated, and the box's Face1 written as a cylinder's: each a name, of no element.
    let model = plain(&[a_box("k7f2qx3m", 0, 1)]);
    for text in [
        "k7f2qx3m.box.Face7",
        "k7f2qx3m.box.Face4294967295",
        "k7f2qx3m.box.Edge13",
        "k7f2qx3m.box.Vertex9",
        "k7f2qx3m.box.Face1~1",
        "k7f2qx3m.cylinder.Face1",
    ] {
        let error = sole_reason(&model, text);
        assert_eq!(
            error.code(),
            ErrorCode::ReferenceBroken,
            "{text:?}: {error:?}"
        );
        assert!(
            error
                .message()
                .ends_with(", names no element of the live solids"),
            "{text:?}: {}",
            error.message()
        );
        assert_actionable(&error, text);
        // Whatever is offered is a name the model finds, and stable.
        for candidate in error.candidates() {
            let found = resolve(&model, &[candidate.as_str()]);
            assert_eq!(found.kept.len(), 1, "{text:?} offers {candidate:?}");
            assert!(found.kept[0].stable(), "{text:?} offers {candidate:?}");
        }
    }
}

#[test]
fn a_name_astride_the_cut_in_multibyte_text_is_cut_on_a_character() {
    // ASCII characters, then four-byte characters from about the 40th (the message's cut)
    // and the 200th (the element's): neither cut falls inside a character.
    let model = plain(&[a_box("k7f2qx3m", 0, 1)]);
    for prefix in (37..=41).chain(197..=201) {
        let text = format!("{}{}", "k".repeat(prefix), "\u{1f600}".repeat(5));
        let error = sole_reason(&model, &text);
        assert_eq!(error.code(), ErrorCode::ReferenceBroken);
        assert_actionable(&error, &text);
    }
}

#[test]
fn a_megabyte_name_and_nesting_past_the_bound_are_gone_promptly() {
    // A name a megabyte long, a name nested 100,000 levels (the parser stops at 256), and
    // one with a hundred thousand `~1`s: each refused as no name, quickly, on a test
    // thread's own stack.
    let model = plain(&[a_box("k7f2qx3m", 0, 1)]);
    let long = format!("k7f2qx3m.box.Face{}", "1".repeat(1 << 20));
    let deep = format!(
        "{}k7f2qx3m.box.Face1{}",
        "k7f2qx3m.cut.Face(G ".repeat(100_000),
        ")".repeat(100_000)
    );
    let tildes = format!("k7f2qx3m.box.Face1{}", "~1".repeat(100_000));
    for text in [long, deep, tildes] {
        let shown: String = text.chars().take(30).collect();
        let model = Arc::clone(&model);
        let error = model::promptly("a hostile name", move || sole_reason(&model, &text));
        assert_eq!(
            error.code(),
            ErrorCode::ReferenceBroken,
            "{shown}: {error:?}"
        );
        assert!(error.message().len() < 600, "{shown}");
    }
}

#[test]
fn a_hundred_thousand_names_resolve_each_to_its_place() {
    // Faces 1 to 100,000 of a box, every one past 6 gone with candidates, and the six found:
    // the candidates built once, each name's cost its own.
    let model = plain(&[a_box("k7f2qx3m", 0, 1)]);
    let names: Vec<String> = (1..=100_000)
        .map(|n| format!("k7f2qx3m.box.Face{n}"))
        .collect();
    let resolved = model::promptly("100,000 names", move || resolve(&model, &names));
    assert_eq!(resolved.kept.len(), 6);
    assert_eq!(resolved.gone.len(), 99_994);
}

#[test]
fn every_name_of_every_element_is_kept_as_selected_and_stored_by_a_name_it_finds() {
    // Each name of each element of each body, alone: kept, its text the selection's
    // exactly, in the body whose map has it, the element the map says. A store's name, if
    // any, finds the same element, is stable in the map, and stores itself; if none, every
    // name of the element is unstable (the map says so, not the selection).
    let model = rich();
    let all = every_name(&model);
    assert!(all.len() > 70, "{}", all.len());
    assert!(round_trips(&model, &all) >= 2);
}

#[test]
fn every_name_of_a_pad_and_a_pocket_of_a_sketch_is_kept_as_selected() {
    // A pad of a rectangle, pocketed by a smaller one: names derived from a sketch's
    // curves (`Edge.na000001`), each kept as selected, as the box's are.
    let features = [
        sweeps::rectangle("s0000001", "xy", [0.0, 0.0], 10.0, 10.0),
        sweeps::pad("p0000001", None, "s0000001", sweeps::TEN),
        sweeps::rectangle("s0000002", "xy", [2.0, 2.0], 3.0, 3.0),
        sweeps::pocket("q0000001", "p0000001", "s0000002", &sweeps::one_side("2")),
    ]
    .map(|f| f.to_owned());
    let model = plain(&features);
    let all = every_name(&model);
    assert!(
        all.iter().any(|(_, _, n)| n.contains(".sketch.")),
        "{all:?}"
    );
    round_trips(&model, &all);
    // A sketch's own elements, and its face, whole: in no solid, gone, saying why.
    for text in [
        "s0000001.sketch.Edge.na000001",
        "s0000001.sketch.Vertex.pa000001",
        "s0000001.sketch.Face1",
        "s0000001.box.Face1",
    ] {
        let error = sole_reason(&model, text);
        assert!(
            matches!(
                error.code(),
                ErrorCode::ReferenceBroken | ErrorCode::InvalidInput
            ),
            "{text}: {error:?}"
        );
        assert_actionable(&error, text);
    }
}

/// Checks each of `all` round-trips, and returns how many were held unstable.
fn round_trips(model: &Model, all: &[(FeatureId, ElementRef, String)]) -> usize {
    let mut unstable_seen = 0;
    for (body, element, text) in all {
        let resolved = resolve(model, &[text.as_str()]);
        assert!(resolved.gone.is_empty(), "{text}: {resolved:?}");
        let [kept] = resolved.kept.as_slice() else {
            panic!("{text}: {resolved:?}")
        };
        assert_eq!(kept.name.to_string(), *text);
        assert_eq!(&kept.feature, body, "{text}");
        assert_eq!(kept.element, *element, "{text}");
        let map = model.element_map(body).unwrap();
        match &kept.stored {
            Ok(stored) => {
                assert!(!map.is_unstable(stored), "{text} stored as {stored}");
                assert_eq!(map.element(stored), Some(*element), "{text}");
                if !map.is_unstable(&kept.name) {
                    assert_eq!(stored.to_string(), *text);
                }
                let again = resolve(model, &[stored.to_string()]);
                assert_eq!(again.kept.len(), 1);
                assert_eq!(again.kept[0].element, *element);
                assert_eq!(
                    again.kept[0].stored.as_ref().ok().map(ToString::to_string),
                    Some(stored.to_string())
                );
            }
            Err(refusal) => {
                unstable_seen += 1;
                assert!(
                    map.names(*element).iter().all(|n| map.is_unstable(n)),
                    "{text}"
                );
                assert_eq!(refusal.code(), ErrorCode::ReferenceBroken);
                assert_eq!(refusal.parameter(), Some("selection.0"));
                for candidate in refusal.candidates() {
                    let found = resolve(model, &[candidate.as_str()]);
                    assert!(
                        found.kept.len() == 1 && found.kept[0].stable(),
                        "{text} offers {candidate}"
                    );
                }
            }
        }
    }
    // All at once: one kept for each element, in the order its first name came, none gone.
    let texts: Vec<&str> = all.iter().map(|(_, _, t)| t.as_str()).collect();
    let resolved = resolve(model, &texts);
    assert!(resolved.gone.is_empty());
    let elements: BTreeSet<(FeatureId, ElementRef)> =
        all.iter().map(|(b, e, _)| (b.clone(), *e)).collect();
    assert_eq!(resolved.kept.len(), elements.len());
    unstable_seen
}

#[test]
fn every_edge_a_selection_stores_is_one_the_gate_and_a_fillet_of_its_body_take() {
    // M5.7: whether a store takes a selected edge is the selection's `stable`. Each edge of
    // each body, by each of its names: if the selection stores a name, a fillet of the body
    // naming that name passes the gate and rebuilds with its reference found; if it refuses
    // it, a fillet naming the selected text is refused (by the gate or the rebuild), with
    // `reference_broken` (or `invalid_input`, the gate's).
    let features = rich_features();
    let model = rich();
    let edges: Vec<(FeatureId, ElementRef, String)> = every_name(&model)
        .into_iter()
        .filter(|(_, e, _)| e.kind == ElementKind::Edge)
        .collect();
    let mut checked_stored = 0;
    let mut checked_refused = 0;
    for (body, _, text) in &edges {
        let resolved = resolve(&model, &[text.as_str()]);
        let kept = &resolved.kept[0];
        let (name, expect_taken) = match &kept.stored {
            Ok(stored) => (stored.to_string(), true),
            Err(_) => (text.clone(), false),
        };
        let mut with = features.clone();
        with.push(fillet("ffffffff", body.as_str(), &[name.as_str()]));
        let document = text_of(&with);
        let gated = silico_core::command::gate(document.as_bytes());
        let taken = match gated {
            Err(problems) => {
                assert!(
                    !expect_taken,
                    "{text} stored {name}, gate refused: {problems:?}"
                );
                false
            }
            Ok(_) => {
                let filleted = rebuilt(&with, rich_setup());
                match filleted.outcome(&FeatureId::new("ffffffff").unwrap()) {
                    Some(FeatureOutcome::Built { .. }) => true,
                    Some(FeatureOutcome::Broken { errors, .. }) => {
                        assert!(
                            errors
                                .iter()
                                .all(|e| e.code() == ErrorCode::ReferenceBroken),
                            "{errors:?}"
                        );
                        false
                    }
                    other => panic!("{text}: {other:?}"),
                }
            }
        };
        assert_eq!(taken, expect_taken, "{text} (stored as {name})");
        if expect_taken {
            checked_stored += 1;
        } else {
            checked_refused += 1;
        }
    }
    assert!(checked_stored > 10 && checked_refused >= 2);
}

fn text_of(features: &[String]) -> String {
    text(features)
}

#[test]
fn a_failed_features_names_are_gone_each_saying_its_place_in_the_selection() {
    // `Gone`'s promise: each reason an error at the parameter `selection.<n>`, its place.
    // A box the kernel refuses: its Face1 at place 1, its Edge2 at place 2.
    let model = rebuilt(
        &[a_box("a0000001", 0, 1), a_box("e0000001", 5, 2)],
        refusing_length(2.0, ErrorCode::KernelFailed),
    );
    let resolved = resolve(
        &model,
        &[
            "a0000001.box.Face1",
            "e0000001.box.Face1",
            "e0000001.box.Edge2",
        ],
    );
    assert_eq!(resolved.gone.len(), 2);
    for (place, gone) in [1, 2].into_iter().zip(&resolved.gone) {
        let parameters: Vec<Option<&str>> =
            gone.why.errors().iter().map(Error::parameter).collect();
        let want = format!("selection.{place}");
        // BREAKS: the feature's failure is passed on as it is (no parameter, the kernel's
        // words), so a reason says nothing of where in the selection its name is, against
        // `Gone`'s doc ("each reason an error at the parameter `selection.<n>`").
        assert_eq!(parameters, [Some(want.as_str())], "{gone:?}");
    }
}

#[test]
fn a_name_of_a_feature_blocked_by_a_failure_is_gone_naming_the_feature_and_its_blocker() {
    // A box the kernel refuses, a fillet of it blocked: the fillet's face is gone, saying
    // the fillet didn't build since its base failed; the box's face is gone with the box's
    // own failure. Then the box mended: both found, and the box's face is the fillet's.
    let features = [
        a_box("a0000001", 0, 2),
        fillet("f0000001", "a0000001", &["a0000001.box.Edge1"]),
    ];
    let model = rebuilt(&features, refusing_length(2.0, ErrorCode::KernelFailed));
    let resolved = resolve(&model, &["f0000001.fillet.Face1", "a0000001.box.Face1"]);
    assert!(resolved.kept.is_empty(), "{resolved:?}");
    let [fillet_face, box_face] = resolved.gone.as_slice() else {
        panic!("{resolved:?}")
    };
    let [blocked] = fillet_face.why.errors() else {
        panic!("{:?}", fillet_face.why)
    };
    assert_eq!(blocked.feature(), Some("f0000001"), "{blocked:?}");
    assert!(
        blocked.message().contains("a0000001"),
        "{}",
        blocked.message()
    );
    let [failed] = box_face.why.errors() else {
        panic!("{:?}", box_face.why)
    };
    assert_eq!(failed.code(), ErrorCode::KernelFailed);
    assert_eq!(failed.feature(), Some("a0000001"));
    let mended = plain(&features);
    let resolved = resolve(&mended, &["a0000001.box.Face1"]);
    assert_eq!(resolved.kept.len(), 1, "{resolved:?}");
    assert_eq!(resolved.kept[0].feature.as_str(), "f0000001");
}

#[test]
fn a_sketchs_elements_and_a_solid_are_gone_saying_why() {
    // A sketch's curve, its face, and a box's solid: no element a selection holds.
    let model = plain(&[a_box("k7f2qx3m", 0, 1)]);
    let solid = sole_reason(&model, "k7f2qx3m.box.Solid1");
    assert_eq!(solid.code(), ErrorCode::InvalidInput);
    assert_actionable(&solid, "k7f2qx3m.box.Solid1");
}

#[test]
fn ten_thousand_names_of_a_broken_fillet_each_carry_its_whole_failure() {
    // A fillet of a box naming 200 edges the box hasn't (Edge13 to Edge212): broken, its
    // first hundred references listed, each with candidates. A selection of 10,000 of its
    // faces (Face1 to Face10000, each a name of a feature that failed): each gone with the
    // fillet's failure. Measured: how many errors the result holds.
    let missing: Vec<String> = (13..213).map(|n| format!("bbbbbbbb.box.Edge{n}")).collect();
    let refs: Vec<&str> = missing.iter().map(String::as_str).collect();
    let features = [
        a_box("bbbbbbbb", 0, 1),
        fillet("ffffffff", "bbbbbbbb", &refs),
    ];
    let model = plain(&features);
    let Some(FeatureOutcome::Broken { errors, total }) =
        model.outcome(&FeatureId::new("ffffffff").unwrap())
    else {
        panic!("{:?}", model.features())
    };
    assert_eq!(*total, 200);
    let listed = errors.len();
    let names: Vec<String> = (1..=10_000)
        .map(|n| format!("ffffffff.fillet.Face{n}"))
        .collect();
    let resolved = model::promptly("10,000 names of a broken fillet", {
        let model = Arc::clone(&model);
        move || resolve(&model, &names)
    });
    let errors_held: usize = resolved.gone.iter().map(|g| g.why.errors().len()).sum();
    let candidates_held: usize = resolved
        .gone
        .iter()
        .flat_map(|g| g.why.errors())
        .map(|e| e.candidates().len())
        .sum();
    // Measured at the break-it run (2026-10-29): each name holds the fillet's hundred
    // listed errors, each with its candidates, a million errors and about 300 MB of text for
    // a 250 KB selection. Said here so a change to it is seen: the
    // design gives each name its own copy (selection.rs), and the selection has no bound.
    assert_eq!(errors_held, 10_000 * listed);
    assert!(candidates_held >= errors_held, "{candidates_held}");
    assert_eq!(resolved.gone.len(), 10_000);
}

// Any selection drawn from a pool of names: found and stable, found and unstable, aliases of
// one element, gone (consumed, failed, malformed, a solid), each any number of times, in any
// order.
const POOL: &[&str] = &[
    "aaaaaaaa.box.Face2",
    "rrrrrrrr.fuse.Edge(G1/2 aaaaaaaa.box.Face1, G1/2 aaaaaaaa.box.Face6)",
    "rrrrrrrr.fuse.Edge(G aaaaaaaa.box.Face1, G aaaaaaaa.box.Face3)",
    "cccccccc.box.Face1",
    "aaaaaaaa.box.Face1",
    "bbbbbbbb.box.Face1",
    "dddddddd.box.Face1",
    "eeeeeeee.box.Face2",
    "tttttttt.cut.Face(I eeeeeeee.box.Face2)",
    "dddddddd.box.Solid1",
    "nonsense",
    "zzzzzzzz.box.Face1",
    "aaaaaaaa.box.Face99",
    "",
];

fn rich_model() -> &'static Arc<Model> {
    static MODEL: std::sync::OnceLock<Arc<Model>> = std::sync::OnceLock::new();
    MODEL.get_or_init(rich)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn any_selection_reads_each_text_once_each_element_once_in_its_order(
        picks in proptest::collection::vec(0..POOL.len(), 0..40)
    ) {
        let model = rich_model();
        let names: Vec<&str> = picks.iter().map(|&i| POOL[i]).collect();
        let resolved = resolve(model, &names);
        // Each text at most once, kept or gone, at its first place, in order.
        let mut firsts: Vec<&str> = Vec::new();
        for name in &names {
            if !firsts.contains(name) {
                firsts.push(name);
            }
        }
        let gone: Vec<&str> = resolved.gone.iter().map(|g| g.name.as_str()).collect();
        let kept: Vec<String> = resolved.kept.iter().map(|s| s.name.to_string()).collect();
        let order = |list: &[&str]| -> Vec<usize> {
            list.iter().map(|n| firsts.iter().position(|f| f == n).unwrap()).collect()
        };
        let kept_refs: Vec<&str> = kept.iter().map(String::as_str).collect();
        let kept_order = order(&kept_refs);
        let gone_order = order(&gone);
        prop_assert!(kept_order.windows(2).all(|w| w[0] < w[1]));
        prop_assert!(gone_order.windows(2).all(|w| w[0] < w[1]));
        // Each element once.
        let elements: BTreeSet<(String, ElementRef)> = resolved
            .kept
            .iter()
            .map(|s| (s.feature.to_string(), s.element))
            .collect();
        prop_assert_eq!(elements.len(), resolved.kept.len());
        // Every first text is kept, gone, or another name of an element kept before it.
        for name in &firsts {
            let alone = resolve(model, &[*name]);
            if let Some(found) = alone.kept.first() {
                prop_assert!(
                    resolved.kept.iter().any(|s| s.feature == found.feature && s.element == found.element)
                );
            } else {
                prop_assert!(gone.contains(name));
                // Its reasons as when selected alone, but for its place.
                let place = names.iter().position(|n| n == name).unwrap();
                let at = resolved.gone.iter().find(|g| g.name == *name).unwrap();
                let codes = |p: &silico_core::command::Problems| -> Vec<ErrorCode> {
                    p.errors().iter().map(Error::code).collect()
                };
                prop_assert_eq!(codes(&at.why), codes(&alone.gone[0].why));
                for (here, there) in at.why.errors().iter().zip(alone.gone[0].why.errors()) {
                    prop_assert_eq!(
                        here.message().replacen(&format!("selection.{place}"), "selection.0", 1),
                        there.message().to_owned()
                    );
                    prop_assert_eq!(here.candidates(), there.candidates());
                }
            }
        }
        // Again, the same answer; and the same from a model rebuilt alike is checked below.
        prop_assert_eq!(resolve(model, &names), resolved);
    }
}

#[test]
fn two_rebuilds_of_one_document_resolve_a_selection_alike() {
    let names: Vec<&str> = POOL.iter().rev().chain(POOL.iter()).copied().collect();
    let first = resolve(&rich(), &names);
    let second = resolve(&rich(), &names);
    assert_eq!(first, second);
}

/// A fuse's history like the duplicating one but for its Edge2, kept: Edge1 alone comes
/// about from Face1 and Face6, so its name is stable.
fn single_fuse() -> History {
    let mut history = duplicating_fuse();
    let kept2 = ElementFate {
        kept: Some(2),
        ..ElementFate::default()
    };
    history.inputs[0].edges[1] = kept2;
    let edge = |index| Reported {
        element: element(ElementKind::Edge, index),
        position: 1,
    };
    history.inputs[0].faces[0].generated = vec![edge(1), edge(3)];
    history.inputs[0].faces[5].generated = vec![edge(1)];
    history.validate(2).unwrap();
    history
}

#[test]
fn a_stable_name_a_rebuild_makes_ambiguous_is_held_refused_never_stored() {
    // Before: the fuse's Edge1 alone comes about from Face1 and Face6: its name is stable,
    // stored as selected. After a rebuild in which two edges come about so (the second its
    // `~1`): the name finds the first, but a store refuses it (`reference_broken`), and no
    // name of the other edge is offered as if it were it; the `~1` name, never selected
    // before, finds the second, unstable too.
    let name = "rrrrrrrr.fuse.Edge(G aaaaaaaa.box.Face1, G aaaaaaaa.box.Face6)";
    let features = [
        a_box("aaaaaaaa", 0, 1),
        a_box("bbbbbbbb", 5, 1),
        operation("rrrrrrrr", "fuse", "aaaaaaaa", "bbbbbbbb"),
    ];
    let before = rebuilt(&features, forging(|_| Some(single_fuse())));
    let resolved = resolve(&before, &[name, &format!("{name}~1")]);
    assert_eq!(resolved.kept.len(), 1, "{resolved:?}");
    assert_eq!(resolved.kept[0].element, element(ElementKind::Edge, 1));
    assert_eq!(
        resolved.kept[0]
            .stored
            .as_ref()
            .map(ToString::to_string)
            .ok(),
        Some(name.to_owned())
    );
    assert_eq!(resolved.gone.len(), 1);
    assert_eq!(
        resolved.gone[0].why.errors()[0].code(),
        ErrorCode::ReferenceBroken
    );
    let after = rebuilt(&features, forging(|_| Some(duplicating_fuse())));
    // The old name, now that two edges come about alike, finds neither (their names say
    // their places, `G1/2`): gone, never kept as one of them.
    let resolved = resolve(&after, &[name]);
    assert!(resolved.kept.is_empty(), "{resolved:?}");
    let [gone] = resolved.gone.as_slice() else {
        panic!("{resolved:?}")
    };
    assert_eq!(gone.why.errors()[0].code(), ErrorCode::ReferenceBroken);
    // The two new names: each found, its own edge, each refused a store.
    let ranked = "rrrrrrrr.fuse.Edge(G1/2 aaaaaaaa.box.Face1, G1/2 aaaaaaaa.box.Face6)";
    let tilde = format!("{ranked}~1");
    let resolved = resolve(&after, &[ranked, tilde.as_str()]);
    assert!(resolved.gone.is_empty(), "{resolved:?}");
    let [first, second] = resolved.kept.as_slice() else {
        panic!("{resolved:?}")
    };
    assert_eq!(first.element, element(ElementKind::Edge, 1));
    assert_eq!(second.element, element(ElementKind::Edge, 2));
    for selected in [first, second] {
        let Err(refusal) = &selected.stored else {
            panic!("{selected:?}")
        };
        assert_eq!(refusal.code(), ErrorCode::ReferenceBroken);
        for candidate in refusal.candidates() {
            assert!(
                !candidate.starts_with(ranked),
                "{candidate} offered for an ambiguous edge"
            );
        }
    }
}

#[test]
fn a_model_read_by_many_threads_at_once_resolves_alike_on_each() {
    // The model's live bodies are worked out once, the first time a name needs them: eight
    // threads asking at once, on a fresh model, each get the answer one thread alone gets.
    let names: Vec<String> = POOL.iter().map(|s| (*s).to_owned()).collect();
    let alone = resolve(&rich(), &names);
    for _ in 0..5 {
        let model = rich();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let model = Arc::clone(&model);
                let names = names.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    resolve(&model, &names)
                })
            })
            .collect();
        for thread in threads {
            assert_eq!(thread.join().unwrap(), alone);
        }
    }
}

#[test]
fn a_name_of_a_removed_feature_is_gone_without_a_place_in_the_timeline_it_has_none_of() {
    // A selection has no place in the timeline: a name of a feature the document no longer
    // has is gone because there's no such feature, not because none is "before this one".
    let model = plain(&[a_box("a0000001", 0, 1)]);
    let error = sole_reason(&model, "b0000001.box.Face2");
    assert_eq!(error.code(), ErrorCode::ReferenceBroken);
    assert!(error.message().contains("b0000001"), "{}", error.message());
    // BREAKS: the parser's words for a document's reference ("no feature b0000001 before
    // this one") are passed on to a selection, which has no "this one".
    assert!(
        !error.message().contains("before this one"),
        "{}",
        error.message()
    );
}
