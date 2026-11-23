//! M6.3 on the fake kernel: linear patterns, polar patterns, and mirrors rebuilt
//! (docs/design/m6.0-patterns.md, sections 2.4 and 3; N10). What the core sends the kernel,
//! each placement by hand from the document's integers; the names each step gives its
//! copies (`C<instance> <name>` of every name of the original's sweep, the original's own
//! unmarked), held to N10's rules; and which edits keep a stored copy's name and which break
//! it (patterns.md §3's table). The geometry is the real kernel's (silico-kernel's
//! `patterns.rs`); the fake places nothing, so only names and requests are read here.

use std::sync::Arc;
use std::time::Duration;

use silico_core::document::{Checked, read};
use silico_core::fake::{FakeKernel, FakeSketches};
use silico_core::kernel::{ElementKind, Op, Placement};
use silico_core::naming::ElementMap;
use silico_core::recompute::{FeatureOutcome, Model, Recompute, RunOutcome};
use silico_core::{ErrorCode, document::FeatureId};

use super::sweeps::{circles, pad, pocket, rectangle};

fn id(text: &str) -> FeatureId {
    FeatureId::new(text).unwrap()
}

const THROUGH: &str = r#"{"type": "through_all", "reversed": true}"#;
const FIVE: &str = r#"{"type": "one_side", "length": 5, "reversed": false}"#;

/// The plate's first features (section 2.2): a 100 × 60 rectangle from (−50, −30) padded
/// 5, and a hole r 2.75 at (−40, −20) pocketed up through it.
fn plate() -> Vec<String> {
    plate_with_hole_at([-40.0, -20.0])
}

/// The plate, its hole at `at`.
fn plate_with_hole_at(at: [f64; 2]) -> Vec<String> {
    vec![
        rectangle("s0000001", "xy", [-50.0, -30.0], 100.0, 60.0),
        pad("p0000001", None, "s0000001", FIVE),
        circles("s0000003", "xy", &[(at, 2.75)]),
        pocket("q0000002", "p0000001", "s0000003", THROUGH),
    ]
}

/// A linear pattern `t0000001` on `base` copying `copies` (its JSON), with `directions`
/// (their JSON) and `skip`.
fn linear(base: &str, copies: &str, directions: &[String], skip: &str) -> String {
    format!(
        r#"{{"id": "t0000001", "type": "linear_pattern", "v": 1, "name": "Holes", "base": "{base}", "copies": {copies}, "directions": [{}], "skip": {skip}}}"#,
        directions.join(", ")
    )
}

fn along(axis: &str, kind: &str, length: &str, count: u32) -> String {
    format!(
        r#"{{"along": "{axis}", "reversed": false, "distribution": {{"type": "{kind}", "length": {length}}}, "count": {count}}}"#
    )
}

const HOLE: &str = r#"{"type": "features", "features": ["q0000002"]}"#;

fn engine() -> Recompute<FakeKernel> {
    Recompute::new(FakeKernel::new(2).unwrap()).with_sketches(FakeSketches::new())
}

fn checked(features: &[String]) -> Checked {
    let text = format!(
        "{{\"silico\": 1, \"units\": {{\"length\": \"mm\", \"resolution\": \"1nm\"}}, \"naming\": 6, \"features\": [{}]}}",
        features.join(", ")
    );
    Checked::new(read(text.as_bytes()).unwrap()).unwrap()
}

fn published(engine: &Recompute<FakeKernel>, features: &[String]) -> Arc<Model> {
    match engine
        .start(checked(features))
        .wait_timeout(Duration::from_secs(60))
    {
        Ok(Ok(RunOutcome::Published(model))) => model,
        _ => panic!("not published"),
    }
}

fn with(mut features: Vec<String>, more: &[String]) -> Vec<String> {
    features.extend(more.iter().cloned());
    features
}

/// Every copies request the kernel was sent, in order: its op and its placements.
fn copies(kernel: &FakeKernel) -> Vec<(silico_protocol::CopiesOp, Vec<Placement>)> {
    kernel
        .calls()
        .into_iter()
        .filter_map(|call| match call.op {
            Op::Copies { op, placements, .. } => Some((op, placements)),
            _ => None,
        })
        .collect()
}

