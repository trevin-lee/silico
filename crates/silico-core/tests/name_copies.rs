//! M6.1: scheme 6's copy form (N10, docs/design/m2.7-naming.md; docs/design/m6.0-patterns.md,
//! section 3): `feature.type.Kind(C<instance> <name>)` for a copy of an element of a
//! feature's tool, `(S<instance> <name>)` for a copy of an element of the base solid. Every
//! name and every refusal's column here is written by hand from the grammar; a name read
//! must equal the one a map holds, so a reference finds its copy by identity.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;

use silico_core::document::FeatureId;
use silico_core::kernel::{
    ElementCounts, ElementFate, ElementKind, ElementRef, History, InputHistory,
};
use silico_core::naming::{
    Copied, ElementMap, Instance, Maker, Name, NameError, OpCode, PrimitiveKind, TypeWord, freecad,
};

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

/// The plate's features (docs/design/m6.0-patterns.md, section 2.2), and a box, `k7f2qx3m`.
/// Each name's type is read from its text (N8): these say only which features there are.
fn makers(feature: &FeatureId) -> Option<Maker> {
    match feature.as_str() {
        "s0000001" | "s0000003" | "s0000005" => Some(Maker::By(OpCode::Sketch)),
        "p0000001" => Some(Maker::By(OpCode::Pad)),
        "q0000001" | "q0000002" | "q0000003" => Some(Maker::By(OpCode::Pocket)),
        "t0000001" | "t0000003" => Some(Maker::By(OpCode::LinearPattern)),
        "t0000002" => Some(Maker::By(OpCode::PolarPattern)),
        "m0000001" | "m0000002" => Some(Maker::By(OpCode::Mirror)),
        "k7f2qx3m" => Some(Maker::Itself(PrimitiveKind::Box)),
        "cccccccc" => Some(Maker::By(OpCode::Cut)),
        _ => None,
    }
}

fn parse(text: &str) -> Result<Name, NameError> {
    Name::parse(text, &makers)
}

/// The corner hole's wall, as the pocket names it.
const WALL: &str = "q0000002.pocket.Face(G s0000003.sketch.Edge.cc000001)";

fn wall() -> Arc<Name> {
    Arc::new(parse(WALL).unwrap())
}

fn copy(
    feature: &str,
    operation: OpCode,
    kind: ElementKind,
    copied: Copied,
    offsets: &[u32],
    of: Arc<Name>,
) -> Name {
    Name::Copy {
        feature: id(feature),
        operation,
        kind,
        copied,
        instance: Instance::new(offsets).unwrap(),
        of,
    }
}

#[test]
fn an_instance_leaves_its_trailing_zeros_off_and_the_original_is_none() {
    let offsets = |offsets: &[u32]| Instance::new(offsets).map(|i| i.offsets().to_vec());
    assert_eq!(offsets(&[2]), Some(vec![2]));
    assert_eq!(offsets(&[2, 0]), Some(vec![2]));
    assert_eq!(offsets(&[0, 1]), Some(vec![0, 1]));
    assert_eq!(offsets(&[2, 1]), Some(vec![2, 1]));
    assert_eq!(offsets(&[u32::MAX]), Some(vec![u32::MAX]));
    assert_eq!(offsets(&[0]), None);
    assert_eq!(offsets(&[0, 0]), None);
    assert_eq!(offsets(&[]), None);
    assert_eq!(Instance::new(&[2, 0]).unwrap().to_string(), "2");
    assert_eq!(Instance::new(&[0, 1]).unwrap().to_string(), "0,1");
    // Instance (2) and instance (2, 0) are one instance, so a second direction added
    // renames no copy.
    assert_eq!(Instance::new(&[2]), Instance::new(&[2, 0]));
}

