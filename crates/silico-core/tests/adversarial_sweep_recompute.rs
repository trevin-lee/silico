//! Break-it on M3.6's steps iv and v on the fake kernel (docs/design/m3.6-sketch-features.md,
//! S3 to S6, S12): recompute's planning of pads, pockets, revolves, and grooves, its checks of
//! their results, and the names of their elements, fed what a confused agent or a careless
//! edit writes: identical features sharing steps, bases of several solids, arcs across the
//! angle's branch cut, sketches broken and then mended, edits that supersede each other, and
//! sketch names where they don't belong.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/sweeps.rs"]
mod sweeps;

use std::sync::Arc;
use std::time::Duration;

use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{Error, ErrorCode};
use sweeps::*;

fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new())
}

fn published(engine: &Recompute<FakeKernel>, features: &[&str]) -> Arc<Model> {
    let document = checked_document(features);
    match engine.start(document).wait_timeout(Duration::from_secs(60)) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        Ok(other) => panic!("not published: {other:?}"),
        Err(_) => panic!("the recompute didn't finish"),
    }
}

fn built(model: &Model, feature: &str) -> bool {
    matches!(
        model.outcome(&id(feature)),
        Some(FeatureOutcome::Built { .. })
    )
}

fn failed(model: &Model, feature: &str) -> Error {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Failed(error)) => error.clone(),
        other => panic!("{feature}: {other:?}"),
    }
}

/// The rectangle 4 by 2 from the origin on `plane`, as sketch `id`.
fn sketch_as(id: &str, plane: &str) -> String {
    rectangle(id, plane, [0.0, 0.0], 4.0, 2.0)
}

/// Every name of `feature`'s elements, as text, sorted.
fn all_names(model: &Model, feature: &str) -> Vec<String> {
    let map = model.element_map(&id(feature)).unwrap();
    let mut names: Vec<String> = map
        .iter()
        .flat_map(|(_, names)| names.iter().map(|n| n.to_string()))
        .collect();
    names.sort();
    names
}

