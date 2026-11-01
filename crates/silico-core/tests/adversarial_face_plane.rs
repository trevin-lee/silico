//! break-it on sketches on faces (M5.9, docs/ports/attacher.md): the face plane's reader,
//! writer, and gate (`document/decode.rs`'s `plane`, `write.rs`'s `plane_out`, the gate's
//! stored names), the rebuild's rounds on the fake kernel (`Engine::run`, `Plan::of`'s
//! waits, `placed_faces`), and the `sketch` read of a face sketch (`command/sketch.rs`'s
//! `plane_of`). Hostile strings and shapes of the plane's object, a megabyte name, long
//! chains of sketches each on the previous pad's face (one round each), hundreds of
//! sketches on one box (one round), a solid that can't hold the name waiting or failing
//! before it, a rebuild superseded mid-rounds, and determinism across engines.
//!
//! The fake describes every face as a surface of type other (never a plane), so each
//! face it finds is refused as not planar: the rounds and refusals are tested here, the
//! frames on the real kernel (silico-kernel's tests/adversarial_face_plane_rounds.rs).
//! Expected values: the format's rules (D7), the note's *In rounds* (each round places the
//! first sketch that waits), and its *Where the face is looked for* (only solids built from
//! one of the name's makers can hold it, wait for it, or block it).

use super::model::WAIT;
use super::sweeps::{box_at, checked, checked_document, document, id, one_side, pad, rectangle};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use silico_core::cache::Cache;
use silico_core::command::{Call, Sessions};
use silico_core::document::{Checked, Operation, read, write};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{Generation, Kernel, KernelRequest, Op};
use silico_core::pool::Pending;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::sketch::SketchSolver;
use silico_core::{Error, ErrorCode};

fn face(name: &str, x: &str, reversed: bool) -> String {
    format!(
        r#"{{"face": {}, "x": "{x}", "reversed": {reversed}}}"#,
        serde_json::to_string(name).unwrap()
    )
}

/// A 4 × 2 rectangle from (1, 1) on the face plane `plane` (its text), sketch `id`.
fn on_face(id: &str, plane: &str) -> String {
    rectangle(id, "xy", [1.0, 1.0], 4.0, 2.0)
        .replace(r#""plane": "xy""#, &format!(r#""plane": {plane}"#))
}

fn cube() -> String {
    box_at("b0000001", [0.0, 0.0, 0.0], 10.0, 10.0, 10.0)
}

fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new())
}

fn publish(engine: &Recompute<FakeKernel>, document: Checked) -> Arc<Model> {
    match engine.start(document).wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the rebuild didn't finish in {WAIT:?}"),
    }
}

fn strs(features: &[String]) -> Vec<&str> {
    features.iter().map(String::as_str).collect()
}

/// The DOCUMENT of face_sketch.rs with its face plane's text `plane`.
fn with_plane(plane: &str) -> String {
    document(&[&cube(), &on_face("s0000001", plane)])
}

// ---- the reader and writer ----

#[test]
fn a_face_name_of_quotes_backslashes_controls_and_astral_characters_reads_and_writes_back_exactly()
{
    // Whatever the name holds, the reader takes it as written and the writer escapes it so
    // it reads back the same: then the gate refuses it as no name, on plane.face.
    for name in [
        "b0000001.box.Face6\"",
        "b0000001.box.Face6\\",
        "b0000001.box.Face6\n",
        "b0000001.box.Face6\u{0}",
        "b0000001.box.Face6\u{7f}",
        "b0000001.box.Face6\u{1F600}",
        "b0000001.box.Face6\u{2028}\u{2029}",
        "\u{FEFF}b0000001.box.Face6",
        " b0000001.box.Face6",
        "b0000001.box.Face6 ",
        "B0000001.box.Face6",
        "b0000001.BOX.Face6",
        "b0000001.box.face6",
        "b0000001.box.Face06",
        "b0000001.box.Face0",
        "b0000001.box.Face-1",
        "b0000001.box.Face18446744073709551617",
        "b0000001.box.Face4294967297",
        "b0000001..box.Face6",
        "b0000001.box.Face6.",
        ".",
        "b0000001",
        "b0000001.",
        "b0000001.box",
        "b0000001.box.",
    ] {
        let text = with_plane(&face(name, "+x", false));
        let document = read(text.as_bytes()).unwrap_or_else(|e| panic!("{name:?}: {e:?}"));
        let Operation::Sketch(sketch) = &document.features[1].operation else {
            panic!();
        };
        assert_eq!(sketch.plane.face().unwrap().face.as_str(), name, "{name:?}");
        let written = write(&document).unwrap();
        let again = read(written.as_bytes()).unwrap();
        assert_eq!(again, document, "{name:?}");
        assert_eq!(write(&again).unwrap(), written, "{name:?}");
        let invalid = silico_core::document::validate(&document).unwrap_err();
        let e = &invalid.errors()[0];
        assert_eq!(e.code(), ErrorCode::InvalidInput, "{name:?}");
        assert_eq!(e.feature(), Some("s0000001"), "{name:?}");
        assert_eq!(e.parameter(), Some("plane.face"), "{name:?}: {e:?}");
    }
}

