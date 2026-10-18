//! M2.8: a fillet's and a chamfer's edges, named in the document, found by name in their
//! base's solid on the fake kernel (docs/ports/references.md). The fake's booleans keep
//! every element of both inputs, the base's first, so which edge a name finds is worked out
//! by hand: in the example's cut, `k7f2qx3m.EdgeN` is edge N and `a91cw0de.EdgeN` is edge
//! 12 + N.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/model.rs"]
mod model;
#[path = "common/references.rs"]
mod references;

use model::{checked, described, id, made, published};
use references::{BLOCK, HEAD, HOLE, PIN, document, dressings, engine, fillet, names};
use silico_core::document::{Checked, read};
use silico_core::fake::FakeKernel;
use silico_core::kernel::Op;
use silico_core::naming::PrimitiveKind;
use silico_core::recompute::{FeatureOutcome, Recompute};
use silico_core::{Error, ErrorCode};

/// A chamfer of `edges` of `base`, `distance` back, as a document writes it.
pub fn chamfer(id: &str, base: &str, edges: &[&str], distance: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "chamfer", "v": 1, "name": "Bevel", "base": "{base}", "edges": [{}], "size": {{"distance": {distance}}}}}"#,
        names(edges)
    )
}

/// What an error says it's about: its code, feature, parameter, and elements.
type About<'a> = (ErrorCode, Option<&'a str>, Option<&'a str>, Vec<String>);

#[test]
fn a_fillets_edges_are_found_by_name_in_its_base() {
    let engine = engine();
    let text = document(&[&fillet(
        "f0000001",
        "m3tq8v0z",
        &["a91cw0de.cylinder.Edge2", "k7f2qx3m.box.Edge1"],
        "0.5",
    )]);
    let model = published(&engine, checked(&text));
    assert!(
        model.errors().next().is_none(),
        "{:?}",
        model.errors().collect::<Vec<_>>()
    );
    // In the name's order: the pin's Edge2 is the cut's 14th edge, the block's Edge1 its 1st.
    assert_eq!(dressings(engine.kernel()), [(vec![14, 1], 0.5)]);
    assert!(matches!(
        model.outcome(&id("f0000001")),
        Some(FeatureOutcome::Built { .. })
    ));
    // The fillet's own elements are named, from its history.
    let map = model.element_map(&id("f0000001")).unwrap();
    assert_eq!(map.iter().count(), 9 + 15 + 10);
}

#[test]
fn a_chamfers_edges_are_found_by_name_too() {
    let engine = engine();
    let text = document(&[&chamfer(
        "c0000001",
        "m3tq8v0z",
        &["k7f2qx3m.box.Edge12"],
        "1",
    )]);
    let model = published(&engine, checked(&text));
    assert!(model.errors().next().is_none());
    assert_eq!(dressings(engine.kernel()), [(vec![12], 1.0)]);
}

#[test]
fn a_fillet_of_a_fillet_finds_its_edges_through_the_first() {
    // The fake's fillet keeps every element, so the block's Edge3 is the first fillet's
    // edge 3 too.
    let engine = engine();
    let text = document(&[
        &fillet("f0000001", "m3tq8v0z", &["k7f2qx3m.box.Edge1"], "0.5"),
        &fillet("f0000002", "f0000001", &["k7f2qx3m.box.Edge3"], "0.25"),
    ]);
    let model = published(&engine, checked(&text));
    assert!(model.errors().next().is_none());
    assert_eq!(
        dressings(engine.kernel()),
        [(vec![1], 0.5), (vec![3], 0.25)]
    );
}

