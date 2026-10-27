//! A document's timeline edited by its chains (`document::chain`: the M5.0 design's
//! section 7, A9, and section 12's items 1, 2, and 5): insert, move, and remove on the M4.9
//! bracket and on small documents of boxes, each expected chain written out by hand, each
//! result through the gate, and the edits rebuilt on the fake kernel beside a document
//! written in that order by hand, their fingerprints alike.

use super::bracket::*;
use super::model::published;

use silico_core::document::chain::{Rewired, insert, move_feature, remove};
use silico_core::document::{Checked, Document, FeatureId, read, validate};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::Recompute;

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn document(features: &[&str]) -> Document {
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    );
    read(text.as_bytes()).unwrap_or_else(|e| panic!("{e}: {text}"))
}

/// A feature as the document reads it, from its JSON.
fn feature(json: &str) -> silico_core::document::Feature {
    document(&[json]).features.remove(0)
}

/// `json` with its `base` set to `base`.
fn on(json: &str, base: &str) -> String {
    let at = json.find(r#""base": ""#).unwrap() + r#""base": ""#.len();
    format!("{}{base}{}", &json[..at], &json[at + 8..])
}

fn bracket() -> Document {
    document(&[
        S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001,
    ])
}

fn ids(document: &Document) -> Vec<&str> {
    document.features.iter().map(|f| f.id.as_str()).collect()
}

fn rewired(feature: &str, field: &'static str, from: Option<&str>, to: Option<&str>) -> Rewired {
    Rewired {
        feature: id(feature),
        field,
        from: from.map(id),
        to: to.map(id),
    }
}

/// `edited` passes the gate and is `expected`, written by hand, feature for feature; both
/// rebuilt on the fake kernel, every feature's fingerprint alike.
fn is_written(edited: &Document, expected: &Document) {
    validate(edited).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(
        edited.canonically_eq(expected),
        "{:?}\n!=\n{:?}",
        ids(edited),
        ids(expected)
    );
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    let built = published(&engine, Checked::new(edited.clone()).unwrap());
    let by_hand = published(&engine, Checked::new(expected.clone()).unwrap());
    for f in &expected.features {
        assert_eq!(
            built.fingerprint(&f.id),
            by_hand.fingerprint(&f.id),
            "{}",
            f.id
        );
    }
}

const CHAMFER: &str = r#"{"id": "c0000002", "type": "chamfer", "v": 1, "name": "Base edge", "base": "h0000001", "edges": ["p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000002)"], "size": {"distance": 1}}"#;

// ---------------------------------------------------------------------------------------
// Insert (`add_feature`'s `before`; section 12, item 1).

#[test]
fn a_chamfer_inserted_before_the_second_pocket_takes_its_input_from_it() {
    // The design's example: c0000002 on h0000001, before h0000002, which consumed h0000001;
    // h0000002 now builds on the chamfer.
    let inserted = insert(&bracket(), feature(CHAMFER), Some(&id("h0000002"))).unwrap();
    assert_eq!(inserted.index, 5);
    assert_eq!(
        inserted.rewired,
        [rewired(
            "h0000002",
            "base",
            Some("h0000001"),
            Some("c0000002")
        )]
    );
    let by_hand = document(&[
        S0000001,
        P0000001,
        S0000002,
        H0000001,
        S0000003,
        CHAMFER,
        &on(H0000002, "c0000002"),
        F0000001,
    ]);
    is_written(&inserted.document, &by_hand);
}

#[test]
fn an_insert_whose_input_is_consumed_before_the_place_is_refused() {
    // h0000001's consumer, h0000002, comes before f0000001: the design's message.
    let error = insert(&bracket(), feature(CHAMFER), Some(&id("f0000001"))).unwrap_err();
    assert_eq!(
        error.message(),
        "h0000001's solid is already consumed by h0000002, which comes before the place asked for"
    );
    assert_eq!(error.feature(), Some("c0000002"));
    assert_eq!(error.parameter(), Some("before"));
    assert_eq!(
        error.suggestion(),
        Some("add it before h0000002, or on h0000002's result")
    );
}

#[test]
fn an_insert_before_a_feature_the_document_hasnt_is_refused() {
    let error = insert(&bracket(), feature(CHAMFER), Some(&id("z0000001"))).unwrap_err();
    assert_eq!(
        error.message(),
        "the document has no feature z0000001 to add this one before"
    );
    assert_eq!(error.parameter(), Some("before"));
    assert_eq!(
        error.suggestion(),
        Some("name a feature by its ID, or give null to add at the end")
    );
}

#[test]
fn a_boolean_whose_two_inputs_both_had_consumers_is_refused_naming_both() {
    // h0000001 is consumed by h0000002, and h0000002 by f0000001.
    let fuse = r#"{"id": "j0000001", "type": "fuse", "v": 1, "name": "Both", "base": "h0000001", "tool": "h0000002"}"#;
    let error = insert(&bracket(), feature(fuse), None).unwrap_err();
    assert!(error.message().contains("h0000002"), "{error}");
    assert!(error.message().contains("f0000001"), "{error}");
    assert!(error.message().contains("consumed twice"), "{error}");
    assert_eq!(error.feature(), Some("j0000001"));
}

#[test]
fn an_insert_at_the_end_on_an_unconsumed_result_rewires_nothing() {
    let chamfer = on(CHAMFER, "f0000001");
    let inserted = insert(&bracket(), feature(&chamfer), None).unwrap();
    assert_eq!(inserted.index, 7);
    assert!(inserted.rewired.is_empty());
    let by_hand = document(&[
        S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001, &chamfer,
    ]);
    is_written(&inserted.document, &by_hand);
}

#[test]
fn a_pocket_inserted_between_the_pockets_is_built_as_written_by_hand() {
    // Another pocket of the upright's holes on h0000001, before h0000002.
    let pocket = r#"{"id": "h0000003", "type": "pocket", "v": 1, "name": "More", "base": "h0000001", "sketch": "s0000003", "extent": {"type": "through_all", "reversed": true}}"#;
    let inserted = insert(&bracket(), feature(pocket), Some(&id("h0000002"))).unwrap();
    assert_eq!(
        inserted.rewired,
        [rewired(
            "h0000002",
            "base",
            Some("h0000001"),
            Some("h0000003")
        )]
    );
    let by_hand = document(&[
        S0000001,
        P0000001,
        S0000002,
        H0000001,
        S0000003,
        pocket,
        &on(H0000002, "h0000003"),
        F0000001,
    ]);
    is_written(&inserted.document, &by_hand);
}

// ---------------------------------------------------------------------------------------
// Remove (A9; section 12, item 5).

#[test]
fn removing_the_first_pocket_gives_the_second_its_base() {
    // The design's example: h0000002 builds on p0000001.
    let removed = remove(&bracket(), &id("h0000001")).unwrap();
    assert_eq!(removed.index, 3);
    assert_eq!(removed.feature.id, id("h0000001"));
    assert_eq!(
        removed.rewired,
        [rewired(
            "h0000002",
            "base",
            Some("h0000001"),
            Some("p0000001")
        )]
    );
    let by_hand = document(&[
        S0000001,
        P0000001,
        S0000002,
        S0000003,
        &on(H0000002, "p0000001"),
        F0000001,
    ]);
    is_written(&removed.document, &by_hand);
}

#[test]
fn removing_a_chains_root_under_a_pocket_is_refused_naming_both() {
    // Without the fillet, which names the pad's elements: the root rule alone.
    let unfilleted = document(&[S0000001, P0000001, S0000002, H0000001, S0000003, H0000002]);
    let error = remove(&unfilleted, &id("p0000001")).unwrap_err();
    assert_eq!(
        error.message(),
        "p0000001 is the root of h0000001's chain, and h0000001, a pocket, can't be left without a base"
    );
    assert_eq!(error.feature(), Some("p0000001"));
    assert_eq!(
        error.suggestion(),
        Some("nothing before h0000001 but p0000001 can be its base; remove h0000001 first")
    );
}

#[test]
fn removing_a_sketch_a_pocket_uses_or_a_feature_whose_elements_are_named_is_refused() {
    let error = remove(&bracket(), &id("s0000002")).unwrap_err();
    assert_eq!(
        error.message(),
        "h0000001 uses s0000002 as its sketch, so s0000002 can't be removed"
    );
    assert_eq!(error.feature(), Some("s0000002"));
    // The fillet's edge, p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000004), names the
    // pad's elements and the profile's.
    let error = remove(&bracket(), &id("p0000001")).unwrap_err();
    assert_eq!(
        error.message(),
        "f0000001 names p0000001's elements, so p0000001 can't be removed"
    );
}

#[test]
fn removing_a_pads_root_lets_the_pad_after_it_begin_the_body() {
    // A pad on a pad: removing the first, the second's base is left out.
    let second = r#"{"id": "p0000002", "type": "pad", "v": 1, "name": "More", "base": "p0000001", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    let removed = remove(&document(&[S0000001, P0000001, second]), &id("p0000001")).unwrap();
    assert_eq!(
        removed.rewired,
        [rewired("p0000002", "base", Some("p0000001"), None)]
    );
    let alone = r#"{"id": "p0000002", "type": "pad", "v": 1, "name": "More", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    is_written(&removed.document, &document(&[S0000001, alone]));
}

const BLOCK: &str = r#"{"id": "b0000001", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#;
const TOOL: &str = r#"{"id": "t0000001", "type": "box", "v": 1, "name": "Tool", "position": [2, 2, 2], "length": 2, "width": 2, "height": 20}"#;
const CUT: &str = r#"{"id": "c0000001", "type": "cut", "v": 1, "name": "Cut", "base": "b0000001", "tool": "t0000001"}"#;

#[test]
fn removing_a_booleans_tool_with_no_base_of_its_own_is_refused_naming_both() {
    let error = remove(&document(&[BLOCK, TOOL, CUT]), &id("t0000001")).unwrap_err();
    assert_eq!(
        error.message(),
        "t0000001 is c0000001's tool, and has no base to stand in for it"
    );
    assert_eq!(error.feature(), Some("t0000001"));
}

// ---------------------------------------------------------------------------------------
// Move (section 12, item 2, with the format review's correction).

#[test]
fn moving_the_first_pocket_after_the_second_swaps_them_in_the_chain() {
    // The design's example: s0000001, p0000001, s0000002, s0000003, h0000002, h0000001,
    // f0000001, and the three bases rewired, in timeline order.
    let moved = move_feature(&bracket(), &id("h0000001"), Some(&id("f0000001"))).unwrap();
    assert_eq!((moved.from, moved.to, moved.changed), (3, 5, true));
    assert_eq!(
        moved.rewired,
        [
            rewired("h0000002", "base", Some("h0000001"), Some("p0000001")),
            rewired("h0000001", "base", Some("p0000001"), Some("h0000002")),
            rewired("f0000001", "base", Some("h0000002"), Some("h0000001")),
        ]
    );
    let by_hand = document(&[
        S0000001,
        P0000001,
        S0000002,
        S0000003,
        &on(H0000002, "p0000001"),
        &on(H0000001, "h0000002"),
        &on(F0000001, "h0000001"),
    ]);
    is_written(&moved.document, &by_hand);
}

#[test]
fn moving_the_fillet_between_the_pockets_builds_it_on_the_first() {
    let moved = move_feature(&bracket(), &id("f0000001"), Some(&id("h0000002"))).unwrap();
    assert_eq!((moved.from, moved.to), (6, 5));
    assert_eq!(
        moved.rewired,
        [
            rewired("f0000001", "base", Some("h0000002"), Some("h0000001")),
            rewired("h0000002", "base", Some("h0000001"), Some("f0000001")),
        ]
    );
    let by_hand = document(&[
        S0000001,
        P0000001,
        S0000002,
        H0000001,
        S0000003,
        &on(F0000001, "h0000001"),
        &on(H0000002, "f0000001"),
    ]);
    is_written(&moved.document, &by_hand);
}

#[test]
fn a_move_that_leaves_a_feature_between_its_base_and_its_consumer_rewires_nothing() {
    // h0000001 past s0000003, still after p0000001 and before h0000002.
    let moved = move_feature(&bracket(), &id("h0000001"), Some(&id("h0000002"))).unwrap();
    assert_eq!((moved.from, moved.to, moved.changed), (3, 4, true));
    assert!(moved.rewired.is_empty());
    let by_hand = document(&[
        S0000001, P0000001, S0000002, S0000003, H0000001, H0000002, F0000001,
    ]);
    is_written(&moved.document, &by_hand);
}

#[test]
fn a_move_to_where_a_feature_is_changes_nothing() {
    for before in [Some(id("h0000001")), Some(id("s0000003"))] {
        let moved = move_feature(&bracket(), &id("h0000001"), before.as_ref()).unwrap();
        assert!(!moved.changed);
        assert!(moved.rewired.is_empty());
        assert!(moved.document.canonically_eq(&bracket()));
    }
    let moved = move_feature(&bracket(), &id("f0000001"), None).unwrap();
    assert!(!moved.changed);
}

#[test]
fn a_move_before_its_sketch_is_refused_naming_both() {
    // The design's example.
    let error = move_feature(&bracket(), &id("h0000002"), Some(&id("h0000001"))).unwrap_err();
    assert_eq!(
        error.message(),
        "h0000002 would come before s0000003, which it uses as its sketch"
    );
    assert_eq!(error.feature(), Some("h0000002"));
    assert_eq!(error.parameter(), Some("before"));
    assert_eq!(
        error.suggestion(),
        Some("move s0000003 first, or move h0000002 after it")
    );
}

#[test]
fn a_chains_root_moved_past_a_pocket_that_needs_it_is_refused() {
    // The design's example, word for word: p0000001 before s0000003, past h0000001.
    let error = move_feature(&bracket(), &id("p0000001"), Some(&id("s0000003"))).unwrap_err();
    assert_eq!(
        error.message(),
        "p0000001 is the root of h0000001's chain, and h0000001, a pocket, can't be left without a base"
    );
    assert_eq!(error.feature(), Some("p0000001"));
    assert_eq!(error.parameter(), Some("feature"));
    assert_eq!(
        error.suggestion(),
        Some(
            "nothing before h0000001 but p0000001 can be its base, so p0000001 can't move past it; move another feature instead, or remove h0000001 first"
        )
    );
    assert!(error.candidates().is_empty());
}

#[test]
fn a_move_after_a_feature_naming_its_elements_or_before_one_it_names_is_refused() {
    // The fillet names p0000001's and s0000001's elements.
    let error = move_feature(&bracket(), &id("s0000001"), None).unwrap_err();
    assert_eq!(
        error.message(),
        "s0000001 would come after p0000001, which uses s0000001 as its sketch"
    );
    let error = move_feature(&bracket(), &id("f0000001"), Some(&id("p0000001"))).unwrap_err();
    assert_eq!(
        error.message(),
        "f0000001 would come before p0000001, whose elements it names"
    );
}

#[test]
fn a_boolean_moved_before_its_tool_is_refused() {
    let error = move_feature(
        &document(&[BLOCK, TOOL, CUT]),
        &id("c0000001"),
        Some(&id("t0000001")),
    )
    .unwrap_err();
    assert_eq!(
        error.message(),
        "c0000001 would come before t0000001, which it uses as its tool"
    );
}

#[test]
fn a_feature_needing_a_base_placed_before_its_chains_root_is_refused() {
    // The cut moved before the block, its base, the root: nothing can be its base there.
    let error = move_feature(
        &document(&[TOOL, BLOCK, CUT]),
        &id("c0000001"),
        Some(&id("b0000001")),
    )
    .unwrap_err();
    assert_eq!(
        error.message(),
        "c0000001, a cut, can't be placed before b0000001, its chain's root: it needs a base"
    );
}

#[test]
fn a_move_off_a_tools_chain_past_its_boolean_is_refused_naming_the_boolean() {
    // The tool's chain: t0000001, then its fillet r0000001, which the cut consumes as its
    // tool. Moving the fillet past the cut would make it the cut's tool's chain's end.
    let fillet = r#"{"id": "r0000001", "type": "fillet", "v": 1, "name": "Round", "base": "t0000001", "edges": ["t0000001.box.Edge1"], "radius": 0.5}"#;
    let cut = r#"{"id": "c0000001", "type": "cut", "v": 1, "name": "Cut", "base": "b0000001", "tool": "r0000001"}"#;
    let error = move_feature(
        &document(&[BLOCK, TOOL, fillet, cut]),
        &id("r0000001"),
        None,
    )
    .unwrap_err();
    assert!(error.message().contains("c0000001"), "{error}");
    assert!(error.message().contains("tool"), "{error}");
    assert_eq!(error.feature(), Some("r0000001"));
}

#[test]
fn a_primitive_moved_into_a_chain_after_its_first_feature_is_refused() {
    // A pad on a box: the box moved after the pad would take the pad as its base.
    let pad = r#"{"id": "p0000002", "type": "pad", "v": 1, "name": "More", "base": "b0000001", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    let error =
        move_feature(&document(&[S0000001, BLOCK, pad]), &id("b0000001"), None).unwrap_err();
    assert_eq!(
        error.message(),
        "b0000001, a box, takes no base, and the place asked for comes after p0000002 in its chain"
    );
}

#[test]
fn a_move_of_a_feature_the_document_hasnt_or_before_one_is_refused() {
    let error = move_feature(&bracket(), &id("z0000001"), None).unwrap_err();
    assert_eq!(error.parameter(), Some("feature"));
    let error = move_feature(&bracket(), &id("h0000001"), Some(&id("z0000001"))).unwrap_err();
    assert_eq!(error.parameter(), Some("before"));
}

#[test]
fn a_stored_names_makers_are_every_feature_it_writes_once_each_in_order() {
    // The fillet's edge: the pad's edge generated from the profile's vertex, so both are
    // its makers, the pad first, as it's written.
    use silico_core::naming::{Maker, Name};
    let document = bracket();
    let maker = |id: &FeatureId| {
        document
            .features
            .iter()
            .find(|f| f.id == *id)
            .map(|f| Maker::of(&f.operation))
    };
    let name = Name::parse(
        "p0000001.pad.Edge(G s0000001.sketch.Vertex.pa000004)",
        &maker,
    )
    .unwrap();
    let makers: Vec<&str> = name.features().iter().map(|id| id.as_str()).collect();
    assert_eq!(makers, ["p0000001", "s0000001"]);
}