#[test]
fn each_hostile_shape_of_the_plane_is_refused_naming_its_parameter() {
    let cases: [(&str, &str); 16] = [
        ("null", "plane"),
        ("true", "plane"),
        ("{}", "plane.face"),
        (
            r#"{"face": null, "x": "+x", "reversed": false}"#,
            "plane.face",
        ),
        (
            r#"{"face": ["b0000001.box.Face6"], "x": "+x", "reversed": false}"#,
            "plane.face",
        ),
        (
            r#"{"face": {"face": "b0000001.box.Face6"}, "x": "+x", "reversed": false}"#,
            "plane.face",
        ),
        (
            r#"{"face": "b0000001.box.Face6", "x": "+X", "reversed": false}"#,
            "plane.x",
        ),
        (
            r#"{"face": "b0000001.box.Face6", "x": "x", "reversed": false}"#,
            "plane.x",
        ),
        (
            r#"{"face": "b0000001.box.Face6", "x": " +x", "reversed": false}"#,
            "plane.x",
        ),
        (
            r#"{"face": "b0000001.box.Face6", "x": "+x\u0000", "reversed": false}"#,
            "plane.x",
        ),
        (
            r#"{"face": "b0000001.box.Face6", "x": "", "reversed": false}"#,
            "plane.x",
        ),
        (
            r#"{"face": "b0000001.box.Face6", "x": 1, "reversed": false}"#,
            "plane.x",
        ),
        (
            r#"{"face": "b0000001.box.Face6", "x": "+x", "reversed": 0}"#,
            "plane.reversed",
        ),
        (
            r#"{"face": "b0000001.box.Face6", "x": "+x", "reversed": null}"#,
            "plane.reversed",
        ),
        (
            r#"{"face": "b0000001.box.Face6", "x": "+x", "reversed": false, "plane": "xy"}"#,
            "plane.plane",
        ),
        (
            r#"{"face": "b0000001.box.Face6", "x": "+x", "reversed": false, "": 1}"#,
            "plane.\"\"",
        ),
    ];
    for (form, parameter) in cases {
        let error = read(with_plane(form).as_bytes()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{form}");
        assert_eq!(error.feature(), Some("s0000001"), "{form}");
        assert_eq!(error.parameter(), Some(parameter), "{form}: {error:?}");
        assert!(error.suggestion().is_some(), "{form}: {error:?}");
    }
    // A plane object nested a thousand deep is refused, not overflowed.
    let deep = format!(
        "{}{}",
        "{\"face\": ".repeat(1000),
        "1".to_owned() + &"}".repeat(1000)
    );
    let error = read(with_plane(&deep).as_bytes()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidInput, "{error:?}");
}

#[test]
fn a_megabyte_face_name_is_refused_by_the_gate_promptly_with_a_message_cut_short() {
    let name = format!("b0000001.box.Face6{}", "x".repeat(1 << 20));
    let text = with_plane(&face(&name, "+x", false));
    let started = Instant::now();
    let document = read(text.as_bytes()).unwrap();
    let invalid = silico_core::document::validate(&document).unwrap_err();
    let took = started.elapsed();
    let e = &invalid.errors()[0];
    assert_eq!(
        e.parameter(),
        Some("plane.face"),
        "{:?}",
        &e.message()[..200]
    );
    // Read and gated in well under a second's work, and the message doesn't echo the
    // megabyte back to the agent.
    assert!(took < Duration::from_secs(10), "{took:?}");
    assert!(e.message().len() < 4096, "{} bytes", e.message().len());
    // And a name nested a hundred thousand deep is refused, not overflowed.
    let deep = format!(
        "{}s0000001.sketch.Face1{}",
        "p0000001.pad.Face(G ".repeat(100_000),
        ")".repeat(100_000)
    );
    let text = with_plane(&face(&deep, "+x", false));
    let document = read(text.as_bytes()).unwrap();
    let invalid = silico_core::document::validate(&document).unwrap_err();
    assert_eq!(invalid.errors()[0].parameter(), Some("plane.face"));
}

// ---- the gate ----

#[test]
fn a_face_named_of_an_earlier_sketch_is_refused_by_the_gate_naming_what_to_name_instead() {
    // A sketch's faces (s0000001.sketch.Face1) are in no solid: the gate refuses the name
    // on plane.face, saying to name what a feature made of it, so no rebuild meets it.
    let features = [
        rectangle("s0000001", "xy", [0.0, 0.0], 4.0, 2.0),
        pad("p0000001", None, "s0000001", &one_side("3")),
        on_face("s0000002", &face("s0000001.sketch.Face1", "+x", false)),
    ];
    let invalid = checked(&strs(&features)).unwrap_err();
    assert_eq!(invalid.errors().len(), 1, "{:?}", invalid.errors());
    let e = &invalid.errors()[0];
    assert_eq!(e.code(), ErrorCode::InvalidInput);
    assert_eq!(e.feature(), Some("s0000002"));
    assert_eq!(e.parameter(), Some("plane.face"));
    assert_eq!(
        e.message(),
        "plane.face, \"s0000001.sketch.Face1\", isn't a face's name: s0000001 is a sketch: its elements are in no solid; name what a feature made of it, at column 1"
    );
    assert_eq!(
        e.suggestion(),
        Some(
            "write the face's name as Silico names it (feature.type.Face...), of a feature before this sketch"
        )
    );
    assert!(e.candidates().is_empty());
}

#[test]
fn a_face_of_a_suppressed_feature_named_through_a_fuse_is_refused_by_the_gate() {
    // The box b2 is suppressed and the fuse passes b1 through; a sketch on b2's face is a
    // dependent of a suppressed feature left on: the gate's to refuse, on plane.face.
    let flagged = box_at("b0000002", [20.0, 0.0, 0.0], 1.0, 1.0, 1.0).replace(
        r#""name": "Box", "#,
        r#""name": "Box", "suppressed": true, "#,
    );
    let fuse = r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "Both", "base": "b0000001", "tool": "b0000002"}"#;
    let invalid = checked(&[
        &cube(),
        &flagged,
        fuse,
        &on_face("s0000001", &face("b0000002.box.Face6", "+x", false)),
    ])
    .unwrap_err();
    let e = invalid
        .errors()
        .iter()
        .find(|e| e.feature() == Some("s0000001"))
        .unwrap_or_else(|| panic!("{:#?}", invalid.errors()));
    assert_eq!(e.parameter(), Some("plane.face"), "{e:?}");
}

// ---- the rounds ----

/// A chain of `n` sketches, each on the top of the pad before it (the first on the box's),
/// each padded onto the one before: `n` rounds.
fn chain(n: usize) -> Vec<String> {
    let mut features = vec![cube()];
    for k in 1..=n {
        let name = if k == 1 {
            "b0000001.box.Face6".to_owned()
        } else {
            format!("p{:07}.pad.Face(G s{:07}.sketch.Face1)", k - 1, k - 1)
        };
        features.push(on_face(&format!("s{k:07}"), &face(&name, "+x", false)));
        let base = if k == 1 {
            "b0000001".to_owned()
        } else {
            format!("p{:07}", k - 1)
        };
        features.push(pad(
            &format!("p{k:07}"),
            Some(&base),
            &format!("s{k:07}"),
            &one_side("1"),
        ));
    }
    features
}

