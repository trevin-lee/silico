//! break-it: M4.4's descriptions made in the rebuild (`Engine::measure`, the cache's
//! `Measure::Measured`, `Model::description`). A later batch of a solid's edges refused or
//! failing after the earlier ones and the summary came back; batches a worker at fault
//! answers (a total that isn't the summary's, more elements than asked for); how many
//! batches are in flight at once; recomputes superseded and cancelled while a solid is half
//! described; and forged cache entries, sealed as their writer would seal them, holding
//! descriptions no worker could send (values out of range) or that aren't the solid's (an
//! element short, one too many, another solid's, a hundred thousand vertices). A
//! description refused or failed leaves the summary standing (docs/design/m4.4-describe.md,
//! decision 4). Expected values are counts by hand (a box has 6 faces, 12 edges, and 8
//! vertices; the fake keeps every box's in a fusion of separate boxes) and the fake's
//! description of element n, which stands for n alone (`fake::described`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::cache;
use super::cache_format;
use super::describing;
use super::example;
use super::model;
use super::refusals;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use cache::{Scratch, calls, session, subdirectory_files};
use cache_format::{KeptDescription, MAGIC, Measure, decode, encode, entry_path};
use describing::{asked, asked_of, fused_boxes, fused_tree, settled};
use example::EXAMPLE;
use model::{WAIT, checked, id, published};
use refusals::worker_failure;
use silico_core::ErrorCode;
use silico_core::elements::elements;
use silico_core::fake::FakeKernel;
use silico_core::kernel::{ElementKind, Op};
use silico_core::recompute::{Built, FeatureOutcome, Model, Recompute, RunOutcome};
use silico_protocol::{Description, EdgeCurve, EdgeDescription, Reply, Surface};

/// The example's three features.
const FEATURES: [&str; 3] = ["k7f2qx3m", "a91cw0de", "m3tq8v0z"];

fn built(model: &Model, feature: &str) -> (String, Arc<Built>) {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { key, built }) => (key.to_string(), Arc::clone(built)),
        other => panic!("{feature}: {other:?}"),
    }
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

