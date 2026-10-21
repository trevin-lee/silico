//! break-it: a measure the kernel refuses for the solid's values, kept with the result in
//! memory and in the rebuild cache (5065d58). Hostile cases: features that share one step,
//! a refusal among measures, a transient failure followed by a refusal, a measure superseded
//! while it runs, sessions racing on one cache, refusals with every field at an edge, many
//! features at once, and entries with a sound seal that keep what no worker's reply would
//! give (a code never kept, a count under the elements listed, a summary no shape has).
//! A property test runs every pattern of codes over the example through four sessions.
//! Expected values are written by hand: which codes are kept (the rule as decided after the
//! commit: in memory, `kernel_failed`, `invalid_result`, and `invalid_input`; on disk, only
//! `invalid_result` and `invalid_input`, since OCCT's Gauss and Kronrod point computations
//! catch every `Standard_Failure`, out of memory included, so a `kernel_failed` measure may be
//! transient and must not outlive a restart), which kernel calls each recompute makes, and
//! the refusals the fake gives.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/cache.rs"]
mod cache;
#[path = "common/example.rs"]
mod example;
#[path = "common/model.rs"]
mod model;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cache::{HOLE, Scratch, calls, files, hole, secret_of, session};
use example::EXAMPLE;
use hmac::{Hmac, Mac};
use model::{WAIT, checked, id, published};
use proptest::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use silico_core::command;
use silico_core::document::FeatureId;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{History, Op, ShapeSummary};
use silico_core::recompute::{Built, FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};

const MAGIC: &[u8] = b"silico cache 5\n";

/// The codes kept for the session: those that may follow from the solid's values.
const KEPT: [ErrorCode; 3] = [
    ErrorCode::KernelFailed,
    ErrorCode::InvalidResult,
    ErrorCode::InvalidInput,
];

/// The codes kept on disk, past a restart: those that surely follow from the solid's values.
/// `kernel_failed` isn't among them: OCCT's integration reports a memory shortage as a plain
/// failure, so it may be transient.
const KEPT_ON_DISK: [ErrorCode; 2] = [ErrorCode::InvalidResult, ErrorCode::InvalidInput];

fn is_kept(code: ErrorCode) -> bool {
    KEPT.contains(&code)
}

fn is_kept_on_disk(code: ErrorCode) -> bool {
    KEPT_ON_DISK.contains(&code)
}

/// How many measures a kernel was asked for.
fn measures(kernel: &FakeKernel) -> usize {
    kernel
        .calls()
        .iter()
        .filter(|call| matches!(call.op, Op::Summarize { .. }))
        .count()
}

/// What a model says of its features' measures: each error's code and feature.
fn said(errors: &[Error]) -> Vec<(ErrorCode, String)> {
    errors
        .iter()
        .map(|e| (e.code(), e.feature().unwrap_or_default().to_owned()))
        .collect()
}

fn refusing(kernel: &FakeKernel, code: ErrorCode) {
    kernel.refuse(move |op| {
        matches!(op, Op::Summarize { .. }).then(|| Error::new(code, "Summarize failed"))
    });
}

