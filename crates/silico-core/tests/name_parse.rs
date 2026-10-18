//! M2.8: Silico's form of a name read back (`Name::parse`), exactly the grammar M2.7 writes.
//! Expected names are written by hand from the grammar (docs/ports/naming.md); a name read
//! must equal the name a map holds, so a reference finds its element by identity.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::num::NonZeroU32;
use std::sync::Arc;

use proptest::prelude::*;
use silico_core::document::FeatureId;
use silico_core::kernel::{ElementKind, ElementRef};
use silico_core::naming::{
    Maker, Name, NameError, OpCode, Place, PrimitiveKind, Raw, Relation, Source,
};

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

/// The features these tests know: `aaaaaaaa` and `bbbbbbbb` primitives, `cccccccc` a cut,
/// `ffffffff` a fillet.
fn makers(feature: &FeatureId) -> Option<Maker> {
    match feature.as_str() {
        "aaaaaaaa" | "bbbbbbbb" => Some(Maker::Itself(PrimitiveKind::Box)),
        "cccccccc" => Some(Maker::By(OpCode::Cut)),
        "ffffffff" => Some(Maker::By(OpCode::Fillet)),
        _ => None,
    }
}

fn parse(text: &str) -> Result<Name, NameError> {
    Name::parse(text, &makers)
}

fn n(k: u32) -> NonZeroU32 {
    NonZeroU32::new(k).unwrap()
}

/// `feature`'s element by its number: a primitive's (`aaaaaaaa` and `bbbbbbbb` are boxes)
/// says its kind.
fn element(feature: &str, kind: ElementKind, index: u32) -> Arc<Name> {
    let primitive = matches!(feature, "aaaaaaaa" | "bbbbbbbb").then_some(PrimitiveKind::Box);
    Arc::new(Name::Element {
        feature: id(feature),
        primitive,
        element: ElementRef { kind, index },
    })
}

fn source(relation: Relation, place: Option<(u32, u32)>, name: Arc<Name>) -> Source {
    Source {
        relation,
        place: place.map(|(rank, size)| Place {
            rank: n(rank),
            size: n(size),
        }),
        position: Raw(n(7)),
        name,
    }
}