#[test]
fn every_name_that_finds_nothing_is_said_and_the_feature_fails_and_its_users_wait() {
    // N6: the block has no Edge13; the pin is no part of the block (only of the cut). Both
    // broken, said at once, each naming its place in `edges`; the fillet isn't asked for,
    // and a chamfer of the fillet is blocked on it.
    let engine = engine();
    let text = format!(
        "{HEAD}{BLOCK},\n{PIN},\n{},\n{}]}}",
        fillet(
            "f0000001",
            "k7f2qx3m",
            &[
                "k7f2qx3m.box.Edge2",
                "k7f2qx3m.box.Edge13",
                "a91cw0de.cylinder.Edge1"
            ],
            "0.5"
        ),
        chamfer("c0000001", "f0000001", &["k7f2qx3m.box.Edge2"], "0.1"),
    );
    let model = published(&engine, checked(&text));
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("f0000001")) else {
        panic!("{:?}", model.outcome(&id("f0000001")));
    };
    let said: Vec<About> = errors
        .iter()
        .map(|e| (e.code(), e.feature(), e.parameter(), e.elements().to_vec()))
        .collect();
    assert_eq!(
        said,
        [
            (
                ErrorCode::ReferenceBroken,
                Some("f0000001"),
                Some("edges.1"),
                vec!["k7f2qx3m.box.Edge13".to_owned()]
            ),
            (
                ErrorCode::ReferenceBroken,
                Some("f0000001"),
                Some("edges.2"),
                vec!["a91cw0de.cylinder.Edge1".to_owned()]
            ),
        ]
    );
    for error in errors {
        assert!(
            error
                .message()
                .contains("names no element of k7f2qx3m's solid"),
            "{error}"
        );
        assert!(error.suggestion().is_some());
    }
    assert!(dressings(engine.kernel()).is_empty());
    assert_eq!(
        model.outcome(&id("c0000001")),
        Some(&FeatureOutcome::Blocked { on: id("f0000001") })
    );
    // The model's errors: both, and nothing for the chamfer it blocked.
    let all: Vec<&Error> = model.errors().collect();
    assert_eq!(all.len(), 2);
}

