//! M4.4: each built feature's solid is described as part of its rebuild, before the model is
//! published (docs/design/m4.1-command-api.md, *Where the entries come from*): its faces,
//! edges, and vertices, a batch of at most `DESCRIBE_BATCH` at a time, kept on the model
//! beside its summary and in the rebuild cache, so a read of the model asks the kernel
//! nothing. A description refused or failed is kept beside the summary, which stands
//! (docs/design/m4.4-describe.md, decision 4): refused for the solid's values, it's kept as a
//! refused summary is (tests/measure_refusal.rs); a worker's failure is asked again. The
//! fake describes each element as its number
//! alone (`fake::described`), so the expected values here are counts by hand and those
//! numbers; a worker's geometry is tested on a worker (silico-kernel's tests/elements.rs).

use super::cache;
use super::cache_format;
use super::describing;
use super::example;
use super::model;
use super::refusals;

use std::sync::Arc;

use cache::{HOLE, Scratch, calls, files, hole, session};
use cache_format::{Describe, Measure, decode, entry_path};
use describing::{asked, fused_boxes, settled};
use example::EXAMPLE;
use model::{checked, id, published};
use refusals::{refusing_to, worker_failure};
use silico_core::ErrorCode;
use silico_core::elements::elements;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{EdgeCurve, ElementKind, Surface};
use silico_core::recompute::{Built, FeatureOutcome, Model, Recompute};

/// The example's three features: the block, the pin, and the hole.
const FEATURES: [&str; 3] = ["k7f2qx3m", "a91cw0de", "m3tq8v0z"];

fn built(model: &Model, feature: &str) -> (String, Arc<Built>) {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { key, built }) => (key.to_string(), Arc::clone(built)),
        other => panic!("{feature}: {other:?}"),
    }
}

/// Whether `op` is a request for a batch of a description.
fn describe(op: &silico_core::kernel::Op) -> bool {
    op.name() == "Describe"
}

/// Asserts each of the example's features built, kept its summary and fingerprint, and has
/// no description, for the reason `code`.
#[track_caller]
fn assert_summarized_not_described(model: &Model, code: ErrorCode) {
    for feature in FEATURES {
        let (_, built) = built(model, feature);
        assert!(built.summary().is_some(), "{code:?}: {feature}'s summary");
        assert!(
            model.fingerprint(&id(feature)).is_some(),
            "{code:?}: {feature}"
        );
        assert!(
            model.description(&id(feature)).is_none(),
            "{code:?}: {feature}"
        );
        assert_eq!(
            built.description_refused().map(silico_core::Error::code),
            Some(code),
            "{feature}"
        );
    }
}

#[test]
fn each_built_features_solid_is_described_as_its_rebuilt_element_for_element() {
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    let model = published(&engine, checked(EXAMPLE));
    // The block is a box: 6 faces, 12 edges, 8 vertices, by hand.
    let block = model.description(&id("k7f2qx3m")).unwrap();
    assert_eq!(
        (block.faces.len(), block.edges.len(), block.vertices.len()),
        (6, 12, 8)
    );
    for feature in ["k7f2qx3m", "a91cw0de", "m3tq8v0z"] {
        let d = model.description(&id(feature)).unwrap();
        let built = match model.outcome(&id(feature)) {
            Some(silico_core::recompute::FeatureOutcome::Built { built, .. }) => built,
            other => panic!("{feature}: {other:?}"),
        };
        // Its summary's counts, and the solid's own description.
        let summary = built.summary().unwrap();
        assert_eq!(
            [d.faces.len(), d.edges.len(), d.vertices.len()],
            [summary.faces, summary.edges, summary.vertices].map(|n| n as usize),
            "{feature}"
        );
        assert_eq!(built.description(), Some(d));
        // Element n is the fake's element n, in the kernel's numbering, from 1.
        for (i, face) in d.faces.iter().enumerate() {
            assert_eq!(face.surface, Surface::Other);
            assert_eq!(
                face.centroid,
                [(i + 1) as f64, 0.0, 0.0],
                "{feature} face {i}"
            );
        }
        for (i, edge) in d.edges.iter().enumerate() {
            assert_eq!(edge.curve, EdgeCurve::Other);
            assert_eq!(edge.start, [(i + 1) as f64, 0.0, 0.0], "{feature} edge {i}");
        }
        for (i, vertex) in d.vertices.iter().enumerate() {
            assert_eq!(*vertex, [(i + 1) as f64, 0.0, 0.0], "{feature} vertex {i}");
        }
        // And it's named by the feature's element map, every element once.
        let map = model.element_map(&id(feature)).unwrap();
        let listed = elements(map, d.clone()).unwrap();
        let total = [ElementKind::Face, ElementKind::Edge, ElementKind::Vertex]
            .map(|kind| map.count(kind))
            .iter()
            .sum::<usize>();
        assert_eq!(listed.len(), total, "{feature}");
    }
    // Three solids measured: each one summary, and one batch of each kind, every kind
    // under a batch.
    assert_eq!(asked(engine.kernel(), "Summarize"), 3);
    assert_eq!(asked(engine.kernel(), "Describe"), 9);
}