fn map(model: &Model, feature: &str) -> ElementMap {
    match model.outcome(&id(feature)) {
        Some(FeatureOutcome::Built { .. }) => model.element_map(&id(feature)).unwrap().clone(),
        other => panic!("{feature}: {other:?}"),
    }
}

/// Every name `map` holds, written out, with its element's kind.
fn names(map: &ElementMap) -> Vec<(ElementKind, String)> {
    map.iter()
        .flat_map(|(element, names)| names.iter().map(move |n| (element.kind, n.to_string())))
        .collect()
}

/// The kind word a name writes for an element of `kind`.
fn word(kind: ElementKind) -> &'static str {
    match kind {
        ElementKind::Face => "Face",
        ElementKind::Edge => "Edge",
        ElementKind::Vertex => "Vertex",
        ElementKind::Solid => "Solid",
    }
}

/// N10's copy of `name`, an element of `kind`, at `instance`: by hand from the grammar.
fn copy_of(
    feature: &str,
    type_word: &str,
    kind: ElementKind,
    letter: char,
    instance: &str,
    name: &str,
) -> String {
    format!(
        "{feature}.{type_word}.{}({letter}{instance} {name})",
        word(kind)
    )
}

/// The names of `map` that are `feature`'s own (the sweep's, which a copy of its tool wraps).
fn own(map: &ElementMap, feature: &str) -> Vec<(ElementKind, String)> {
    names(map)
        .into_iter()
        .filter(|(_, name)| name.starts_with(&format!("{feature}.")))
        .collect()
}

#[test]
fn a_grids_copies_are_its_originals_names_wrapped_once_and_the_original_keeps_its_own() {
    let kernel_engine = engine();
    let base = published(&kernel_engine, &plate());
    let pocket = map(&base, "q0000002");
    let grid = with(
        plate(),
        &[linear(
            "q0000002",
            HOLE,
            &[along("x", "extent", "80", 3), along("y", "extent", "40", 2)],
            "[]",
        )],
    );
    let model = published(&kernel_engine, &grid);
    let pattern = map(&model, "t0000001");
    // The original unmarked: every name the pocket's map holds, the pattern's holds of the
    // same kind (N2, N7).
    let held = names(&pattern);
    for name in names(&pocket) {
        assert!(held.contains(&name), "{name:?}");
    }
    // Each copy's: every name of the pocket's own, wrapped once at the copy's instance.
    let sweep = own(&pocket, "q0000002");
    assert!(!sweep.is_empty());
    for instance in ["0,1", "1", "1,1", "2", "2,1"] {
        for (kind, name) in &sweep {
            let copy = copy_of("t0000001", "linear_pattern", *kind, 'C', instance, name);
            assert!(held.contains(&(*kind, copy.clone())), "{copy}");
        }
    }
    // And nothing else: the base's names and five copies of the sweep's.
    assert_eq!(held.len(), names(&pocket).len() + 5 * sweep.len());
    // One request, a cut (a pocket's), each copy moved by hand: 80 over 3 is 40, and 40
    // over 2 is 40, direction 1 first.
    assert_eq!(
        copies(kernel_engine.kernel()),
        [(
            silico_protocol::CopiesOp::Cut,
            [
                [0.0, 40.0, 0.0],
                [40.0, 0.0, 0.0],
                [40.0, 40.0, 0.0],
                [80.0, 0.0, 0.0],
                [80.0, 40.0, 0.0]
            ]
            .map(|offset| Placement::Translation { offset })
            .to_vec()
        )]
    );
}