#[test]
fn an_edit_that_takes_a_named_edge_from_the_base_breaks_the_reference_loudly() {
    // The fillet names the pin's Edge2, an edge of the hole. An edit makes the hole with
    // another tool: the pin is no longer in the cut, so the name finds nothing, and the
    // fillet fails, never taking another edge for it.
    let engine = engine();
    let round = fillet("f0000001", "m3tq8v0z", &["a91cw0de.cylinder.Edge2"], "0.5");
    let before = document(&[&round]);
    let model = published(&engine, checked(&before));
    assert!(model.errors().next().is_none());
    let drill = r#"{"id": "d0000001", "type": "cylinder", "v": 1, "name": "Drill", "position": [20, 10, -1], "radius": 2.5, "height": 12}"#;
    let after = format!(
        "{HEAD}{BLOCK},\n{PIN},\n{drill},\n{},\n{round}]}}",
        HOLE.replace(r#""tool": "a91cw0de""#, r#""tool": "d0000001""#)
    );
    let model = published(&engine, checked(&after));
    let errors: Vec<ErrorCode> = model.errors().map(Error::code).collect();
    assert_eq!(errors, [ErrorCode::ReferenceBroken]);
    assert_eq!(
        dressings(engine.kernel()).len(),
        1,
        "asked for once, before the edit"
    );
}

#[test]
fn a_fillet_rebuilt_from_the_cache_finds_the_same_edges_without_the_kernel() {
    let dir = std::env::temp_dir().join(format!("silico-references-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let secret = dir.join("secret");
    let text = document(&[&fillet(
        "f0000001",
        "m3tq8v0z",
        &["a91cw0de.cylinder.Edge2"],
        "0.5",
    )]);
    let cache = || silico_core::cache::Cache::with_secret(dir.join("cache"), &secret);
    let first = Recompute::with_cache(FakeKernel::new(2).unwrap(), cache());
    let built = published(&first, checked(&text));
    drop(first);
    let second = Recompute::with_cache(FakeKernel::new(2).unwrap(), cache());
    let again = published(&second, checked(&text));
    assert!(
        second.kernel().calls().is_empty(),
        "{:?}",
        second.kernel().calls()
    );
    assert_eq!(again.features(), built.features());
    let _ = std::fs::remove_dir_all(&dir);
}

/// What each of `kernel`'s dressings was: a fillet (false) or a chamfer (true).
fn chamfers(kernel: &FakeKernel) -> Vec<bool> {
    kernel
        .calls()
        .into_iter()
        .filter_map(|call| match call.op {
            Op::Fillet { .. } => Some(false),
            Op::Chamfer { .. } => Some(true),
            _ => None,
        })
        .collect()
}

#[test]
fn a_fillet_of_another_radius_or_a_chamfer_of_the_same_edges_is_another_step() {
    // Found by the tests review: a dressing's step key holds its size and its kind, so a
    // fillet's radius changed, or a fillet made a chamfer of the same edges, is asked of the
    // kernel again, never taken from the last result: in memory, and from the cache on disk.
    let edges = &["k7f2qx3m.box.Edge1"][..];
    let dressings_of = [
        fillet("f0000001", "m3tq8v0z", edges, "0.5"),
        fillet("f0000001", "m3tq8v0z", edges, "0.25"),
        chamfer("f0000001", "m3tq8v0z", edges, "0.25"),
    ];
    let engine = engine();
    for more in &dressings_of {
        let model = published(&engine, checked(&document(&[more])));
        assert!(model.errors().next().is_none(), "{more}");
    }
    assert_eq!(
        dressings(engine.kernel()),
        [(vec![1], 0.5), (vec![1], 0.25), (vec![1], 0.25)]
    );
    assert_eq!(chamfers(engine.kernel()), [false, false, true]);
    // Each from a cache holding the one before it only, by a new engine each time.
    let dir = std::env::temp_dir().join(format!(
        "silico-references-dress-keys-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let secret = dir.join("secret");
    let cache = || silico_core::cache::Cache::with_secret(dir.join("cache"), &secret);
    for (n, more) in dressings_of.iter().enumerate() {
        let engine = Recompute::with_cache(FakeKernel::new(2).unwrap(), cache());
        let model = published(&engine, checked(&document(&[more])));
        assert!(model.errors().next().is_none(), "{more}");
        let asked = dressings(engine.kernel());
        assert_eq!(asked.len(), 1, "{more}: {asked:?}");
        assert_eq!(chamfers(engine.kernel()), [n == 2], "{more}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_gate_refuses_a_name_that_doesnt_say_which_anywhere_in_it() {
    // Found by the tests review: a duplicate (`~n`) or an operation's element by its number
    // is refused wherever it is in a name, not only as the whole name.
    let unsaid = "names an edge by a name that doesn't say which";
    let first = fillet("f0000001", "m3tq8v0z", &["k7f2qx3m.box.Edge1"], "1");
    for (base, edge) in [
        ("m3tq8v0z", "k7f2qx3m.box.Edge1~1"),
        ("m3tq8v0z", "m3tq8v0z.Edge(G k7f2qx3m.box.Face1)~2"),
        // A fillet of the first fillet's edges, one named from the hole's Face3 by number,
        // one from a duplicate.
        ("f0000001", "f0000001.Edge(G m3tq8v0z.Face3)"),
        (
            "f0000001",
            "f0000001.Edge(G m3tq8v0z.Edge(G k7f2qx3m.box.Face1)~1)",
        ),
    ] {
        let text = if base == "f0000001" {
            document(&[&first, &fillet("f0000002", base, &[edge], "0.5")])
        } else {
            document(&[&fillet("f0000002", base, &[edge], "0.5")])
        };
        let said = refused(&text);
        assert!(
            said.iter()
                .any(|(p, m)| p == "edges.0" && m.contains(unsaid)),
            "{edge}: {said:?}"
        );
    }
}

#[test]
fn the_same_fillet_of_other_edges_is_another_step() {
    // A step's key holds the names: a fillet of other edges is asked for, not taken from
    // the first's result.
    let engine = engine();
    published(
        &engine,
        checked(&document(&[&fillet(
            "f0000001",
            "m3tq8v0z",
            &["k7f2qx3m.box.Edge1"],
            "0.5",
        )])),
    );
    let before = engine.kernel().calls().len();
    let model = published(
        &engine,
        checked(&document(&[&fillet(
            "f0000001",
            "m3tq8v0z",
            &["k7f2qx3m.box.Edge2"],
            "0.5",
        )])),
    );
    assert_eq!(dressings(engine.kernel()), [(vec![1], 0.5), (vec![2], 0.5)]);
    // Only the fillet made again: the block, the pin, and the hole are the same steps.
    assert_eq!(made(&engine.kernel().calls()[before..]), ["Fillet"]);
    assert!(
        described(&model, "f0000001").starts_with("fillet(cut("),
        "{}",
        described(&model, "f0000001")
    );
}

/// The gate's problems with `text`: (parameter, words in the message).
fn refused(text: &str) -> Vec<(String, String)> {
    let invalid = Checked::new(read(text.as_bytes()).unwrap()).unwrap_err();
    invalid
        .errors()
        .iter()
        .map(|e| {
            assert_eq!(e.code(), ErrorCode::InvalidInput);
            (
                e.parameter().unwrap_or_default().to_owned(),
                e.message().to_owned(),
            )
        })
        .collect()
}

#[test]
fn the_gate_refuses_a_fillet_whose_edges_or_radius_cant_be() {
    let cases: [(String, &str, &str); 9] = [
        (
            fillet("f0000001", "m3tq8v0z", &[], "1"),
            "edges",
            "names no edges",
        ),
        (
            fillet("f0000001", "m3tq8v0z", &["k7f2qx3m.box.Face1"], "1"),
            "edges.0",
            "names a face, not an edge",
        ),
        (
            fillet(
                "f0000001",
                "m3tq8v0z",
                &["k7f2qx3m.box.Edge1", "k7f2qx3m.box.Edge1"],
                "1",
            ),
            "edges.1",
            "names the edge edges.0 names",
        ),
        (
            fillet(
                "f0000001",
                "m3tq8v0z",
                &["k7f2qx3m.box.Edge(G k7f2qx3m.box.Face1)"],
                "1",
            ),
            "edges.0",
            "makes its elements itself",
        ),
        (
            fillet("f0000001", "m3tq8v0z", &["zzzzzzzz.Edge1"], "1"),
            "edges.0",
            "isn't an edge's name",
        ),
        (
            fillet("f0000001", "m3tq8v0z", &["k7f2qx3m.box.Edge 1"], "1"),
            "edges.0",
            "expected a number, at column 18",
        ),
        (
            // Trevin's (2026-10-17): a primitive's element says what made it.
            fillet("f0000001", "m3tq8v0z", &["k7f2qx3m.Edge1"], "1"),
            "edges.0",
            "k7f2qx3m is a primitive: its elements say which (`k7f2qx3m.box.Edge1`)",
        ),
        (
            fillet("f0000001", "m3tq8v0z", &["m3tq8v0z.box.Edge1"], "1"),
            "edges.0",
            "m3tq8v0z isn't a primitive",
        ),
        (
            fillet("f0000001", "m3tq8v0z", &["k7f2qx3m.box.Edge1"], "0"),
            "radius",
            "fillet's radius must be",
        ),
    ];
    for (feature, parameter, words) in cases {
        let said = refused(&document(&[&feature]));
        assert!(
            said.iter()
                .any(|(p, m)| p == parameter && m.contains(words)),
            "{feature}: {said:?}"
        );
    }
    // A chamfer's distance, by the kernel's rule, named as the file names it.
    let said = refused(&document(&[&chamfer(
        "c0000001",
        "m3tq8v0z",
        &["k7f2qx3m.box.Edge1"],
        "0",
    )]));
    assert!(
        said.iter()
            .any(|(p, m)| p == "size.distance" && m.contains("chamfer's distance")),
        "{said:?}"
    );
    // A name of a feature after this one isn't one it can name.
    let later = format!(
        "{HEAD}{BLOCK},\n{},\n{PIN}]}}",
        fillet("f0000001", "k7f2qx3m", &["a91cw0de.cylinder.Edge1"], "1")
    );
    let said = refused(&later);
    assert!(
        said.iter()
            .any(|(p, m)| p == "edges.0" && m.contains("no feature a91cw0de")),
        "{said:?}"
    );
}

#[test]
fn a_fillet_consumes_its_base_as_a_boolean_does() {
    // D2: a solid is used once. A fillet of the hole, and a cut of the hole too, is refused.
    let twice = document(&[
        &fillet("f0000001", "m3tq8v0z", &["k7f2qx3m.box.Edge1"], "1"),
        r#"{"id": "e0000001", "type": "box", "v": 1, "name": "Extra", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}"#,
        r#"{"id": "e0000002", "type": "cut", "v": 1, "name": "Again", "base": "m3tq8v0z", "tool": "e0000001"}"#,
    ]);
    let said = refused(&twice);
    assert!(
        said.iter()
            .any(|(p, m)| p == "base" && m.contains("already consumed")),
        "{said:?}"
    );
}

#[test]
fn a_fillet_is_written_canonically_an_edge_to_a_line() {
    let text = document(&[&fillet(
        "f0000001",
        "m3tq8v0z",
        &["a91cw0de.cylinder.Edge2", "k7f2qx3m.box.Edge1"],
        "0.5",
    )]);
    let written = silico_core::document::write(&read(text.as_bytes()).unwrap()).unwrap();
    assert!(
        written.contains(
            "    {\n      \"id\": \"f0000001\",\n      \"type\": \"fillet\",\n      \"v\": 1,\n      \"name\": \"Round\",\n      \"base\": \"m3tq8v0z\",\n      \"edges\": [\n        \"a91cw0de.cylinder.Edge2\",\n        \"k7f2qx3m.box.Edge1\"\n      ],\n      \"radius\": 0.5\n    }"
        ),
        "{written}"
    );
    // It reads back as itself, and writes the same again.
    let again = silico_core::document::write(&read(written.as_bytes()).unwrap()).unwrap();
    assert_eq!(again, written);
}

/// A primitive of `kind` (`box`, `cylinder`, ...) with ID `id`, as a document writes it.
fn primitive(id: &str, kind: &str) -> String {
    let values = match kind {
        "box" => r#""length": 40, "width": 20, "height": 10"#,
        "cylinder" => r#""radius": 10, "height": 10"#,
        "sphere" => r#""radius": 10"#,
        "cone" => r#""radius1": 10, "radius2": 5, "height": 10"#,
        "torus" => r#""radius1": 10, "radius2": 2"#,
        other => panic!("{other}"),
    };
    format!(
        r#"{{"id": "{id}", "type": "{kind}", "v": 1, "name": "Part", "position": [0, 0, 0], {values}}}"#
    )
}

const KINDS: [&str; 5] = ["box", "cylinder", "sphere", "cone", "torus"];

#[test]
fn a_primitive_made_another_kind_under_its_id_breaks_a_reference_to_its_elements() {
    // Trevin's (2026-10-17): a primitive's element says what made it, so a box made a
    // cylinder in place (by an agent, or a merge) breaks a fillet of its Edge1, loudly,
    // never taking the cylinder's Edge1. Every pair of kinds; each kind's own name finds
    // its Edge1 (the fake's dressing names the edge by its number).
    for made in KINDS {
        for named in KINDS {
            let engine = engine();
            let edge = format!("k7f2qx3m.{named}.Edge1");
            let text = format!(
                "{HEAD}{},\n{}]}}",
                primitive("k7f2qx3m", made),
                fillet("f0000001", "k7f2qx3m", &[&edge], "0.5")
            );
            let model = published(&engine, checked(&text));
            // The word a name says is the type the document writes (found by the quality
            // review: nothing tied the two).
            let operation = &model.document().features[0].operation;
            assert_eq!(operation.type_name(), made);
            let primitive = operation.primitive().unwrap();
            assert_eq!(PrimitiveKind::of(&primitive).word(), made);
            let outcome = model.outcome(&id("f0000001"));
            if made == named {
                assert!(
                    matches!(outcome, Some(FeatureOutcome::Built { .. })),
                    "{made}, {edge}: {outcome:?}"
                );
                assert_eq!(
                    dressings(engine.kernel()),
                    [(vec![1], 0.5)],
                    "{made}, {edge}"
                );
                continue;
            }
            let Some(FeatureOutcome::Broken { errors, .. }) = outcome else {
                panic!("{made}, {edge}: {outcome:?}");
            };
            let about: Vec<_> = errors
                .iter()
                .map(|e| (e.code(), e.parameter(), e.candidates().to_vec()))
                .collect();
            assert_eq!(
                about,
                [(ErrorCode::ReferenceBroken, Some("edges.0"), vec![])],
                "{made}, {edge}"
            );
            // Saying what changed (found by the architecture review: it said only that the
            // name found nothing).
            assert!(
                errors[0].message().contains(&format!(
                    "names an element of a {named}, but k7f2qx3m is a {made} now"
                )),
                "{}",
                errors[0]
            );
        }
    }
}

/// The one run of lines `edited` changes in `base`: where it starts, how many of `base`'s
/// lines it takes the place of, and the lines it puts there.
fn hunk<'a>(base: &[&str], edited: &[&'a str]) -> (usize, usize, Vec<&'a str>) {
    let start = base.iter().zip(edited).take_while(|(a, b)| a == b).count();
    let end = base[start..]
        .iter()
        .rev()
        .zip(edited[start..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    (
        start,
        base.len() - start - end,
        edited[start..edited.len() - end].to_vec(),
    )
}

#[test]
fn a_merge_of_a_kind_change_and_a_new_fillet_on_other_lines_breaks_the_fillet() {
    // Trevin's (2026-10-17): one branch makes the block a cylinder under its ID; another adds
    // a fillet of the block's Edge1. Each edits other lines of the file as `silico fmt`
    // writes it, so a line merge takes both without a conflict: the merged fillet names a
    // box's edge the cylinder doesn't have, and breaks, never filleting the cylinder's.
    let write = |text: &str| silico_core::document::write(&read(text.as_bytes()).unwrap()).unwrap();
    let block_cylinder = primitive("k7f2qx3m", "cylinder");
    let base = write(&document(&[]));
    let ours = write(&document(&[]).replace(BLOCK, &block_cylinder));
    let theirs = write(&document(&[&fillet(
        "f0000001",
        "m3tq8v0z",
        &["k7f2qx3m.box.Edge1"],
        "0.5",
    )]));
    let (base, ours, theirs): (Vec<&str>, Vec<&str>, Vec<&str>) = (
        base.lines().collect(),
        ours.lines().collect(),
        theirs.lines().collect(),
    );
    let (o_at, o_len, o_new) = hunk(&base, &ours);
    let (t_at, t_len, t_new) = hunk(&base, &theirs);
    // Apart, with an unchanged line between them: no conflict.
    assert!(o_at + o_len < t_at, "{o_at}+{o_len}, {t_at}");
    let mut merged: Vec<&str> = base[..o_at].to_vec();
    merged.extend(o_new);
    merged.extend(&base[o_at + o_len..t_at]);
    merged.extend(t_new);
    merged.extend(&base[t_at + t_len..]);
    let merged = merged.join("\n");
    let engine = engine();
    for text in [ours.join("\n"), theirs.join("\n")] {
        let model = published(&engine, checked(&text));
        assert!(model.errors().next().is_none(), "{text}");
    }
    let model = published(&engine, checked(&merged));
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("f0000001")) else {
        panic!("{merged}: {:?}", model.outcome(&id("f0000001")));
    };
    assert_eq!(
        errors.iter().map(Error::code).collect::<Vec<_>>(),
        [ErrorCode::ReferenceBroken]
    );
}

#[test]
fn a_fillet_of_a_sketch_or_of_a_sketchs_element_is_refused_saying_it_makes_no_solid() {
    // M3.2's sketches, met on rebasing M2.8 onto them: a sketch makes no solid, so a fillet
    // can't round one, and no name can be of its elements.
    let sketch = r#"{"id": "s0000001", "type": "sketch", "v": 1, "name": "Profile", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}], "lines": [], "circles": [], "arcs": [], "constraints": []}"#;
    let said = refused(&document(&[
        sketch,
        &fillet("f0000001", "s0000001", &["k7f2qx3m.box.Edge1"], "1"),
    ]));
    assert!(
        said.iter()
            .any(|(p, m)| p == "base" && m.contains("makes no solid")),
        "{said:?}"
    );
    let said = refused(&document(&[
        sketch,
        &fillet("f0000001", "m3tq8v0z", &["s0000001.Edge1"], "1"),
    ]));
    assert!(
        said.iter().any(|(p, m)| p == "edges.0"
            && m.contains(
                "s0000001 makes no solid (a sketch): it has no faces, edges, or vertices"
            )),
        "{said:?}"
    );
}
