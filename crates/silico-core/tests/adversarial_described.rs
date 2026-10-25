//! break-it: M4.4's descriptions made in the rebuild (`Engine::measure`, the cache's
//! `Measure::Summary`, `Model::description`). A later batch of a solid's edges refused or
//! failing after the earlier ones and the summary came back; recomputes superseded and
//! cancelled while a solid is half described; and forged cache entries, sealed as their
//! writer would seal them, holding descriptions no worker could send (values out of range)
//! or that aren't the solid's (an element short, one too many, another solid's, a
//! hundred thousand vertices). Expected values are counts by hand (a box has 6 faces, 12
//! edges, and 8 vertices; the fake keeps every box's in a fusion of separate boxes) and
//! the fake's description of element n, which stands for n alone (`fake::described`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::cache;
use super::example;
use super::model;
use super::refusals;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use cache::{HOLE, Scratch, calls, example_with_pin, files, hole, secret_of, session};
use example::EXAMPLE;
use hmac::{Hmac, Mac};
use model::{WAIT, checked, id, published};
use refusals::worker_failure;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use silico_core::ErrorCode;
use silico_core::elements::elements;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{ElementKind, History, Op, ShapeSummary};
use silico_core::recompute::{Built, FeatureOutcome, Model, Recompute, RunOutcome};
use silico_protocol::{Description, EdgeCurve, Failure, Surface};

const MAGIC: &[u8] = b"silico cache 7\n";

/// The example's three features.
const FEATURES: [&str; 3] = ["k7f2qx3m", "a91cw0de", "m3tq8v0z"];

fn built(model: &Model, feature: &str) -> (String, Arc<Built>) {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { key, built }) => (key.to_string(), Arc::clone(built)),
        other => panic!("{feature}: {other:?}"),
    }
}

/// How many requests of `op` a kernel was asked.
fn asked(kernel: &FakeKernel, op: &str) -> usize {
    kernel
        .calls()
        .iter()
        .filter(|call| call.op.name() == op)
        .count()
}

/// The faces, edges, and vertices a description holds.
fn lengths(d: &Description) -> [usize; 3] {
    [d.faces.len(), d.edges.len(), d.vertices.len()]
}

/// Asserts the feature's published description is whole and the fake's: as many of each
/// kind as its summary counts and its element map names, element n the fake's n, and
/// `elements` lists them all.
#[track_caller]
fn assert_described_whole(model: &Model, feature: &str) {
    let (_, built) = built(model, feature);
    let d = model
        .description(&id(feature))
        .unwrap_or_else(|| panic!("{feature} isn't described"));
    let summary = built.summary().unwrap();
    assert_eq!(
        lengths(d),
        [summary.faces, summary.edges, summary.vertices].map(|n| n as usize),
        "{feature}: against its summary"
    );
    let map = model.element_map(&id(feature)).unwrap();
    assert_eq!(
        lengths(d),
        [ElementKind::Face, ElementKind::Edge, ElementKind::Vertex].map(|k| map.count(k)),
        "{feature}: against its element map"
    );
    for (i, face) in d.faces.iter().enumerate() {
        assert_eq!(
            face.centroid,
            [(i + 1) as f64, 0.0, 0.0],
            "{feature} face {i}"
        );
    }
    for (i, edge) in d.edges.iter().enumerate() {
        assert_eq!(edge.start, [(i + 1) as f64, 0.0, 0.0], "{feature} edge {i}");
    }
    for (i, vertex) in d.vertices.iter().enumerate() {
        assert_eq!(*vertex, [(i + 1) as f64, 0.0, 0.0], "{feature} vertex {i}");
    }
    let listed = elements(map, d.clone()).unwrap();
    assert_eq!(listed.len(), lengths(d).iter().sum::<usize>(), "{feature}");
}

/// A document of `n` boxes 20 apart along x, each fused onto the fusion of those before:
/// fusion `f{i}` holds `i + 1` boxes' elements.
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