#[test]
fn each_copy_in_the_designs_table_reads_as_the_name_it_writes() {
    // N10's table and section 3's, each built by hand.
    let wall = wall();
    let grid_copy = |offsets: &[u32]| {
        copy(
            "t0000001",
            OpCode::LinearPattern,
            ElementKind::Face,
            Copied::Tool,
            offsets,
            Arc::clone(&wall),
        )
    };
    let top = Arc::new(parse("p0000001.pad.Face(G s0000001.sketch.Face1)").unwrap());
    let notch = Arc::new(parse("q0000001.pocket.Face(G s0000002.sketch.Edge.nb000001)").ok());
    // `s0000002` isn't among the makers: a name of a feature that isn't there is refused.
    assert!(notch.is_none());
    let front = Arc::new(parse("p0000001.pad.Face(G s0000001.sketch.Edge.na000001)").unwrap());
    let cases: Vec<(String, Name)> = vec![
        (
            format!("t0000001.linear_pattern.Face(C2 {WALL})"),
            grid_copy(&[2]),
        ),
        (
            format!("t0000001.linear_pattern.Face(C0,1 {WALL})"),
            grid_copy(&[0, 1]),
        ),
        (
            format!("t0000001.linear_pattern.Face(C2,1 {WALL})"),
            grid_copy(&[2, 1]),
        ),
        (
            "m0000002.mirror.Face(S1 p0000001.pad.Face(G s0000001.sketch.Edge.na000001))"
                .to_owned(),
            copy(
                "m0000002",
                OpCode::Mirror,
                ElementKind::Face,
                Copied::Solid,
                &[1],
                Arc::clone(&front),
            ),
        ),
        (
            format!("m0000002.mirror.Face(S1 t0000001.linear_pattern.Face(C2,1 {WALL}))"),
            copy(
                "m0000002",
                OpCode::Mirror,
                ElementKind::Face,
                Copied::Solid,
                &[1],
                Arc::new(grid_copy(&[2, 1])),
            ),
        ),
        (
            format!(
                "t0000001.linear_pattern.Edge(G p0000001.pad.Face(G s0000001.sketch.Face1), G t0000001.linear_pattern.Face(C2,1 {WALL}))"
            ),
            Name::Derived {
                feature: id("t0000001"),
                operation: OpCode::LinearPattern,
                kind: ElementKind::Edge,
                first: source(silico_core::naming::Relation::Generated, None, top),
                more: vec![source(
                    silico_core::naming::Relation::Generated,
                    None,
                    Arc::new(grid_copy(&[2, 1])),
                )],
            },
        ),
        (
            format!("t0000001.linear_pattern.Face(M1/2 t0000001.linear_pattern.Face(C2 {WALL}))"),
            Name::Derived {
                feature: id("t0000001"),
                operation: OpCode::LinearPattern,
                kind: ElementKind::Face,
                first: source(
                    silico_core::naming::Relation::Modified,
                    Some((1, 2)),
                    Arc::new(grid_copy(&[2])),
                ),
                more: Vec::new(),
            },
        ),
        (
            "t0000002.polar_pattern.Face(C3 q0000003.pocket.Face(G s0000005.sketch.Edge.ce000001))"
                .to_owned(),
            copy(
                "t0000002",
                OpCode::PolarPattern,
                ElementKind::Face,
                Copied::Tool,
                &[3],
                Arc::new(parse("q0000003.pocket.Face(G s0000005.sketch.Edge.ce000001)").unwrap()),
            ),
        ),
        // Of the base solid, any word it can hold: a primitive's, by number.
        (
            "t0000003.linear_pattern.Face(S2,1 k7f2qx3m.box.Face6)".to_owned(),
            copy(
                "t0000003",
                OpCode::LinearPattern,
                ElementKind::Face,
                Copied::Solid,
                &[2, 1],
                Arc::new(parse("k7f2qx3m.box.Face6").unwrap()),
            ),
        ),
        // The largest offset an instance holds.
        (
            format!("t0000001.linear_pattern.Face(C4294967295,4294967295 {WALL})"),
            grid_copy(&[u32::MAX, u32::MAX]),
        ),
        // A copy's duplicate, and a copy of a duplicate: read, and unstable in any map.
        (
            format!("t0000001.linear_pattern.Face(C2 {WALL})~1"),
            Name::Duplicate {
                name: Arc::new(grid_copy(&[2])),
                ordinal: std::num::NonZeroU32::MIN,
            },
        ),
        (
            "t0000001.linear_pattern.Face(C2 q0000002.pocket.Face3~2)".to_owned(),
            copy(
                "t0000001",
                OpCode::LinearPattern,
                ElementKind::Face,
                Copied::Tool,
                &[2],
                Arc::new(parse("q0000002.pocket.Face3~2").unwrap()),
            ),
        ),
    ];
    for (text, expected) in cases {
        let read = parse(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(read, expected, "{text}");
        assert_eq!(expected.to_string(), text);
        assert_eq!(read.to_string(), text);
    }
}

fn source(
    relation: silico_core::naming::Relation,
    place: Option<(u32, u32)>,
    name: Arc<Name>,
) -> silico_core::naming::Source {
    silico_core::naming::Source {
        relation,
        place: place.map(|(rank, size)| silico_core::naming::Place {
            rank: std::num::NonZeroU32::new(rank).unwrap(),
            size: std::num::NonZeroU32::new(size).unwrap(),
        }),
        position: silico_core::naming::Raw(None),
        name,
    }
}

#[test]
fn a_copy_that_isnt_one_is_refused_at_its_column_saying_why() {
    // `t0000001.linear_pattern.Face(` is 29 characters, so its letter is column 30, its
    // first offset 31; a `C`'s inner name begins at 33 after `C2 `, its type word at 42.
    let p = "t0000001.linear_pattern.Face(";
    let cases: Vec<(String, usize, &str)> = vec![
        // Every spelling an instance mustn't have.
        (format!("{p}C {WALL})"), 31, "expected an offset"),
        (format!("{p}S {WALL})"), 31, "expected an offset"),
        (format!("{p}C0 {WALL})"), 31, "trailing zeros are left off"),
        (format!("{p}S0 {WALL})"), 31, "trailing zeros are left off"),
        (
            format!("{p}C2,0 {WALL})"),
            33,
            "trailing zeros are left off",
        ),
        (
            format!("{p}S2,0 {WALL})"),
            33,
            "trailing zeros are left off",
        ),
        (
            format!("{p}C0,0 {WALL})"),
            33,
            "trailing zeros are left off",
        ),
        (format!("{p}C02 {WALL})"), 31, "without a leading zero"),
        (format!("{p}C2,01 {WALL})"), 33, "without a leading zero"),
        (format!("{p}C2, {WALL})"), 33, "expected an offset"),
        (format!("{p}C,2 {WALL})"), 31, "expected an offset"),
        (format!("{p}C-1 {WALL})"), 31, "expected an offset"),
        (format!("{p}C+1 {WALL})"), 31, "expected an offset"),
        (format!("{p}C4294967296 {WALL})"), 31, "past 4294967295"),
        (format!("{p}C1,4294967296 {WALL})"), 33, "past 4294967295"),
        (format!("{p}C2{WALL})"), 32, "\" \""),
        (format!("{p}C2  {WALL})"), 33, "a feature's ID"),
        (format!("{p}C2 {WALL}"), 86, "\")\""),
        // More offsets than the type word's directions, at the `,` that begins the extra.
        (format!("{p}C2,1,1 {WALL})"), 34, "at most 2 directions"),
        (format!("{p}S0,0,1 {WALL})"), 34, "at most 2 directions"),
        (
            format!("t0000002.polar_pattern.Face(C1,1 {WALL})"),
            31,
            "a polar_pattern's copy is placed along one direction",
        ),
        (
            format!("m0000001.mirror.Face(C1,1 {WALL})"),
            24,
            "a mirror's copy is placed along one direction",
        ),
        (
            format!("m0000001.mirror.Face(S0,1 {WALL})"),
            24,
            "a mirror's copy is placed along one direction",
        ),
        // A `C` is of a sweep's element; an `S` of anything but a sketch's.
        (
            format!("{p}C2 k7f2qx3m.box.Face6)"),
            42,
            "a copy marked `C` is of a tool's element, a pad's, pocket's, revolve's, or groove's, and a box's isn't one",
        ),
        (
            format!("{p}C2 cccccccc.cut.Face(G k7f2qx3m.box.Face6))"),
            42,
            "and a cut's isn't one",
        ),
        (
            format!("t0000003.linear_pattern.Face(C1 t0000001.linear_pattern.Face(C2 {WALL}))"),
            42,
            "and a linear_pattern's isn't one",
        ),
        (
            format!("{p}C2 s0000003.sketch.Edge.cc000001)"),
            42,
            "and a sketch's isn't one",
        ),
        (
            format!("{p}S2 s0000003.sketch.Edge.cc000001)"),
            42,
            "a sketch's elements are in no solid",
        ),
        (
            format!("{p}S2 s0000003.sketch.Face1)"),
            42,
            "a sketch's elements are in no solid",
        ),
    ];
    for (text, column, why) in cases {
        let error = match parse(&text) {
            Ok(name) => panic!("{text:?} read as {name}"),
            Err(error) => error,
        };
        assert!(error.why.contains(why), "{text:?}: {error}");
        assert_eq!(error.column, column, "{text:?}: {error}");
    }
}

#[test]
fn a_copy_under_a_word_that_copies_nothing_is_refused_in_the_designs_words() {
    // Section 3's example: a chamfer naming the corner hole's rim as copied by the pocket.
    // `q0000002.pocket.Edge(` is 21 characters: the letter is column 22.
    let text = format!("q0000002.pocket.Edge(C2 {WALL})");
    let error = parse(&text).unwrap_err();
    assert_eq!(
        error.to_string(),
        "a pocket copies nothing: only a linear_pattern, polar_pattern, or mirror names an element by its copy (`(C…)` or `(S…)`), at column 22"
    );
    // Each word that isn't one of the three, `C` and `S` alike, at the letter.
    for (text, column, word) in [
        (format!("p0000001.pad.Face(C2 {WALL})"), 19, "pad"),
        (
            "p0000001.pad.Face(S1 k7f2qx3m.box.Face6)".to_owned(),
            19,
            "pad",
        ),
        (
            "cccccccc.cut.Face(S1 k7f2qx3m.box.Face6)".to_owned(),
            19,
            "cut",
        ),
        (format!("cccccccc.fillet.Edge(C1 {WALL})"), 22, "fillet"),
        (
            "cccccccc.fuse.Face(S1 k7f2qx3m.box.Face6)".to_owned(),
            20,
            "fuse",
        ),
        (
            "cccccccc.common.Face(S1 k7f2qx3m.box.Face6)".to_owned(),
            22,
            "common",
        ),
        (
            "cccccccc.chamfer.Face(S1 k7f2qx3m.box.Face6)".to_owned(),
            23,
            "chamfer",
        ),
        (format!("q0000002.revolve.Face(C1 {WALL})"), 23, "revolve"),
        (format!("q0000002.groove.Face(C1 {WALL})"), 22, "groove"),
    ] {
        let error = parse(&text).unwrap_err();
        assert_eq!(
            error.why,
            format!(
                "a {word} copies nothing: only a linear_pattern, polar_pattern, or mirror names an element by its copy (`(C…)` or `(S…)`)"
            ),
            "{text}"
        );
        assert_eq!(error.column, column, "{text}");
    }
    // A primitive's word refuses any form in parentheses, as it did.
    let error = parse(&format!("k7f2qx3m.box.Face(C2 {WALL})")).unwrap_err();
    assert!(
        error.why.contains("a box makes its elements itself"),
        "{error}"
    );
}

#[test]
fn a_copys_type_is_read_from_its_text_whatever_the_feature_is_now() {
    // N8: a pattern that's now a pocket, under its ID, still reads its copies' names (and
    // they find nothing); a feature there's no such thing as is refused, at its ID.
    let text = format!("q0000001.linear_pattern.Face(C2 {WALL})");
    let read = parse(&text).unwrap();
    assert_eq!(read.type_word(), TypeWord::Operation(OpCode::LinearPattern));
    assert_eq!(read.to_string(), text);
    let error = parse(&format!("zzzzzzzz.linear_pattern.Face(C2 {WALL})")).unwrap_err();
    assert_eq!(error.column, 1, "{error}");
    assert!(error.why.contains("no feature zzzzzzzz"), "{error}");
    let error = parse("t0000001.linear_pattern.Face(C2 zzzzzzzz.pad.Face1)").unwrap_err();
    assert_eq!(error.column, 33, "{error}");
}

#[test]
fn copies_are_equal_only_when_written_alike_and_order_and_hash_as_they_compare() {
    let wall = wall();
    let at = |feature, operation, copied, offsets: &[u32]| {
        copy(
            feature,
            operation,
            ElementKind::Face,
            copied,
            offsets,
            Arc::clone(&wall),
        )
    };
    let names = [
        at("t0000001", OpCode::LinearPattern, Copied::Tool, &[2]),
        at("t0000001", OpCode::LinearPattern, Copied::Tool, &[2, 1]),
        at("t0000001", OpCode::LinearPattern, Copied::Tool, &[0, 1]),
        at("t0000001", OpCode::LinearPattern, Copied::Solid, &[2]),
        at("t0000001", OpCode::PolarPattern, Copied::Tool, &[2]),
        at("t0000003", OpCode::LinearPattern, Copied::Tool, &[2]),
        copy(
            "t0000001",
            OpCode::LinearPattern,
            ElementKind::Edge,
            Copied::Tool,
            &[2],
            Arc::clone(&wall),
        ),
        copy(
            "t0000001",
            OpCode::LinearPattern,
            ElementKind::Face,
            Copied::Tool,
            &[2],
            Arc::new(parse("q0000002.pocket.Face(G s0000003.sketch.Edge.cc000002)").unwrap()),
        ),
    ];
    // Every pair differs: as text, as names, and in a set of each.
    let texts: BTreeSet<String> = names.iter().map(ToString::to_string).collect();
    assert_eq!(texts.len(), names.len());
    let ordered: BTreeSet<&Name> = names.iter().collect();
    assert_eq!(ordered.len(), names.len());
    let hashed: std::collections::HashSet<&Name> = names.iter().collect();
    assert_eq!(hashed.len(), names.len());
    // The same copy, built twice and read, is one name, in every set.
    for name in &names {
        let again = parse(&name.to_string()).unwrap();
        assert_eq!(&again, name);
        assert!(ordered.contains(&again));
        assert!(hashed.contains(&again));
        assert_eq!(again.cmp(name), std::cmp::Ordering::Equal);
    }
    // A copy is no other kind of name, nor its original.
    assert_ne!(names[0], *wall);
}

#[test]
fn a_copy_names_its_pattern_first_then_its_originals_features() {
    let name = parse(&format!(
        "m0000002.mirror.Face(S1 t0000001.linear_pattern.Face(C2,1 {WALL}))"
    ))
    .unwrap();
    let features: Vec<&str> = name.features().iter().map(|f| f.as_str()).collect();
    assert_eq!(features, ["m0000002", "t0000001", "q0000002", "s0000003"]);
    assert_eq!(name.feature().as_str(), "m0000002");
    assert_eq!(name.kind(), ElementKind::Face);
}

#[test]
fn freecads_form_of_a_copy_is_its_originals_then_freecads_suffix_from_the_second() {
    // FreeCAD's postfix is `Data::indexSuffix` of the copy's place from 1: nothing for the
    // first copy, `_k` from the second (docs/ports/patterns.md, section 3: copy 1 of a
    // cylinder's side `Face1`, copy 2 `Face1;_2`, tags left off). A grid's place, which
    // FreeCAD flattens with a count no name holds, is written as Silico's offsets. The wall
    // as the pocket builds it, generated first from its sketch's circle (a name read from
    // text holds no OCCT position, so its form stops at its source).
    let wall = Arc::new(Name::Derived {
        feature: id("q0000002"),
        operation: OpCode::Pocket,
        kind: ElementKind::Face,
        first: silico_core::naming::Source {
            relation: silico_core::naming::Relation::Generated,
            place: None,
            position: silico_core::naming::Raw(std::num::NonZeroU32::new(1)),
            name: Arc::new(Name::Sketch {
                feature: id("s0000003"),
                kind: ElementKind::Edge,
                id: silico_core::document::ElementId::new("cc000001").unwrap(),
            }),
        },
        more: Vec::new(),
    });
    assert_eq!(wall.to_string(), WALL);
    let original = freecad::text(&wall);
    let at = |offsets: &[u32]| {
        freecad::text(&copy(
            "t0000001",
            OpCode::LinearPattern,
            ElementKind::Face,
            Copied::Tool,
            offsets,
            Arc::clone(&wall),
        ))
    };
    assert_eq!(original, "gcc000001;SKT;:G;XTR");
    assert_eq!(at(&[1]), original);
    assert_eq!(at(&[2]), format!("{original};_2"));
    assert_eq!(at(&[12]), format!("{original};_12"));
    assert_eq!(at(&[0, 1]), format!("{original};_0,1"));
    assert_eq!(at(&[2, 1]), format!("{original};_2,1"));
    let mirrored = Name::Copy {
        feature: id("m0000002"),
        operation: OpCode::Mirror,
        kind: ElementKind::Face,
        copied: Copied::Solid,
        instance: Instance::new(&[1]).unwrap(),
        of: Arc::new(parse("k7f2qx3m.box.Face6").unwrap()),
    };
    assert_eq!(freecad::text(&mirrored), "Face6");
}

#[test]
fn the_three_words_copy_and_bite_every_input_and_no_other_word_copies() {
    for op in OpCode::ALL {
        let directions = match op {
            OpCode::LinearPattern => Some(2),
            OpCode::PolarPattern | OpCode::Mirror => Some(1),
            _ => None,
        };
        assert_eq!(op.directions(), directions, "{op:?}");
        assert_eq!(
            op.sweeps(),
            matches!(
                op,
                OpCode::Pad | OpCode::Pocket | OpCode::Revolve | OpCode::Groove
            ),
            "{op:?}"
        );
    }
    for (op, word) in [
        (OpCode::LinearPattern, "linear_pattern"),
        (OpCode::PolarPattern, "polar_pattern"),
        (OpCode::Mirror, "mirror"),
    ] {
        assert_eq!(op.word(), word);
        assert!(TypeWord::all().any(|w| w == TypeWord::Operation(op)));
        for stage in [
            silico_core::naming::Stage::Sweep,
            silico_core::naming::Stage::Last,
        ] {
            for input in 0..4 {
                assert_eq!(
                    op.side(stage, input),
                    silico_core::naming::Side::Outside,
                    "{op:?} {stage:?} {input}"
                );
                assert!(op.bites(input));
            }
        }
    }
}

/// A sketch's one face, `s0000001.sketch.Face1`, and a pad of it whose sweep keeps it as
/// its bottom, `p0000001.pad.Face(M s0000001.sketch.Face1)`, and makes a second face no
/// history names, `p0000001.pad.Face2`, by its number, unstable.
fn pad() -> ElementMap {
    let sketch = ElementMap::by_number(
        id("s0000001"),
        OpCode::Sketch,
        ElementCounts {
            faces: 1,
            edges: 0,
            vertices: 0,
            ..ElementCounts::default()
        },
    );
    let history = History {
        inputs: vec![InputHistory {
            faces: vec![ElementFate {
                kept: Some(1),
                modified: Vec::new(),
                generated: Vec::new(),
            }],
            edges: Vec::new(),
            vertices: Vec::new(),
        }],
        result: ElementCounts {
            faces: 2,
            edges: 0,
            vertices: 0,
            ..ElementCounts::default()
        },
        ..History::default()
    };
    ElementMap::swept(id("p0000001"), OpCode::Pad, &sketch, &history)
}

fn face(index: u32) -> ElementRef {
    ElementRef {
        kind: ElementKind::Face,
        index,
    }
}

#[test]
fn a_copys_map_numbers_each_element_as_its_tool_and_wraps_each_name_once() {
    let pad = pad();
    assert_eq!(
        pad.name(face(1)).unwrap().to_string(),
        "p0000001.pad.Face(M s0000001.sketch.Face1)"
    );
    assert_eq!(pad.name(face(2)).unwrap().to_string(), "p0000001.pad.Face2");
    assert!(pad.is_unstable(&pad.name(face(2)).unwrap()));
    assert!(!pad.is_unstable(&pad.name(face(1)).unwrap()));
    let instance = Instance::new(&[2, 1]).unwrap();
    let copies = ElementMap::copy(
        &pad,
        id("t0000001"),
        OpCode::LinearPattern,
        Copied::Tool,
        &instance,
    )
    .unwrap();
    assert_eq!(copies.feature().as_str(), "t0000001");
    assert_eq!(copies.made(), TypeWord::Operation(OpCode::LinearPattern));
    let written: Vec<(ElementRef, String)> = copies
        .iter()
        .flat_map(|(element, names)| names.iter().map(move |n| (element, n.to_string())))
        .collect();
    assert_eq!(
        written,
        [
            (
                face(1),
                "t0000001.linear_pattern.Face(C2,1 p0000001.pad.Face(M s0000001.sketch.Face1))"
                    .to_owned()
            ),
            (
                face(2),
                "t0000001.linear_pattern.Face(C2,1 p0000001.pad.Face2)".to_owned()
            ),
        ]
    );
    // Each name finds its element, read back as text.
    let pad_makers = |feature: &FeatureId| match feature.as_str() {
        "s0000001" => Some(Maker::By(OpCode::Sketch)),
        "p0000001" => Some(Maker::By(OpCode::Pad)),
        "t0000001" => Some(Maker::By(OpCode::LinearPattern)),
        _ => None,
    };
    for (element, text) in &written {
        let read = Name::parse(text, &pad_makers).unwrap();
        assert_eq!(copies.element(&read), Some(*element), "{text}");
    }
    // A copy of an unstable name is unstable; of a stable one, stable.
    let stable = copies.name(face(1)).unwrap();
    let unstable = copies.name(face(2)).unwrap();
    assert!(!copies.is_unstable(&stable));
    assert!(copies.is_unstable(&unstable));
    assert_eq!(copies.unstable().collect::<Vec<_>>(), [&unstable]);
}

#[test]
fn a_step_keeping_a_copy_keeps_its_name_and_its_instability() {
    // The pattern's step bites every input (N10): a copy kept whole keeps its `C` name, and
    // one unstable in the copy's map is unstable in the step's.
    let pad = pad();
    let instance = Instance::new(&[2]).unwrap();
    let copies = ElementMap::copy(
        &pad,
        id("t0000001"),
        OpCode::LinearPattern,
        Copied::Tool,
        &instance,
    )
    .unwrap();
    let kept = |n: u32| ElementFate {
        kept: Some(n),
        modified: Vec::new(),
        generated: Vec::new(),
    };
    // The base is the pad itself; the step keeps its two faces as 1 and 2, the copy's as 3
    // and 4.
    let history = History {
        inputs: vec![
            InputHistory {
                faces: vec![kept(1), kept(2)],
                edges: Vec::new(),
                vertices: Vec::new(),
            },
            InputHistory {
                faces: vec![kept(3), kept(4)],
                edges: Vec::new(),
                vertices: Vec::new(),
            },
        ],
        result: ElementCounts {
            faces: 4,
            edges: 0,
            vertices: 0,
            ..ElementCounts::default()
        },
        ..History::default()
    };
    let step = ElementMap::operation(
        id("t0000001"),
        OpCode::LinearPattern,
        &[&pad, &copies],
        &history,
    );
    let at = |n| step.name(face(n)).unwrap();
    assert_eq!(
        at(1).to_string(),
        "p0000001.pad.Face(M s0000001.sketch.Face1)"
    );
    assert_eq!(at(2).to_string(), "p0000001.pad.Face2");
    assert_eq!(
        at(3).to_string(),
        "t0000001.linear_pattern.Face(C2 p0000001.pad.Face(M s0000001.sketch.Face1))"
    );
    assert_eq!(
        at(4).to_string(),
        "t0000001.linear_pattern.Face(C2 p0000001.pad.Face2)"
    );
    assert!(!step.is_unstable(&at(1)));
    assert!(step.is_unstable(&at(2)));
    assert!(!step.is_unstable(&at(3)));
    assert!(step.is_unstable(&at(4)));
}

#[test]
fn no_map_is_copied_where_no_step_makes_such_a_copy() {
    let pad = pad();
    let cube = ElementMap::primitive(
        id("k7f2qx3m"),
        &silico_core::kernel::Primitive::Box {
            dx: 1.0,
            dy: 1.0,
            dz: 1.0,
        },
    );
    let sketch = ElementMap::by_number(
        id("s0000001"),
        OpCode::Sketch,
        ElementCounts {
            faces: 1,
            edges: 0,
            vertices: 0,
            ..ElementCounts::default()
        },
    );
    let cut = ElementMap::by_number(
        id("cccccccc"),
        OpCode::Cut,
        ElementCounts {
            faces: 1,
            edges: 0,
            vertices: 0,
            ..ElementCounts::default()
        },
    );
    let one = Instance::new(&[1]).unwrap();
    let two = Instance::new(&[0, 1]).unwrap();
    let three = Instance::new(&[0, 0, 1]).unwrap();
    let t = || id("t0000001");
    // An operation that copies nothing.
    for op in [OpCode::Fuse, OpCode::Pad, OpCode::Pocket, OpCode::Fillet] {
        assert!(
            ElementMap::copy(&pad, t(), op, Copied::Tool, &one).is_none(),
            "{op:?}"
        );
        assert!(
            ElementMap::copy(&cube, t(), op, Copied::Solid, &one).is_none(),
            "{op:?}"
        );
    }
    // More offsets than directions.
    assert!(ElementMap::copy(&pad, t(), OpCode::LinearPattern, Copied::Tool, &two).is_some());
    assert!(ElementMap::copy(&pad, t(), OpCode::LinearPattern, Copied::Tool, &three).is_none());
    for op in [OpCode::PolarPattern, OpCode::Mirror] {
        assert!(
            ElementMap::copy(&pad, t(), op, Copied::Tool, &one).is_some(),
            "{op:?}"
        );
        assert!(
            ElementMap::copy(&pad, t(), op, Copied::Tool, &two).is_none(),
            "{op:?}"
        );
    }
    // A tool's copy of what isn't a sweep's; a solid's copy of a sketch's faces.
    let linear = OpCode::LinearPattern;
    assert!(ElementMap::copy(&cube, t(), linear, Copied::Tool, &one).is_none());
    assert!(ElementMap::copy(&cut, t(), linear, Copied::Tool, &one).is_none());
    assert!(ElementMap::copy(&sketch, t(), linear, Copied::Tool, &one).is_none());
    assert!(ElementMap::copy(&sketch, t(), linear, Copied::Solid, &one).is_none());
    // A solid's copy of any solid's map.
    for map in [&pad, &cube, &cut] {
        assert!(ElementMap::copy(map, t(), linear, Copied::Solid, &one).is_some());
    }
}

#[test]
fn a_copy_past_the_depth_bound_is_left_for_the_step_to_number_and_the_parser_agrees() {
    // A box's face copied whole 255 times is 256 names deep, the most a name may be (N10:
    // a copy counts a level): copied once more, its name would be 257, so the copy's map
    // leaves it out, never naming it by the tool's number, which every copy would share.
    let mut map = ElementMap::primitive(
        id("k7f2qx3m"),
        &silico_core::kernel::Primitive::Box {
            dx: 1.0,
            dy: 1.0,
            dz: 1.0,
        },
    );
    let one = Instance::new(&[1]).unwrap();
    for _ in 0..255 {
        map = ElementMap::copy(&map, id("m0000001"), OpCode::Mirror, Copied::Solid, &one).unwrap();
    }
    let deepest = map.name(face(6)).unwrap();
    let text = deepest.to_string();
    assert!(text.starts_with("m0000001.mirror.Face(S1 m0000001.mirror.Face(S1 "));
    assert_eq!(text.matches("(S1 ").count(), 255);
    assert!(text.ends_with(&format!("k7f2qx3m.box.Face6{}", ")".repeat(255))));
    // The parser reads it, 256 deep, as the map holds it.
    assert_eq!(parse(&text).unwrap(), *deepest);
    let past = ElementMap::copy(&map, id("m0000001"), OpCode::Mirror, Copied::Solid, &one).unwrap();
    assert!(past.iter().all(|(_, names)| names.is_empty()) || past.iter().next().is_none());
    assert_eq!(past.name(face(6)), None);
    assert_eq!(past.count(ElementKind::Face), 0);
    // Nor does the parser read it: one level deeper than any reference may be.
    let error = parse(&format!("m0000001.mirror.Face(S1 {text})")).unwrap_err();
    assert!(error.why.contains("nested more than 256"), "{error}");
}

/// A plate padded from a square, a hole pocketed in it, and a chamfer naming `edge`, under
/// naming scheme `scheme`.
fn chamfered(scheme: u32, edge: &str) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": {scheme}, "features": [
{{"id": "s0000001", "type": "sketch", "v": 1, "name": "Plate", "plane": "xy", "points": [{{"id": "pa000001", "x": 0, "y": 0, "construction": false}}, {{"id": "pa000002", "x": 10, "y": 0, "construction": false}}, {{"id": "pa000003", "x": 10, "y": 10, "construction": false}}, {{"id": "pa000004", "x": 0, "y": 10, "construction": false}}], "lines": [{{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}}, {{"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false}}, {{"id": "na000003", "from": "pa000003", "to": "pa000004", "construction": false}}, {{"id": "na000004", "from": "pa000004", "to": "pa000001", "construction": false}}], "circles": [], "arcs": [], "constraints": []}},
{{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", "sketch": "s0000001", "extent": {{"type": "one_side", "length": 5, "reversed": false}}}},
{{"id": "s0000003", "type": "sketch", "v": 1, "name": "Hole", "plane": "xy", "points": [{{"id": "pc000001", "x": 5, "y": 5, "construction": false}}], "lines": [], "circles": [{{"id": "ca000001", "center": "pc000001", "radius": 1, "construction": false}}], "arcs": [], "constraints": []}},
{{"id": "q0000002", "type": "pocket", "v": 1, "name": "Pocket", "base": "p0000001", "sketch": "s0000003", "extent": {{"type": "through_all", "reversed": true}}}},
{{"id": "c0000001", "type": "chamfer", "v": 1, "name": "Chamfer", "base": "q0000002", "edges": [{edge:?}], "size": {{"distance": 0.5}}}}]}}"#
    )
}

#[test]
fn the_gate_refuses_a_copy_under_a_word_that_copies_nothing_in_the_designs_words() {
    // docs/design/m6.0-patterns.md, section 3: a chamfer naming the corner hole's rim as
    // copied by the pocket, not by a pattern; its message and suggestion, byte for byte.
    let edge = "q0000002.pocket.Edge(C2 q0000002.pocket.Face(G s0000003.sketch.Edge.ca000001))";
    for scheme in [5, 6] {
        let document = silico_core::document::read(chamfered(scheme, edge).as_bytes()).unwrap();
        let invalid = silico_core::document::Checked::new(document).unwrap_err();
        assert_eq!(invalid.errors().len(), 1, "{:?}", invalid.errors());
        let error = &invalid.errors()[0];
        assert_eq!(
            error.code(),
            silico_core::ErrorCode::InvalidInput,
            "{error}"
        );
        assert_eq!(error.feature(), Some("c0000001"));
        assert_eq!(error.parameter(), Some("edges.0"));
        assert_eq!(
            error.message(),
            "edges.0, \"q0000002.pocket.Edge(C2 q0000002.pocket.\"…, isn't an edge's name: a pocket copies nothing: only a linear_pattern, polar_pattern, or mirror names an element by its copy (`(C…)` or `(S…)`), at column 22"
        );
        assert_eq!(
            error.suggestion(),
            Some(
                "write the edge's name as Silico names it (feature.type.Edge(...)), of a feature before this one"
            )
        );
    }
}

#[test]
fn a_copys_name_passes_the_gate_under_either_scheme_and_finds_nothing_loudly() {
    // N10: a copy's name reads by its text, under a header of 5 as of 6 (Fable, 2026-11-10);
    // no feature makes a copy yet, so the reference breaks, with candidates, never
    // re-pointed.
    let edge =
        "q0000002.linear_pattern.Edge(C2 q0000002.pocket.Face(G s0000003.sketch.Edge.ca000001))";
    let engine =
        silico_core::recompute::Recompute::new(silico_core::fake::FakeKernel::new(2).unwrap())
            .with_sketches(silico_core::fake::FakeSketches::new());
    for scheme in [5, 6] {
        let document = silico_core::document::read(chamfered(scheme, edge).as_bytes()).unwrap();
        let checked = silico_core::document::Checked::new(document).unwrap();
        let Ok(silico_core::recompute::RunOutcome::Published(model)) = engine.start(checked).wait()
        else {
            panic!("not published");
        };
        let errors: Vec<_> = model.errors().collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        let error = errors[0];
        assert_eq!(error.feature(), Some("c0000001"), "{error}");
        assert_eq!(
            error.code(),
            silico_core::ErrorCode::ReferenceBroken,
            "{error}"
        );
        // N8: the name says a linear_pattern made it, and q0000002 is a pocket.
        assert!(
            error
                .message()
                .contains("names an element a linear_pattern made, but q0000002 is a pocket now"),
            "{error}"
        );
        assert!(!error.candidates().is_empty(), "{error}");
    }
}

#[test]
fn the_gate_refuses_a_copy_of_an_unstable_name_as_any_unstable_name() {
    // N10: a copy of an unstable name is unstable, so a stored reference to one is refused
    // (M4.1, section 2), whether what it copies is an operation's element by its number or
    // a duplicate; a copy of a stable name passes.
    for (edge, refused) in [
        (
            "q0000002.linear_pattern.Edge(C2 q0000002.pocket.Edge3)",
            true,
        ),
        (
            "q0000002.linear_pattern.Edge(C2 q0000002.pocket.Edge(G s0000003.sketch.Edge.ca000001)~1)",
            true,
        ),
        ("q0000002.mirror.Edge(S1 q0000002.pocket.Edge3)", true),
        (
            "q0000002.linear_pattern.Edge(C2 q0000002.pocket.Edge(G s0000003.sketch.Edge.ca000001))",
            false,
        ),
    ] {
        let document = silico_core::document::read(chamfered(6, edge).as_bytes()).unwrap();
        match silico_core::document::Checked::new(document) {
            Ok(_) => assert!(!refused, "{edge} passed"),
            Err(invalid) => {
                assert_eq!(invalid.errors().len(), 1, "{edge}: {:?}", invalid.errors());
                let error = &invalid.errors()[0];
                assert!(refused, "{edge}: {error}");
                assert_eq!(error.parameter(), Some("edges.0"), "{edge}");
                assert!(
                    error
                        .message()
                        .contains("names an edge by a name that doesn't say which"),
                    "{edge}: {error}"
                );
            }
        }
    }
}

#[test]
fn a_copy_counts_a_level_on_top_of_its_names_duplicates_and_under_its_own() {
    // The parser's height counts a copy's level whatever its inner name's height is made
    // of (the review: only copies of copies were nested at the bound, which the depth
    // argument refuses alone). The wall is 2 high; 254 `~1`s make it 256; a copy of it is
    // 257, past the bound. And 254 `~1`s on a copy of the wall (3 high) make 257 too.
    let high = format!("{WALL}{}", "~1".repeat(254));
    assert!(parse(&high).is_ok());
    let error = parse(&format!("t0000001.linear_pattern.Face(C2 {high})")).unwrap_err();
    assert!(error.why.contains("nested more than 256"), "{error}");
    let at = format!("{WALL}{}", "~1".repeat(253));
    assert!(parse(&format!("t0000001.linear_pattern.Face(C2 {at})")).is_ok());
    let copy = format!("t0000001.linear_pattern.Face(C2 {WALL})");
    assert!(parse(&format!("{copy}{}", "~1".repeat(253))).is_ok());
    let error = parse(&format!("{copy}{}", "~1".repeat(254))).unwrap_err();
    assert!(error.why.contains("nested more than 256"), "{error}");
}

#[test]
fn a_pattern_steps_freecad_op_code_is_fus_a_cut_steps_too() {
    // N10's open item, settled by M6.3: a pattern's or mirror's every step writes `FUS` in
    // FreeCAD's form, its cut step's too, as a sweep feature writes `XTR` for its fuse or cut
    // (S4: one word names every step of the feature), where FreeCAD 1.1 writes `CUT` for a
    // subtractive tool's copies (`FeatureTransformed.cpp:399` at `af2d42a4aa`). A pocket's
    // copy cut into a base, its face split in two by the base: each piece's name, a source
    // of the copy's face, writes `FUS`, never `CUT`.
    for op in [OpCode::LinearPattern, OpCode::PolarPattern, OpCode::Mirror] {
        assert_eq!(op.code(), "FUS", "{op:?}");
    }
    let base = ElementMap::primitive(
        id("k7f2qx3m"),
        &silico_core::kernel::Primitive::Box {
            dx: 1.0,
            dy: 1.0,
            dz: 1.0,
        },
    );
    let wall = ElementMap::by_number(
        id("q0000002"),
        OpCode::Pocket,
        ElementCounts {
            faces: 1,
            ..ElementCounts::default()
        },
    );
    let copy = ElementMap::copy(
        &wall,
        id("t0000001"),
        OpCode::LinearPattern,
        Copied::Tool,
        &Instance::new(&[2]).unwrap(),
    )
    .unwrap();
    let kept = |n: u32| ElementFate {
        kept: Some(n),
        modified: Vec::new(),
        generated: Vec::new(),
    };
    let piece = |n: u32, position: u32| silico_core::kernel::Reported {
        element: face(n),
        position,
    };
    let history = History {
        inputs: vec![
            InputHistory {
                faces: (1..=6).map(kept).collect(),
                edges: Vec::new(),
                vertices: Vec::new(),
            },
            InputHistory {
                faces: vec![ElementFate {
                    kept: None,
                    modified: vec![piece(7, 1), piece(8, 2)],
                    generated: Vec::new(),
                }],
                edges: Vec::new(),
                vertices: Vec::new(),
            },
        ],
        result: ElementCounts {
            faces: 8,
            ..ElementCounts::default()
        },
        ..History::default()
    };
    let map = ElementMap::operation(
        id("t0000001"),
        OpCode::LinearPattern,
        &[&base, &copy],
        &history,
    );
    for n in [7, 8] {
        let name = map.name(face(n)).unwrap();
        assert!(
            name.to_string()
                .starts_with("t0000001.linear_pattern.Face(M"),
            "{name}"
        );
        let text = freecad::text(&name);
        assert!(text.contains(";FUS") && !text.contains("CUT"), "{text}");
    }
}

#[test]
fn a_copys_element_past_the_bound_is_numbered_by_its_place_in_the_steps_result() {
    // N10's open item, settled by M6.3: a copy's map leaves out an element whose every name
    // is past the bound (above); the step's map numbers it, and what's made from it, by its
    // place in the step's result, never by the tool's number, which every copy of it shares.
    // A box copied 255 times as a mirror's whole solid is at the bound; its next copy has no
    // names. The step keeps that copy's faces 1 to 5 as 7 to 11 and splits its face 6 into
    // 12 and 13: each is named by its result's number, unstable.
    let mut map = ElementMap::primitive(
        id("k7f2qx3m"),
        &silico_core::kernel::Primitive::Box {
            dx: 1.0,
            dy: 1.0,
            dz: 1.0,
        },
    );
    let one = Instance::new(&[1]).unwrap();
    for _ in 0..255 {
        map = ElementMap::copy(&map, id("m0000001"), OpCode::Mirror, Copied::Solid, &one).unwrap();
    }
    let past = ElementMap::copy(&map, id("m0000001"), OpCode::Mirror, Copied::Solid, &one).unwrap();
    assert_eq!(past.count(ElementKind::Face), 0);
    let kept = |n: u32| ElementFate {
        kept: Some(n),
        modified: Vec::new(),
        generated: Vec::new(),
    };
    let piece = |n: u32, position: u32| silico_core::kernel::Reported {
        element: face(n),
        position,
    };
    let mut copied: Vec<ElementFate> = (7..=11).map(kept).collect();
    copied.push(ElementFate {
        kept: None,
        modified: vec![piece(12, 1), piece(13, 2)],
        generated: Vec::new(),
    });
    let history = History {
        inputs: vec![
            InputHistory {
                faces: (1..=6).map(kept).collect(),
                edges: Vec::new(),
                vertices: Vec::new(),
            },
            InputHistory {
                faces: copied,
                edges: Vec::new(),
                vertices: Vec::new(),
            },
        ],
        result: ElementCounts {
            faces: 13,
            ..ElementCounts::default()
        },
        ..History::default()
    };
    let step = ElementMap::operation(id("m0000001"), OpCode::Mirror, &[&map, &past], &history);
    // The base's faces keep their names.
    for n in 1..=6 {
        assert_eq!(step.name(face(n)), map.name(face(n)), "Face{n}");
    }
    // The copy's, each its own number in the result.
    for n in 7..=13 {
        let name = step.name(face(n)).unwrap();
        assert_eq!(name.to_string(), format!("m0000001.mirror.Face{n}"));
        assert!(step.is_unstable(&name), "Face{n}");
        assert_eq!(step.names(face(n)).len(), 1, "Face{n}");
    }
    // None is a tool's number, as each copy of a face would share: Face1 to Face6 name the
    // base's faces only, as the base's deep names, never a copy's.
    for n in 1..=6 {
        let tool = parse(&format!("m0000001.mirror.Face{n}")).unwrap();
        assert_eq!(step.element(&tool), None, "Face{n}");
    }
}

#[test]
fn a_copy_sorts_after_every_other_kind_of_name_and_equals_none() {
    let wall = wall();
    let copy = copy(
        "t0000001",
        OpCode::LinearPattern,
        ElementKind::Face,
        Copied::Tool,
        &[2],
        Arc::clone(&wall),
    );
    let others = [
        "k7f2qx3m.box.Face6",
        WALL,
        "q0000002.pocket.Edge(U q0000002.pocket.Face(G s0000003.sketch.Edge.cc000001))",
        "q0000002.pocket.Face(L q0000002.pocket.Edge3)",
        "q0000002.pocket.Face3~1",
    ];
    for text in others {
        let other = parse(text).unwrap();
        assert_eq!(other.cmp(&copy), std::cmp::Ordering::Less, "{text}");
        assert_ne!(other, copy, "{text}");
    }
    // A sketch's element sorts before a copy too.
    let sketch = Name::Sketch {
        feature: id("s0000003"),
        kind: ElementKind::Edge,
        id: silico_core::document::ElementId::new("cc000001").unwrap(),
    };
    assert_eq!(sketch.cmp(&copy), std::cmp::Ordering::Less);
}

#[test]
fn a_tools_copy_of_a_sweeps_name_from_above_or_below_reads_and_of_a_cuts_is_refused() {
    // `type_word` of a name from above (`U`) or below (`L`) is its operation's, so a tool's
    // copy of a pocket's edge named from above reads, and of a cut's is refused, at the
    // inner name's type word (column 42).
    let p = "t0000001.linear_pattern.Edge(";
    let upper = "q0000002.pocket.Edge(U q0000002.pocket.Face(G s0000003.sketch.Edge.cc000001))";
    let text = format!("{p}C2 {upper})");
    assert_eq!(parse(&text).unwrap().to_string(), text);
    let lower = "q0000002.pocket.Face(L q0000002.pocket.Edge3)";
    let text = format!("t0000001.linear_pattern.Face(C2 {lower})");
    assert_eq!(parse(&text).unwrap().to_string(), text);
    for inner in [
        "cccccccc.cut.Edge(U k7f2qx3m.box.Face6)",
        "cccccccc.cut.Edge(L k7f2qx3m.box.Vertex1)",
    ] {
        let error = parse(&format!("{p}C2 {inner})")).unwrap_err();
        assert_eq!(error.column, 42, "{inner}: {error}");
        assert!(
            error.why.contains("and a cut's isn't one"),
            "{inner}: {error}"
        );
    }
}

/// A pad's last step on `base`, whose six faces it keeps as 1 to 6 under their own names,
/// and the pad's sweep's two faces ([`pad`]) as 7 and 8.
fn pad_on(base: &ElementMap) -> ElementMap {
    let kept = |n: u32| ElementFate {
        kept: Some(n),
        modified: Vec::new(),
        generated: Vec::new(),
    };
    let history = History {
        inputs: vec![
            InputHistory {
                faces: (1..=6).map(kept).collect(),
                edges: Vec::new(),
                vertices: Vec::new(),
            },
            InputHistory {
                faces: vec![kept(7), kept(8)],
                edges: Vec::new(),
                vertices: Vec::new(),
            },
        ],
        result: ElementCounts {
            faces: 8,
            edges: 0,
            vertices: 0,
            ..ElementCounts::default()
        },
        ..History::default()
    };
    ElementMap::operation(id("p0000001"), OpCode::Pad, &[base, &pad()], &history)
}

#[test]
fn a_tools_copy_given_a_sweep_features_last_map_leaves_out_its_bases_names() {
    // The filter's fallback, not the map M6.3 gives a tool's copy (the sweep step's, N10's
    // open item): a pad's last step keeps its base's names, which no tool's copy may wrap
    // (`Copied::wraps`), whether the base's word is a primitive's (a box's) or an
    // operation's that isn't a sweep's (a cut's); the parser refuses `(C1
    // k7f2qx3m.box.Face1)` and `(C1 cccccccc.cut.Face1)` alike. The pad's own are wrapped,
    // and every name kept reads back.
    let cube = ElementMap::primitive(
        id("k7f2qx3m"),
        &silico_core::kernel::Primitive::Box {
            dx: 1.0,
            dy: 1.0,
            dz: 1.0,
        },
    );
    let cut = ElementMap::by_number(
        id("cccccccc"),
        OpCode::Cut,
        ElementCounts {
            faces: 6,
            edges: 0,
            vertices: 0,
            ..ElementCounts::default()
        },
    );
    let pad_makers = |feature: &FeatureId| match feature.as_str() {
        "s0000001" => Some(Maker::By(OpCode::Sketch)),
        "p0000001" => Some(Maker::By(OpCode::Pad)),
        "t0000001" => Some(Maker::By(OpCode::LinearPattern)),
        "k7f2qx3m" => Some(Maker::Itself(PrimitiveKind::Box)),
        "cccccccc" => Some(Maker::By(OpCode::Cut)),
        _ => None,
    };
    let one = Instance::new(&[1]).unwrap();
    for (base, first) in [(&cube, "k7f2qx3m.box.Face1"), (&cut, "cccccccc.cut.Face1")] {
        let last = pad_on(base);
        assert_eq!(last.made(), TypeWord::Operation(OpCode::Pad));
        assert_eq!(last.name(face(1)).unwrap().to_string(), first);
        let copies = ElementMap::copy(
            &last,
            id("t0000001"),
            OpCode::LinearPattern,
            Copied::Tool,
            &one,
        )
        .unwrap();
        for n in 1..=6 {
            assert_eq!(copies.name(face(n)), None, "{first}: Face{n}");
        }
        assert_eq!(
            copies.name(face(7)).unwrap().to_string(),
            "t0000001.linear_pattern.Face(C1 p0000001.pad.Face(M s0000001.sketch.Face1))"
        );
        assert_eq!(
            copies.name(face(8)).unwrap().to_string(),
            "t0000001.linear_pattern.Face(C1 p0000001.pad.Face2)"
        );
        for (_, names) in copies.iter() {
            for name in names {
                assert!(
                    Name::parse(&name.to_string(), &pad_makers).is_ok(),
                    "{name}"
                );
            }
        }
        // A solid's copy of the same map wraps every name, the base's among them.
        let whole =
            ElementMap::copy(&last, id("m0000001"), OpCode::Mirror, Copied::Solid, &one).unwrap();
        assert_eq!(
            whole.name(face(1)).unwrap().to_string(),
            format!("m0000001.mirror.Face(S1 {first})")
        );
    }
}

#[test]
fn only_a_pattern_steps_own_copies_after_its_base_are_numbered_by_their_place_in_the_result() {
    // `Builder::is_copy`: a map leaving an element unnamed is numbered by the step's result
    // only where it's a copy the step places, an input after its base, of a pattern's or
    // mirror's own feature. Any other input's unnamed element keeps its own number, by its own
    // map's feature and type, as every input's has. A box copied past the bound (255 whole
    // copies, then one more) leaves its six faces unnamed; each step keeps them.
    let mut map = ElementMap::primitive(
        id("k7f2qx3m"),
        &silico_core::kernel::Primitive::Box {
            dx: 1.0,
            dy: 1.0,
            dz: 1.0,
        },
    );
    let one = Instance::new(&[1]).unwrap();
    for _ in 0..255 {
        map = ElementMap::copy(&map, id("m0000001"), OpCode::Mirror, Copied::Solid, &one).unwrap();
    }
    let past = ElementMap::copy(&map, id("m0000001"), OpCode::Mirror, Copied::Solid, &one).unwrap();
    let kept = |n: u32| ElementFate {
        kept: Some(n),
        modified: Vec::new(),
        generated: Vec::new(),
    };
    // Input 0's six faces kept as `first` to `first + 5`, input 1's as `second` on.
    let history = |first: u32, second: u32| History {
        inputs: [first, second]
            .map(|from| InputHistory {
                faces: (from..from + 6).map(kept).collect(),
                edges: Vec::new(),
                vertices: Vec::new(),
            })
            .to_vec(),
        result: ElementCounts {
            faces: 12,
            ..ElementCounts::default()
        },
        ..History::default()
    };
    let named = |step: &ElementMap, n: u32| step.name(face(n)).unwrap().to_string();
    // A fuse places no copies: the unnamed faces keep the mirror's own numbers.
    let fuse = ElementMap::operation(id("f0000001"), OpCode::Fuse, &[&map, &past], &history(1, 7));
    assert_eq!(named(&fuse, 7), "m0000001.mirror.Face1");
    // Another pattern's step, given the mirror's copy: not its own, so likewise.
    let other = ElementMap::operation(
        id("t0000009"),
        OpCode::LinearPattern,
        &[&map, &past],
        &history(1, 7),
    );
    assert_eq!(named(&other, 7), "m0000001.mirror.Face1");
    // The mirror's own step: its base (input 0) is no copy, so its unnamed faces keep their
    // numbers too, kept here as 7 to 12.
    let own = ElementMap::operation(
        id("m0000001"),
        OpCode::Mirror,
        &[&past, &past],
        &history(7, 1),
    );
    assert_eq!(named(&own, 7), "m0000001.mirror.Face1");
}

#[test]
fn a_past_bound_copys_element_numbers_no_solid_and_nothing_named_already() {
    // `Builder::derived`, of a copy's element its map leaves unnamed (past the bound): only
    // what it made that's still unnamed, and no solid, is numbered by its place in the
    // step's result. The box's 256th whole copy (above) has no names; the step keeps the
    // base's faces 1 to 6 and the copy's 1 to 5 as 7 to 11, and the copy's face 6 becomes
    // 12, a piece of the base's face 1 (named already, as the base's), and a whole solid
    // (as an extrusion's history reports one). Face 12 is numbered; face 1 keeps the base's
    // one name, gaining none; and the solid is named by nothing, as every step leaves it.
    let mut map = ElementMap::primitive(
        id("k7f2qx3m"),
        &silico_core::kernel::Primitive::Box {
            dx: 1.0,
            dy: 1.0,
            dz: 1.0,
        },
    );
    let one = Instance::new(&[1]).unwrap();
    for _ in 0..255 {
        map = ElementMap::copy(&map, id("m0000001"), OpCode::Mirror, Copied::Solid, &one).unwrap();
    }
    let past = ElementMap::copy(&map, id("m0000001"), OpCode::Mirror, Copied::Solid, &one).unwrap();
    let kept = |n: u32| ElementFate {
        kept: Some(n),
        modified: Vec::new(),
        generated: Vec::new(),
    };
    let made =
        |element: ElementRef, position: u32| silico_core::kernel::Reported { element, position };
    let solid = ElementRef {
        kind: ElementKind::Solid,
        index: 1,
    };
    let mut copied: Vec<ElementFate> = (7..=11).map(kept).collect();
    copied.push(ElementFate {
        kept: None,
        modified: vec![made(face(12), 1), made(face(1), 2)],
        generated: vec![made(solid, 1)],
    });
    let history = History {
        inputs: vec![
            InputHistory {
                faces: (1..=6).map(kept).collect(),
                edges: Vec::new(),
                vertices: Vec::new(),
            },
            InputHistory {
                faces: copied,
                edges: Vec::new(),
                vertices: Vec::new(),
            },
        ],
        result: ElementCounts {
            faces: 12,
            solids: 1,
            ..ElementCounts::default()
        },
        ..History::default()
    };
    let step = ElementMap::operation(id("m0000001"), OpCode::Mirror, &[&map, &past], &history);
    assert_eq!(
        step.name(face(12)).unwrap().to_string(),
        "m0000001.mirror.Face12"
    );
    assert_eq!(step.names(face(1)), map.names(face(1)));
    assert_eq!(step.names(face(1)).len(), 1);
    let base_face = parse("m0000001.mirror.Face1").unwrap();
    assert_eq!(step.element(&base_face), None);
    assert_eq!(step.name(solid), None);
    assert_eq!(step.count(ElementKind::Solid), 0);
}
