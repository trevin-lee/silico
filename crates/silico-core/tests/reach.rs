//! What an edit reaches (docs/design/m4.1-command-api.md, section 5; decision 28), on
//! documents written by hand: the features whose own values changed, compared by ID, those
//! added, and everything that uses them through what it consumes and the sketch it sweeps.
//! Every expected set is read off the documents by hand.

use std::collections::BTreeSet;

use silico_core::document::{Document, FeatureId, reached, read, with_users};

/// Two sketches, a pad of the first, a pocket of the second in the pad, a box apart, a
/// second box, and a cut of the second box from the pocket: who uses whom is
/// s0000001 → p0000001 → q0000001 → c0000001, s0000002 → q0000001, and b0000001 →
/// c0000001; k7f2qx3m stands alone.
const PART: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [
  {"id": "s0000001", "type": "sketch", "v": 1, "name": "Outline", "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []},
  {"id": "p0000001", "type": "pad", "v": 1, "name": "Plate", "sketch": "s0000001", "extent": {"type": "one_side", "length": 10, "reversed": false}},
  {"id": "s0000002", "type": "sketch", "v": 1, "name": "Hole", "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []},
  {"id": "q0000001", "type": "pocket", "v": 1, "name": "Bore", "base": "p0000001", "sketch": "s0000002", "extent": {"type": "through_all", "reversed": true}},
  {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10},
  {"id": "b0000001", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0], "length": 10, "width": 10, "height": 10},
  {"id": "c0000001", "type": "cut", "v": 1, "name": "Notch", "base": "q0000001", "tool": "b0000001"}
]}"#;

fn part() -> Document {
    read(PART.as_bytes()).unwrap()
}

/// [`PART`] with `from` replaced by `to` (exactly once).
fn edited(from: &str, to: &str) -> Document {
    assert_eq!(PART.matches(from).count(), 1, "{from}");
    read(PART.replacen(from, to, 1).as_bytes()).unwrap()
}

fn ids(texts: &[&str]) -> BTreeSet<FeatureId> {
    texts.iter().map(|t| FeatureId::new(t).unwrap()).collect()
}

#[test]
fn nothing_changed_reaches_nothing() {
    assert_eq!(reached(&part(), &part()), ids(&[]));
}

#[test]
fn a_pads_value_reaches_it_and_what_is_made_from_it() {
    let after = edited(
        r#""length": 10, "reversed": false"#,
        r#""length": 12, "reversed": false"#,
    );
    assert_eq!(
        reached(&part(), &after),
        ids(&["p0000001", "q0000001", "c0000001"])
    );
}

#[test]
fn a_sketch_reaches_what_sweeps_it() {
    // The second sketch, on another plane: the pocket sweeps it, and the cut uses the pocket.
    let after = edited(
        r#""name": "Hole", "plane": "xy""#,
        r#""name": "Hole", "plane": "xz""#,
    );
    assert_eq!(
        reached(&part(), &after),
        ids(&["s0000002", "q0000001", "c0000001"])
    );
}

#[test]
fn a_tool_reaches_what_consumes_it_and_nothing_before() {
    let after = edited(
        r#""position": [100, 0, 0], "length": 10"#,
        r#""position": [100, 0, 0], "length": 11"#,
    );
    assert_eq!(reached(&part(), &after), ids(&["b0000001", "c0000001"]));
}

#[test]
fn a_name_a_note_and_a_fingerprint_reach_nothing() {
    let after = edited(
        r#""name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#,
        r#""name": "Block, renamed", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10, "note": "a note", "fingerprint": {"volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}"#,
    );
    assert_eq!(reached(&part(), &after), ids(&[]));
}

#[test]
fn a_feature_made_another_type_under_its_id_is_reached() {
    let after = edited(
        r#""type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10"#,
        r#""type": "sphere", "v": 1, "name": "Block", "position": [0, 0, 0], "radius": 5"#,
    );
    assert_eq!(reached(&part(), &after), ids(&["k7f2qx3m"]));
}

#[test]
fn a_feature_added_is_reached_and_one_removed_reaches_nothing() {
    let added = edited(
        r#""tool": "b0000001"}"#,
        r#""tool": "b0000001"}, {"id": "z0000001", "type": "sphere", "v": 1, "name": "Ball", "position": [0, 0, 0], "radius": 5}"#,
    );
    assert_eq!(reached(&part(), &added), ids(&["z0000001"]));
    // The cut removed: the features it used are as they were.
    let removed = edited(
        r#",
  {"id": "c0000001", "type": "cut", "v": 1, "name": "Notch", "base": "q0000001", "tool": "b0000001"}"#,
        "",
    );
    assert_eq!(reached(&part(), &removed), ids(&[]));
}

#[test]
fn a_feature_that_changed_what_it_uses_is_reached_with_what_uses_it() {
    // The cut's tool is now the block: the cut changed, not the block or the lug.
    let after = edited(r#""tool": "b0000001""#, r#""tool": "k7f2qx3m""#);
    assert_eq!(reached(&part(), &after), ids(&["c0000001"]));
}

#[test]
fn with_users_follows_every_use_and_leaves_out_ids_the_document_hasnt() {
    assert_eq!(
        with_users(&part(), &ids(&["s0000001", "zzzzzzzz"])),
        ids(&["s0000001", "p0000001", "q0000001", "c0000001"])
    );
    assert_eq!(with_users(&part(), &ids(&["k7f2qx3m"])), ids(&["k7f2qx3m"]));
}