/// Whether `op` asks for a batch of edges after the first.
fn later_edge_batch(op: &Op) -> bool {
    matches!(
        op,
        Op::Describe {
            kind: ElementKind::Edge,
            first,
            ..
        } if *first > 1
    )
}

/// Waits until the kernel has started a request `picks`, failing after ten seconds.
fn wait_for_call(kernel: &FakeKernel, picks: impl Fn(&Op) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !kernel.calls().iter().any(|call| picks(&call.op)) {
        assert!(Instant::now() < deadline, "the request never started");
        std::thread::sleep(Duration::from_millis(5));
    }
}

// ---------------------------------------------------------------------------------------
// A later batch refused or failing, after the summary and the batches before it.

#[test]
fn a_later_edge_batch_refused_for_its_values_leaves_the_solid_wholly_unmeasured_and_kept_so() {
    // Fusion i holds 12(i + 1) edges: from f0000083 (1,008) on, its edges take two batches.
    // The second refused as invalid input, those fusions are unmeasured (no summary, though
    // it came back, and no description, though its faces and first 1,000 edges did), each
    // reported by drift with the worker's code; every one before is described whole. A
    // second session takes it all back from the cache, the refusals too, asking nothing.
    let dir = Scratch::new("later-batch-refused");
    let first = session(&dir.0);
    let failure = worker_failure(ErrorCode::InvalidInput).unwrap();
    first
        .kernel()
        .refuse_as_worker(move |op| later_edge_batch(op).then(|| failure.clone()));
    let document = fused_boxes(86);
    let model = published(&first, checked(&document));
    let refused: Vec<String> = (83..86).map(|i| format!("f{i:07}")).collect();
    for i in 1..86 {
        let fusion = format!("f{i:07}");
        if refused.contains(&fusion) {
            let (_, built) = built(&model, &fusion);
            assert!(built.summary().is_none(), "{fusion}: a summary kept alone");
            assert!(model.description(&id(&fusion)).is_none(), "{fusion}");
            assert_eq!(
                built.measure_refused().map(silico_core::Error::code),
                Some(ErrorCode::InvalidInput),
                "{fusion}"
            );
        } else {
            assert_described_whole(&model, &fusion);
        }
    }
    let drift: Vec<(ErrorCode, String)> = model
        .drift()
        .iter()
        .map(|e| (e.code(), e.feature().unwrap_or_default().to_owned()))
        .collect();
    assert_eq!(
        drift,
        refused
            .iter()
            .map(|f| (ErrorCode::InvalidInput, f.clone()))
            .collect::<Vec<_>>()
    );
    let second = session(&dir.0);
    let again = published(&second, checked(&document));
    assert!(second.kernel().calls().is_empty(), "{:?}", calls(&second));
    assert_eq!(again.drift(), model.drift());
    for fusion in &refused {
        assert!(again.description(&id(fusion)).is_none(), "{fusion}");
    }
    assert_described_whole(&again, "f0000082");
}