/// A kernel that refuses the measure of each shape whose description starts with one of
/// `prefixes`, with that prefix's code.
fn refusing_some(kernel: &FakeKernel, prefixes: Vec<(&'static str, ErrorCode)>) {
    kernel.refuse(move |op| match op {
        Op::Summarize { shape } => {
            let made = FakeKernel::describe(shape)?;
            prefixes
                .iter()
                .find(|(prefix, _)| made.starts_with(prefix))
                .map(|(prefix, code)| {
                    Error::new(*code, format!("Summarize failed on {prefix}"))
                        .with_cause("the fake was told to")
                })
        }
        _ => None,
    });
}

fn built(model: &Model, feature: &str) -> (String, Arc<Built>) {
    match model.outcome(&FeatureId::new(feature).unwrap()) {
        Some(FeatureOutcome::Built { key, built }) => (key.to_string(), Arc::clone(built)),
        other => panic!("{feature}: {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------
// The format, by hand, as tests/adversarial_cache.rs derives it.

fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn entry_path(dir: &Path, step: &str) -> PathBuf {
    let platform = platform();
    let secret = std::fs::read(secret_of(dir)).unwrap();
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&secret).unwrap();
    mac.update(MAGIC);
    for part in ["silico fake kernel 1", &platform, step] {
        mac.update(&(part.len() as u64).to_le_bytes());
        mac.update(part.as_bytes());
    }
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    dir.join(&hex[..2]).join(&hex[2..])
}

/// An error as format 5 keeps it, field for field.
#[derive(Serialize, Deserialize, Clone)]
struct KeptError {
    code: String,
    message: String,
    feature: Option<String>,
    elements: Vec<String>,
    elements_total: u64,
    parameter: Option<String>,
    cause: Option<String>,
    suggestion: Option<String>,
    candidates: Vec<String>,
}

#[derive(Serialize, Deserialize)]
enum Measure {
    Unmeasured,
    Summary(ShapeSummary),
    Refused(KeptError),
}

#[derive(Serialize, Deserialize)]
struct Entry {
    key: [u8; 32],
    brep: Vec<u8>,
    valid: bool,
    history: Option<History>,
    measure: Measure,
}

fn decode(bytes: &[u8]) -> Entry {
    postcard::from_bytes(&bytes[MAGIC.len() + 32..]).unwrap()
}

fn encode(dir: &Path, entry: &Entry) -> Vec<u8> {
    let secret = std::fs::read(secret_of(dir)).unwrap();
    let payload = postcard::to_allocvec(entry).unwrap();
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&secret).unwrap();
    mac.update(&payload);
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&mac.finalize().into_bytes());
    bytes.extend_from_slice(&payload);
    bytes
}

/// Rewrites the block's entry in `dir` (sealed as its own writer would) with `measure`.
fn forge_block(dir: &Path, model: &Model, measure: Measure) -> PathBuf {
    let (key, _) = built(model, "k7f2qx3m");
    let path = entry_path(dir, &key);
    let mut entry = decode(&std::fs::read(&path).unwrap());
    entry.measure = measure;
    std::fs::write(&path, encode(dir, &entry)).unwrap();
    path
}

fn kept_error(code: &str) -> KeptError {
    KeptError {
        code: code.to_owned(),
        message: "Summarize failed".to_owned(),
        feature: None,
        elements: Vec::new(),
        elements_total: 0,
        parameter: None,
        cause: None,
        suggestion: None,
        candidates: Vec::new(),
    }
}

// ---------------------------------------------------------------------------------------
// Steps shared, and refusals among measures.

const HEAD: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "features": ["#;

fn boxes(ids: &[&str]) -> String {
    let features: Vec<String> = ids
        .iter()
        .map(|id| {
            format!(
                r#"{{"id": "{id}", "type": "box", "v": 1, "name": "Block", "position": [0, 0, 0], "length": 40, "width": 20, "height": 10}}"#
            )
        })
        .collect();
    format!("{HEAD}{}]}}\n", features.join(",\n"))
}

#[test]
fn two_features_on_one_step_are_measured_once_and_each_says_the_refusal_as_its_own() {
    let doc = boxes(&["k7f2qx30", "k7f2qx31"]);
    let dir = Scratch::new("shared-step");
    let first = session(&dir.0);
    refusing(first.kernel(), ErrorCode::InvalidInput);
    let model = published(&first, checked(&doc));
    assert_eq!(measures(first.kernel()), 1);
    let expected = vec![
        (ErrorCode::InvalidInput, "k7f2qx30".to_owned()),
        (ErrorCode::InvalidInput, "k7f2qx31".to_owned()),
    ];
    assert_eq!(said(&model.drift()), expected);
    // Again in memory, and from disk: nothing asked, each named.
    published(&first, checked(&doc));
    assert_eq!(measures(first.kernel()), 1);
    let second = session(&dir.0);
    let again = published(&second, checked(&doc));
    assert!(second.kernel().calls().is_empty());
    assert_eq!(again.drift(), model.drift());
}

#[test]
fn one_refusal_among_measures_formats_alike_fresh_and_from_disk() {
    // The pin's measure is refused; the block's and the hole's aren't. `silico fmt` writes
    // their fingerprints, keeps none for the pin, and says the pin's refusal, the same from
    // the disk as fresh, without asking the kernel.
    let dir = Scratch::new("one-of-three");
    let first = session(&dir.0);
    refusing_some(
        first.kernel(),
        vec![("translate(cylinder", ErrorCode::InvalidResult)],
    );
    let model = published(&first, checked(EXAMPLE));
    let fresh = command::format(&model).unwrap().unwrap();
    assert_eq!(
        said(fresh.problems.errors()),
        [(ErrorCode::InvalidResult, "a91cw0de".to_owned())]
    );
    assert!(model.fingerprint(&id("k7f2qx3m")).is_some());
    assert!(model.fingerprint(&id("a91cw0de")).is_none());
    assert!(model.fingerprint(&id("m3tq8v0z")).is_some());
    let second = session(&dir.0);
    let again = published(&second, checked(EXAMPLE));
    assert!(second.kernel().calls().is_empty(), "{:?}", calls(&second));
    let cached = command::format(&again).unwrap().unwrap();
    assert_eq!(cached, fresh);
    // The formatted text, read back: only the pin's refusal is a problem, and `check` says it
    // without asking.
    let third = session(&dir.0);
    let reread = published(&third, checked(&fresh.text));
    assert!(third.kernel().calls().is_empty());
    let problems = command::check(&reread).unwrap();
    assert_eq!(
        said(problems.errors()),
        [(ErrorCode::InvalidResult, "a91cw0de".to_owned())]
    );
}

#[test]
fn a_hang_then_a_refusal_then_nothing_is_asked() {
    // Session one's worker hangs on every measure: written unmeasured. Session two's
    // refuses them for their values: written refused. Session three asks nothing, and says
    // session two's refusals.
    let dir = Scratch::new("hang-then-refusal");
    let first = session(&dir.0);
    refusing(first.kernel(), ErrorCode::WorkerTimeout);
    published(&first, checked(EXAMPLE));
    let second = session(&dir.0);
    refusing(second.kernel(), ErrorCode::InvalidInput);
    let model = published(&second, checked(EXAMPLE));
    assert_eq!(measures(second.kernel()), 3);
    assert!(calls(&second).is_empty());
    let third = session(&dir.0);
    let again = published(&third, checked(EXAMPLE));
    assert!(third.kernel().calls().is_empty(), "{:?}", calls(&third));
    assert_eq!(again.drift(), model.drift());
    assert_eq!(said(&again.drift()).len(), 3);
    assert!(
        again
            .drift()
            .iter()
            .all(|e| e.code() == ErrorCode::InvalidInput)
    );
}

// BREAKS until the rule decided after the commit is in: cache.rs writes a `kernel_failed`
// refusal to disk, so session two asks nothing and says the refusal a memory shortage may
// have caused.
#[test]
fn a_kernel_failed_measure_is_kept_for_the_session_and_asked_again_after_a_restart() {
    // Session one refuses every measure with `kernel_failed`: asked once, and a second
    // recompute in the session asks nothing. Session two, on the same cache, asks all three
    // again: its kernel measures them, so the model has fingerprints and nothing drifts, and
    // what it writes holds for session three, which asks nothing.
    let dir = Scratch::new("kernel-failed-restart");
    let first = session(&dir.0);
    refusing(first.kernel(), ErrorCode::KernelFailed);
    let model = published(&first, checked(EXAMPLE));
    assert_eq!(measures(first.kernel()), 3);
    assert_eq!(
        said(&model.drift()),
        [
            (ErrorCode::KernelFailed, "k7f2qx3m".to_owned()),
            (ErrorCode::KernelFailed, "a91cw0de".to_owned()),
            (ErrorCode::KernelFailed, "m3tq8v0z".to_owned()),
        ]
    );
    published(&first, checked(EXAMPLE));
    assert_eq!(measures(first.kernel()), 3);
    let second = session(&dir.0);
    let again = published(&second, checked(EXAMPLE));
    assert!(calls(&second).is_empty(), "{:?}", calls(&second));
    assert_eq!(measures(second.kernel()), 3);
    assert!(again.drift().is_empty(), "{:?}", again.drift());
    for feature in ["k7f2qx3m", "a91cw0de", "m3tq8v0z"] {
        assert!(again.fingerprint(&id(feature)).is_some(), "{feature}");
    }
    let third = session(&dir.0);
    published(&third, checked(EXAMPLE));
    assert!(third.kernel().calls().is_empty());
}

#[test]
fn a_refusal_with_every_field_at_its_edge_comes_back_from_disk_as_it_was_said() {
    // A refusal naming ten thousand elements of a total past them, a cause with a newline,
    // a NUL, and an emoji, an empty suggestion and parameter, candidates, and a feature the
    // kernel named wrongly: the disk gives back what the fresh model said, to the field,
    // and each feature is named as itself, not as the kernel named it.
    let dir = Scratch::new("every-field");
    let elements: Vec<String> = (1..=10_000).map(|n| format!("Face{n}")).collect();
    let refusal = Error::new(ErrorCode::InvalidInput, "Summarize failed: \u{1F4A5}")
        .with_feature("zzzzzzzz")
        .with_elements(elements.clone())
        .with_elements_total(u64::MAX)
        .with_parameter("")
        .with_cause("line one\nline two\0after a NUL")
        .with_suggestion("")
        .with_candidates(["Edge1", ""]);
    let first = session(&dir.0);
    let given = refusal.clone();
    first
        .kernel()
        .refuse(move |op| matches!(op, Op::Summarize { .. }).then(|| given.clone()));
    let model = published(&first, checked(EXAMPLE));
    let fresh = model.drift();
    assert_eq!(fresh.len(), 3);
    for (error, feature) in fresh.iter().zip(["k7f2qx3m", "a91cw0de", "m3tq8v0z"]) {
        assert_eq!(error.feature(), Some(feature));
        assert_eq!(error.elements(), &elements[..]);
        assert_eq!(error.elements_total(), u64::MAX);
        assert_eq!(error.cause(), Some("line one\nline two\0after a NUL"));
        assert_eq!(error.suggestion(), Some(""));
        assert_eq!(error.parameter(), Some(""));
        assert_eq!(error.candidates(), ["Edge1", ""]);
        assert_eq!(error.message(), "Summarize failed: \u{1F4A5}");
    }
    let second = session(&dir.0);
    let again = published(&second, checked(EXAMPLE));
    assert!(second.kernel().calls().is_empty());
    assert_eq!(again.drift(), fresh);
}

#[test]
fn many_features_refused_are_each_asked_once_in_memory_and_from_disk() {
    let n = 300;
    let features: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"id": "b{i:07}", "type": "box", "v": 1, "name": "B", "position": [{i}, 0, 0], "length": 1, "width": 1, "height": 1}}"#
            )
        })
        .collect();
    let doc = format!("{HEAD}{}]}}\n", features.join(",\n"));
    let dir = Scratch::new("many");
    let first = session(&dir.0);
    refusing(first.kernel(), ErrorCode::InvalidInput);
    let model = published(&first, checked(&doc));
    assert_eq!(measures(first.kernel()), n);
    assert_eq!(model.drift().len(), n);
    let asked = first.kernel().calls().len();
    published(&first, checked(&doc));
    assert_eq!(first.kernel().calls().len(), asked);
    let second = session(&dir.0);
    let again = published(&second, checked(&doc));
    assert!(second.kernel().calls().is_empty());
    assert_eq!(again.drift(), model.drift());
}

