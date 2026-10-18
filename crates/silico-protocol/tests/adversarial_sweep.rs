//! break-it: the rules for M3.6's sweeps of a profile's faces (silico_protocol's sweep
//! module): values at and around every edge the checks draw, the through-all length composed
//! with the prism it becomes, and `History::sweep_fault` on histories no worker would send.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::f64::consts::TAU;

use proptest::prelude::*;
use silico_protocol::{
    ANGULAR_TOLERANCE, ElementCounts, ElementFate, ElementKind, ElementRef, History, InputHistory,
    LENGTH_TOLERANCE, MAX_LENGTH, Reported, SweepKind, check_prism, check_revolution,
    check_through_all, is_whole_turn, radians, through_all_length,
};

/// A direction the check takes as a unit vector, 2 ulps long: (1 + 2ε, 0, 0), whose length
/// is 1 + 2ε, within check_unit's 4ε.
const LONG_UNIT: [f64; 3] = [1.0 + 2.0 * f64::EPSILON, 0.0, 0.0];

/// The box `[0, d] × [0, 0] × [0, 0]` whose through-all length is at most a kilometre, but
/// no more than a few ulps under it.
fn box_at_the_kilometre() -> ([[f64; 3]; 2], f64) {
    let mut d = MAX_LENGTH / 2.02;
    for _ in 0..1000 {
        let corners = [[0.0; 3], [d, 0.0, 0.0]];
        match through_all_length(corners) {
            Ok(length) if length * LONG_UNIT[0] > MAX_LENGTH => return (corners, length),
            Ok(_) => d = d.next_up(),
            Err(_) => d = d.next_down(),
        }
    }
    panic!("no box found");
}

#[test]
fn every_through_all_the_rules_admit_is_a_prism_the_rules_admit() {
    // Found by break-it, fixed: check_through_all takes a direction within 4ε of unit
    // length, and through_all_length any length up to a kilometre, but check_prism refused
    // their product a few ulps past a kilometre: the kernel refused a base the rule admits
    // naming `vector`, and the fake wrote a shape it then couldn't read. check_prism now
    // allows the rounding a unit direction adds (MAX_SWEEP), so the rules compose.
    let (corners, length) = box_at_the_kilometre();
    assert!(length <= MAX_LENGTH);
    assert_eq!(check_through_all(LONG_UNIT), Ok(()));
    let vector = LONG_UNIT.map(|c| c * length);
    assert_eq!(
        check_prism([0.0; 3], vector),
        Ok(()),
        "box {corners:?}: through all {length} along {LONG_UNIT:?}"
    );
}

#[test]
fn angles_at_and_around_every_edge_the_revolution_check_draws() {
    let z = [0.0, 0.0, 1.0];
    let ok = |angle: f64| check_revolution([0.0; 3], z, 0.0, angle).is_ok();
    // OCCT refuses an angle of at most Precision::Angular (BRepSweep_Revol's constructor);
    // the check refuses the same, and takes the next float.
    assert!(!ok(ANGULAR_TOLERANCE));
    assert!(ok(ANGULAR_TOLERANCE.next_up()));
    // A whole turn is radians(360), exactly TAU; the next float up is past it.
    assert!(ok(radians(360.0)));
    assert!(ok(TAU));
    assert!(!ok(TAU.next_up()));
    assert!(!ok(-0.0));
    assert!(!ok(f64::MIN_POSITIVE));
    assert!(!ok(5e-324));
    assert!(!ok(f64::INFINITY));
    // Degrees passed as radians: 90 is refused, 6 is a valid turn (and not a whole one).
    assert!(!ok(90.0));
    assert!(ok(6.0));
    assert!(!is_whole_turn(6.0));
    // OCCT's whole-turn test (|Angle(D) − 2π| ≤ Precision::Angular, BRepSweep_Revol's
    // NumShape) against is_whole_turn, at its edges.
    for angle in [
        TAU,
        TAU - ANGULAR_TOLERANCE,
        TAU - 2.0 * ANGULAR_TOLERANCE,
        TAU.next_down(),
    ] {
        let occt = (angle - 2.0 * std::f64::consts::PI).abs() <= 1e-12;
        assert_eq!(is_whole_turn(angle), occt, "{angle}");
    }
    // Starts: within a turn either way, -0 as 0.
    for start in [-TAU, TAU, -0.0, 5e-324] {
        assert!(check_revolution([0.0; 3], z, start, 1.0).is_ok(), "{start}");
    }
    for start in [
        TAU.next_up(),
        (-TAU).next_down(),
        f64::NAN,
        f64::NEG_INFINITY,
    ] {
        let e = check_revolution([0.0; 3], z, start, 1.0).unwrap_err();
        assert_eq!(e.parameter, "start", "{start}");
    }
}

