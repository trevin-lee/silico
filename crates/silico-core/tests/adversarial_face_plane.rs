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

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use silico_core::ErrorCode;
use silico_core::cache::Cache;
use silico_core::command::{Call, Sessions};
use silico_core::document::{Checked, Operation, read, write};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::sketch::SketchSolver;

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
fn a_face_named_of_an_earlier_sketch_is_refused_by_the_gate_or_broken_with_candidates() {
    // A sketch's faces (s0000001.sketch.Face1) aren't a solid's: a sketch on one is either
    // the gate's refusal or the rebuild's reference_broken, never a frame or a bug.
    let features = [
        rectangle("s0000001", "xy", [0.0, 0.0], 4.0, 2.0),
        pad("p0000001", None, "s0000001", &one_side("3")),
        on_face("s0000002", &face("s0000001.sketch.Face1", "+x", false)),
    ];
    match checked(&strs(&features)) {
        Err(invalid) => {
            assert_eq!(invalid.errors()[0].parameter(), Some("plane.face"));
        }
        Ok(()) => {
            let model = publish(&engine(), checked_document(&strs(&features)));
            let outcome = model.outcome(&id("s0000002"));
            let Some(FeatureOutcome::Broken { errors, .. }) = outcome else {
                panic!("{outcome:?}");
            };
            assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken, "{errors:?}");
            assert_eq!(errors[0].parameter(), Some("plane.face"));
        }
    }
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

/// A chain of `n` rebuilt on the fake: the time it took. Each sketch but the first is
/// blocked on the first, whose face the fake doesn't type a plane.
fn rebuild_chain(n: usize) -> Duration {
    let features = chain(n);
    let document = checked_document(&strs(&features));
    let engine = engine();
    let started = Instant::now();
    let model = publish(&engine, document);
    let took = started.elapsed();
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
    took
}

#[test]
fn a_chain_of_sixty_sketches_each_on_the_last_pads_face_rebuilds_in_sixty_rounds_promptly() {
    // Sixty sketches on faces, each on the one before's pad: a part modelled the way
    // FreeCAD users model, every feature sketched on the last. Each round places one, so
    // sixty rounds; each round's plan, model, and face search must stay cheap.
    let took = rebuild_chain(60);
    assert!(took < Duration::from_secs(20), "{took:?}");
}

#[test]
fn a_chain_of_sketches_on_faces_grows_no_faster_than_the_square_of_its_length() {
    // The rounds are one per link; each looks at every sketch still waiting, and for each
    // walks the features before it: so a chain of n costs about n³ if nothing is shared.
    // Doubling the chain from 100 to 200 should cost about four times as much (n²), not
    // eight (n³).
    //
    // BREAKS: each round runs `placed_faces` over every sketch still waiting, and each of
    // those walks every feature before it (its standing, its makers' descendants, and the
    // solids consumed, a map of them built again for each sketch), though only the first
    // can be placed: n rounds of n sketches of n features, n³. Measured in a debug build:
    // 80 links 0.64 s, 160 4.5 s, 320 36 s (ratio 7 to 8 per doubling); a part of 300
    // sketches each on the last pad's face spends half a minute planning every rebuild,
    // every step cached.
    let small = rebuild_chain(100);
    let large = rebuild_chain(200);
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-3);
    assert!(
        ratio < 5.5,
        "doubling the chain cost {ratio:.1} times as much: {small:?} then {large:?}"
    );
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
    let started = Instant::now();
    let model = publish(&engine(), document);
    let took = started.elapsed();
    assert!(took < Duration::from_secs(20), "{took:?}");
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
fn the_sketch_read_of_a_face_sketch_blocked_upstream_is_refused_naming_the_root_not_a_bug() {
    // s2's face could only be in p1, which is blocked on s1 (not planar on the fake): the
    // read of s2 is refused as check reports s2, blocked on s1, never `internal`.
    let dir = tempfile::tempdir().unwrap();
    let sessions = sessions_on(&dir, &chain(2));
    for (sketch, code) in [("s0000001", "reference_broken"), ("s0000002", "blocked")] {
        let read = call(
            &sessions,
            "sketch",
            &json!({"session": "s1", "model": "current", "sketch": sketch}).to_string(),
        );
        assert_eq!(read["ok"], false, "{read}");
        let error = &read["errors"][0];
        assert_ne!(error["code"], "internal", "{sketch}: {read}");
        if sketch == "s0000001" {
            assert_eq!(error["code"], code, "{read}");
            assert_eq!(error["parameter"], "plane.face", "{read}");
        } else {
            assert!(
                read.to_string().contains("s0000001"),
                "{sketch}'s refusal names its root: {read}"
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
