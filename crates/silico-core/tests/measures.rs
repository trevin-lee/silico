//! M4.5: `distance` and `angle` on a model and on a session (`silico_core::command::measure`):
//! how their two elements are found by name, in the live solids or one feature's, every reason
//! one isn't said, and each read's own kernel client. On the fake kernel, whose distance is how
//! far apart the elements' numbers are and whose faces aren't planes; the measures themselves
//! are tested on workers (silico-kernel's tests/measures.rs).

use super::example;
use super::listing::{answer, fixture, model_of};
use super::model::id;

use example::EXAMPLE;
use silico_core::ErrorCode;
use silico_core::command::measure::{DistanceAnswer, angle, distance};
use silico_core::command::{ModelChoice, ModelRead, Revision};
use silico_core::document::Length;
use silico_core::fake::FakeKernel;

fn pair(a: &str, b: &str) -> [String; 2] {
    [a.to_owned(), b.to_owned()]
}

fn mm(whole: i64) -> Length {
    Length::from_nm(whole * 1_000_000)
}

#[test]
fn two_faces_are_found_by_name_in_the_live_solids_and_measured_on_the_kernel() {
    // The example's cut keeps the block's faces under their own names: Face5 and Face6,
    // which the fake places at x = 5 and 6, so 1 apart.
    let model = model_of(EXAMPLE);
    let kernel = FakeKernel::new(1).unwrap();
    let d = distance(
        &model,
        &kernel,
        &pair("k7f2qx3m.box.Face5", "k7f2qx3m.box.Face6"),
        None,
    )
    .unwrap();
    assert_eq!(
        d,
        DistanceAnswer {
            distance: mm(1),
            from: [mm(5), mm(0), mm(0)],
            to: [mm(6), mm(0), mm(0)],
            center_distance: None,
            normal_distance: None,
        }
    );
    // The same name twice: 0 apart.
    let same = distance(
        &model,
        &kernel,
        &pair("k7f2qx3m.box.Face5", "k7f2qx3m.box.Face5"),
        None,
    )
    .unwrap();
    assert_eq!(same.distance, mm(0));
}

#[test]
fn feature_narrows_where_the_names_are_found_to_its_solid() {
    let model = model_of(EXAMPLE);
    let kernel = FakeKernel::new(1).unwrap();
    // The block, consumed by the cut: its own solid, found by its feature.
    let block = distance(
        &model,
        &kernel,
        &pair("k7f2qx3m.box.Face1", "k7f2qx3m.box.Face2"),
        Some(&id("k7f2qx3m")),
    )
    .unwrap();
    assert_eq!(block.distance, mm(1));
    // The pin has no block's face: broken, saying where it was looked for.
    let refused = distance(
        &model,
        &kernel,
        &pair("k7f2qx3m.box.Face1", "a91cw0de.cylinder.Face1"),
        Some(&id("a91cw0de")),
    )
    .unwrap_err();
    assert_eq!(refused.total(), 1);
    let error = &refused.errors()[0];
    assert_eq!(error.code(), ErrorCode::ReferenceBroken);
    assert_eq!(error.parameter(), Some("elements.0"));
    assert_eq!(
        error.message(),
        r#"elements.0, "k7f2qx3m.box.Face1", names no element of a91cw0de's solid"#
    );
    // A feature the document hasn't: refused naming feature, its names not looked for.
    let missing = distance(
        &model,
        &kernel,
        &pair("k7f2qx3m.box.Face1", "k7f2qx3m.box.Face2"),
        Some(&id("zzzzzzzz")),
    )
    .unwrap_err();
    assert_eq!(missing.total(), 1);
    assert_eq!(missing.errors()[0].parameter(), Some("feature"));
}

