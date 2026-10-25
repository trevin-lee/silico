//! What an edit reaches (docs/design/m4.1-command-api.md, section 5; decision 28), on
//! documents written by hand: the features whose own values changed, compared by ID, those
//! added, and everything that uses them through what it consumes and the sketch it sweeps.
//! Every expected set is read off the documents by hand.

use std::collections::BTreeSet;
use std::time::Duration;

use silico_core::document::{
    Before, Document, FeatureId, QuotedIds, net_reach, reached, read, with_users,
};

use crate::within::within;

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

/// A sketch whose lists aren't in ID order (its points pb then pa, its constraints
/// k0000002 then k0000001), and a pad of it.
const UNSORTED: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [
  {"id": "s0000001", "type": "sketch", "v": 1, "name": "Sketch", "plane": "xy", "points": [{"id": "pb000000", "x": 10, "y": 0, "construction": false}, {"id": "pa000000", "x": 0, "y": 0, "construction": false}], "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}], "circles": [], "arcs": [], "constraints": [{"id": "k0000002", "type": "length", "line": "na000000", "value": 10, "driving": true}, {"id": "k0000001", "type": "horizontal", "line": "na000000"}]},
  {"id": "p0000001", "type": "pad", "v": 1, "name": "Plate", "sketch": "s0000001", "extent": {"type": "one_side", "length": 10, "reversed": false}}
]}"#;