/// A chain of `n` rebuilt on the fake: the rounds it took. Each sketch but the first is
/// blocked on the first, whose face the fake doesn't type a plane.
fn rebuild_chain(n: usize) -> usize {
    let features = chain(n);
    let document = checked_document(&strs(&features));
    let engine = engine();
    let run = engine.start(document);
    let progress = run.progress();
    let model = match run.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the rebuild didn't finish in {WAIT:?}"),
    };
    let first = id("s0000001");
    assert!(
        matches!(model.outcome(&first), Some(FeatureOutcome::Broken { .. })),
        "{:?}",
        model.outcome(&first)
    );
    for k in 2..=n {
        for feature in [format!("s{k:07}"), format!("p{k:07}")] {
            assert_eq!(
                model.outcome(&id(&feature)),
                Some(&FeatureOutcome::Blocked { on: first.clone() }),
                "{feature}"
            );
        }
    }
    assert_eq!(model.error_count(), 1);
    progress.rounds()
}

#[test]
fn a_chain_of_sixty_sketches_each_on_the_last_pads_face_rebuilds_in_sixty_rounds() {
    // Sixty sketches on faces, each on the one before's pad: a part modelled the way
    // FreeCAD users model, every feature sketched on the last. Each round places one, so
    // sixty rounds, and one more that finds none waiting. (Each round's walk is linear in
    // the timeline, counted, not timed, in recompute's unit tests:
    // many_sketches_on_one_face_of_a_chain_cost_each_round_a_walk_linear_in_the_timeline.)
    assert_eq!(rebuild_chain(60), 61);
}

#[test]
fn a_chain_of_two_hundred_sketches_on_faces_takes_a_round_a_link() {
    // BREAKS (found by break-it, timed then): each round ran `placed_faces` over every
    // sketch still waiting, and each walked every feature before it, though only the first
    // can be placed: n rounds of n sketches of n features, n³ (80 links 0.64 s, 160 4.5 s,
    // 320 36 s in a debug build). Now counted, not timed (recompute's unit tests count the
    // walk a round); here, the rounds: one a link, and one more.
    assert_eq!(rebuild_chain(100), 101);
    assert_eq!(rebuild_chain(200), 201);
}

#[test]
fn four_hundred_sketches_on_one_boxs_faces_resolve_in_one_round() {
    let mut features = vec![cube()];
    for k in 1..=400 {
        let face_number = (k % 6) + 1;
        features.push(on_face(
            &format!("s{k:07}"),
            &face(&format!("b0000001.box.Face{face_number}"), "+x", false),
        ));
    }
    let document = checked_document(&strs(&features));
    let engine = engine();
    let run = engine.start(document);
    let progress = run.progress();
    let model = match run.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        other => panic!("{:?}", other.map(|r| r.map(|_| ())).ok()),
    };
    // Every sketch placed (refused, on the fake) in the first round, and a second that
    // finds none waiting. (The walk, counted: recompute's unit test
    // four_hundred_sketches_on_one_boxs_faces_are_looked_for_in_one_walk_of_the_timeline.)
    assert_eq!(progress.rounds(), 2);
    for k in 1..=400 {
        let sketch = id(&format!("s{k:07}"));
        let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&sketch) else {
            panic!("{sketch}: {:?}", model.outcome(&sketch));
        };
        assert!(errors[0].message().contains("isn't planar"), "{errors:?}");
    }
}

#[test]
fn a_new_body_padded_from_a_sketch_on_the_box_neither_holds_nor_blocks_a_later_sketch_on_the_box() {
    // p1 is a body of its own (no base), made from s1 on the box's top: it isn't built from
    // the box, so it can't hold a name of the box's. s2 names the box's Face7, which isn't
    // one: broken, with the box's Face6 as its candidate, though p1 waits in the first
    // round and is blocked in the second.
    let features = [
        cube(),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
        pad("p0000001", None, "s0000001", &one_side("3")),
        on_face("s0000002", &face("b0000001.box.Face7", "+x", false)),
    ];
    let model = publish(&engine(), checked_document(&strs(&features)));
    assert_eq!(
        model.outcome(&id("p0000001")),
        Some(&FeatureOutcome::Blocked { on: id("s0000001") })
    );
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("s0000002")) else {
        panic!("{:?}", model.outcome(&id("s0000002")));
    };
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
    assert_eq!(errors[0].parameter(), Some("plane.face"));
    assert_eq!(errors[0].candidates(), ["b0000001.box.Face6"]);
}

#[test]
fn a_sketch_on_a_face_of_a_box_consumed_by_a_fillet_before_it_is_looked_for_in_the_fillet() {
    // The box is consumed by the fillet; its top is found in the fillet's solid, never in
    // the box's (which isn't live), and refused there as the fake types it: not planar.
    // A blocked or bug outcome would mean the consumed box was searched, or nothing was.
    let fillet = r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "F", "base": "b0000001", "edges": ["b0000001.box.Edge1"], "radius": 1}"#;
    let features = [
        cube(),
        fillet.to_owned(),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
        pad("p0000001", Some("f0000001"), "s0000001", &one_side("1")),
    ];
    let model = publish(&engine(), checked_document(&strs(&features)));
    let outcome = model.outcome(&id("s0000001"));
    let Some(FeatureOutcome::Broken { errors, .. }) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken, "{errors:?}");
    assert_eq!(errors[0].parameter(), Some("plane.face"));
    // Found (in the fillet's solid, the fillet keeping the top), then refused as the fake
    // types it: not "names no face", which would mean it wasn't found.
    assert!(
        errors[0].message().contains("isn't planar"),
        "{}",
        errors[0].message()
    );
    assert!(!errors[0].message().contains("names no face"));
    assert_eq!(
        model.outcome(&id("p0000001")),
        Some(&FeatureOutcome::Blocked { on: id("s0000001") })
    );
}

