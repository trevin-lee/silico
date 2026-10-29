//! A document's timeline edited by its chains (`document::chain`: the M5.0 design's
//! section 7, A9, and section 12's items 1, 2, and 5): insert, move, and remove on the M4.9
//! bracket and on small documents of boxes, each expected chain written out by hand, each
//! result through the gate, written by hand as the edit should give it, and rebuilt on the
//! fake kernel, every feature built and every stored name found.

use super::bracket::*;
use super::model::published;

use silico_core::Error;
use silico_core::document::chain::{self, Inserted, Moved, Removed, Rewired};
use silico_core::document::{Checked, Document, Feature, FeatureId, read, validate};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::{FeatureOutcome, Recompute};

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
fn feature(json: &str) -> Feature {
    document(&[json]).features.remove(0)
}

/// `json` with its `base` set to `base`.
fn on(json: &str, base: &str) -> String {
    let at = json.find(r#""base": ""#).unwrap() + r#""base": ""#.len();
    format!("{}{base}{}", &json[..at], &json[at + 8..])
}

/// `document` through the gate: the edits take only a document it took.
fn gated(document: &Document) -> Checked {
    Checked::new(document.clone()).unwrap_or_else(|e| panic!("{e:?}"))
}

fn insert(
    document: &Document,
    feature: Feature,
    before: Option<&FeatureId>,
) -> Result<Inserted, Error> {
    chain::insert(&gated(document), feature, before)
}

fn remove(document: &Document, id: &FeatureId) -> Result<Removed, Error> {
    chain::remove(&gated(document), id)
}

fn move_feature(
    document: &Document,
    id: &FeatureId,
    before: Option<&FeatureId>,
) -> Result<Moved, Error> {
    chain::move_feature(&gated(document), id, before)
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

/// `edited` passes the gate, is `expected`, written by hand, feature for feature, and
/// builds on the fake kernel: every feature built, every stored name found.
fn is_written(edited: &Document, expected: &Document) {
    validate(edited).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(
        edited.canonically_eq(expected),
        "{:?}\n!=\n{:?}",
        ids(edited),
        ids(expected)
    );
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
    let model = published(&engine, Checked::new(edited.clone()).unwrap());
    for f in &edited.features {
        let outcome = model.outcome(&f.id).unwrap();
        assert!(
            !matches!(outcome, FeatureOutcome::Blocked { .. }) && outcome.errors().is_empty(),
            "{}: {outcome:?}",
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

// A review finding: every maker the tests' names had was its chain's root, so a lineage
// that left out its last feature (the root) went unnoticed. Here the fillet names the second
// pad, which, moved before the first, still holds what the fillet's base is built from.
#[test]
fn a_pad_moved_before_the_pad_it_was_on_keeps_the_fillet_naming_it_in_its_lineage() {
    let first = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "One", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    let second = r#"{"id": "p0000002", "type": "pad", "v": 1, "name": "Two", "base": "p0000001", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": true}}"#;
    let fillet = r#"{"id": "r0000001", "type": "fillet", "v": 1, "name": "R", "base": "p0000002", "edges": ["p0000002.pad.Edge(G s0000001.sketch.Vertex.pa000004)"], "radius": 0.5}"#;
    let original = document(&[S0000001, first, second, fillet]);
    let moved = move_feature(&original, &id("p0000002"), Some(&id("p0000001"))).unwrap();
    // The chain p2, p1, r: the fillet on p1, which is built on p2.
    assert_eq!(
        moved.rewired,
        [
            rewired("p0000002", "base", Some("p0000001"), None),
            rewired("p0000001", "base", None, Some("p0000002")),
            rewired("r0000001", "base", Some("p0000002"), Some("p0000001")),
        ]
    );
    let second_alone = r#"{"id": "p0000002", "type": "pad", "v": 1, "name": "Two", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": true}}"#;
    let first_on_second = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "One", "base": "p0000002", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
    is_written(
        &moved.document,
        &document(&[
            S0000001,
            second_alone,
            first_on_second,
            &on(fillet, "p0000001"),
        ]),
    );
}

#[test]
fn removing_a_revolves_root_lets_the_revolve_begin_the_body() {
    // A revolve's base is optional, as a pad's is.
    let revolve = r#"{"id": "v0000001", "type": "revolve", "v": 1, "name": "Turn", "base": "p0000001", "sketch": "s0000001", "axis": "x_axis", "extent": {"type": "one_side", "angle": 90, "reversed": false}}"#;
    let removed = remove(&document(&[S0000001, P0000001, revolve]), &id("p0000001")).unwrap();
    assert_eq!(
        removed.rewired,
        [rewired("v0000001", "base", Some("p0000001"), None)]
    );
    let alone = r#"{"id": "v0000001", "type": "revolve", "v": 1, "name": "Turn", "sketch": "s0000001", "axis": "x_axis", "extent": {"type": "one_side", "angle": 90, "reversed": false}}"#;
    is_written(&removed.document, &document(&[S0000001, alone]));
}

// FreeCAD's TestFillet.py, testDeletingPreviousFeatureRelinksUniqueMatchingBaseEdge (fix
// 26c895c30d): the feature under a fillet removed, the fillet's edge, which only passed
// through it, is found on the new base.
#[test]
fn removing_the_fillets_base_builds_it_on_the_base_before_its_name_found_there() {
    let removed = remove(&bracket(), &id("h0000002")).unwrap();
    assert_eq!(
        removed.rewired,
        [rewired(
            "f0000001",
            "base",
            Some("h0000002"),
            Some("h0000001")
        )]
    );
    is_written(
        &removed.document,
        &document(&[
            S0000001,
            P0000001,
            S0000002,
            H0000001,
            S0000003,
            &on(F0000001, "h0000001"),
        ]),
    );
}

// FreeCAD's testDeletingPreviousFeatureDoesNotRelinkUnsafeBaseEdge: where the fillet names
// an edge the removed feature made (a hole's rim), FreeCAD leaves the link broken; Silico
// refuses the removal, naming both.
#[test]
fn removing_the_feature_that_made_a_fillets_edge_is_refused_naming_both() {
    let rim = r#"{"id": "f0000002", "type": "fillet", "v": 1, "name": "Rim", "base": "h0000002", "edges": ["h0000001.pocket.Edge(M s0000002.sketch.Edge.cb000001)"], "radius": 0.5}"#;
    let document = document(&[
        S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, rim,
    ]);
    let error = remove(&document, &id("h0000001")).unwrap_err();
    assert_eq!(error.feature(), Some("h0000001"));
    assert_eq!(
        error.message(),
        "f0000002 names h0000001's elements, so h0000001 can't be removed"
    );
    assert_eq!(
        error.suggestion(),
        Some("remove f0000002 first, or set its edges to names of other features' elements")
    );
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

// ---------------------------------------------------------------------------------------
// Break-it: hostile documents and arguments. Expected values from the design's rules (the
// M5.0 design, section 7 and A9), the gate, the fake kernel's rebuild, and a model of the
// chain written apart from `document::chain`: every body's chain is its `base` links, so
// after an edit each chain is its members in their new timeline order, each built on the
// one before it, and a boolean's tool is the last of its tool's chain.

mod hostile {
    use super::*;

    use std::collections::{BTreeMap, BTreeSet};
    use std::time::Duration;

    use proptest::prelude::*;
    use silico_core::ErrorCode;
    use silico_core::document::Operation;

    fn config(cases: u32) -> ProptestConfig {
        ProptestConfig {
            cases,
            failure_persistence: None,
            rng_seed: proptest::test_runner::RngSeed::Fixed(0xc4a1_7ed1),
            ..ProptestConfig::default()
        }
    }

    const FUSE: &str = r#"{"id": "j0000001", "type": "fuse", "v": 1, "name": "Fuse", "base": "b0000001", "tool": "t0000001"}"#;
    /// A fillet of the fused solid's edge that came from the tool: an edge of t0000001's, as
    /// the fuse kept it, which the fake builds.
    const ROUND_TOOL_EDGE: &str = r#"{"id": "r0000001", "type": "fillet", "v": 1, "name": "Round", "base": "j0000001", "edges": ["t0000001.box.Edge1"], "radius": 0.5}"#;

    /// Every feature's outcome in `document`'s rebuild on the fake: its errors' messages,
    /// or the feature it's blocked on; none for one built.
    fn rebuilt(document: &Document) -> BTreeMap<String, Vec<String>> {
        let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new());
        let model = published(&engine, Checked::new(document.clone()).unwrap());
        document
            .features
            .iter()
            .map(|f| {
                let outcome = model.outcome(&f.id).unwrap();
                let said = match outcome {
                    FeatureOutcome::Blocked { on } => vec![format!("blocked on {on}")],
                    other => other
                        .errors()
                        .iter()
                        .map(|e| e.message().to_owned())
                        .collect(),
                };
                (f.id.to_string(), said)
            })
            .collect()
    }

    #[test]
    fn the_fuse_fixture_builds_its_fillet_of_the_tools_edge() {
        // The two tests below rest on this: as written, every feature builds.
        let fused = document(&[BLOCK, TOOL, FUSE, ROUND_TOOL_EDGE]);
        validate(&fused).unwrap();
        assert!(
            rebuilt(&fused).values().all(Vec::is_empty),
            "{:?}",
            rebuilt(&fused)
        );
    }

    // A break-it finding: the move was taken and the gate passed it, and the fillet, which
    // names an edge of the fuse's tool, then built on the block alone, where that edge isn't
    // (reference_broken at the rebuild). The lineage rule (pending Trevin, extending the
    // design's "names its elements"; section 12, item 2b) refuses it from the text: r0000001
    // names t0000001's elements, and its base would no longer be built from t0000001.
    #[test]
    fn a_fuse_moved_past_a_fillet_naming_its_tools_edge_is_refused_naming_both() {
        let fused = document(&[BLOCK, TOOL, FUSE, ROUND_TOOL_EDGE]);
        let error = move_feature(&fused, &id("j0000001"), None).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidInput);
        assert_eq!(error.feature(), Some("j0000001"));
        assert_eq!(error.parameter(), Some("before"));
        assert_eq!(
            error.message(),
            "r0000001 names elements of t0000001, which its base would no longer be built from once j0000001 moves there"
        );
        assert_eq!(
            error.suggestion(),
            Some("move j0000001 elsewhere, or remove r0000001 first")
        );
    }

    // As above, for a removal: without j0000001, r0000001 would build on b0000001 alone.
    #[test]
    fn a_fuse_removed_under_a_fillet_naming_its_tools_edge_is_refused_naming_both() {
        let fused = document(&[BLOCK, TOOL, FUSE, ROUND_TOOL_EDGE]);
        let error = remove(&fused, &id("j0000001")).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidInput);
        assert_eq!(error.feature(), Some("j0000001"));
        assert_eq!(error.parameter(), Some("feature"));
        assert_eq!(
            error.message(),
            "r0000001 names elements of t0000001, which its base would no longer be built from without j0000001"
        );
        assert_eq!(error.suggestion(), Some("remove r0000001 first"));
    }

    /// A fuse of the block and the tool under a fillet naming `edges`, beside a box `z0000001`
    /// nothing builds on.
    fn fused_under(edges: &str) -> Document {
        let other = r#"{"id": "z0000001", "type": "box", "v": 1, "name": "Other", "position": [20, 0, 0], "length": 1, "width": 1, "height": 1}"#;
        let fillet = format!(
            r#"{{"id": "r0000001", "type": "fillet", "v": 1, "name": "Round", "base": "j0000001", "edges": {edges}, "radius": 0.5}}"#
        );
        document(&[BLOCK, TOOL, other, FUSE, &fillet])
    }

    // A review finding: the rule looked only at the first maker missing after the edit, and
    // one missing before it too (z0000001, never in the fillet's lineage) hid t0000001,
    // which the edit takes out.
    #[test]
    fn a_maker_never_in_the_lineage_hides_none_the_edit_takes_out() {
        for edges in [
            r#"["t0000001.box.Edge1", "z0000001.box.Edge1"]"#,
            r#"["z0000001.box.Edge1", "t0000001.box.Edge1"]"#,
        ] {
            let fused = fused_under(edges);
            for error in [
                move_feature(&fused, &id("j0000001"), None).unwrap_err(),
                remove(&fused, &id("j0000001")).unwrap_err(),
            ] {
                assert_eq!(error.feature(), Some("j0000001"), "{edges}");
                assert!(
                    error
                        .message()
                        .starts_with("r0000001 names elements of t0000001, which its base"),
                    "{edges}: {error}"
                );
            }
        }
        // Only z0000001's edge: the edit takes nothing out of the lineage the fillet had.
        let fused = fused_under(r#"["z0000001.box.Edge1"]"#);
        let removed = remove(&fused, &id("j0000001")).unwrap();
        assert_eq!(
            removed.rewired,
            [rewired(
                "r0000001",
                "base",
                Some("j0000001"),
                Some("b0000001")
            )]
        );
    }

    // The rule is a name's: a name one of whose makers was never in the fillet's lineage
    // (here p0000002, a pad of its own) was broken before the edit, and the edit is no
    // reason to refuse, though it takes another of its makers (s0000001, swept by the
    // pocket) out of the lineage. Found by the generated documents: a pocket inserted and
    // then removed was refused.
    #[test]
    fn a_name_broken_before_the_edit_refuses_nothing() {
        let pocket = r#"{"id": "h0000001", "type": "pocket", "v": 1, "name": "H", "base": "b0000001", "sketch": "s0000001", "extent": {"type": "through_all", "reversed": true}}"#;
        let fuse = r#"{"id": "j0000001", "type": "fuse", "v": 1, "name": "Fuse", "base": "h0000001", "tool": "t0000001"}"#;
        let pad = r#"{"id": "p0000002", "type": "pad", "v": 1, "name": "Apart", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
        let fillet = r#"{"id": "r0000001", "type": "fillet", "v": 1, "name": "Round", "base": "j0000001", "edges": ["p0000002.pad.Edge(G s0000001.sketch.Vertex.pa000004)"], "radius": 0.5}"#;
        let document = document(&[S0000001, BLOCK, pocket, TOOL, fuse, pad, fillet]);
        let removed = remove(&document, &id("h0000001")).unwrap();
        assert_eq!(
            removed.rewired,
            [rewired(
                "j0000001",
                "base",
                Some("h0000001"),
                Some("b0000001")
            )]
        );
    }

    // A review finding: no test had a maker only a nested source names decide a refusal.
    #[test]
    fn a_maker_named_only_inside_a_source_keeps_its_removal_refused() {
        let rounded_tool = r#"{"id": "t0000002", "type": "fillet", "v": 1, "name": "Rounded tool", "base": "t0000001", "edges": ["t0000001.box.Edge1"], "radius": 0.5}"#;
        let cut = r#"{"id": "c0000001", "type": "cut", "v": 1, "name": "Cut", "base": "b0000001", "tool": "t0000002"}"#;
        let fillet = r#"{"id": "r0000001", "type": "fillet", "v": 1, "name": "Round", "base": "c0000001", "edges": ["c0000001.cut.Edge(G b0000001.box.Face6, G t0000002.fillet.Face(G t0000001.box.Edge1))"], "radius": 0.5}"#;
        let document = document(&[BLOCK, TOOL, rounded_tool, cut, fillet]);
        validate(&document).unwrap();
        let error = remove(&document, &id("t0000002")).unwrap_err();
        assert_eq!(error.feature(), Some("t0000002"));
        assert_eq!(
            error.message(),
            "r0000001 names t0000002's elements, so t0000002 can't be removed"
        );
    }

    #[test]
    fn the_fillet_removed_first_the_fuse_can_be_removed_and_leaves_both_solids() {
        let fused = document(&[BLOCK, TOOL, FUSE, ROUND_TOOL_EDGE]);
        let removed = remove(&fused, &id("r0000001")).unwrap();
        assert_eq!(removed.rewired, []);
        is_written(&removed.document, &document(&[BLOCK, TOOL, FUSE]));
        let removed = remove(&removed.document, &id("j0000001")).unwrap();
        assert_eq!(removed.rewired, []);
        is_written(&removed.document, &document(&[BLOCK, TOOL]));
    }

    // Was a panic (index out of bounds in `join`'s walk down from the consumer) when the
    // edits took any `Document`: a feature whose base is itself is its own consumer, and
    // moved from the end, the walk started past the end of the timeline. The edits take a
    // `Checked` document now, and the gate refuses this one, so it can't reach them.
    #[test]
    fn a_pad_on_itself_is_refused_by_the_gate_so_no_edit_takes_it() {
        let alone = r#"{"id": "p0000001", "type": "pad", "v": 1, "name": "One", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
        let itself = r#"{"id": "p0000002", "type": "pad", "v": 1, "name": "More", "base": "p0000002", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}"#;
        let unchecked = document(&[S0000001, alone, itself]);
        let invalid = Checked::new(unchecked).unwrap_err();
        assert!(
            invalid
                .errors()
                .iter()
                .any(|e| e.feature() == Some("p0000002") && e.parameter() == Some("base")),
            "{invalid:?}"
        );
    }

    #[test]
    fn a_fuse_of_one_solid_with_itself_inserted_is_refused_as_the_gate_words_it() {
        // h0000001 as both base and tool: the gate's words are "tool h0000001 is base too: a
        // fuse takes two different solids". The insert must not word it as two consumers
        // (h0000002 twice) and advise "h0000002's or h0000002's result".
        let fuse = r#"{"id": "j0000001", "type": "fuse", "v": 1, "name": "Both", "base": "h0000001", "tool": "h0000001"}"#;
        let error = insert(&bracket(), feature(fuse), Some(&id("h0000002"))).unwrap_err();
        assert_eq!(error.feature(), Some("j0000001"));
        // Was (a break-it finding): "j0000001 would be consumed twice: its base h0000001's
        // solid is consumed by h0000002, and its tool h0000001's by h0000002", advising
        // "use h0000002's or h0000002's result": neither said base and tool are one solid.
        assert_eq!(error.parameter(), Some("feature.tool"));
        assert_eq!(
            error.message(),
            "tool h0000001 is base too: a fuse takes two different solids"
        );
        assert_eq!(error.suggestion(), Some("give the fuse a different tool"));
    }

    /// `n` boxes, `b0000000` to `b{n-1}` in hexadecimal, nothing consuming any.
    fn boxes(n: usize) -> Document {
        let features: Vec<String> = (0..n)
            .map(|i| {
                format!(
                    r#"{{"id": "b{i:07x}", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}}"#
                )
            })
            .collect();
        document(&features.iter().map(String::as_str).collect::<Vec<_>>())
    }

    /// `edit` of `document`, taking at most twenty times the CPU time the gate takes on it
    /// and half a second more: CPU time, which a loaded machine doesn't change (`within`).
    fn promptly<T: Send + 'static>(
        document: &Document,
        edit: impl FnOnce(&Checked) -> T + Send + 'static,
    ) -> T {
        let start = cpu_time::ThreadTime::now();
        validate(document).unwrap();
        let gate = start.elapsed();
        let checked = gated(document);
        crate::within::within(gate * 20 + Duration::from_millis(500), move || {
            edit(&checked)
        })
    }

    // Was (a break-it finding): every edit found each feature of the result in the document
    // before it by a scan (`rewired`), so its time grew with the square of the timeline:
    // 20,000 boxes (2.4 MB) took 1.7 s in a debug build against the gate's 70 ms, and a
    // document at the 16 MB limit (130,000 boxes) took over a minute to lose one. The edits
    // index the document once now, as the gate does.
    #[test]
    fn removing_or_moving_one_of_40000_boxes_promptly() {
        let document = boxes(40_000);
        let removed = promptly(&document, |d| chain::remove(d, &id("b0009c3f")));
        assert_eq!(removed.unwrap().index, 39_999);
        let moved = promptly(&document, |d| {
            chain::move_feature(d, &id("b0009c3f"), Some(&id("b0000000")))
        });
        assert_eq!(moved.unwrap().to, 0);
    }

    /// A box, then `n` pockets each on the one before (`h0000001` on the box), of `s0000002`.
    fn pocket_chain(n: usize) -> Document {
        let mut features = vec![BLOCK.to_owned(), S0000002.to_owned()];
        for i in 1..=n {
            let base = if i == 1 {
                "b0000001".to_owned()
            } else {
                format!("h{:07x}", i - 1)
            };
            features.push(format!(
                r#"{{"id": "h{i:07x}", "type": "pocket", "v": 1, "name": "H", "base": "{base}", "sketch": "s0000002", "extent": {{"type": "through_all", "reversed": true}}}}"#
            ));
        }
        document(&features.iter().map(String::as_str).collect::<Vec<_>>())
    }

    // Was (a break-it finding): a move walked the chain it joins finding each next member's
    // consumer by a scan of the timeline, and checked each member against those found by
    // another scan, so a move along a long chain was quadratic too (the walk, then
    // `rewired`).
    #[test]
    fn moving_the_first_of_10000_pockets_to_the_chains_end_promptly() {
        let moved = promptly(&pocket_chain(10_000), |d| {
            chain::move_feature(d, &id("h0000001"), None)
        })
        .unwrap();
        // The chain b, h2, ..., h10000, h1: two bases rewired.
        assert_eq!(
            moved.rewired,
            [
                rewired("h0000002", "base", Some("h0000001"), Some("b0000001")),
                rewired("h0000001", "base", Some("b0000001"), Some("h0002710")),
            ]
        );
        validate(&moved.document).unwrap();
    }

    /// A box, then `n` fillets each on the one before (`r0000001` on the box), each naming
    /// the box's edge.
    fn fillet_chain(n: usize) -> Document {
        let mut features = vec![BLOCK.to_owned()];
        for i in 1..=n {
            let base = if i == 1 {
                "b0000001".to_owned()
            } else {
                format!("r{:07x}", i - 1)
            };
            features.push(format!(
                r#"{{"id": "r{i:07x}", "type": "fillet", "v": 1, "name": "R", "base": "{base}", "edges": ["b0000001.box.Edge1"], "radius": 0.5}}"#
            ));
        }
        document(&features.iter().map(String::as_str).collect::<Vec<_>>())
    }

    // A review finding: the lineage rule walked each fillet's chain up to the box, so an
    // edit of a long chain of them was quadratic (8,000 took 31 s against the gate's 15 ms).
    #[test]
    fn removing_or_moving_one_of_10000_fillets_naming_the_box_promptly() {
        let fillets = fillet_chain(10_000);
        let removed = promptly(&fillets, |d| chain::remove(d, &id("r0002710"))).unwrap();
        assert_eq!(removed.index, 10_000);
        assert_eq!(removed.rewired, []);
        let moved = promptly(&fillets, |d| chain::move_feature(d, &id("r0000001"), None)).unwrap();
        assert_eq!(
            moved.rewired,
            [
                rewired("r0000002", "base", Some("r0000001"), Some("b0000001")),
                rewired("r0000001", "base", Some("b0000001"), Some("r0002710")),
            ]
        );
    }

    // Was (a break-it finding): the refusal listed every earlier solid nothing consumes as a
    // candidate, all 20,000 of them (a 200 KB error), where a broken reference lists five;
    // and each candidate was found by a scan of the timeline for its consumer, so the
    // refusal was quadratic.
    #[test]
    fn a_roots_removal_refused_among_20000_unconsumed_boxes_lists_the_first_five_promptly() {
        let mut features: Vec<String> = (0..20_000)
            .map(|i| {
                format!(
                    r#"{{"id": "a{i:07x}", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}}"#
                )
            })
            .collect();
        features.push(BLOCK.to_owned());
        features.push(S0000002.to_owned());
        features.push(H0000001.replace("p0000001", "b0000001"));
        let document = document(&features.iter().map(String::as_str).collect::<Vec<_>>());
        let error = promptly(&document, |d| chain::remove(d, &id("b0000001"))).unwrap_err();
        assert_eq!(error.feature(), Some("b0000001"));
        assert_eq!(
            error.candidates(),
            ["a0000000", "a0000001", "a0000002", "a0000003", "a0000004"]
        );
    }

    #[test]
    fn an_insert_before_its_own_input_is_left_to_the_gate_which_refuses_it() {
        // A chamfer on h0000001 placed before p0000001: the chain rewires h0000002 to it,
        // and the gate refuses the result, naming the chamfer's base as coming after it.
        let inserted = insert(&bracket(), feature(CHAMFER), Some(&id("p0000001"))).unwrap();
        let invalid = validate(&inserted.document).unwrap_err();
        assert!(
            invalid
                .errors()
                .iter()
                .any(|e| e.feature() == Some("c0000002")
                    && e.parameter() == Some("base")
                    && e.message().contains("comes after this feature")),
            "{invalid:?}"
        );
    }

    #[test]
    fn an_insert_under_an_id_the_document_has_is_refused_by_the_gate() {
        // A pocket given h0000002's ID, on h0000001 before h0000002: the gate refuses the
        // result for the ID twice; the original is untouched.
        let pocket = r#"{"id": "h0000002", "type": "pocket", "v": 1, "name": "Again", "base": "h0000001", "sketch": "s0000003", "extent": {"type": "through_all", "reversed": true}}"#;
        let original = bracket();
        let inserted = insert(&original, feature(pocket), Some(&id("h0000002"))).unwrap();
        let invalid = validate(&inserted.document).unwrap_err();
        assert!(
            invalid.errors().iter().any(|e| e.parameter() == Some("id")),
            "{invalid:?}"
        );
        assert!(original.canonically_eq(&bracket()));
    }

    #[test]
    fn a_stored_names_makers_are_each_feature_it_writes_once_through_every_kind_of_source() {
        use silico_core::naming::{Maker, Name};
        // Upper, lower, derived, and nested names, each feature as first written, once.
        let fixtures = document(&[
            r#"{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#,
            r#"{"id": "bbbbbbbb", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}"#,
            r#"{"id": "cccccccc", "type": "cut", "v": 1, "name": "C", "base": "aaaaaaaa", "tool": "bbbbbbbb"}"#,
            r#"{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "cccccccc", "edges": ["aaaaaaaa.box.Edge1"], "radius": 1}"#,
        ]);
        let maker = |id: &FeatureId| {
            fixtures
                .features
                .iter()
                .find(|f| f.id == *id)
                .map(|f| Maker::of(&f.operation))
        };
        for (text, expected) in [
            ("aaaaaaaa.box.Edge1", &["aaaaaaaa"][..]),
            (
                "cccccccc.cut.Edge(G aaaaaaaa.box.Face6, G2/3 bbbbbbbb.box.Face1)",
                &["cccccccc", "aaaaaaaa", "bbbbbbbb"],
            ),
            (
                "ffffffff.fillet.Vertex(U2/3 aaaaaaaa.box.Face6, aaaaaaaa.box.Edge1)",
                &["ffffffff", "aaaaaaaa"],
            ),
            (
                "ffffffff.fillet.Edge(U cccccccc.cut.Edge(G bbbbbbbb.box.Face6))",
                &["ffffffff", "cccccccc", "bbbbbbbb"],
            ),
            (
                "ffffffff.fillet.Face(L aaaaaaaa.box.Edge1, aaaaaaaa.box.Edge2)",
                &["ffffffff", "aaaaaaaa"],
            ),
            (
                "ffffffff.fillet.Vertex(U2/3 aaaaaaaa.box.Face6, bbbbbbbb.box.Edge1)",
                &["ffffffff", "aaaaaaaa", "bbbbbbbb"],
            ),
            (
                "ffffffff.fillet.Face(L aaaaaaaa.box.Edge1, bbbbbbbb.box.Edge2)",
                &["ffffffff", "aaaaaaaa", "bbbbbbbb"],
            ),
            (
                "ffffffff.fillet.Face(G cccccccc.cut.Edge(G aaaaaaaa.box.Face6, G2/3 bbbbbbbb.box.Face1))",
                &["ffffffff", "cccccccc", "aaaaaaaa", "bbbbbbbb"],
            ),
        ] {
            let name = Name::parse(text, &maker).unwrap_or_else(|e| panic!("{text}: {e:?}"));
            let makers: Vec<&str> = name.features().iter().map(|id| id.as_str()).collect();
            assert_eq!(makers, expected, "{text}");
        }
    }

    // ---------------------------------------------------------------------------------
    // The chain model, and random documents the gate takes.

    /// A feature's `base`: none (a primitive's, a sketch's), optional (a pad's, a
    /// revolve's), or needed.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum BaseRule {
        Without,
        Optional,
        Needed,
    }

    fn rule(operation: &Operation) -> BaseRule {
        match operation {
            Operation::Pad { .. } | Operation::Revolve { .. } => BaseRule::Optional,
            Operation::Fuse { .. }
            | Operation::Cut { .. }
            | Operation::Common { .. }
            | Operation::Fillet { .. }
            | Operation::Chamfer { .. }
            | Operation::Pocket { .. }
            | Operation::Groove { .. } => BaseRule::Needed,
            _ => BaseRule::Without,
        }
    }

    fn base_of(f: &Feature) -> Option<String> {
        f.operation
            .inputs()
            .into_iter()
            .find(|(field, _)| *field == "base")
            .map(|(_, b)| b.to_string())
    }

    fn tool_of(f: &Feature) -> Option<String> {
        f.operation
            .inputs()
            .into_iter()
            .find(|(field, _)| *field == "tool")
            .map(|(_, t)| t.to_string())
    }

    /// For each of a feature's stored names, the features of `document` it names, read off
    /// the text: every word of it that is one of their IDs.
    fn names(f: &Feature, document: &Document) -> Vec<Vec<String>> {
        f.operation
            .edges()
            .iter()
            .map(|edge| {
                edge.as_str()
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .filter(|word| document.features.iter().any(|g| g.id.as_str() == *word))
                    .map(str::to_owned)
                    .collect()
            })
            .collect()
    }

    /// Every feature of `document` a feature's stored names name.
    fn named(f: &Feature, document: &Document) -> Vec<String> {
        names(f, document).concat()
    }

    /// `f` with its `base` and `tool` as given, where given: `Some(None)` leaves a pad
    /// without a base.
    fn with_inputs(f: &Feature, base: Option<Option<&str>>, tool: Option<&str>) -> Feature {
        let mut f = f.clone();
        if let Some(to) = base {
            match &mut f.operation {
                Operation::Pad { base, .. } | Operation::Revolve { base, .. } => {
                    *base = to.map(id);
                }
                Operation::Fuse { base, .. }
                | Operation::Cut { base, .. }
                | Operation::Common { base, .. }
                | Operation::Fillet { base, .. }
                | Operation::Chamfer { base, .. }
                | Operation::Pocket { base, .. }
                | Operation::Groove { base, .. } => *base = id(to.unwrap()),
                _ => {}
            }
        }
        if let Some(to) = tool
            && let Operation::Fuse { tool, .. }
            | Operation::Cut { tool, .. }
            | Operation::Common { tool, .. } = &mut f.operation
        {
            *tool = id(to);
        }
        f
    }

    /// Each feature's chain, by `base` links: the ID of its root.
    fn roots(document: &Document) -> BTreeMap<String, String> {
        let by_id: BTreeMap<String, &Feature> = document
            .features
            .iter()
            .map(|f| (f.id.to_string(), f))
            .collect();
        let mut roots = BTreeMap::new();
        for f in &document.features {
            let mut at = f.id.to_string();
            let mut seen = 0;
            while let Some(base) = by_id.get(&at).and_then(|f| base_of(f)) {
                at = base;
                seen += 1;
                assert!(seen <= document.features.len(), "a cycle");
            }
            roots.insert(f.id.to_string(), at);
        }
        roots
    }

    /// The document after `order` (the original's IDs, in the new order, `gone` left out),
    /// each chain of `chains` (by its features' roots in the original) linked in that order,
    /// and each boolean's tool the last of its tool's chain; or none where a chain's first
    /// needs a base, a later one takes none, or the moved feature `made_tool` would become a
    /// tool it wasn't.
    fn relinked(
        original: &Document,
        order: &[String],
        made_tool: Option<&str>,
    ) -> Option<Document> {
        let roots = roots(original);
        let by_id: BTreeMap<String, &Feature> = original
            .features
            .iter()
            .map(|f| (f.id.to_string(), f))
            .collect();
        let mut chains: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for id in order {
            if by_id[id].operation.makes_solid() {
                chains.entry(roots[id].as_str()).or_default().push(id);
            }
        }
        let mut bases: BTreeMap<&str, Option<&str>> = BTreeMap::new();
        for members in chains.values() {
            for (i, &m) in members.iter().enumerate() {
                match (i, rule(&by_id[m].operation)) {
                    (0, BaseRule::Needed) => return None,
                    (0, _) => {
                        bases.insert(m, None);
                    }
                    (_, BaseRule::Without) => return None,
                    (_, _) => {
                        bases.insert(m, Some(members[i - 1]));
                    }
                }
            }
        }
        let mut features = Vec::new();
        for id in order {
            let f = by_id[id];
            let base = match rule(&f.operation) {
                BaseRule::Without => None,
                _ => Some(bases[id.as_str()]),
            };
            let tool = match tool_of(f) {
                Some(old) => {
                    let last = *chains.get(roots[&old].as_str())?.last()?;
                    if Some(last) == made_tool && old != last {
                        return None;
                    }
                    Some(last)
                }
                None => None,
            };
            features.push(with_inputs(f, base, tool));
        }
        let mut document = original.clone();
        document.features = features;
        validate(&document).ok()?;
        lineage_kept(original, &document).then_some(document)
    }

    /// Every feature `id` is built from in `document`: itself, its inputs, and the sketches
    /// they sweep, up every link.
    fn built_from(document: &Document, id: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut todo = vec![id.to_owned()];
        while let Some(at) = todo.pop() {
            if found.insert(at.clone())
                && let Some(f) = document.features.iter().find(|f| f.id.as_str() == at)
            {
                todo.extend(f.operation.uses().map(|u| u.to_string()));
            }
        }
        found
    }

    /// The lineage rule (pending Trevin): each stored name in `edited` whose makers its
    /// feature's base in `original` was built from, every one, has a base built from them
    /// still.
    fn lineage_kept(original: &Document, edited: &Document) -> bool {
        edited.features.iter().all(|f| {
            let Some(base) = base_of(f) else {
                return true;
            };
            let was = original
                .features
                .iter()
                .find(|o| o.id == f.id)
                .and_then(base_of)
                .map(|b| built_from(original, &b))
                .unwrap_or_default();
            let now = built_from(edited, &base);
            names(f, edited).iter().all(|makers| {
                let makers: Vec<&String> = makers.iter().filter(|m| **m != f.id.as_str()).collect();
                !makers.iter().all(|m| was.contains(*m)) || makers.iter().all(|m| now.contains(*m))
            })
        })
    }

    /// What the design says a move of `id` before `before` gives: the document, or a
    /// refusal (none); unchanged where it stays.
    fn move_model(original: &Document, id: &str, before: Option<&str>) -> Option<(Document, bool)> {
        let ids: Vec<String> = original.features.iter().map(|f| f.id.to_string()).collect();
        let from = ids.iter().position(|i| i == id).unwrap();
        let target = before.map_or(ids.len(), |b| ids.iter().position(|i| i == b).unwrap());
        if target == from || target == from + 1 {
            return Some((original.clone(), false));
        }
        let mut order = ids.clone();
        let moving = order.remove(from);
        let to = if target > from { target - 1 } else { target };
        order.insert(to, moving);
        relinked(original, &order, Some(id)).map(|d| (d, true))
    }

    /// What A9 says removing `id` gives, or a refusal.
    fn remove_model(original: &Document, id: &str) -> Option<Document> {
        if original.features.iter().any(|f| {
            f.operation.sketch().is_some_and(|s| s.as_str() == id)
                || named(f, original).iter().any(|n| n == id)
        }) {
            return None;
        }
        let order: Vec<String> = original
            .features
            .iter()
            .map(|f| f.id.to_string())
            .filter(|i| i != id)
            .collect();
        relinked(original, &order, None)
    }

    /// A document the gate takes, made from `choices`: boxes, pads and revolves (alone or on
    /// a body), pockets, grooves, fillets and chamfers of one to three edges of boxes and of
    /// pads (on a body or not), booleans of two bodies, and a second sketch.
    fn generated(choices: &[(u8, u8, u8)]) -> Document {
        let mut texts = vec![S0000001.to_owned()];
        let mut sketches = vec!["s0000001"];
        let mut tips: Vec<String> = Vec::new();
        // Edges a fillet or chamfer can name: each box's, and each pad of s0000001's, whose
        // maker isn't its chain's root where the pad has a base.
        let mut nameable: Vec<String> = Vec::new();
        for (n, &(kind, a, b)) in choices.iter().enumerate() {
            let pick = |list: &Vec<String>, k: u8| list[k as usize % list.len()].clone();
            let sketch = sketches[b as usize % sketches.len()];
            let mut kind = kind % 11;
            if (matches!(kind, 2..=4 | 9 | 10) && tips.is_empty())
                || (matches!(kind, 4 | 10) && nameable.is_empty())
                || (kind == 5 && tips.len() < 2)
                || (kind == 6 && sketches.len() == 2)
            {
                kind = 0;
            }
            if kind == 8 && tips.is_empty() {
                kind = 1;
            }
            // One to three different boxes' edges, for a fillet or chamfer to name.
            let edges = || {
                let mut named: Vec<String> = Vec::new();
                for k in 0..=(b / 86) as usize {
                    let one = format!(r#""{}""#, nameable[(b as usize + 5 * k) % nameable.len()]);
                    if !named.contains(&one) {
                        named.push(one);
                    }
                }
                format!("[{}]", named.join(", "))
            };
            let id = |prefix: char| format!("{prefix}{n:07}");
            let text = match kind {
                8 if a % 2 == 0 => {
                    let id = id('v');
                    tips.push(id.clone());
                    format!(
                        r#"{{"id": "{id}", "type": "revolve", "v": 1, "name": "V", "sketch": "{sketch}", "axis": "x_axis", "extent": {{"type": "one_side", "angle": 90, "reversed": false}}}}"#
                    )
                }
                0 | 7 => {
                    let id = id('b');
                    tips.push(id.clone());
                    nameable.push(format!("{id}.box.Edge1"));
                    format!(
                        r#"{{"id": "{id}", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 10, "width": 10, "height": 10}}"#
                    )
                }
                1 => {
                    let id = id('p');
                    tips.push(id.clone());
                    if sketch == "s0000001" {
                        nameable.push(format!("{id}.pad.Edge(G s0000001.sketch.Vertex.pa000004)"));
                    }
                    format!(
                        r#"{{"id": "{id}", "type": "pad", "v": 1, "name": "P", "sketch": "{sketch}", "extent": {{"type": "one_side", "length": 5, "reversed": false}}}}"#
                    )
                }
                2..=4 | 8..=10 => {
                    let base = pick(&tips, a);
                    let (prefix, rest) = match kind {
                        2 => (
                            'p',
                            format!(
                                r#""type": "pad", "v": 1, "name": "P", "base": "{base}", "sketch": "{sketch}", "extent": {{"type": "one_side", "length": 5, "reversed": false}}"#
                            ),
                        ),
                        3 => (
                            'h',
                            format!(
                                r#""type": "pocket", "v": 1, "name": "H", "base": "{base}", "sketch": "{sketch}", "extent": {{"type": "through_all", "reversed": true}}"#
                            ),
                        ),
                        4 => (
                            'r',
                            format!(
                                r#""type": "fillet", "v": 1, "name": "R", "base": "{base}", "edges": {}, "radius": 0.5"#,
                                edges()
                            ),
                        ),
                        8 => (
                            'v',
                            format!(
                                r#""type": "revolve", "v": 1, "name": "V", "base": "{base}", "sketch": "{sketch}", "axis": "x_axis", "extent": {{"type": "one_side", "angle": 90, "reversed": false}}"#
                            ),
                        ),
                        9 => (
                            'g',
                            format!(
                                r#""type": "groove", "v": 1, "name": "G", "base": "{base}", "sketch": "{sketch}", "axis": "x_axis", "extent": {{"type": "one_side", "angle": 90, "reversed": false}}"#
                            ),
                        ),
                        _ => (
                            'c',
                            format!(
                                r#""type": "chamfer", "v": 1, "name": "C", "base": "{base}", "edges": {}, "size": {{"distance": 1}}"#,
                                edges()
                            ),
                        ),
                    };
                    let id = id(prefix);
                    tips.retain(|t| *t != base);
                    tips.push(id.clone());
                    if kind == 2 && sketch == "s0000001" {
                        nameable.push(format!("{id}.pad.Edge(G s0000001.sketch.Vertex.pa000004)"));
                    }
                    format!(r#"{{"id": "{id}", {rest}}}"#)
                }
                5 => {
                    let base = pick(&tips, a);
                    let others: Vec<String> =
                        tips.iter().filter(|t| **t != base).cloned().collect();
                    let tool = pick(&others, b);
                    let op = ["fuse", "cut", "common"][(a / 7) as usize % 3];
                    let id = id('j');
                    tips.retain(|t| *t != base && *t != tool);
                    tips.push(id.clone());
                    format!(
                        r#"{{"id": "{id}", "type": "{op}", "v": 1, "name": "J", "base": "{base}", "tool": "{tool}"}}"#
                    )
                }
                _ => {
                    sketches.push("s0000002");
                    S0000002.to_owned()
                }
            };
            texts.push(text);
        }
        let document = document(&texts.iter().map(String::as_str).collect::<Vec<_>>());
        validate(&document).unwrap_or_else(|e| panic!("the generator: {e:?}"));
        document
    }

    fn choices() -> impl Strategy<Value = Vec<(u8, u8, u8)>> {
        proptest::collection::vec((any::<u8>(), any::<u8>(), any::<u8>()), 1..9)
    }

    proptest! {
        #![proptest_config(config(1500))]

        /// Every move of every feature before every place (and to the end), against the
        /// model: refused where it refuses, else the model's document; each result through
        /// the gate; each refusal structured; each move undone by the move back.
        #[test]
        fn every_move_is_the_models_and_the_move_back_undoes_it(choices in choices()) {
            let original = generated(&choices);
            let all: Vec<String> = original.features.iter().map(|f| f.id.to_string()).collect();
            for f in &all {
                for before in all.iter().map(Some).chain([None]) {
                    let got = move_feature(&original, &id(f), before.map(|b| id(b)).as_ref());
                    let want = move_model(&original, f, before.map(String::as_str));
                    let at = format!("{f} before {before:?} in {all:?}");
                    match (got, want) {
                        (Err(error), None) => {
                            prop_assert_eq!(error.code(), ErrorCode::InvalidInput, "{}", at);
                            prop_assert!(error.feature().is_some(), "{}: {}", at, error);
                            prop_assert!(error.suggestion().is_some(), "{}: {}", at, error);
                        }
                        (Ok(moved), Some((expected, changed))) => {
                            prop_assert!(validate(&moved.document).is_ok(), "{}", at);
                            prop_assert!(
                                moved.document.canonically_eq(&expected),
                                "{}: got {:?}, the model {:?}",
                                at,
                                moved.document.features.iter().map(|f| (f.id.to_string(), base_of(f), tool_of(f))).collect::<Vec<_>>(),
                                expected.features.iter().map(|f| (f.id.to_string(), base_of(f), tool_of(f))).collect::<Vec<_>>()
                            );
                            prop_assert_eq!(moved.changed, changed, "{}", at);
                            prop_assert_eq!(moved.document.features[moved.to].id.as_str(), f.as_str(), "{}", at);
                            let from = all.iter().position(|i| i == f).unwrap();
                            let back = all.get(from + 1).map(|b| id(b));
                            // The move back, where the model takes it (a move never makes a
                            // feature a tool, so a tool chain's end moved in can't move back).
                            if changed && move_model(&moved.document, f, back.as_ref().map(FeatureId::as_str)).is_some() {
                                let undone = move_feature(&moved.document, &id(f), back.as_ref());
                                prop_assert!(
                                    undone.as_ref().is_ok_and(|u| u.document.canonically_eq(&original)),
                                    "{}: moved back, {:?}",
                                    at,
                                    undone.map(|u| u.document.features.iter().map(|f| (f.id.to_string(), base_of(f), tool_of(f))).collect::<Vec<_>>()).map_err(|e| e.to_string())
                                );
                            }
                        }
                        (Ok(moved), None) => prop_assert!(
                            false,
                            "{}: taken, the model refuses: {:?}, the gate {:?}",
                            at,
                            moved.document.features.iter().map(|f| (f.id.to_string(), base_of(f), tool_of(f))).collect::<Vec<_>>(),
                            validate(&moved.document).err()
                        ),
                        (Err(error), Some(_)) => prop_assert!(false, "{}: refused, the model takes it: {}", at, error),
                    }
                }
            }
        }

        /// Every removal against A9's model, and each undone, where the feature had inputs,
        /// by adding it back before the feature that followed it.
        #[test]
        fn every_removal_is_the_models_and_adding_it_back_undoes_it(choices in choices()) {
            let original = generated(&choices);
            for (index, f) in original.features.iter().enumerate() {
                let got = remove(&original, &f.id);
                let want = remove_model(&original, f.id.as_str());
                let at = format!("{} in {:?}", f.id, ids(&original));
                match (got, want) {
                    (Err(error), None) => {
                        prop_assert_eq!(error.feature(), Some(f.id.as_str()), "{}", at);
                        prop_assert!(error.suggestion().is_some(), "{}", at);
                    }
                    (Ok(removed), Some(expected)) => {
                        prop_assert!(removed.document.canonically_eq(&expected), "{}", at);
                        prop_assert_eq!(removed.index, index, "{}", at);
                        prop_assert!(removed.feature.canonically_eq(f), "{}", at);
                        if !f.operation.inputs().is_empty() {
                            let next = original.features.get(index + 1).map(|n| n.id.clone());
                            let back = insert(&removed.document, removed.feature.clone(), next.as_ref());
                            prop_assert!(
                                back.as_ref().is_ok_and(|b| b.document.canonically_eq(&original) && b.index == index),
                                "{}: added back, {:?}",
                                at,
                                back.map(|b| ids(&b.document).join(",")).map_err(|e| e.to_string())
                            );
                        }
                    }
                    (Ok(removed), None) => prop_assert!(false, "{}: taken, the model refuses: {:?}", at, removed.rewired),
                    (Err(error), Some(_)) => prop_assert!(false, "{}: refused, the model takes it: {}", at, error),
                }
            }
        }

        /// A pocket of `s0000001` inserted on every solid before every place: where its
        /// base's consumer comes at or after the place, that consumer is rewired to it and
        /// nothing else changes; else refused. Either way, a result the gate takes is undone
        /// by removing the pocket.
        #[test]
        fn every_insert_rewires_only_its_bases_consumer_and_its_removal_undoes_it(choices in choices()) {
            let original = generated(&choices);
            let all: Vec<String> = original.features.iter().map(|f| f.id.to_string()).collect();
            for base in original.features.iter().filter(|f| f.operation.makes_solid()) {
                for (place, before) in all.iter().map(Some).chain([None]).enumerate() {
                    let pocket = feature(&format!(
                        r#"{{"id": "hzzzzzzz", "type": "pocket", "v": 1, "name": "New", "base": "{}", "sketch": "s0000001", "extent": {{"type": "through_all", "reversed": true}}}}"#,
                        base.id
                    ));
                    let got = insert(&original, pocket.clone(), before.map(|b| id(b)).as_ref());
                    let consumer = original.features.iter().position(|f| {
                        f.operation.inputs().iter().any(|(_, i)| *i == &base.id)
                    });
                    let at = format!("on {} before {before:?} in {all:?}", base.id);
                    match (got, consumer) {
                        (Err(error), Some(c)) if c < place => {
                            prop_assert_eq!(error.parameter(), Some("before"), "{}", at);
                        }
                        (Err(error), _) => prop_assert!(false, "{}: refused: {}", at, error),
                        (Ok(_), Some(c)) if c < place => prop_assert!(false, "{}: taken past its base's consumer", at),
                        (Ok(inserted), consumer) => {
                            prop_assert_eq!(inserted.index, place, "{}", at);
                            let expected_rewired: Vec<Rewired> = consumer
                                .map(|c| {
                                    let user = &original.features[c];
                                    let field = if base_of(user).as_deref() == Some(base.id.as_str()) { "base" } else { "tool" };
                                    Rewired { feature: user.id.clone(), field, from: Some(base.id.clone()), to: Some(id("hzzzzzzz")) }
                                })
                                .into_iter()
                                .collect();
                            prop_assert_eq!(&inserted.rewired, &expected_rewired, "{}", at);
                            if validate(&inserted.document).is_ok() {
                                let undone = remove(&inserted.document, &id("hzzzzzzz"));
                                prop_assert!(
                                    undone.as_ref().is_ok_and(|u| u.document.canonically_eq(&original)),
                                    "{}: removed again, {:?}",
                                    at,
                                    undone.map(|u| u.rewired).map_err(|e| e.to_string())
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------------------------
    // Documents of references to anything, themselves, later features, nothing, and IDs
    // twice: the gate refuses most, and no edit of one it takes may panic or hang.

    fn unchecked(choices: &[(u8, u8, u8, u8)]) -> Document {
        const IDS: [&str; 6] = [
            "a0000000", "a0000001", "a0000002", "a0000003", "a0000004", "zzzzzzzz",
        ];
        let texts: Vec<String> = choices
            .iter()
            .enumerate()
            .map(|(n, &(kind, a, b, c))| {
                let own = IDS[(c as usize) % 5].to_owned();
                let own = if c >= 200 { own } else { format!("a{n:07}") };
                let base = IDS[a as usize % IDS.len()];
                let tool = IDS[b as usize % IDS.len()];
                match kind % 7 {
                    0 => format!(r#"{{"id": "{own}", "type": "box", "v": 1, "name": "B", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}}"#),
                    1 => format!(r#"{{"id": "{own}", "type": "pad", "v": 1, "name": "P", "sketch": "{tool}", "extent": {{"type": "one_side", "length": 5, "reversed": false}}}}"#),
                    2 => format!(r#"{{"id": "{own}", "type": "pad", "v": 1, "name": "P", "base": "{base}", "sketch": "{tool}", "extent": {{"type": "one_side", "length": 5, "reversed": false}}}}"#),
                    3 => format!(r#"{{"id": "{own}", "type": "pocket", "v": 1, "name": "H", "base": "{base}", "sketch": "{tool}", "extent": {{"type": "through_all", "reversed": true}}}}"#),
                    4 => format!(r#"{{"id": "{own}", "type": "fillet", "v": 1, "name": "R", "base": "{base}", "edges": ["{tool}.box.Edge1"], "radius": 0.5}}"#),
                    5 => format!(r#"{{"id": "{own}", "type": "cut", "v": 1, "name": "J", "base": "{base}", "tool": "{tool}"}}"#),
                    _ => S0000002.replace("s0000002", &own),
                }
            })
            .collect();
        document(&texts.iter().map(String::as_str).collect::<Vec<_>>())
    }

    proptest! {
        #![proptest_config(config(1500))]

        #[test]
        fn no_edit_of_a_generated_document_the_gate_takes_panics_or_hangs(
            choices in proptest::collection::vec((any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>()), 1..8)
        ) {
            let Ok(document) = Checked::new(unchecked(&choices)) else {
                return Ok(());
            };
            let (sender, receiver) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                let result = std::panic::catch_unwind(|| {
                    let features = &document.document().features;
                    let mut all: Vec<FeatureId> = features.iter().map(|f| f.id.clone()).collect();
                    all.push(id("zzzzzzzz"));
                    for f in &all {
                        let _ = chain::remove(&document, f);
                        for before in all.iter().map(Some).chain([None]) {
                            let _ = chain::move_feature(&document, f, before);
                            if let Some(source) = features.iter().find(|x| x.id == *f) {
                                let _ = chain::insert(&document, source.clone(), before);
                            }
                        }
                    }
                });
                let _ = sender.send(result.map_err(|e| {
                    e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned())).unwrap_or_default()
                }));
            });
            let outcome = receiver.recv_timeout(Duration::from_secs(20));
            prop_assert!(outcome.is_ok(), "hung on {:?}", choices);
            let _ = worker.join();
            prop_assert!(outcome.unwrap().is_ok(), "panicked on {:?}", choices);
        }
    }
}