#[test]
fn a_polar_patterns_copies_turn_about_its_axis_and_a_mirrors_reflect_in_its_plane() {
    let kernel_engine = engine();
    let polar = r#"{"id": "t0000002", "type": "polar_pattern", "v": 1, "name": "Holes", "base": "q0000002", "copies": {"type": "features", "features": ["q0000002"]}, "axis": "z", "reversed": true, "distribution": {"type": "extent", "angle": 180}, "count": 4, "skip": [[2]]}"#;
    let model = published(&kernel_engine, &with(plate(), &[polar.to_owned()]));
    map(&model, "t0000002");
    // 180° over 4 is 60° a step: instances 1 and 3 (2 skipped) at 60° and 180°, about −z.
    let axis = silico_protocol::Axis {
        origin: [0.0; 3],
        direction: [0.0, 0.0, -1.0],
    };
    assert_eq!(
        copies(kernel_engine.kernel()),
        [(
            silico_protocol::CopiesOp::Cut,
            vec![
                Placement::Rotation {
                    axis,
                    angle: silico_protocol::radians(60.0)
                },
                Placement::Rotation {
                    axis,
                    angle: silico_protocol::radians(180.0)
                },
            ]
        )]
    );
    let pattern = map(&model, "t0000002");
    let pocket = map(&model, "q0000002");
    let held = names(&pattern);
    for (kind, name) in own(&pocket, "q0000002") {
        for instance in ["1", "3"] {
            assert!(held.contains(&(
                kind,
                copy_of("t0000002", "polar_pattern", kind, 'C', instance, &name)
            )));
        }
        let skipped = copy_of("t0000002", "polar_pattern", kind, 'C', "2", &name);
        assert!(!held.contains(&(kind, skipped.clone())), "{skipped}");
    }
    // The whole plate mirrored about xz: its one copy `S1`, of every name of the base, fused.
    let kernel_engine = engine();
    let mirror = r#"{"id": "m0000002", "type": "mirror", "v": 1, "name": "Doubled", "base": "q0000002", "copies": {"type": "solid"}, "plane": "xz"}"#;
    let model = published(&kernel_engine, &with(plate(), &[mirror.to_owned()]));
    assert_eq!(
        copies(kernel_engine.kernel()),
        [(
            silico_protocol::CopiesOp::Fuse,
            vec![Placement::Mirror {
                origin: [0.0; 3],
                normal: [0.0, -1.0, 0.0]
            }]
        )]
    );
    let mirrored = names(&map(&model, "m0000002"));
    let base = names(&map(&model, "q0000002"));
    for (kind, name) in &base {
        assert!(mirrored.contains(&(*kind, name.clone())), "{name}");
        let copy = copy_of("m0000002", "mirror", *kind, 'S', "1", name);
        assert!(mirrored.contains(&(*kind, copy.clone())), "{copy}");
    }
    assert_eq!(mirrored.len(), 2 * base.len());
}

#[test]
fn a_count_of_1_or_every_copy_skipped_passes_its_base_through_asking_the_kernel_nothing() {
    for directions in [
        vec![along("x", "spacing", "10", 1)],
        vec![along("x", "spacing", "10", 2)],
    ] {
        let skip = if directions[0].contains("\"count\": 2") {
            "[[1]]"
        } else {
            "[]"
        };
        let kernel_engine = engine();
        let model = published(
            &kernel_engine,
            &with(plate(), &[linear("q0000002", HOLE, &directions, skip)]),
        );
        assert!(copies(kernel_engine.kernel()).is_empty(), "{skip}");
        assert_eq!(
            names(&map(&model, "t0000001")),
            names(&map(&model, "q0000002"))
        );
        let (
            Some(FeatureOutcome::Built { built: pattern, .. }),
            Some(FeatureOutcome::Built { built: base, .. }),
        ) = (
            model.outcome(&id("t0000001")),
            model.outcome(&id("q0000002")),
        )
        else {
            panic!("not built");
        };
        assert_eq!(pattern.shape, base.shape);
        assert_eq!(pattern.summary(), base.summary());
        assert!(model.errors().next().is_none());
    }
}

#[test]
fn a_skipped_instance_and_a_shorter_count_placing_one_copy_alike_have_two_keys_and_two_names() {
    // A count of 3 with skip [[1]] against a count of 2, each an extent of 80: one copy at
    // 80 alike (section 10), named C2 and C1, each by its own steps.
    let kernel_engine = engine();
    let three = published(
        &kernel_engine,
        &with(
            plate(),
            &[linear(
                "q0000002",
                HOLE,
                &[along("x", "extent", "80", 3)],
                "[[1]]",
            )],
        ),
    );
    let two = published(
        &kernel_engine,
        &with(
            plate(),
            &[linear(
                "q0000002",
                HOLE,
                &[along("x", "extent", "80", 2)],
                "[]",
            )],
        ),
    );
    let at_80 = vec![Placement::Translation {
        offset: [80.0, 0.0, 0.0],
    }];
    // Two requests: the second's key isn't the first's, so it isn't taken from it.
    assert_eq!(
        copies(kernel_engine.kernel()),
        [
            (silico_protocol::CopiesOp::Cut, at_80.clone()),
            (silico_protocol::CopiesOp::Cut, at_80)
        ]
    );
    let pocket = map(&three, "q0000002");
    let (kind, wall) = own(&pocket, "q0000002").remove(0);
    let named = |model: &Model, instance: &str| {
        names(&map(model, "t0000001")).contains(&(
            kind,
            copy_of("t0000001", "linear_pattern", kind, 'C', instance, &wall),
        ))
    };
    assert!(named(&three, "2") && !named(&three, "1"));
    assert!(named(&two, "1") && !named(&two, "2"));
}

