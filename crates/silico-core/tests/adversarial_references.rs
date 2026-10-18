//! break-it, M2.8: references by name. Hostile names for the parser, hostile fillets and
//! chamfers for the reader, writer, and gate, and hostile timelines, caches, and edits for
//! resolving a fillet's edges during recompute, on the fake kernel. Expected values come from
//! the grammar (docs/ports/naming.md), the fake's hand-countable numbering (its booleans keep
//! every element of both inputs, the base's first), and invariants: a name reads back as the
//! name it writes, two names are never written alike, a reference never re-points, and a
//! recompute's outcome is the same whatever order its steps happen to run in.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use proptest::prelude::*;
use silico_core::document::{Checked, Document, FeatureId, read, write};
use silico_core::fake::FakeKernel;
use silico_core::kernel::{Blob, ElementKind, ElementRef, Op, ShapeData};
use silico_core::naming::{Maker, Name, OpCode, Place, PrimitiveKind, Raw, Relation, Source};
use silico_core::recompute::{FeatureOutcome, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};

#[path = "common/model.rs"]
mod model;
#[path = "common/references.rs"]
mod references;
#[path = "common/within.rs"]
mod within;

use model::{checked, described, id, made, published};
use references::{document, dressings, engine, fillet};
use within::within;

fn n(k: u32) -> NonZeroU32 {
    NonZeroU32::new(k).unwrap()
}

fn makers(feature: &FeatureId) -> Option<Maker> {
    match feature.as_str() {
        "aaaaaaaa" | "bbbbbbbb" => Some(Maker::Itself(PrimitiveKind::Box)),
        "cccccccc" => Some(Maker::By(OpCode::Cut)),
        "dddddddd" => Some(Maker::By(OpCode::Fuse)),
        "ffffffff" => Some(Maker::By(OpCode::Fillet)),
        _ => None,
    }
}

fn parse(text: &str) -> Result<Name, silico_core::naming::NameError> {
    Name::parse(text, &makers)
}

// ---------------------------------------------------------------------------------------
// The parser
// ---------------------------------------------------------------------------------------

#[test]
fn name_numbers_at_the_edges_of_u32() {
    // Counts are 1 to 2^32 - 1, with no leading zero and no sign.
    let max = parse("aaaaaaaa.box.Edge4294967295").unwrap();
    assert_eq!(
        max,
        Name::Element {
            feature: id("aaaaaaaa"),
            primitive: Some(PrimitiveKind::Box),
            element: ElementRef {
                kind: ElementKind::Edge,
                index: u32::MAX
            }
        }
    );
    for (text, column) in [
        ("aaaaaaaa.box.Edge4294967296", 18),
        ("aaaaaaaa.box.Edge0", 18),
        ("aaaaaaaa.box.Edge00", 18),
        ("aaaaaaaa.box.Edge01", 18),
        ("aaaaaaaa.box.Edge+1", 18),
        ("aaaaaaaa.box.Edge-1", 18),
        ("aaaaaaaa.box.Edge", 18),
        ("aaaaaaaa.box.Edge1~0", 20),
        ("aaaaaaaa.box.Edge1~01", 20),
        ("aaaaaaaa.box.Edge1~", 20),
        ("aaaaaaaa.box.Edge1~4294967296", 20),
    ] {
        let error = parse(text).unwrap_err();
        assert_eq!(error.column, column, "{text}: {error}");
    }
    // A thousand digits: refused as past 2^32 - 1, at the number, in linear time.
    let long = format!("aaaaaaaa.box.Edge{}", "9".repeat(1_000_000));
    let error = within(Duration::from_secs(2), move || parse(&long).unwrap_err());
    assert_eq!(error.column, 18);
}

#[test]
fn name_places_and_unnamed_counts_at_their_edges() {
    let ok = [
        "cccccccc.Edge(G2/2 aaaaaaaa.box.Face1)",
        "cccccccc.Edge(M1/4294967295 aaaaaaaa.box.Face1)",
        "cccccccc.Edge(M4294967295/4294967295 aaaaaaaa.box.Face1)",
        "cccccccc.Edge(U aaaaaaaa.box.Face1)",
        "cccccccc.Edge(U aaaaaaaa.box.Face1, aaaaaaaa.box.Face2)",
        "cccccccc.Edge(U2/2 aaaaaaaa.box.Face1)",
        "cccccccc.Edge(U4294967295/4294967295 aaaaaaaa.box.Face1, aaaaaaaa.box.Face2)",
        "aaaaaaaa.box.Edge1~1~1",
        "cccccccc.Edge(G aaaaaaaa.box.Face1~2)~3",
    ];
    for text in ok {
        let name = parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(name.to_string(), text);
    }
    let refused = [
        "cccccccc.Edge(G1/1 aaaaaaaa.box.Face1)",
        "cccccccc.Edge(G0/2 aaaaaaaa.box.Face1)",
        "cccccccc.Edge(G3/2 aaaaaaaa.box.Face1)",
        "cccccccc.Edge(G1/02 aaaaaaaa.box.Face1)",
        "cccccccc.Edge(G1/ aaaaaaaa.box.Face1)",
        "cccccccc.Edge(G/2 aaaaaaaa.box.Face1)",
        "cccccccc.Edge(G1 aaaaaaaa.box.Face1)",
        "cccccccc.Edge(U1 aaaaaaaa.box.Face1)",
        "cccccccc.Edge(U0 aaaaaaaa.box.Face1)",
        "cccccccc.Edge(U01 aaaaaaaa.box.Face1)",
        "cccccccc.Edge(U2aaaaaaaa.Face1)",
        "cccccccc.Edge(Uaaaaaaaa.Face1)",
        "cccccccc.Edge(L)",
        "cccccccc.Edge(L )",
        "cccccccc.Edge()",
        "cccccccc.Edge(",
        "cccccccc.Edge(G aaaaaaaa.box.Face1",
        "cccccccc.Edge(G aaaaaaaa.box.Face1))",
        "cccccccc.Edge(G aaaaaaaa.box.Face1,G aaaaaaaa.box.Face2)",
        "cccccccc.Edge(G aaaaaaaa.box.Face1,  G aaaaaaaa.box.Face2)",
        "cccccccc.Edge(G aaaaaaaa.box.Face1, )",
        "cccccccc.Edge(G  aaaaaaaa.box.Face1)",
        "cccccccc.Edge(G\taaaaaaaa.Face1)",
        "cccccccc.Edge (G aaaaaaaa.box.Face1)",
        "cccccccc.Edge(g aaaaaaaa.box.Face1)",
        "cccccccc.Edge(X aaaaaaaa.box.Face1)",
        // A primitive names its elements by number only.
        "aaaaaaaa.box.Edge(G bbbbbbbb.box.Face1)",
        "aaaaaaaa.box.Edge(U bbbbbbbb.box.Face1)",
        "aaaaaaaa.box.Edge(L bbbbbbbb.box.Vertex1)",
    ];
    for text in refused {
        assert!(parse(text).is_err(), "{text} read as {:?}", parse(text));
    }
}