/// Waits until the kernel has started a request `picks`, failing after [`WAIT`]: a request
/// held mid-run starts when the run reaches it, which a loaded machine delays.
fn wait_for_call(kernel: &FakeKernel, picks: impl Fn(&Op) -> bool) {
    let deadline = Instant::now() + WAIT;
    while !kernel.calls().iter().any(|call| picks(&call.op)) {
        assert!(Instant::now() < deadline, "the request never started");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Whether `op` describes a solid of more than a batch of edges (in these tests, a fusion of
/// 84 boxes or more: 1,008 edges and up).
fn of_many_edges(op: &Op) -> bool {
    matches!(op, Op::Describe { shape, .. } if FakeKernel::counts(shape).is_some_and(|c| c.edges > 1000))
}

/// Whether `shape` is the example's block, as the fake made it.
fn is_block(shape: &silico_core::kernel::ShapeData) -> bool {
    FakeKernel::describe(shape).as_deref() == Some("box(40, 20, 10)")
}

/// The fake's description of every element of the block, changed by `change`, as a
/// `DescribeAll` reply: a worker at fault.
fn block_all(shape: &silico_core::kernel::ShapeData, change: impl Fn(&mut Description)) -> Reply {
    let mut description = FakeKernel::described(shape).unwrap();
    change(&mut description);
    Reply::DescribedAll(description)
}

/// Whether `op` describes the example's block: a batch, or all of it at once.
fn describes_block(op: &Op) -> bool {
    matches!(op, Op::Describe { shape, .. } | Op::DescribeAll { shape } if is_block(shape))
}

/// The fake's edges numbered `numbers`, as `fake::described` describes them.
fn edges(numbers: std::ops::RangeInclusive<u32>) -> Description {
    Description {
        faces: Vec::new(),
        edges: numbers
            .map(|n| EdgeDescription {
                curve: EdgeCurve::Other,
                length: 1.0,
                start: [f64::from(n), 0.0, 0.0],
                end: [f64::from(n), 1.0, 0.0],
                middle: [f64::from(n), 0.5, 0.0],
            })
            .collect(),
        vertices: Vec::new(),
    }
}

/// Asserts `feature` built, its summary kept and its fingerprint with it, and its
/// description not had, for the reason `code`: the solid's own result, which the
/// description's refusal doesn't touch.
#[track_caller]
fn assert_refused_beside_its_summary(model: &Model, feature: &str, code: ErrorCode) -> Arc<Built> {
    let (_, built) = built(model, feature);
    assert!(
        built.summary().is_some(),
        "{feature}: its summary wasn't kept"
    );
    assert!(built.measure_refused().is_none(), "{feature}");
    assert!(model.fingerprint(&id(feature)).is_some(), "{feature}");
    assert!(model.description(&id(feature)).is_none(), "{feature}");
    assert!(built.description().is_none(), "{feature}");
    assert_eq!(
        built.why_undescribed().map(silico_core::Error::code),
        Some(code),
        "{feature}"
    );
    built
}

// ---------------------------------------------------------------------------------------
// A later batch refused or failing, after the summary and the batches before it.

#[test]
fn a_later_edge_batch_refused_for_its_values_keeps_the_summary_and_the_refusal_on_disk_too() {
    // Fusion i holds 12(i + 1) edges: from f0000083 (1,008) on, its edges take two batches.
    // The second refused as invalid input, those fusions keep their summaries (12(i + 1)
    // edges, by hand) and the refusal beside them, and drift is empty; every one before is
    // described whole. Not asked again in the session, and a second session takes it all
    // back from the cache, the refusals too, asking nothing.
    let dir = Scratch::new("later-batch-refused");
    let first = session(&dir.0);
    let failure = worker_failure(ErrorCode::InvalidInput).unwrap();
    let given = failure.clone();
    first
        .kernel()
        .refuse_as_worker(move |op| later_edge_batch(op).then(|| given.clone()));
    let document = fused_boxes(86);
    let model = published(&first, checked(&document));
    let refused: Vec<String> = (83..86).map(|i| format!("f{i:07}")).collect();
    for i in 1..86u32 {
        let fusion = format!("f{i:07}");
        if refused.contains(&fusion) {
            let built = assert_refused_beside_its_summary(&model, &fusion, ErrorCode::InvalidInput);
            assert_eq!(built.summary().unwrap().edges, 12 * (i + 1), "{fusion}");
        } else {
            assert_described_whole(&model, &fusion);
        }
    }
    assert!(model.drift().is_empty(), "{:?}", model.drift());
    // Each of the three fusions' four batches was sent at once, its vertices' too; once its
    // edges were refused, one a worker hadn't taken was cancelled, so as many as all twelve
    // were asked, and some of them run.
    let asked_of_many = first
        .kernel()
        .asked()
        .iter()
        .filter(|call| of_many_edges(&call.op))
        .count();
    assert_eq!(asked_of_many, 12);
    let asked_before = first.kernel().calls().len();
    let kept = published(&first, checked(&document));
    assert_eq!(first.kernel().calls().len(), asked_before, "asked again");
    for fusion in &refused {
        assert_refused_beside_its_summary(&kept, fusion, ErrorCode::InvalidInput);
    }
    // Kept on disk as the worker said it, beside the summary.
    let (key, _) = built(&model, "f0000085");
    match decode(&std::fs::read(entry_path(&dir.0, &key)).unwrap()).measure {
        Measure::Measured {
            description: KeptDescription::Refused { op, failure: kept },
            ..
        } => {
            assert_eq!(op, "Describe");
            assert_eq!(kept, failure);
        }
        _ => panic!("f0000085's refusal isn't kept beside its summary"),
    }
    let second = session(&dir.0);
    let again = published(&second, checked(&document));
    assert!(second.kernel().calls().is_empty(), "{:?}", calls(&second));
    assert!(again.drift().is_empty(), "{:?}", again.drift());
    for fusion in &refused {
        assert_refused_beside_its_summary(&again, fusion, ErrorCode::InvalidInput);
    }
    assert_described_whole(&again, "f0000082");
}

#[test]
fn a_later_edge_batch_a_worker_crashed_on_keeps_the_summary_and_is_described_again_alone() {
    // The first time each solid's second batch of edges is asked, the worker crashes: the
    // solid keeps its summary, and the crash beside it, and drift is empty. Each solid's
    // batches are its faces, its first and second thousand edges, and its vertices, sent
    // together and awaited in order: the crash is the third's, so the first three of each
    // start, and its vertex batch, cancelled unrun if it's still waiting (#220), may or may
    // not: between 6 and 8 for the two. The next recompute summarizes nothing again
    // and describes those two again, every batch from the first (four each), joined in
    // order: 1,020 edges for 85 boxes, the 1,001st edge the fake's 1,001st.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    let crashes = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&crashes);
    engine.kernel().refuse(move |op| {
        (later_edge_batch(op) && counted.fetch_add(1, Ordering::SeqCst) < 2)
            .then(|| silico_core::Error::new(ErrorCode::WorkerCrashed, "the kernel worker crashed"))
    });
    let document = fused_boxes(85);
    let failed = published(&engine, checked(&document));
    assert!(failed.drift().is_empty(), "{:?}", failed.drift());
    for fusion in ["f0000083", "f0000084"] {
        assert_refused_beside_its_summary(&failed, fusion, ErrorCode::WorkerCrashed);
    }
    assert_described_whole(&failed, "f0000082");
    // Every batch taken has run out; one cancelled while it waited never will.
    assert!(engine.kernel().wait_idle(WAIT));
    let first = asked_of(engine.kernel(), of_many_edges);
    assert!(
        (6..=8).contains(&first),
        "{first} of the two fusions' batches started"
    );
    let summaries = asked(engine.kernel(), "Summarize");
    let describes = asked(engine.kernel(), "Describe");
    let again = published(&engine, checked(&document));
    assert!(again.drift().is_empty(), "{:?}", again.drift());
    assert_eq!(
        asked(engine.kernel(), "Summarize"),
        summaries,
        "summarized again"
    );
    // The two fusions' eight batches, and no other solid's: none fails, so every one is
    // awaited before the model is published.
    assert_eq!(asked_of(engine.kernel(), of_many_edges), first + 8);
    assert_eq!(asked(engine.kernel(), "Describe"), describes + 8);
    assert_described_whole(&again, "f0000083");
    assert_described_whole(&again, "f0000084");
    assert_eq!(
        lengths(again.description(&id("f0000084")).unwrap()),
        [510, 1020, 680]
    );
}

// ---------------------------------------------------------------------------------------
// Batches a worker at fault answers (`FakeKernel::answer_instead`).

#[test]
fn a_description_short_of_the_summarys_faces_is_a_workers_fault_kept_beside_the_summary() {
    // The block's faces answered as a worker at fault might: 5 faces, a description that
    // holds together, where the summary counts 6. A protocol error, naming both counts,
    // beside the block's summary, which stands; drift is empty; the pin and the hole
    // are described whole. A worker's fault, so the next recompute asks the block's
    // description again (and only that), the cache keeps nothing of it, and a fresh session
    // over the cache describes the block again, whole, making and summarizing nothing.
    let dir = Scratch::new("face-total-short");
    let first = session(&dir.0);
    // The block, 26 elements, is described in one request (`DescribeAll`), so its faces
    // come in that reply: 5 of them.
    first.kernel().answer_instead(|op| match op {
        Op::DescribeAll { shape } if is_block(shape) => Some(block_all(shape, |d| {
            d.faces.pop();
        })),
        _ => None,
    });
    let model = published(&first, checked(EXAMPLE));
    assert!(model.drift().is_empty(), "{:?}", model.drift());
    let block = assert_refused_beside_its_summary(&model, "k7f2qx3m", ErrorCode::ProtocolError);
    assert_eq!(block.summary().unwrap().faces, 6);
    assert_eq!(
        block.why_undescribed().unwrap().cause(),
        Some("it describes 5 faces, and its summary counts 6")
    );
    assert_described_whole(&model, "a91cw0de");
    assert_described_whole(&model, "m3tq8v0z");
    // The block's one request, and nothing more asked of it.
    assert_eq!(settled(first.kernel(), describes_block, 1), 1);
    let again = published(&first, checked(EXAMPLE));
    assert_eq!(settled(first.kernel(), describes_block, 2), 2);
    assert_eq!(asked(first.kernel(), "Summarize"), 3, "summarized again");
    // The pin's one and the hole's one, and the block's twice.
    assert_eq!(asked(first.kernel(), "Describe"), 4);
    assert_refused_beside_its_summary(&again, "k7f2qx3m", ErrorCode::ProtocolError);
    let (key, _) = built(&model, "k7f2qx3m");
    assert!(matches!(
        decode(&std::fs::read(entry_path(&dir.0, &key)).unwrap()).measure,
        Measure::Measured {
            description: KeptDescription::Unknown,
            ..
        }
    ));
    let next = session(&dir.0);
    let fresh = published(&next, checked(EXAMPLE));
    assert!(calls(&next).is_empty(), "{:?}", calls(&next));
    assert_eq!(asked(next.kernel(), "Summarize"), 0);
    // The block alone, in one request.
    assert_eq!(asked(next.kernel(), "Describe"), 1);
    assert_described_whole(&fresh, "k7f2qx3m");
}

#[test]
fn a_second_edge_batch_whose_total_isnt_the_summarys_is_a_workers_fault_and_the_run_publishes() {
    // The last fusion's second batch of edges (from 1,001, 20 asked) answered with its 20
    // edges and a total of 1,021, where the summary counts 1,020: the batch holds together
    // alone, and is refused against the summary. The run is published, not superseded; the
    // fusion keeps its summary, the protocol error beside it naming both totals.
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().answer_instead(|op| match op {
        Op::Describe {
            shape,
            kind: ElementKind::Edge,
            first: 1001,
            count: 20,
        } if FakeKernel::counts(shape).is_some_and(|c| c.edges == 1020) => Some(Reply::Described {
            total: 1021,
            description: edges(1001..=1020),
        }),
        _ => None,
    });
    let model = match engine.start(checked(&fused_boxes(85))).wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish in {WAIT:?}"),
    };
    assert!(model.drift().is_empty(), "{:?}", model.drift());
    let last = assert_refused_beside_its_summary(&model, "f0000084", ErrorCode::ProtocolError);
    assert_eq!(
        last.why_undescribed().unwrap().cause(),
        Some("it says the shape has 1021 edges, and its summary counts 1020")
    );
    assert_described_whole(&model, "f0000083");
}

#[test]
fn a_description_of_more_vertices_or_fewer_edges_than_the_summary_counts_is_a_workers_fault() {
    // The block, described in one request (`DescribeAll`), answered with a ninth vertex,
    // sound in itself, where the summary counts 8; or with an edge short of its 12. Each a
    // protocol error saying what's wrong, beside the block's summary; and the next
    // recompute asks the block's description again.
    type Change = fn(&mut Description);
    let answers: [(Change, &str); 2] = [
        (
            |d| d.vertices.push([9.0, 0.0, 0.0]),
            "it describes 9 vertices, and its summary counts 8",
        ),
        (
            |d| {
                d.edges.pop();
            },
            "it describes 11 edges, and its summary counts 12",
        ),
    ];
    for (change, cause) in answers {
        let engine = Recompute::new(FakeKernel::new(2).unwrap());
        engine.kernel().answer_instead(move |op| match op {
            Op::DescribeAll { shape } if is_block(shape) => Some(block_all(shape, change)),
            _ => None,
        });
        let model = published(&engine, checked(EXAMPLE));
        assert!(model.drift().is_empty(), "{cause}: {:?}", model.drift());
        let block = assert_refused_beside_its_summary(&model, "k7f2qx3m", ErrorCode::ProtocolError);
        assert_eq!(block.why_undescribed().unwrap().cause(), Some(cause));
        assert_described_whole(&model, "m3tq8v0z");
        // One request of the block, and one again in the next recompute.
        assert_eq!(settled(engine.kernel(), describes_block, 1), 1, "{cause}");
        published(&engine, checked(EXAMPLE));
        assert_eq!(settled(engine.kernel(), describes_block, 2), 2, "{cause}");
        assert_eq!(asked(engine.kernel(), "Summarize"), 3, "{cause}");
    }
}

// ---------------------------------------------------------------------------------------
// How many batches of a solid are in flight at once.

/// Whether `op` describes the last fusion of `fused_tree(8)`: 256 boxes, 3,072 edges.
fn describes_the_tree(op: &Op) -> bool {
    matches!(op, Op::Describe { shape, .. } if FakeKernel::counts(shape).is_some_and(|c| c.edges == 3072))
}

/// How many requests describing the tree `kernel` was asked, whether or not they started.
fn asked_of_tree(kernel: &FakeKernel) -> usize {
    kernel
        .asked()
        .iter()
        .filter(|call| describes_the_tree(&call.op))
        .count()
}

#[test]
fn a_small_solid_is_described_in_one_request() {
    // The block has 6 faces, 12 edges, and 8 vertices, 26 in all: one `DescribeAll`, where
    // a batch of each kind would take three requests, and never a batch.
    let engine = Recompute::new(FakeKernel::new(8).unwrap());
    let model = published(&engine, checked(EXAMPLE));
    assert_described_whole(&model, "k7f2qx3m");
    let block: Vec<&str> = engine
        .kernel()
        .calls()
        .iter()
        .filter(|call| describes_block(&call.op))
        .map(|call| match call.op {
            Op::DescribeAll { .. } => "all",
            _ => "batch",
        })
        .collect();
    assert_eq!(block, ["all"]);
}

/// `boxes` boxes and then `tori` tori, apart, fused one after another: the fake keeps each
/// one's elements, a box's 6 faces, 12 edges, and 8 vertices (26), a torus's 1, 2, and 1
/// (4), by hand.
fn boxes_and_tori(boxes: usize, tori: usize) -> String {
    let n = boxes + tori;
    let mut features = Vec::new();
    for i in 0..n {
        let x = i * 30;
        features.push(if i < boxes {
            format!(
                r#"    {{"id": "p{i:07}", "type": "box", "v": 1, "name": "Box", "position": [{x}, 0, 0], "length": 10, "width": 10, "height": 10}}"#
            )
        } else {
            format!(
                r#"    {{"id": "p{i:07}", "type": "torus", "v": 1, "name": "Ring", "position": [{x}, 0, 0], "radius1": 10, "radius2": 2}}"#
            )
        });
    }
    for i in 1..n {
        let base = if i == 1 {
            "p0000000".to_owned()
        } else {
            format!("f{:07}", i - 1)
        };
        features.push(format!(
            r#"    {{"id": "f{i:07}", "type": "fuse", "v": 1, "name": "Fuse", "base": "{base}", "tool": "p{i:07}"}}"#
        ));
    }
    format!(
        "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"naming\": 5,\n  \"features\": [\n{}\n  ]\n}}\n",
        features.join(",\n")
    )
}

#[test]
fn a_solid_of_a_batch_of_elements_in_all_is_one_request_and_one_more_is_batches() {
    // At the bound: 38 boxes and 3 tori, 38 × 26 + 3 × 4 = 1,000 elements, by hand, is one
    // request; 38 boxes and 4 tori, 1,004, a batch of each kind (each under 1,000), three.
    for (tori, all, batches) in [(3, 1, 0), (4, 0, 3)] {
        let engine = Recompute::new(FakeKernel::new(2).unwrap());
        let document = boxes_and_tori(38, tori);
        let last = format!("f{:07}", 38 + tori - 1);
        let model = published(&engine, checked(&document));
        let description = model.description(&id(&last)).unwrap();
        assert_eq!(
            lengths(description).iter().sum::<usize>(),
            38 * 26 + tori * 4,
            "{last}"
        );
        let of_last = |op: &Op| match op {
            Op::Describe { shape, .. } | Op::DescribeAll { shape } => {
                FakeKernel::counts(shape).is_some_and(|c| c.faces == 38 * 6 + tori as u32)
            }
            _ => false,
        };
        let calls = engine.kernel().calls();
        let alls = calls
            .iter()
            .filter(|c| of_last(&c.op) && matches!(c.op, Op::DescribeAll { .. }))
            .count();
        let parts = calls
            .iter()
            .filter(|c| of_last(&c.op) && matches!(c.op, Op::Describe { .. }))
            .count();
        assert_eq!((alls, parts), (all, batches), "{tori} tori");
    }
}

/// 80 boxes and 20 tori, apart, fused one after another: by hand, 80 × 12 + 20 × 2 = 1,000
/// edges, exactly a batch, beside 500 faces and 660 vertices.
fn a_batch_of_edges_exactly() -> String {
    let mut features = Vec::new();
    for i in 0..100 {
        let x = i * 30;
        features.push(if i < 80 {
            format!(
                r#"    {{"id": "p{i:07}", "type": "box", "v": 1, "name": "Box", "position": [{x}, 0, 0], "length": 10, "width": 10, "height": 10}}"#
            )
        } else {
            format!(
                r#"    {{"id": "p{i:07}", "type": "torus", "v": 1, "name": "Ring", "position": [{x}, 0, 0], "radius1": 10, "radius2": 2}}"#
            )
        });
    }
    for i in 1..100 {
        let base = if i == 1 {
            "p0000000".to_owned()
        } else {
            format!("f{:07}", i - 1)
        };
        features.push(format!(
            r#"    {{"id": "f{i:07}", "type": "fuse", "v": 1, "name": "Fuse", "base": "{base}", "tool": "p{i:07}"}}"#
        ));
    }
    format!(
        "{{\n  \"silico\": 1,\n  \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}},\n  \"naming\": 5,\n  \"features\": [\n{}\n  ]\n}}\n",
        features.join(",\n")
    )
}

#[test]
fn a_solid_of_exactly_a_batch_of_a_kind_is_sent_one_batch_at_a_time() {
    // At the bound: 1,000 edges is one batch, so the last fusion's three batches go one
    // after another, as a small solid's do; a thousand and one would be two, and four at
    // once (the test below).
    let engine = Recompute::new(FakeKernel::new(8).unwrap());
    let last = |op: &Op| {
        matches!(op, Op::Describe { shape, .. }
            if FakeKernel::counts(shape).is_some_and(|c| c.edges == 1000 && c.faces == 500))
    };
    let hold = engine.kernel().hold(last);
    let run = engine.start(checked(&a_batch_of_edges_exactly()));
    assert_eq!(settled(engine.kernel(), last, 1), 1);
    hold.release();
    match run.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => {
            assert_eq!(
                lengths(model.description(&id("f0000099")).unwrap()),
                [500, 1000, 660]
            );
        }
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish in {WAIT:?}"),
    }
}

#[test]
fn a_solids_batches_are_sent_four_at_a_time_and_joined_in_order() {
    // 256 boxes fused: 1,536 faces, 3,072 edges, and 2,048 vertices, by hand, so nine
    // batches. With every one of them held, on a kernel of eight threads with room for more,
    // four are sent and no fifth until one comes back; released, all nine are asked, each
    // as planned from the summary, and joined in the kernel's numbering.
    let engine = Recompute::new(FakeKernel::new(8).unwrap());
    let hold = engine.kernel().hold(describes_the_tree);
    let run = engine.start(checked(&fused_tree(8)));
    assert_eq!(settled(engine.kernel(), describes_the_tree, 4), 4);
    // Asked, not only started: a fifth no thread had taken yet is still one too many.
    assert_eq!(asked_of_tree(engine.kernel()), 4);
    hold.release();
    let model = match run.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish in {WAIT:?}"),
    };
    let batches: Vec<(ElementKind, u32, u32)> = engine
        .kernel()
        .calls()
        .iter()
        .filter_map(|call| match &call.op {
            Op::Describe {
                kind, first, count, ..
            } if describes_the_tree(&call.op) => Some((*kind, *first, *count)),
            _ => None,
        })
        .collect();
    assert_eq!(
        batches,
        [
            (ElementKind::Face, 1, 1000),
            (ElementKind::Face, 1001, 536),
            (ElementKind::Edge, 1, 1000),
            (ElementKind::Edge, 1001, 1000),
            (ElementKind::Edge, 2001, 1000),
            (ElementKind::Edge, 3001, 72),
            (ElementKind::Vertex, 1, 1000),
            (ElementKind::Vertex, 1001, 1000),
            (ElementKind::Vertex, 2001, 48),
        ]
    );
    assert!(model.drift().is_empty(), "{:?}", model.drift());
    assert_described_whole(&model, "t8000000");
    assert_eq!(
        lengths(model.description(&id("t8000000")).unwrap()),
        [1536, 3072, 2048]
    );
}

