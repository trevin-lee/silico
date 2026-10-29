//! `silico_core::timeline` on the fake kernel (docs/design/m5.0-app.md, `timeline`, M5.1):
//! each feature's health, problems, and warnings, by hand from what the document does on
//! the fake: a fuse the kernel refuses, a fillet blocked by it, a fillet naming edges a
//! block hasn't, a box measured otherwise than its fingerprint, a box with none, a sketch
//! and its pad, and blocks that build as recorded.

use std::collections::BTreeSet;
use std::sync::Arc;

use silico_core::document::{Checked, Document, FeatureId, Operation, read};
use silico_core::recompute::Model;
use silico_core::timeline::{Entry, Excepted, Health, Timeline, timeline};
use silico_core::{Error, ErrorCode};
use silico_protocol::Op;

use crate::commands::engine;
use crate::model::published;
use crate::sweeps::{TEN, block, document, pad, rectangle};

fn id(s: &str) -> FeatureId {
    FeatureId::new(s).unwrap()
}

fn fuse(id: &str, base: &str, tool: &str) -> String {
    format!(
        r#"{{"id": "{id}", "type": "fuse", "v": 1, "name": "Joined", "base": "{base}", "tool": "{tool}"}}"#
    )
}

fn fillet(id: &str, base: &str, edges: &[&str]) -> String {
    let edges: Vec<String> = edges.iter().map(|e| format!("\"{e}\"")).collect();
    format!(
        r#"{{"id": "{id}", "type": "fillet", "v": 1, "name": "Round", "base": "{base}", "edges": [{}], "radius": 0.1}}"#,
        edges.join(", ")
    )
}

/// The document, rebuilt once on an engine whose kernel refuses every boolean with `code`,
/// saved with its fingerprints, then: dddddddd made 2 long (its fingerprint stale),
/// eeeeeeee's fingerprint dropped; and rebuilt again. That model.
fn rebuilt(code: ErrorCode) -> Arc<Model> {
    let text = document(&[
        &block("aaaaaaaa"),
        &block("cccccccc"),
        &block("bbbbbbbb"),
        &fuse("j0000001", "aaaaaaaa", "cccccccc"),
        &fillet("f0000001", "j0000001", &["aaaaaaaa.box.Edge1"]),
        &fillet(
            "f0000002",
            "bbbbbbbb",
            &["bbbbbbbb.box.Edge90", "bbbbbbbb.box.Edge91"],
        ),
        &rectangle("s0000001", "xy", [0.0, 0.0], 4.0, 2.0),
        &pad("p0000001", None, "s0000001", TEN),
        &block("dddddddd"),
        &block("eeeeeeee"),
    ]);
    let engine = engine();
    engine.kernel().refuse(move |op| {
        matches!(op, Op::Boolean { .. }).then(|| Error::new(code, "the fuse failed"))
    });
    let first = published(
        &engine,
        Checked::new(read(text.as_bytes()).unwrap()).unwrap(),
    );
    let mut saved: Document = first.saved();
    for feature in &mut saved.features {
        match feature.id.as_str() {
            "dddddddd" => {
                if let Operation::Box { length, .. } = &mut feature.operation {
                    *length = silico_core::document::Length::rounded(2.0).unwrap();
                }
            }
            "eeeeeeee" => feature.fingerprint = None,
            _ => {}
        }
    }
    published(&engine, Checked::new(saved).unwrap())
}

fn entry(
    id: &str,
    kind: &'static str,
    name: &str,
    inputs: &[&str],
    mark: (Health, u64, u64),
) -> Entry {
    Entry {
        id: crate::timeline::id(id),
        kind,
        name: name.to_owned(),
        suppressed: false,
        inputs: inputs.iter().map(|i| crate::timeline::id(i)).collect(),
        health: mark.0,
        problems: mark.1,
        warnings: mark.2,
    }
}