#[test]
fn directions_whose_length_overflows_or_underflows_are_refused_naming_the_axis() {
    for direction in [
        [1e300, 0.0, 1e300],
        [f64::MAX, 0.0, 0.0],
        [1e-200, 0.0, 0.0],
        [5e-324, 5e-324, 5e-324],
        [-0.0, -0.0, -0.0],
    ] {
        let e = check_revolution([0.0; 3], direction, 0.0, 1.0).unwrap_err();
        assert_eq!(e.parameter, "axis", "{direction:?}");
        let e = check_through_all(direction).unwrap_err();
        assert_eq!(e.parameter, "direction", "{direction:?}");
    }
    // A vector whose squared length overflows is refused as too long, not taken as 0.
    let e = check_prism([0.0; 3], [1e300, 0.0, 1e300]).unwrap_err();
    assert_eq!(e.parameter, "vector");
    // A vector of subnormals, whose squared length underflows to 0: refused, too short.
    let e = check_prism([0.0; 3], [5e-324, 0.0, 0.0]).unwrap_err();
    assert_eq!(e.parameter, "vector");
}

#[test]
fn a_box_with_infinite_or_inverted_corners_is_no_through_all_length() {
    for corners in [
        [[0.0; 3], [f64::INFINITY, 0.0, 0.0]],
        [[f64::NEG_INFINITY, 0.0, 0.0], [0.0; 3]],
        [[f64::NAN; 3], [f64::NAN; 3]],
        [[f64::MAX, 0.0, 0.0], [-f64::MAX, 0.0, 0.0]],
    ] {
        assert!(through_all_length(corners).is_err(), "{corners:?}");
    }
}

// --- sweep_fault on any history ------------------------------------------------------------

fn kind() -> impl Strategy<Value = ElementKind> {
    prop_oneof![
        Just(ElementKind::Face),
        Just(ElementKind::Edge),
        Just(ElementKind::Vertex),
        Just(ElementKind::Solid),
    ]
}

fn reported() -> impl Strategy<Value = Reported> {
    (
        kind(),
        prop_oneof![Just(0u32), Just(1), 1u32..12, Just(u32::MAX)],
        prop_oneof![Just(0u32), Just(1), Just(u32::MAX)],
    )
        .prop_map(|(kind, index, position)| Reported {
            element: ElementRef { kind, index },
            position,
        })
}

fn fate() -> impl Strategy<Value = ElementFate> {
    (
        prop::option::of(prop_oneof![Just(0u32), 1u32..5, Just(u32::MAX)]),
        prop::collection::vec(reported(), 0..3),
        prop::collection::vec(reported(), 0..4),
    )
        .prop_map(|(kept, modified, generated)| ElementFate {
            kept,
            modified,
            generated,
        })
}

fn history() -> impl Strategy<Value = History> {
    let fates = |n| prop::collection::vec(fate(), 0..n);
    let count = || prop_oneof![Just(0u32), 1u32..20, Just(u32::MAX)];
    (
        prop::collection::vec((fates(3), fates(6), fates(6)), 0..3),
        (count(), count(), count(), count()),
    )
        .prop_map(|(inputs, (faces, edges, vertices, solids))| History {
            inputs: inputs
                .into_iter()
                .map(|(faces, edges, vertices)| InputHistory {
                    faces,
                    edges,
                    vertices,
                })
                .collect(),
            result: ElementCounts {
                faces,
                edges,
                vertices,
                solids,
            },
            ..History::default()
        })
}