#[test]
fn a_later_edge_batch_a_worker_crashed_on_is_described_again_whole_from_the_first_batch() {
    // The first time each solid's second batch of edges is asked, the worker crashes: the
    // solid is unmeasured, and the next recompute measures it again, every batch from the
    // first, joined in order: 1,020 edges for 85 boxes, the 1,001st edge the fake's 1,001st.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    let crashes = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&crashes);
    engine.kernel().refuse(move |op| {
        (later_edge_batch(op) && counted.fetch_add(1, Ordering::SeqCst) < 2)
            .then(|| silico_core::Error::new(ErrorCode::WorkerCrashed, "the kernel worker crashed"))
    });
    let document = fused_boxes(85);
    let failed = published(&engine, checked(&document));
    let unmeasured: Vec<String> = failed
        .drift()
        .iter()
        .map(|e| e.feature().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(unmeasured, ["f0000083", "f0000084"]);
    for (code, fusion) in failed.drift().iter().map(|e| (e.code(), e.feature())) {
        assert_eq!(code, ErrorCode::WorkerCrashed, "{fusion:?}");
    }
    let summaries = asked(engine.kernel(), "Summarize");
    let again = published(&engine, checked(&document));
    assert!(again.drift().is_empty(), "{:?}", again.drift());
    // Measured again from the summary: two more of them.
    assert_eq!(asked(engine.kernel(), "Summarize"), summaries + 2);
    assert_described_whole(&again, "f0000084");
    assert_eq!(
        lengths(again.description(&id("f0000084")).unwrap()),
        [510, 1020, 680]
    );
}

// ---------------------------------------------------------------------------------------
// Superseded and cancelled while a solid is half described.

/// An entry's payload, as the cache's module documents it (postcard is structural).
#[derive(Serialize, Deserialize)]
struct Entry {
    key: [u8; 32],
    brep: Vec<u8>,
    valid: bool,
    history: Option<History>,
    measure: Measure,
}

#[derive(Serialize, Deserialize)]
enum Measure {
    Unmeasured,
    Summary {
        summary: ShapeSummary,
        description: Description,
    },
    Refused {
        op: String,
        failure: Failure,
    },
}

fn decode(bytes: &[u8]) -> Entry {
    postcard::from_bytes(&bytes[MAGIC.len() + 32..]).unwrap()
}

/// Every entry in the cache in `dir`, each measured one whole: as many of each kind
/// described as its summary counts.
#[track_caller]
fn assert_no_entry_half_measured(dir: &Path) {
    for file in files(dir) {
        let bytes = std::fs::read(&file).unwrap();
        if !bytes.starts_with(MAGIC) {
            continue;
        }
        if let Measure::Summary {
            summary,
            description,
        } = decode(&bytes).measure
        {
            assert_eq!(
                lengths(&description),
                [summary.faces, summary.edges, summary.vertices].map(|n| n as usize),
                "{}",
                file.display()
            );
        }
    }
}

#[test]
fn a_recompute_superseded_mid_describe_leaves_nothing_half_measured_in_memory_or_on_disk() {
    // Every solid's edges are held once its summary and faces are back; a recompute of the
    // example with another pin then supersedes the first. Nothing of the first is published;
    // the second's model is described whole; no entry on disk holds a part of a description;
    // and a session over the cache rebuilds the first document described whole, asking
    // nothing to make it.
    let dir = Scratch::new("superseded-mid-describe");
    let first = session(&dir.0);
    let hold = first.kernel().hold(|op| {
        matches!(
            op,
            Op::Describe {
                kind: ElementKind::Edge,
                ..
            }
        )
    });
    let run = first.start(checked(EXAMPLE));
    wait_for_call(first.kernel(), |op| {
        matches!(
            op,
            Op::Describe {
                kind: ElementKind::Edge,
                ..
            }
        )
    });
    let newer = first.start(example_with_pin("3"));
    hold.release();
    assert!(matches!(
        run.wait_timeout(WAIT),
        Ok(Ok(RunOutcome::Superseded))
    ));
    let model = match newer.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the newer recompute didn't finish in {WAIT:?}"),
    };
    assert!(model.drift().is_empty(), "{:?}", model.drift());
    for feature in FEATURES {
        assert_described_whole(&model, feature);
    }
    assert_no_entry_half_measured(&dir.0);
    let next = session(&dir.0);
    let again = published(&next, checked(EXAMPLE));
    assert_eq!(hole(&again), HOLE);
    assert!(again.drift().is_empty(), "{:?}", again.drift());
    for feature in FEATURES {
        assert_described_whole(&again, feature);
    }
    assert_no_entry_half_measured(&dir.0);
}