// ---------------------------------------------------------------------------------------
// Timing: a measure superseded while it runs, and sessions racing.

// BREAKS: a superseded recompute keeps what it made (`Engine::keep`, recompute.rs) even when
// it ends after a newer one has kept the same step refused, and puts its own copy, neither
// measured nor refused, in its place: the next recompute asks the kernel for the measure
// again. The older one's measures are held; the newer one starts (which ends them
// superseded) and refuses its own. Which run keeps last is a race: about one in seventy alone,
// more under load, so the case runs 500 times. The race predates the commit: with nothing
// refused, a measured copy is overwritten by an unmeasured one the same way (10 rounds in
// 500 asked a measured solid again, checked by hand); here it breaks "asked once". Disk
// holds the refusal (a superseded measure writes nothing), so only memory is wrong, and
// only by a call a solid.
#[test]
fn a_superseded_recompute_finishing_last_doesnt_undo_a_kept_refusal() {
    let mut undone = Vec::new();
    for round in 0..500 {
        let engine = Recompute::new(FakeKernel::new(2).unwrap());
        refusing(engine.kernel(), ErrorCode::KernelFailed);
        let hold = engine
            .kernel()
            .hold(|op| matches!(op, Op::Summarize { .. }));
        let older = engine.start(checked(EXAMPLE));
        assert!(engine.kernel().wait_running(1, WAIT));
        let newer = engine.start(checked(EXAMPLE));
        hold.release();
        match older.wait_timeout(WAIT) {
            Ok(Ok(RunOutcome::Superseded)) => {}
            Ok(other) => panic!("the older recompute: {other:?}"),
            Err(_) => panic!("the older recompute hung"),
        }
        let model = match newer.wait_timeout(WAIT) {
            Ok(Ok(RunOutcome::Published(model))) => model,
            Ok(other) => panic!("the newer recompute: {other:?}"),
            Err(_) => panic!("the newer recompute hung"),
        };
        assert_eq!(said(&model.drift()).len(), 3);
        assert_eq!(hole(&model), HOLE);
        let asked = engine.kernel().calls().len();
        let again = published(&engine, checked(EXAMPLE));
        assert_eq!(again.drift(), model.drift());
        let more: Vec<String> = engine.kernel().calls()[asked..]
            .iter()
            .map(|call| match &call.op {
                Op::Summarize { shape } => {
                    format!(
                        "Summarize {}",
                        FakeKernel::describe(shape).unwrap_or_default()
                    )
                }
                op => op.name().to_owned(),
            })
            .collect();
        if !more.is_empty() {
            undone.push((round, more));
        }
    }
    assert!(undone.is_empty(), "asked again, by round: {undone:?}");
}