#[test]
fn each_feature_carries_the_health_its_model_gave_it() {
    let model = rebuilt(ErrorCode::KernelFailed);
    let got = timeline(model.document(), Some(&model), &Excepted::default());
    use Health::{Error, Healthy, Warning};
    let want = vec![
        entry("aaaaaaaa", "box", "Block", &[], (Healthy, 0, 0)),
        entry("cccccccc", "box", "Block", &[], (Healthy, 0, 0)),
        entry("bbbbbbbb", "box", "Block", &[], (Healthy, 0, 0)),
        // Refused by the kernel: its one problem.
        entry(
            "j0000001",
            "fuse",
            "Joined",
            &["aaaaaaaa", "cccccccc"],
            (Error, 1, 0),
        ),
        // Blocked by the fuse: no problem of its own, but nothing computed.
        entry("f0000001", "fillet", "Round", &["j0000001"], (Error, 0, 0)),
        // Two edges the block hasn't: each a problem.
        entry("f0000002", "fillet", "Round", &["bbbbbbbb"], (Error, 2, 0)),
        entry("s0000001", "sketch", "Sketch", &[], (Healthy, 0, 0)),
        entry("p0000001", "pad", "Pad", &["s0000001"], (Healthy, 0, 0)),
        // Measured otherwise than its fingerprint: drift.
        entry("dddddddd", "box", "Block", &[], (Error, 1, 0)),
        // Built, with no fingerprint: warned.
        entry("eeeeeeee", "box", "Block", &[], (Warning, 0, 1)),
    ];
    assert_eq!(got.features, want);
    assert_eq!(got.bodies, model.bodies());
}

#[test]
fn a_feature_an_open_transactions_edits_reached_isnt_warned_of_its_missing_fingerprint() {
    let model = rebuilt(ErrorCode::KernelFailed);
    let spared = BTreeSet::from([id("eeeeeeee")]);
    let got = timeline(
        model.document(),
        Some(&model),
        &Excepted {
            spared,
            ..Excepted::default()
        },
    );
    let e = got
        .features
        .iter()
        .find(|e| e.id == id("eeeeeeee"))
        .unwrap();
    assert_eq!((e.health, e.problems, e.warnings), (Health::Healthy, 0, 0));
}

#[test]
fn a_kernel_fault_and_what_it_blocked_are_unknown_not_the_documents_problems() {
    let model = rebuilt(ErrorCode::WorkerCrashed);
    let got = timeline(model.document(), Some(&model), &Excepted::default());
    let mark = |f: &str| {
        let e = got.features.iter().find(|e| e.id == id(f)).unwrap();
        (e.health, e.problems, e.warnings)
    };
    assert_eq!(mark("j0000001"), (Health::Unknown, 0, 0));
    assert_eq!(mark("f0000001"), (Health::Unknown, 0, 0));
    // The rest as before.
    assert_eq!(mark("f0000002"), (Health::Error, 2, 0));
    assert_eq!(mark("aaaaaaaa"), (Health::Healthy, 0, 0));
}

#[test]
fn a_feature_the_models_revision_hasnt_and_a_document_with_no_model_are_unknown() {
    let model = rebuilt(ErrorCode::KernelFailed);
    // The session's document has one more box than the model's.
    let mut newer = model.document().clone();
    let extra = read(document(&[&block("zzzzzzzz")]).as_bytes()).unwrap();
    newer.features.extend(extra.features);
    let got = timeline(&newer, Some(&model), &Excepted::default());
    let last = got.features.last().unwrap();
    assert_eq!(
        last,
        &entry("zzzzzzzz", "box", "Block", &[], (Health::Unknown, 0, 0))
    );
    assert_eq!(got.features.len(), 11);
    // No model: every feature unknown, as the document holds it, and no bodies.
    let Timeline { features, bodies } = timeline(&newer, None, &Excepted::default());
    assert!(bodies.is_empty());
    assert_eq!(features.len(), 11);
    for e in &features {
        assert_eq!(
            (e.health, e.problems, e.warnings),
            (Health::Unknown, 0, 0),
            "{:?}",
            e.id
        );
    }
    assert_eq!(features[3].inputs, [id("aaaaaaaa"), id("cccccccc")]);
}