#[test]
fn every_name_that_finds_nothing_is_said_in_order_and_one_not_a_name_says_why() {
    let model = model_of(EXAMPLE);
    let kernel = FakeKernel::new(1).unwrap();
    let refused = distance(
        &model,
        &kernel,
        &pair("k7f2qx3m.box", "zzzzzzzz.box.Face1"),
        None,
    )
    .unwrap_err();
    let said: Vec<(Option<&str>, ErrorCode)> = refused
        .errors()
        .iter()
        .map(|e| (e.parameter(), e.code()))
        .collect();
    assert_eq!(
        said,
        [
            (Some("elements.0"), ErrorCode::ReferenceBroken),
            (Some("elements.1"), ErrorCode::ReferenceBroken)
        ]
    );
    assert!(
        refused.errors()[0]
            .message()
            .starts_with(r#"elements.0, "k7f2qx3m.box", isn't a name: "#),
        "{}",
        refused.errors()[0].message()
    );
    // Nothing reached the kernel.
    assert!(
        kernel.calls().iter().all(|c| c.op.name() != "Distance"),
        "{:?}",
        kernel.calls()
    );
}

#[test]
fn a_face_that_isnt_a_plane_has_no_angle() {
    // The fake describes every face as of no surface it names (other): no plane.
    let model = model_of(EXAMPLE);
    let refused = angle(
        &model,
        &pair("k7f2qx3m.box.Face5", "k7f2qx3m.box.Face6"),
        None,
    )
    .unwrap_err();
    let causes: Vec<Option<&str>> = refused.errors().iter().map(|e| e.cause()).collect();
    assert_eq!(
        causes,
        [
            Some("elements.0 is a face on a surface, not a plane"),
            Some("elements.1 is a face on a surface, not a plane")
        ]
    );
}

#[test]
fn on_a_session_each_read_measures_on_a_kernel_client_of_its_own() {
    let f = fixture();
    let s = f.open("part.silico", EXAMPLE);
    let current = answer(f.sessions.distance(
        &s,
        ModelChoice::Current,
        pair("k7f2qx3m.box.Face5", "k7f2qx3m.box.Face6"),
        None,
    ));
    assert_eq!(
        current.model,
        Some(ModelRead {
            revision: Revision::OPENED,
            of: ModelChoice::Current
        })
    );
    assert_eq!(current.result.unwrap().distance, mm(1));
    let published = answer(f.sessions.distance(
        &s,
        ModelChoice::Published,
        pair("k7f2qx3m.box.Face1", "k7f2qx3m.box.Face6"),
        None,
    ));
    assert_eq!(published.result.unwrap().distance, mm(5));
    // Each read's request came from a client other than the session's rebuilds'.
    let calls = f.kernel.calls();
    let rebuild = calls
        .iter()
        .find(|c| c.op.name() == "MakeBox")
        .map(|c| c.client)
        .unwrap();
    let reads: Vec<_> = calls
        .iter()
        .filter(|c| c.op.name() == "Distance")
        .map(|c| c.client)
        .collect();
    assert_eq!(reads.len(), 2);
    assert!(reads.iter().all(|c| *c != rebuild), "{reads:?} {rebuild:?}");
    assert_ne!(reads[0], reads[1]);
    // `angle` asks the kernel nothing.
    let angled = answer(f.sessions.angle(
        &s,
        ModelChoice::Current,
        pair("k7f2qx3m.box.Face5", "k7f2qx3m.box.Face6"),
        None,
    ));
    assert!(angled.result.is_err());
    assert_eq!(
        f.kernel
            .calls()
            .iter()
            .filter(|c| c.op.name() == "Distance")
            .count(),
        2
    );
}

#[test]
fn an_element_of_a_feature_that_failed_is_refused_with_its_failure_not_as_a_broken_name() {
    // The example's pin refused by the kernel: the cut is blocked, so the block and nothing
    // else is a body, and the cut's wall is no live solid's. Asked for, it's refused with what
    // became of the cut (blocked, since the pin failed), not as a name that finds nothing with
    // another body's elements as its candidates.
    use silico_core::recompute::Recompute;
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().refuse(|op| {
        (op.name() == "MakeCylinder")
            .then(|| silico_core::Error::new(ErrorCode::InvalidInput, "no pins today"))
    });
    let model = super::model::published(&engine, super::model::checked(EXAMPLE));
    let kernel = FakeKernel::new(1).unwrap();
    let refused = distance(
        &model,
        &kernel,
        &pair(
            "k7f2qx3m.box.Face5",
            "m3tq8v0z.cut.Face(I a91cw0de.cylinder.Face1)",
        ),
        None,
    )
    .unwrap_err();
    assert_eq!(refused.total(), 1, "{:?}", refused.errors());
    let error = &refused.errors()[0];
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert_eq!(
        error.message(),
        "m3tq8v0z didn't build, since it uses a91cw0de, which failed"
    );
    assert!(error.candidates().is_empty());
    // The pin's own face: its failure, as the kernel gave it.
    let pin = angle(
        &model,
        &pair("a91cw0de.cylinder.Face1", "k7f2qx3m.box.Face5"),
        None,
    )
    .unwrap_err();
    assert_eq!(pin.errors()[0].message(), "no pins today");
}

#[test]
fn a_model_with_no_live_solid_is_refused_with_why_not_as_a_bug() {
    // A box the kernel refuses: no body at all. Measuring its faces says why.
    use silico_core::recompute::Recompute;
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().refuse(|op| {
        (op.name() == "MakeBox")
            .then(|| silico_core::Error::new(ErrorCode::InvalidInput, "no boxes today"))
    });
    let model = super::model::published(&engine, super::model::checked(EXAMPLE));
    assert!(model.bodies().iter().all(|b| b.as_str() != "k7f2qx3m"));
    let kernel = FakeKernel::new(1).unwrap();
    let refused = distance(
        &model,
        &kernel,
        &pair("k7f2qx3m.box.Face5", "k7f2qx3m.box.Face6"),
        None,
    )
    .unwrap_err();
    // Said once, the box's own failure, never an internal error.
    assert_eq!(refused.total(), 1, "{:?}", refused.errors());
    assert_eq!(refused.errors()[0].message(), "no boxes today");
}

/// A triangle sketch padded twice, each pad its own body.
const TWO_PADS: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "s0000001", "type": "sketch", "v": 1, "name": "Triangle", "plane": "xy", "points": [{"id": "pa000000", "x": 0, "y": 0, "construction": false}, {"id": "pb000000", "x": 20, "y": 0, "construction": false}, {"id": "pc000000", "x": 0, "y": 10, "construction": false}], "lines": [{"id": "na000000", "from": "pa000000", "to": "pb000000", "construction": false}, {"id": "nb000000", "from": "pb000000", "to": "pc000000", "construction": false}, {"id": "nc000000", "from": "pc000000", "to": "pa000000", "construction": false}], "circles": [], "arcs": [], "constraints": []}, {"id": "p0000001", "type": "pad", "v": 1, "name": "First", "sketch": "s0000001", "extent": {"type": "one_side", "length": 5, "reversed": false}}, {"id": "p0000002", "type": "pad", "v": 1, "name": "Second", "sketch": "s0000001", "extent": {"type": "one_side", "length": 9, "reversed": false}}]}"#;