#[test]
fn sessions_racing_to_keep_refusals_leave_entries_a_third_asks_nothing_of() {
    let dir = Scratch::new("racing");
    for _ in 0..10 {
        let threads: Vec<_> = [ErrorCode::InvalidResult, ErrorCode::InvalidResult]
            .into_iter()
            .map(|code| {
                let dir = dir.0.clone();
                std::thread::spawn(move || {
                    let engine = session(&dir);
                    refusing(engine.kernel(), code);
                    published(&engine, checked(EXAMPLE)).drift()
                })
            })
            .collect();
        let drifts: Vec<Vec<Error>> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(drifts[0], drifts[1]);
        let third = session(&dir.0);
        let model = published(&third, checked(EXAMPLE));
        assert!(third.kernel().calls().is_empty(), "{:?}", calls(&third));
        assert_eq!(model.drift(), drifts[0]);
        // Cleared for the next round.
        for file in files(&dir.0) {
            std::fs::remove_file(file).unwrap();
        }
    }
}

#[test]
fn a_kept_refusal_holds_while_the_document_is_edited_away_and_back() {
    // Three other documents in between push the example's results out of memory (the
    // engine keeps two published models'); the cache still has them.
    let dir = Scratch::new("away-and-back");
    let engine = session(&dir.0);
    refusing(engine.kernel(), ErrorCode::InvalidResult);
    let model = published(&engine, checked(EXAMPLE));
    for radius in ["1", "2", "3"] {
        published(&engine, cache::example_with_pin(radius));
    }
    let asked = measures(engine.kernel());
    let back = published(&engine, checked(EXAMPLE));
    assert_eq!(measures(engine.kernel()), asked);
    assert_eq!(back.drift(), model.drift());
}