#[test]
fn name_text_that_is_almost_a_name() {
    for text in [
        "",
        " aaaaaaaa.box.Edge1",
        "aaaaaaaa.box.Edge1 ",
        "aaaaaaaa.box.Edge1\n",
        "aaaaaaaa.box.Edge1\r\n",
        "\u{feff}aaaaaaaa.box.Edge1",
        "aaaaaaaa.box.Edge1\u{0}",
        "AAAAAAAA.Edge1",
        "aaaaaaaa.box.edge1",
        "aaaaaaaa.box.EDGE1",
        "aaaaaaaa.box.Edges1",
        "aaaaaaaa..Edge1",
        "aaaaaaaa,Edge1",
        "aaaaaaa.Edge1",
        "aaaaaaaaa.Edge1",
        "aaaaaaai.Edge1",
        "aaaaaaau.Edge1",
        "aaaaaaaa.box.Shell1",
        "aaaaaaaa.box.Wire1",
        "aaaaaaaa.box.Compound1",
        "aaaaaaaa",
        "aaaaaaaa.",
        // A primitive's type: lowercase, once, then a dot and the element.
        "aaaaaaaa.Edge1",
        "aaaaaaaa.Box.Edge1",
        "aaaaaaaa.BOX.Edge1",
        "aaaaaaaa.boxes.Edge1",
        "aaaaaaaa.box.box.Edge1",
        "aaaaaaaa.boxEdge1",
        "aaaaaaaa.box",
        "aaaaaaaa.box.",
        "aaaaaaaa.prism.Edge1",
        "cccccccc.box.Edge1",
        "zzzzzzzz.Edge1",
        // Fullwidth digits, Arabic-Indic digits: not ASCII.
        "aaaaaaaa.box.Edge\u{ff11}",
        "aaaaaaaa.box.Edge\u{661}",
    ] {
        assert!(parse(text).is_err(), "{text:?} read as {:?}", parse(text));
    }
}

#[test]
fn name_with_a_multibyte_character_cut_by_the_ids_eight_bytes() {
    // The ID is read as 8 bytes: `é` is 2, so the slice ends inside a character. Refused at
    // the ID, never a panic, with a column that counts characters (everything before it is
    // ASCII).
    for (text, column) in [
        ("aaaaaaaé.Edge1", 1),
        ("aaaaaaé.Edge1", 1),
        ("é", 1),
        ("😀😀.Edge1", 1),
        ("cccccccc.Edge(G aaaaaaaé.Face1)", 17),
        ("aaaaaaaa.box.Edge1~é", 20),
        ("aaaaaaaa.box.Ed\u{301}ge1", 14),
    ] {
        let error = parse(text).unwrap_err();
        assert_eq!(error.column, column, "{text}: {error}");
        let shown: String = text.chars().take(error.column - 1).collect();
        assert!(shown.is_ascii(), "{text}: column {}", error.column);
    }
}

/// A name each of whose levels is a `~n` chain as long as that level's depth leaves room
/// for, to the most names written out: 17 levels of text, but each chain on top of the
/// last, so about 4,096 deep.
fn staircase() -> String {
    let (mut tildes, mut weight, mut depth) = (Vec::new(), 0usize, 0usize);
    while weight < 4096 {
        let t = (255 - depth).min(4096 - weight - 1);
        tildes.push(t);
        weight += t + 1;
        depth += 1;
    }
    let last = tildes.last().copied().unwrap_or(0);
    let mut text = format!("aaaaaaaa.box.Face1{}", "~1".repeat(last));
    for &t in tildes.iter().rev().skip(1) {
        text = format!("cccccccc.Face(M {text}){}", "~1".repeat(t));
    }
    text
}