#[test]
fn a_recompute_cancelled_mid_describe_publishes_nothing_and_the_next_describes_whole() {
    // As superseded, by cancel: the held batches end superseded, the run publishes
    // nothing, and the next recompute of the same document measures each solid again,
    // whole, keeping no half of the cancelled one's.
    let dir = Scratch::new("cancelled-mid-describe");
    let engine = session(&dir.0);
    let hold = engine.kernel().hold(|op| {
        matches!(
            op,
            Op::Describe {
                kind: ElementKind::Vertex,
                ..
            }
        )
    });
    let run = engine.start(checked(EXAMPLE));
    wait_for_call(engine.kernel(), |op| {
        matches!(
            op,
            Op::Describe {
                kind: ElementKind::Vertex,
                ..
            }
        )
    });
    engine.cancel();
    assert!(matches!(
        run.wait_timeout(WAIT),
        Ok(Ok(RunOutcome::Superseded))
    ));
    assert!(engine.published().is_none());
    hold.release();
    assert_no_entry_half_measured(&dir.0);
    let model = published(&engine, checked(EXAMPLE));
    assert!(model.drift().is_empty(), "{:?}", model.drift());
    for feature in FEATURES {
        assert_described_whole(&model, feature);
    }
    assert_no_entry_half_measured(&dir.0);
}

#[test]
fn a_summary_refused_for_its_values_is_never_followed_by_a_describe() {
    // The summary is the measure's first request: refused as invalid input, the solid is
    // unmeasured and nothing describes it.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    let failure = worker_failure(ErrorCode::InvalidInput).unwrap();
    engine
        .kernel()
        .refuse_as_worker(move |op| matches!(op, Op::Summarize { .. }).then(|| failure.clone()));
    let model = published(&engine, checked(EXAMPLE));
    assert_eq!(model.drift().len(), 3);
    assert_eq!(asked(engine.kernel(), "Describe"), 0);
    for feature in FEATURES {
        assert!(model.description(&id(feature)).is_none(), "{feature}");
    }
}

// ---------------------------------------------------------------------------------------
// Forged cache entries, sealed as their writer would seal them.

/// Where the fake kernel's entry for step `key` lives in the cache in `dir` (as
/// tests/adversarial_cache.rs derives it, from the cache module's documented format).
fn entry_path(dir: &Path, step: &str) -> PathBuf {
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
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

/// An entry's bytes, sealed with the secret of the cache in `dir`.
fn encode(dir: &Path, entry: &Entry) -> Vec<u8> {
    let payload = postcard::to_allocvec(entry).unwrap();
    let secret = std::fs::read(secret_of(dir)).unwrap();
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&secret).unwrap();
    mac.update(&payload);
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&mac.finalize().into_bytes());
    bytes.extend_from_slice(&payload);
    bytes
}

/// Seeds a cache with the example, rewrites `feature`'s entry with its own summary and
/// the description `forge` makes of its own and the model's, and opens a session over it:
/// the model it publishes, and the session.
fn forged(
    name: &str,
    feature: &str,
    forge: impl FnOnce(&mut Description, &Model),
) -> (Arc<Model>, Arc<Model>, Recompute<FakeKernel>, Scratch) {
    let dir = Scratch::new(name);
    let model = published(&session(&dir.0), checked(EXAMPLE));
    let (key, built) = built(&model, feature);
    let mut description = built.description().unwrap().clone();
    forge(&mut description, &model);
    let path = entry_path(&dir.0, &key);
    let mut entry = decode(&std::fs::read(&path).unwrap());
    entry.measure = Measure::Summary {
        summary: built.summary().unwrap().clone(),
        description,
    };
    std::fs::write(&path, encode(&dir.0, &entry)).unwrap();
    let next = session(&dir.0);
    let again = published(&next, checked(EXAMPLE));
    (model, again, next, dir)
}

/// Asserts a forged entry for `feature` was taken for its shape but measured again: no
/// shape made, one summary asked, and the published description the kernel's, whole.
#[track_caller]
fn assert_measured_again(
    model: &Model,
    again: &Model,
    next: &Recompute<FakeKernel>,
    feature: &str,
) {
    assert!(calls(next).is_empty(), "made again: {:?}", calls(next));
    assert_eq!(
        asked(next.kernel(), "Summarize"),
        1,
        "{feature}: the forged description was taken back"
    );
    assert_eq!(
        again.description(&id(feature)),
        model.description(&id(feature))
    );
    assert_described_whole(again, feature);
}