#[test]
fn each_kind_of_name_reads_as_the_name_it_writes() {
    let face6 = element("aaaaaaaa", ElementKind::Face, 6);
    let pin = element("bbbbbbbb", ElementKind::Face, 1);
    let rim = Name::Derived {
        feature: id("cccccccc"),
        operation: OpCode::Cut,
        kind: ElementKind::Edge,
        first: source(Relation::Generated, None, Arc::clone(&face6)),
        more: vec![source(Relation::Generated, Some((2, 3)), Arc::clone(&pin))],
    };
    let rim = Arc::new(rim);
    let cases: Vec<(&str, Name)> = vec![
        ("aaaaaaaa.box.Face6", (*face6).clone()),
        (
            "cccccccc.Edge(G aaaaaaaa.box.Face6, G2/3 bbbbbbbb.box.Face1)",
            (*rim).clone(),
        ),
        (
            "ffffffff.Face(G cccccccc.Edge(G aaaaaaaa.box.Face6, G2/3 bbbbbbbb.box.Face1))",
            Name::Derived {
                feature: id("ffffffff"),
                operation: OpCode::Fillet,
                kind: ElementKind::Face,
                first: source(Relation::Generated, None, Arc::clone(&rim)),
                more: vec![],
            },
        ),
        (
            "ffffffff.Edge(U aaaaaaaa.box.Face6)",
            Name::Upper {
                feature: id("ffffffff"),
                operation: OpCode::Fillet,
                kind: ElementKind::Edge,
                upper: Arc::clone(&face6),
                more: vec![],
                place: None,
                nth: Raw(n(1)),
            },
        ),
        (
            "ffffffff.Vertex(U2/3 aaaaaaaa.box.Face6, aaaaaaaa.box.Edge1)",
            Name::Upper {
                feature: id("ffffffff"),
                operation: OpCode::Fillet,
                kind: ElementKind::Vertex,
                upper: Arc::clone(&face6),
                more: vec![element("aaaaaaaa", ElementKind::Edge, 1)],
                place: Some(Place {
                    rank: n(2),
                    size: n(3),
                }),
                nth: Raw(n(12)),
            },
        ),
        (
            "ffffffff.Face(L aaaaaaaa.box.Edge1, aaaaaaaa.box.Edge2)",
            Name::Lower {
                feature: id("ffffffff"),
                operation: OpCode::Fillet,
                kind: ElementKind::Face,
                first: element("aaaaaaaa", ElementKind::Edge, 1),
                more: vec![element("aaaaaaaa", ElementKind::Edge, 2)],
            },
        ),
        (
            "cccccccc.Edge(M1/2 aaaaaaaa.box.Edge10)~1~2",
            Name::Duplicate {
                name: Arc::new(Name::Duplicate {
                    name: Arc::new(Name::Derived {
                        feature: id("cccccccc"),
                        operation: OpCode::Cut,
                        kind: ElementKind::Edge,
                        first: source(
                            Relation::Modified,
                            Some((1, 2)),
                            element("aaaaaaaa", ElementKind::Edge, 10),
                        ),
                        more: vec![],
                    }),
                    ordinal: n(1),
                }),
                ordinal: n(2),
            },
        ),
        (
            "cccccccc.Solid1",
            (*element("cccccccc", ElementKind::Solid, 1)).clone(),
        ),
    ];
    for (text, name) in cases {
        let read = parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(read, name, "{text}");
        assert_eq!(read.to_string(), text);
        assert_eq!(name.to_string(), text);
    }
}

#[test]
fn a_name_that_isnt_one_is_refused_saying_where() {
    // (text, column, words in the reason)
    let cases = [
        ("", 1, "a feature's ID"),
        ("aaaaaaa.Face1", 1, "a feature's ID"),
        ("AAAAAAAA.Face1", 1, "a feature's ID"),
        ("aaaaaaaa", 9, "\".\""),
        ("aaaaaaaa.face1", 10, "its elements say which"),
        ("aaaaaaaa.box.face1", 14, "`Vertex`, `Edge`"),
        // A primitive's kind only on a primitive's elements.
        ("cccccccc.box.Edge1", 1, "isn't a primitive"),
        ("aaaaaaaa.box.Face", 18, "a number"),
        ("aaaaaaaa.box.Face0", 18, "without a leading zero"),
        ("aaaaaaaa.box.Face01", 18, "without a leading zero"),
        ("aaaaaaaa.box.Face4294967296", 18, "past 4294967295"),
        ("aaaaaaaa.box.Face1 ", 19, "more after"),
        ("aaaaaaaa.box.Face1~0", 20, "without a leading zero"),
        ("aaaaaaaa.box.Face1~", 20, "a number"),
        // A primitive names its elements by number; an unknown feature is no one's.
        (
            "aaaaaaaa.box.Edge(G aaaaaaaa.box.Face1)",
            1,
            "by their numbers",
        ),
        (
            "zzzzzzzz.Edge(G aaaaaaaa.box.Face1)",
            1,
            "no feature zzzzzzzz",
        ),
        ("cccccccc.Edge(G aaaaaaaa.box.Face1", 35, "\")\""),
        ("cccccccc.Edge(X aaaaaaaa.box.Face1)", 15, "`M` or `G`"),
        ("cccccccc.Edge(G  aaaaaaaa.box.Face1)", 17, "a feature's ID"),
        ("cccccccc.Edge(Gaaaaaaaa.Face1)", 16, "\" \""),
        ("cccccccc.Edge(G1 aaaaaaaa.box.Face1)", 17, "\"/\""),
        (
            "cccccccc.Edge(G1/1 aaaaaaaa.box.Face1)",
            19,
            "among 2 or more",
        ),
        (
            "cccccccc.Edge(G3/2 aaaaaaaa.box.Face1)",
            19,
            "at most its group's size",
        ),
        (
            "cccccccc.Edge(G0/2 aaaaaaaa.box.Face1)",
            16,
            "without a leading zero",
        ),
        (
            "cccccccc.Edge(G aaaaaaaa.box.Face1,aaaaaaaa.box.Face2)",
            35,
            "\")\"",
        ),
        // `U` takes a place, as a source does, not FreeCAD's count.
        ("ffffffff.Edge(U1 aaaaaaaa.box.Face1)", 17, "\"/\""),
        (
            "ffffffff.Edge(U1/1 aaaaaaaa.box.Face1)",
            19,
            "among 2 or more",
        ),
        (
            "ffffffff.Edge(U aaaaaaaa.box.Face1,aaaaaaaa.box.Face2)",
            35,
            "\")\"",
        ),
        ("ffffffff.Face(L)", 15, "`M` or `G`"),
    ];
    for (text, column, why) in cases {
        let error = match parse(text) {
            Ok(name) => panic!("{text:?} read as {name}"),
            Err(error) => error,
        };
        assert!(error.why.contains(why), "{text:?}: {error}");
        assert_eq!(error.column, column, "{text:?}: {error}");
    }
}