/// A triangle's prism as the rule has it: side faces 1–3, start face 4, end face 5; start
/// edges 1–3, end edges 4–6, side edges 7–9; start vertices 1–3, end 4–6; solid 1.
fn triangle_prism() -> History {
    let r = |kind, index| Reported {
        element: ElementRef { kind, index },
        position: 1,
    };
    let fate = |start, generated| ElementFate {
        kept: None,
        modified: vec![start],
        generated,
    };
    use ElementKind::{Edge, Face, Solid, Vertex};
    History {
        inputs: vec![InputHistory {
            faces: vec![fate(r(Face, 4), vec![r(Face, 5), r(Solid, 1)])],
            edges: (1..=3)
                .map(|i| fate(r(Edge, i), vec![r(Edge, i + 3), r(Face, i)]))
                .collect(),
            vertices: (1..=3)
                .map(|j| fate(r(Vertex, j), vec![r(Vertex, j + 3), r(Edge, j + 6)]))
                .collect(),
        }],
        result: ElementCounts {
            faces: 5,
            edges: 9,
            vertices: 6,
            solids: 1,
        },
        ..History::default()
    }
}

/// One change to a history: (which fate, which list, which entry, what to do, a value).
type Change = (usize, u8, usize, u8, u32);

fn spoil(history: &mut History, (fate, list, entry, what, value): Change) {
    let input = &mut history.inputs[0];
    let n = input.faces.len() + input.edges.len() + input.vertices.len();
    let fate = fate % n;
    let fate = if fate < input.faces.len() {
        &mut input.faces[fate]
    } else if fate < input.faces.len() + input.edges.len() {
        &mut input.edges[fate - input.faces.len()]
    } else {
        &mut input.vertices[fate - input.faces.len() - input.edges.len()]
    };
    let kinds = [
        ElementKind::Face,
        ElementKind::Edge,
        ElementKind::Vertex,
        ElementKind::Solid,
    ];
    let list = if list % 2 == 0 {
        &mut fate.modified
    } else {
        &mut fate.generated
    };
    match what % 5 {
        0 if !list.is_empty() => {
            let i = entry % list.len();
            list[i].element.index = value % 12;
        }
        1 if !list.is_empty() => {
            let i = entry % list.len();
            list[i].element.kind = kinds[value as usize % 4];
        }
        2 if !list.is_empty() => {
            list.remove(entry % list.len());
        }
        3 => list.push(Reported {
            element: ElementRef {
                kind: kinds[value as usize % 4],
                index: value % 12,
            },
            position: 1,
        }),
        _ => fate.kept = Some(value % 6),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4000))]

    /// A valid prism's history with one to three changes: whatever both checks pass is still
    /// a prism's (each element one start of its kind, one end of its kind, one side of the
    /// next kind up, nothing twice).
    #[test]
    fn a_spoiled_prism_history_passes_only_if_it_is_still_a_prism(
        changes in prop::collection::vec(
            (0usize..7, 0u8..2, 0usize..3, 0u8..5, 0u32..1000), 1..4
        )
    ) {
        let mut history = triangle_prism();
        for change in changes {
            spoil(&mut history, change);
        }
        if history.validate(1).is_ok() && history.sweep_fault(SweepKind::Prism).is_none() {
            let input = &history.inputs[0];
            let mut seen = std::collections::BTreeSet::new();
            for (own, side, fates) in [
                (ElementKind::Face, ElementKind::Solid, &input.faces),
                (ElementKind::Edge, ElementKind::Face, &input.edges),
                (ElementKind::Vertex, ElementKind::Edge, &input.vertices),
            ] {
                for fate in fates {
                    prop_assert!(fate.kept.is_none());
                    prop_assert_eq!(fate.modified.len(), 1);
                    prop_assert_eq!(fate.modified[0].element.kind, own);
                    prop_assert_eq!(fate.generated.len(), 2);
                    prop_assert_eq!(
                        fate.generated.iter().filter(|r| r.element.kind == own).count(), 1
                    );
                    prop_assert_eq!(
                        fate.generated.iter().filter(|r| r.element.kind == side).count(), 1
                    );
                    for r in fate.modified.iter().chain(&fate.generated) {
                        prop_assert!(seen.insert(r.element));
                    }
                }
            }
        }
    }

    /// sweep_fault never panics, and what it passes keeps the rule it states: never kept,
    /// no result element reported twice, a solid per face, and for a prism exactly one
    /// start, end, and side per element and the counts they make.
    #[test]
    fn sweep_fault_passes_only_what_keeps_the_rule(history in history()) {
        for kind in [SweepKind::Prism, SweepKind::Turn, SweepKind::WholeTurn] {
            if history.sweep_fault(kind).is_some() {
                continue;
            }
            let Some(input) = history.inputs.first() else { continue };
            prop_assert_eq!(history.result.solids as usize, input.faces.len());
            let mut seen = std::collections::BTreeSet::new();
            for fate in input.faces.iter().chain(&input.edges).chain(&input.vertices) {
                prop_assert!(fate.kept.is_none());
                for r in fate.modified.iter().chain(&fate.generated) {
                    prop_assert!(seen.insert(r.element), "{:?} twice", r.element);
                }
            }
            if kind == SweepKind::Prism {
                for (want, fates) in [
                    (ElementKind::Face, &input.faces),
                    (ElementKind::Edge, &input.edges),
                    (ElementKind::Vertex, &input.vertices),
                ] {
                    for fate in fates {
                        prop_assert_eq!(fate.modified.len(), 1);
                        prop_assert_eq!(fate.generated.len(), 2);
                        prop_assert!(fate.generated.iter().any(|r| r.element.kind == want));
                    }
                }
            }
        }
    }

    /// The checks never panic, and what they pass is finite and within the kilometre.
    #[test]
    fn the_checks_pass_only_finite_values_within_reach(
        start in prop::array::uniform3(edge_float()),
        vector in prop::array::uniform3(edge_float()),
        angle in edge_float(),
        turn in edge_float(),
    ) {
        if check_prism(start, vector).is_ok() {
            prop_assert!(start.iter().chain(&vector).all(|c| c.is_finite()));
            prop_assert!(start.iter().all(|c| c.abs() <= MAX_LENGTH));
        }
        if check_revolution(start, vector, turn, angle).is_ok() {
            prop_assert!(angle > ANGULAR_TOLERANCE && angle <= TAU);
            prop_assert!(turn.abs() <= TAU);
            let size = vector.iter().map(|c| c * c).sum::<f64>().sqrt();
            prop_assert!((size - 1.0).abs() <= 4.0 * f64::EPSILON);
        }
        if let Ok(length) = through_all_length([start, vector]) {
            prop_assert!(length > LENGTH_TOLERANCE && length <= MAX_LENGTH);
        }
    }
}

/// Floats biased to the edges the checks draw.
fn edge_float() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0),
        Just(-0.0),
        Just(1.0),
        Just(-1.0),
        Just(1.0 + 4.0 * f64::EPSILON),
        Just(1.0 + 8.0 * f64::EPSILON),
        Just(LENGTH_TOLERANCE),
        Just(LENGTH_TOLERANCE.next_up()),
        Just(MAX_LENGTH),
        Just(MAX_LENGTH.next_up()),
        Just(ANGULAR_TOLERANCE),
        Just(TAU),
        Just(TAU.next_up()),
        Just(f64::NAN),
        Just(f64::INFINITY),
        Just(f64::NEG_INFINITY),
        Just(f64::MAX),
        Just(5e-324),
        -1e7f64..1e7,
    ]
}