#[test]
fn a_feature_edited_since_the_models_revision_and_what_uses_it_are_unknown() {
    // From #265's architecture review: the session's document a revision past the model's,
    // the block bbbbbbbb made 3 long: it and the fillet of it (f0000002) are what the model
    // hasn't built; the rest keep its marks, and bbbbbbbb, no longer as built, isn't a body.
    let model = rebuilt(ErrorCode::KernelFailed);
    let mut newer = model.document().clone();
    for feature in &mut newer.features {
        if feature.id.as_str() == "bbbbbbbb"
            && let Operation::Box { length, .. } = &mut feature.operation
        {
            *length = silico_core::document::Length::rounded(3.0).unwrap();
        }
    }
    let got = timeline(&newer, Some(&model), &Excepted::default());
    let mark = |f: &str| {
        let e = got.features.iter().find(|e| e.id == id(f)).unwrap();
        (e.health, e.problems, e.warnings)
    };
    assert_eq!(mark("bbbbbbbb"), (Health::Unknown, 0, 0));
    assert_eq!(mark("f0000002"), (Health::Unknown, 0, 0));
    assert_eq!(mark("aaaaaaaa"), (Health::Healthy, 0, 0));
    assert_eq!(mark("j0000001"), (Health::Error, 1, 0));
    assert_eq!(mark("eeeeeeee"), (Health::Warning, 0, 1));
    // A rename or a note isn't an edit the model would build otherwise.
    let mut renamed = model.document().clone();
    renamed.features[0].name = silico_core::document::Name::new("Other").unwrap();
    let got = timeline(&renamed, Some(&model), &Excepted::default());
    assert_eq!(got.features[0].health, Health::Healthy);
}

#[test]
fn bodies_are_the_documents_built_solids_that_nothing_in_it_consumes() {
    // From #265's round 2: bodies judged by the session's document, not the model's: a
    // solid no feature consumes that the model built or hasn't built yet. (A solid whose
    // consumer failed is a body still: the first test's blocks under the refused fuse.)
    let text = document(&[
        &block("aaaaaaaa"),
        &block("bbbbbbbb"),
        &fuse("j0000001", "aaaaaaaa", "bbbbbbbb"),
    ]);
    let engine = engine();
    let fused = published(
        &engine,
        Checked::new(read(text.as_bytes()).unwrap()).unwrap(),
    );
    let got = |document: &Document, model: &Model| {
        timeline(document, Some(model), &Excepted::default()).bodies
    };
    assert_eq!(got(fused.document(), &fused), [id("j0000001")]);
    // The fuse removed since: its blocks are bodies again.
    let mut unfused = fused.document().clone();
    unfused.features.truncate(2);
    assert_eq!(got(&unfused, &fused), [id("aaaaaaaa"), id("bbbbbbbb")]);
    // A model of the blocks alone, and the fuse added since: they're consumed, and the fuse
    // isn't built yet.
    let blocks = published(
        &engine,
        Checked::new(read(document(&[&block("aaaaaaaa"), &block("bbbbbbbb")]).as_bytes()).unwrap())
            .unwrap(),
    );
    assert!(got(fused.document(), &blocks).is_empty());
    // A body edited since stays, marked unknown.
    let mut grown = fused.document().clone();
    if let Operation::Box { length, .. } = &mut grown.features[0].operation {
        *length = silico_core::document::Length::rounded(3.0).unwrap();
    }
    let t = timeline(&grown, Some(&fused), &Excepted::default());
    assert_eq!(t.bodies, [id("j0000001")]);
    assert_eq!(t.features[2].health, Health::Unknown);
}
