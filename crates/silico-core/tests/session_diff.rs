//! `diff` (Fable's decision of 2026-11-04, the three core gaps, section 1): the changes
//! between two revisions a session holds, `document::diff` beneath it. Each case a pair of
//! documents built by hand, its changes written out by hand (paths, values as the file
//! writes them, places counted by hand); the session's held revisions (its current one, the
//! one its last reload replaced, a transaction's base) read through reloads made by writing
//! the file outside the session; and, as an independent check on the paths, each field
//! change applied through `set_field` to `from` giving `to` back. The design's examples
//! (docs/design/m4.1-command-api.md, `diff`) are its own blocks and the session's answers.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::wire::reads::DiffRequest;
use silico_core::command::wire::{Layout, Named, ToWire, response_text, write};
use silico_core::command::{DiffAnswer, Response, Revision, SessionId, Sessions};
use silico_core::disk::Disk;
use silico_core::document::json::{self, Node};
use silico_core::document::{self, Change, Document};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::sketch::SketchSolver;
use tempfile::TempDir;

use crate::sessions::{answer, id};

/// Revision `n`, named.
fn at(n: u64) -> Named {
    Named::Revision(Revision::from_number(n).unwrap())
}

/// The number of `named`, a revision.
#[track_caller]
fn number(named: Named) -> u64 {
    match named {
        Named::Revision(revision) => revision.number(),
        Named::Disk => panic!("disk, not a revision"),
    }
}

/// A document's header and its features, given as JSON objects, on one line.
fn doc_text(features: &[&str]) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    )
}

fn doc(features: &[&str]) -> Document {
    document::read(doc_text(features).as_bytes()).unwrap()
}

/// Each change, as the API writes it on one line.
fn lines(changes: &[Change]) -> Vec<String> {
    changes
        .iter()
        .map(|c| write(&c.to_wire(), Layout::OneLine).unwrap())
        .collect()
}

/// The changes from `from` to `to`, each on one line.
fn diffed(from: &[&str], to: &[&str]) -> Vec<String> {
    lines(&document::diff(&doc(from), &doc(to)).unwrap())
}

const BOX: &str = r#"{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}"#;

fn a_box(id: &str, name: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "box", "v": 1, "name": "{name}", "position": [0, 0, 0], "length": 4, "width": 2, "height": 1}}"#
    )
}

fn cylinder(radius: &str) -> String {
    format!(
        r#"{{"id": "a91cw0de", "type": "cylinder", "v": 1, "name": "Pin", "position": [20, 10, -1], "radius": {radius}, "height": 12}}"#
    )
}