#[test]
fn a_kept_description_an_element_short_is_measured_again() {
    // BREAKS: `cache::built_of` checks a kept description's values (`validate`) but not
    // its length: the block's entry with its last face dropped is taken back, so the
    // published block has 5 faces described where its summary counts 6 and its element map
    // names 6, and `elements` refuses the model's own description as a fault in Silico.
    let (model, again, next, _dir) = forged("description-short", "k7f2qx3m", |d, _| {
        d.faces.pop();
    });
    assert_measured_again(&model, &again, &next, "k7f2qx3m");
}

#[test]
fn a_kept_description_an_element_long_is_measured_again() {
    // BREAKS: as an element short, with a ninth vertex, sound in itself.
    let (model, again, next, _dir) = forged("description-long", "k7f2qx3m", |d, _| {
        d.vertices.push([9.0, 0.0, 0.0]);
    });
    assert_measured_again(&model, &again, &next, "k7f2qx3m");
}

#[test]
fn a_kept_description_of_another_solid_is_measured_again() {
    // BREAKS: the pin's description (3 faces, 3 edges, 2 vertices: a cylinder's, by hand)
    // in the block's entry is taken back as the block's.
    let (model, again, next, _dir) = forged("description-other", "k7f2qx3m", |d, model| {
        *d = model.description(&id("a91cw0de")).unwrap().clone();
        assert_eq!(lengths(d), [3, 3, 2]);
    });
    assert_measured_again(&model, &again, &next, "k7f2qx3m");
}

#[test]
fn a_kept_description_of_a_hundred_thousand_vertices_is_measured_again() {
    // BREAKS: a box's entry claiming 100,000 vertices, each sound, is taken back whole.
    let (model, again, next, _dir) = forged("description-huge", "k7f2qx3m", |d, _| {
        d.vertices = (1..=100_000).map(|n| [f64::from(n), 0.0, 0.0]).collect();
    });
    assert_measured_again(&model, &again, &next, "k7f2qx3m");
}

#[test]
fn a_kept_description_with_nothing_in_it_is_measured_again() {
    // BREAKS: the hole's description with no elements at all, for a solid that has 34
    // (the fake's cut keeps the block's 26 and the pin's 8).
    let (model, again, next, _dir) = forged("description-empty", "m3tq8v0z", |d, _| {
        d.faces.clear();
        d.edges.clear();
        d.vertices.clear();
    });
    assert_measured_again(&model, &again, &next, "m3tq8v0z");
}

#[test]
fn kept_descriptions_holding_values_no_worker_could_send_are_measured_again() {
    // Each a value `Description::validate` refuses from a worker: not finite, a normal not
    // a unit vector, a cone's half-angle a right angle, a degenerated edge whose middle
    // isn't its start, an edge 0 long that isn't degenerated.
    type Forge = fn(&mut Description);
    let forges: [(&str, Forge); 7] = [
        ("nan-centroid", |d| d.faces[0].centroid[1] = f64::NAN),
        ("infinite-vertex", |d| d.vertices[7][2] = f64::INFINITY),
        ("negative-infinite-length", |d| {
            d.edges[3].length = f64::NEG_INFINITY;
        }),
        ("normal-not-unit", |d| {
            d.faces[1].surface = Surface::Plane {
                normal: [0.0, 0.0, 1.5],
            };
        }),
        ("cone-right-angle", |d| {
            d.faces[2].surface = Surface::Cone {
                axis: silico_core::kernel::Axis {
                    origin: [0.0; 3],
                    direction: [0.0, 0.0, 1.0],
                },
                radius: 1.0,
                half_angle: std::f64::consts::FRAC_PI_2,
            };
        }),
        ("degenerated-apart", |d| {
            d.edges[0].curve = EdgeCurve::Degenerated;
            d.edges[0].length = 0.0;
            d.edges[0].end = d.edges[0].start;
        }),
        ("zero-length-line", |d| {
            d.edges[5].curve = EdgeCurve::Line;
            d.edges[5].length = 0.0;
        }),
    ];
    for (name, forge) in forges {
        let (model, again, next, _dir) =
            forged(&format!("description-{name}"), "k7f2qx3m", |d, _| forge(d));
        assert_measured_again(&model, &again, &next, "k7f2qx3m");
    }
}