#[test]
fn candidates_from_every_live_body_are_ordered_as_one_list_its_own_features_first() {
    // A face of the second pad said to come of two of the sketch's lines, which none does:
    // each pad has a face of each line, so four faces share one line each with it. Equally
    // likely, the broken name's own feature's come first, then by name, whichever body is
    // first in the timeline.
    let model = model_of(TWO_PADS);
    let kernel = FakeKernel::new(1).unwrap();
    let broken =
        "p0000002.pad.Face(G s0000001.sketch.Edge.na000000, G s0000001.sketch.Edge.nb000000)";
    let refused = distance(
        &model,
        &kernel,
        &pair("p0000001.pad.Face(G s0000001.sketch.Face1)", broken),
        None,
    )
    .unwrap_err();
    assert_eq!(refused.total(), 1);
    assert_eq!(
        refused.errors()[0].candidates(),
        [
            "p0000002.pad.Face(G s0000001.sketch.Edge.na000000)",
            "p0000002.pad.Face(G s0000001.sketch.Edge.nb000000)",
            "p0000001.pad.Face(G s0000001.sketch.Edge.na000000)",
            "p0000001.pad.Face(G s0000001.sketch.Edge.nb000000)",
        ]
    );
}

#[test]
fn closing_a_session_stops_its_reads_kernel_work_and_says_so() {
    // A published distance held in the kernel: closing its session retires the read's own
    // client, so the read answers at once, saying the session closed, rather than waiting on
    // its request (up to the kernel's 60 s timeout); and dropping the sessions waits for
    // none.
    use std::time::{Duration, Instant};
    let f = fixture();
    let s = f.open("part.silico", EXAMPLE);
    // Rebuilt and published first.
    answer(f.sessions.distance(
        &s,
        ModelChoice::Current,
        pair("k7f2qx3m.box.Face5", "k7f2qx3m.box.Face6"),
        None,
    ))
    .result
    .unwrap();
    let held = f
        .kernel
        .hold(|op| matches!(op, silico_core::kernel::Op::Distance { .. }));
    let reading = f.sessions.distance(
        &s,
        ModelChoice::Published,
        pair("k7f2qx3m.box.Face1", "k7f2qx3m.box.Face6"),
        None,
    );
    assert!(f.kernel.wait_running(1, Duration::from_secs(10)));
    let started = Instant::now();
    answer(f.sessions.close(&s)).result.unwrap();
    let read = match reading.wait_timeout(Duration::from_secs(10)) {
        Ok(read) => read,
        Err(_) => panic!("the read went on after its session closed"),
    };
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    let refused = read.result.unwrap_err();
    assert_eq!(refused.errors()[0].code(), ErrorCode::InvalidInput);
    assert_eq!(
        refused.errors()[0].message(),
        "the session closed before the kernel measured the distance"
    );
    assert_eq!(refused.errors()[0].parameter(), Some("session"));
    // Another, held, as the sessions are dropped: they don't wait for it.
    let s = f.open("again.silico", EXAMPLE);
    // Published by a read that asks the kernel nothing (the hold is still on).
    answer(f.sessions.elements(&s, ModelChoice::Current, None, None, 1))
        .result
        .unwrap();
    let _second = f.sessions.distance(
        &s,
        ModelChoice::Published,
        pair("k7f2qx3m.box.Face1", "k7f2qx3m.box.Face6"),
        None,
    );
    assert!(f.kernel.wait_running(1, Duration::from_secs(10)));
    let started = Instant::now();
    drop(f.sessions);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    held.release();
}

