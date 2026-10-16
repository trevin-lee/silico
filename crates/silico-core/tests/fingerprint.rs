//! M2.6: fingerprints. Each built feature's solid measured and written with it; a rebuild
//! that measures otherwise is drift, naming the feature; an unchanged document rebuilds to
//! the same bytes, and an edit rewrites exactly the fingerprints it changes. Expected values
//! are worked by hand: the fake's measures by formula (a box's, a cylinder's, and its own
//! boolean's, its inputs' together), rounded to 6 decimals with Python's exact decimals.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/example.rs"]
mod example;
#[path = "common/model.rs"]
mod model;

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
    write(&model.fingerprinted(&document)).unwrap()
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
    assert!(model.drift(&document).is_empty());
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
    model.drift(document)
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
fn a_feature_that_doesnt_build_isnt_drift_and_loses_its_fingerprint() {
    let document = read(saved(EXAMPLE).as_bytes()).unwrap();
    let engine = engine();
    engine.kernel().refuse(|op| {
        matches!(op, Op::MakePrimitive(Primitive::Cylinder { .. }))
            .then(|| Error::new(ErrorCode::KernelFailed, "MakeCylinder failed"))
    });
    let model = published(&engine, Checked::new(document.clone()).unwrap());
    assert!(matches!(
        model.outcome(&id("a91cw0de")),
        Some(FeatureOutcome::Failed(_))
    ));
    assert!(matches!(
        model.outcome(&id("m3tq8v0z")),
        Some(FeatureOutcome::Blocked { .. })
    ));
    // The pin failed and the hole was blocked: no drift, the failure says why.
    assert!(model.drift(&document).is_empty());
    // Written as rebuilt: the block's kept, the others' gone (nothing is kept from a
    // previous save).
    let fingerprints: Vec<_> = model
        .fingerprinted(&document)
        .features
        .iter()
        .map(|f| f.fingerprint.is_some())
        .collect();
    assert_eq!(fingerprints, [true, false, false]);
}

#[test]
fn a_shape_that_cant_be_summarized_fails_its_feature() {
    let engine = engine();
    engine.kernel().refuse(|op| {
        matches!(op, Op::Summarize { .. })
            .then(|| Error::new(ErrorCode::KernelFailed, "Summarize failed"))
    });
    let run = engine.start(checked(EXAMPLE)).wait_timeout(WAIT);
    let Ok(Ok(RunOutcome::Published(model))) = run else {
        panic!("not published");
    };
    let block = model.outcome(&id("k7f2qx3m"));
    let Some(FeatureOutcome::Failed(error)) = block else {
        panic!("{block:?}");
    };
    assert_eq!(error.code(), ErrorCode::KernelFailed);
    assert_eq!(error.feature(), Some("k7f2qx3m"));
    assert_eq!(model.fingerprint(&id("k7f2qx3m")), None);
}

#[test]
fn each_step_is_summarized_once_and_a_kept_one_not_again() {
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
    // Four steps (the block, the pin and its move, the cut), each made and summarized.
    assert_eq!(
        names,
        [
            "Cut",
            "MakeBox",
            "MakeCylinder",
            "Summarize",
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
