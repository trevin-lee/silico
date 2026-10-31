//! M5.1's `suppress_feature` on the wire (docs/design/m5.0-app.md, section 7's
//! `suppress_feature`, A10, and the test plan's *Suppress*). The design's examples of it, byte
//! for byte: each file the design's own block; each request read, written back to it, and
//! held to its snapshot, every field it gives required and any other refused; and each
//! result the session's own, made by running the command from the request's text on the M4.9
//! bracket (tests/common/bracket.rs) on the fake kernel, inside a transaction at revision 12,
//! as the design sets them up. Then the test plan's cases that are the session's: what a
//! commit writes (a suppressed feature's fingerprint dropped, the features using it
//! refingerprinted, a suppression undone giving the file back byte for byte), a hand edit
//! and `format` giving the command's file, drift only where a hand edit left it, the reads
//! that refuse a suppressed feature, `set_field`'s `suppressed`, undo, the timeline's
//! `suppressed` health, and the call by name. Each expected value is the design's text or
//! worked out by hand from its rules; the volumes are the real kernel's, in silico-cli's
//! tests/bracket.rs.

use std::path::PathBuf;
use std::sync::Arc;

use silico_core::cache::Cache;
use silico_core::command::wire::edits::SuppressFeatureRequest;
use silico_core::command::wire::{Layout, ToWire, write};
use silico_core::command::{
    CheckModel, ModelChoice, Problems, Response, Revision, Running, SessionId, Sessions, Suppressed,
};
use silico_core::document::json::{Node, read};
use silico_core::document::{self, FeatureId};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::sketch::SketchSolver;
use silico_core::timeline::Health;
use silico_core::{Error, ErrorCode};
use tempfile::TempDir;

use super::bracket::*;

const HANG: std::time::Duration = std::time::Duration::from_secs(60);

fn contract(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/contract")
        .join(name)
}

