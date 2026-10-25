//! M2.6: fingerprints. Each built feature's solid measured and written with it; a rebuild
//! that measures otherwise is drift, naming the feature; an unchanged document rebuilds to
//! the same bytes, and an edit rewrites exactly the fingerprints it changes. Expected values
//! are worked by hand: the fake's measures by formula (a box's, a cylinder's, and its own
//! boolean's, its inputs' together), rounded to 6 decimals with Python's exact decimals.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::example;
use super::model;

use example::EXAMPLE;
use model::{WAIT, checked, described, id, made, published};
use silico_core::ErrorCode;
use silico_core::document::{Document, Fingerprint, Length, Measure, read, write};
use silico_core::fake::FakeKernel;
use silico_core::kernel::{Op, Primitive};
use silico_core::recompute::{FeatureOutcome, Recompute, RunOutcome};
use silico_core::{Error, document::Checked};

const BLOCK: &str = r#""fingerprint": {"volume": 8000, "area": 2800, "bbox": [0, 0, 0, 40, 20, 10], "faces": 6, "edges": 12, "vertices": 8}"#;
// π r² h and 2π r h + 2π r² for r 2.5, h 12; the box 17.5 to 22.5, 7.5 to 12.5, -1 to 11.
const PIN: &str = r#""fingerprint": {"volume": 235.619449, "area": 227.765467, "bbox": [17.5, 7.5, -1, 22.5, 12.5, 11], "faces": 3, "edges": 3, "vertices": 2}"#;
// The fake's cut: the block's and the pin's together.
const HOLE: &str = r#""fingerprint": {"volume": 8235.619449, "area": 3027.765467, "bbox": [0, 0, -1, 40, 20, 11], "faces": 9, "edges": 15, "vertices": 10}"#;

fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap())
}

/// The document `text` holds, rebuilt on a new kernel and written with its fingerprints.
fn saved(text: &str) -> String {
    let document = read(text.as_bytes()).unwrap();
    let model = published(&engine(), Checked::new(document.clone()).unwrap());
    write(&model.saved()).unwrap()
}

