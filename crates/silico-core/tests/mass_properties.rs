//! M4.5: `mass_properties` on a model and on a session (`silico_core::command::measure`;
//! docs/design/m4.1-command-api.md, section 7): which solids it reads, in what order, and
//! every reason one can't be read. On the fake kernel, whose measures aren't a solid's; the
//! values themselves are measured on a worker (silico-kernel's tests/mass_properties.rs).

use super::example;
use super::listing::{TWO_BODIES, answer, codes, fixture, model_of};
use super::model;

use example::EXAMPLE;
use model::{checked, id, published};
use silico_core::ErrorCode;
use silico_core::command::measure::mass_properties;
use silico_core::command::{ModelChoice, ModelRead, Revision};
use silico_core::document::Fingerprint;
use silico_core::fake::FakeKernel;
use silico_core::kernel::Op;
use silico_core::recompute::{Model, Recompute};

/// The fingerprint the feature `feature`'s rebuild gives, as `fmt` writes it.
fn fingerprint(model: &Model, feature: &str) -> Fingerprint {
    model.fingerprint(&id(feature)).unwrap()
}

#[test]
fn every_body_is_measured_in_timeline_order_its_numbers_its_fingerprints() {
    let model = model_of(TWO_BODIES);
    let bodies = mass_properties(&model, None).unwrap().bodies;
    let features: Vec<&str> = bodies.iter().map(|b| b.feature.as_str()).collect();
    assert_eq!(features, ["aaaaaaaa", "bbbbbbbb"]);
    for body in &bodies {
        let held = fingerprint(&model, body.feature.as_str());
        assert_eq!((body.volume, body.area), (held.volume, held.area));
    }
    // One feature's own solid, a body or not: the example's box, which its cut consumed.
    let model = model_of(EXAMPLE);
    let every = mass_properties(&model, None).unwrap().bodies;
    assert_eq!(
        every.iter().map(|b| b.feature.as_str()).collect::<Vec<_>>(),
        ["m3tq8v0z"]
    );
    let block = mass_properties(&model, Some(&id("k7f2qx3m")))
        .unwrap()
        .bodies;
    assert_eq!(block.len(), 1);
    assert_eq!(block[0].feature, id("k7f2qx3m"));
    assert_eq!(block[0].volume, fingerprint(&model, "k7f2qx3m").volume);
}

const SKETCH_AND_BOX: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "Outline", "plane": "xy", "points": [], "lines": [], "circles": [], "arcs": [], "constraints": []}, {"id": "k7f2qx3m", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}]}"#;

#[test]
fn a_solid_that_cant_be_read_is_refused_as_elements_refuses_it() {
    let model = model_of(SKETCH_AND_BOX);
    let sketch = mass_properties(&model, Some(&id("s0000001"))).unwrap_err();
    assert_eq!(
        codes(&sketch),
        [(ErrorCode::InvalidInput, Some("feature".to_owned()))]
    );
    assert_eq!(
        sketch.errors()[0].message(),
        "s0000001 is a sketch, which makes no solid"
    );
    let missing = mass_properties(&model, Some(&id("zzzzzzzz"))).unwrap_err();
    assert_eq!(
        missing.errors()[0].message(),
        "the document has no feature zzzzzzzz"
    );
    assert_eq!(missing.errors()[0].parameter(), Some("feature"));

    // A feature that failed: its failure; one that used it: blocked, naming it.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().refuse(|op| {
        (op.name() == "MakeCylinder")
            .then(|| silico_core::Error::new(ErrorCode::InvalidInput, "no pins today"))
    });
    let model = published(&engine, checked(EXAMPLE));
    let failed = mass_properties(&model, Some(&id("a91cw0de"))).unwrap_err();
    assert_eq!(failed.errors()[0].message(), "no pins today");
    let blocked = mass_properties(&model, Some(&id("m3tq8v0z"))).unwrap_err();
    assert_eq!(
        blocked.errors()[0].message(),
        "m3tq8v0z didn't build, since it uses a91cw0de, which failed"
    );
    assert_eq!(blocked.errors()[0].parameter(), Some("feature"));
}