#[test]
fn name_whose_tilde_chains_stack_up_past_the_deepest_is_refused_not_walked() {
    // Found by the robustness review: each level's `~n` chain was bounded by that level's
    // depth alone, so chains on top of chains read as a name about 4,096 deep, which
    // writing it out or comparing it on a recompute thread's stack overflowed. The depth
    // that counts is the name's whole, as the builder counts it: refused at 256.
    let text = staircase();
    let refused = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || parse(&text).map(|name| name.to_string()))
        .unwrap()
        .join()
        .unwrap();
    let error = refused.unwrap_err();
    assert!(error.why.contains("nested more than 256 deep"), "{error}");
    // At the bound: a chain inside a level and a chain on it count together. The face
    // and 127 `~1`s inside, the cut's level, and 127 on it: 256 deep, read; one more, not.
    let inside = format!("aaaaaaaa.box.Face1{}", "~1".repeat(127));
    let at = format!("cccccccc.Face(M {inside}){}", "~1".repeat(127));
    assert_eq!(parse(&at).unwrap().to_string(), at);
    let past = format!("{at}~1");
    assert!(
        parse(&past)
            .unwrap_err()
            .why
            .contains("nested more than 256 deep")
    );
    // And every level counts, not only a `~n`'s (found by the round-3 review): levels
    // without one on top of a chain, 301 or 511 deep, are refused too.
    for (chain, levels) in [(200, 100), (255, 255)] {
        let mut text = format!("aaaaaaaa.box.Face1{}", "~1".repeat(chain));
        for _ in 0..levels {
            text = format!("cccccccc.Face(M {text})");
        }
        let error = parse(&text).unwrap_err();
        assert!(error.why.contains("nested more than 256 deep"), "{error}");
    }
}

#[test]
fn name_wide_and_deep_on_a_recompute_threads_stack() {
    // The widest name the grammar takes at every level (5 sources) over 255 levels of
    // nesting on the path down: read on a 2 MB thread (a recompute thread's) in a debug
    // build, and read back as written.
    let mut text = "aaaaaaaa.box.Face1".to_owned();
    for level in 0..255 {
        let others: Vec<String> = (2..=5)
            .map(|i| format!("G aaaaaaaa.box.Face{}", level * 4 + i))
            .collect();
        text = format!("cccccccc.Face(M {text}, {})", others.join(", "));
    }
    let name = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let name = parse(&text).unwrap();
            assert_eq!(name.to_string(), text);
            // Compare and drop it on this stack too.
            assert_eq!(parse(&text).unwrap(), name);
            name.to_string().len()
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(name > 255 * 50);
}

/// A name for the round-trip property: built by hand, any shape the grammar allows, its
/// features' makers as `makers` says, with raw positions and `Modified` wrappers that the
/// text leaves out.
fn arb_name() -> impl Strategy<Value = Name> {
    let kind = prop_oneof![
        Just(ElementKind::Vertex),
        Just(ElementKind::Edge),
        Just(ElementKind::Face),
        Just(ElementKind::Solid),
    ];
    let count = prop_oneof![
        Just(1u32),
        Just(2),
        Just(9),
        Just(10),
        Just(u32::MAX),
        1u32..1000
    ];
    let leaf = (
        prop_oneof![
            Just("aaaaaaaa"),
            Just("bbbbbbbb"),
            Just("cccccccc"),
            Just("dddddddd"),
            Just("ffffffff")
        ],
        kind.clone(),
        count.clone(),
    )
        .prop_map(|(f, kind, index)| Name::Element {
            feature: id(f),
            primitive: matches!(f, "aaaaaaaa" | "bbbbbbbb").then_some(PrimitiveKind::Box),
            element: ElementRef { kind, index },
        });
    let op_feature = prop_oneof![
        Just(("cccccccc", OpCode::Cut)),
        Just(("dddddddd", OpCode::Fuse)),
        Just(("ffffffff", OpCode::Fillet)),
    ];
    leaf.prop_recursive(5, 64, 5, move |inner| {
        let place = prop_oneof![
            Just(None),
            (2u32..6)
                .prop_flat_map(|size| (1..=size, Just(size)))
                .prop_map(|(rank, size)| Some(Place {
                    rank: n(rank),
                    size: n(size)
                })),
        ];
        let source = (
            prop_oneof![Just(Relation::Modified), Just(Relation::Generated)],
            place.clone(),
            1u32..50,
            inner.clone(),
        )
            .prop_map(|(relation, place, position, name)| Source {
                relation,
                place,
                position: Raw(n(position)),
                name: Arc::new(name),
            });
        prop_oneof![
            (
                op_feature.clone(),
                kind.clone(),
                source.clone(),
                proptest::collection::vec(source, 0..=4)
            )
                .prop_map(|((f, operation), kind, first, more)| Name::Derived {
                    feature: id(f),
                    operation,
                    kind,
                    first,
                    more,
                }),
            (
                op_feature.clone(),
                kind.clone(),
                inner.clone(),
                proptest::collection::vec(inner.clone(), 0..=4),
                place.clone(),
                count.clone()
            )
                .prop_map(|((f, operation), kind, upper, more, place, nth)| {
                    Name::Upper {
                        feature: id(f),
                        operation,
                        kind,
                        upper: Arc::new(upper),
                        more: more.into_iter().map(Arc::new).collect(),
                        place,
                        nth: Raw(n(nth)),
                    }
                }),
            (
                op_feature.clone(),
                kind.clone(),
                inner.clone(),
                proptest::collection::vec(inner.clone(), 0..=4)
            )
                .prop_map(|((f, operation), kind, first, more)| Name::Lower {
                    feature: id(f),
                    operation,
                    kind,
                    first: Arc::new(first),
                    more: more.into_iter().map(Arc::new).collect(),
                }),
            (inner.clone(), count.clone()).prop_map(|(name, ordinal)| Name::Duplicate {
                name: Arc::new(name),
                ordinal: n(ordinal),
            }),
            (
                inner,
                prop_oneof![Just(OpCode::Cut), Just(OpCode::Fuse), Just(OpCode::Chamfer)]
            )
                .prop_map(|(name, op)| Name::Modified {
                    name: Arc::new(name),
                    last: silico_core::naming::LastOp(op),
                }),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, failure_persistence: None, ..ProptestConfig::default() })]

    /// Every name the grammar allows reads back from its text as itself (by identity), and
    /// writes the same text again.
    #[test]
    fn name_any_constructed_name_reads_back_as_itself(name in arb_name()) {
        let text = name.to_string();
        let read = parse(&text);
        prop_assert!(read.is_ok(), "{} doesn't read: {:?}", text, read);
        let read = read.unwrap();
        prop_assert_eq!(&read, &name, "{}", text);
        prop_assert_eq!(read.to_string(), text);
        prop_assert_eq!(read.kind(), name.kind());
    }

    /// Two names are never written alike unless they're the same name: the text is the
    /// name, so a reference by text can't find another element's name.
    #[test]
    fn name_two_names_written_alike_are_one_name(a in arb_name(), b in arb_name()) {
        if a.to_string() == b.to_string() {
            prop_assert_eq!(&a, &b);
        }
        if a == b {
            prop_assert_eq!(a.to_string(), b.to_string());
        }
    }
}

