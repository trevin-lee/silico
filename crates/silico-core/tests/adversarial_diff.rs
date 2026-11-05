//! Hostile inputs for `diff` (break-it): documents at the size limits (50,000 features, a
//! side of just under 16 MiB, a side past it), sides that aren't documents (not JSON, not
//! UTF-8, a byte-order mark, JSON of another shape), documents the gate refuses but that
//! read (an ID two features share, an ID a sketch's points share, a feature whose type
//! changed under its ID), a name of a
//! megabyte on a feature with thousands of changes, requests wrong in every way the reader
//! can be, and a diff cancelled. Expected values: counted and placed by hand, the reader's
//! own message where the refusal quotes it, and the rule that every answer is a result or a
//! structured refusal naming the side.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::wire::{Layout, MAX_RESPONSE_LEN, Named, ToWire, response_text, write};
use silico_core::command::{Call, DiffAnswer, Response, Revision, SessionId, Sessions};
use silico_core::document::json::MAX_DOCUMENT_LEN;
use silico_core::document::{self, Change};
use silico_core::fake::FakeKernel;
use tempfile::TempDir;

use crate::sessions::answer;

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

struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
}

fn sessions() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sessions =
        Sessions::with_cache_opener(kernel, move |cache| Cache::with_secret(cache, &secret));
    Fixture { dir, sessions }
}

impl Fixture {
    fn path(&self) -> PathBuf {
        self.dir.path().join("a.silico")
    }

    fn write(&self, text: &[u8]) {
        std::fs::write(self.path(), text).unwrap();
    }

    fn open(&self, text: &[u8]) -> SessionId {
        self.write(text);
        answer(self.sessions.open(self.path(), false))
            .session
            .unwrap()
            .id
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

    /// The text written outside, then `diff(from, null)`, whose own reread reloads it.
    fn reloaded_diff(&self, s: &SessionId, text: &[u8], from: u64) -> Response<DiffAnswer> {
        self.write(text);
        let diffed = self.diff(s, Some(from), None);
        assert!(diffed.session.as_ref().unwrap().reloaded);
        diffed
    }
}

fn header(features: &str) -> String {
    format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{features}]}}"#
    )
}

/// The ID of feature `n`: 8 characters of the alphabet, distinct for each `n` below 32^7.
fn feature_id(n: usize) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let mut id = String::from("b");
    let mut rest = n;
    for _ in 0..7 {
        id.insert(1, char::from(ALPHABET[rest % 32]));
        rest /= 32;
    }
    id
}

fn a_box(n: usize, name: &str, length: u32) -> String {
    format!(
        r#"{{"id": "{}", "type": "box", "v": 1, "name": "{name}", "position": [0, 0, 0], "length": {length}, "width": 2, "height": 1}}"#,
        feature_id(n)
    )
}

fn lines(changes: &[Change]) -> Vec<String> {
    changes
        .iter()
        .map(|c| write(&c.to_wire(), Layout::OneLine).unwrap())
        .collect()
}

// --- Size. ---