const FUSED: &str = r#"{"silico": 1, "units": {"length": "mm", "resolution": "1nm"}, "naming": 5, "features": [{"id": "aaaaaaaa", "type": "box", "v": 1, "name": "First", "position": [0, 0, 0], "length": 1, "width": 1, "height": 1}, {"id": "bbbbbbbb", "type": "box", "v": 1, "name": "Second", "position": [5, 0, 0], "length": 1, "width": 1, "height": 1}, {"id": "rrrrrrrr", "type": "fuse", "v": 1, "name": "Both", "base": "aaaaaaaa", "tool": "bbbbbbbb"}]}"#;

#[test]
fn an_unstable_name_is_taken_as_a_measure_answers_once_and_stores_it_nowhere() {
    // A fake whose fuse gives the base box back, its history reporting nothing of the base's
    // Face1 or Edge1 (as adversarial_entries.rs's an_element_nothing_names_is_unstable...):
    // Face1 is named by its number, `rrrrrrrr.fuse.Face1`, unstable. A fillet refuses such a
    // name; a measure, answered once and stored nowhere, takes it (design §2).
    use silico_core::kernel::{
        BooleanOp, BuiltData, ElementCounts, ElementFate, History, InputHistory, Op, Topology,
    };
    use silico_core::recompute::Recompute;
    use silico_protocol::Reply;
    let kept = |n: u32| ElementFate {
        kept: Some(n),
        ..ElementFate::default()
    };
    let none = |n: usize| vec![ElementFate::default(); n];
    let face1 = |rest: Vec<Vec<u32>>| -> Vec<Vec<u32>> {
        std::iter::once(vec![1, 2]).chain(rest).collect()
    };
    let history = History {
        inputs: vec![
            InputHistory {
                faces: none(1).into_iter().chain((2..=6).map(kept)).collect(),
                edges: none(1).into_iter().chain((2..=12).map(kept)).collect(),
                vertices: (1..=8).map(kept).collect(),
            },
            InputHistory {
                faces: none(6),
                edges: none(12),
                vertices: none(8),
            },
        ],
        result: ElementCounts {
            faces: 6,
            edges: 12,
            vertices: 8,
            solids: 1,
        },
        ends: Default::default(),
        topology: Topology {
            face_edges: face1(vec![vec![]; 5]),
            outer_edges: face1(vec![vec![]; 5]),
            edge_vertices: vec![vec![]; 12],
        },
    };
    let engine = Recompute::new(FakeKernel::new(2).unwrap());
    engine.kernel().answer_instead(move |op| match op {
        Op::Boolean {
            op: BooleanOp::Fuse,
            base,
            ..
        } => Some(Reply::Built(BuiltData {
            shape: base.clone(),
            history: history.clone(),
        })),
        _ => None,
    });
    let model = super::model::published(&engine, super::model::checked(FUSED));
    let all = silico_core::command::elements::entries(&model, &id("rrrrrrrr")).unwrap();
    let face = all
        .iter()
        .find(|e| e.name.to_string() == "rrrrrrrr.fuse.Face1")
        .unwrap();
    assert!(!face.stable);
    let kernel = FakeKernel::new(1).unwrap();
    let d = distance(
        &model,
        &kernel,
        &pair("rrrrrrrr.fuse.Face1", "aaaaaaaa.box.Face2"),
        None,
    )
    .unwrap();
    assert_eq!(d.distance, mm(1));
}