#[test]
fn a_name_lists_at_most_five_sources_or_five_elements_below() {
    let sources = |k: usize| {
        (1..=k)
            .map(|i| format!("G aaaaaaaa.box.Face{i}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    assert!(parse(&format!("cccccccc.Edge({})", sources(5))).is_ok());
    let error = parse(&format!("cccccccc.Edge({})", sources(6))).unwrap_err();
    assert!(error.why.contains("more than 5 sources"), "{error}");
    let below = |k: usize| {
        (1..=k)
            .map(|i| format!("aaaaaaaa.box.Edge{i}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    assert!(parse(&format!("ffffffff.Face(L {})", below(5))).is_ok());
    let error = parse(&format!("ffffffff.Face(L {})", below(6))).unwrap_err();
    assert!(error.why.contains("more than 5 elements below"), "{error}");
    // And above (found by the tests review: untested), with or without a place.
    let above = |k: usize| {
        (1..=k)
            .map(|i| format!("aaaaaaaa.box.Face{i}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    for place in ["", "2/3"] {
        assert!(parse(&format!("ffffffff.Edge(U{place} {})", above(5))).is_ok());
        let error = parse(&format!("ffffffff.Edge(U{place} {})", above(6))).unwrap_err();
        assert!(error.why.contains("more than 5 elements above"), "{error}");
    }
}

#[test]
fn a_name_nested_deeper_than_any_map_holds_is_refused_without_a_deep_stack() {
    // 256 levels is the bound a map's names keep to; past it, the text is refused, on a
    // default thread's 2 MB stack (as names are written on one), whatever its length.
    let nested = |depth: usize| {
        format!(
            "{}aaaaaaaa.box.Face1{}",
            "cccccccc.Face(M ".repeat(depth - 1),
            ")".repeat(depth - 1)
        )
    };
    assert!(parse(&nested(256)).is_ok());
    let deep = nested(257);
    let refused = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || parse(&deep))
        .unwrap()
        .join()
        .unwrap();
    // One message for every depth refusal (found by the round-5 review: nothing held it).
    assert_eq!(
        refused.unwrap_err().why,
        "a name nested more than 256 deep, deeper than a reference can be"
    );
    let deeper = nested(100_000);
    let refused = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || parse(&deeper))
        .unwrap()
        .join()
        .unwrap();
    assert!(refused.is_err());
}

#[test]
fn a_parsed_name_is_its_identity_whatever_raw_positions_it_was_built_with() {
    // The text holds no raw OCCT positions: a name read equals the map's, built with its
    // own positions, and they order and hash alike.
    let built = Name::Derived {
        feature: id("cccccccc"),
        operation: OpCode::Cut,
        kind: ElementKind::Edge,
        first: source(
            Relation::Generated,
            Some((1, 2)),
            element("aaaaaaaa", ElementKind::Face, 1),
        ),
        more: vec![],
    };
    let read = parse(&built.to_string()).unwrap();
    assert_eq!(read, built);
    assert_eq!(read.cmp(&built), std::cmp::Ordering::Equal);
    let hash = |n: &Name| {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        n.hash(&mut h);
        h.finish()
    };
    assert_eq!(hash(&read), hash(&built));
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn any_text_is_read_or_refused_never_a_panic(text in ".{0,80}") {
        if let Ok(name) = parse(&text) {
            prop_assert_eq!(name.to_string(), text);
        }
    }

    #[test]
    fn text_from_the_grammars_own_pieces_reads_back_as_written_or_is_refused(
        pieces in proptest::collection::vec(
            prop_oneof![
                Just("aaaaaaaa"), Just("cccccccc"), Just("ffffffff"), Just("."), Just("Face"),
                Just("Edge"), Just("Vertex"), Just("1"), Just("2"), Just("0"), Just("("),
                Just(")"), Just("G"), Just("M"), Just("U"), Just("L"), Just(" "), Just(", "),
                Just("/"), Just("~"),
            ],
            0..40,
        )
    ) {
        let text: String = pieces.concat();
        match parse(&text) {
            // A name read writes back as its text, exactly: the form is canonical.
            Ok(name) => prop_assert_eq!(name.to_string(), text),
            Err(error) => prop_assert!(error.column >= 1 && error.column <= text.len() + 1),
        }
    }
}

#[test]
fn a_chain_of_duplicates_longer_than_any_map_holds_is_refused_on_a_small_stack() {
    // Found by the review: `~1` repeated 20,000 times (40 KB) built a chain the gate's walks
    // overflowed a 2 MB stack on, aborting the process. A `~n` is a level, as the builder
    // counts it: past 256, refused while reading.
    let chain = |n: usize| format!("aaaaaaaa.box.Edge1{}", "~1".repeat(n));
    assert!(parse(&chain(255)).is_ok());
    for n in [256, 20_000, 1_000_000] {
        let text = chain(n);
        let refused = std::thread::Builder::new()
            .stack_size(256 << 10)
            .spawn(move || parse(&text).map(|_| ()))
            .unwrap()
            .join()
            .unwrap();
        assert!(
            refused.unwrap_err().why.contains("nested more than 256"),
            "{n}"
        );
    }
}

#[test]
fn a_name_of_more_names_than_any_map_holds_is_refused() {
    // Five sources a level, three levels: 1 + 5 + 25 + 125 names, fine; a wider, deeper
    // name past 4,096 names written out is refused, as the builder bounds a name.
    fn wide(depth: u32) -> String {
        if depth == 0 {
            return "aaaaaaaa.box.Face1".to_owned();
        }
        let inner = wide(depth - 1);
        let sources: Vec<String> = (0..5).map(|_| format!("G {inner}")).collect();
        format!("cccccccc.Face({})", sources.join(", "))
    }
    assert!(parse(&wide(3)).is_ok());
    let error = parse(&wide(6)).unwrap_err();
    assert!(error.why.contains("more than 4096 names"), "{error}");
}

#[test]
fn tilde_chains_inside_upper_lower_and_later_sources_count_toward_the_bound() {
    // Found by the tests review (round 3): only a derived name's first source was tested.
    // A face and 127 `~1`s inside, the outer name's level, 127 `~1`s on it: 256, read. One
    // more: 257, refused. Through each composite form and each slot the reader takes.
    let inside = format!("aaaaaaaa.box.Face1{}", "~1".repeat(127));
    let other = "bbbbbbbb.box.Face1";
    // The deep one first too (found by the round-4 review): the tallest slot counts, not
    // the last.
    for outer in [
        format!("ffffffff.Edge(U {inside})"),
        format!("ffffffff.Edge(U {other}, {inside})"),
        format!("ffffffff.Edge(U {inside}, {other})"),
        format!("ffffffff.Edge(L {inside})"),
        format!("ffffffff.Edge(L {other}, {inside})"),
        format!("ffffffff.Edge(L {inside}, {other})"),
        format!("cccccccc.Face(M {other}, G {inside})"),
        format!("cccccccc.Face(M {inside}, G {other})"),
    ] {
        let at = format!("{outer}{}", "~1".repeat(127));
        assert!(parse(&at).is_ok(), "{at}");
        let past = format!("{at}~1");
        let error = parse(&past).unwrap_err();
        assert!(
            error.why.contains("nested more than 256 deep"),
            "{outer}: {error}"
        );
    }
}

#[test]
fn a_name_nested_too_deep_through_any_slot_is_refused_without_a_deep_stack() {
    // Found by mutation testing: only a derived name's first source was nested deep, so a
    // reader that didn't count a name above's, a name below's, or a later source's level
    // passed, and read 100,000 levels on its stack. Each slot, on a 2 MB thread.
    let leaf = "aaaaaaaa.box.Face1";
    let forms: [(&str, &str); 5] = [
        ("ffffffff.Edge(U ", ")"),
        ("ffffffff.Edge(U bbbbbbbb.box.Face1, ", ")"),
        ("ffffffff.Edge(L ", ")"),
        ("ffffffff.Edge(L bbbbbbbb.box.Face1, ", ")"),
        ("cccccccc.Face(M bbbbbbbb.box.Face1, G ", ")"),
    ];
    for (open, close) in forms {
        let deep = format!("{}{leaf}{}", open.repeat(100_000), close.repeat(100_000));
        let refused = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || parse(&deep).map(|_| ()))
            .unwrap()
            .join()
            .unwrap();
        let error = refused.unwrap_err();
        assert!(
            error.why.contains("nested more than 256"),
            "{open}: {error}"
        );
    }
}

/// A derived name of five sources, each one `level - 1` levels down, to `level` 0, a leaf:
/// (5^(level+1) - 1) / 4 names written out.
fn wide(level: u32) -> String {
    if level == 0 {
        return "aaaaaaaa.box.Face1".to_owned();
    }
    let inner = wide(level - 1);
    let sources: Vec<String> = (0..5).map(|_| format!("M {inner}")).collect();
    format!("cccccccc.Face({})", sources.join(", "))
}

#[test]
fn a_name_of_exactly_the_most_names_a_map_holds_is_read_and_one_more_is_not() {
    // Found by mutation testing: the bound of 4,096 names written out was tested at 156
    // and 19,531, never at its edge. Each `~n` counts as a name, as the builder counts it.
    // wide(5) is 3,906 names; with 190 `~1`s, 4,096.
    let at = format!("{}{}", wide(5), "~1".repeat(190));
    assert!(parse(&at).is_ok());
    let error = parse(&format!("{at}~1")).unwrap_err();
    assert!(error.why.contains("more than 4096 names"), "{error}");
    // Names alone: one of 3,906, one of 156, one of 31, and two leaves, under one: 4,096.
    let sources = |last: &str| {
        format!(
            "cccccccc.Face(M {}, M {}, M {}, M aaaaaaaa.box.Face1, M {last})",
            wide(5),
            wide(3),
            wide(2)
        )
    };
    assert!(parse(&sources("aaaaaaaa.box.Face1")).is_ok());
    let past = sources("cccccccc.Face(M aaaaaaaa.box.Face1)");
    let error = parse(&past).unwrap_err();
    assert!(error.why.contains("more than 4096 names"), "{error}");
}