#[test]
fn fifty_thousand_features_one_moved_to_the_end_moves_every_other_once() {
    // 50,000 boxes; the first moved to the end and every thousandth lengthened. By rank:
    // feature 0 goes from 0 to 49,999; each other from n to n - 1. So 50,000 moves, and
    // 50 field changes (features 0, 1000, ..., 49000), each right after its move.
    const N: usize = 50_000;
    let from: Vec<String> = (0..N).map(|n| a_box(n, "B", 4)).collect();
    let mut to: Vec<String> = (1..N)
        .map(|n| a_box(n, "B", if n % 1000 == 0 { 5 } else { 4 }))
        .collect();
    to.push(a_box(0, "B", 5));
    let from_doc = document::read(header(&from.join(", ")).as_bytes()).unwrap();
    let to_doc = document::read(header(&to.join(", ")).as_bytes()).unwrap();
    let changes = document::diff(&from_doc, &to_doc).unwrap();
    assert_eq!(changes.len(), N + 50);
    let moves = changes.iter().filter(|c| c.field.is_none()).count();
    assert_eq!(moves, N);
    let shown = lines(&changes);
    assert_eq!(
        shown[0],
        format!(
            r#"{{"feature": "{}", "name": "B", "field": null, "before": 1, "after": 0}}"#,
            feature_id(1)
        )
    );
    // Feature 1000, from place 1000 to 999, then its length.
    let at = shown
        .iter()
        .position(|l| l.contains(&format!("\"{}\"", feature_id(1000))))
        .unwrap();
    assert_eq!(
        shown[at..at + 2],
        [
            format!(
                r#"{{"feature": "{}", "name": "B", "field": null, "before": 1000, "after": 999}}"#,
                feature_id(1000)
            ),
            format!(
                r#"{{"feature": "{}", "name": "B", "field": "length", "before": 4, "after": 5}}"#,
                feature_id(1000)
            ),
        ]
    );
    assert_eq!(
        shown[shown.len() - 2..],
        [
            format!(
                r#"{{"feature": "{}", "name": "B", "field": null, "before": 0, "after": 49999}}"#,
                feature_id(0)
            ),
            format!(
                r#"{{"feature": "{}", "name": "B", "field": "length", "before": 4, "after": 5}}"#,
                feature_id(0)
            ),
        ]
    );

    // And through a session: the file rewritten outside, reloaded by the diff, answered
    // within the response's limit, the same bytes twice.
    let f = sessions();
    let s = f.open(header(&from.join(", ")).as_bytes());
    let diffed = f.reloaded_diff(&s, header(&to.join(", ")).as_bytes(), 1);
    assert_eq!(diffed.result.unwrap().changes, changes);
    // Asked again, with no reload: the same bytes each time.
    let text = response_text(&f.diff(&s, Some(1), None), Layout::OneLine);
    assert!(text.starts_with(r#"{"ok": true"#), "{}", &text[..200]);
    assert_eq!(
        response_text(&f.diff(&s, Some(1), None), Layout::OneLine),
        text
    );
}

/// A document of `n` boxes whose canonical text is as near [`MAX_DOCUMENT_LEN`] as names
/// of one length make it, without passing it; the `changed`th box's length 5, the others 4.
fn near_the_limit(n: usize, changed: Option<usize>) -> String {
    let text = |name_len: usize| {
        let name = "n".repeat(name_len);
        let features: Vec<String> = (0..n)
            .map(|i| a_box(i, &name, if Some(i) == changed { 5 } else { 4 }))
            .collect();
        header(&features.join(", "))
    };
    let canonical = |t: &str| document::write(&document::read(t.as_bytes()).unwrap()).unwrap();
    let base = canonical(&text(1)).len() - n;
    let name_len = (MAX_DOCUMENT_LEN - base) / n;
    let full = text(name_len);
    let len = canonical(&full).len();
    assert!(
        len <= MAX_DOCUMENT_LEN && len > MAX_DOCUMENT_LEN - n,
        "{len}"
    );
    full
}

#[test]
fn a_side_of_just_under_sixteen_mib_against_one_changed_value_is_one_change() {
    let f = sessions();
    let from = near_the_limit(1000, None);
    let s = f.open(from.as_bytes());
    let diffed = f.reloaded_diff(&s, near_the_limit(1000, Some(617)).as_bytes(), 1);
    let result = diffed.result.unwrap();
    assert_eq!((number(result.from), number(result.to)), (1, 2));
    assert_eq!(result.changes.len(), 1);
    let change = &result.changes[0];
    assert_eq!(change.feature.as_str(), feature_id(617));
    assert_eq!(change.field.as_deref(), Some("length"));
    // The name, of some 16 kilobytes, given whole.
    assert!(change.name.len() > 16_000, "{}", change.name.len());
    assert!(change.name.bytes().all(|b| b == b'n'));
}

#[test]
fn a_side_past_sixteen_mib_is_refused_naming_it_with_the_readers_message() {
    // Reloaded from a text the reader refuses as too large (it's within what a file may
    // be, four times that): the session holds it unread, and `to` is refused for it,
    // `from` (the text the reload replaced, read again) diffed fine on its own.
    let f = sessions();
    let from = near_the_limit(1000, None);
    let s = f.open(from.as_bytes());
    let past = from.replacen("\"name\": \"n", "\"name\": \"nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn", 1000);
    let reader = document::read(past.as_bytes()).unwrap_err();
    assert!(
        reader.message().contains("too large"),
        "{}",
        reader.message()
    );
    let refused = f.reloaded_diff(&s, past.as_bytes(), 1).result.unwrap_err();
    assert_eq!(refused.total(), 1);
    let e = &refused.errors()[0];
    assert_eq!(e.code(), ErrorCode::InvalidInput);
    assert_eq!(e.parameter(), Some("to"));
    assert_eq!(e.cause(), Some(reader.message()));
    assert_eq!(f.diff(&s, Some(1), Some(1)).result.unwrap().changes, []);
}

#[test]
fn a_megabyte_name_on_thousands_of_changes_is_refused_saying_the_size_and_the_limit() {
    // A sketch named with a megabyte, 2,000 points each moved in x and y: 4,000 changes,
    // each written with the name, some 4 GiB in all. The changes share the name, and the
    // answer is refused as too long, saying its size, counted by hand below, and the limit,
    // never written whole.
    const L: usize = 1 << 20;
    let name = "m".repeat(L);
    let point_id = |i: usize| {
        let mut id = feature_id(i);
        id.replace_range(0..1, "p");
        id
    };
    let points = |x: u32| -> String {
        (0..2000)
            .map(|i| {
                format!(
                    r#"{{"id": "{}", "x": {x}, "y": {x}, "construction": false}}"#,
                    point_id(i)
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let sketch = |x: u32| {
        header(&format!(
            r#"{{"id": "s0000001", "type": "sketch", "v": 1, "name": "{name}", "plane": "xy", "points": [{}], "lines": [], "circles": [], "arcs": [], "constraints": []}}"#,
            points(x)
        ))
    };
    let from = document::read(sketch(1).as_bytes()).unwrap();
    let to = document::read(sketch(2).as_bytes()).unwrap();
    let changes = document::diff(&from, &to).unwrap();
    assert_eq!(changes.len(), 4000);
    // One name, shared by every change.
    assert!(
        changes
            .iter()
            .all(|c| Arc::ptr_eq(&c.name, &changes[0].name))
    );

    // Through a session's call, as MCP's server makes it: the file rewritten outside,
    // reloaded by the diff.
    let f = sessions();
    let _ = f.open(sketch(1).as_bytes());
    f.write(sketch(2).as_bytes());
    let answer = match f
        .sessions
        .call("diff", r#"{"session": "s1", "from": 1, "to": null}"#)
    {
        Call::Running(calling) => calling.wait(),
        Call::Unknown => panic!("diff is a command"),
    };
    assert!(!answer.ok);
    assert!(answer.text.len() < 4096, "{}", answer.text.len());
    // The answer it would have been, by hand: the envelope, then each change on one line,
    // ", " between them, its name counted as L bytes.
    let path = f.path().display().to_string();
    let head = format!(
        r#"{{"ok": true, "session": {{"id": "s1", "path": "{path}", "revision": 2, "transaction": false, "reloaded": true}}, "model": null, "result": {{"from": 1, "to": 2, "changes": ["#
    );
    let tail = "]}}";
    let mut size = head.len() + tail.len() + 2 * (4000 - 1);
    for i in 0..2000 {
        for axis in ["x", "y"] {
            let line = format!(
                r#"{{"feature": "s0000001", "name": "", "field": "points.{}.{axis}", "before": 1, "after": 2}}"#,
                point_id(i)
            );
            size += line.len() + L;
        }
    }
    let value: serde_json::Value = serde_json::from_str(&answer.text).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["session"]["revision"], 2);
    let error = &value["errors"][0];
    assert_eq!(error["code"], "invalid_input");
    assert_eq!(
        error["message"],
        format!(
            "the diff from revision 1 to revision 2 would be {size} bytes, more than the {MAX_RESPONSE_LEN} bytes a response may be"
        )
    );
    assert_eq!(
        error["cause"],
        "it has 4000 changes, each written with its feature's name"
    );
    assert_eq!(
        error["suggestion"],
        "diff revisions nearer each other, or read the text of each side with text instead"
    );
    // Nearer revisions answer: the session at revision 2 with itself.
    let answer = match f
        .sessions
        .call("diff", r#"{"session": "s1", "from": 2, "to": null}"#)
    {
        Call::Running(calling) => calling.wait(),
        Call::Unknown => panic!("diff is a command"),
    };
    assert!(answer.ok, "{}", answer.text);
}

// --- Sides that aren't documents. ---

#[test]
fn sides_that_arent_documents_are_refused_each_with_the_readers_message() {
    let good = header(&a_box(0, "B", 4));
    let not_documents: [&[u8]; 6] = [
        b"",
        b"[]",
        br#"{"silico": 2}"#,
        b"\xff\xfe not utf-8",
        "\u{feff}{}".as_bytes(),
        br#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "bad"}]}"#,
    ];
    for bytes in not_documents {
        let f = sessions();
        let s = f.open(good.as_bytes());
        let reader = document::read(bytes).unwrap_err();
        let refused = f.reloaded_diff(&s, bytes, 1).result.unwrap_err();
        assert_eq!(refused.total(), 1, "{bytes:?}");
        let e = &refused.errors()[0];
        assert_eq!(e.code(), ErrorCode::InvalidInput);
        assert_eq!(e.parameter(), Some("to"), "{bytes:?}");
        assert_eq!(e.cause(), Some(reader.message()), "{bytes:?}");
        // And the other way: what the reload replaced reads, the document now doesn't.
        let refused = f.diff(&s, None, Some(1)).result.unwrap_err();
        assert_eq!(refused.errors()[0].parameter(), Some("from"));
        // Mended: the session goes on.
        f.write(good.as_bytes());
        let mended = f.diff(&s, Some(2), None);
        assert!(mended.session.as_ref().unwrap().reloaded);
        let mended = mended.result.unwrap_err();
        assert_eq!(mended.errors()[0].parameter(), Some("from"));
        assert_eq!(f.diff(&s, None, None).result.unwrap().changes, []);
    }
}

#[test]
fn line_ends_and_spacing_the_reader_takes_change_nothing() {
    let f = sessions();
    let canonical =
        document::write(&document::read(header(&a_box(0, "B", 4)).as_bytes()).unwrap()).unwrap();
    let s = f.open(canonical.as_bytes());
    let crlf = canonical.replace('\n', "\r\n").replace(": ", ":\t ");
    let result = f.reloaded_diff(&s, crlf.as_bytes(), 1).result.unwrap();
    assert_eq!((number(result.from), number(result.to)), (1, 2));
    assert_eq!(result.changes, []);
}

// --- Documents that read but the gate refuses. ---

#[test]
fn an_id_two_features_share_pairs_first_with_first_and_second_with_second() {
    let a = |length| {
        format!(
            r#"{{"id": "a0000001", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": {length}, "width": 2, "height": 1}}"#
        )
    };
    let from = document::read(header(&format!("{}, {}", a(4), a(5))).as_bytes()).unwrap();
    // The second gone: removed from place 1.
    let to = document::read(header(&a(4)).as_bytes()).unwrap();
    assert_eq!(
        lines(&document::diff(&from, &to).unwrap()),
        [r#"{"feature": "a0000001", "name": "A", "field": null, "before": 1, "after": null}"#]
    );
    // The first changed: its own field, the second untouched.
    let to = document::read(header(&format!("{}, {}", a(6), a(5))).as_bytes()).unwrap();
    assert_eq!(
        lines(&document::diff(&from, &to).unwrap()),
        [r#"{"feature": "a0000001", "name": "A", "field": "length", "before": 4, "after": 6}"#]
    );
    // Through a session, which holds the document the gate refuses.
    let f = sessions();
    let s = f.open(header(&format!("{}, {}", a(4), a(5))).as_bytes());
    let result = f
        .reloaded_diff(&s, header(&a(4)).as_bytes(), 1)
        .result
        .unwrap();
    assert_eq!(result.changes.len(), 1);
}

#[test]
fn an_id_three_points_share_pairs_first_with_first_second_with_second_third_with_third() {
    // A sketch whose three points share one ID: the writer keeps their order (it sorts a
    // list by ID, stably), so each is paired with the one at its own count on the other
    // side.
    let sketch = |xs: &[u32]| {
        let points: Vec<String> = xs
            .iter()
            .map(|x| format!(r#"{{"id": "pa000001", "x": {x}, "y": 0, "construction": false}}"#))
            .collect();
        let text = header(&format!(
            r#"{{"id": "s0000001", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{}], "lines": [], "circles": [], "arcs": [], "constraints": []}}"#,
            points.join(", ")
        ));
        document::read(text.as_bytes()).unwrap()
    };
    let change = |field: &str, before: &str, after: &str| {
        format!(
            r#"{{"feature": "s0000001", "name": "S", "field": "{field}", "before": {before}, "after": {after}}}"#
        )
    };
    // The second moved: the second's x, 2 to 9, and nothing of the first or the third.
    assert_eq!(
        lines(&document::diff(&sketch(&[1, 2, 3]), &sketch(&[1, 9, 3])).unwrap()),
        [change("points.pa000001.x", "2", "9")]
    );
    // The third gone: the third, whole, to nothing; the first and second untouched.
    assert_eq!(
        lines(&document::diff(&sketch(&[1, 2, 3]), &sketch(&[1, 2])).unwrap()),
        [change(
            "points.pa000001",
            r#"{"id": "pa000001", "x": 3, "y": 0, "construction": false}"#,
            "null"
        )]
    );
}

#[test]
fn a_feature_whose_type_changed_under_its_id_lists_each_field_in_both_orders() {
    // A box's fields (position, length, width, height) and a cylinder's (position,
    // radius, height), merged: type, then length and width gone, radius new, height
    // changed, in the order both write them.
    let boxed = r#"{"id": "a0000001", "type": "box", "v": 1, "name": "A", "position": [0, 0, 0], "length": 4, "width": 2, "height": 1}"#;
    let cylinder = r#"{"id": "a0000001", "type": "cylinder", "v": 1, "name": "A", "position": [0, 0, 0], "radius": 3, "height": 7}"#;
    let from = document::read(header(boxed).as_bytes()).unwrap();
    let to = document::read(header(cylinder).as_bytes()).unwrap();
    assert_eq!(
        lines(&document::diff(&from, &to).unwrap()),
        [
            r#"{"feature": "a0000001", "name": "A", "field": "type", "before": "box", "after": "cylinder"}"#,
            r#"{"feature": "a0000001", "name": "A", "field": "length", "before": 4, "after": null}"#,
            r#"{"feature": "a0000001", "name": "A", "field": "width", "before": 2, "after": null}"#,
            r#"{"feature": "a0000001", "name": "A", "field": "radius", "before": null, "after": 3}"#,
            r#"{"feature": "a0000001", "name": "A", "field": "height", "before": 1, "after": 7}"#,
        ]
    );
}

#[test]
fn from_nothing_to_everything_and_back_is_every_feature_added_then_removed() {
    let features: Vec<String> = (0..3).map(|n| a_box(n, "B", 4)).collect();
    let empty = document::read(header("").as_bytes()).unwrap();
    let full = document::read(header(&features.join(", ")).as_bytes()).unwrap();
    let added = lines(&document::diff(&empty, &full).unwrap());
    let removed = lines(&document::diff(&full, &empty).unwrap());
    for n in 0..3 {
        assert_eq!(
            added[n],
            format!(
                r#"{{"feature": "{}", "name": "B", "field": null, "before": null, "after": {n}}}"#,
                feature_id(n)
            )
        );
        assert_eq!(
            removed[n],
            format!(
                r#"{{"feature": "{}", "name": "B", "field": null, "before": {n}, "after": null}}"#,
                feature_id(n)
            )
        );
    }
    assert_eq!((added.len(), removed.len()), (3, 3));
}

// --- Requests. ---

#[test]
fn requests_wrong_in_every_way_are_refused_naming_the_field() {
    let f = sessions();
    let _ = f.open(header(&a_box(0, "B", 4)).as_bytes());
    let call = |arguments: &str| -> serde_json::Value {
        match f.sessions.call("diff", arguments) {
            Call::Running(calling) => serde_json::from_str(&calling.wait().text).unwrap(),
            Call::Unknown => panic!("diff is a command"),
        }
    };
    for (arguments, field) in [
        (r#"{"session": "s1", "from": 0, "to": null}"#, "from"),
        (r#"{"session": "s1", "from": -1, "to": null}"#, "from"),
        (r#"{"session": "s1", "from": 1.0, "to": null}"#, "from"),
        (r#"{"session": "s1", "from": 1e0, "to": null}"#, "from"),
        (r#"{"session": "s1", "from": "1", "to": null}"#, "from"),
        (r#"{"session": "s1", "from": true, "to": null}"#, "from"),
        (
            r#"{"session": "s1", "from": 18446744073709551616, "to": null}"#,
            "from",
        ),
        (r#"{"session": "s1", "from": null, "to": [1]}"#, "to"),
        (r#"{"session": "s1", "from": null}"#, "to"),
        (r#"{"session": "s1", "to": null}"#, "from"),
        (
            r#"{"session": "s1", "from": null, "to": null, "model": "current"}"#,
            "model",
        ),
        (r#"{"session": "s9", "from": null, "to": null}"#, "session"),
    ] {
        let answer = call(arguments);
        assert_eq!(answer["ok"], false, "{arguments}");
        let named: Vec<&str> = answer["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["parameter"].as_str().unwrap_or("null"))
            .collect();
        assert!(named.contains(&field), "{arguments}: {answer}");
    }
    // The largest revision a request can name is read, and refused as not held.
    let answer = call(r#"{"session": "s1", "from": 18446744073709551615, "to": null}"#);
    assert_eq!(answer["ok"], false);
    assert_eq!(answer["errors"][0]["parameter"], "from");
    assert_eq!(answer["errors"][0]["candidates"], serde_json::json!(["1"]));
    // And the session runs on.
    let answer = call(r#"{"session": "s1", "from": 1, "to": null}"#);
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(
        answer["result"],
        serde_json::json!({"from": 1, "to": 1, "changes": []})
    );
}

#[test]
fn a_diff_cancelled_answers_cancelled_and_the_next_runs() {
    // A long diff (50,000 features each lengthened), cancelled by `cancel` as it runs or
    // by its handle before: either way answered (cancelled, or its result if it had
    // finished), never hung, and the session's next diff answers.
    const N: usize = 50_000;
    let from: Vec<String> = (0..N).map(|n| a_box(n, "B", 4)).collect();
    let to: Vec<String> = (0..N).map(|n| a_box(n, "B", 5)).collect();
    let f = sessions();
    let s = f.open(header(&from.join(", ")).as_bytes());
    f.write(header(&to.join(", ")).as_bytes());
    let running = f.sessions.diff(&s, Some(at(1)), None);
    running.cancel();
    let response = running
        .wait_timeout(Duration::from_secs(120))
        .unwrap_or_else(|_| panic!("the cancelled diff never answered"));
    match response.result {
        Ok(result) => assert_eq!(result.changes.len(), N),
        Err(problems) => assert_eq!(problems.errors()[0].code(), ErrorCode::Cancelled),
    }
    let next = f.diff(&s, None, None).result.unwrap();
    assert_eq!(next.changes, []);
}

// --- The file on disk as a side (Fable's decision of 2026-11-04, 6d's other half). ---

#[test]
fn a_disk_side_misspelled_or_of_another_type_is_refused_by_the_reader_naming_the_field() {
    let f = sessions();
    let s = f.open(header(&a_box(0, "B", 4)).as_bytes());
    let call = |arguments: &str| -> serde_json::Value {
        match f.sessions.call("diff", arguments) {
            Call::Running(calling) => serde_json::from_str(&calling.wait().text).unwrap(),
            Call::Unknown => panic!("diff is a command"),
        }
    };
    // Each value as the refusal says it, by hand: a string quoted, a number as written,
    // `true` by its kind.
    for (value, said) in [
        (r#""Disk""#, r#""Disk""#),
        (r#""disk ""#, r#""disk ""#),
        (r#""""#, r#""""#),
        (r#""1""#, r#""1""#),
        ("0", "0"),
        ("-1", "-1"),
    ] {
        for side in ["from", "to"] {
            let other = if side == "from" { "to" } else { "from" };
            let answer = call(&format!(
                r#"{{"session": "{s}", "{side}": {value}, "{other}": null}}"#
            ));
            assert_eq!(answer["ok"], false, "{value}");
            assert_eq!(answer["errors"].as_array().unwrap().len(), 1, "{answer}");
            let e = &answer["errors"][0];
            assert_eq!(e["code"], "invalid_input", "{value}");
            assert_eq!(e["parameter"], side, "{value}");
            assert_eq!(
                e["message"],
                format!(r#"{side} must be an integer from 1, "disk", or null, got {said}"#),
                "{value}"
            );
            // Nothing ran: the session as it was.
            assert!(
                answer["session"].is_null() || answer["session"]["revision"] == 1,
                "{answer}"
            );
        }
    }
    let answer = call(&format!(
        r#"{{"session": "{s}", "from": true, "to": null}}"#
    ));
    assert_eq!(answer["errors"][0]["parameter"], "from");
    let message = answer["errors"][0]["message"].as_str().unwrap();
    assert!(
        message.starts_with(r#"from must be an integer from 1, "disk", or null, got "#),
        "{message}"
    );
    // The word itself reads, both sides, and the session runs on; spelled with an escape,
    // it's the same string, so the same word.
    for from in [r#""disk""#, r#""\u0064isk""#] {
        let answer = call(&format!(
            r#"{{"session": "{s}", "from": {from}, "to": "disk"}}"#
        ));
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(
            answer["result"],
            serde_json::json!({"from": "disk", "to": "disk", "changes": []})
        );
    }
}

impl Fixture {
    fn diff_disk(&self, s: &SessionId, from: Option<Named>) -> Response<DiffAnswer> {
        self.diff_of(s, from, Some(Named::Disk))
    }
}

#[test]
fn a_disk_side_past_what_silico_reads_is_refused_too_long_naming_it() {
    let f = sessions();
    let good = header(&a_box(0, "B", 4));
    let s = f.open(good.as_bytes());
    let past = vec![b' '; silico_core::document::json::MAX_INPUT_LEN + 1];
    let shown = silico_core::command::quoted(f.path().to_str().unwrap());
    let too_long = format!(
        "{shown} is more than the {} bytes Silico reads of a document",
        silico_core::document::json::MAX_INPUT_LEN
    );
    // Inside a transaction: the read finds it too long, refused naming `to`, nothing held
    // moved.
    answer(f.sessions.begin(&s)).result.unwrap();
    f.write(&past);
    let refused = f.diff_disk(&s, Some(at(1)));
    assert_eq!(refused.session.as_ref().unwrap().revision.number(), 1);
    let problems = refused.result.unwrap_err();
    assert_eq!(problems.total(), 1);
    let e = &problems.errors()[0];
    assert_eq!(
        (e.code(), e.parameter(), e.message()),
        (ErrorCode::InvalidInput, Some("to"), too_long.as_str())
    );
    // Rolled back, outside a transaction: the read before the command refuses it, naming
    // `to` too.
    answer(f.sessions.rollback(&s)).result.unwrap();
    let refused = f.diff_disk(&s, Some(at(1))).result.unwrap_err();
    let e = &refused.errors()[0];
    assert_eq!(
        (e.code(), e.parameter(), e.message()),
        (ErrorCode::InvalidInput, Some("to"), too_long.as_str())
    );
    // Exactly what Silico reads of a file: read, inside a transaction, and refused as a side
    // that doesn't read as a document, the reader's message the cause.
    let most = vec![b' '; silico_core::document::json::MAX_INPUT_LEN];
    let reader = document::read(&most).unwrap_err();
    // Begun on the file put back (`begin` reads it first, outside a transaction).
    f.write(good.as_bytes());
    answer(f.sessions.begin(&s)).result.unwrap();
    f.write(&most);
    let refused = f.diff_disk(&s, None).result.unwrap_err();
    assert_eq!(refused.total(), 1);
    let e = &refused.errors()[0];
    assert_eq!(
        (e.code(), e.parameter(), e.cause()),
        (ErrorCode::InvalidInput, Some("to"), Some(reader.message()))
    );
    answer(f.sessions.rollback(&s)).result.unwrap();
    // Put back: read again.
    f.write(good.as_bytes());
    assert_eq!(f.diff_disk(&s, None).result.unwrap().changes, []);
}

#[test]
fn a_disk_side_of_just_under_sixteen_mib_is_diffed_inside_a_transaction() {
    let f = sessions();
    let s = f.open(near_the_limit(1000, None).as_bytes());
    answer(f.sessions.begin(&s)).result.unwrap();
    f.write(near_the_limit(1000, Some(617)).as_bytes());
    let diffed = f.diff_disk(&s, Some(at(1)));
    assert!(!diffed.session.as_ref().unwrap().reloaded);
    let result = diffed.result.unwrap();
    assert_eq!((result.from, result.to), (at(1), Named::Disk));
    assert_eq!(result.changes.len(), 1);
    let change = &result.changes[0];
    assert_eq!(change.feature.as_str(), feature_id(617));
    assert_eq!(change.field.as_deref(), Some("length"));
}

#[test]
fn a_disk_side_that_isnt_a_regular_file_is_refused_naming_it() {
    // A directory put at the file's path, inside a transaction: the read's `io_failed`,
    // naming `to`, advising a regular file; the session goes on once the file's back.
    let f = sessions();
    let good = header(&a_box(0, "B", 4));
    let s = f.open(good.as_bytes());
    answer(f.sessions.begin(&s)).result.unwrap();
    std::fs::remove_file(f.path()).unwrap();
    std::fs::create_dir(f.path()).unwrap();
    let refused = f.diff_disk(&s, Some(at(1))).result.unwrap_err();
    assert_eq!(refused.total(), 1);
    let e = &refused.errors()[0];
    assert_eq!((e.code(), e.parameter()), (ErrorCode::IoFailed, Some("to")));
    let suggestion = e.suggestion().unwrap();
    assert!(
        suggestion.starts_with("make the file at ")
            && suggestion.ends_with(" a regular file this user may read, then read \"disk\" again"),
        "{suggestion}"
    );
    std::fs::remove_dir(f.path()).unwrap();
    f.write(good.as_bytes());
    assert_eq!(f.diff_disk(&s, Some(at(1))).result.unwrap().changes, []);
}