/// The line of `text` holding the feature `feature`'s fingerprint.
fn fingerprint_line(text: &str, feature: &str) -> String {
    let start = text
        .find(&format!(r#""id": "{feature}""#))
        .unwrap_or_else(|| panic!("no {feature} in {text}"));
    let rest = &text[start..];
    let line = rest
        .lines()
        .find(|line| line.trim_start().starts_with(r#""fingerprint""#))
        .unwrap_or_else(|| panic!("{feature} has no fingerprint in {text}"));
    line.trim().trim_end_matches(',').to_owned()
}

#[test]
fn each_built_feature_is_written_with_what_its_solid_measures() {
    let text = saved(EXAMPLE);
    assert_eq!(fingerprint_line(&text, "k7f2qx3m"), BLOCK);
    assert_eq!(fingerprint_line(&text, "a91cw0de"), PIN);
    assert_eq!(fingerprint_line(&text, "m3tq8v0z"), HOLE);
    // One member to a line once a feature holds an object, the fingerprint last, after the
    // note.
    assert!(
        text.contains(&format!(
            "      \"note\": \"Clearance for an M5 bolt\",\n      {HOLE}\n    }}"
        )),
        "{text}"
    );
}

#[test]
fn a_fingerprint_survives_a_round_trip() {
    let text = saved(EXAMPLE);
    let document = read(text.as_bytes()).unwrap();
    assert_eq!(write(&document).unwrap(), text);
    let hole = &document.features[2];
    assert_eq!(
        hole.fingerprint,
        Some(Fingerprint {
            volume: Measure::from_millionths(8_235_619_449),
            area: Measure::from_millionths(3_027_765_467),
            bbox: Some([0, 0, -1_000_000, 40_000_000, 20_000_000, 11_000_000].map(Length::from_nm)),
            faces: 9,
            edges: 15,
            vertices: 10,
        })
    );
}

#[test]
fn a_rebuild_of_an_unchanged_document_leaves_its_bytes_as_they_were() {
    let first = saved(EXAMPLE);
    // Rebuilt by another session, on another kernel, twice over.
    let second = saved(&first);
    assert_eq!(second, first);
    assert_eq!(saved(&second), first);
    // And it isn't drift.
    let document = read(first.as_bytes()).unwrap();
    let model = published(&engine(), Checked::new(document.clone()).unwrap());
    assert!(model.drift().is_empty());
}

#[test]
fn an_edit_rewrites_exactly_the_fingerprints_it_changes() {
    let before = saved(EXAMPLE);
    let edited = before.replace(r#""radius": 2.5"#, r#""radius": 3"#);
    let after = saved(&edited);
    let changed: Vec<(String, String)> = edited
        .lines()
        .zip(after.lines())
        .filter(|(a, b)| a != b)
        .map(|(a, b)| (a.to_owned(), b.to_owned()))
        .collect();
    // π 3² 12 = 339.292007, 2π 3 12 + 2π 3² = 282.743339; the box 17 to 23, 7 to 13.
    let pin = r#""fingerprint": {"volume": 339.292007, "area": 282.743339, "bbox": [17, 7, -1, 23, 13, 11], "faces": 3, "edges": 3, "vertices": 2}"#;
    let hole = r#""fingerprint": {"volume": 8339.292007, "area": 3082.743339, "bbox": [0, 0, -1, 40, 20, 11], "faces": 9, "edges": 15, "vertices": 10}"#;
    assert_eq!(
        changed,
        [
            (format!("      {PIN}"), format!("      {pin}")),
            (format!("      {HOLE}"), format!("      {hole}")),
        ]
    );
    assert_eq!(edited.lines().count(), after.lines().count());
}

/// The saved example with `from` replaced by `to` in its text, read back.
fn tampered(from: &str, to: &str) -> Document {
    let text = saved(EXAMPLE);
    assert!(text.contains(from), "{from} isn't in {text}");
    read(text.replacen(from, to, 1).as_bytes()).unwrap()
}

fn drift(document: &Document) -> Vec<Error> {
    let model = published(&engine(), Checked::new(document.clone()).unwrap());
    assert_eq!(model.document(), document);
    model.drift()
}

#[test]
fn a_tampered_fingerprint_is_drift_naming_the_feature_and_what_moved() {
    let document = tampered(r#""volume": 8000,"#, r#""volume": 7999.5,"#);
    let errors = drift(&document);
    assert_eq!(errors.len(), 1, "{errors:?}");
    let error = &errors[0];
    assert_eq!(error.code(), ErrorCode::Drift);
    assert_eq!(error.feature(), Some("k7f2qx3m"));
    assert_eq!(error.parameter(), Some("fingerprint.volume"));
    assert_eq!(
        error.message(),
        "feature k7f2qx3m rebuilt to other measures than its fingerprint: volume 7999.5 → 8000"
    );
    assert!(error.suggestion().unwrap().contains("determinism bug"));
}

#[test]
fn every_field_that_moved_is_named_and_every_drifting_feature_reported_in_order() {
    let text = saved(EXAMPLE)
        .replacen(r#""faces": 9"#, r#""faces": 8"#, 1)
        .replacen(
            r#""bbox": [0, 0, -1, 40, 20, 11]"#,
            r#""bbox": [0, 0, -1, 40, 20, 12]"#,
            1,
        )
        .replacen(r#""area": 227.765467"#, r#""area": 227.765468"#, 1);
    let document = read(text.as_bytes()).unwrap();
    let errors = drift(&document);
    let said: Vec<_> = errors
        .iter()
        .map(|e| (e.feature().unwrap(), e.parameter().unwrap(), e.message()))
        .collect();
    assert_eq!(
        said,
        [
            (
                "a91cw0de",
                "fingerprint.area",
                "feature a91cw0de rebuilt to other measures than its fingerprint: area 227.765468 → 227.765467"
            ),
            (
                "m3tq8v0z",
                "fingerprint.bbox",
                "feature m3tq8v0z rebuilt to other measures than its fingerprint: bbox [0, 0, -1, 40, 20, 12] → [0, 0, -1, 40, 20, 11], faces 8 → 9"
            ),
        ]
    );
    assert!(errors.iter().all(|e| e.code() == ErrorCode::Drift));
}

#[test]
fn edges_and_vertices_that_moved_are_drift_too() {
    // The pin stored with 3 vertices (it has 2), the hole with 16 edges (it has 15).
    let text = saved(EXAMPLE)
        .replacen(r#""edges": 15"#, r#""edges": 16"#, 1)
        .replacen(r#""vertices": 2"#, r#""vertices": 3"#, 1);
    let document = read(text.as_bytes()).unwrap();
    let said: Vec<_> = drift(&document)
        .iter()
        .map(|e| {
            (
                e.feature().unwrap().to_owned(),
                e.parameter().unwrap().to_owned(),
                e.message().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        said,
        [
            (
                "a91cw0de".to_owned(),
                "fingerprint.vertices".to_owned(),
                "feature a91cw0de rebuilt to other measures than its fingerprint: vertices 3 → 2"
                    .to_owned()
            ),
            (
                "m3tq8v0z".to_owned(),
                "fingerprint.edges".to_owned(),
                "feature m3tq8v0z rebuilt to other measures than its fingerprint: edges 16 → 15"
                    .to_owned()
            ),
        ]
    );
}

#[test]
fn a_box_stored_where_none_rebuilds_is_drift_too() {
    // Stored without a box (a solid with nothing in it), rebuilt with one.
    let document = tampered(r#", "bbox": [0, 0, 0, 40, 20, 10]"#, "");
    let errors = drift(&document);
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0].message(),
        "feature k7f2qx3m rebuilt to other measures than its fingerprint: bbox none → [0, 0, 0, 40, 20, 10]"
    );
}

#[test]
fn a_feature_with_no_fingerprint_isnt_drift_and_the_next_save_writes_one() {
    // The example as written has none.
    let document = read(EXAMPLE.as_bytes()).unwrap();
    assert!(document.features.iter().all(|f| f.fingerprint.is_none()));
    assert!(drift(&document).is_empty());
    let text = saved(EXAMPLE);
    assert_eq!(fingerprint_line(&text, "k7f2qx3m"), BLOCK);
}

#[test]
fn a_feature_that_doesnt_build_isnt_drift_and_keeps_its_fingerprint() {
    // However it failed: for its values, or because a worker hung or crashed. A save then
    // keeps the record a later rebuild is checked against; there's nothing to replace it.
    for code in [
        ErrorCode::KernelFailed,
        ErrorCode::WorkerTimeout,
        ErrorCode::WorkerCrashed,
    ] {
        let document = read(saved(EXAMPLE).as_bytes()).unwrap();
        let engine = engine();
        engine.kernel().refuse(move |op| {
            matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
                .then(|| Error::new(code, "MakeCylinder failed"))
        });
        let model = published(&engine, Checked::new(document.clone()).unwrap());
        assert!(
            matches!(
                model.outcome(&id("a91cw0de")),
                Some(FeatureOutcome::Failed(_))
            ),
            "{code}"
        );
        assert!(
            matches!(
                model.outcome(&id("m3tq8v0z")),
                Some(FeatureOutcome::Blocked { .. })
            ),
            "{code}"
        );
        // The pin failed and the hole was blocked: no drift, the failure says why.
        assert!(model.drift().is_empty(), "{code}");
        // Written back: the block's as rebuilt, the others' as stored, the same bytes.
        let written = model.saved();
        assert_eq!(written, document, "{code}");
        assert_eq!(write(&written).unwrap(), saved(EXAMPLE), "{code}");
    }
}

#[test]
fn a_solid_that_cant_be_measured_is_built_but_says_why_and_keeps_its_fingerprint() {
    let document = read(saved(EXAMPLE).as_bytes()).unwrap();
    let engine = engine();
    engine.kernel().refuse(|op| {
        matches!(op, Op::Summarize { .. })
            .then(|| Error::new(ErrorCode::KernelFailed, "Summarize failed"))
    });
    let run = engine
        .start(Checked::new(document.clone()).unwrap())
        .wait_timeout(WAIT);
    let Ok(Ok(RunOutcome::Published(model))) = run else {
        panic!("not published");
    };
    // Every feature built (measuring isn't making: what uses a solid needs its shape), none
    // measured.
    for feature in ["k7f2qx3m", "a91cw0de", "m3tq8v0z"] {
        assert!(
            matches!(
                model.outcome(&id(feature)),
                Some(FeatureOutcome::Built { .. })
            ),
            "{feature}"
        );
        assert_eq!(model.fingerprint(&id(feature)), None, "{feature}");
    }
    assert_eq!(
        described(&model, "m3tq8v0z"),
        "cut(box(40, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))"
    );
    // The check says why each couldn't be measured, naming it.
    let said: Vec<_> = model
        .drift()
        .iter()
        .map(|e| (e.code(), e.feature().unwrap().to_owned()))
        .collect();
    assert_eq!(
        said,
        ["k7f2qx3m", "a91cw0de", "m3tq8v0z"].map(|f| (ErrorCode::KernelFailed, f.to_owned()))
    );
    // And a save keeps what was stored.
    assert_eq!(model.saved(), document);
    // Asked again (the failure isn't kept), it's measured.
    let again = Recompute::new(FakeKernel::new(2).unwrap());
    let model = published(&again, Checked::new(document.clone()).unwrap());
    assert!(model.drift().is_empty());
}

#[test]
fn what_uses_a_solid_doesnt_wait_for_it_to_be_measured() {
    // Every measure held: the cut still runs, on the block and the moved pin. A held
    // request holds one of the fake's workers, and two measures (the block's and the
    // pin's) can be held before the cut is asked, so the fake has a third for it.
    let engine = Recompute::new(FakeKernel::new(3).unwrap());
    let hold = engine
        .kernel()
        .hold(|op| matches!(op, Op::Summarize { .. }));
    let run = engine.start(checked(EXAMPLE));
    let deadline = std::time::Instant::now() + WAIT;
    while !engine.kernel().calls().iter().any(|c| c.op.name() == "Cut") {
        assert!(
            std::time::Instant::now() < deadline,
            "the cut waited for the measures: {:?}",
            made(&engine.kernel().calls())
        );
        std::thread::yield_now();
    }
    hold.release();
    let Ok(Ok(RunOutcome::Published(model))) = run.wait_timeout(WAIT) else {
        panic!("not published");
    };
    assert!(model.fingerprint(&id("m3tq8v0z")).is_some());
}

#[test]
fn each_features_solid_is_measured_once_and_a_kept_one_not_again() {
    let engine = engine();
    let model = published(&engine, checked(EXAMPLE));
    assert_eq!(
        described(&model, "m3tq8v0z"),
        "cut(box(40, 20, 10), translate(cylinder(2.5, 12), 20, 10, -1))"
    );
    let mut names: Vec<_> = engine
        .kernel()
        .calls()
        .iter()
        .map(|c| c.op.name())
        .collect();
    names.sort_unstable();
    // Four steps (the block, the pin and its move, the cut); three features' solids
    // measured (the pin's is its move's, not its make's), each summarized and described, a
    // batch for each of its faces, edges, and vertices (each fewer than DESCRIBE_BATCH).
    assert_eq!(
        names,
        [
            "Cut",
            "Describe",
            "Describe",
            "Describe",
            "Describe",
            "Describe",
            "Describe",
            "Describe",
            "Describe",
            "Describe",
            "MakeBox",
            "MakeCylinder",
            "Summarize",
            "Summarize",
            "Summarize",
            "Translate"
        ]
    );
    let before = engine.kernel().calls().len();
    published(&engine, checked(EXAMPLE));
    assert!(made(&engine.kernel().calls()[before..]).is_empty());
    assert_eq!(engine.kernel().calls().len(), before);
}

#[test]
fn a_published_model_doesnt_change_when_a_later_recompute_measures_its_solids() {
    // The first recompute's measures time out; the second, of the same document on the same
    // engine, measures the solids the first left unmeasured. The first model, published
    // already, must say and write what it did.
    let failing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let engine = engine();
    let flag = std::sync::Arc::clone(&failing);
    engine.kernel().refuse(move |op| {
        (flag.load(std::sync::atomic::Ordering::SeqCst) && matches!(op, Op::Summarize { .. }))
            .then(|| Error::new(ErrorCode::WorkerTimeout, "the worker hung"))
    });
    let first = published(&engine, checked(EXAMPLE));
    let written = write(&first.saved()).unwrap();
    assert_eq!(first.fingerprint(&id("k7f2qx3m")), None);
    assert_eq!(first.drift().len(), 3);
    failing.store(false, std::sync::atomic::Ordering::SeqCst);
    let second = published(&engine, checked(EXAMPLE));
    assert!(second.fingerprint(&id("k7f2qx3m")).is_some());
    assert!(second.drift().is_empty());
    for feature in ["k7f2qx3m", "a91cw0de", "m3tq8v0z"] {
        assert_eq!(first.fingerprint(&id(feature)), None, "{feature}");
        let Some(FeatureOutcome::Built { built, .. }) = first.outcome(&id(feature)) else {
            panic!("{feature} didn't build");
        };
        assert_eq!(built.summary(), None, "{feature}");
    }
    assert_eq!(first.drift().len(), 3);
    assert_eq!(write(&first.saved()).unwrap(), written);
}

#[test]
fn unmeasured_and_drift_are_reported_in_timeline_order_together() {
    // The block and the hole can't be measured; the pin is stored with 3 vertices (it has
    // 2): an unmeasured error, drift, and an unmeasured error, in the timeline's order.
    let text = saved(EXAMPLE).replacen(r#""vertices": 2"#, r#""vertices": 3"#, 1);
    let document = read(text.as_bytes()).unwrap();
    let engine = engine();
    engine.kernel().refuse(|op| match op {
        Op::Summarize { shape } => {
            let d = FakeKernel::describe(shape).unwrap_or_default();
            (d.starts_with("box") || d.starts_with("cut"))
                .then(|| Error::new(ErrorCode::KernelFailed, "Summarize failed"))
        }
        _ => None,
    });
    let model = published(&engine, Checked::new(document).unwrap());
    let said: Vec<_> = model
        .drift()
        .iter()
        .map(|e| (e.code(), e.feature().unwrap().to_owned()))
        .collect();
    assert_eq!(
        said,
        [
            (ErrorCode::KernelFailed, "k7f2qx3m".to_owned()),
            (ErrorCode::Drift, "a91cw0de".to_owned()),
            (ErrorCode::KernelFailed, "m3tq8v0z".to_owned()),
        ]
    );
}