// ---------------------------------------------------------------------------------------
// Entries with a sound seal that keep what no worker's reply gives.

#[test]
fn a_kept_refusal_of_a_code_this_silico_doesnt_know_is_a_miss_rebuilt_and_written_over() {
    let dir = Scratch::new("unknown-code");
    let model = published(&session(&dir.0), checked(EXAMPLE));
    let path = forge_block(&dir.0, &model, Measure::Refused(kept_error("a_later_code")));
    let next = session(&dir.0);
    let again = published(&next, checked(EXAMPLE));
    assert_eq!(calls(&next), ["MakeBox"]);
    assert!(again.drift().is_empty());
    assert!(again.fingerprint(&id("k7f2qx3m")).is_some());
    assert!(matches!(
        decode(&std::fs::read(&path).unwrap()).measure,
        Measure::Summary(_)
    ));
    let third = session(&dir.0);
    published(&third, checked(EXAMPLE));
    assert!(third.kernel().calls().is_empty());
}

// BREAKS: `built_of` (cache.rs) takes back a refusal of any code it knows, though only
// `invalid_result` and `invalid_input` are kept on disk (`kernel_failed` only for the
// session, under the rule decided after the commit). An entry keeping a refusal of a code that's never kept (a hang's, a
// crash's, a bug's) is taken as kept: the solid is never measured again, and every
// `silico check` of the document fails as the kernel's fault (`check` returns the error,
// not problems) until the cache is deleted. The core decides what's kept from a worker's
// reply; read back, it isn't checked again (the catalog: "whatever the core validates from a
// worker must be validated when read back from disk"). Expected: a miss of the measure (or
// of the entry), so the solid is measured.
#[test]
fn a_kept_refusal_of_a_code_never_kept_is_measured_again() {
    for code in ErrorCode::ALL.iter().filter(|c| !is_kept_on_disk(**c)) {
        let dir = Scratch::new(&format!("never-kept-{}", code.as_str()));
        let model = published(&session(&dir.0), checked(EXAMPLE));
        forge_block(&dir.0, &model, Measure::Refused(kept_error(code.as_str())));
        let next = session(&dir.0);
        let again = published(&next, checked(EXAMPLE));
        assert!(
            again.drift().is_empty(),
            "{code:?}: a refusal never kept, taken from the disk: {:?}",
            again.drift()
        );
        assert!(
            again.fingerprint(&id("k7f2qx3m")).is_some(),
            "{code:?}: not measured"
        );
    }
}