#[test]
fn a_feature_a_fault_stopped_or_blocked_is_refused_with_the_fault_as_elements_refuses_it() {
    // The worker crashes making the pin: the pin fails with the fault, and the cut that
    // uses it is blocked on it. Either's mass properties are refused with the fault itself,
    // naming the pin, which met it (the design's section 6), as the revision keeps it.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().refuse(|op| {
        (op.name() == "MakeCylinder")
            .then(|| silico_core::Error::new(ErrorCode::WorkerCrashed, "crashed"))
    });
    let model = published(&engine, checked(EXAMPLE));
    let fault = (
        ErrorCode::WorkerCrashed,
        "crashed".to_owned(),
        Some("a91cw0de".to_owned()),
        Some(
            "reading this revision again won't, since it keeps the fault; an edit that changes the document, reopening the session, or (M4.3) check with fresh tries again; a save that writes only fingerprints keeps the fault"
                .to_owned(),
        ),
    );
    for feature in ["a91cw0de", "m3tq8v0z"] {
        let refused = mass_properties(&model, Some(&id(feature))).unwrap_err();
        assert_eq!(refused.total(), 1, "{feature}");
        let e = &refused.errors()[0];
        let said = (
            e.code(),
            e.message().to_owned(),
            e.feature().map(str::to_owned),
            e.suggestion().map(str::to_owned),
        );
        assert_eq!(said, fault, "{feature}");
        let entries = silico_core::command::elements::entries(&model, &id(feature)).unwrap_err();
        assert_eq!(entries.errors(), refused.errors(), "{feature}");
    }
}

#[test]
fn a_solid_whose_summary_the_kernel_refused_is_refused_with_its_refusal_and_each_is_said() {
    // Every summary refused for its values: each body's refusal, in timeline order, each
    // naming its feature and not the request's field, which was null.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().refuse(|op| {
        (op.name() == "Summarize").then(|| {
            silico_core::Error::new(ErrorCode::InvalidResult, "the shape can't be measured")
        })
    });
    let model = published(&engine, checked(TWO_BODIES));
    let refused = mass_properties(&model, None).unwrap_err();
    let said: Vec<(Option<&str>, Option<&str>)> = refused
        .errors()
        .iter()
        .map(|e| (e.feature(), e.parameter()))
        .collect();
    assert_eq!(
        said,
        [(Some("aaaaaaaa"), None), (Some("bbbbbbbb"), None)],
        "{:?}",
        refused.errors()
    );
    assert_eq!(refused.total(), 2);
}

#[test]
fn a_solid_a_worker_failed_to_measure_is_refused_with_that_failure_as_check_says_it() {
    // Found by the robustness and architecture reviews: a worker that crashed, timed out,
    // or ran short measuring a solid (kept unmeasured, not as a refusal) was refused as
    // `kernel_failed`, "couldn't be measured", with no cause: the kernel refusing the
    // geometry, to an agent, where `check` says the worker's own failure.
    for code in [
        ErrorCode::WorkerCrashed,
        ErrorCode::WorkerTimeout,
        ErrorCode::ResourceExhausted,
    ] {
        let engine = Recompute::new(FakeKernel::new(2).unwrap());
        engine.kernel().refuse(move |op| {
            (op.name() == "Summarize").then(|| {
                silico_core::Error::new(code, "the worker failed")
                    .with_suggestion("check again: a new worker is started")
            })
        });
        let model = published(&engine, checked(TWO_BODIES));
        let check: Vec<ErrorCode> = model.drift().iter().map(|e| e.code()).collect();
        assert_eq!(check, [code, code]);
        let refused = mass_properties(&model, None).unwrap_err();
        let said: Vec<(ErrorCode, Option<&str>, Option<&str>)> = refused
            .errors()
            .iter()
            .map(|e| (e.code(), e.feature(), e.suggestion()))
            .collect();
        // The worker's own advice, then that the revision keeps the fault, as `elements`
        // gives it for the same solid.
        let advice = Some(
            "check again: a new worker is started; to try again here: reading this revision again won't, since it keeps the fault; an edit that changes the document, reopening the session, or (M4.3) check with fresh tries again; a save that writes only fingerprints keeps the fault",
        );
        assert_eq!(
            said,
            [
                (code, Some("aaaaaaaa"), advice),
                (code, Some("bbbbbbbb"), advice)
            ],
            "{code:?}"
        );
    }
}