#[test]
fn a_sketch_on_a_fuses_tool_face_is_looked_for_in_the_fuse() {
    let fuse = r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "Both", "base": "b0000001", "tool": "b0000002"}"#;
    let features = [
        cube(),
        box_at("b0000002", [20.0, 0.0, 0.0], 1.0, 1.0, 1.0),
        fuse.to_owned(),
        on_face("s0000001", &face("b0000002.box.Face6", "+x", false)),
    ];
    let model = publish(&engine(), checked_document(&strs(&features)));
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("s0000001")) else {
        panic!("{:?}", model.outcome(&id("s0000001")));
    };
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
    // Found in the fuse's solid, then refused as the fake types it, not as no face.
    assert!(
        errors[0].message().contains("isn't planar"),
        "{}",
        errors[0].message()
    );
    assert!(!errors[0].message().contains("names no face"));
}

#[test]
fn a_chain_rebuilt_on_two_engines_and_twice_on_one_gives_the_same_outcomes() {
    // Determinism (ARCHITECTURE.md, invariant 5): the rounds' order doesn't depend on which
    // thread finishes first.
    let features = chain(12);
    let outcomes = |model: &Model| {
        (0..features.len())
            .map(|i| {
                let feature = if i == 0 {
                    "b0000001".to_owned()
                } else if i % 2 == 1 {
                    format!("s{:07}", i.div_ceil(2))
                } else {
                    format!("p{:07}", i / 2)
                };
                format!("{feature}: {:?}", model.outcome(&id(&feature)))
            })
            .collect::<Vec<_>>()
    };
    let one = engine();
    let first = outcomes(&publish(&one, checked_document(&strs(&features))));
    let warm = outcomes(&publish(&one, checked_document(&strs(&features))));
    let other = outcomes(&publish(&engine(), checked_document(&strs(&features))));
    assert_eq!(first, warm);
    assert_eq!(first, other);
}

#[test]
fn a_rebuild_superseded_in_its_rounds_gives_way_and_the_next_publishes_whole() {
    // A long chain started, and at once a short one: the first gives way somewhere in its
    // rounds, and the second publishes, every sketch's outcome its own.
    let engine = engine();
    let long = engine.start(checked_document(&strs(&chain(80))));
    let short_features = chain(3);
    let short = engine.start(checked_document(&strs(&short_features)));
    match long.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Superseded | RunOutcome::Published(_))) => {}
        other => panic!("{:?}", other.map(|r| r.map(|_| ())).ok()),
    }
    let model = match short.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        other => panic!("{:?}", other.map(|r| r.map(|_| ())).ok()),
    };
    assert!(model.outcome(&id("s0000004")).is_none());
    for k in 2..=3 {
        assert_eq!(
            model.outcome(&id(&format!("p{k:07}"))),
            Some(&FeatureOutcome::Blocked { on: id("s0000001") })
        );
    }
    // And the engine still rebuilds the long one whole.
    let model = publish(&engine, checked_document(&strs(&chain(80))));
    assert_eq!(
        model.outcome(&id("p0000080")),
        Some(&FeatureOutcome::Blocked { on: id("s0000001") })
    );
}

/// A box, a sketch on its top, and a pad of it: two rounds (the sketch's face is looked for
/// once the box is built), the sketch refused on the fake and the pad blocked.
fn two_rounds() -> Vec<String> {
    vec![
        cube(),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
        pad("p0000001", Some("b0000001"), "s0000001", &one_side("3")),
    ]
}

/// How many requests `picks` picks the fake kernel started.
fn started(kernel: &FakeKernel, picks: impl Fn(&Op) -> bool) -> usize {
    kernel.calls().iter().filter(|call| picks(&call.op)).count()
}

#[test]
fn a_fresh_rebuild_in_two_rounds_makes_the_box_once() {
    // The second round takes the first's steps as they are, by key, reading nothing kept
    // from before the rebuild: the box is made once, not again for the round after.
    let engine = engine();
    let run = engine.start_fresh(checked_document(&strs(&two_rounds())));
    let progress = run.progress();
    match run.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(_))) => {}
        other => panic!("{:?}", other.map(|r| r.map(|_| ())).ok()),
    }
    assert_eq!(progress.rounds(), 2);
    assert_eq!(started(engine.kernel(), |op| op.name() == "MakeBox"), 1);
}

#[test]
fn a_fillets_broken_references_are_all_said_after_the_rounds() {
    // Found by the tests review: a fillet's broken references are kept by its step's key
    // across the rounds (`unresolved`), the step itself kept failed. A fillet with one
    // edge that's there and two that aren't, and a sketch on the box's top after it (two
    // rounds): the fillet is broken with both, the total 2, as in a rebuild of one round.
    let fillet = r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "F", "base": "b0000001", "edges": ["b0000001.box.Edge1", "b0000001.box.Edge13", "b0000001.box.Edge40"], "radius": 1}"#;
    let features = [
        cube(),
        fillet.to_owned(),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
    ];
    let engine = engine();
    let run = engine.start(checked_document(&strs(&features)));
    let progress = run.progress();
    let model = match run.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        other => panic!("{:?}", other.map(|r| r.map(|_| ())).ok()),
    };
    assert_eq!(progress.rounds(), 2);
    let Some(FeatureOutcome::Broken { errors, total }) = model.outcome(&id("f0000001")) else {
        panic!("{:?}", model.outcome(&id("f0000001")));
    };
    assert_eq!(*total, 2);
    let said: Vec<(Option<&str>, Vec<String>)> = errors
        .iter()
        .map(|e| (e.parameter(), e.elements().to_vec()))
        .collect();
    assert_eq!(
        said,
        [
            (Some("edges.1"), vec!["b0000001.box.Edge13".to_owned()]),
            (Some("edges.2"), vec!["b0000001.box.Edge40".to_owned()]),
        ]
    );
    // And the sketch, looked for in the fillet's solid, is blocked by it.
    assert_eq!(
        model.outcome(&id("s0000001")),
        Some(&FeatureOutcome::Blocked { on: id("f0000001") })
    );
}