// BREAKS: the same root cause: an entry keeping a refusal that names more elements than its
// total is taken back as it is, so `Error::elements_total` is under `elements().len()`,
// which every `Error` the core builds rules out ("never fewer than it names").
#[test]
fn a_kept_refusal_whose_total_is_under_the_elements_it_names_keeps_the_errors_rule() {
    let dir = Scratch::new("total-under");
    let model = published(&session(&dir.0), checked(EXAMPLE));
    let mut refusal = kept_error("invalid_input");
    refusal.elements = vec!["Face1".into(), "Face2".into(), "Face3".into()];
    refusal.elements_total = 0;
    forge_block(&dir.0, &model, Measure::Refused(refusal));
    let next = session(&dir.0);
    let again = published(&next, checked(EXAMPLE));
    for error in again.drift() {
        assert!(
            error.elements_total() >= error.elements().len() as u64,
            "{error:?}"
        );
    }
}

// BREAKS (before this commit, in M2.6, and kept by it): a kept summary is taken back as
// it is, though the core refuses one no shape has from a worker (`summary_fault`, as a
// protocol error, asked again). An entry with a NaN volume gives no fingerprint, and the
// solid is never measured again: a feature with a fingerprint stored drifts forever, and
// one without never gets one. Expected: a miss of the measure, so the solid is measured.
#[test]
fn a_kept_summary_no_shape_has_is_measured_again() {
    let dir = Scratch::new("nan-summary");
    let model = published(&session(&dir.0), checked(EXAMPLE));
    let (_, block) = built(&model, "k7f2qx3m");
    let mut summary = block.summary().unwrap().clone();
    summary.volume = f64::NAN;
    forge_block(&dir.0, &model, Measure::Summary(summary));
    let next = session(&dir.0);
    let again = published(&next, checked(EXAMPLE));
    assert!(
        again.fingerprint(&id("k7f2qx3m")).is_some(),
        "a summary with a NaN volume, taken from the disk; asked: {:?}",
        next.kernel().calls().len()
    );
}

// ---------------------------------------------------------------------------------------
// Every pattern of codes, through four sessions on one cache.

static CASE: AtomicUsize = AtomicUsize::new(0);

/// The example's features' solids, by how the fake describes them, in timeline order.
const SOLIDS: [(&str, &str); 3] = [
    ("box", "k7f2qx3m"),
    ("translate(cylinder", "a91cw0de"),
    ("cut", "m3tq8v0z"),
];

fn code() -> impl Strategy<Value = Option<ErrorCode>> {
    prop_oneof![
        3 => Just(None),
        6 => prop::sample::select(KEPT.to_vec()).prop_map(Some),
        3 => prop::sample::select(ErrorCode::ALL.to_vec()).prop_map(Some),
    ]
}