#[test]
fn two_identical_pockets_on_identical_boxes_each_refused_in_their_own_words() {
    // Two boxes alike in every value make the same steps, so two pockets alike, each of its
    // own box, share their faces, prism, and cut (identical steps run once). Both miss
    // their boxes (the cube above the sketch, the pocket going down) and remove nothing,
    // so both are refused, S7: each refusal must speak of its own feature and its own base.
    // An agent fixing the second is otherwise told to fix the first.
    let above = |id: &str| box_at(id, [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0);
    let engine = engine();
    let model = published(
        &engine,
        &[
            &above("b0000001"),
            &above("b0000002"),
            &sketch_as("ssssssss", "xy"),
            &pocket("q0000001", "b0000001", "ssssssss", TEN),
            &pocket("q0000002", "b0000002", "ssssssss", TEN),
        ],
    );
    let first = failed(&model, "q0000001");
    assert!(
        first
            .message()
            .starts_with("pocket q0000001 removes nothing from its base b0000001"),
        "{}",
        first.message()
    );
    let second = failed(&model, "q0000002");
    assert_eq!(second.code(), ErrorCode::KernelFailed);
    assert_eq!(second.feature(), Some("q0000002"));
    // Found by break-it, fixed: the cut step is shared, and its check was the first
    // feature's, so the second's refusal read "pocket q0000001 removes nothing from its base
    // b0000001" under feature q0000002. Now the step keeps what it measured with its
    // refusal, and each feature words its own.
    assert!(
        second
            .message()
            .starts_with("pocket q0000002 removes nothing from its base b0000002"),
        "q0000002 is told: {}",
        second.message()
    );
}

#[test]
fn two_identical_pads_on_identical_boxes_both_build_and_name_their_own_elements() {
    // The same sharing where the check passes: each pad builds, and each map names its own
    // feature's elements (naming keys hold the ID), none the other's.
    let below = |id: &str| box_at(id, [-1.0, -1.0, -10.0], 10.0, 10.0, 10.0);
    let engine = engine();
    let model = published(
        &engine,
        &[
            &below("b0000001"),
            &below("b0000002"),
            &sketch_as("ssssssss", "xy"),
            &pad("p0000001", Some("b0000001"), "ssssssss", TEN),
            &pad("p0000002", Some("b0000002"), "ssssssss", TEN),
        ],
    );
    assert!(built(&model, "p0000001") && built(&model, "p0000002"));
    let first = all_names(&model, "p0000001");
    let second = all_names(&model, "p0000002");
    assert!(first.iter().any(|n| n.starts_with("p0000001.pad.")));
    assert!(
        !first
            .iter()
            .any(|n| n.contains("p0000002") || n.contains("b0000002"))
    );
    assert!(
        !second
            .iter()
            .any(|n| n.contains("p0000001") || n.contains("b0000001"))
    );
    assert_eq!(
        first.len(),
        second.len(),
        "the same solid, named alike but for its IDs"
    );
}

#[test]
fn a_pocket_into_a_fuse_of_two_boxes_apart_isnt_said_to_cut_it_in_two() {
    // M2's fuse of two boxes apart was a compound of two solids, accepted, and a pocket of a
    // small hole in the first box was refused as cutting its base "in two". Found by
    // break-it, fixed by Fable's decision (design S5): the fuse is refused at its own
    // feature (D2's letter for M2's booleans; on the real kernel, silico-kernel
    // tests/boolean_solids.rs), so no base of several solids exists. The fake's fuse is one
    // solid, so here the pocket builds, and is never blamed for its base.
    let engine = engine();
    let model = published(
        &engine,
        &[
            &box_at("b0000001", [-1.0, -1.0, -10.0], 10.0, 10.0, 10.0),
            &box_at("b0000002", [100.0, 0.0, -10.0], 10.0, 10.0, 10.0),
            r#"{"id": "f0000001", "type": "fuse", "v": 1, "name": "Two", "base": "b0000001", "tool": "b0000002"}"#,
            &rectangle("ssssssss", "xy", [1.0, 1.0], 2.0, 2.0),
            &pocket("qqqqqqqq", "f0000001", "ssssssss", &one_side("3")),
        ],
    );
    assert!(built(&model, "f0000001"));
    assert!(
        built(&model, "qqqqqqqq"),
        "{:?}",
        model.outcome(&id("qqqqqqqq"))
    );
}

#[test]
fn an_arc_bulging_across_the_axis_where_its_angles_pass_pi_is_refused() {
    // Design S6's closed form for an arc, at atan2's branch cut: the arc about (0.8, 0) of
    // radius 1, counter-clockwise from (0.2, 0.8) to (0.2, −0.8) through (−0.2, 0) (its
    // angles 2.214 and 4.069 radians, past π), closed by the chord x = 0.2. Its ends are
    // right of the y axis and its bulge 0.2 left of it: the axis crosses the profile,
    // refused before any kernel step. 3-4-5 triangles put every point exactly on the circle.
    let sketch = r#"{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{"id": "pa000001", "x": 0.2, "y": 0.8, "construction": false}, {"id": "pa000002", "x": 0.2, "y": -0.8, "construction": false}, {"id": "pc000001", "x": 0.8, "y": 0, "construction": false}], "lines": [{"id": "na000001", "from": "pa000002", "to": "pa000001", "construction": false}], "circles": [], "arcs": [{"id": "aa000001", "center": "pc000001", "start": "pa000001", "end": "pa000002", "construction": false}], "constraints": []}"#;
    let engine = engine();
    let model = published(
        &engine,
        &[
            sketch,
            &turned("rrrrrrrr", "revolve", None, "ssssssss", "y_axis", TURN),
        ],
    );
    let error = failed(&model, "rrrrrrrr");
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert!(
        error.message().contains("crosses its profile"),
        "{}",
        error.message()
    );
    assert!(error.elements().contains(&"aa000001".to_owned()));
    // The same segment moved 0.4 right (its bulge at x = 0.2) is on one side: it turns.
    let right = sketch
        .replace(r#""x": 0.2,"#, r#""x": 0.6,"#)
        .replace(r#""x": 0.8,"#, r#""x": 1.2,"#);
    let model = published(
        &self::engine(),
        &[
            &right,
            &turned("rrrrrrrr", "revolve", None, "ssssssss", "y_axis", TURN),
        ],
    );
    assert!(
        built(&model, "rrrrrrrr"),
        "{:?}",
        model.outcome(&id("rrrrrrrr"))
    );
}

#[test]
fn a_sketch_that_failed_to_solve_and_is_mended_builds_its_pad_on_the_same_engine() {
    // Corruption: a failure kept by the engine must not outlive its cause. The first
    // recompute's sketch doesn't solve (the fake says so for any sketch with a point at
    // x = 7), blocking the pad; the mended sketch solves, and the pad builds.
    let sketches = FakeSketches::new();
    sketches.answer(|system| {
        let solved = FakeSketches::as_sent(system);
        if solved.values.contains(&7.0) {
            Err(Error::new(
                ErrorCode::SolveFailed,
                "the fake won't solve x = 7",
            ))
        } else {
            Ok(solved)
        }
    });
    let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(sketches);
    let broken = rectangle("ssssssss", "xy", [0.0, 0.0], 7.0, 2.0);
    let model = published(&engine, &[&broken, &pad("pppppppp", None, "ssssssss", TEN)]);
    assert_eq!(
        model.outcome(&id("pppppppp")),
        Some(&FeatureOutcome::Blocked { on: id("ssssssss") })
    );
    let mended = rectangle("ssssssss", "xy", [0.0, 0.0], 4.0, 2.0);
    let model = published(&engine, &[&mended, &pad("pppppppp", None, "ssssssss", TEN)]);
    assert!(
        built(&model, "pppppppp"),
        "{:?}",
        model.outcome(&id("pppppppp"))
    );
    // And broken again, it's blocked again: the good solve isn't taken for the bad sketch.
    let model = published(&engine, &[&broken, &pad("pppppppp", None, "ssssssss", TEN)]);
    assert_eq!(
        model.outcome(&id("pppppppp")),
        Some(&FeatureOutcome::Blocked { on: id("ssssssss") })
    );
}

#[test]
fn a_sketch_with_no_profile_then_mended_then_emptied_again_follows_each_edit() {
    let engine = engine();
    let empty = empty_sketch("ssssssss", "S", "xy");
    let full = sketch_as("ssssssss", "xy");
    for (sketch, builds) in [
        (&empty, false),
        (&full, true),
        (&empty, false),
        (&full, true),
    ] {
        let model = published(&engine, &[sketch, &pad("pppppppp", None, "ssssssss", TEN)]);
        match model.outcome(&id("pppppppp")) {
            Some(FeatureOutcome::Built { .. }) => assert!(builds),
            Some(FeatureOutcome::Broken { errors, total }) => {
                assert!(!builds);
                assert!(*total >= 1 && !errors.is_empty());
                assert!(errors.iter().all(|e| e.feature() == Some("ssssssss")));
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn a_storm_of_edits_to_a_swept_sketch_ends_where_a_fresh_engine_does() {
    // Supersession through `solve_swept`: edits started one after another without waiting
    // (each pad longer, each sketch wider), only the last waited for. Its model must be
    // the one a fresh engine makes of the last document: the same outcomes and the same
    // names, nothing of an earlier sketch's solve or profile left in it.
    let document = |k: u32| {
        let sketch = rectangle("ssssssss", "xy", [0.0, 0.0], 1.0 + f64::from(k), 2.0);
        let pad = pad(
            "pppppppp",
            None,
            "ssssssss",
            &one_side(&format!("{}", k + 1)),
        );
        let fillet = r#"{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "pppppppp", "edges": ["pppppppp.pad.Edge(G ssssssss.sketch.Edge.na000003)"], "radius": 0.1}"#;
        checked_document(&[&sketch, &pad, fillet])
    };
    let engine = engine();
    let mut last = None;
    for k in 0..30 {
        last = Some(engine.start(document(k)));
    }
    let last = match last.unwrap().wait_timeout(Duration::from_secs(60)) {
        Ok(Ok(RunOutcome::Published(model))) => model,
        other => panic!("{:?}", other.map(|r| r.map(|_| ())).ok()),
    };
    let fresh = match self::engine().start(document(29)).wait().unwrap() {
        RunOutcome::Published(model) => model,
        other => panic!("{other:?}"),
    };
    assert_eq!(last.features().len(), fresh.features().len());
    for ((a_id, a), (b_id, b)) in last.features().iter().zip(fresh.features()) {
        assert_eq!(a_id, b_id);
        match (a, b) {
            (FeatureOutcome::Built { built: x, .. }, FeatureOutcome::Built { built: y, .. }) => {
                assert_eq!(x.shape, y.shape, "{a_id}");
            }
            (FeatureOutcome::Sketch(_), FeatureOutcome::Sketch(_)) => {}
            _ => panic!("{a_id}: {a:?} against {b:?}"),
        }
    }
    for feature in ["pppppppp", "ffffffff"] {
        assert_eq!(all_names(&last, feature), all_names(&fresh, feature));
    }
}

#[test]
fn a_revolve_onto_a_pad_of_the_same_sketch_names_every_element_once() {
    // A sketch is used, not consumed (S2): the pad and the revolve onto it share one faces
    // step and one faces map. The revolve turns the 4 by 2 rectangle a quarter turn about
    // its own left side (na000004, the line x = 0): every element of the result has names
    // no other element has, the pad's elements say pad and the revolve's revolve, and a
    // fillet can take an edge of each by name.
    let engine = engine();
    let model = published(
        &engine,
        &[
            &sketch_as("ssssssss", "xy"),
            &pad("pppppppp", None, "ssssssss", TEN),
            &turned(
                "rrrrrrrr",
                "revolve",
                Some("pppppppp"),
                "ssssssss",
                "na000004",
                &angle("90"),
            ),
        ],
    );
    assert!(
        built(&model, "rrrrrrrr"),
        "{:?}",
        model.outcome(&id("rrrrrrrr"))
    );
    let map = model.element_map(&id("rrrrrrrr")).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for (element, names) in map.iter() {
        for name in names {
            assert!(
                seen.insert(name.to_string()),
                "{name} names two elements, one of them {element:?}"
            );
        }
    }
    let names = all_names(&model, "rrrrrrrr");
    assert!(names.iter().any(|n| n.starts_with("pppppppp.pad.")));
    assert!(names.iter().any(|n| n.starts_with("rrrrrrrr.revolve.")));
    assert!(!names.iter().any(|n| n.starts_with("ssssssss.")));
}

#[test]
fn a_boxs_element_written_in_a_sketchs_form_is_never_resolved() {
    // `b.sketch.Edge.<id>` for a box: the grammar reads a sketch's ID form for any feature
    // (the type word is the name's own, N8), and the whole-name refusal looks at what the
    // feature is. A fillet naming it must be refused by the gate or break at the rebuild,
    // never round some edge, never panic.
    let fillet = r#"{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "bbbbbbbb", "edges": ["bbbbbbbb.sketch.Edge.na000001"], "radius": 0.1}"#;
    let features = [
        box_at("bbbbbbbb", [0.0; 3], 10.0, 10.0, 10.0),
        fillet.to_owned(),
    ];
    let refs: Vec<&str> = features.iter().map(String::as_str).collect();
    if checked(&refs).is_err() {
        return;
    }
    let engine = engine();
    let model = published(&engine, &refs);
    match model.outcome(&id("ffffffff")) {
        Some(FeatureOutcome::Broken { errors, .. }) => {
            assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken, "{errors:?}");
        }
        other => panic!("a box's edge in a sketch's form came to {other:?}"),
    }
}

#[test]
fn hostile_sketch_forms_inside_a_pads_name_are_refused_by_the_gate_naming_the_column() {
    // Forms S4's grammar excludes for a sketch, each in a fillet's edge on a pad: a region
    // by any number but 1, a curve by number, a face by ID, a derived source, an ID one
    // character short or long, and an ID in another case.
    let cases = [
        "pppppppp.pad.Face(G ssssssss.sketch.Face2)",
        "pppppppp.pad.Face(G ssssssss.sketch.Face0)",
        "pppppppp.pad.Edge(G ssssssss.sketch.Edge1)",
        "pppppppp.pad.Edge(G ssssssss.sketch.Face.na000001)",
        "pppppppp.pad.Edge(G ssssssss.sketch.Edge(M ssssssss.sketch.Edge.na000001))",
        "pppppppp.pad.Edge(G ssssssss.sketch.Edge(G ssssssss.sketch.Vertex.pa000001))",
        "pppppppp.pad.Edge(G ssssssss.sketch.Edge.na00000)",
        "pppppppp.pad.Edge(G ssssssss.sketch.Edge.na0000011)",
        "pppppppp.pad.Edge(G ssssssss.sketch.Edge.NA000001)",
        "pppppppp.pad.Edge(G ssssssss.sketch.Edge.)",
        "pppppppp.pad.Edge(G ssssssss.sketch.Edge.na000001 )",
        "pppppppp.pad.Edge.na000001",
        "pppppppp.pad.Edge(G ssssssss.sketch.Solid1)",
    ];
    for edge in cases {
        let fillet = format!(
            r#"{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "pppppppp", "edges": ["{edge}"], "radius": 0.1}}"#
        );
        let error = problem(&[
            &sketch_as("ssssssss", "xy"),
            &pad("pppppppp", None, "ssssssss", TEN),
            &fillet,
        ]);
        assert_eq!(error.code(), ErrorCode::InvalidInput, "{edge}");
        assert_eq!(error.feature(), Some("ffffffff"), "{edge}");
        assert!(
            error.message().contains("column"),
            "{edge}: {}",
            error.message()
        );
    }
}

#[test]
fn a_sketchs_edge_given_whole_is_refused_with_advice_the_gate_takes_and_that_resolves() {
    // S12: a sketch's element named whole is refused, with what to write where the fillet's
    // base sweeps that sketch: `p.pad.Edge(M s.sketch.Edge.c)`. Following the advice must
    // reach a fillet that builds (catalog: examples an error gives, fed back).
    let sketch = sketch_as("ssssssss", "xy");
    let pad = pad("pppppppp", None, "ssssssss", TEN);
    let fillet = |edge: &str| {
        format!(
            r#"{{"id": "ffffffff", "type": "fillet", "v": 1, "name": "F", "base": "pppppppp", "edges": ["{edge}"], "radius": 0.1}}"#
        )
    };
    let error = problem(&[&sketch, &pad, &fillet("ssssssss.sketch.Edge.na000001")]);
    let message = error.message();
    let advised = message.split('(').skip(1).collect::<Vec<_>>().join("(");
    let advised = advised
        .strip_suffix(&format!(
            "), at column {}",
            message.rsplit(' ').next().unwrap_or_default()
        ))
        .map(str::to_owned);
    let Some(advised) = advised else {
        panic!("no advice to follow in: {message}");
    };
    assert_eq!(
        advised,
        "pppppppp.pad.Edge(M ssssssss.sketch.Edge.na000001)"
    );
    let engine = engine();
    let model = published(&engine, &[&sketch, &pad, &fillet(&advised)]);
    assert!(
        built(&model, "ffffffff"),
        "{:?}",
        model.outcome(&id("ffffffff"))
    );
}

#[test]
fn a_sweep_of_a_sketch_after_its_user_in_the_timeline_is_refused_by_the_gate() {
    // A pad naming a sketch that comes after it, and one naming a fillet as its sketch.
    let after = problem(&[
        &pad("pppppppp", None, "ssssssss", TEN),
        &sketch_as("ssssssss", "xy"),
    ]);
    assert_eq!(after.feature(), Some("pppppppp"));
    assert_eq!(after.parameter(), Some("sketch"));
}

#[test]
fn a_pad_kept_from_a_failed_check_isnt_built_by_the_next_document_that_needs_it_to_pass() {
    // A refusal of a result's check is kept with the step's key (S12): the same fuse asked
    // again is refused again with no kernel call. But a pocket that changes nothing and
    // then its base moved so it does must build: the cut's key holds the base's.
    let engine = engine();
    let base = |z: f64| box_at("bbbbbbbb", [-1.0, -1.0, z], 10.0, 10.0, 10.0);
    let features = |z: f64| {
        vec![
            base(z),
            sketch_as("ssssssss", "xy"),
            pocket("qqqqqqqq", "bbbbbbbb", "ssssssss", TEN),
        ]
    };
    fn as_refs(v: &[String]) -> Vec<&str> {
        v.iter().map(String::as_str).collect()
    }
    let above = features(1.0);
    let model = published(&engine, &as_refs(&above));
    assert_eq!(failed(&model, "qqqqqqqq").code(), ErrorCode::KernelFailed);
    let below = features(-10.0);
    let model = published(&engine, &as_refs(&below));
    assert!(
        built(&model, "qqqqqqqq"),
        "{:?}",
        model.outcome(&id("qqqqqqqq"))
    );
    let model = published(&engine, &as_refs(&above));
    assert_eq!(failed(&model, "qqqqqqqq").code(), ErrorCode::KernelFailed);
}

#[test]
fn a_refused_pocket_re_added_under_a_new_id_is_refused_in_its_own_words() {
    // The same sharing across edits: an agent deletes a pocket that removes nothing and
    // adds it again under a new ID (and its base under a new ID too). The kept refusal of
    // the cut (S12: kept with the step's key, which holds no ID) must not speak of the
    // deleted feature.
    let above = |id: &str| box_at(id, [-1.0, -1.0, 1.0], 10.0, 10.0, 10.0);
    let engine = engine();
    let model = published(
        &engine,
        &[
            &above("b0000001"),
            &sketch_as("ssssssss", "xy"),
            &pocket("q0000001", "b0000001", "ssssssss", TEN),
        ],
    );
    assert_eq!(failed(&model, "q0000001").code(), ErrorCode::KernelFailed);
    let model = published(
        &engine,
        &[
            &above("b0000009"),
            &sketch_as("ssssssss", "xy"),
            &pocket("q0000009", "b0000009", "ssssssss", TEN),
        ],
    );
    let error = failed(&model, "q0000009");
    assert_eq!(error.feature(), Some("q0000009"));
    // Found by break-it, fixed: the message was the deleted feature's, "pocket q0000001
    // removes nothing from its base b0000001"; the kept refusal is now worded per feature
    // from what its step measured.
    assert!(
        error
            .message()
            .starts_with("pocket q0000009 removes nothing from its base b0000009"),
        "{}",
        error.message()
    );
}

mod axis_reach {
    //! S6's closed form for an arc's reach either side of the axis, against the arc sampled
    //! densely: an arc about `c` of radius `r` from angle `a` counter-clockwise by `s`,
    //! closed by its chord, and an axis line at angle `θ` placed, half the time, tangent to
    //! the arc's circle give or take a hair, so its bulge crosses by less than the tolerance
    //! or just more. Solutions are built here, in doubles, so every point is on its circle.
    use super::*;
    use proptest::prelude::*;
    use silico_core::document::{Coordinate, ElementId};
    use silico_core::sketch::SketchSolution;
    use std::collections::{BTreeMap, BTreeSet};
    use std::f64::consts::{PI, TAU};

    const TOL: f64 = 1e-7;

    fn sq(v: f64) -> f64 {
        v * v
    }

    fn case(
        c: [f64; 2],
        r: f64,
        a: f64,
        s: f64,
        theta: f64,
        offset: f64,
    ) -> (
        silico_core::document::Checked,
        SketchSolution,
        [f64; 2],
        [f64; 2],
    ) {
        let start = [c[0] + r * libm::cos(a), c[1] + r * libm::sin(a)];
        let end = [c[0] + r * libm::cos(a + s), c[1] + r * libm::sin(a + s)];
        // The axis: through the point `offset` from c along the axis's left normal.
        let d = [libm::cos(theta), libm::sin(theta)];
        let n = [-d[1], d[0]];
        let o = [c[0] + offset * n[0], c[1] + offset * n[1]];
        let p = [o[0] - 20.0 * d[0], o[1] - 20.0 * d[1]];
        let q = [o[0] + 20.0 * d[0], o[1] + 20.0 * d[1]];
        let stored = |v: f64| format!("{:.6}", v);
        let points = [
            ("pa000001", start),
            ("pa000002", end),
            ("pc000001", c),
            ("pb000001", p),
            ("pb000002", q),
        ];
        let point_text: Vec<String> = points
            .iter()
            .map(|(id, [x, y])| {
                format!(
                    r#"{{"id": "{id}", "x": {}, "y": {}, "construction": false}}"#,
                    stored(*x),
                    stored(*y)
                )
            })
            .collect();
        let sketch = format!(
            r#"{{"id": "ssssssss", "type": "sketch", "v": 1, "name": "S", "plane": "xy", "points": [{}], "lines": [{{"id": "na000001", "from": "pa000002", "to": "pa000001", "construction": false}}, {{"id": "nb000001", "from": "pb000001", "to": "pb000002", "construction": true}}], "circles": [], "arcs": [{{"id": "aa000001", "center": "pc000001", "start": "pa000001", "end": "pa000002", "construction": false}}], "constraints": []}}"#,
            point_text.join(", ")
        );
        let document = checked_document(&[
            &sketch,
            &turned(
                "rrrrrrrr",
                "revolve",
                None,
                "ssssssss",
                "nb000001",
                &angle("90"),
            ),
        ]);
        let mut solved = BTreeMap::new();
        for (id, [x, y]) in points {
            solved.insert(
                ElementId::new(id).unwrap(),
                [Coordinate::new(x).unwrap(), Coordinate::new(y).unwrap()],
            );
        }
        let solution = SketchSolution {
            points: solved,
            radii: BTreeMap::new(),
            references: BTreeMap::new(),
            dofs: 0,
            free: BTreeSet::new(),
            redundant: BTreeMap::new(),
            algorithm: silico_protocol::Algorithm::DogLeg,
        };
        (document, solution, p, q)
    }

    fn offsets() -> impl Strategy<Value = f64> {
        let hair = prop_oneof![
            Just(0.0),
            Just(1e-8),
            Just(-1e-8),
            Just(5e-8),
            Just(-5e-8),
            Just(2e-7),
            Just(-2e-7),
            Just(1e-6),
            Just(-1e-6),
            -1e-3..1e-3f64,
        ];
        prop_oneof![
            -6.0..6.0f64,
            hair.clone().prop_map(|h| 1.0 + h),
            hair.prop_map(|h| -1.0 + h),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2000))]

        #[test]
        fn an_arcs_crossing_is_refused_exactly_when_sampling_finds_one(
            cx in -5.0..5.0f64,
            cy in -5.0..5.0f64,
            r in 0.5..3.0f64,
            a in -PI..PI,
            s in 0.1..(TAU - 0.1),
            theta in prop_oneof![Just(0.0), Just(PI / 2.0), Just(PI), Just(-PI / 2.0), -PI..PI],
            unit_offset in offsets(),
        ) {
            // The offset scaled by r: a unit offset of ±1 puts the axis tangent to the circle.
            let offset = unit_offset * r;
            let (document, solution, p, q) = case([cx, cy], r, a, s, theta, offset);
            let profile = silico_core::profile::profile(&document, &id("ssssssss"), &solution);
            prop_assert!(profile.is_ok(), "{:?}", profile.err());
            let profile = profile.unwrap();
            // Ground truth: the arc sampled (its chord's ends are the arc's), distances from
            // the axis as the line from p to q has it.
            let len = (sq(q[0] - p[0]) + sq(q[1] - p[1])).sqrt();
            let d = [(q[0] - p[0]) / len, (q[1] - p[1]) / len];
            let side = |x: f64, y: f64| d[0] * (y - p[1]) - d[1] * (x - p[0]);
            let n = 200_000;
            let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
            for k in 0..=n {
                let t = a + s * (k as f64) / (n as f64);
                let v = side(cx + r * libm::cos(t), cy + r * libm::sin(t));
                low = low.min(v);
                high = high.max(v);
            }
            // Sampling misses an extreme by at most r(1 − cos(s/2n)) < 3e-10 here; skip what
            // lies within 1e-8 of either boundary, where rounding decides.
            let near = |v: f64| (v.abs() - TOL).abs() < 1e-8;
            prop_assume!(!near(low) && !near(high));
            let crossing = low < -TOL && high > TOL;
            let got = silico_core::sweep::check_axis_turn(&document, &id("rrrrrrrr"), &solution, &profile);
            match got {
                Err(error) if error.message().contains("crosses its profile") => {
                    prop_assert!(crossing, "refused, but sampled {low} .. {high}: {}", error.message());
                }
                Err(error) => prop_assert!(false, "{error:?}"),
                Ok(()) => prop_assert!(!crossing, "passed, but sampled {low} .. {high}"),
            }
        }
    }
}

#[test]
fn a_solve_that_panics_fails_its_sketch_alike_whether_or_not_a_feature_sweeps_it() {
    // A bug in the solver (a panic) is a bug in Silico, as under a step. A sketch nothing
    // sweeps is solved beside the steps; one a pad sweeps is solved before the plan (S3).
    // Either way the box beside it is the document's too: what the recompute comes to must
    // not depend on whether a pad sweeps the sketch, and the engine must go on to build the
    // next document once the solver answers.
    use std::sync::atomic::{AtomicBool, Ordering};
    let outcome = |swept: bool| {
        let sketches = FakeSketches::new();
        let panicked = Arc::new(AtomicBool::new(false));
        let once = Arc::clone(&panicked);
        sketches.answer(move |system| {
            if !once.swap(true, Ordering::SeqCst) {
                panic!("the fake solver's bug");
            }
            Ok(FakeSketches::as_sent(system))
        });
        let engine = Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(sketches);
        let mut features = vec![
            box_at("bbbbbbbb", [0.0; 3], 1.0, 1.0, 1.0),
            sketch_as("ssssssss", "xy"),
        ];
        if swept {
            features.push(pad("pppppppp", None, "ssssssss", TEN));
        }
        let refs: Vec<&str> = features.iter().map(String::as_str).collect();
        let first = engine
            .start(checked_document(&refs))
            .wait_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("swept {swept}: the recompute hung"));
        let first = match first {
            Ok(RunOutcome::Published(model)) => Ok((
                built(&model, "bbbbbbbb"),
                model.outcome(&id("ssssssss")).cloned(),
            )),
            Ok(other) => panic!("{other:?}"),
            Err(error) => Err(error.code()),
        };
        let second = published(&engine, &refs);
        assert!(built(&second, "bbbbbbbb"), "swept {swept}");
        if swept {
            assert!(
                built(&second, "pppppppp"),
                "{:?}",
                second.outcome(&id("pppppppp"))
            );
        }
        first
    };
    let beside = outcome(false);
    let before = outcome(true);
    // Both fail the whole recompute as a bug (`Internal`), as M3.4's path did before M3.6;
    // what's held is that the two paths agree and the engine goes on.
    let code = |o: &Result<(bool, Option<FeatureOutcome>), ErrorCode>| match o {
        Ok((built, outcome)) => format!("{built} {:?}", outcome.as_ref().map(|_| ())),
        Err(code) => format!("{code:?}"),
    };
    assert_eq!(code(&beside), code(&before));
}