#[test]
fn a_summary_that_failed_once_in_the_first_round_is_asked_no_more_that_rebuild() {
    // The robustness review: a measure a worker failed in the first round was asked again
    // in the second, and, measured then, its first failure stayed beside the measure: a
    // solid summarized and unmeasured at once. Now each solid is measured once a rebuild:
    // the box's summary, failed by a crash the first time it's asked, is unmeasured with
    // that error, and asked again only by the next rebuild.
    let engine = engine();
    let once = Arc::new(AtomicBool::new(true));
    let first = Arc::clone(&once);
    engine.kernel().refuse(move |op| {
        (matches!(op, Op::Summarize { .. }) && first.swap(false, Ordering::SeqCst))
            .then(|| Error::new(ErrorCode::WorkerCrashed, "the kernel worker crashed"))
    });
    let model = publish(&engine, checked_document(&strs(&two_rounds())));
    assert_eq!(
        started(engine.kernel(), |op| matches!(op, Op::Summarize { .. })),
        1
    );
    let Some(FeatureOutcome::Built { built, .. }) = model.outcome(&id("b0000001")) else {
        panic!("{:?}", model.outcome(&id("b0000001")));
    };
    assert!(built.summary().is_none());
    let unmeasured = model.unmeasured(&id("b0000001")).map(Error::code);
    assert_eq!(unmeasured, Some(ErrorCode::WorkerCrashed));
    // The next rebuild asks again, and measures it: measured, and no longer unmeasured.
    let model = publish(&engine, checked_document(&strs(&two_rounds())));
    assert_eq!(
        started(engine.kernel(), |op| matches!(op, Op::Summarize { .. })),
        2
    );
    let Some(FeatureOutcome::Built { built, .. }) = model.outcome(&id("b0000001")) else {
        panic!();
    };
    assert!(built.summary().is_some());
    assert!(model.unmeasured(&id("b0000001")).is_none());
}

#[test]
fn a_describe_that_crashes_its_worker_is_asked_once_a_rebuild_not_once_a_round() {
    let engine = engine();
    engine.kernel().refuse(|op| {
        matches!(op, Op::DescribeAll { .. })
            .then(|| Error::new(ErrorCode::WorkerCrashed, "the kernel worker crashed"))
    });
    let model = publish(&engine, checked_document(&strs(&two_rounds())));
    assert_eq!(
        started(engine.kernel(), |op| matches!(op, Op::DescribeAll { .. })),
        1
    );
    // The sketch on the box's face fails with why the box has no description.
    let Some(FeatureOutcome::Failed(e)) = model.outcome(&id("s0000001")) else {
        panic!("{:?}", model.outcome(&id("s0000001")));
    };
    assert_eq!(e.code(), ErrorCode::WorkerCrashed, "{e:?}");
    // A rebuild asks it again (a crash may not be the solid's), once.
    publish(&engine, checked_document(&strs(&two_rounds())));
    assert_eq!(
        started(engine.kernel(), |op| matches!(op, Op::DescribeAll { .. })),
        2
    );
}

/// The fake kernel, whose identity, asked once a round by a recompute with a cache, holds
/// the first round of a rebuild armed until it's let go: so a test can act between rounds.
struct Gated {
    kernel: FakeKernel,
    gate: Arc<(Mutex<Gate>, Condvar)>,
}

#[derive(Default)]
struct Gate {
    armed: bool,
    asked: usize,
    held: bool,
    let_go: bool,
}

impl Kernel for Gated {
    fn identity(&self) -> &str {
        let (gate, changed) = &*self.gate;
        let mut state = gate.lock().unwrap();
        if state.armed {
            state.asked += 1;
            if state.asked == 1 {
                state.held = true;
                changed.notify_all();
                while !state.let_go {
                    state = changed.wait(state).unwrap();
                }
            }
        }
        self.kernel.identity()
    }

    fn generation(&self) -> Generation {
        self.kernel.generation()
    }

    fn advance(&self, generation: Generation) {
        self.kernel.advance(generation);
    }

    fn submit<T: Send + 'static>(
        &self,
        generation: Generation,
        request: KernelRequest<T>,
    ) -> Pending<T> {
        self.kernel.submit(generation, request)
    }
}

#[test]
fn a_warm_rebuild_cancelled_between_its_rounds_plans_no_later_round() {
    // The robustness review: a round whose steps are all kept asks the kernel nothing, so
    // only a check at each round's top stops a cancelled rebuild before it plans every
    // round left (a long chain's, each its walk). Built once to warm it, then rebuilt with
    // the first round held at its kernel's identity (asked once a round, as the cache is
    // read), cancelled there, and let go: superseded, with that one round planned.
    let dir = tempfile::tempdir().unwrap();
    let gate = Arc::new((Mutex::new(Gate::default()), Condvar::new()));
    let engine = Recompute::with_cache(
        Gated {
            kernel: FakeKernel::new(2).unwrap(),
            gate: Arc::clone(&gate),
        },
        Cache::unsealed(dir.path()),
    )
    .with_sketches(FakeSketches::new());
    let features = chain(3);
    let warm = engine.start(checked_document(&strs(&features)));
    let warmed = warm.progress();
    assert!(matches!(
        warm.wait_timeout(WAIT),
        Ok(Ok(RunOutcome::Published(_)))
    ));
    assert_eq!(warmed.rounds(), 4);
    gate.0.lock().unwrap().armed = true;
    let run = engine.start(checked_document(&strs(&features)));
    let progress = run.progress();
    {
        let (state, changed) = &*gate;
        let mut state = state.lock().unwrap();
        while !state.held {
            let (next, timeout) = changed.wait_timeout(state, WAIT).unwrap();
            assert!(!timeout.timed_out(), "the first round never asked");
            state = next;
        }
    }
    engine.cancel();
    {
        let (state, changed) = &*gate;
        state.lock().unwrap().let_go = true;
        changed.notify_all();
    }
    assert!(matches!(
        run.wait_timeout(WAIT),
        Ok(Ok(RunOutcome::Superseded))
    ));
    assert_eq!(progress.rounds(), 1);
    assert_eq!(gate.0.lock().unwrap().asked, 1);
}

// ---- the sketch read ----

