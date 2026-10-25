//! M4.4: each built feature's solid is described as part of its rebuild, before the model is
//! published (docs/design/m4.1-command-api.md, *Where the entries come from*): its faces,
//! edges, and vertices, a batch of at most `DESCRIBE_BATCH` at a time, kept on the model
//! beside its summary and in the rebuild cache, so a read of the model asks the kernel
//! nothing. A description the kernel refuses for the solid's values is kept as a refused
//! summary is (tests/measure_refusal.rs). The fake describes each element as its number
//! alone (`fake::described`), so the expected values here are counts by hand and those
//! numbers; a worker's geometry is tested on a worker (silico-kernel's tests/elements.rs).

use super::cache;
use super::example;
use super::model;
use super::refusals;

use cache::{HOLE, Scratch, calls, files, hole, session};
use example::EXAMPLE;
use model::{checked, id, published};
use refusals::refusing_to;
use silico_core::ErrorCode;
use silico_core::elements::elements;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{EdgeCurve, ElementKind, Surface};
use silico_core::recompute::Recompute;

/// How many requests of `op` a kernel was asked.
fn asked(kernel: &FakeKernel, op: &str) -> usize {
    kernel
        .calls()
        .iter()
        .filter(|call| call.op.name() == op)
        .count()
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

/// A document of `n` boxes 20 apart along x, each fused onto the fusion of those before.
fn fused_boxes(n: usize) -> String {
    let mut features = Vec::new();
    for i in 0..n {
        features.push(format!(
            r#"    {{"id": "b{i:07}", "type": "box", "v": 1, "name": "Box", "position": [{}, 0, 0], "length": 10, "width": 10, "height": 10}}"#,
            i * 20
        ));
    }
    for i in 1..n {
        let base = if i == 1 {
            "b0000000".to_owned()
        } else {
            format!("f{:07}", i - 1)
        };
        features.push(format!(
            r#"    {{"id": "f{i:07}", "type": "fuse", "v": 1, "name": "Fuse", "base": "{base}", "tool": "b{i:07}"}}"#
        ));
    }
    format!(
        "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"naming\": 5,\n  \"features\": [\n{}\n  ]\n}}\n",
        features.join(",\n")
    )
}

#[test]
fn a_solid_of_more_elements_than_a_batch_is_described_in_batches_joined_in_order() {
    // 90 separate boxes fused: 540 faces, 1,080 edges, and 720 vertices, by hand (the fake
    // keeps each box's). Its edges take two batches, 1,000 and then 80, joined in the
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
            (ElementKind::Face, 1, 1000),
            (ElementKind::Edge, 1, 1000),
            (ElementKind::Edge, 1001, 1000),
            (ElementKind::Vertex, 1, 1000),
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
fn a_description_refused_for_its_values_is_kept_and_never_asked_again() {
    // As a summary refused for the solid's values: the solid stays built and is reported
    // unmeasured (drift says so), and its refusal is kept, for the session and, for invalid
    // input or an invalid result, on disk, so it isn't asked again.
    for code in [ErrorCode::InvalidInput, ErrorCode::InvalidResult] {
        let dir = Scratch::new(&format!("describe-refused-{}", code.as_str()));
        let first = session(&dir.0);
        refusing_to(first.kernel(), code, "Describe");
        let model = published(&first, checked(EXAMPLE));
        let drift: Vec<(ErrorCode, String)> = model
            .drift()
            .iter()
            .map(|e| (e.code(), e.feature().unwrap_or_default().to_owned()))
            .collect();
        assert_eq!(
            drift,
            ["k7f2qx3m", "a91cw0de", "m3tq8v0z"].map(|f| (code, f.to_owned())),
            "{code:?}"
        );
        assert_eq!(hole(&model), HOLE);
        assert!(model.description(&id("m3tq8v0z")).is_none());
        // Asked once a solid in the session.
        let asked_before = first.kernel().calls().len();
        published(&first, checked(EXAMPLE));
        assert_eq!(first.kernel().calls().len(), asked_before, "{code:?}");
        // And a session that would describe them asks nothing, and says the same.
        let second = session(&dir.0);
        let again = published(&second, checked(EXAMPLE));
        assert!(second.kernel().calls().is_empty(), "{:?}", calls(&second));
        assert_eq!(again.drift(), model.drift(), "{code:?}");
        assert!(!files(&dir.0).is_empty());
    }
}

#[test]
fn a_description_a_worker_failed_is_asked_again() {
    // A crash or a hang follows from nothing about the solid: each recompute asks again, and
    // the cache keeps the solid unmeasured, so the next session describes it.
    for code in [ErrorCode::WorkerCrashed, ErrorCode::WorkerTimeout] {
        let dir = Scratch::new(&format!("describe-failed-{}", code.as_str()));
        let failing = session(&dir.0);
        refusing_to(failing.kernel(), code, "Describe");
        let failed = published(&failing, checked(EXAMPLE));
        assert_eq!(failed.drift().len(), 3, "{code:?}");
        assert!(failed.description(&id("m3tq8v0z")).is_none());
        // Each solid's first batch refused: three asked, and three again.
        assert_eq!(asked(failing.kernel(), "Describe"), 3, "{code:?}");
        published(&failing, checked(EXAMPLE));
        assert_eq!(asked(failing.kernel(), "Describe"), 6, "{code:?}");
        let next = session(&dir.0);
        let model = published(&next, checked(EXAMPLE));
        assert!(calls(&next).is_empty(), "{code:?}: {:?}", calls(&next));
        assert_eq!(asked(next.kernel(), "Describe"), 9, "{code:?}");
        assert!(model.drift().is_empty(), "{code:?}");
        assert!(model.description(&id("m3tq8v0z")).is_some(), "{code:?}");
    }
}
