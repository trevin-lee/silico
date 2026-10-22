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
    Maker, Name, NameError, OpCode, Place, PrimitiveKind, Raw, Relation, Source, TypeWord,
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

/// `feature`'s element by its number, with the type `feature` is (`makers`').
fn element(feature: &str, kind: ElementKind, index: u32) -> Arc<Name> {
    let made = match makers(&id(feature)) {
        Some(Maker::Itself(kind)) => TypeWord::Primitive(kind),
        Some(Maker::By(op)) => TypeWord::Operation(op),
        other => panic!("{feature}: {other:?}"),
    };
    Arc::new(Name::Element {
        feature: id(feature),
        made,
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
        position: Raw(Some(n(7))),
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
            "cccccccc.cut.Edge(G aaaaaaaa.box.Face6, G2/3 bbbbbbbb.box.Face1)",
            (*rim).clone(),
        ),
        (
            "ffffffff.fillet.Face(G cccccccc.cut.Edge(G aaaaaaaa.box.Face6, G2/3 bbbbbbbb.box.Face1))",
            Name::Derived {
                feature: id("ffffffff"),
                operation: OpCode::Fillet,
                kind: ElementKind::Face,
                first: source(Relation::Generated, None, Arc::clone(&rim)),
                more: vec![],
            },
        ),
        (
            "ffffffff.fillet.Edge(U aaaaaaaa.box.Face6)",
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
            "ffffffff.fillet.Vertex(U2/3 aaaaaaaa.box.Face6, aaaaaaaa.box.Edge1)",
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
            "ffffffff.fillet.Face(L aaaaaaaa.box.Edge1, aaaaaaaa.box.Edge2)",
            Name::Lower {
                feature: id("ffffffff"),
                operation: OpCode::Fillet,
                kind: ElementKind::Face,
                first: element("aaaaaaaa", ElementKind::Edge, 1),
                more: vec![element("aaaaaaaa", ElementKind::Edge, 2)],
            },
        ),
        (
            "cccccccc.cut.Edge(M1/2 aaaaaaaa.box.Edge10)~1~2",
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
            "cccccccc.cut.Solid1",
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
fn what_an_operation_keeps_inside_its_other_operand_reads_as_i() {
    // N9's letter: `I`, alone or with a place, reads back as written, an `Inside` source.
    for (text, place) in [
        ("cccccccc.cut.Edge(I aaaaaaaa.box.Edge9)", None),
        ("cccccccc.cut.Edge(I1/2 aaaaaaaa.box.Edge9)", Some((1, 2))),
        (
            "cccccccc.cut.Face(I aaaaaaaa.box.Face1, G aaaaaaaa.box.Edge1)",
            None,
        ),
    ] {
        let name = parse(text).unwrap();
        assert_eq!(name.to_string(), text);
        let Name::Derived { first, .. } = &name else {
            panic!("{name:?}");
        };
        assert_eq!(first.relation, Relation::Inside, "{text}");
        assert_eq!(
            first.place.map(|p| (p.rank.get(), p.size.get())),
            place,
            "{text}"
        );
    }
    let error = parse("cccccccc.cut.Edge(X aaaaaaaa.box.Edge9)").unwrap_err();
    assert!(error.why.contains("`I`"), "{error}");
}

#[test]
fn a_name_reads_its_features_type_as_written_whatever_the_feature_is_now() {
    // N8: the type is read from the text, never taken from the feature. A box's word on a
    // cut's element, and a cut's on a fillet's, read as written: names no map of theirs
    // holds (the lookup breaks the reference, with candidates: references.rs). An
    // operation's element by its number reads, and is unstable (the gate refuses it).
    for (text, made) in [
        (
            "cccccccc.box.Edge1",
            TypeWord::Primitive(PrimitiveKind::Box),
        ),
        ("cccccccc.cut.Edge1", TypeWord::Operation(OpCode::Cut)),
        ("cccccccc.common.Edge7", TypeWord::Operation(OpCode::Common)),
        (
            "aaaaaaaa.cylinder.Face3",
            TypeWord::Primitive(PrimitiveKind::Cylinder),
        ),
    ] {
        let name = parse(text).unwrap();
        assert!(
            matches!(name, Name::Element { made: m, .. } if m == made),
            "{text}: {name:?}"
        );
        assert_eq!(name.to_string(), text);
    }
    let name = parse("ffffffff.chamfer.Edge(G aaaaaaaa.box.Face1)").unwrap();
    assert!(
        matches!(
            name,
            Name::Derived {
                operation: OpCode::Chamfer,
                ..
            }
        ),
        "{name:?}"
    );
    // Every one of the fifteen words, on an element by its number: a sketch's only number
    // is its only region's, Face1 (M3.6, S4).
    for made in TypeWord::all() {
        let face = if made.word() == "sketch" { 1 } else { 2 };
        let text = format!("cccccccc.{made}.Face{face}");
        assert_eq!(parse(&text).unwrap().to_string(), text);
    }
    assert_eq!(TypeWord::all().count(), 15);
}

#[test]
fn a_name_that_isnt_one_is_refused_saying_where() {
    // (text, column, words in the reason)
    let cases = [
        ("", 1, "a feature's ID"),
        ("aaaaaaa.Face1", 1, "a feature's ID"),
        ("AAAAAAAA.Face1", 1, "a feature's ID"),
        ("aaaaaaaa", 9, "\".\""),
        // Every element says its feature's type, listing them, and an operation's too.
        (
            "aaaaaaaa.face1",
            10,
            "`box`, `cylinder`, `sphere`, `cone`, `torus`, `fuse`, `cut`",
        ),
        ("aaaaaaaa.Face1", 10, "as `aaaaaaaa.box.Face1`"),
        ("cccccccc.Edge1", 10, "as `cccccccc.cut.Edge1`"),
        (
            "cccccccc.Edge(G aaaaaaaa.box.Face1)",
            10,
            "the type of the feature",
        ),
        ("cccccccc.Cut.Edge1", 10, "the type of the feature"),
        ("aaaaaaaa.box.face1", 14, "`Vertex`, `Edge`"),
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
            14,
            "a box makes its elements itself",
        ),
        (
            "cccccccc.cylinder.Edge(G aaaaaaaa.box.Face1)",
            19,
            "a cylinder makes its elements itself",
        ),
        (
            "zzzzzzzz.Edge(G aaaaaaaa.box.Face1)",
            1,
            "no feature zzzzzzzz",
        ),
        ("cccccccc.cut.Edge(G aaaaaaaa.box.Face1", 39, "\")\""),
        (
            "cccccccc.cut.Edge(X aaaaaaaa.box.Face1)",
            19,
            "`M`, `G`, or `I`",
        ),
        (
            "cccccccc.cut.Edge(G  aaaaaaaa.box.Face1)",
            21,
            "a feature's ID",
        ),
        ("cccccccc.cut.Edge(Gaaaaaaaa.Face1)", 20, "\" \""),
        ("cccccccc.cut.Edge(G1 aaaaaaaa.box.Face1)", 21, "\"/\""),
        (
            "cccccccc.cut.Edge(G1/1 aaaaaaaa.box.Face1)",
            23,
            "among 2 or more",
        ),
        (
            "cccccccc.cut.Edge(G3/2 aaaaaaaa.box.Face1)",
            23,
            "at most its group's size",
        ),
        (
            "cccccccc.cut.Edge(G0/2 aaaaaaaa.box.Face1)",
            20,
            "without a leading zero",
        ),
        (
            "cccccccc.cut.Edge(G aaaaaaaa.box.Face1,aaaaaaaa.box.Face2)",
            39,
            "\")\"",
        ),
        // `U` takes a place, as a source does, not FreeCAD's count.
        ("ffffffff.fillet.Edge(U1 aaaaaaaa.box.Face1)", 24, "\"/\""),
        (
            "ffffffff.fillet.Edge(U1/1 aaaaaaaa.box.Face1)",
            26,
            "among 2 or more",
        ),
        (
            "ffffffff.fillet.Edge(U aaaaaaaa.box.Face1,aaaaaaaa.box.Face2)",
            42,
            "\")\"",
        ),
        ("ffffffff.fillet.Face(L)", 22, "`M`, `G`, or `I`"),
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
    assert!(parse(&format!("cccccccc.cut.Edge({})", sources(5))).is_ok());
    let error = parse(&format!("cccccccc.cut.Edge({})", sources(6))).unwrap_err();
    assert!(error.why.contains("more than 5 sources"), "{error}");
    let below = |k: usize| {
        (1..=k)
            .map(|i| format!("aaaaaaaa.box.Edge{i}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    assert!(parse(&format!("ffffffff.fillet.Face(L {})", below(5))).is_ok());
    let error = parse(&format!("ffffffff.fillet.Face(L {})", below(6))).unwrap_err();
    assert!(error.why.contains("more than 5 elements below"), "{error}");
    // And above (found by the tests review: untested), with or without a place.
    let above = |k: usize| {
        (1..=k)
            .map(|i| format!("aaaaaaaa.box.Face{i}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    for place in ["", "2/3"] {
        assert!(parse(&format!("ffffffff.fillet.Edge(U{place} {})", above(5))).is_ok());
        let error = parse(&format!("ffffffff.fillet.Edge(U{place} {})", above(6))).unwrap_err();
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
            "cccccccc.cut.Face(M ".repeat(depth - 1),
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
        text in grammar_pieces()
    ) {
        match parse(&text) {
            // A name read writes back as its text, exactly: the form is canonical.
            Ok(name) => prop_assert_eq!(name.to_string(), text),
            Err(error) => prop_assert!(error.column >= 1 && error.column <= text.len() + 1),
        }
    }
}

/// The grammar's own pieces, type words among them (N8: a name without one never reads).
const PIECES: [&str; 29] = [
    "aaaaaaaa",
    "cccccccc",
    "ffffffff",
    ".",
    "box.",
    "cut.",
    "fillet.",
    "common.",
    "aaaaaaaa.box.",
    "cccccccc.cut.",
    "ffffffff.fillet.",
    "Face",
    "Edge",
    "Vertex",
    "1",
    "2",
    "0",
    "(",
    ")",
    "G",
    "M",
    "I",
    "U",
    "L",
    " ",
    ", ",
    ",",
    "/",
    "~",
];

/// A name's text, as pieces: a primitive's element by number, or an operation's derived
/// from one or two, from above, or from below.
fn name_pieces() -> impl Strategy<Value = Vec<&'static str>> {
    let kind = prop_oneof![Just("Face"), Just("Edge"), Just("Vertex")];
    let number = prop_oneof![Just("1"), Just("2"), Just("12")];
    let leaf = (kind.clone(), number.clone()).prop_map(|(k, n)| vec!["aaaaaaaa.box.", k, n]);
    let op = prop_oneof![Just("cccccccc.cut."), Just("ffffffff.fillet.")];
    prop_oneof![
        leaf.clone(),
        (op.clone(), kind.clone(), number).prop_map(|(o, k, n)| vec![o, k, n]),
        (
            op.clone(),
            kind.clone(),
            prop_oneof![Just("M"), Just("G"), Just("I"), Just("I1/2")],
            leaf.clone()
        )
            .prop_map(|(o, k, r, l)| [vec![o, k, "(", r, " "], l, vec![")"]].concat()),
        (op.clone(), kind.clone(), leaf.clone(), leaf.clone()).prop_map(|(o, k, a, b)| [
            vec![o, k, "(", "G", " "],
            a,
            vec![", ", "G", " "],
            b,
            vec![")"]
        ]
        .concat()),
        (op, kind, prop_oneof![Just("U"), Just("L")], leaf).prop_map(|(o, k, w, l)| [
            vec![o, k, "(", w, " "],
            l,
            vec![")"]
        ]
        .concat()),
    ]
}

/// A name's text, or a near miss: one of its pieces replaced by another of the grammar's,
/// or dropped, or one put in; so text close to a name is read exactly or refused.
fn grammar_pieces() -> impl Strategy<Value = String> {
    (
        name_pieces(),
        0usize..32,
        0usize..4,
        proptest::sample::select(PIECES.to_vec()),
    )
        .prop_map(|(mut pieces, at, edit, piece)| {
            let at = at % (pieces.len() + 1);
            match edit {
                1 if at < pieces.len() => pieces[at] = piece,
                2 if at < pieces.len() => {
                    pieces.remove(at);
                }
                3 => pieces.insert(at, piece),
                _ => {}
            }
            pieces.concat()
        })
}

#[test]
fn some_text_from_the_grammars_pieces_is_a_name() {
    // So the property above reads names, not only refusals (found by the milestone's tests
    // review: under N8 its pieces could spell no type word, and every case was refused).
    use proptest::strategy::ValueTree;
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let strategy = grammar_pieces();
    let mut read = 0;
    for _ in 0..3000 {
        let text = strategy.new_tree(&mut runner).unwrap().current();
        if let Ok(name) = parse(&text) {
            assert_eq!(name.to_string(), text);
            read += 1;
        }
    }
    assert!(read >= 20, "only {read} of 3000 read");
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
        format!("cccccccc.cut.Face({})", sources.join(", "))
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
        format!("ffffffff.fillet.Edge(U {inside})"),
        format!("ffffffff.fillet.Edge(U {other}, {inside})"),
        format!("ffffffff.fillet.Edge(U {inside}, {other})"),
        format!("ffffffff.fillet.Edge(L {inside})"),
        format!("ffffffff.fillet.Edge(L {other}, {inside})"),
        format!("ffffffff.fillet.Edge(L {inside}, {other})"),
        format!("cccccccc.cut.Face(M {other}, G {inside})"),
        format!("cccccccc.cut.Face(M {inside}, G {other})"),
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
        ("ffffffff.fillet.Edge(U ", ")"),
        ("ffffffff.fillet.Edge(U bbbbbbbb.box.Face1, ", ")"),
        ("ffffffff.fillet.Edge(L ", ")"),
        ("ffffffff.fillet.Edge(L bbbbbbbb.box.Face1, ", ")"),
        ("cccccccc.cut.Face(M bbbbbbbb.box.Face1, G ", ")"),
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
    format!("cccccccc.cut.Face({})", sources.join(", "))
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
            "cccccccc.cut.Face(M {}, M {}, M {}, M aaaaaaaa.box.Face1, M {last})",
            wide(5),
            wide(3),
            wide(2)
        )
    };
    assert!(parse(&sources("aaaaaaaa.box.Face1")).is_ok());
    let past = sources("cccccccc.cut.Face(M aaaaaaaa.box.Face1)");
    let error = parse(&past).unwrap_err();
    assert!(error.why.contains("more than 4096 names"), "{error}");
}

#[test]
fn a_name_256_deep_through_any_slot_is_read_and_one_deeper_is_not() {
    // Found by the review: only a derived name's first source was read at the bound, so a
    // reader counting another slot's depth too fast refused names a map holds. Each slot
    // that holds a name, 256 deep and 257.
    let leaf = "aaaaaaaa.box.Face1";
    let forms: [(&str, &str); 5] = [
        ("ffffffff.fillet.Edge(U ", ")"),
        ("ffffffff.fillet.Edge(U bbbbbbbb.box.Face1, ", ")"),
        ("ffffffff.fillet.Edge(L ", ")"),
        ("ffffffff.fillet.Edge(L bbbbbbbb.box.Face1, ", ")"),
        ("cccccccc.cut.Face(M bbbbbbbb.box.Face1, G ", ")"),
    ];
    for (open, close) in forms {
        let at = format!("{}{leaf}{}", open.repeat(255), close.repeat(255));
        assert!(parse(&at).is_ok(), "{open}: {:?}", parse(&at).err());
        let past = format!("{}{leaf}{}", open.repeat(256), close.repeat(256));
        assert!(
            parse(&past)
                .unwrap_err()
                .why
                .contains("nested more than 256"),
            "{open}"
        );
    }
}

/// The features of M3.6's grammar tests: a sketch `ssssssss`, a pad `pppppppp`, and a
/// pocket `qqqqqqqq`.
fn sweep_makers(feature: &FeatureId) -> Option<Maker> {
    match feature.as_str() {
        "ssssssss" => Some(Maker::By(OpCode::Sketch)),
        "pppppppp" => Some(Maker::By(OpCode::Pad)),
        "qqqqqqqq" => Some(Maker::By(OpCode::Pocket)),
        "aaaaaaaa" => Some(Maker::Itself(PrimitiveKind::Box)),
        _ => None,
    }
}

#[test]
fn a_sketchs_elements_read_by_their_ids_inside_what_a_feature_made_of_them() {
    // M3.6, design S4: a sketch's curve `s.sketch.Edge.<id>`, its point
    // `s.sketch.Vertex.<id>`, a circle's vertex `s.sketch.Vertex(U s.sketch.Edge.<id>)`,
    // its only region `s.sketch.Face1`, and one of several `s.sketch.Face(L
    // s.sketch.Edge.<id>)`; each as a source of a pad's or pocket's element, read as the
    // name written, the sketch's elements as `Name::Sketch` by ID.
    let parse = |text: &str| Name::parse(text, &sweep_makers);
    let curve = Arc::new(Name::Sketch {
        feature: id("ssssssss"),
        kind: ElementKind::Edge,
        id: silico_core::document::ElementId::new("6t7v8w9x").unwrap(),
    });
    let wall = Name::Derived {
        feature: id("pppppppp"),
        operation: OpCode::Pad,
        kind: ElementKind::Face,
        first: source(Relation::Generated, None, Arc::clone(&curve)),
        more: Vec::new(),
    };
    let text = "pppppppp.pad.Face(G ssssssss.sketch.Edge.6t7v8w9x)";
    assert_eq!(wall.to_string(), text);
    assert_eq!(parse(text).unwrap(), wall);
    for text in [
        "pppppppp.pad.Edge(M ssssssss.sketch.Edge.6t7v8w9x)",
        "pppppppp.pad.Edge(G ssssssss.sketch.Vertex.0a1b2c3d)",
        "pppppppp.pad.Face(G ssssssss.sketch.Face1)",
        "pppppppp.pad.Face(M ssssssss.sketch.Face1)",
        "pppppppp.pad.Edge(G ssssssss.sketch.Vertex(U ssssssss.sketch.Edge.c1rc1e00))",
        "qqqqqqqq.pocket.Face(G ssssssss.sketch.Face(L ssssssss.sketch.Edge.6t7v8w9x))",
        // An ID that's all digits: the dot keeps it apart from a number.
        "qqqqqqqq.pocket.Face(G ssssssss.sketch.Edge.12345678)",
        "qqqqqqqq.pocket.Edge(G aaaaaaaa.box.Face6, G qqqqqqqq.pocket.Face(G ssssssss.sketch.Edge.6t7v8w9x))",
        "qqqqqqqq.pocket.Face(M1/2 qqqqqqqq.pocket.Face(G ssssssss.sketch.Edge.6t7v8w9x))",
    ] {
        let name = parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(name.to_string(), text);
    }
    // A sketch's only region is stable: a name by number that is, by design.
    let region = parse("pppppppp.pad.Face(G ssssssss.sketch.Face1)").unwrap();
    assert_ne!(
        region,
        parse("pppppppp.pad.Face(M ssssssss.sketch.Face1)").unwrap()
    );
}

#[test]
fn a_sketchs_name_in_any_other_form_or_whole_is_refused_saying_why() {
    // Design S4: `sketch` takes only Face1, the ID forms, and U and L, never M or G derived
    // (a sketch derives nothing); no other type names by ID; and a whole name that's a
    // sketch's element is refused as a reference, its elements being in no solid.
    let parse = |text: &str| Name::parse(text, &sweep_makers);
    for (text, column, why) in [
        (
            "ssssssss.sketch.Edge.6t7v8w9x",
            1,
            "ssssssss is a sketch: its elements are in no solid; name what a feature made of it",
        ),
        (
            "ssssssss.sketch.Face1",
            1,
            "ssssssss is a sketch: its elements are in no solid",
        ),
        (
            "pppppppp.pad.Face(G ssssssss.sketch.Edge3)",
            37,
            "a sketch names its curves and points by ID",
        ),
        (
            "pppppppp.pad.Face(G ssssssss.sketch.Face2)",
            37,
            "its only region `Face1`: no other number",
        ),
        (
            "pppppppp.pad.Face(G ssssssss.sketch.Face(G ssssssss.sketch.Edge.6t7v8w9x))",
            42,
            "a sketch derives nothing",
        ),
        (
            "pppppppp.pad.Face(G ssssssss.sketch.Face.6t7v8w9x)",
            37,
            "a sketch names its curves (`Edge.`) and points (`Vertex.`) by ID",
        ),
        (
            "pppppppp.pad.Face(G aaaaaaaa.box.Edge.6t7v8w9x)",
            38,
            "only a sketch's are by ID",
        ),
        (
            "pppppppp.pad.Face(G ssssssss.sketch.Edge.6t7v8w9)",
            42,
            "a sketch element's ID",
        ),
        (
            "pppppppp.pad.Face(G ssssssss.sketch.Edge.6T7V8W9X)",
            42,
            "a sketch element's ID",
        ),
    ] {
        let error = parse(text).unwrap_err();
        assert!(error.why.contains(why), "{text}: {error}");
        assert_eq!(error.column, column, "{text}: {error}");
    }
    // A sketch's name inside a feature's is a source: read.
    assert!(parse("pppppppp.pad.Face(G ssssssss.sketch.Edge.6t7v8w9x)").is_ok());
}