// BREAKS until the rule decided after the commit is in: a `kernel_failed` refusal is kept on
// disk, so the sessions after the first ask fewer measures than `retried_after_restart`.
proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, ..ProptestConfig::default() })]

    #[test]
    fn every_pattern_of_refusals_is_asked_as_often_as_its_codes_say(
        codes in prop::collection::vec(code(), 3),
    ) {
        let rules: Vec<(&'static str, ErrorCode)> = SOLIDS
            .iter()
            .zip(&codes)
            .filter_map(|((prefix, _), code)| code.map(|c| (*prefix, c)))
            .collect();
        let retried = rules.iter().filter(|(_, c)| !is_kept(*c)).count();
        let retried_after_restart = rules.iter().filter(|(_, c)| !is_kept_on_disk(*c)).count();
        let expected: Vec<(ErrorCode, String)> = SOLIDS
            .iter()
            .zip(&codes)
            .filter_map(|((_, feature), code)| code.map(|c| (c, (*feature).to_owned())))
            .collect();
        let kept_only: Vec<(ErrorCode, String)> =
            expected.iter().filter(|(c, _)| is_kept_on_disk(*c)).cloned().collect();
        let dir = Scratch::new(&format!("pattern-{}", CASE.fetch_add(1, Ordering::Relaxed)));

        // One: every measure asked, then only those not kept.
        let one = session(&dir.0);
        refusing_some(one.kernel(), rules.clone());
        let first = published(&one, checked(EXAMPLE));
        prop_assert_eq!(measures(one.kernel()), 3);
        prop_assert_eq!(said(&first.drift()), expected.clone());
        let again = published(&one, checked(EXAMPLE));
        prop_assert_eq!(measures(one.kernel()), 3 + retried);
        prop_assert_eq!(again.drift(), first.drift());
        prop_assert_eq!(hole(&again), HOLE);

        // Two: the same kernel, from the disk: nothing made, the ones not kept on disk asked.
        let two = session(&dir.0);
        refusing_some(two.kernel(), rules.clone());
        let second = published(&two, checked(EXAMPLE));
        prop_assert!(calls(&two).is_empty());
        prop_assert_eq!(measures(two.kernel()), retried_after_restart);
        prop_assert_eq!(second.drift(), first.drift());

        // Three: a kernel that refuses nothing: the ones not kept on disk are measured now,
        // the kept ones still said.
        let three = session(&dir.0);
        let third = published(&three, checked(EXAMPLE));
        prop_assert_eq!(measures(three.kernel()), retried_after_restart);
        prop_assert_eq!(said(&third.drift()), kept_only.clone());
        for ((_, feature), code) in SOLIDS.iter().zip(&codes) {
            let refused_for_good = code.is_some_and(is_kept_on_disk);
            prop_assert_eq!(third.fingerprint(&id(feature)).is_none(), refused_for_good);
        }

        // Four: nothing asked at all, and the same said.
        let four = session(&dir.0);
        let fourth = published(&four, checked(EXAMPLE));
        prop_assert!(four.kernel().calls().is_empty());
        prop_assert_eq!(fourth.drift(), third.drift());
    }
}

#[test]
fn a_refused_measures_drift_is_actionable() {
    // Through the fake, worded as the worker words a refusal (a code, "Summarize failed", its
    // cause, its suggestion): kept on disk, the error still says what failed and what to do.
    let dir = Scratch::new("actionable");
    let first = session(&dir.0);
    first.kernel().refuse(|op| {
        matches!(op, Op::Summarize { .. }).then(|| {
            Error::new(ErrorCode::InvalidResult, "Summarize failed")
                .with_cause("BRepGProp: integration didn't converge")
                .with_suggestion(
                    "change the operation's values: the kernel can't build it from these",
                )
        })
    });
    published(&first, checked(EXAMPLE));
    let second = session(&dir.0);
    let model = published(&second, checked(EXAMPLE));
    assert!(second.kernel().calls().is_empty());
    assert_eq!(model.drift().len(), 3);
    for error in model.drift() {
        assert!(error.feature().is_some());
        assert_eq!(
            error.cause(),
            Some("BRepGProp: integration didn't converge")
        );
        assert!(error.suggestion().is_some());
    }
}