#[test]
fn a_solids_batches_share_its_bytes_with_each_other_and_the_model() {
    // From M4's review: each batch's request held a copy of the solid's bytes, so four in
    // flight for each of 64 recompute jobs held 256 copies. A request now shares them:
    // the nine batches of 256 fused boxes name one allocation, the model's own.
    let engine = Recompute::new(FakeKernel::new(8).unwrap());
    let model = match engine.start(checked(&fused_tree(8))).wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish in {WAIT:?}"),
    };
    let (_, tree) = built(&model, "t8000000");
    let held: std::collections::BTreeSet<usize> = engine
        .kernel()
        .asked()
        .iter()
        .filter(|call| describes_the_tree(&call.op))
        .map(|call| match &call.op {
            Op::Describe { shape, .. } => shape.brep().as_bytes().as_ptr() as usize,
            other => panic!("{other:?}"),
        })
        .collect();
    let batches = engine
        .kernel()
        .asked()
        .iter()
        .filter(|call| describes_the_tree(&call.op))
        .count();
    assert_eq!(batches, 9);
    assert_eq!(
        held,
        [tree.shape.brep().as_bytes().as_ptr() as usize].into(),
        "{batches} batches held {} allocations",
        held.len()
    );
}

// ---------------------------------------------------------------------------------------
// Superseded and cancelled while a solid is half described.