#[test]
fn canonical_order_alone_reaches_nothing() {
    let file = read(UNSORTED.as_bytes()).unwrap();
    // Its canonical text, read again: the sketch's lists in ID order.
    let canonical = read(silico_core::document::write(&file).unwrap().as_bytes()).unwrap();
    assert_ne!(file, canonical, "the lists are held in another order");
    assert!(reached(&file, &canonical).is_empty());
    assert!(reached(&canonical, &file).is_empty());
    assert!(net_reach(Before::Read(&file), &BTreeSet::new(), &canonical).is_empty());
    // A point moved in either order is still the sketch's change, and the pad's.
    let moved = read(
        UNSORTED
            .replacen(r#""x": 10, "y": 0"#, r#""x": 11, "y": 0"#, 1)
            .as_bytes(),
    )
    .unwrap();
    assert_eq!(reached(&canonical, &moved), ids(&["s0000001", "p0000001"]));
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

#[test]
fn net_reach_carries_what_was_reached_and_an_edit_undone_reaches_nothing() {
    // An edit undone (the pad 12 long, then 10 again) leaves the part as it was: nothing.
    assert_eq!(
        net_reach(Before::Read(&part()), &ids(&[]), &part()),
        ids(&[])
    );
    // The lug carried from before (its record unwritten): it and the cut that uses it.
    assert_eq!(
        net_reach(Before::Read(&part()), &ids(&["b0000001"]), &part()),
        ids(&["b0000001", "c0000001"])
    );
    // And a change since, with what it reaches.
    let after = edited(
        r#""position": [0, 0, 0], "length": 40"#,
        r#""position": [0, 0, 0], "length": 41"#,
    );
    assert_eq!(
        net_reach(Before::Read(&part()), &ids(&["b0000001"]), &after),
        ids(&["b0000001", "c0000001", "k7f2qx3m"])
    );
}

#[test]
fn from_text_that_didnt_read_only_features_whose_ids_it_hasnt_are_reached() {
    // A merge's markers in the text before: every feature of the part but the cut is named
    // in it, quoted; the cut, whose ID it hasn't, is reached (and nothing uses it).
    let before = PART
        .replacen("[", "[\n<<<<<<< ours", 1)
        .replace(
            r#",
  {"id": "c0000001", "type": "cut", "v": 1, "name": "Notch", "base": "q0000001", "tool": "b0000001"}"#,
            "",
        );
    assert!(read(before.as_bytes()).is_err());
    assert!(!before.contains("c0000001"));
    assert_eq!(
        net_reach(
            Before::Unread(&QuotedIds::of(before.as_bytes())),
            &ids(&[]),
            &part()
        ),
        ids(&["c0000001"])
    );
    // An ID that only appears unquoted, inside another word, isn't one the text holds.
    let unquoted = before.replace(r#""id": "k7f2qx3m""#, r#""id": "xk7f2qx3m""#);
    assert_eq!(
        net_reach(
            Before::Unread(&QuotedIds::of(unquoted.as_bytes())),
            &ids(&[]),
            &part()
        ),
        ids(&["c0000001", "k7f2qx3m"])
    );
}

/// [`PART`] as a merge left it, markers and all: every feature but the cut, as the text
/// before [`from_text_that_didnt_read_only_features_whose_ids_it_hasnt_are_reached`] has it.
fn merged() -> String {
    let before = PART.replacen("[", "[\n<<<<<<< ours", 1).replace(
        r#",
  {"id": "c0000001", "type": "cut", "v": 1, "name": "Notch", "base": "q0000001", "tool": "b0000001"}"#,
        "",
    );
    assert!(read(before.as_bytes()).is_err());
    before
}

/// What the part reaches from text that didn't read, `before`.
fn reached_from(before: &str) -> BTreeSet<FeatureId> {
    net_reach(
        Before::Unread(&QuotedIds::of(before.as_bytes())),
        &ids(&[]),
        &part(),
    )
}

#[test]
fn from_text_that_didnt_read_an_id_written_with_escapes_is_held() {
    // The block's ID with its last letter escaped, as JSON may write any character: the
    // text holds k7f2qx3m all the same, so only the cut is reached.
    let escaped = merged().replace(r#""id": "k7f2qx3m""#, r#""id": "k7f2qx3\u006d""#);
    assert!(!escaped.contains("k7f2qx3m"));
    assert_eq!(reached_from(&escaped), ids(&["c0000001"]));
    // Every character escaped.
    let all = merged().replace(
        r#""id": "k7f2qx3m""#,
        r#""id": "\u006b\u0037\u0066\u0032\u0071\u0078\u0033\u006d""#,
    );
    assert_eq!(reached_from(&all), ids(&["c0000001"]));
    // An escape that decodes to another ID isn't this one's.
    let other = merged().replace(r#""id": "k7f2qx3m""#, r#""id": "k7f2qx3\u006e""#);
    assert_eq!(reached_from(&other), ids(&["c0000001", "k7f2qx3m"]));
}

#[test]
fn from_text_that_didnt_read_a_stray_quote_hides_no_id_after_it() {
    // A quote the merge left on the marker's line: pairing quotes from the start would take
    // every string after it for the space between two, and every ID for new.
    let stray = merged().replacen("<<<<<<< ours", "<<<<<<< \"ours", 1);
    assert_eq!(reached_from(&stray), ids(&["c0000001"]));
}

#[test]
fn from_text_that_didnt_read_a_feature_that_uses_a_new_one_is_reached() {
    // Before: no lug, and the cut's tool the block. After (the part): the lug is new, and
    // the cut, whose ID the text holds, uses it now: both are reached. The cut's name, a
    // quote then the lug's ID, holds the ID after an escaped quote, which ends no string:
    // the text doesn't hold the lug's ID quoted.
    let before = PART.replacen("[", "[\n<<<<<<< ours", 1)
        .replace(
            r#",
  {"id": "b0000001", "type": "box", "v": 1, "name": "Lug", "position": [100, 0, 0], "length": 10, "width": 10, "height": 10}"#,
            "",
        )
        .replace(r#""tool": "b0000001""#, r#""tool": "k7f2qx3m""#)
        .replace(r#""name": "Notch""#, r#""name": "\"b0000001""#);
    assert_eq!(before.matches("b0000001").count(), 1);
    assert_eq!(reached_from(&before), ids(&["b0000001", "c0000001"]));
}

#[test]
fn from_a_large_text_that_didnt_read_what_edits_reach_is_found_promptly() {
    // Five thousand boxes, the text before them with a merge's markers: each ID is held, so
    // nothing is reached. Searching the text for each ID would read its 0.8 MB five
    // thousand times over (4e9 bytes); one pass reads it once.
    let boxes: Vec<String> = (0..5000)
        .map(|i| {
            format!(
                r#"{{"id": "b{i:07}", "type": "box", "v": 1, "name": "Box", "position": [{i}, 0, 0], "length": 1, "width": 1, "height": 1}}"#
            )
        })
        .collect();
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [
{}
]}}"#,
        boxes.join(",\n")
    );
    let after = read(text.as_bytes()).unwrap();
    let before = text.replacen("[\n", "[\n<<<<<<< ours\n", 1);
    assert!(read(before.as_bytes()).is_err());
    // Milliseconds of CPU: two seconds is room to spare.
    let reached = within(Duration::from_secs(2), move || {
        net_reach(
            Before::Unread(&QuotedIds::of(before.as_bytes())),
            &BTreeSet::new(),
            &after,
        )
    });
    assert_eq!(reached, ids(&[]));
}