#[test]
fn a_feature_that_didnt_build_has_no_description() {
    // The pin refused: it failed, and the cut that uses it is blocked; neither has a solid
    // to describe. The block still has its own.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().refuse(|op| {
        (op.name() == "MakeCylinder")
            .then(|| silico_core::Error::new(ErrorCode::InvalidInput, "no pins today"))
    });
    let model = published(&engine, checked(EXAMPLE));
    assert!(model.description(&id("a91cw0de")).is_none());
    assert!(model.description(&id("m3tq8v0z")).is_none());
    assert!(model.description(&id("k7f2qx3m")).is_some());
    // And nothing describes a feature the document doesn't have.
    assert!(model.description(&id("zzzzzzzz")).is_none());
}

#[test]
fn a_solid_of_more_elements_than_a_batch_is_described_in_batches_joined_in_order() {
    // 90 separate boxes fused: 540 faces, 1,080 edges, and 720 vertices, by hand (the fake
    // keeps each box's). Each batch is planned from the summary: its faces and vertices one
    // batch each of as many as it has, its edges two, 1,000 and then 80, joined in the
    // kernel's numbering.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    let model = published(&engine, checked(&fused_boxes(90)));
    let last = model.description(&id("f0000089")).unwrap();
    assert_eq!(
        (last.faces.len(), last.edges.len(), last.vertices.len()),
        (540, 1080, 720)
    );
    for (i, edge) in last.edges.iter().enumerate() {
        assert_eq!(edge.start, [(i + 1) as f64, 0.0, 0.0], "edge {i}");
    }
    let describes: Vec<(ElementKind, u32, u32)> = engine
        .kernel()
        .calls()
        .iter()
        .filter_map(|call| match &call.op {
            silico_core::kernel::Op::Describe {
                shape,
                kind,
                first,
                count,
            } if FakeKernel::counts(shape).is_some_and(|c| c.edges == 1080) => {
                Some((*kind, *first, *count))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        describes,
        [
            (ElementKind::Face, 1, 540),
            (ElementKind::Edge, 1, 1000),
            (ElementKind::Edge, 1001, 80),
            (ElementKind::Vertex, 1, 720),
        ]
    );
}

#[test]
fn a_rebuild_from_the_cache_takes_the_descriptions_back_and_asks_the_kernel_nothing() {
    let dir = Scratch::new("described");
    let first = session(&dir.0);
    let model = published(&first, checked(EXAMPLE));
    let next = session(&dir.0);
    let again = published(&next, checked(EXAMPLE));
    assert!(next.kernel().calls().is_empty(), "{:?}", calls(&next));
    for feature in ["k7f2qx3m", "a91cw0de", "m3tq8v0z"] {
        assert_eq!(
            again.description(&id(feature)),
            model.description(&id(feature)),
            "{feature}"
        );
        assert!(again.description(&id(feature)).is_some());
    }
}

#[test]
fn a_description_refused_for_its_values_keeps_the_summary_and_the_refusal() {
    // Refused for the solid's values, a description is kept beside the summary, which
    // stands: the solid builds, its fingerprint is had, drift is empty, and only element
    // reads find nothing. The refusal is kept for the session, so it isn't asked again; and
    // on disk, as a refused summary is, for invalid input or an invalid result (as the
    // worker said it), but not for a kernel's failure, which the next session asks again:
    // the description alone, the summary taken back.
    for code in [
        ErrorCode::InvalidInput,
        ErrorCode::InvalidResult,
        ErrorCode::KernelFailed,
    ] {
        let lasting = code != ErrorCode::KernelFailed;
        let dir = Scratch::new(&format!("describe-refused-{}", code.as_str()));
        let first = session(&dir.0);
        refusing_to(first.kernel(), code, "Describe");
        let model = published(&first, checked(EXAMPLE));
        assert!(model.drift().is_empty(), "{code:?}: {:?}", model.drift());
        assert_eq!(hole(&model), HOLE);
        assert_summarized_not_described(&model, code);
        // Each solid's three batches sent at once, every one refused.
        assert_eq!(settled(first.kernel(), describe, 9), 9, "{code:?}");
        // Asked once a solid in the session.
        let asked_before = first.kernel().calls().len();
        let kept = published(&first, checked(EXAMPLE));
        assert_eq!(first.kernel().calls().len(), asked_before, "{code:?}");
        assert_summarized_not_described(&kept, code);
        // On disk: the worker's refusal as it said it, beside the summary, or nothing of
        // the description.
        for feature in FEATURES {
            let (key, _) = built(&model, feature);
            match decode(&std::fs::read(entry_path(&dir.0, &key)).unwrap()).measure {
                Measure::Measured {
                    description: Describe::Refused { op, failure },
                    ..
                } => {
                    assert!(lasting, "{code:?}: {feature} kept on disk");
                    assert_eq!(op, "Describe");
                    assert_eq!(Some(failure), worker_failure(code), "{code:?}");
                }
                Measure::Measured {
                    description: Describe::Unknown,
                    ..
                } => assert!(!lasting, "{code:?}: {feature} not kept on disk"),
                _ => panic!("{code:?}: {feature} isn't written with its summary"),
            }
        }
        let second = session(&dir.0);
        let again = published(&second, checked(EXAMPLE));
        assert!(calls(&second).is_empty(), "{code:?}: {:?}", calls(&second));
        assert_eq!(asked(second.kernel(), "Summarize"), 0, "{code:?}");
        assert!(again.drift().is_empty(), "{code:?}: {:?}", again.drift());
        assert!(!files(&dir.0).is_empty());
        if lasting {
            // A session that would describe them asks nothing, and says the same.
            assert!(second.kernel().calls().is_empty(), "{code:?}");
            assert_summarized_not_described(&again, code);
        } else {
            // Described again, a batch of each kind for each solid, and whole.
            assert_eq!(asked(second.kernel(), "Describe"), 9, "{code:?}");
            for feature in FEATURES {
                assert_eq!(
                    again.description(&id(feature)).map(|d| d.faces.len()),
                    built(&again, feature).1.summary().map(|s| s.faces as usize),
                    "{code:?}: {feature}"
                );
            }
        }
    }
}

#[test]
fn a_description_a_worker_failed_is_asked_again_and_only_it() {
    // A crash or a hang follows from nothing about the solid: the summary stands (drift is
    // empty), and each recompute asks the description again, not the summary; the cache
    // keeps the summary and nothing of the description, so the next session describes it,
    // making and summarizing nothing.
    for code in [ErrorCode::WorkerCrashed, ErrorCode::WorkerTimeout] {
        let dir = Scratch::new(&format!("describe-failed-{}", code.as_str()));
        let failing = session(&dir.0);
        refusing_to(failing.kernel(), code, "Describe");
        let failed = published(&failing, checked(EXAMPLE));
        assert!(failed.drift().is_empty(), "{code:?}: {:?}", failed.drift());
        assert_summarized_not_described(&failed, code);
        // Each solid's three batches sent at once: nine asked, and nine again.
        assert_eq!(settled(failing.kernel(), describe, 9), 9, "{code:?}");
        let again = published(&failing, checked(EXAMPLE));
        assert_eq!(settled(failing.kernel(), describe, 18), 18, "{code:?}");
        assert_eq!(asked(failing.kernel(), "Summarize"), 3, "{code:?}");
        assert_summarized_not_described(&again, code);
        let next = session(&dir.0);
        let model = published(&next, checked(EXAMPLE));
        assert!(calls(&next).is_empty(), "{code:?}: {:?}", calls(&next));
        assert_eq!(asked(next.kernel(), "Summarize"), 0, "{code:?}");
        assert_eq!(asked(next.kernel(), "Describe"), 9, "{code:?}");
        assert!(model.drift().is_empty(), "{code:?}");
        assert!(model.description(&id("m3tq8v0z")).is_some(), "{code:?}");
    }
}