// ---------------------------------------------------------------------------------------
// The reader, the writer, and the gate
// ---------------------------------------------------------------------------------------

/// The gate's problems with `text`: (parameter, message).
fn refused(text: &str) -> Vec<(String, String)> {
    let invalid = Checked::new(read(text.as_bytes()).unwrap()).unwrap_err();
    invalid
        .errors()
        .iter()
        .map(|e| {
            (
                e.parameter().unwrap_or_default().to_owned(),
                e.message().to_owned(),
            )
        })
        .collect()
}

/// What reading `text` refuses: its error's parameter and message.
fn unread(text: &str) -> (Option<String>, String, ErrorCode) {
    let error = read(text.as_bytes()).unwrap_err();
    (
        error.parameter().map(str::to_owned),
        error.message().to_owned(),
        error.code(),
    )
}

#[test]
fn fillet_edges_of_every_wrong_json_kind_are_refused_by_the_reader() {
    for (edges, parameter) in [
        (r#""k7f2qx3m.box.Edge1""#, "edges"),
        ("null", "edges"),
        ("{}", "edges"),
        ("1", "edges"),
        ("[1]", "edges.0"),
        ("[null]", "edges.0"),
        (r#"[["k7f2qx3m.box.Edge1"]]"#, "edges.0"),
        (r#"[{"name": "k7f2qx3m.box.Edge1"}]"#, "edges.0"),
        (r#"["k7f2qx3m.box.Edge1", ""]"#, "edges.1"),
    ] {
        let feature = format!(
            r#"{{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "m3tq8v0z", "edges": {edges}, "radius": 1}}"#
        );
        let (got, message, code) = unread(&document(&[&feature]));
        assert_eq!(code, ErrorCode::InvalidInput, "{edges}");
        assert_eq!(got.as_deref(), Some(parameter), "{edges}: {message}");
    }
}

#[test]
fn chamfer_size_of_every_wrong_shape_is_refused_by_the_reader() {
    for size in [
        "1",
        "null",
        "[]",
        "{}",
        r#"{"distance": 1, "other": 2}"#,
        r#"{"distance": 1, "angle": 45}"#,
        r#"{"distance": "1"}"#,
        r#"{"distance": [1]}"#,
    ] {
        let feature = format!(
            r#"{{"id": "c0000001", "type": "chamfer", "v": 1, "name": "Bevel", "base": "m3tq8v0z", "edges": ["k7f2qx3m.box.Edge1"], "size": {size}}}"#
        );
        let (parameter, message, code) = unread(&document(&[&feature]));
        assert_eq!(code, ErrorCode::InvalidInput, "{size}");
        // BREAKS: decode.rs `length` names a nested length by its own key, not its path:
        // `{"distance": "1"}` and `{"distance": [1]}` are refused as `distance`, where the
        // gate names the same value `size.distance`, and the schema's path is size.distance.
        assert!(
            parameter.as_deref().is_some_and(|p| p.starts_with("size")),
            "{size}: {parameter:?} {message}"
        );
    }
    // A chamfer with a radius, a fillet with a size: unknown keys.
    let chamfer_with_radius = r#"{"id": "c0000001", "type": "chamfer", "v": 1, "name": "Bevel", "base": "m3tq8v0z", "edges": ["k7f2qx3m.box.Edge1"], "size": {"distance": 1}, "radius": 1}"#;
    assert_eq!(
        unread(&document(&[chamfer_with_radius])).2,
        ErrorCode::InvalidInput
    );
    let fillet_with_size = r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "m3tq8v0z", "edges": ["k7f2qx3m.box.Edge1"], "radius": 1, "size": {"distance": 1}}"#;
    assert_eq!(
        unread(&document(&[fillet_with_size])).2,
        ErrorCode::InvalidInput
    );
}

#[test]
fn fillet_with_every_value_bad_at_once_is_refused_for_each() {
    // A checker that promises every problem: the radius, an empty name list's sibling (a
    // face where an edge is wanted), a name of nothing, and a repeat, all at once.
    let text = document(&[&fillet(
        "f0000001",
        "m3tq8v0z",
        &[
            "k7f2qx3m.box.Face1",
            "zzzzzzzz.Edge1",
            "k7f2qx3m.box.Edge1",
            "k7f2qx3m.box.Edge1",
            "k7f2qx3m.box.Edge0",
        ],
        "-1",
    )]);
    let said = refused(&text);
    let parameters: Vec<&str> = said.iter().map(|(p, _)| p.as_str()).collect();
    for wanted in ["radius", "edges.0", "edges.1", "edges.3", "edges.4"] {
        assert!(parameters.contains(&wanted), "{wanted}: {said:?}");
    }
}

#[test]
fn fillet_names_quoted_in_the_gates_errors_are_shown_short_and_escaped() {
    // A megabyte of name, and a name of terminal escapes and newlines: the gate's message
    // shows a few characters of each, escaped, never the whole.
    let huge = format!("k7f2qx3m.box.Edge1{}", "x".repeat(1 << 20));
    let escapes = "k7f2qx3m.box.Edge1\u{1b}[2J\n\u{2028}";
    let text = document(&[&fillet("f0000001", "m3tq8v0z", &[&huge, escapes], "1")]);
    let invalid = Checked::new(read(text.as_bytes()).unwrap()).unwrap_err();
    for error in invalid.errors() {
        let all = format!(
            "{}{}{:?}",
            error.message(),
            error.suggestion().unwrap_or_default(),
            error.elements()
        );
        assert!(all.len() < 4096, "{} bytes: {}", all.len(), &all[..200]);
        assert!(!all.contains('\u{1b}'), "{all}");
        assert!(!all.contains('\n'), "{all}");
    }
}

#[test]
fn fillet_with_forty_thousand_names_is_checked_promptly() {
    // 40,000 names, each of an edge the block doesn't have (a 1 MB document, well inside
    // the 16 MB a document may be): the gate checks each, and each against the others for
    // a repeat: linear work, a few milliseconds.
    let names: Vec<String> = (1..=40_000)
        .map(|i| format!("k7f2qx3m.box.Edge{i}"))
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let text = document(&[&fillet("f0000001", "m3tq8v0z", &refs, "1")]);
    let document = read(text.as_bytes()).unwrap();
    // BREAKS: the gate's check for a name listed twice (document/validate.rs, `edges()[..i]
    // .iter().position`) is quadratic: in a debug build 10,000 names take 0.7 s, 20,000
    // 3.6 s, 40,000 15.5 s (8e8 string comparisons); the 600,000 names a 16 MB document
    // holds would take about an hour, opening a file.
    within(Duration::from_secs(2), move || {
        Checked::new(document).is_ok()
    });
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, failure_persistence: None, ..ProptestConfig::default() })]

    /// `read(write(d)) == d` for a fillet whose edge names are any non-empty strings the
    /// in-memory type holds (the gate's to judge; the file must still hold them exactly).
    #[test]
    fn fillet_names_of_any_text_are_written_and_read_back_exactly(
        names in proptest::collection::vec(
            prop_oneof![
                "\\PC{1,12}",
                Just("\u{0}".to_owned()),
                Just("\"".to_owned()),
                Just("\\".to_owned()),
                Just("\u{2028}\u{2029}".to_owned()),
                Just("\u{feff}".to_owned()),
                Just("\u{7f}\u{1b}".to_owned()),
                Just("\u{10ffff}".to_owned()),
                Just("e\u{301}".to_owned()),
                Just(" ".to_owned()),
            ],
            1..6,
        )
    ) {
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let text = document(&[&fillet("f0000001", "m3tq8v0z", &names, "1")]);
        let doc: Document = read(text.as_bytes()).unwrap();
        let written = write(&doc).unwrap();
        let again = read(written.as_bytes()).unwrap();
        prop_assert_eq!(&again, &doc);
        prop_assert_eq!(write(&again).unwrap(), written);
    }
}