/// A sketch of two points, a line, and a distance of `value`; with a third point, if
/// `third` is given, put first (the reader takes a list in any order; canonical form sorts
/// it by ID).
fn sketch(value: &str, third: Option<&str>) -> String {
    let points = [
        third.map(str::to_owned),
        Some(r#"{"id": "pa000001", "x": 0, "y": 0, "construction": false}"#.to_owned()),
        Some(r#"{"id": "pa000002", "x": 10, "y": 0, "construction": false}"#.to_owned()),
    ];
    let points: Vec<String> = points.into_iter().flatten().collect();
    format!(
        r#"{{"id": "s0000001", "type": "sketch", "v": 1, "name": "Sketch", "plane": "xy", "points": [{}], "lines": [{{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}}], "circles": [], "arcs": [], "constraints": [{{"id": "ka000001", "type": "distance", "a": "pa000001", "b": "pa000002", "value": {value}, "driving": true}}]}}"#,
        points.join(", ")
    )
}

const THIRD: &str = r#"{"id": "pa000003", "x": 1, "y": 2, "construction": false}"#;

fn fillet(second: &str) -> String {
    format!(
        r#"{{"id": "f0000001", "type": "fillet", "v": 1, "name": "Round", "base": "b0000001", "edges": ["b0000001.box.Edge1", "{second}"], "radius": 1}}"#
    )
}

fn fingerprinted(volume: &str) -> String {
    format!(
        r#"{{"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10, "fingerprint": {{"volume": {volume}, "area": 2800, "center": [20, 10, 5], "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}}}"#
    )
}

// --- Field changes, each pair by hand. ---

#[test]
fn a_radius_changed_is_its_field_with_the_texts_before_and_after() {
    assert_eq!(
        diffed(&[&cylinder("5")], &[&cylinder("3")]),
        [r#"{"feature": "a91cw0de", "name": "Pin", "field": "radius", "before": 5, "after": 3}"#]
    );
    // A number is its text as the file writes it, never through a float.
    assert_eq!(
        diffed(&[&cylinder("2.5")], &[&cylinder("0.000001")]),
        [
            r#"{"feature": "a91cw0de", "name": "Pin", "field": "radius", "before": 2.5, "after": 0.000001}"#
        ]
    );
}

#[test]
fn a_position_changed_is_its_item_by_place() {
    let moved = BOX.replace("[0, 0, 0]", "[0, 0, 5]");
    assert_eq!(
        diffed(&[BOX], &[&moved]),
        [
            r#"{"feature": "k7f2qx3m", "name": "Block", "field": "position.2", "before": 0, "after": 5}"#
        ]
    );
}

#[test]
fn a_constraints_value_is_named_by_the_constraints_id() {
    assert_eq!(
        diffed(&[&sketch("10", None)], &[&sketch("12.5", None)]),
        [
            r#"{"feature": "s0000001", "name": "Sketch", "field": "constraints.ka000001.value", "before": 10, "after": 12.5}"#
        ]
    );
}

#[test]
fn a_point_added_is_its_id_path_with_the_whole_point_after() {
    assert_eq!(
        diffed(&[&sketch("10", None)], &[&sketch("10", Some(THIRD))]),
        [
            r#"{"feature": "s0000001", "name": "Sketch", "field": "points.pa000003", "before": null, "after": {"id": "pa000003", "x": 1, "y": 2, "construction": false}}"#
        ]
    );
    // And removed, the other way.
    assert_eq!(
        diffed(&[&sketch("10", Some(THIRD))], &[&sketch("10", None)]),
        [
            r#"{"feature": "s0000001", "name": "Sketch", "field": "points.pa000003", "before": {"id": "pa000003", "x": 1, "y": 2, "construction": false}, "after": null}"#
        ]
    );
}

#[test]
fn a_fillets_second_edge_is_edges_1() {
    let base = a_box("b0000001", "Base");
    assert_eq!(
        diffed(
            &[&base, &fillet("b0000001.box.Edge2")],
            &[&base, &fillet("b0000001.box.Edge7")]
        ),
        [
            r#"{"feature": "f0000001", "name": "Round", "field": "edges.1", "before": "b0000001.box.Edge2", "after": "b0000001.box.Edge7"}"#
        ]
    );
}

#[test]
fn a_fingerprints_volume_is_a_field_like_any_other() {
    assert_eq!(
        diffed(&[&fingerprinted("8000")], &[&fingerprinted("8000.5")]),
        [
            r#"{"feature": "k7f2qx3m", "name": "Block", "field": "fingerprint.volume", "before": 8000, "after": 8000.5}"#
        ]
    );
    // A fingerprint written where there was none is one field, whole.
    assert_eq!(
        diffed(&[BOX], &[&fingerprinted("8000")]),
        [
            r#"{"feature": "k7f2qx3m", "name": "Block", "field": "fingerprint", "before": null, "after": {"volume": 8000, "area": 2800, "center": [20, 10, 5], "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}}"#
        ]
    );
}

#[test]
fn a_name_changed_is_name_and_the_change_carries_the_new_name() {
    let renamed = BOX.replace("\"Block\"", "\"Base\"");
    assert_eq!(
        diffed(&[BOX], &[&renamed]),
        [
            r#"{"feature": "k7f2qx3m", "name": "Base", "field": "name", "before": "Block", "after": "Base"}"#
        ]
    );
}

#[test]
fn fields_come_in_canonical_key_order() {
    // The changes come as the document writes its keys (id, type, v, name, the
    // operation's fields: a sketch's plane, then its lists; then the note, then the
    // fingerprint), whatever order the text gave them in.
    let from = sketch("10", None);
    let to = sketch("11", Some(THIRD))
        .replace(
            "\"name\": \"Sketch\"",
            "\"name\": \"Profile\", \"note\": \"moved\"",
        )
        .replace("\"plane\": \"xy\"", "\"plane\": \"xz\"");
    assert_eq!(
        diffed(&[&from], &[&to]),
        [
            r#"{"feature": "s0000001", "name": "Profile", "field": "name", "before": "Sketch", "after": "Profile"}"#,
            r#"{"feature": "s0000001", "name": "Profile", "field": "plane", "before": "xy", "after": "xz"}"#,
            r#"{"feature": "s0000001", "name": "Profile", "field": "points.pa000003", "before": null, "after": {"id": "pa000003", "x": 1, "y": 2, "construction": false}}"#,
            r#"{"feature": "s0000001", "name": "Profile", "field": "constraints.ka000001.value", "before": 10, "after": 11}"#,
            r#"{"feature": "s0000001", "name": "Profile", "field": "note", "before": null, "after": "moved"}"#,
        ]
    );
}

/// A sketch of the points `ids` alone, each at the origin.
fn points(ids: &[&str]) -> String {
    let points: Vec<String> = ids
        .iter()
        .map(|id| format!(r#"{{"id": "{id}", "x": 0, "y": 0, "construction": false}}"#))
        .collect();
    format!(
        r#"{{"id": "s0000001", "type": "sketch", "v": 1, "name": "Sketch", "plane": "xy", "points": [{}], "lines": [], "circles": [], "arcs": [], "constraints": []}}"#,
        points.join(", ")
    )
}

#[test]
fn members_each_side_alone_has_come_in_id_order_as_the_writer_sorts_a_list() {
    // From holds pa000001 and pa000003, to pa000001 and pa000002: the writer writes a list
    // sorted by ID, so pa000002 (added) comes before pa000003 (removed), either way round.
    let (from, to) = (
        points(&["pa000001", "pa000003"]),
        points(&["pa000001", "pa000002"]),
    );
    let point = |id: &str| format!(r#"{{"id": "{id}", "x": 0, "y": 0, "construction": false}}"#);
    let change = |id: &str, before: &str, after: &str| {
        format!(
            r#"{{"feature": "s0000001", "name": "Sketch", "field": "points.{id}", "before": {before}, "after": {after}}}"#
        )
    };
    assert_eq!(
        diffed(&[&from], &[&to]),
        [
            change("pa000002", "null", &point("pa000002")),
            change("pa000003", &point("pa000003"), "null"),
        ]
    );
    assert_eq!(
        diffed(&[&to], &[&from]),
        [
            change("pa000002", &point("pa000002"), "null"),
            change("pa000003", "null", &point("pa000003")),
        ]
    );
}

/// A pad of the sketch s0000001, with `members` (a `suppressed`, a `base`) after its name.
fn pad(members: &str) -> String {
    format!(
        r#"{{"id": "p0000001", "type": "pad", "v": 1, "name": "Pad", {members}"sketch": "s0000001", "extent": {{"type": "one_side", "length": 5, "reversed": false}}}}"#
    )
}

#[test]
fn top_level_members_each_side_alone_has_come_in_the_order_the_writer_gives_them() {
    // From ends with a fingerprint, to with a note: the writer puts a note before a
    // fingerprint (`silico fmt` writes id, type, v, name, suppressed, the operation's
    // fields, note, fingerprint), so the note comes first, either way round.
    let noted = BOX.replace("\"height\": 10}", "\"height\": 10, \"note\": \"n\"}");
    let fingerprint = r#"{"volume": 8000, "area": 2800, "center": [20, 10, 5], "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}"#;
    let block = |field: &str, before: &str, after: &str| {
        format!(
            r#"{{"feature": "k7f2qx3m", "name": "Block", "field": "{field}", "before": {before}, "after": {after}}}"#
        )
    };
    assert_eq!(
        diffed(&[&fingerprinted("8000")], &[&noted]),
        [
            block("note", "null", r#""n""#),
            block("fingerprint", fingerprint, "null"),
        ]
    );
    assert_eq!(
        diffed(&[&noted], &[&fingerprinted("8000")]),
        [
            block("note", r#""n""#, "null"),
            block("fingerprint", "null", fingerprint),
        ]
    );
    // A pad's `suppressed` and its `base`: the writer puts `suppressed` right after the
    // name, before the operation's fields, of which `base` is the first.
    let (suppressed, based) = (
        pad(r#""suppressed": true, "#),
        pad(r#""base": "b0000001", "#),
    );
    let of_pad = |field: &str, before: &str, after: &str| {
        format!(
            r#"{{"feature": "p0000001", "name": "Pad", "field": "{field}", "before": {before}, "after": {after}}}"#
        )
    };
    assert_eq!(
        diffed(&[&based], &[&suppressed]),
        [
            of_pad("suppressed", "null", "true"),
            of_pad("base", r#""b0000001""#, "null"),
        ]
    );
    assert_eq!(
        diffed(&[&suppressed], &[&based]),
        [
            of_pad("suppressed", "true", "null"),
            of_pad("base", "null", r#""b0000001""#),
        ]
    );
    // `suppressed` on the from side alone, with the name changed too: the writer puts the
    // name before `suppressed`, so the name's change comes first, then `suppressed`, then
    // `base` (a member only the to side has).
    let renamed = based.replace(r#""name": "Pad""#, r#""name": "Boss""#);
    let named = |field: &str, before: &str, after: &str| {
        format!(
            r#"{{"feature": "p0000001", "name": "Boss", "field": "{field}", "before": {before}, "after": {after}}}"#
        )
    };
    assert_eq!(
        diffed(&[&suppressed], &[&renamed]),
        [
            named("name", r#""Pad""#, r#""Boss""#),
            named("suppressed", "true", "null"),
            named("base", "null", r#""b0000001""#),
        ]
    );
    // That is the writer's order: a feature holding both members of each pair, written.
    let both = [
        (
            BOX.replace(
                "\"height\": 10}",
                &format!(r#""height": 10, "note": "n", "fingerprint": {fingerprint}}}"#),
            ),
            ["\"note\"", "\"fingerprint\""],
        ),
        (
            pad(r#""base": "b0000001", "suppressed": true, "#),
            ["\"suppressed\"", "\"base\""],
        ),
    ];
    for (feature, [first, second]) in both {
        let text = document::write(&doc(&[&feature])).unwrap();
        let at = |key: &str| text.find(key).unwrap();
        assert!(at(first) < at(second), "{text}");
    }
}

// --- Places: added, removed, moved, by rank among the features both hold. ---

#[test]
fn a_feature_added_is_its_place_after_and_moves_none_of_the_others() {
    let (a, b, c) = (
        a_box("a0000001", "A"),
        a_box("b0000001", "B"),
        a_box("c0000001", "C"),
    );
    // C put between A and B: B's place goes 1 to 2, its rank among A and B stays 1.
    assert_eq!(
        diffed(&[&a, &b], &[&a, &c, &b]),
        [r#"{"feature": "c0000001", "name": "C", "field": null, "before": null, "after": 1}"#]
    );
}

#[test]
fn a_feature_removed_is_its_place_before_listed_after_tos_features_and_shifts_nothing() {
    let (a, b, c, d, e) = (
        a_box("a0000001", "A"),
        a_box("b0000001", "B"),
        a_box("c0000001", "C"),
        a_box("d0000001", "D"),
        a_box("e0000001", "E"),
    );
    // B removed from place 1: C and D each move up a place, but keep their ranks.
    assert_eq!(
        diffed(&[&a, &b, &c], &[&a, &c]),
        [r#"{"feature": "b0000001", "name": "B", "field": null, "before": 1, "after": null}"#]
    );
    // One removed and one added: the added in `to`'s order, then the removed in `from`'s.
    assert_eq!(
        diffed(&[&a, &b, &c, &d], &[&a, &c, &d, &e]),
        [
            r#"{"feature": "e0000001", "name": "E", "field": null, "before": null, "after": 3}"#,
            r#"{"feature": "b0000001", "name": "B", "field": null, "before": 1, "after": null}"#,
        ]
    );
    // Two removed, in `from`'s order.
    assert_eq!(
        diffed(&[&a, &b, &c, &d], &[&c]),
        [
            r#"{"feature": "a0000001", "name": "A", "field": null, "before": 0, "after": null}"#,
            r#"{"feature": "b0000001", "name": "B", "field": null, "before": 1, "after": null}"#,
            r#"{"feature": "d0000001", "name": "D", "field": null, "before": 3, "after": null}"#,
        ]
    );
}

#[test]
fn a_feature_moved_changes_the_ranks_of_each_it_passed() {
    let (a, b, c, d) = (
        a_box("a0000001", "A"),
        a_box("b0000001", "B"),
        a_box("c0000001", "C"),
        a_box("d0000001", "D"),
    );
    // B moved from 1 to the end: ranks A0 B1 C2 D3 become A0 C1 D2 B3. C, D, and B moved,
    // in `to`'s order.
    assert_eq!(
        diffed(&[&a, &b, &c, &d], &[&a, &c, &d, &b]),
        [
            r#"{"feature": "c0000001", "name": "C", "field": null, "before": 2, "after": 1}"#,
            r#"{"feature": "d0000001", "name": "D", "field": null, "before": 3, "after": 2}"#,
            r#"{"feature": "b0000001", "name": "B", "field": null, "before": 1, "after": 3}"#,
        ]
    );
    // C moved to the front: every feature it passed moves too.
    assert_eq!(
        diffed(&[&a, &b, &c], &[&c, &a, &b]),
        [
            r#"{"feature": "c0000001", "name": "C", "field": null, "before": 2, "after": 0}"#,
            r#"{"feature": "a0000001", "name": "A", "field": null, "before": 0, "after": 1}"#,
            r#"{"feature": "b0000001", "name": "B", "field": null, "before": 1, "after": 2}"#,
        ]
    );
    // A move and a field change of one feature: its place first, then the field.
    let b_longer = b.replace("\"length\": 4", "\"length\": 6");
    assert_eq!(
        diffed(&[&a, &b], &[&b_longer, &a]),
        [
            r#"{"feature": "b0000001", "name": "B", "field": null, "before": 1, "after": 0}"#,
            r#"{"feature": "b0000001", "name": "B", "field": "length", "before": 4, "after": 6}"#,
            r#"{"feature": "a0000001", "name": "A", "field": null, "before": 0, "after": 1}"#,
        ]
    );
}

#[test]
fn a_hand_written_document_diffs_empty_against_its_canonical_text() {
    // Keys out of the canonical order, a sketch's lists out of ID order, numbers spelled
    // as canonical form doesn't spell them: the same document as its canonical text.
    let hand = r#"{"naming": 5, "features": [
        {"name": "Sketch", "v": 1, "type": "sketch", "id": "s0000001", "plane": "xy",
         "constraints": [{"value": 1.50, "id": "ka000002", "type": "distance", "a": "pa000002", "b": "pa000003", "driving": true},
                         {"id": "ka000001", "type": "distance", "a": "pa000001", "b": "pa000002", "value": 10, "driving": true}],
         "points": [{"y": 0, "x": 1e1, "id": "pa000002", "construction": false},
                    {"id": "pa000003", "x": 1, "y": 2, "construction": false},
                    {"id": "pa000001", "x": 0, "y": 0, "construction": false}],
         "lines": [{"id": "na000002", "from": "pa000002", "to": "pa000003", "construction": false},
                   {"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}],
         "circles": [], "arcs": []},
        {"height": 10, "width": 20, "length": 40.0, "position": [0, 0, 0], "name": "Block", "v": 1, "type": "box", "id": "k7f2qx3m"}
    ], "units": {"resolution": "1nm", "length": "mm"}, "silico": 1}"#;
    let read = document::read(hand.as_bytes()).unwrap();
    let canonical = document::write(&read).unwrap();
    assert_ne!(canonical, hand);
    let again = document::read(canonical.as_bytes()).unwrap();
    assert_eq!(document::diff(&read, &again).unwrap(), []);
    assert_eq!(document::diff(&again, &read).unwrap(), []);
}

#[test]
fn the_same_pair_gives_the_same_changes_byte_for_byte() {
    let (a, b, c) = (
        a_box("a0000001", "A"),
        a_box("b0000001", "B"),
        a_box("c0000001", "C"),
    );
    let from = doc(&[&a, &b, &sketch("10", None), BOX]);
    let to = doc(&[&sketch("12", Some(THIRD)), &c, &a, &fingerprinted("1")]);
    let first = lines(&document::diff(&from, &to).unwrap());
    assert!(!first.is_empty());
    for _ in 0..3 {
        assert_eq!(lines(&document::diff(&from, &to).unwrap()), first);
    }
}

// --- The paths, applied through `set_field`: an independent check. ---

/// Sessions on the fake kernel and fake sketches, in a directory of their own.
struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn sessions() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = FakeSketches::new();
    let sessions = Sessions::with_stand_ins(
        kernel,
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(sketches.clone()) as Box<dyn SketchSolver>),
    );
    Fixture { dir, sessions }
}

impl Fixture {
    fn path(&self) -> PathBuf {
        self.dir.path().join("bracket.silico")
    }

    /// `text` written to the session's file, outside the session.
    fn write(&self, text: &[u8]) {
        std::fs::write(self.path(), text).unwrap();
    }

    /// The session on `text`, opened.
    fn open(&self, text: &[u8]) -> SessionId {
        self.write(text);
        answer(self.sessions.open(self.path(), false))
            .session
            .unwrap()
            .id
    }

    fn revision(&self, s: &SessionId) -> Revision {
        self.sessions.status(s).session.unwrap().revision
    }

    /// Once the session has reloaded its file itself, `status` stepped until its revision
    /// is past `from`: its new revision.
    #[track_caller]
    fn reloads(&self, s: &SessionId, from: Revision) -> Revision {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let revision = self.revision(s);
            if revision > from {
                return revision;
            }
            assert!(Instant::now() < deadline, "never reloaded past {from:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// `diff` of revisions by number, each `None` for the document now.
    fn diff(&self, s: &SessionId, from: Option<u64>, to: Option<u64>) -> Response<DiffAnswer> {
        self.diff_of(s, from.map(at), to.map(at))
    }

    /// `diff` of the sides named, a revision or `"disk"`.
    fn diff_of(
        &self,
        s: &SessionId,
        from: Option<Named>,
        to: Option<Named>,
    ) -> Response<DiffAnswer> {
        answer(self.sessions.diff(s, from, to))
    }
}

/// A value as the request's reader reads it, from its text as the API writes it.
fn node(value: &json::Plain) -> Node {
    let text = write(
        &silico_core::command::wire::Json::plain(value),
        Layout::OneLine,
    )
    .unwrap();
    json::read(text.as_bytes()).unwrap()
}

/// `from`'s text in a session, inside a transaction, with each of `changes`' fields set to
/// its `after` (or, `back`, its `before`) through `set_field`: the session's text then, in
/// canonical form.
fn applied(from: &str, changes: &[Change], back: bool) -> String {
    let f = sessions();
    let s = f.open(from.as_bytes());
    answer(f.sessions.begin(&s)).result.unwrap();
    for change in changes {
        let field = change.field.clone().expect("a field change");
        let value = if back { &change.before } else { &change.after };
        let set = answer(f.sessions.set_field(
            &s,
            f.revision(&s),
            change.feature.clone(),
            field.clone(),
            node(value),
        ));
        assert!(set.result.is_ok(), "{field}: {:?}", set.result);
    }
    let text = answer(f.sessions.text(&s, None)).result.unwrap().text;
    document::write(&document::read(text.as_bytes()).unwrap()).unwrap()
}

#[test]
fn each_field_change_set_through_set_field_gives_to_back_and_before_gives_from() {
    let base = a_box("b0000001", "Base");
    let renamed = BOX.replace("\"Block\"", "\"Base\"");
    let moved = BOX.replace("[0, 0, 0]", "[0, 0, 5]");
    let cases: Vec<(Vec<String>, Vec<String>)> = vec![
        (vec![cylinder("5")], vec![cylinder("3")]),
        (vec![BOX.to_owned()], vec![moved]),
        (vec![BOX.to_owned()], vec![renamed.clone()]),
        (vec![sketch("10", None)], vec![sketch("12.5", None)]),
        (
            vec![base.clone(), fillet("b0000001.box.Edge2")],
            vec![base.clone(), fillet("b0000001.box.Edge7")],
        ),
        // A sketch angle's `reversed`, the one member below the top level that set_field
        // adds and takes away: added, and (back) removed.
        (vec![angled("")], vec![angled(r#""reversed": true, "#)]),
        // A constraint's type changed to one that takes the same fields.
        (vec![lined("horizontal")], vec![lined("vertical")]),
        // A pad's `suppressed` and its `base`, members a feature may leave out.
        (
            vec![base.clone(), sketch("10", None), pad("")],
            vec![
                base.clone(),
                sketch("10", None),
                pad(r#""suppressed": true, "base": "b0000001", "#),
            ],
        ),
        // Several fields of several features at once, a note among them.
        (
            vec![BOX.to_owned(), cylinder("2.5"), sketch("10", None)],
            vec![
                renamed.replace("\"length\": 40", "\"length\": 41"),
                cylinder("2.75").replace(
                    "\"name\": \"Pin\"",
                    "\"name\": \"Pin\", \"note\": \"press fit\"",
                ),
                sketch("9", None).replace("\"plane\": \"xy\"", "\"plane\": \"yz\""),
            ],
        ),
    ];
    for (from, to) in cases {
        let from: Vec<&str> = from.iter().map(String::as_str).collect();
        let to: Vec<&str> = to.iter().map(String::as_str).collect();
        let changes = document::diff(&doc(&from), &doc(&to)).unwrap();
        assert!(!changes.is_empty());
        assert!(changes.iter().all(|c| c.field.is_some()), "{changes:?}");
        let want = document::write(&doc(&to)).unwrap();
        assert_eq!(applied(&doc_text(&from), &changes, false), want, "{to:?}");
        let back = document::write(&doc(&from)).unwrap();
        assert_eq!(applied(&doc_text(&to), &changes, true), back, "{from:?}");
    }
}

/// A sketch of two lines from one point and the angle `ka000001` between them, with
/// `reversed` (`"reversed": true, ` or nothing) after its `b`.
fn angled(reversed: &str) -> String {
    format!(
        r#"{{"id": "s0000001", "type": "sketch", "v": 1, "name": "Sketch", "plane": "xy", "points": [{{"id": "pa000001", "x": 0, "y": 0, "construction": false}}, {{"id": "pa000002", "x": 10, "y": 0, "construction": false}}, {{"id": "pa000003", "x": 0, "y": 10, "construction": false}}], "lines": [{{"id": "na000001", "from": "pa000001", "to": "pa000002", "construction": false}}, {{"id": "na000002", "from": "pa000001", "to": "pa000003", "construction": false}}], "circles": [], "arcs": [], "constraints": [{{"id": "ka000001", "type": "angle", "a": "na000001", "b": "na000002", {reversed}"value": 90, "driving": false}}]}}"#
    )
}

/// [`sketch`], its constraint `ka000001` one of `kind` (`horizontal`, `vertical`) on its
/// line.
fn lined(kind: &str) -> String {
    sketch("10", None).replace(
        r#"{"id": "ka000001", "type": "distance", "a": "pa000001", "b": "pa000002", "value": 10, "driving": true}"#,
        &format!(r#"{{"id": "ka000001", "type": "{kind}", "line": "na000001"}}"#),
    )
}

/// Each of `changes` set alone through `set_field`, to its `after`, in a session on
/// `from`'s text, inside a transaction: each refused, `invalid_input` naming the request's
/// field or a path in the member it changes, the session's text and revision as they were. Then the first change's
/// feature renamed, so the document is one set_field edits: what refused each was the
/// change.
fn refused_each(from: &str, changes: &[Change]) {
    let f = sessions();
    let s = f.open(from.as_bytes());
    answer(f.sessions.begin(&s)).result.unwrap();
    let text = || answer(f.sessions.text(&s, None)).result.unwrap().text;
    let (was, at) = (text(), f.revision(&s));
    for change in changes {
        let field = change.field.clone().expect("a field change");
        let set = answer(f.sessions.set_field(
            &s,
            at,
            change.feature.clone(),
            field.clone(),
            node(&change.after),
        ));
        let Err(refused) = set.result else {
            panic!("{field} was set to {:?}", change.after);
        };
        let e = &refused.errors()[0];
        assert_eq!(e.code(), ErrorCode::InvalidInput, "{field}: {e:?}");
        // Refused by set_field's path (`field`), or by the reader of what it would write,
        // naming a path in the member the change is to.
        let top = |path: &str| path.split('.').next().map(str::to_owned);
        assert!(
            e.parameter() == Some("field") || e.parameter().and_then(top) == top(&field),
            "{field}: {e:?}"
        );
        assert_eq!(text(), was, "{field}");
        assert_eq!(f.revision(&s), at, "{field}");
    }
    let renamed = answer(f.sessions.set_field(
        &s,
        at,
        changes[0].feature.clone(),
        "name".to_owned(),
        json::read(br#""Renamed""#).unwrap(),
    ));
    assert!(renamed.result.is_ok(), "{:?}", renamed.result);
}

#[test]
fn each_change_set_field_doesnt_accept_is_refused_by_it_and_nothing_is_written() {
    // The kinds the design names: fingerprint fields; `type` (with the fields a feature's
    // new type brings and takes away); a member one side alone has below the top level (a
    // sketch's member, an item of a list by place); an extent's or a constraint's type
    // changed to one that takes other fields. Each pair diffed, its fields written out by
    // hand, then each change set through `set_field`.
    let base = a_box("b0000001", "Base");
    let one_edge = fillet("b0000001.box.Edge2").replace(r#", "b0000001.box.Edge2""#, "");
    let cylinder_block = r#"{"id": "k7f2qx3m", "type": "cylinder", "v": 1, "name": "Block", "position": [0, 0, 0], "radius": 5, "height": 10}"#;
    let symmetric = pad("").replace(
        r#"{"type": "one_side", "length": 5, "reversed": false}"#,
        r#"{"type": "symmetric", "length": 5}"#,
    );
    let cases: Vec<(Vec<String>, Vec<String>, Vec<&str>)> = vec![
        (
            vec![fingerprinted("8000")],
            vec![fingerprinted("8000.5")],
            vec!["fingerprint.volume"],
        ),
        (
            vec![BOX.to_owned()],
            vec![fingerprinted("8000")],
            vec!["fingerprint"],
        ),
        (
            vec![BOX.to_owned()],
            vec![cylinder_block.to_owned()],
            vec!["type", "length", "width", "radius"],
        ),
        (
            vec![sketch("10", None)],
            vec![sketch("10", Some(THIRD))],
            vec!["points.pa000003"],
        ),
        (
            vec![sketch("10", Some(THIRD))],
            vec![sketch("10", None)],
            vec!["points.pa000003"],
        ),
        (
            vec![base.clone(), one_edge.clone()],
            vec![base.clone(), fillet("b0000001.box.Edge2")],
            vec!["edges.1"],
        ),
        (
            vec![base.clone(), fillet("b0000001.box.Edge2")],
            vec![base.clone(), one_edge.clone()],
            vec!["edges.1"],
        ),
        (
            vec![sketch("10", None), pad("")],
            vec![sketch("10", None), symmetric],
            vec!["extent.type", "extent.reversed"],
        ),
        (
            vec![sketch("10", None)],
            vec![lined("horizontal")],
            vec![
                "constraints.ka000001.type",
                "constraints.ka000001.a",
                "constraints.ka000001.b",
                "constraints.ka000001.value",
                "constraints.ka000001.driving",
                "constraints.ka000001.line",
            ],
        ),
    ];
    for (from, to, fields) in cases {
        let from: Vec<&str> = from.iter().map(String::as_str).collect();
        let to: Vec<&str> = to.iter().map(String::as_str).collect();
        let changes = document::diff(&doc(&from), &doc(&to)).unwrap();
        let named: Vec<&str> = changes.iter().filter_map(|c| c.field.as_deref()).collect();
        assert_eq!(named, fields);
        refused_each(&doc_text(&from), &changes);
    }
    // `id` and `v`, which no diff names (features are paired by ID, and every feature is
    // version 1), refused all the same.
    let block = id("k7f2qx3m");
    let made = |field: &str, value: &[u8]| Change {
        feature: block.clone(),
        name: Arc::from("Block"),
        field: Some(field.to_owned()),
        before: json::Plain::Null,
        after: json::Plain::of(&json::read(value).unwrap()),
    };
    refused_each(
        &doc_text(&[BOX]),
        &[made("id", br#""k7f2qx3n""#), made("v", b"2")],
    );
}

// --- The session: the revisions it holds, and the design's examples. ---

fn contract(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract")
        .join(name)
}

fn example(name: &str) -> String {
    let path = contract(&format!("commands/examples/{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The design's JSON code blocks between the headings `from` and `to`, in order.
fn design(from: &str, to: &str) -> Vec<String> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/m4.1-command-api.md");
    let doc = std::fs::read_to_string(&path).unwrap();
    let section = &doc[doc.find(from).unwrap()..doc.find(to).unwrap()];
    section
        .split("```json\n")
        .skip(1)
        .map(|block| block.split("\n```").next().unwrap().to_owned())
        .collect()
}

/// `value` as the API writes it, and a newline, as each example file holds it.
fn written<T: ToWire>(value: &T) -> String {
    write(&value.to_wire(), Layout::Canonical).unwrap() + "\n"
}

/// The design's bracket's first three features (a block, a pin, the cut between them), in
/// canonical form, as `outline`'s example has them.
fn bracket() -> String {
    let text = std::fs::read_to_string(contract("naming-scheme-5.silico")).unwrap();
    let mut read = document::read(text.as_bytes()).unwrap();
    read.features.truncate(3);
    document::write(&read).unwrap()
}

/// The bracket changed as the design's example has it, outside the session: the pin's
/// radius 2.5 to 3, the cut renamed, a box appended.
fn bracket_changed() -> String {
    let text = bracket()
        .replace("\"radius\": 2.5,", "\"radius\": 3,")
        .replace("\"name\": \"Hole\"", "\"name\": \"Bolt hole\"");
    let boss = r#"{"id": "b0000001", "type": "box", "v": 1, "name": "Boss", "position": [0, 0, 10], "length": 10, "width": 10, "height": 5}"#;
    let end = text.rfind("\n  ]\n}").unwrap();
    format!("{},\n    {boss}{}", &text[..end], &text[end..])
}

#[test]
fn the_examples_are_the_designs_text_and_the_sessions_answers() {
    let blocks = design("### `diff` (M5", "## 8. Versioning");
    let names = [
        "request",
        "result",
        "request-disk",
        "result-disk",
        "refused-not-held",
    ];
    assert_eq!(blocks.len(), names.len());
    for (block, name) in blocks.iter().zip(names) {
        assert_eq!(
            format!("{block}\n"),
            example(&format!("diff/{name}")),
            "{name}"
        );
    }
    let mut files: Vec<String> = std::fs::read_dir(contract("commands/examples/diff"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    files.sort();
    assert_eq!(
        files,
        [
            "refused-not-held.json",
            "request-disk.json",
            "request.json",
            "result-disk.json",
            "result.json"
        ]
    );

    // The request, read and written back.
    let text = example("diff/request");
    let request = DiffRequest::read(&json::read(text.as_bytes()).unwrap()).unwrap();
    assert_eq!(
        request,
        DiffRequest {
            session: SessionId::parse("s1").unwrap(),
            from: Some(at(1)),
            to: None,
        }
    );
    assert_eq!(written(&request), text);

    // The bracket opened at revision 1, changed outside, `status` stepped until the
    // session reloads it (revision 2), then the request: the design's result.
    let f = sessions();
    let s = f.open(bracket().as_bytes());
    f.write(bracket_changed().as_bytes());
    let two = f.reloads(&s, Revision::OPENED);
    assert_eq!(two.number(), 2);
    let diffed = answer(f.sessions.diff(&s, request.from, request.to));
    assert!(!diffed.session.as_ref().unwrap().reloaded);
    assert_eq!(written(&diffed.result.unwrap()), example("diff/result"));

    // Changed again, reloaded again (revision 3): revision 1's text is gone, the design's
    // refusal, the revisions held its candidates; revision 2's is the one now replaced.
    f.write(
        bracket_changed()
            .replace("\"radius\": 3,", "\"radius\": 4,")
            .as_bytes(),
    );
    assert_eq!(f.reloads(&s, two).number(), 3);
    let refused = f.diff(&s, Some(1), None);
    let problems = refused.result.unwrap_err();
    assert_eq!(problems.total(), 1);
    assert_eq!(
        written(&problems.errors()[0]),
        example("diff/refused-not-held")
    );
    assert_eq!(problems.errors()[0].candidates(), ["2", "3"]);
    // To 2 as well (revisions by number, reading nothing of the file): `from` refused alone,
    // the same refusal.
    let refused = f.diff(&s, Some(1), Some(2));
    assert!(!refused.session.as_ref().unwrap().reloaded);
    let problems = refused.result.unwrap_err();
    assert_eq!(problems.total(), 1);
    assert_eq!(
        written(&problems.errors()[0]),
        example("diff/refused-not-held")
    );
    // And from 2: the second change alone.
    let second = f.diff(&s, Some(2), None).result.unwrap();
    assert_eq!((number(second.from), number(second.to)), (2, 3));
    assert_eq!(
        lines(&second.changes),
        [r#"{"feature": "a91cw0de", "name": "Pin", "field": "radius", "before": 3, "after": 4}"#]
    );
    // Its reverse, `to` the replaced one.
    let back = f.diff(&s, None, Some(2)).result.unwrap();
    assert_eq!((number(back.from), number(back.to)), (3, 2));
    assert_eq!(
        lines(&back.changes),
        [r#"{"feature": "a91cw0de", "name": "Pin", "field": "radius", "before": 4, "after": 3}"#]
    );

    // The file on disk (Fable's 6d decision): the request, read and written back.
    let text = example("diff/request-disk");
    let request = DiffRequest::read(&json::read(text.as_bytes()).unwrap()).unwrap();
    assert_eq!(
        request,
        DiffRequest {
            session: SessionId::parse("s1").unwrap(),
            from: Some(at(1)),
            to: Some(Named::Disk),
        }
    );
    assert_eq!(written(&request), text);
    // Inside a transaction begun at revision 1, the block's height set from 10 to 12, and
    // the pin's radius written 2.5 to 3 to the file outside: the design's result.
    let f = sessions();
    let s = f.editing();
    f.write(agents_radius().as_bytes());
    let diffed = answer(f.sessions.diff(&s, request.from, request.to));
    assert!(!diffed.session.as_ref().unwrap().reloaded);
    assert_eq!(
        written(&diffed.result.unwrap()),
        example("diff/result-disk")
    );
}

#[test]
fn a_diff_sent_after_a_change_outside_reloads_first_and_reads_the_text_replaced() {
    // No `status`: the diff's own reread reloads (its response says so), and revision 1 is
    // then the one the reload replaced.
    let f = sessions();
    let s = f.open(bracket().as_bytes());
    f.write(bracket_changed().as_bytes());
    let diffed = f.diff(&s, Some(1), None);
    let session = diffed.session.unwrap();
    assert!(session.reloaded);
    assert_eq!(session.revision.number(), 2);
    let result = diffed.result.unwrap();
    assert_eq!((number(result.from), number(result.to)), (1, 2));
    assert_eq!(result.changes.len(), 3);
}

#[test]
fn a_hand_written_file_reloaded_as_its_canonical_text_diffs_empty() {
    // The reloaded-over side read from its text (the session keeps no model of it), the
    // current one from what the gate found: both as canonical form writes them.
    let hand = doc_text(&[&sketch("10", Some(THIRD)), BOX]);
    let f = sessions();
    let s = f.open(hand.as_bytes());
    let canonical = document::write(&document::read(hand.as_bytes()).unwrap()).unwrap();
    assert_ne!(canonical, hand);
    f.write(canonical.as_bytes());
    let diffed = f.diff(&s, Some(1), None);
    assert!(diffed.session.unwrap().reloaded);
    let result = diffed.result.unwrap();
    assert_eq!((number(result.from), number(result.to)), (1, 2));
    assert_eq!(result.changes, []);
}

#[test]
fn inside_a_transaction_its_base_is_held_until_it_ends() {
    let f = sessions();
    let s = f.open(doc_text(&[BOX]).as_bytes());
    // Nothing to compare yet: the document now with itself.
    let same = f.diff(&s, None, None).result.unwrap();
    assert_eq!(
        (number(same.from), number(same.to), same.changes.len()),
        (1, 1, 0)
    );
    answer(f.sessions.begin(&s)).result.unwrap();
    let set = answer(f.sessions.set_field(
        &s,
        Revision::OPENED,
        id("k7f2qx3m"),
        "length".to_owned(),
        json::read(b"50").unwrap(),
    ));
    let now = set.session.unwrap().revision;
    assert_eq!(now.number(), 2);
    let base = f.diff(&s, Some(1), None).result.unwrap();
    assert_eq!((number(base.from), number(base.to)), (1, 2));
    assert_eq!(
        lines(&base.changes),
        [
            r#"{"feature": "k7f2qx3m", "name": "Block", "field": "length", "before": 40, "after": 50}"#
        ]
    );
    // A revision the transaction passed through isn't held: its base and now are.
    answer(f.sessions.set_field(
        &s,
        now,
        id("k7f2qx3m"),
        "width".to_owned(),
        json::read(b"30").unwrap(),
    ))
    .result
    .unwrap();
    let refused = f.diff(&s, Some(2), None).result.unwrap_err();
    assert_eq!(refused.errors()[0].parameter(), Some("from"));
    assert_eq!(refused.errors()[0].candidates(), ["1", "3"]);
    // Rolled back: the base is gone with the transaction.
    let rolled = answer(f.sessions.rollback(&s));
    let after = rolled.session.unwrap().revision;
    let refused = f.diff(&s, None, Some(1)).result.unwrap_err();
    assert_eq!(refused.errors()[0].parameter(), Some("to"));
    assert_eq!(refused.errors()[0].code(), ErrorCode::InvalidInput);
    assert_eq!(
        refused.errors()[0].candidates(),
        [after.number().to_string()]
    );
}

#[test]
fn committed_its_base_is_let_go_with_the_transaction() {
    let f = sessions();
    let s = f.open(doc_text(&[BOX]).as_bytes());
    answer(f.sessions.begin(&s)).result.unwrap();
    answer(f.sessions.set_field(
        &s,
        Revision::OPENED,
        id("k7f2qx3m"),
        "length".to_owned(),
        json::read(b"50").unwrap(),
    ))
    .result
    .unwrap();
    // Held while the transaction is open.
    assert_eq!(f.diff(&s, Some(1), None).result.unwrap().changes.len(), 1);
    let committed = answer(f.sessions.commit(&s));
    assert!(committed.result.unwrap().written);
    let now = committed.session.unwrap().revision;
    // Committed: the base is gone with the transaction, and the revision written is the
    // one held.
    let refused = f.diff(&s, Some(1), None).result.unwrap_err();
    assert_eq!(refused.total(), 1);
    let e = &refused.errors()[0];
    assert_eq!(
        (e.code(), e.parameter()),
        (ErrorCode::InvalidInput, Some("from"))
    );
    assert_eq!(e.candidates(), [now.number().to_string()]);
}

#[test]
fn an_untitled_session_and_one_never_reloaded_hold_their_revision_alone() {
    let f = sessions();
    let s = answer(f.sessions.open_untitled()).session.unwrap().id;
    let same = f.diff(&s, Some(1), None).result.unwrap();
    assert_eq!(
        (number(same.from), number(same.to), same.changes.len()),
        (1, 1, 0)
    );
    for (from, to, parameter) in [(Some(2), None, "from"), (None, Some(7), "to")] {
        let refused = f.diff(&s, from, to).result.unwrap_err();
        assert_eq!(refused.total(), 1);
        let e = &refused.errors()[0];
        assert_eq!(e.parameter(), Some(parameter));
        assert_eq!(e.candidates(), ["1"]);
    }
    // Both sides not held: both refused, `from` first.
    let refused = f.diff(&s, Some(5), Some(6)).result.unwrap_err();
    let named: Vec<_> = refused.errors().iter().map(|e| e.parameter()).collect();
    assert_eq!(named, [Some("from"), Some("to")]);
}

#[test]
fn a_side_that_doesnt_read_as_a_document_is_refused_with_the_readers_message() {
    let f = sessions();
    let s = f.open(b"not json");
    let reader = document::read(b"not json").unwrap_err();
    let refused = f.diff(&s, None, None).result.unwrap_err();
    assert_eq!(refused.total(), 2);
    for (e, side) in refused.errors().iter().zip(["from", "to"]) {
        assert_eq!(e.code(), ErrorCode::InvalidInput);
        assert_eq!(e.parameter(), Some(side));
        assert_eq!(e.cause(), Some(reader.message()));
        assert_eq!(
            e.message(),
            format!("{side}, revision 1, doesn't read as a document, so it can't be compared")
        );
        assert_eq!(
            e.suggestion(),
            Some(
                format!(
                    "give {side} a revision whose text reads as a document; text gives a revision's text"
                )
                .as_str()
            )
        );
    }
    // Mended outside: the side now reads, the one replaced still doesn't.
    f.write(doc_text(&[BOX]).as_bytes());
    let refused = f.diff(&s, Some(1), None).result.unwrap_err();
    assert_eq!(refused.total(), 1);
    assert_eq!(refused.errors()[0].parameter(), Some("from"));
    assert_eq!(refused.errors()[0].cause(), Some(reader.message()));
    assert_eq!(f.diff(&s, Some(2), Some(2)).result.unwrap().changes, []);
}

#[test]
fn the_sessions_answer_is_the_same_bytes_every_time() {
    let f = sessions();
    let s = f.open(bracket().as_bytes());
    f.write(bracket_changed().as_bytes());
    f.reloads(&s, Revision::OPENED);
    let first = response_text(&f.diff(&s, Some(1), None), Layout::OneLine);
    assert!(first.contains("\"ok\": true"), "{first}");
    for _ in 0..3 {
        assert_eq!(
            response_text(&f.diff(&s, Some(1), None), Layout::OneLine),
            first
        );
    }
}

// --- The file on disk as a side (Fable's decision of 2026-11-04, 6d's other half). ---

/// The bracket as an agent wrote it to the file: the pin's radius 2.5 to 3, nothing else.
fn agents_radius() -> String {
    bracket().replace("\"radius\": 2.5,", "\"radius\": 3,")
}

/// The pin's radius changed, 2.5 to 3, as the API writes it on one line.
const PIN_RADIUS: &str =
    r#"{"feature": "a91cw0de", "name": "Pin", "field": "radius", "before": 2.5, "after": 3}"#;

impl Fixture {
    /// `status`'s transaction and disk finding.
    fn held(&self, s: &SessionId) -> (Option<silico_core::command::Transaction>, Option<Disk>) {
        let status = self.sessions.status(s).result.unwrap();
        (status.transaction, status.disk)
    }

    /// `status` asked until its disk finding is `want`: as the app's frames ask it.
    #[track_caller]
    fn finds(&self, s: &SessionId, want: Disk) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if self.held(s).1 == Some(want) {
                return;
            }
            assert!(Instant::now() < deadline, "never found {want:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The bracket opened, a transaction begun at revision 1, and the block's height set
    /// from 10 to 12 (revision 2): the person's edit, held.
    fn editing(&self) -> SessionId {
        let s = self.open(bracket().as_bytes());
        answer(self.sessions.begin(&s)).result.unwrap();
        let set = answer(self.sessions.set_field(
            &s,
            Revision::OPENED,
            id("k7f2qx3m"),
            "height".to_owned(),
            json::read(b"12").unwrap(),
        ));
        assert_eq!(set.session.unwrap().revision.number(), 2);
        s
    }
}

#[test]
fn inside_a_transaction_disk_is_the_file_changed_outside_and_nothing_held_moves() {
    let f = sessions();
    let s = f.open(bracket().as_bytes());
    answer(f.sessions.begin(&s)).result.unwrap();
    f.write(agents_radius().as_bytes());
    // The watch finds the change, and, inside a transaction, reloads nothing.
    f.finds(&s, Disk::Changed);
    let before = f.held(&s).0.unwrap();
    let diffed = f.diff_of(&s, Some(at(1)), Some(Named::Disk));
    let session = diffed.session.unwrap();
    assert_eq!((session.revision.number(), session.reloaded), (1, false));
    let result = diffed.result.unwrap();
    assert_eq!((result.from, result.to), (at(1), Named::Disk));
    assert_eq!(lines(&result.changes), [PIN_RADIUS]);
    // Nothing held moved: the revision, the transaction's stack, the watch's finding.
    assert_eq!(f.revision(&s).number(), 1);
    let (after, disk) = f.held(&s);
    let after = after.unwrap();
    assert_eq!(
        (
            after.base,
            after.undo,
            after.redo,
            after.bytes,
            after.changed
        ),
        (
            before.base,
            before.undo,
            before.redo,
            before.bytes,
            before.changed
        )
    );
    assert_eq!(disk, Some(Disk::Changed));
    // The text the session holds is its own still.
    assert_eq!(
        answer(f.sessions.text(&s, None)).result.unwrap().text,
        bracket()
    );
}

#[test]
fn with_the_persons_edit_held_the_two_lists_are_theirs_and_mine() {
    let f = sessions();
    let s = f.editing();
    f.write(agents_radius().as_bytes());
    // From the document now to the file: the person's edit undone, then the agent's change.
    let now_to_disk = f.diff_of(&s, None, Some(Named::Disk));
    assert!(!now_to_disk.session.unwrap().reloaded);
    let now_to_disk = now_to_disk.result.unwrap();
    assert_eq!((now_to_disk.from, now_to_disk.to), (at(2), Named::Disk));
    assert_eq!(
        lines(&now_to_disk.changes),
        [
            r#"{"feature": "k7f2qx3m", "name": "Block", "field": "height", "before": 12, "after": 10}"#,
            PIN_RADIUS,
        ]
    );
    // 6d's two lists: theirs, from the base to the file; mine, from the base to now.
    let theirs = f
        .diff_of(&s, Some(at(1)), Some(Named::Disk))
        .result
        .unwrap();
    assert_eq!(lines(&theirs.changes), [PIN_RADIUS]);
    let mine = f.diff(&s, Some(1), Some(2)).result.unwrap();
    assert_eq!((mine.from, mine.to), (at(1), at(2)));
    assert_eq!(
        lines(&mine.changes),
        [
            r#"{"feature": "k7f2qx3m", "name": "Block", "field": "height", "before": 10, "after": 12}"#
        ]
    );
    assert_eq!(f.revision(&s).number(), 2);
}

#[test]
fn outside_a_transaction_disk_reloads_first_and_is_the_file_reloaded() {
    let f = sessions();
    let s = f.open(bracket().as_bytes());
    f.write(agents_radius().as_bytes());
    // The document now reads the file first, as every command does: reloaded at 2, and the
    // file is what it reloaded, so nothing differs.
    let diffed = f.diff_of(&s, None, Some(Named::Disk));
    let session = diffed.session.unwrap();
    assert_eq!((session.revision.number(), session.reloaded), (2, true));
    let result = diffed.result.unwrap();
    assert_eq!((result.from, result.to), (at(2), Named::Disk));
    assert_eq!(result.changes, []);
    // From the revision the reload replaced: the change.
    let replaced = f.diff_of(&s, Some(at(1)), Some(Named::Disk));
    assert!(!replaced.session.unwrap().reloaded);
    let replaced = replaced.result.unwrap();
    assert_eq!((replaced.from, replaced.to), (at(1), Named::Disk));
    assert_eq!(lines(&replaced.changes), [PIN_RADIUS]);
    // And `"disk"` alone reloads too: written again, read with revision 1 on the other side.
    f.write(
        agents_radius()
            .replace("\"radius\": 3,", "\"radius\": 4,")
            .as_bytes(),
    );
    let again = f.diff_of(&s, Some(Named::Disk), Some(at(2)));
    let session = again.session.unwrap();
    assert_eq!((session.revision.number(), session.reloaded), (3, true));
    let again = again.result.unwrap();
    assert_eq!((again.from, again.to), (Named::Disk, at(2)));
    assert_eq!(
        lines(&again.changes),
        [r#"{"feature": "a91cw0de", "name": "Pin", "field": "radius", "before": 4, "after": 3}"#]
    );
}

#[test]
fn the_file_unchanged_inside_a_transaction_is_the_bytes_held() {
    let f = sessions();
    let s = f.editing();
    // Nothing written outside: the file is as the session holds it.
    let base = f
        .diff_of(&s, Some(at(1)), Some(Named::Disk))
        .result
        .unwrap();
    assert_eq!((base.from, base.to), (at(1), Named::Disk));
    assert_eq!(base.changes, []);
    let both = f
        .diff_of(&s, Some(Named::Disk), Some(Named::Disk))
        .result
        .unwrap();
    assert_eq!((both.from, both.to), (Named::Disk, Named::Disk));
    assert_eq!(both.changes, []);
    // From the file to the document now: the person's edit alone.
    let mine = f.diff_of(&s, Some(Named::Disk), None).result.unwrap();
    assert_eq!(
        lines(&mine.changes),
        [
            r#"{"feature": "k7f2qx3m", "name": "Block", "field": "height", "before": 10, "after": 12}"#
        ]
    );
    // Two `"disk"` sides of a changed file are one read: the same bytes, nothing between.
    f.write(agents_radius().as_bytes());
    let both = f
        .diff_of(&s, Some(Named::Disk), Some(Named::Disk))
        .result
        .unwrap();
    assert_eq!(both.changes, []);
}

/// The one refusal `response` gives: its code, parameter, and cause.
#[track_caller]
fn refused_side(response: &Response<DiffAnswer>) -> (ErrorCode, Option<String>, Option<String>) {
    let problems = response.result.as_ref().unwrap_err();
    assert_eq!(problems.total(), 1, "{problems:?}");
    let e = &problems.errors()[0];
    (
        e.code(),
        e.parameter().map(str::to_owned),
        e.cause().map(str::to_owned),
    )
}

/// The cause of a `"disk"` side refused inside a transaction.
const DISK_READ: &str =
    "\"disk\" is the session's file as this command reads it, inside a transaction too";

#[test]
fn a_disk_side_the_file_cant_give_is_refused_naming_the_side_the_transaction_left_open() {
    let f = sessions();
    let s = f.editing();
    let open = || {
        let t = f.held(&s).0.unwrap();
        assert_eq!((t.base.number(), t.changed), (1, true));
        assert_eq!(f.revision(&s).number(), 2);
    };
    // Gone: `io_failed`, naming each side that's `"disk"`, the first only where both are.
    std::fs::remove_file(f.path()).unwrap();
    for (from, to, side) in [
        (Some(at(1)), Some(Named::Disk), "to"),
        (Some(Named::Disk), None, "from"),
    ] {
        let refused = f.diff_of(&s, from, to);
        let problems = refused.result.as_ref().unwrap_err();
        let e = &problems.errors()[0];
        assert_eq!(
            refused_side(&refused),
            (
                ErrorCode::IoFailed,
                Some(side.to_owned()),
                Some(DISK_READ.to_owned())
            )
        );
        let suggestion = e.suggestion().unwrap();
        assert!(
            suggestion.starts_with("put the file back at ")
                && suggestion.ends_with(", then read \"disk\" again"),
            "{suggestion}"
        );
        open();
    }
    // Both `"disk"`: each refused, the one read's finding.
    let both = f
        .diff_of(&s, Some(Named::Disk), Some(Named::Disk))
        .result
        .unwrap_err();
    let named: Vec<_> = both.errors().iter().map(|e| e.parameter()).collect();
    assert_eq!(named, [Some("from"), Some("to")]);
    // Revisions by number answer while it's gone.
    assert_eq!(
        lines(&f.diff(&s, Some(1), Some(2)).result.unwrap().changes).len(),
        1
    );
    // Not a document: the reader's message the cause, naming the side.
    f.write(b"{");
    let reader = document::read(b"{").unwrap_err();
    let refused = f.diff_of(&s, Some(at(1)), Some(Named::Disk));
    assert_eq!(
        refused_side(&refused),
        (
            ErrorCode::InvalidInput,
            Some("to".to_owned()),
            Some(reader.message().to_owned())
        )
    );
    let e = &refused.result.as_ref().unwrap_err().errors()[0];
    assert_eq!(
        e.message(),
        "to, the file on disk, doesn't read as a document, so it can't be compared"
    );
    // The file to mend, not a revision to choose.
    assert_eq!(
        e.suggestion(),
        Some("mend the file on disk, or give to a revision the session holds")
    );
    open();
    // And mended outside, it reads again: nothing reloaded, the edit still held.
    f.write(agents_radius().as_bytes());
    let mended = f
        .diff_of(&s, Some(at(1)), Some(Named::Disk))
        .result
        .unwrap();
    assert_eq!(lines(&mended.changes), [PIN_RADIUS]);
    open();
}

#[test]
fn outside_a_transaction_a_disk_side_the_read_refuses_names_the_side() {
    // The read every command makes before it runs refuses the file gone, as for any
    // command, but naming the `"disk"` side, which asked for the file, rather than `path`.
    let f = sessions();
    let s = f.open(bracket().as_bytes());
    std::fs::remove_file(f.path()).unwrap();
    let refused = f.diff_of(&s, Some(at(1)), Some(Named::Disk));
    let (code, parameter, cause) = refused_side(&refused);
    assert_eq!(code, ErrorCode::IoFailed);
    assert_eq!(parameter.as_deref(), Some("to"));
    assert_eq!(cause.as_deref(), Some(silico_core::command::NOT_RELOADED));
    // A `null` side alone names `path`, as every command does.
    let refused = f.diff(&s, Some(1), None);
    assert_eq!(refused_side(&refused).1.as_deref(), Some("path"));
    // Revisions by number alone answer.
    assert_eq!(f.diff(&s, Some(1), Some(1)).result.unwrap().changes, []);
}

#[test]
fn an_untitled_session_has_no_disk_and_says_so() {
    let f = sessions();
    let s = answer(f.sessions.open_untitled()).session.unwrap().id;
    // Its first save is a commit given a path: outside a transaction, where `commit` is
    // refused, one begun first, as an edit's refusal there says.
    for (transaction, save) in [
        (
            false,
            "begin a transaction first; its commit, given a path, saves it (Save As)",
        ),
        (true, "commit it given a path, which saves it (Save As)"),
    ] {
        if transaction {
            answer(f.sessions.begin(&s)).result.unwrap();
        }
        let suggestion = |parameter: &str| {
            format!(
                "{save}; until then, give {parameter} a revision the session holds, or null for the document now"
            )
        };
        let refused = f.diff_of(&s, None, Some(Named::Disk));
        assert_eq!(
            refused_side(&refused),
            (
                ErrorCode::InvalidInput,
                Some("to".to_owned()),
                Some("the session is untitled: nothing is on disk until its first save".to_owned())
            )
        );
        assert_eq!(
            refused.result.as_ref().unwrap_err().errors()[0].suggestion(),
            Some(suggestion("to").as_str())
        );
        let text = answer(f.sessions.text_at(&s, None, Some(Named::Disk)));
        let e = &text.result.as_ref().unwrap_err().errors()[0];
        assert_eq!(e.parameter(), Some("revision"));
        assert_eq!(
            e.cause(),
            Some("the session is untitled: nothing is on disk until its first save")
        );
        assert_eq!(e.suggestion(), Some(suggestion("revision").as_str()));
    }
}

#[test]
fn undo_after_disk_reads_steps_back_as_without_them() {
    let f = sessions();
    let s = f.editing();
    let set = answer(f.sessions.set_field(
        &s,
        Revision::from_number(2).unwrap(),
        id("k7f2qx3m"),
        "width".to_owned(),
        json::read(b"30").unwrap(),
    ));
    let three = set.session.unwrap().revision;
    assert_eq!(three.number(), 3);
    f.write(agents_radius().as_bytes());
    f.diff_of(&s, Some(at(1)), Some(Named::Disk))
        .result
        .unwrap();
    answer(f.sessions.text_at(&s, None, Some(Named::Disk)))
        .result
        .unwrap();
    // Undone: the state before the last edit, the height 12 and the width 20.
    let undone = answer(f.sessions.undo(&s, three, None)).result.unwrap();
    assert_eq!(undone.restored.number(), 2);
    let now = f.diff(&s, Some(1), None).result.unwrap();
    assert_eq!(
        lines(&now.changes),
        [
            r#"{"feature": "k7f2qx3m", "name": "Block", "field": "height", "before": 10, "after": 12}"#
        ]
    );
}

#[test]
fn the_same_disk_diff_is_the_same_bytes_every_time() {
    let f = sessions();
    let s = f.editing();
    f.write(agents_radius().as_bytes());
    let first = response_text(
        &f.diff_of(&s, Some(at(1)), Some(Named::Disk)),
        Layout::OneLine,
    );
    assert!(first.contains("\"to\": \"disk\""), "{first}");
    for _ in 0..3 {
        assert_eq!(
            response_text(
                &f.diff_of(&s, Some(at(1)), Some(Named::Disk)),
                Layout::OneLine
            ),
            first
        );
    }
}