#[test]
fn originals_are_copied_one_step_each_in_timeline_order_whatever_the_list_says() {
    // z0000001 is pocketed before a0000002; the pattern lists a0000002 first.
    let features = vec![
        rectangle("s0000001", "xy", [-50.0, -30.0], 100.0, 60.0),
        pad("p0000001", None, "s0000001", FIVE),
        circles("s0000003", "xy", &[([-40.0, -20.0], 2.75)]),
        pocket("z0000001", "p0000001", "s0000003", THROUGH),
        circles("s0000004", "xy", &[([-40.0, 0.0], 1.5)]),
        pocket(
            "a0000002",
            "z0000001",
            "s0000004",
            r#"{"type": "one_side", "length": 4, "reversed": true}"#,
        ),
        linear(
            "a0000002",
            r#"{"type": "features", "features": ["a0000002", "z0000001"]}"#,
            &[along("x", "spacing", "20", 2)],
            "[]",
        ),
    ];
    let kernel_engine = engine();
    let model = published(&kernel_engine, &features);
    let calls: Vec<_> = kernel_engine
        .kernel()
        .calls()
        .into_iter()
        .filter_map(|call| match call.op {
            Op::Copies { base, tool, .. } => Some((
                FakeKernel::describe(&base).unwrap(),
                FakeKernel::describe(&tool).unwrap(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(calls.len(), 2);
    // The first copies z0000001's hole (through all) onto the base; the second a0000002's
    // (4 deep) onto the first's result.
    assert!(
        calls[0].1.contains("235.78") && !calls[0].0.contains("copies"),
        "{calls:?}"
    );
    assert!(
        calls[1].1.contains(", 4)") && calls[1].0.contains("copies"),
        "{calls:?}"
    );
    let held = names(&map(&model, "t0000001"));
    for original in ["z0000001", "a0000002"] {
        let sweep = own(&map(&model, original), original);
        for (kind, name) in sweep {
            let copy = copy_of("t0000001", "linear_pattern", kind, 'C', "1", &name);
            assert!(held.contains(&(kind, copy.clone())), "{copy}");
        }
    }
}

#[test]
fn a_copy_of_a_pad_on_a_pad_wraps_only_the_upper_pads_own_names() {
    // N10's third item: a tool's copy takes the original's sweep step's map, so a pad on a
    // pad, copied, wraps only the upper pad's names, never the lower's its last step keeps.
    let features = vec![
        rectangle("s0000001", "xy", [0.0, 0.0], 40.0, 20.0),
        pad("p0000001", None, "s0000001", FIVE),
        rectangle("s0000002", "xy", [2.0, 2.0], 4.0, 4.0),
        pad(
            "p0000002",
            Some("p0000001"),
            "s0000002",
            r#"{"type": "one_side", "length": 8, "reversed": false}"#,
        ),
        linear(
            "p0000002",
            r#"{"type": "features", "features": ["p0000002"]}"#,
            &[along("x", "spacing", "10", 2)],
            "[]",
        ),
    ];
    let model = published(&engine(), &features);
    let upper = own(&map(&model, "p0000002"), "p0000002");
    let copies: Vec<(ElementKind, String)> = names(&map(&model, "t0000001"))
        .into_iter()
        .filter(|(_, name)| name.contains("(C1 "))
        .collect();
    for (_, name) in &copies {
        assert!(name.contains("(C1 p0000002.pad."), "{name}");
    }
    assert_eq!(copies.len(), upper.len());
}

#[test]
fn a_copies_step_that_hangs_is_kept_for_the_session_until_check_with_fresh() {
    let kernel_engine = engine();
    kernel_engine.kernel().refuse(|op| {
        matches!(op, Op::Copies { .. }).then(|| {
            silico_core::Error::new(ErrorCode::WorkerTimeout, "the kernel worker timed out")
        })
    });
    let features = with(
        plate(),
        &[linear(
            "q0000002",
            HOLE,
            &[along("x", "spacing", "20", 2)],
            "[]",
        )],
    );
    for _ in 0..2 {
        let model = published(&kernel_engine, &features);
        let Some(FeatureOutcome::Failed(error)) = model.outcome(&id("t0000001")) else {
            panic!("{:?}", model.outcome(&id("t0000001")));
        };
        assert_eq!(error.code(), ErrorCode::WorkerTimeout);
        let suggestion = error.suggestion().unwrap();
        assert!(suggestion.contains("check with fresh"), "{suggestion}");
    }
    // Asked once: the second rebuild took the hang kept, as a description's that hangs is.
    assert_eq!(copies(kernel_engine.kernel()).len(), 1);
    // `check` with `fresh` asks again.
    let fresh = engine_fresh(&kernel_engine, &features);
    assert!(matches!(
        fresh.outcome(&id("t0000001")),
        Some(FeatureOutcome::Failed(_))
    ));
    assert_eq!(copies(kernel_engine.kernel()).len(), 2);
}

fn engine_fresh(engine: &Recompute<FakeKernel>, features: &[String]) -> Arc<Model> {
    match engine
        .start_fresh(checked(features))
        .wait_timeout(Duration::from_secs(60))
    {
        Ok(Ok(RunOutcome::Published(model))) => model,
        _ => panic!("not published"),
    }
}

/// A chamfer `c0000001` on the pattern's solid, of `edge`.
fn chamfer(edge: &str) -> String {
    format!(
        r#"{{"id": "c0000001", "type": "chamfer", "v": 1, "name": "Rim", "base": "t0000001", "edges": ["{edge}"], "size": {{"distance": 0.5}}}}"#
    )
}

/// The chamfer's outcome: built, or each broken reference's error.
fn chamfered(features: &[String]) -> Result<(), Vec<silico_core::Error>> {
    let model = published(&engine(), features);
    match model.outcome(&id("c0000001")) {
        Some(FeatureOutcome::Built { .. }) => Ok(()),
        Some(FeatureOutcome::Broken { errors, .. }) => Err(errors.clone()),
        other => panic!(
            "{other:?}, the pattern {:?}",
            model.outcome(&id("t0000001"))
        ),
    }
}

#[test]
fn every_edit_of_patterns_table_keeps_or_breaks_a_stored_copys_name_as_the_table_says() {
    // patterns.md §3's table, on the fake (names only): a chamfer on copy 2's edge, then
    // each edit of the pattern, by its text. The hole at the plate's centre, so its copies
    // lie on it either way along x and y.
    let plate = || plate_with_hole_at([0.0, 0.0]);
    let base = published(&engine(), &plate());
    let (kind, edge) = own(&map(&base, "q0000002"), "q0000002")
        .into_iter()
        .find(|(kind, name)| *kind == ElementKind::Edge && !name.contains('~'))
        .unwrap();
    let c2 = copy_of("t0000001", "linear_pattern", kind, 'C', "2", &edge);
    let pattern = |directions: &[String], skip: &str, copies: &str| {
        with(
            plate(),
            &[linear("q0000002", copies, directions, skip), chamfer(&c2)],
        )
    };
    let x3 = along("x", "extent", "40", 3);
    // As written: found.
    assert_eq!(
        chamfered(&pattern(std::slice::from_ref(&x3), "[]", HOLE)),
        Ok(())
    );
    // Kept: a count raised, a distribution changed, reversed, a direction added, the
    // directions swapped (a count of 1 first, so (2) is two steps along the new direction
    // 1), an instance skipped and taken back.
    for (directions, skip) in [
        (vec![along("x", "extent", "40", 5)], "[]"),
        (vec![along("x", "spacing", "7", 3)], "[]"),
        (
            vec![x3.replace("\"reversed\": false", "\"reversed\": true")],
            "[]",
        ),
        (vec![along("y", "extent", "20", 3)], "[]"),
        (vec![x3.clone(), along("y", "extent", "20", 2)], "[]"),
        (
            vec![along("y", "extent", "20", 3), along("x", "extent", "40", 1)],
            "[]",
        ),
        (vec![x3.clone()], "[[1]]"),
    ] {
        assert_eq!(
            chamfered(&pattern(&directions, skip, HOLE)),
            Ok(()),
            "{directions:?} {skip}"
        );
    }
    // Broken, loudly, with candidates: a count lowered past it, the instance skipped, the
    // copies switched to the whole solid.
    for (directions, skip, copies) in [
        (vec![along("x", "extent", "40", 2)], "[]", HOLE),
        (vec![x3.clone()], "[[2]]", HOLE),
        (vec![x3.clone()], "[]", r#"{"type": "solid"}"#),
    ] {
        let errors = chamfered(&pattern(&directions, skip, copies)).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
        assert_eq!(errors[0].parameter(), Some("edges.0"));
        assert!(
            !errors[0].candidates().is_empty(),
            "{directions:?} {skip} {copies}"
        );
    }
    // A grid's (0, 1), with direction 1 dropped: two offsets in a pattern of one direction,
    // which finds nothing, and the hole one step along the direction left, `C1`, among its
    // candidates.
    let c01 = copy_of("t0000001", "linear_pattern", kind, 'C', "0,1", &edge);
    let c1 = copy_of("t0000001", "linear_pattern", kind, 'C', "1", &edge);
    let grid = with(
        plate(),
        &[
            linear(
                "q0000002",
                HOLE,
                &[x3.clone(), along("y", "extent", "20", 2)],
                "[]",
            ),
            chamfer(&c01),
        ],
    );
    assert_eq!(chamfered(&grid), Ok(()));
    let dropped = with(
        plate(),
        &[
            linear("q0000002", HOLE, &[along("y", "extent", "20", 2)], "[]"),
            chamfer(&c01),
        ],
    );
    let errors = chamfered(&dropped).unwrap_err();
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
    assert!(
        errors[0].candidates().iter().any(|c| c == &c1),
        "{:?}",
        errors[0].candidates()
    );
}

#[test]
fn a_broken_reference_is_refused_naming_its_field_with_candidates_and_a_name_found_resolves() {
    // A direction along a pad's edge that isn't there: `reference_broken`, the field its
    // parameter, as a fillet's edge is (M2.8). The fake describes no edge as a line, so a
    // name found is refused for its kind, which says it was found.
    let found = map(&published(&engine(), &plate()), "q0000002")
        .iter()
        .find(|(element, names)| {
            element.kind == ElementKind::Edge
                && names[0].to_string().starts_with("p0000001.pad.Edge(G ")
        })
        .map(|(_, names)| names[0].to_string())
        .unwrap();
    // Its name as a split's piece (`M1/2`), which the plate's edge isn't: broken, the edge
    // itself its candidate.
    let missing = found.replacen("(G ", "(M1/2 ", 1);
    let model = published(
        &engine(),
        &with(
            plate(),
            &[linear(
                "q0000002",
                HOLE,
                &[along(&missing, "spacing", "10", 2)],
                "[]",
            )],
        ),
    );
    let Some(FeatureOutcome::Broken { errors, .. }) = model.outcome(&id("t0000001")) else {
        panic!("{:?}", model.outcome(&id("t0000001")));
    };
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code(), ErrorCode::ReferenceBroken);
    assert_eq!(errors[0].parameter(), Some("directions.0.along"));
    assert!(errors[0].candidates().contains(&found), "{errors:?}");
    let named = along(&found, "spacing", "10", 2);
    let model = published(
        &engine(),
        &with(plate(), &[linear("q0000002", HOLE, &[named], "[]")]),
    );
    let Some(FeatureOutcome::Failed(error)) = model.outcome(&id("t0000001")) else {
        panic!("{:?}", model.outcome(&id("t0000001")));
    };
    assert_eq!(error.code(), ErrorCode::InvalidInput);
    assert!(
        error
            .message()
            .ends_with(", is an edge of a curve OCCT doesn't type: a direction is a straight edge or a planar face"),
        "{error}"
    );
}