// ---------------------------------------------------------------------------------------
// Resolution during recompute
// ---------------------------------------------------------------------------------------

/// `text` built once with a cache in a directory of its own (`case`), the cache then left
/// with only the entries `keep` keeps (by their shapes' bytes: `kept` of them), and `text`
/// built again from it, with each primitive's rebuild held 300 ms, as a busy kernel would
/// hold it, so the steps the cache let start start first. The first build, and the second,
/// which made the block again.
fn rebuilt_from_a_partial_cache(
    case: &str,
    text: &str,
    keep: impl Fn(&[u8]) -> bool,
    kept: usize,
) -> (
    Arc<silico_core::recompute::Model>,
    Arc<silico_core::recompute::Model>,
) {
    let dir = std::env::temp_dir().join(format!(
        "silico-adversarial-references-{case}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let secret = dir.join("secret");
    let cache = || silico_core::cache::Cache::with_secret(dir.join("cache"), &secret);
    let first = Recompute::with_cache(FakeKernel::new(2).unwrap(), cache());
    let built = published(&first, checked(text));
    assert!(built.errors().next().is_none());
    drop(first);
    let mut left = 0;
    let files: Vec<std::path::PathBuf> = std::fs::read_dir(dir.join("cache"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .flat_map(|e| std::fs::read_dir(e.path()).unwrap().flatten())
        .map(|e| e.path())
        .collect();
    for path in files {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let shape = b"silico fake shape 1";
        if !bytes.windows(shape.len()).any(|w| w == shape) {
            continue;
        }
        if keep(&bytes) {
            left += 1;
        } else {
            std::fs::remove_file(&path).unwrap();
        }
    }
    assert_eq!(left, kept, "the entries kept");
    let kernel = FakeKernel::new(2).unwrap();
    let hold = kernel.hold(|op| matches!(op, Op::MakePrimitive(_)));
    let second = Recompute::with_cache(kernel, cache());
    let pending = second.start(checked(text));
    std::thread::sleep(Duration::from_millis(300));
    hold.release();
    let model = match pending.wait() {
        Ok(RunOutcome::Published(model)) => model,
        other => panic!("{other:?}"),
    };
    assert!(
        made(&second.kernel().calls()).contains(&"MakeBox"),
        "{:?}",
        second.kernel().calls()
    );
    let _ = std::fs::remove_dir_all(&dir);
    (built, model)
}

/// How many times `needle` is in `bytes`.
fn count(bytes: &[u8], needle: &[u8]) -> usize {
    bytes.windows(needle.len()).filter(|w| *w == needle).count()
}

#[test]
fn fillet_rebuilt_from_a_cache_missing_its_bases_inputs_still_finds_its_edges() {
    // A cache holding the hole's result but not the block's (an entry lost: a crash between
    // writes, a disk that filled, a user clearing files). The hole comes from the cache, so
    // the fillet's step is ready at once, while the block is rebuilt; the fillet's names
    // are of the hole's elements, which are named from the block's and the pin's.
    let text = document(&[&fillet(
        "f0000001",
        "m3tq8v0z",
        &["a91cw0de.cylinder.Edge2"],
        "0.5",
    )]);
    // Only the hole's entry: the one whose shape is a cut and not a fillet.
    let (built, model) = rebuilt_from_a_partial_cache(
        "one-level",
        &text,
        |bytes| count(bytes, b"\ncut ") == 1 && count(bytes, b"\nfillet ") == 0,
        1,
    );
    let errors: Vec<(ErrorCode, String)> = model
        .errors()
        .map(|e| (e.code(), e.message().to_owned()))
        .collect();
    // BREAKS: the fillet's step runs as soon as the hole is in hand, and names the hole's
    // elements only from the solids already built: the block isn't, so `Plan::map_of`
    // gives up and the fillet fails with an internal error ("a fillet's base was built,
    // but its elements couldn't be named"). Without the hold it fails the same way (3 runs
    // of 3): the fillet's step is ready at the start, beside the block's.
    assert!(errors.is_empty(), "{errors:?}");
    // The same fillet as built with the whole cache's help.
    assert_eq!(described(&model, "f0000001"), described(&built, "f0000001"));
}

#[test]
fn fillet_of_a_fillet_rebuilt_from_a_cache_missing_the_block_still_finds_its_edges() {
    // Found by the tests review: a fillet waits for its base's whole lineage, not only the
    // base and its inputs. A fillet of a fillet of the hole, from a cache holding the hole
    // and the first fillet but not the block: the second fillet's names are of the first's
    // elements, named from the hole's, named from the block's, which is still being built.
    let text = document(&[
        &fillet("f0000001", "m3tq8v0z", &["a91cw0de.cylinder.Edge2"], "0.5"),
        &fillet("f0000002", "f0000001", &["k7f2qx3m.box.Edge1"], "0.25"),
    ]);
    let (built, model) = rebuilt_from_a_partial_cache(
        "two-levels",
        &text,
        |bytes| count(bytes, b"\ncut ") == 1 && count(bytes, b"\nfillet ") <= 1,
        2,
    );
    let errors: Vec<(ErrorCode, String)> = model
        .errors()
        .map(|e| (e.code(), e.message().to_owned()))
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(described(&model, "f0000002"), described(&built, "f0000002"));
}

#[test]
fn fillet_name_nested_to_the_limit_is_resolved_on_a_recompute_thread() {
    // A name 256 levels deep, the deepest the grammar reads, of features the document has:
    // read again and looked up on a recompute thread, it finds nothing and fails the
    // fillet, never the thread's stack.
    let deep = format!(
        "{}k7f2qx3m.box.Face1{}",
        "m3tq8v0z.Face(M ".repeat(255),
        ")".repeat(255)
    );
    let deep_edge = deep.replacen("m3tq8v0z.Face(", "m3tq8v0z.Edge(", 1);
    let engine = engine();
    let text = document(&[&fillet("f0000001", "m3tq8v0z", &[&deep_edge], "0.5")]);
    let model = published(&engine, checked(&text));
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("f0000001")) else {
        panic!("{:?}", model.outcome(&id("f0000001")));
    };
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
    assert_eq!(errors[0].parameter(), Some("edges.0"));
    assert!(dressings(engine.kernel()).is_empty());
}

#[test]
fn fillet_reference_broken_error_quoting_a_long_name_stays_small() {
    // The widest name the grammar takes (the builder's bound, 4,096 names written out):
    // five sources at each of 5 levels, 3,906 names, about 60 KB of text. Broken, its error
    // says so, and what it carries back to the agent is the reference cut short, not a
    // second copy of it. (Wider is refused when read: a_name_of_more_names_than_any_map_holds.)
    fn wide(level: u32) -> String {
        if level == 0 {
            return "k7f2qx3m.box.Face1".to_owned();
        }
        let inner = wide(level - 1);
        let sources: Vec<String> = (0..5).map(|_| format!("M {inner}")).collect();
        format!("m3tq8v0z.Face({})", sources.join(", "))
    }
    let name = wide(5).replacen("m3tq8v0z.Face(", "m3tq8v0z.Edge(", 1);
    assert!(name.len() > 50_000, "{}", name.len());
    let engine = engine();
    let text = document(&[&fillet("f0000001", "m3tq8v0z", &[&name], "0.5")]);
    let model = published(&engine, checked(&text));
    let errors: Vec<&Error> = model.errors().collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
    assert!(errors[0].message().len() < 4096);
    let carried = serde_json::to_string(errors[0]).unwrap().len();
    // Found by break-it: the whole reference was in the error's elements, a copy of the
    // name per broken name.
    assert!(carried < 4096, "the error is {carried} bytes");
}

#[test]
fn fillet_with_many_broken_names_says_each_without_running_away() {
    // 20,000 broken names, each of an edge the block doesn't have (Edge13 on): every one is
    // said (N6), so an agent fixes them all at once. Measure the cost, and that each is
    // right. (A `~n` name the gate refuses, so it never gets this far.)
    let names: Vec<String> = (13..13 + 20_000)
        .map(|i| format!("k7f2qx3m.box.Edge{i}"))
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let text = document(&[&fillet("f0000001", "m3tq8v0z", &refs, "0.5")]);
    let document = checked(&text);
    let start = Instant::now();
    let engine = engine();
    let model = published(&engine, document);
    let spent = start.elapsed();
    // The first hundred listed, each in place, and how many there are, as the gate lists
    // its problems and counts them (found by the review: 500,000 errors from an 11 MB
    // document, 231 MB of JSON; and by the rebase's review: a summary error counted as one).
    let errors: Vec<&Error> = model.errors().collect();
    assert_eq!(errors.len(), 100);
    for (i, error) in errors.iter().enumerate() {
        assert_eq!(error.code(), ErrorCode::ReferenceBroken);
        assert_eq!(error.parameter(), Some(format!("edges.{i}").as_str()));
    }
    let Some(FeatureOutcome::Broken { total, .. }) = model.outcome(&id("f0000001")) else {
        panic!("{:?}", model.outcome(&id("f0000001")));
    };
    assert_eq!(*total, 20_000);
    assert_eq!(model.error_count(), 20_000);
    assert!(spent < Duration::from_secs(20), "{spent:?}");
}

#[test]
fn fillet_of_an_edge_named_by_a_boolean_by_its_number_is_refused_not_taken() {
    // The hole names every element from its inputs (the fake keeps them all): the hole's
    // own `Edge5` is no name it holds, and is never taken for its fifth edge. An
    // operation's element by its number is never a stable name (N3), so the gate refuses
    // it before anything is rebuilt.
    let text = document(&[&fillet("f0000001", "m3tq8v0z", &["m3tq8v0z.Edge5"], "0.5")]);
    let invalid = Checked::new(read(text.as_bytes()).unwrap()).unwrap_err();
    let said: Vec<(ErrorCode, Option<&str>)> = invalid
        .errors()
        .iter()
        .map(|e| (e.code(), e.parameter()))
        .collect();
    assert_eq!(said, [(ErrorCode::InvalidInput, Some("edges.0"))]);
    assert!(invalid.errors()[0].message().contains("by its number"));
}

#[test]
fn fillet_names_that_alias_one_edge_are_one_edge_in_the_request() {
    // The same edge three ways through a fillet of a fillet: the first fillet keeps the
    // block's names, so `k7f2qx3m.box.Edge3` is the second fillet's base's edge 3. Listed once,
    // it's asked for once, and the other edges keep their order.
    let engine = engine();
    let text = document(&[
        &fillet("f0000001", "m3tq8v0z", &["k7f2qx3m.box.Edge1"], "0.5"),
        &fillet(
            "f0000002",
            "f0000001",
            &[
                "k7f2qx3m.box.Edge3",
                "a91cw0de.cylinder.Edge1",
                "k7f2qx3m.box.Edge2",
            ],
            "0.25",
        ),
    ]);
    let model = published(&engine, checked(&text));
    assert!(model.errors().next().is_none());
    let edges: Vec<Vec<u32>> = dressings(engine.kernel())
        .into_iter()
        .map(|(e, _)| e)
        .collect();
    assert_eq!(edges, [vec![1], vec![3, 13, 2]]);
}

#[test]
fn fillet_edits_flipping_between_resolving_and_broken_match_a_fresh_rebuild() {
    // Edits in a row, some waited for and some superseded, alternating a fillet between
    // names that resolve and names that don't: each published model's outcomes are those a
    // fresh engine gives the same document, and the kernel is asked for each fillet only
    // with edges its names find.
    let good = document(&[&fillet(
        "f0000001",
        "m3tq8v0z",
        &["a91cw0de.cylinder.Edge2", "k7f2qx3m.box.Edge4"],
        "0.5",
    )]);
    let bad = document(&[&fillet(
        "f0000001",
        "m3tq8v0z",
        &["a91cw0de.cylinder.Edge4", "k7f2qx3m.box.Edge4"],
        "0.5",
    )]);
    let fresh = |text: &str| {
        let engine = engine();
        let model = published(&engine, checked(text));
        model.features().to_vec()
    };
    let (want_good, want_bad) = (fresh(&good), fresh(&bad));
    let engine = Recompute::new(FakeKernel::new(4).unwrap());
    for round in 0..40 {
        let text = if round % 2 == 0 { &good } else { &bad };
        let pending = engine.start(checked(text));
        if round % 3 == 2 {
            // Superseded by the next edit.
            continue;
        }
        if let Ok(RunOutcome::Published(model)) = pending.wait() {
            let want = if round % 2 == 0 {
                &want_good
            } else {
                &want_bad
            };
            assert_eq!(model.features(), want.as_slice(), "round {round}");
        }
    }
    let model = published(&engine, checked(&good));
    assert_eq!(model.features(), want_good.as_slice());
    for (edges, _) in dressings(engine.kernel()) {
        assert_eq!(edges, [14, 4]);
    }
}

// ---------------------------------------------------------------------------------------
// The fake kernel's fillet and chamfer
// ---------------------------------------------------------------------------------------

fn fake_shape(text: &str) -> ShapeData {
    ShapeData::unchecked(Blob::inline(text.as_bytes().to_vec()))
}

#[test]
fn fake_reads_only_dressings_a_request_could_have_made() {
    // The fake says it reads only what a request could have made (its primitives are
    // checked by the protocol's rules): a fillet of an edge the box hasn't got, of a
    // negative radius, or of one edge twice is no shape it made.
    let made = fake_shape("silico fake shape 1\nbox 1 1 1\nfillet 0 0.1 1\n");
    assert!(FakeKernel::counts(&made).is_some());
    for text in [
        "silico fake shape 1\nbox 1 1 1\nfillet 0 0.1 13\n",
        "silico fake shape 1\nbox 1 1 1\nfillet 0 -1 1\n",
        "silico fake shape 1\nbox 1 1 1\nfillet 0 0 1\n",
        "silico fake shape 1\nbox 1 1 1\nchamfer 0 0.1 1,1\n",
    ] {
        // BREAKS: fake.rs `parse_step` reads a fillet's or chamfer's size as any finite
        // number and its edges as any numbers from 1, never checked against the shape or
        // the protocol's rules: bytes no request could make are taken as the fake's own.
        assert!(
            FakeKernel::counts(&fake_shape(text)).is_none(),
            "{text:?} read as {:?}",
            FakeKernel::describe(&fake_shape(text))
        );
    }
}

#[test]
fn fake_refuses_bytes_that_are_almost_a_dressing() {
    for text in [
        "silico fake shape 1\nbox 1 1 1\nfillet 0 0.1 \n",
        "silico fake shape 1\nbox 1 1 1\nfillet 0 0.1 ,\n",
        "silico fake shape 1\nbox 1 1 1\nfillet 0 0.1 1,\n",
        "silico fake shape 1\nbox 1 1 1\nfillet 0 0.1 01\n",
        "silico fake shape 1\nbox 1 1 1\nfillet 0 0.1 +1\n",
        "silico fake shape 1\nbox 1 1 1\nfillet 0 0.1 1, 2\n",
        "silico fake shape 1\nbox 1 1 1\nfillet 0 NaN 1\n",
        "silico fake shape 1\nbox 1 1 1\nfillet 0 inf 1\n",
        "silico fake shape 1\nbox 1 1 1\nfillet 0 0.10 1\n",
        "silico fake shape 1\nbox 1 1 1\nfillet 1 0.1 1\n",
        "silico fake shape 1\nbox 1 1 1\nfillet 0 0.1 1 2\n",
        "silico fake shape 1\nbox 1 1 1\nfillet 0 0.1 0\n",
        "silico fake shape 1\nbox 1 1 1\nfillet 0 0.1 4294967296\n",
        "silico fake shape 1\nfillet 0 0.1 1\n",
        "silico fake shape 1\nbox 1 1 1\nFillet 0 0.1 1\n",
    ] {
        assert!(FakeKernel::counts(&fake_shape(text)).is_none(), "{text:?}");
    }
}

#[test]
fn many_broken_names_on_a_big_base_list_a_hundred_and_count_the_rest() {
    // Found by the review: each broken name scanned every edge of the base for candidates,
    // 0.5 ms a name on a 6,000-edge base, minutes for a big document. The base's edges are
    // indexed once, and only the hundred errors listed look for candidates, so 50,000
    // broken names cost what naming the base does.
    let mut features = vec![
        r#"{"id": "b0000000", "type": "box", "v": 1, "name": "b", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#.to_owned(),
    ];
    let mut base = "b0000000".to_owned();
    for k in 1..100 {
        features.push(format!(r#"{{"id": "x{k:07}", "type": "box", "v": 1, "name": "x", "position": [{k}, 0, 0], "length": 1, "width": 1, "height": 1}}"#));
        features.push(format!(r#"{{"id": "w{k:07}", "type": "fuse", "v": 1, "name": "w", "base": "{base}", "tool": "x{k:07}"}}"#));
        base = format!("w{k:07}");
    }
    // 50,000 names, of the fused boxes' edges that aren't there.
    let broken: Vec<String> = (0..50_000)
        .map(|i| format!("x{:07}.box.Edge{}", 1 + i % 99, 13 + i / 99))
        .collect();
    let refs: Vec<&str> = broken.iter().map(String::as_str).collect();
    features.push(fillet("f0000001", &base, &refs, "0.1"));
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{}]}}"#,
        features.join(",\n")
    );
    let engine = engine();
    let model = published(&engine, checked(&text));
    let errors: Vec<&Error> = model.errors().collect();
    assert_eq!(errors.len(), 100);
    assert_eq!(model.error_count(), 50_000);
}

#[test]
fn fillet_too_large_to_save_is_told_to_name_fewer_or_shorter_edges() {
    // Found by the rebase's review: the advice for a feature too large to write said to
    // shorten its name or note, which for a fillet of 300 long edge names isn't the cause.
    let edge = format!("\"{}\"", "x".repeat(60_000));
    let edges = vec![edge; 300].join(", ");
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "features": [{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}}, {{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "k7f2qx3m", "edges": [{edges}], "radius": 1}}]}}"#
    );
    let error = read(text.as_bytes()).unwrap_err();
    assert_eq!(error.feature(), Some("f0000001"));
    let advice = error.suggestion().unwrap_or_default();
    assert!(advice.contains("fewer edges"), "{advice}");
}