fn committed(name: &str) -> String {
    let path = contract(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn example(name: &str) -> String {
    committed(&format!("commands/examples/suppress_feature/{name}.json"))
}

/// The M5.0 design's JSON code blocks between the headings `from` and `to`, in order.
fn design(from: &str, to: &str) -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/design/m5.0-app.md");
    let doc = std::fs::read_to_string(&path).unwrap();
    let section = &doc[doc.find(from).unwrap()..doc.find(to).unwrap()];
    section
        .split("```json\n")
        .skip(1)
        .map(|block| block.split("\n```").next().unwrap().to_owned())
        .collect()
}

fn written<T: ToWire>(value: &T) -> String {
    write(&value.to_wire(), Layout::Canonical).unwrap() + "\n"
}

fn parsed(text: &str) -> Node {
    read(text.as_bytes()).unwrap()
}

fn schema_of(part: &str) -> serde_json::Value {
    let schema: serde_json::Value =
        serde_json::from_str(&committed("commands/suppress_feature.json")).unwrap();
    schema[part].clone()
}

fn error_schema() -> serde_json::Value {
    let envelope: serde_json::Value =
        serde_json::from_str(&committed("commands/envelope.json")).unwrap();
    serde_json::json!({"$defs": envelope["$defs"], "$ref": "#/$defs/Error"})
}

fn holds(schema: &serde_json::Value, text: &str, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{what}: {errors:?}");
}

fn fails(schema: &serde_json::Value, text: &str, what: &str) {
    let validator = jsonschema::validator_for(schema).unwrap();
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    assert!(!validator.is_valid(&value), "{what} held");
}

fn revision(n: u64) -> Revision {
    let mut r = Revision::OPENED;
    for _ in 1..n {
        r = r.next().unwrap();
    }
    r
}

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

fn ids(list: &[&str]) -> Vec<FeatureId> {
    list.iter().map(|i| id(i)).collect()
}

fn s1() -> SessionId {
    SessionId::parse("s1").unwrap()
}

fn answer<T>(running: Running<T>) -> Response<T> {
    match running.wait_timeout(HANG) {
        Ok(response) => response,
        Err(_) => panic!("the command didn't answer in {HANG:?}"),
    }
}

fn the_error<T: std::fmt::Debug>(response: &Response<T>) -> Error {
    let problems: &Problems = response.result.as_ref().unwrap_err();
    assert_eq!(problems.total(), 1, "{:?}", problems.errors());
    problems.errors()[0].clone()
}

fn text_of(features: &[&str]) -> String {
    let text = format!(
        r#"{{"silico": 1, "units": {{"length": "mm", "resolution": "1nm"}}, "naming": 5, "features": [{}]}}"#,
        features.join(", ")
    );
    document::write(&document::read(text.as_bytes()).unwrap()).unwrap()
}

fn bracket() -> String {
    text_of(&[
        S0000001, P0000001, S0000002, H0000001, S0000003, H0000002, F0000001,
    ])
}

struct Fixture {
    dir: TempDir,
    sessions: Sessions<Arc<FakeKernel>>,
    id: SessionId,
}

fn opened(text: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sketches = FakeSketches::new();
    let sessions = Sessions::with_stand_ins(
        kernel,
        move |cache| Cache::with_secret(cache, &secret),
        move || Ok(Box::new(sketches.clone()) as Box<dyn SketchSolver>),
    );
    let path = dir.path().join("bracket.silico");
    std::fs::write(&path, text).unwrap();
    let id = answer(sessions.open(&path, false)).session.unwrap().id;
    assert_eq!(id, s1());
    Fixture { dir, sessions, id }
}

/// The bracket, saved by `format` with the fake kernel's fingerprints.
fn fingerprinted() -> String {
    let f = opened(&bracket());
    let formatted = answer(f.sessions.format(&f.id, None, true)).result.unwrap();
    assert!(formatted.written);
    f.file()
}

impl Fixture {
    fn path(&self) -> PathBuf {
        self.dir.path().join("bracket.silico")
    }

    fn file(&self) -> String {
        std::fs::read_to_string(self.path()).unwrap()
    }

    fn revision(&self) -> Revision {
        self.sessions.status(&self.id).session.unwrap().revision
    }

    fn begin(&self) {
        answer(self.sessions.begin(&self.id)).result.unwrap();
    }

    fn inside_at(&self, n: u64) {
        while self.revision() < revision(n) {
            self.begin();
            answer(self.sessions.rollback(&self.id)).result.unwrap();
        }
        assert_eq!(self.revision(), revision(n));
        self.begin();
    }

    fn suppress(&self, feature: &str, suppressed: bool) -> Response<Suppressed> {
        answer(
            self.sessions
                .suppress_feature(&self.id, self.revision(), id(feature), suppressed),
        )
    }

    /// The session's document as its text reads.
    fn document(&self) -> document::Document {
        let text = answer(self.sessions.text(&self.id, None))
            .result
            .unwrap()
            .text;
        document::read(text.as_bytes()).unwrap()
    }

    /// The features of the session's document that hold a fingerprint.
    fn fingerprinted(&self) -> Vec<FeatureId> {
        self.document()
            .features
            .iter()
            .filter(|f| f.fingerprint.is_some())
            .map(|f| f.id.clone())
            .collect()
    }

    /// The features a fresh check of the current model finds drift on, in timeline order;
    /// and how many problems it has.
    fn drift(&self) -> (Vec<FeatureId>, u64) {
        let checked = answer(
            self.sessions
                .check(&self.id, CheckModel::Current { fresh: true }),
        )
        .result
        .unwrap();
        let drift = checked
            .problems
            .errors()
            .iter()
            .filter(|e| e.code() == ErrorCode::Drift)
            .map(|e| id(e.feature().unwrap()))
            .collect();
        (drift, checked.problems.total())
    }
}

/// The examples' names, in the design's order.
const NAMES: [&str; 6] = [
    "request",
    "result",
    "request-sketch",
    "result-sketch",
    "request-unsuppress",
    "result-unsuppress",
];

#[test]
fn the_examples_are_the_m5_designs_own_text_and_no_other() {
    let blocks = design(
        "### `suppress_feature` (M5.1; Fable)",
        "### `timeline` (M5.1)",
    );
    assert_eq!(blocks.len(), NAMES.len());
    for (block, name) in blocks.iter().zip(NAMES) {
        assert_eq!(format!("{block}\n"), example(name), "{name}");
    }
    let mut files: Vec<String> = std::fs::read_dir(contract("commands/examples/suppress_feature"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    files.sort();
    let mut want: Vec<String> = NAMES.iter().map(|n| format!("{n}.json")).collect();
    want.sort();
    assert_eq!(files, want);
}

#[test]
fn the_designs_requests_are_read_written_back_byte_for_byte_and_hold_to_their_schema() {
    for (name, feature, at, suppressed) in [
        ("request", "h0000001", 12, true),
        ("request-sketch", "s0000002", 12, true),
        ("request-unsuppress", "h0000001", 13, false),
    ] {
        let text = example(name);
        let request = SuppressFeatureRequest::read(&parsed(&text)).unwrap();
        assert_eq!(
            request,
            SuppressFeatureRequest {
                session: s1(),
                revision: revision(at),
                feature: id(feature),
                suppressed,
            },
            "{name}"
        );
        assert_eq!(written(&request), text, "{name}");
        holds(&schema_of("request"), &text, name);
    }
}

/// Each refusal's parameter and message, in order.
fn said(problems: &Problems) -> Vec<(String, String)> {
    problems
        .errors()
        .iter()
        .map(|e| {
            (
                e.parameter().unwrap_or("").to_owned(),
                e.message().to_owned(),
            )
        })
        .collect()
}

fn without(text: &str, key: &str) -> String {
    let mut value: serde_json::Value = serde_json::from_str(text).unwrap();
    assert!(
        value.as_object_mut().unwrap().remove(key).is_some(),
        "{key}"
    );
    value.to_string()
}

fn with(text: &str, key: &str, value: &str) -> String {
    let mut object: serde_json::Value = serde_json::from_str(text).unwrap();
    object[key] = serde_json::from_str(value).unwrap();
    object.to_string()
}

#[test]
fn the_request_requires_each_of_its_fields_and_refuses_any_other() {
    let text = example("request");
    for key in ["session", "revision", "feature", "suppressed"] {
        let refused = SuppressFeatureRequest::read(&parsed(&without(&text, key))).unwrap_err();
        assert_eq!(
            said(&refused),
            [(key.to_owned(), format!("{key} is missing"))]
        );
        fails(&schema_of("request"), &without(&text, key), key);
    }
    let extra = with(&text, "extra", "1");
    assert_eq!(
        said(&SuppressFeatureRequest::read(&parsed(&extra)).unwrap_err()),
        [(
            "extra".to_owned(),
            "extra isn't a field of this request".to_owned()
        )]
    );
    fails(&schema_of("request"), &extra, "an extra field");
    // `suppressed` is a boolean, never a string or null; `feature` an ID.
    for bad in [r#""true""#, "null", "1"] {
        let bad = with(&text, "suppressed", bad);
        let refused = SuppressFeatureRequest::read(&parsed(&bad)).unwrap_err();
        assert_eq!(refused.errors()[0].parameter(), Some("suppressed"), "{bad}");
        fails(&schema_of("request"), &bad, &bad);
    }
    let refused =
        SuppressFeatureRequest::read(&parsed(&with(&text, "feature", "null"))).unwrap_err();
    assert_eq!(refused.errors()[0].parameter(), Some("feature"));
}

#[test]
fn the_designs_results_are_the_sessions_own() {
    let f = opened(&bracket());
    f.inside_at(12);
    // h0000001 alone suppressed.
    let request = SuppressFeatureRequest::read(&parsed(&example("request"))).unwrap();
    let done = answer(f.sessions.suppress_feature(
        &request.session,
        request.revision,
        request.feature,
        request.suppressed,
    ));
    let text = written(done.result.as_ref().unwrap());
    assert_eq!(text, example("result"));
    holds(&schema_of("result"), &text, "suppressed");
    assert_eq!(f.revision(), revision(13));

    // The sketch: h0000001 with it. Then h0000001 unsuppressed, at 13: its sketch with it.
    let f = opened(&bracket());
    f.inside_at(12);
    for (request, result) in [
        ("request-sketch", "result-sketch"),
        ("request-unsuppress", "result-unsuppress"),
    ] {
        let request = SuppressFeatureRequest::read(&parsed(&example(request))).unwrap();
        assert_eq!(f.revision(), request.revision);
        let done = answer(f.sessions.suppress_feature(
            &request.session,
            request.revision,
            request.feature,
            request.suppressed,
        ));
        let text = written(done.result.as_ref().unwrap());
        assert_eq!(text, example(result), "{result}");
        holds(&schema_of("result"), &text, result);
    }
    // Back as it began: suppressed, then unsuppressed with its sketch.
    assert!(
        f.document()
            .canonically_eq(&document::read(bracket().as_bytes()).unwrap())
    );
}

#[test]
fn an_edit_that_changes_nothing_makes_no_revision_and_refusals_hold_to_the_error_contract() {
    let error = error_schema();
    let f = opened(&bracket());
    f.inside_at(12);
    f.suppress("h0000001", true).result.unwrap();
    let at = f.revision();
    // Suppressed already, and unsuppressing one that isn't: both lists empty.
    for (feature, suppressed) in [("h0000001", true), ("h0000002", false)] {
        let done = f.suppress(feature, suppressed);
        let result = done.result.as_ref().unwrap();
        assert!(!result.changed && result.suppressed.is_empty() && result.unsuppressed.is_empty());
        assert!(result.commit.is_none());
        holds(&schema_of("result"), &written(result), feature);
        assert_eq!(f.revision(), at);
    }
    // A feature the document hasn't, naming `feature`.
    let missing = the_error(&f.suppress("z0000001", true));
    assert_eq!(
        (missing.code(), missing.parameter(), missing.message()),
        (
            ErrorCode::InvalidInput,
            Some("feature"),
            "the document has no feature z0000001"
        )
    );
    holds(&error, &written(&missing), "no feature");
    // A stale revision: a conflict.
    let stale = answer(
        f.sessions
            .suppress_feature(&f.id, revision(12), id("h0000002"), true),
    );
    let conflict = the_error(&stale);
    assert_eq!(conflict.code(), ErrorCode::Conflict);
    holds(&error, &written(&conflict), "stale");
    assert_eq!(f.revision(), at);
}

#[test]
fn suppressing_the_first_hole_keeps_its_fingerprint_and_refingerprints_its_users_and_back() {
    // Outside a transaction: each edit its own commit.
    let original = fingerprinted();
    let f = opened(&original);
    let done = f.suppress("h0000001", true);
    let result = done.result.unwrap();
    assert_eq!(result.suppressed, ids(&["h0000001"]));
    let commit = result.commit.unwrap();
    assert!(commit.written);
    // The test plan's: h0000002 and f0000001 written fresh (they build on the pad now), and
    // h0000001's carried as stored, never checked or rewritten (A10, as amended).
    assert_eq!(commit.refingerprinted, ids(&["h0000002", "f0000001"]));
    assert_eq!(
        f.fingerprinted(),
        ids(&["p0000001", "h0000001", "h0000002", "f0000001"])
    );
    let stored = document::read(original.as_bytes()).unwrap();
    assert_eq!(
        document::read(f.file().as_bytes()).unwrap().features[3].fingerprint,
        stored.features[3].fingerprint
    );
    let saved = document::read(f.file().as_bytes()).unwrap();
    assert!(saved.features[3].suppressed);
    // A fresh check: clean, no drift, no problem.
    assert_eq!(f.drift(), (Vec::new(), 0));
    // The timeline: h0000001 suppressed, the fillet the one body.
    let timeline = answer(f.sessions.timeline(&f.id, ModelChoice::Current))
        .result
        .unwrap();
    let features = timeline.features.unwrap();
    let entry = &features[3];
    assert_eq!((entry.health, entry.suppressed), (Health::Suppressed, true));
    assert_eq!((entry.problems, entry.warnings), (0, 0));
    for (at, e) in features.iter().enumerate() {
        if at != 3 {
            assert_eq!(
                (e.health, e.suppressed),
                (Health::Healthy, false),
                "{}",
                e.id
            );
        }
    }
    // Its inputs as the text names them: suppression rewrites no reference.
    assert_eq!(features[5].inputs, ids(&["h0000001", "s0000003"]));
    assert_eq!(timeline.bodies, ids(&["f0000001"]));

    // Unsuppressed and committed: the file byte for byte as it was. h0000001 is written
    // fresh, the same as it was stored, so only its users' are listed as changed.
    let back = f.suppress("h0000001", false).result.unwrap();
    assert_eq!(back.unsuppressed, ids(&["h0000001"]));
    assert_eq!(
        back.commit.unwrap().refingerprinted,
        ids(&["h0000002", "f0000001"])
    );
    assert_eq!(f.file(), original);
}

#[test]
fn a_hand_edit_and_format_give_the_file_the_command_and_its_commit_give() {
    let original = fingerprinted();
    let by_command = {
        let f = opened(&original);
        f.suppress("h0000001", true).result.unwrap();
        f.file()
    };
    // By hand: `"suppressed": true` added after h0000001's name, its fingerprint left; then
    // format, as `silico fmt` runs it.
    let hand = original.replacen(
        r#""name": "Base holes",
      "base": "p0000001""#,
        r#""name": "Base holes",
      "suppressed": true,
      "base": "p0000001""#,
        1,
    );
    assert_ne!(hand, original, "the edit by hand found its place");
    let f = opened(&hand);
    let formatted = answer(f.sessions.format(&f.id, None, true)).result.unwrap();
    assert!(formatted.written);
    assert_eq!(f.file(), by_command);
}

#[test]
fn unsuppressing_by_command_after_a_hand_edit_reports_drift_only_where_the_hand_edit_is() {
    // Hand-edited: h0000001 suppressed with its fingerprint still, and the pad's length
    // changed, never formatted.
    let original = fingerprinted();
    let hand = original
        .replacen(
            r#""name": "Base holes",
      "base": "p0000001""#,
            r#""name": "Base holes",
      "suppressed": true,
      "base": "p0000001""#,
            1,
        )
        .replacen(
            r#""length": 40, "reversed": true"#,
            r#""length": 41, "reversed": true"#,
            1,
        );
    assert_eq!(hand.matches("suppressed").count(), 1);
    assert_eq!(hand.matches(r#""length": 41"#).count(), 1);
    let f = opened(&hand);
    f.begin();
    let done = f.suppress("h0000001", false).result.unwrap();
    assert_eq!(done.unsuppressed, ids(&["h0000001"]));
    // Drift on exactly the pad (a hand edit, unreached), none on what the unsuppression
    // reached: h0000001, h0000002, and f0000001.
    assert_eq!(f.drift().0, ids(&["p0000001"]));
    let commit = answer(f.sessions.commit(&f.id)).result.unwrap();
    assert!(commit.written);
    // The commit writes h0000001's fingerprint fresh, and those of what it reached.
    assert_eq!(
        commit.refingerprinted,
        ids(&["h0000001", "h0000002", "f0000001"])
    );
    let saved = document::read(f.file().as_bytes()).unwrap();
    let stored = document::read(hand.as_bytes()).unwrap();
    assert!(saved.features[3].fingerprint.is_some());
    assert_ne!(
        saved.features[3].fingerprint,
        stored.features[3].fingerprint
    );
    // The pad's stale fingerprint is left as stored, its drift still said.
    assert_eq!(
        saved.features[1].fingerprint,
        stored.features[1].fingerprint
    );
}

#[test]
fn the_root_suppressed_keeps_its_fingerprints_and_the_fillet_unsuppressed_takes_back_the_pad() {
    let f = opened(&fingerprinted());
    let done = f.suppress("p0000001", true).result.unwrap();
    assert_eq!(
        done.suppressed,
        ids(&["p0000001", "h0000001", "h0000002", "f0000001"])
    );
    // A10, as amended: the four carry their stored fingerprints; the sketches never had any.
    assert_eq!(
        f.fingerprinted(),
        ids(&["p0000001", "h0000001", "h0000002", "f0000001"])
    );
    let published = answer(f.sessions.timeline(&f.id, ModelChoice::Current))
        .result
        .unwrap();
    assert!(published.bodies.is_empty());
    // Unsuppressing the fillet: the pad with it, no drift, both written fresh: the pad as
    // stored, the fillet anew (it builds on the pad through both pockets now).
    let back = f.suppress("f0000001", false).result.unwrap();
    assert_eq!(back.unsuppressed, ids(&["p0000001", "f0000001"]));
    assert_eq!(back.commit.unwrap().refingerprinted, ids(&["f0000001"]));
    assert_eq!(f.drift(), (Vec::new(), 0));
    assert_eq!(
        f.fingerprinted(),
        ids(&["p0000001", "h0000001", "h0000002", "f0000001"])
    );
}

#[test]
fn a_suppression_and_its_undoing_outside_a_transaction_leave_another_suppressed_as_it_was() {
    let f = opened(&fingerprinted());
    f.suppress("h0000002", true).result.unwrap();
    let before = f.file();
    assert!(f.fingerprinted().contains(&id("h0000002")));
    f.suppress("h0000001", true).result.unwrap();
    f.suppress("h0000001", false).result.unwrap();
    assert_eq!(f.file(), before);
}

#[test]
fn an_edit_of_a_suppressed_features_extent_committed_carries_its_fingerprint_as_stored() {
    let original = fingerprinted();
    let f = opened(&original);
    f.suppress("h0000001", true).result.unwrap();
    let set = answer(f.sessions.set_field(
        &f.id,
        f.revision(),
        id("h0000001"),
        "extent".to_owned(),
        parsed(r#"{"type": "one_side", "length": 3, "reversed": true}"#),
    ));
    set.result.unwrap();
    let saved = document::read(f.file().as_bytes()).unwrap();
    assert!(saved.features[3].suppressed);
    assert_eq!(
        saved.features[3].fingerprint,
        document::read(original.as_bytes()).unwrap().features[3].fingerprint
    );
}

/// The golden plate's first three features, as `tests/golden/plate-with-holes.silico`
/// holds them (read only: built here), its cut, c0000001, last, so nothing uses it.
const PLATE: [&str; 3] = [
    r#"{"id": "pppppppp", "type": "box", "v": 1, "name": "Plate", "position": [0, 0, 0], "length": 60, "width": 30, "height": 5, "fingerprint": {"volume": 9000, "area": 4500, "bbox": [0, 0, 0, 60, 30, 5], "faces": 6, "edges": 12, "vertices": 8}}"#,
    r#"{"id": "y0000001", "type": "cylinder", "v": 1, "name": "Pin 1", "position": [15, 15, -1], "radius": 1.5, "height": 7, "fingerprint": {"volume": 49.480084, "area": 80.110613, "bbox": [13.5, 13.5, -1, 16.5, 16.5, 6], "faces": 3, "edges": 3, "vertices": 2}}"#,
    r#"{"id": "c0000001", "type": "cut", "v": 1, "name": "Hole 1", "base": "pppppppp", "tool": "y0000001", "fingerprint": {"volume": 8964.657083, "area": 4532.986723, "bbox": [0, 0, 0, 60, 30, 5], "faces": 7, "edges": 15, "vertices": 10}}"#,
];

/// The lines `after` has that `before` hasn't, and those `before` has that `after` hasn't,
/// each with its place, by a line-for-line walk that takes one line inserted or changed.
fn lines_changed(before: &str, after: &str) -> (Vec<String>, Vec<String>) {
    let (b, a): (Vec<&str>, Vec<&str>) = (before.lines().collect(), after.lines().collect());
    let start = b.iter().zip(&a).take_while(|(x, y)| x == y).count();
    let end = b
        .iter()
        .rev()
        .zip(a.iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
        .min(b.len().min(a.len()) - start);
    (
        a[start..a.len() - end]
            .iter()
            .map(|l| (*l).to_owned())
            .collect(),
        b[start..b.len() - end]
            .iter()
            .map(|l| (*l).to_owned())
            .collect(),
    )
}

#[test]
fn suppressing_the_plates_cut_adds_one_line_and_nothing_else() {
    // Main's: the file otherwise byte for byte, the cut's fingerprint carried as stored (a
    // drop would take its fingerprint's line too, and fold the cut onto one line, D7).
    let original = text_of(&PLATE);
    let f = opened(&original);
    let done = f.suppress("c0000001", true).result.unwrap();
    assert_eq!(done.suppressed, ids(&["c0000001"]));
    assert!(done.commit.unwrap().refingerprinted.is_empty());
    let (added, removed) = lines_changed(&original, &f.file());
    assert_eq!(added, [r#"      "suppressed": true,"#]);
    assert!(removed.is_empty(), "{removed:?}");
    // In its place: right after the cut's name.
    assert!(
        f.file().contains(
            "      \"name\": \"Hole 1\",\n      \"suppressed\": true,\n      \"base\": \"pppppppp\""
        ),
        "{}",
        f.file()
    );
}

#[test]
fn editing_a_suppressed_boxs_width_changes_that_line_alone() {
    // Main's: a fingerprinted box suppressed, then its width set: the width's line is the
    // one that changes; its fingerprint, stored, stays (a drop would remove its line).
    let f = opened(&text_of(&[PLATE[0]]));
    f.suppress("pppppppp", true).result.unwrap();
    let suppressed = f.file();
    let set = answer(f.sessions.set_field(
        &f.id,
        f.revision(),
        id("pppppppp"),
        "width".to_owned(),
        parsed("31"),
    ));
    assert!(set.result.unwrap().changed);
    let (added, removed) = lines_changed(&suppressed, &f.file());
    assert_eq!(added, [r#"      "width": 31,"#]);
    assert_eq!(removed, [r#"      "width": 30,"#]);
}

#[test]
fn a_stale_fingerprint_carried_while_suppressed_is_drift_once_unsuppressed_by_hand() {
    // Main's: h0000001 suppressed and its extent changed, committed (its fingerprint, the
    // old extent's, carried as stored); then its `suppressed` line removed by hand: a check
    // reports its drift loudly. Were the fingerprint dropped while suppressed, there'd be
    // nothing to hold the rebuild to, and no drift.
    let f = opened(&fingerprinted());
    f.suppress("h0000001", true).result.unwrap();
    answer(f.sessions.set_field(
        &f.id,
        f.revision(),
        id("h0000001"),
        "extent".to_owned(),
        parsed(r#"{"type": "one_side", "length": 3, "reversed": true}"#),
    ))
    .result
    .unwrap();
    let saved = f.file();
    let hand = saved.replacen("      \"suppressed\": true,\n", "", 1);
    assert_ne!(hand, saved);
    answer(f.sessions.close(&f.id)).result.unwrap();
    let g = opened(&hand);
    let (drift, _) = g.drift();
    assert!(drift.contains(&id("h0000001")), "{drift:?}");
}

#[test]
fn a_suppression_removed_by_hand_from_a_file_never_saved_since_is_drift() {
    // The bracket fingerprinted; then by hand, h0000001 suppressed (its fingerprint kept),
    // its extent changed while suppressed, and `suppressed` removed again, never saved: its
    // stored fingerprint is the old extent's, so its rebuild differs, as any hand edit's.
    let original = fingerprinted();
    let hand = original.replacen(
        r#""extent": {"type": "through_all", "reversed": true},
      "fingerprint""#,
        r#""extent": {"type": "one_side", "length": 3, "reversed": true},
      "fingerprint""#,
        1,
    );
    assert_ne!(hand, original);
    let f = opened(&hand);
    let (drift, _) = f.drift();
    assert!(drift.contains(&id("h0000001")), "{drift:?}");
}

#[test]
fn a_read_of_a_suppressed_features_solid_is_refused_naming_feature() {
    let f = opened(&bracket());
    f.suppress("h0000001", true).result.unwrap();
    let listed =
        answer(
            f.sessions
                .elements(&f.id, ModelChoice::Current, Some(id("h0000001")), None, 10),
        );
    let refused = the_error(&listed);
    assert_eq!(
        (
            refused.code(),
            refused.parameter(),
            refused.feature(),
            refused.message()
        ),
        (
            ErrorCode::InvalidInput,
            Some("feature"),
            Some("h0000001"),
            "h0000001 is suppressed, so it has no solid"
        )
    );
    holds(&error_schema(), &written(&refused), "elements");
    // Its consumer's solid is read as usual.
    answer(
        f.sessions
            .elements(&f.id, ModelChoice::Current, Some(id("h0000002")), None, 10),
    )
    .result
    .unwrap();
}

#[test]
fn a_read_of_a_suppressed_sketch_is_refused_naming_sketch_current_and_published() {
    // M5.0's `sketch`: "a feature that isn't a sketch, naming `sketch`; a suppressed one,
    // likewise", worded as the read of a suppressed feature's solid above. A suppressed
    // sketch is never solved, so the read has no solve to give, which is the user's doing,
    // never `internal`.
    let f = opened(&bracket());
    let read = |model| answer(f.sessions.sketch(&f.id, model, id("s0000002")));
    let before = read(ModelChoice::Current).result.unwrap();
    // Outside a transaction the suppression is its own commit, and a check of the current
    // model waits for its rebuild, which publishes it.
    let done = f.suppress("s0000002", true).result.unwrap();
    assert_eq!(done.suppressed, ids(&["s0000002", "h0000001"]));
    let current = CheckModel::Current { fresh: false };
    answer(f.sessions.check(&f.id, current)).result.unwrap();
    for model in [ModelChoice::Current, ModelChoice::Published] {
        let refused = read(model);
        assert_eq!(
            refused.model.as_ref().unwrap().revision,
            f.revision(),
            "{model:?}"
        );
        let error = the_error(&refused);
        assert_eq!(
            (
                error.code(),
                error.parameter(),
                error.feature(),
                error.message(),
                error.suggestion(),
                error.candidates(),
            ),
            (
                ErrorCode::InvalidInput,
                Some("sketch"),
                Some("s0000002"),
                "s0000002 is suppressed, so it isn't solved",
                Some(
                    "unsuppress s0000002 first (suppress_feature with suppressed false), or \
                     name a sketch that isn't suppressed"
                ),
                &[][..],
            ),
            "{model:?}"
        );
        holds(&error_schema(), &written(&error), "sketch");
    }
    // The bracket's other sketches read as usual.
    for sketch in ["s0000001", "s0000003"] {
        answer(f.sessions.sketch(&f.id, ModelChoice::Current, id(sketch)))
            .result
            .unwrap();
    }
    // Unsuppressed (alone: its pocket stays suppressed, as a parent's unsuppression leaves
    // its children), it reads as it did before, from either model.
    let done = f.suppress("s0000002", false).result.unwrap();
    assert_eq!(done.unsuppressed, ids(&["s0000002"]));
    answer(f.sessions.check(&f.id, current)).result.unwrap();
    for model in [ModelChoice::Current, ModelChoice::Published] {
        let again = read(model);
        assert_eq!(again.model.as_ref().unwrap().revision, f.revision());
        assert_eq!(again.result.unwrap(), before, "{model:?}");
    }
}

#[test]
fn set_field_takes_suppressed_true_and_false_and_the_gate_refuses_a_dependent_left() {
    let f = opened(&bracket());
    f.begin();
    let set = |field_value: &str, feature: &str| {
        answer(f.sessions.set_field(
            &f.id,
            f.revision(),
            id(feature),
            "suppressed".to_owned(),
            parsed(field_value),
        ))
    };
    // true sets it, as the command would for a feature with no dependents.
    let on = set("true", "h0000001").result.unwrap();
    assert!(on.changed);
    assert!(f.document().features[3].suppressed);
    // false removes it: the reader never sees `false`.
    let off = set("false", "h0000001").result.unwrap();
    assert!(off.changed);
    assert!(!f.document().features[3].suppressed);
    let text = answer(f.sessions.text(&f.id, None)).result.unwrap().text;
    assert!(!text.contains("suppressed"));
    // false on one that isn't changes nothing.
    assert!(!set("false", "h0000001").result.unwrap().changed);
    // A sketch set suppressed by hand, its pocket not: the gate refuses, naming both.
    let refused = the_error(&set("true", "s0000002"));
    assert_eq!(refused.feature(), Some("h0000001"));
    assert_eq!(refused.parameter(), Some("sketch"));
    assert!(
        refused.message().contains("s0000002"),
        "{}",
        refused.message()
    );
}

#[test]
fn undo_takes_a_suppression_back_byte_for_byte() {
    let f = opened(&bracket());
    f.begin();
    let before = answer(f.sessions.text(&f.id, None)).result.unwrap().text;
    f.suppress("p0000001", true).result.unwrap();
    let after = answer(f.sessions.text(&f.id, None)).result.unwrap().text;
    assert_ne!(before, after);
    answer(f.sessions.undo(&f.id, f.revision(), None))
        .result
        .unwrap();
    assert_eq!(
        answer(f.sessions.text(&f.id, None)).result.unwrap().text,
        before
    );
    answer(f.sessions.redo(&f.id, f.revision(), None))
        .result
        .unwrap();
    assert_eq!(
        answer(f.sessions.text(&f.id, None)).result.unwrap().text,
        after
    );
}

#[test]
fn the_outline_says_suppressed_before_the_type_and_its_bodies_pass_the_base_through() {
    let f = opened(&bracket());
    f.suppress("h0000001", true).result.unwrap();
    let outline = answer(f.sessions.outline(&f.id)).result.unwrap().outline;
    assert!(
        outline
            .contains(r#"h0000001 "Base holes" suppressed pocket base p0000001, sketch s0000002"#),
        "{outline}"
    );
    assert!(outline.ends_with("bodies f0000001\n"), "{outline}");
    f.suppress("p0000001", true).result.unwrap();
    let outline = answer(f.sessions.outline(&f.id)).result.unwrap().outline;
    assert!(outline.ends_with("bodies none\n"), "{outline}");
}

#[test]
fn suppress_feature_by_name_answers_as_the_typed_method() {
    use silico_core::command::Call;
    use silico_core::command::wire::response_text;
    let typed = opened(&bracket());
    typed.inside_at(12);
    let by_name = opened(&bracket());
    by_name.inside_at(12);
    for request in ["request-sketch", "request-unsuppress"] {
        let text = example(request);
        let r = SuppressFeatureRequest::read(&parsed(&text)).unwrap();
        let direct = answer(typed.sessions.suppress_feature(
            &r.session,
            r.revision,
            r.feature,
            r.suppressed,
        ));
        let Call::Running(calling) = by_name.sessions.call("suppress_feature", &text) else {
            panic!("suppress_feature is a command");
        };
        let Ok(called) = calling.wait_timeout(HANG) else {
            panic!("the call didn't answer");
        };
        assert!(called.ok, "{}", called.text);
        assert_eq!(
            unplaced(&called.text, &by_name),
            unplaced(&response_text(&direct, Layout::OneLine), &typed),
            "{request}"
        );
    }
    // A refusal alike: a feature the document hasn't.
    let text = with(&example("request-unsuppress"), "feature", r#""z0000001""#);
    let text = with(&text, "revision", "14");
    let r = SuppressFeatureRequest::read(&parsed(&text)).unwrap();
    let direct = answer(typed.sessions.suppress_feature(
        &r.session,
        r.revision,
        r.feature,
        r.suppressed,
    ));
    let Call::Running(calling) = by_name.sessions.call("suppress_feature", &text) else {
        panic!("suppress_feature is a command");
    };
    let Ok(called) = calling.wait_timeout(HANG) else {
        panic!("the call didn't answer");
    };
    assert!(!called.ok);
    assert_eq!(
        unplaced(&called.text, &by_name),
        unplaced(&response_text(&direct, Layout::OneLine), &typed)
    );
}

/// `text`, an envelope, with `f`'s document's path, which each fixture's directory makes its
/// own, written `DOCUMENT`.
fn unplaced(text: &str, f: &Fixture) -> String {
    let path = serde_json::to_string(&f.path().display().to_string()).unwrap();
    assert!(text.contains(&path), "{text}");
    text.replace(&path, "DOCUMENT")
}