/// Every entry in the cache in `dir`, each measured one whole: as many of each kind
/// described as its summary counts.
#[track_caller]
fn assert_no_entry_half_measured(dir: &Path) {
    for file in subdirectory_files(dir) {
        let bytes = std::fs::read(&file).unwrap();
        if !bytes.starts_with(MAGIC) {
            continue;
        }
        if let Measure::Measured {
            summary,
            description: KeptDescription::Described(description),
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

/// Asserts every feature of `fused_boxes(n)` in `model` described whole: its boxes and its
/// fusions, the solids held mid-describe among them.
#[track_caller]
fn assert_every_box_and_fusion_described_whole(model: &Model, n: usize) {
    for i in 0..n {
        assert_described_whole(model, &format!("b{i:07}"));
    }
    for i in 1..n {
        assert_described_whole(model, &format!("f{i:07}"));
    }
}

#[test]
fn a_recompute_superseded_mid_describe_leaves_nothing_half_measured_in_memory_or_on_disk() {
    // 90 boxes fused (540 faces, 1,080 edges, 720 vertices, so described in batches): every
    // solid's second batch of edges is held once its faces and first edges are back; a
    // recompute of 91 boxes then supersedes the first. Nothing of the first is published;
    // the second's model is described whole; no entry on disk holds a part of a
    // description; and a session over the cache rebuilds the first document described
    // whole, asking nothing to make it. (A solid of a batch of elements or fewer is one
    // request, never half described.)
    let dir = Scratch::new("superseded-mid-describe");
    let first = session(&dir.0);
    let hold = first.kernel().hold(later_edge_batch);
    let run = first.start(checked(&fused_boxes(90)));
    wait_for_call(first.kernel(), later_edge_batch);
    let newer = first.start(checked(&fused_boxes(91)));
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
    assert_every_box_and_fusion_described_whole(&model, 91);
    assert_no_entry_half_measured(&dir.0);
    let next = session(&dir.0);
    let again = published(&next, checked(&fused_boxes(90)));
    assert!(calls(&next).is_empty(), "{:?}", calls(&next));
    assert!(again.drift().is_empty(), "{:?}", again.drift());
    assert_every_box_and_fusion_described_whole(&again, 90);
    assert_no_entry_half_measured(&dir.0);
}

#[test]
fn a_recompute_cancelled_mid_describe_publishes_nothing_and_the_next_describes_whole() {
    // As superseded, by cancel, on 90 boxes fused: the held batches end superseded, the run
    // publishes nothing, and the next recompute of the same document measures each solid
    // again, whole, keeping no half of the cancelled one's.
    let dir = Scratch::new("cancelled-mid-describe");
    let engine = session(&dir.0);
    let document = fused_boxes(90);
    let hold = engine.kernel().hold(later_edge_batch);
    let run = engine.start(checked(&document));
    wait_for_call(engine.kernel(), later_edge_batch);
    engine.cancel();
    assert!(matches!(
        run.wait_timeout(WAIT),
        Ok(Ok(RunOutcome::Superseded))
    ));
    assert!(engine.published().is_none());
    hold.release();
    assert_no_entry_half_measured(&dir.0);
    let model = published(&engine, checked(&document));
    assert!(model.drift().is_empty(), "{:?}", model.drift());
    assert_every_box_and_fusion_described_whole(&model, 90);
    assert_no_entry_half_measured(&dir.0);
}

#[test]
fn a_batch_that_fails_cancels_the_batches_still_waiting_and_none_after_is_asked() {
    // 256 boxes fused (1,536 faces, 3,072 edges, 2,048 vertices: nine batches, four in
    // flight at once: both of faces, then the first two of edges), on a kernel of one
    // thread, its first batch, of faces, refused, and every other batch of it held. Once the
    // refusal is back, the core drops the three still in flight, one by one, which cancels
    // them, and asks none of the five planned after. The thread may take one of the three
    // before the core has dropped it, and be held there: the scheduler's timing. Then, the
    // queue first in first out, the three are all dropped once those after it wait
    // cancelled, those before it passed over; and only then is it released, so it alone
    // runs. If the thread takes none, it passes over all three, and the run goes on. Either
    // way, the model is published, the tree's refusal beside its summary. A core that waited
    // for the batches in flight, or didn't cancel them, would leave the one taken held and
    // the run unfinished; one that asked on would ask more than four.
    let engine = Recompute::new(FakeKernel::new(1).unwrap());
    let first_faces = |op: &Op| {
        describes_the_tree(op)
            && matches!(
                op,
                Op::Describe {
                    kind: ElementKind::Face,
                    first: 1,
                    ..
                }
            )
    };
    engine.kernel().refuse(move |op| {
        first_faces(op).then(|| silico_core::Error::new(ErrorCode::KernelFailed, "no faces today"))
    });
    let hold = engine
        .kernel()
        .hold(move |op| describes_the_tree(op) && !first_faces(op));
    let of_tree = |calls: Vec<silico_core::fake::Call>| -> Vec<(ElementKind, u32)> {
        calls
            .into_iter()
            .filter_map(|call| match call.op {
                Op::Describe { kind, first, .. } if describes_the_tree(&call.op) => {
                    Some((kind, first))
                }
                _ => None,
            })
            .collect()
    };
    let in_flight = [
        (ElementKind::Face, 1001),
        (ElementKind::Edge, 1),
        (ElementKind::Edge, 1001),
    ];
    // The batch the thread took after the refused one, held, once the core has dropped all
    // three: those after it in the queue wait cancelled.
    let all_dropped = || {
        let ran = of_tree(engine.kernel().calls());
        let at = in_flight
            .iter()
            .position(|batch| Some(batch) == ran.get(1))?;
        (engine.kernel().cancelled_waiting() == in_flight.len() - 1 - at).then_some(())
    };
    let mut run = engine.start(checked(&fused_tree(8)));
    let deadline = Instant::now() + WAIT;
    let outcome = loop {
        if all_dropped().is_some() {
            hold.release();
            break run.wait_timeout(WAIT);
        }
        match run.wait_timeout(Duration::from_millis(5)) {
            Ok(outcome) => break Ok(outcome),
            Err(waiting) => run = waiting,
        }
        assert!(
            Instant::now() < deadline,
            "the batches in flight weren't cancelled: {:?} ran, {} waiting cancelled",
            of_tree(engine.kernel().calls()),
            engine.kernel().cancelled_waiting()
        );
    };
    hold.release();
    let model = match outcome {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish in {WAIT:?}"),
    };
    let tree = assert_refused_beside_its_summary(&model, "t8000000", ErrorCode::KernelFailed);
    assert_eq!(tree.why_undescribed().unwrap().message(), "no faces today");
    assert!(engine.kernel().wait_idle(WAIT));
    // The thread passes over a cancelled batch only when it next looks for work, which can
    // come after nothing is running (`wait_idle` returns then): each is dropped unrun in its
    // own time, which a loaded machine delays.
    let deadline = Instant::now() + WAIT;
    while engine.kernel().cancelled_waiting() > 0 {
        assert!(
            Instant::now() < deadline,
            "{} cancelled batches never passed over",
            engine.kernel().cancelled_waiting()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        of_tree(engine.kernel().asked()),
        [&[(ElementKind::Face, 1)][..], &in_flight].concat()
    );
    let ran = of_tree(engine.kernel().calls());
    assert!(
        ran.first() == Some(&(ElementKind::Face, 1))
            && ran.len() <= 2
            && ran.get(1).is_none_or(|taken| in_flight.contains(taken)),
        "a cancelled batch ran: {ran:?}"
    );
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
    entry.measure = Measure::Measured {
        summary: built.summary().unwrap().clone(),
        description: KeptDescription::Described(description),
    };
    std::fs::write(&path, encode(&dir.0, &entry)).unwrap();
    let next = session(&dir.0);
    let again = published(&next, checked(EXAMPLE));
    (model, again, next, dir)
}

/// Asserts a forged entry for `feature` was taken for its shape and its summary but
/// described again: no shape made, nothing summarized, the feature described in one request
/// (each of the example's solids has under a batch of elements in all), and the published
/// description the kernel's, whole.
#[track_caller]
fn assert_described_again(
    model: &Model,
    again: &Model,
    next: &Recompute<FakeKernel>,
    feature: &str,
) {
    assert!(calls(next).is_empty(), "made again: {:?}", calls(next));
    assert_eq!(
        asked(next.kernel(), "Summarize"),
        0,
        "{feature}: summarized again"
    );
    assert_eq!(
        asked(next.kernel(), "Describe"),
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
fn a_kept_description_an_element_short_is_described_again() {
    // BREAKS: `cache::built_of` checks a kept description's values (`validate`) but not
    // its length: the block's entry with its last face dropped is taken back, so the
    // published block has 5 faces described where its summary counts 6 and its element map
    // names 6, and `elements` refuses the model's own description as a fault in Silico.
    let (model, again, next, _dir) = forged("description-short", "k7f2qx3m", |d, _| {
        d.faces.pop();
    });
    assert_described_again(&model, &again, &next, "k7f2qx3m");
}

#[test]
fn a_kept_description_an_element_long_is_described_again() {
    // BREAKS: as an element short, with a ninth vertex, sound in itself.
    let (model, again, next, _dir) = forged("description-long", "k7f2qx3m", |d, _| {
        d.vertices.push([9.0, 0.0, 0.0]);
    });
    assert_described_again(&model, &again, &next, "k7f2qx3m");
}

#[test]
fn a_kept_description_of_another_solid_is_described_again() {
    // BREAKS: the pin's description (3 faces, 3 edges, 2 vertices: a cylinder's, by hand)
    // in the block's entry is taken back as the block's.
    let (model, again, next, _dir) = forged("description-other", "k7f2qx3m", |d, model| {
        *d = model.description(&id("a91cw0de")).unwrap().clone();
        assert_eq!(lengths(d), [3, 3, 2]);
    });
    assert_described_again(&model, &again, &next, "k7f2qx3m");
}

#[test]
fn a_kept_description_of_a_hundred_thousand_vertices_is_described_again() {
    // BREAKS: a box's entry claiming 100,000 vertices, each sound, is taken back whole.
    let (model, again, next, _dir) = forged("description-huge", "k7f2qx3m", |d, _| {
        d.vertices = (1..=100_000).map(|n| [f64::from(n), 0.0, 0.0]).collect();
    });
    assert_described_again(&model, &again, &next, "k7f2qx3m");
}

#[test]
fn a_kept_description_with_nothing_in_it_is_described_again() {
    // BREAKS: the hole's description with no elements at all, for a solid that has 34
    // (the fake's cut keeps the block's 26 and the pin's 8).
    let (model, again, next, _dir) = forged("description-empty", "m3tq8v0z", |d, _| {
        d.faces.clear();
        d.edges.clear();
        d.vertices.clear();
    });
    assert_described_again(&model, &again, &next, "m3tq8v0z");
}

#[test]
fn kept_descriptions_holding_values_no_worker_could_send_are_described_again() {
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
        assert_described_again(&model, &again, &next, "k7f2qx3m");
    }
}