#[test]
fn each_solid_a_worker_failed_to_measure_gives_its_own_failure() {
    // Found by the tests review: a `Model::unmeasured` that gave every feature the first
    // one's failure passed every test, each body's failure being alike. One body's worker
    // crashed, the other's timed out: each is said as its own.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().refuse(|op| match op {
        Op::Summarize { shape } => Some(
            if FakeKernel::describe(shape).is_some_and(|d| d.contains("5, 0, 0")) {
                silico_core::Error::new(ErrorCode::WorkerTimeout, "timed out")
            } else {
                silico_core::Error::new(ErrorCode::WorkerCrashed, "crashed")
            },
        ),
        _ => None,
    });
    let model = published(&engine, checked(TWO_BODIES));
    let refused = mass_properties(&model, None).unwrap_err();
    let codes: Vec<_> = refused
        .errors()
        .iter()
        .map(|e| (e.code(), e.feature()))
        .collect();
    assert_eq!(
        codes,
        [
            (ErrorCode::WorkerCrashed, Some("aaaaaaaa")),
            (ErrorCode::WorkerTimeout, Some("bbbbbbbb"))
        ]
    );
}

#[test]
fn entries_of_a_solid_a_worker_failed_to_measure_give_that_failure() {
    // Found by the tests review: `elements` reads a worker's failure to measure a solid
    // too, as `check` says it; with that read gone, it said `kernel_failed`, "couldn't be
    // described", and every test passed.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().refuse(|op| {
        (op.name() == "Summarize")
            .then(|| silico_core::Error::new(ErrorCode::WorkerCrashed, "crashed"))
    });
    let model = published(&engine, checked(TWO_BODIES));
    let refused = silico_core::command::elements::entries(&model, &id("bbbbbbbb")).unwrap_err();
    let e = &refused.errors()[0];
    assert_eq!(
        (e.code(), e.feature(), e.parameter()),
        (ErrorCode::WorkerCrashed, Some("bbbbbbbb"), Some("feature"))
    );
}

#[test]
fn a_summary_refused_for_a_feature_the_request_named_names_its_feature_as_elements_does() {
    // Found by the quality and robustness reviews: the refusal named the feature but not
    // the request's `feature`, where `elements` names both for the same solid.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().refuse(|op| {
        (op.name() == "Summarize").then(|| {
            silico_core::Error::new(ErrorCode::InvalidResult, "the shape can't be measured")
        })
    });
    let model = published(&engine, checked(TWO_BODIES));
    let refused = mass_properties(&model, Some(&id("bbbbbbbb"))).unwrap_err();
    let said: Vec<(ErrorCode, Option<&str>, Option<&str>)> = refused
        .errors()
        .iter()
        .map(|e| (e.code(), e.feature(), e.parameter()))
        .collect();
    assert_eq!(
        said,
        [(ErrorCode::InvalidResult, Some("bbbbbbbb"), Some("feature"))]
    );
    let listed = silico_core::command::elements::entries(&model, &id("bbbbbbbb")).unwrap_err();
    let listed: Vec<(ErrorCode, Option<&str>, Option<&str>)> = listed
        .errors()
        .iter()
        .map(|e| (e.code(), e.feature(), e.parameter()))
        .collect();
    assert_eq!(listed, said);
}

#[test]
fn on_a_session_it_says_which_model_it_read_and_a_published_read_waits_for_one() {
    let f = fixture();
    let s = f.open("two.silico", TWO_BODIES);
    let none = answer(f.sessions.mass_properties(&s, ModelChoice::Published, None));
    assert_eq!(
        none.result.unwrap_err().errors()[0].parameter(),
        Some("model")
    );
    let current = answer(f.sessions.mass_properties(&s, ModelChoice::Current, None));
    assert_eq!(
        current.model,
        Some(ModelRead {
            revision: Revision::OPENED,
            of: ModelChoice::Current,
            fault: None,
        })
    );
    assert_eq!(current.result.unwrap().bodies.len(), 2);
    let published = answer(f.sessions.mass_properties(
        &s,
        ModelChoice::Published,
        Some(id("bbbbbbbb")),
    ));
    assert_eq!(
        published.model,
        Some(ModelRead {
            revision: Revision::OPENED,
            of: ModelChoice::Published,
            fault: None,
        })
    );
    assert_eq!(published.result.unwrap().bodies[0].feature, id("bbbbbbbb"));
}