fn sessions_on(dir: &tempfile::TempDir, features: &[String]) -> Sessions<Arc<FakeKernel>> {
    let kernel = Arc::new(FakeKernel::new(2).unwrap());
    let sessions = Sessions::with_stand_ins(
        kernel,
        |cache: &std::path::Path| Cache::unsealed(cache),
        || Ok(Box::new(FakeSketches::new()) as Box<dyn SketchSolver>),
    );
    let path = dir.path().join("part.silico");
    std::fs::write(&path, document(&strs(features))).unwrap();
    let opened = call(
        &sessions,
        "open",
        &json!({"path": path.to_str().unwrap(), "create": false}).to_string(),
    );
    assert_eq!(opened["ok"], true, "{opened}");
    sessions
}

fn call(sessions: &Sessions<Arc<FakeKernel>>, name: &str, arguments: &str) -> Value {
    match sessions.call(name, arguments) {
        Call::Running(calling) => {
            let answered = calling
                .wait_timeout(WAIT)
                .unwrap_or_else(|_| panic!("{name} didn't answer in {WAIT:?}"));
            serde_json::from_str(&answered.text).unwrap()
        }
        Call::Unknown => panic!("no command {name}"),
    }
}

#[test]
fn the_sketch_read_of_a_face_sketch_blocked_upstream_answers_naming_the_root_not_a_bug() {
    // s2's face could only be in p1, which is blocked on s1 (not planar on the fake): the
    // read of s2 answers, its plane null and its plane problem s2 blocked on s1, as check
    // reports it, never `internal`; s1's, its own face refused.
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions_on(&dir, &chain(2));
    for (sketch, code) in [
        ("s0000001", "reference_broken"),
        ("s0000002", "invalid_input"),
    ] {
        let read = call(
            &sessions,
            "sketch",
            &json!({"session": "s1", "model": "current", "sketch": sketch}).to_string(),
        );
        assert_eq!(read["ok"], true, "{read}");
        assert_eq!(read["result"]["plane"], Value::Null, "{read}");
        let error = &read["result"]["plane_problem"];
        assert_eq!(error["code"], code, "{sketch}: {read}");
        assert_eq!(error["feature"], sketch, "{read}");
        if sketch == "s0000001" {
            assert_eq!(error["parameter"], "plane.face", "{read}");
        } else {
            assert!(
                error["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("uses s0000001, which failed")),
                "{sketch}'s plane problem names its root: {read}"
            );
        }
    }
    // A read of the box that isn't a sketch, and of a sketch that doesn't exist, are the
    // read's own refusals, and the session still answers.
    let read = call(
        &sessions,
        "sketch",
        &json!({"session": "s1", "model": "current", "sketch": "b0000001"}).to_string(),
    );
    assert_eq!(read["ok"], false, "{read}");
    assert_ne!(read["errors"][0]["code"], "internal", "{read}");
    let status = call(&sessions, "status", &json!({"session": "s1"}).to_string());
    assert_eq!(status["ok"], true, "{status}");
}

/// The fake's description of `shape` with each face's surface as `surface` gives it, by
/// number from 1: a planar face whose normal is +Z, through the origin, or a cylinder's.
fn surfaced(
    shape: &silico_core::kernel::ShapeData,
    planar: impl Fn(usize) -> bool,
) -> Option<silico_protocol::Reply> {
    use silico_protocol::{Axis, Surface};
    let mut description = FakeKernel::described(shape)?;
    for (number, face) in (1..).zip(description.faces.iter_mut()) {
        face.surface = if planar(number) {
            Surface::Plane {
                normal: [0.0, 0.0, 1.0],
                location: [0.0, 0.0, 0.0],
            }
        } else {
            Surface::Cylinder {
                axis: Axis {
                    origin: [0.0; 3],
                    direction: [0.0, 0.0, 1.0],
                },
                radius: 1.0,
            }
        };
    }
    Some(silico_protocol::Reply::DescribedAll(description))
}

#[test]
fn a_broken_faces_candidates_are_the_same_bytes_however_long_another_branch_is_held() {
    // From Main: candidates read across live solids must not depend on which settles
    // first. The box b1's faces are a cylinder's but its Face1; every other solid's are
    // planes. s1, on b1's top, is refused as not planar, its candidates b1's Face1, then the
    // planar faces of the other live solids by name: the pad p2's, a branch of its own
    // (b2, sketched on its top in a round of its own, padded). So s1's refusal waits for
    // p2, which a first round hasn't built. Each of several fresh sessions holds p2's
    // sweep a different time; check's envelope is the same bytes in each. A determinism
    // check, not a race: rounds are barriers (a round's plan is made once the round before
    // has settled every step), so the holds don't change the order steps settle in across
    // rounds, and the refusal's wait for p2 is what makes its candidates whole
    // (face_plane.rs's tests of `place` pin the wait itself).
    let features = vec![
        cube(),
        box_at("b0000002", [20.0, 0.0, 0.0], 1.0, 1.0, 1.0),
        on_face("s0000002", &face("b0000002.box.Face6", "+x", false)),
        pad("p0000002", Some("b0000002"), "s0000002", &one_side("1")),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
    ];
    let mut envelopes = Vec::new();
    for held in [0u64, 5, 40, 150] {
        let dir = tempfile::tempdir().unwrap();
        let kernel = Arc::new(FakeKernel::new(2).unwrap());
        kernel.answer_instead(|op| match op {
            Op::DescribeAll { shape } => {
                let b1 = FakeKernel::describe(shape).as_deref() == Some("box(10, 10, 10)");
                surfaced(shape, |number| !b1 || number == 1)
            }
            _ => None,
        });
        let hold = kernel.hold(|op| matches!(op, Op::Prism { .. }));
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(held));
            hold.release();
        });
        let sessions = Sessions::with_stand_ins(
            Arc::clone(&kernel),
            |cache: &std::path::Path| Cache::unsealed(cache),
            || Ok(Box::new(FakeSketches::new()) as Box<dyn SketchSolver>),
        );
        let path = dir.path().join("part.silico");
        std::fs::write(&path, document(&strs(&features))).unwrap();
        let opened = call(
            &sessions,
            "open",
            &json!({"path": path.to_str().unwrap(), "create": false}).to_string(),
        );
        assert_eq!(opened["ok"], true, "{opened}");
        let arguments = json!({"session": "s1", "model": "current", "fresh": false}).to_string();
        let checked = match sessions.call("check", &arguments) {
            Call::Running(calling) => match calling.wait_timeout(WAIT) {
                Ok(answered) => answered.text,
                Err(_) => panic!("check didn't answer in {WAIT:?}"),
            },
            Call::Unknown => panic!("no check"),
        };
        release.join().unwrap();
        // Each session's file is its own: the rest of the envelope is compared.
        envelopes.push(checked.replace(path.to_str().unwrap(), "PART"));
    }
    for (envelope, held) in envelopes.iter().zip([0, 5, 40, 150]).skip(1) {
        assert_eq!(envelope, &envelopes[0], "held {held} ms");
    }
    let checked: Value = serde_json::from_str(&envelopes[0]).unwrap();
    let errors = checked["result"]["problems"]
        .as_array()
        .unwrap_or_else(|| panic!("{checked}"));
    let refused = errors
        .iter()
        .find(|e| e["feature"] == "s0000001")
        .unwrap_or_else(|| panic!("{checked}"));
    assert_eq!(refused["code"], "reference_broken", "{refused}");
    let candidates: Vec<&str> = refused["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    // b1's one planar face, its own feature's, first; then p2's by name, those it keeps of
    // b2 named as b2 names them (p2 consumes b2): five in all.
    assert_eq!(
        candidates,
        [
            "b0000001.box.Face1",
            "b0000002.box.Face1",
            "b0000002.box.Face2",
            "b0000002.box.Face3",
            "b0000002.box.Face4",
        ],
        "{refused}"
    );
}

/// An engine whose fake kernel describes the 10 mm box's faces (`box(10, 10, 10)`) as
/// planes, every other solid's as the fake does (no plane): so a sketch on the box's face
/// is placed only if it's found in the box's own solid.
fn engine_with_a_planar_box() -> Recompute<FakeKernel> {
    let kernel = FakeKernel::new(2).unwrap();
    kernel.answer_instead(|op| match op {
        Op::DescribeAll { shape }
            if FakeKernel::describe(shape).as_deref() == Some("box(10, 10, 10)") =>
        {
            surfaced(shape, |_| true)
        }
        _ => None,
    });
    Recompute::new(kernel).with_sketches(FakeSketches::new())
}

#[test]
fn a_face_is_looked_for_past_a_suppressed_consumer_in_what_consumes_its_box_next() {
    // f1, a fillet of the box, is suppressed; f2, a fillet of f1, builds on the box (f1's
    // base passed through), so it consumes the box. s1, after f2, finds the top in f2's
    // solid (typed no plane): refused as not planar. Had the walk stopped at the
    // suppressed fillet, the box would be live, and s1 placed on its plane.
    let suppressed = r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "F", "suppressed": true, "base": "b0000001", "edges": ["b0000001.box.Edge1"], "radius": 1}"#;
    let after = r#"{"id": "f0000002", "type": "fillet", "v": 1, "name": "G", "base": "f0000001", "edges": ["b0000001.box.Edge2"], "radius": 1}"#;
    let features = [
        cube(),
        suppressed.to_owned(),
        after.to_owned(),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
    ];
    let model = publish(
        &engine_with_a_planar_box(),
        checked_document(&strs(&features)),
    );
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("s0000001")) else {
        panic!("{:?}", model.outcome(&id("s0000001")));
    };
    assert!(errors[0].message().contains("isn't planar"), "{errors:?}");
    assert!(model.frame(&id("s0000001")).is_none());
}

#[test]
fn a_face_is_looked_for_in_its_box_when_what_consumes_the_box_comes_after_the_sketch() {
    // s1 is on the box's top before the fillet that consumes the box: the box is live at
    // s1's place, so its plane (the box's description, planes) places s1: o = n(n·p) with
    // n = +Z through the origin, (0, 0, 0), x = +X, y = +Y.
    let fillet = r#"{"id": "f0000001", "type": "fillet", "v": 1, "name": "F", "base": "b0000001", "edges": ["b0000001.box.Edge1"], "radius": 1}"#;
    let features = [
        cube(),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
        fillet.to_owned(),
    ];
    let model = publish(
        &engine_with_a_planar_box(),
        checked_document(&strs(&features)),
    );
    assert!(
        matches!(
            model.outcome(&id("s0000001")),
            Some(FeatureOutcome::Sketch(_))
        ),
        "{:?}",
        model.outcome(&id("s0000001"))
    );
    let frame = model.frame(&id("s0000001")).unwrap();
    assert_eq!(
        (frame.origin, frame.x, frame.y),
        ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0])
    );
}

#[test]
fn a_step_never_run_in_two_rounds_is_counted_once_so_done_stays_under_steps_while_one_is_held() {
    // From #344's review. A pocket that removes nothing (its box above the sketch's plane)
    // is refused, and kept so by a first rebuild. A second, on the same engine, adds a cut
    // of that pocket (made from a refused feature, so never run: settled without a result)
    // and a sketch on the 10 mm box's top, padded onto it: a second round. That round plans
    // the cut unsettled again (it has no result) and settles it again. Its pad's last
    // step, the fuse onto the box, is held: while it is, one step is still to settle, so
    // `done` is under `steps`. Counted twice, the cut made them equal.
    let engine = engine_with_a_planar_box();
    let refused_pocket = [
        cube(),
        box_at("b0000002", [-1.0, -1.0, 1.0], 9.0, 9.0, 9.0),
        rectangle("s0000009", "xy", [0.0, 0.0], 4.0, 2.0),
        super::sweeps::pocket("q0000001", "b0000002", "s0000009", super::sweeps::TEN),
    ];
    let first = publish(&engine, checked_document(&strs(&refused_pocket)));
    assert!(
        matches!(
            first.outcome(&id("q0000001")),
            Some(FeatureOutcome::Failed(_))
        ),
        "{:?}",
        first.outcome(&id("q0000001"))
    );
    let mut features = refused_pocket.to_vec();
    features.extend([
        box_at("b0000003", [30.0, 0.0, 0.0], 1.0, 1.0, 1.0),
        r#"{"id": "c0000001", "type": "cut", "v": 1, "name": "Cut", "base": "q0000001", "tool": "b0000003"}"#
            .to_owned(),
        on_face("s0000001", &face("b0000001.box.Face6", "+x", false)),
        pad("p0000001", Some("b0000001"), "s0000001", &one_side("3")),
    ]);
    let fuse = |op: &Op| {
        matches!(
            op,
            Op::Boolean {
                op: silico_protocol::BooleanOp::Fuse,
                ..
            }
        )
    };
    let hold = engine.kernel().hold(fuse);
    let run = engine.start(checked_document(&strs(&features)));
    let progress = run.progress();
    let deadline = Instant::now() + WAIT;
    while !engine.kernel().asked().iter().any(|call| fuse(&call.op)) {
        assert!(Instant::now() < deadline, "the pad's fuse was never asked");
        std::thread::sleep(Duration::from_millis(2));
    }
    let held = progress.now();
    assert_eq!(progress.rounds(), 2, "{held:?}");
    assert!(held.done < held.steps, "{held:?}");
    assert_eq!(
        held.done + 1,
        held.steps,
        "only the held fuse is left: {held:?}"
    );
    hold.release();
    let model = match run.wait_timeout(WAIT) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the rebuild didn't finish in {WAIT:?}"),
    };
    assert!(matches!(
        model.outcome(&id("p0000001")),
        Some(FeatureOutcome::Built { .. })
    ));
    assert_eq!(
        model.outcome(&id("c0000001")),
        Some(&FeatureOutcome::Blocked { on: id("q0000001") })
    );
    let done = progress.now();
    assert_eq!((done.done, done.ended), (done.steps, true), "{done:?}");
}

// ---- any face plane, read, written, gated, and rebuilt ----

use proptest::prelude::*;

/// Names built of the pieces a face's name is made of and the characters that break text:
/// IDs before and after the sketch, type words, element words and numbers (0, huge),
/// parentheses, history codes, `~`, quotes, escapes, controls, and astral characters.
fn face_text() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("b0000001".to_owned()),
        Just("s0000001".to_owned()),
        Just("p0000001".to_owned()),
        Just("zzzzzzzz".to_owned()),
        Just(".box.".to_owned()),
        Just(".pad.".to_owned()),
        Just(".sketch.".to_owned()),
        Just(".fuse.".to_owned()),
        Just("Face".to_owned()),
        Just("Edge".to_owned()),
        Just("Vertex".to_owned()),
        Just("(G ".to_owned()),
        Just("(M ".to_owned()),
        Just(")".to_owned()),
        Just("~1".to_owned()),
        Just(";".to_owned()),
        Just("\"".to_owned()),
        Just("\\".to_owned()),
        Just("\n".to_owned()),
        Just("\u{0}".to_owned()),
        Just("\u{202e}".to_owned()),
        Just("\u{1F600}".to_owned()),
        Just(" ".to_owned()),
        (0u64..10).prop_map(|n| n.to_string()),
        Just("18446744073709551616".to_owned()),
        any::<char>().prop_map(String::from),
    ];
    // Names the gate takes, each a face of a feature before the sketch or not, half the
    // time: so the rebuild's search is reached, not only the gate's refusal.
    let whole = prop::sample::select(vec![
        "b0000001.box.Face1",
        "b0000001.box.Face6",
        "b0000001.box.Face7",
        "b0000001.box.Face4294967295",
        "p0000001.pad.Face(G s0000009.sketch.Face1)",
        "p0000001.pad.Face(G s0000009.sketch.Face2)",
        "p0000001.pad.Face(M s0000009.sketch.Edge1)",
        "b0000001.box.Face5",
    ])
    .prop_map(str::to_owned);
    prop_oneof![
        whole,
        prop::collection::vec(piece, 1..12).prop_map(|pieces| pieces.concat()),
    ]
}

fn token() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec!["+x", "-x", "+y", "-y", "+z", "-z"])
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1500,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0xface_0001),
        ..ProptestConfig::default()
    })]

    /// Any face plane reads back as written and writes back the same bytes; the gate
    /// refuses it on plane.face or takes it, never panicking; and a document it takes
    /// rebuilds on the fake, the sketch's outcome a reference refusal or blocked, never a
    /// bug or a hang, and the same on a second engine.
    #[test]
    fn any_face_plane_round_trips_gates_and_rebuilds_without_a_bug(
        name in face_text(),
        x in token(),
        reversed in any::<bool>(),
    ) {
        let text = document(&[
            &cube(),
            &rectangle("s0000009", "xy", [0.0, 0.0], 2.0, 2.0),
            &pad("p0000001", Some("b0000001"), "s0000009", &one_side("1")),
            &on_face("s0000001", &face(&name, x, reversed)),
            &pad("p0000002", Some("p0000001"), "s0000001", &one_side("1")),
        ]);
        let document = read(text.as_bytes()).unwrap();
        let written = write(&document).unwrap();
        let again = read(written.as_bytes()).unwrap();
        prop_assert_eq!(&again, &document);
        prop_assert_eq!(write(&again).unwrap(), written);
        match silico_core::document::validate(&document) {
            Err(invalid) => {
                for e in invalid.errors() {
                    if e.feature() == Some("s0000001") {
                        prop_assert_eq!(e.parameter(), Some("plane.face"), "{:?}", e);
                    }
                    prop_assert!(e.message().len() < 8192);
                }
            }
            Ok(()) => {
                let checked = Checked::new(document).unwrap();
                let model = publish(&engine(), checked.clone());
                let outcome = model.outcome(&id("s0000001"));
                match outcome {
                    Some(FeatureOutcome::Broken { errors, .. }) => {
                        prop_assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
                    }
                    Some(FeatureOutcome::Blocked { .. }) => {}
                    other => prop_assert!(false, "{name:?}: {other:?}"),
                }
                prop_assert!(model.fault().is_none(), "{:?}", model.fault());
                let other = publish(&engine(), checked);
                prop_assert_eq!(
                    format!("{:?}", other.outcome(&id("s0000001"))),
                    format!("{:?}", model.outcome(&id("s0000001")))
                );
            }
        }
    }
}
