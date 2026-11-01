//! /break-it on `SketchSystem::parts` and the drag's `part(points)` rebuilt on it (decision 5
//! per part). A part is a set of unknowns joined by the constraints that read them, as
//! PlaneGCS's rows read them (`System::initSolution` joins a constraint's params that are in
//! its param list, and nothing else); fixed values join nothing. The expected partition here
//! is worked out independently: each constraint's reads written from PlaneGCS's own
//! constraint classes (FreeCAD's `GCS.cpp`, `Constraints.cpp`, the bridge's calls), and the
//! parts found by a breadth-first search over them, not by union-find.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use proptest::prelude::*;
use silico_protocol::{
    Algorithm, DragBegin, Geometry, PointOf, PointValues, SketchGeometry, SketchSystem,
    SolverConstraint, Tag, Value,
};

// ---------------------------------------------------------------------------------------
// An independent reading of what each row reads.

fn point_values(s: &SketchSystem, p: PointOf) -> Option<PointValues> {
    match (s.geometry.get(p.geometry().0 as usize)?, p) {
        (SketchGeometry::Point(v), PointOf::Point(_)) => Some(*v),
        (SketchGeometry::Line { start, .. }, PointOf::Start(_)) => Some(*start),
        (SketchGeometry::Line { end, .. }, PointOf::End(_)) => Some(*end),
        (SketchGeometry::Arc { start, .. }, PointOf::Start(_)) => Some(*start),
        (SketchGeometry::Arc { end, .. }, PointOf::End(_)) => Some(*end),
        (SketchGeometry::Circle { center, .. }, PointOf::Center(_)) => Some(*center),
        (SketchGeometry::Arc { center, .. }, PointOf::Center(_)) => Some(*center),
        _ => None,
    }
}

/// Every value of a piece of geometry, as PlaneGCS's `PushOwnParams` pushes a curve's.
fn all_of(s: &SketchSystem, g: Geometry) -> Vec<Value> {
    match s.geometry.get(g.0 as usize) {
        None => vec![],
        Some(SketchGeometry::Point(p)) => vec![p.x, p.y],
        Some(SketchGeometry::Line { start, end }) => vec![start.x, start.y, end.x, end.y],
        Some(SketchGeometry::Circle { center, radius }) => vec![center.x, center.y, *radius],
        Some(SketchGeometry::Arc {
            center,
            radius,
            start_angle,
            end_angle,
            start,
            end,
        }) => vec![
            center.x,
            center.y,
            *radius,
            *start_angle,
            *end_angle,
            start.x,
            start.y,
            end.x,
            end.y,
        ],
    }
}

fn pt(s: &SketchSystem, p: PointOf) -> Vec<Value> {
    point_values(s, p).map_or_else(Vec::new, |v| vec![v.x, v.y])
}

/// A circle's or arc's centre and radius (`GCS::Circle`).
fn circle_of(s: &SketchSystem, g: Geometry) -> Vec<Value> {
    match s.geometry.get(g.0 as usize) {
        Some(
            SketchGeometry::Circle { center, radius } | SketchGeometry::Arc { center, radius, .. },
        ) => {
            vec![center.x, center.y, *radius]
        }
        _ => vec![],
    }
}

fn radius_of(s: &SketchSystem, g: Geometry) -> Vec<Value> {
    match s.geometry.get(g.0 as usize) {
        Some(SketchGeometry::Circle { radius, .. } | SketchGeometry::Arc { radius, .. }) => {
            vec![*radius]
        }
        _ => vec![],
    }
}

/// What the row(s) for `c` read: never less than PlaneGCS's rows read (or a part would be
/// finer than PlaneGCS's), and here, for the joins between a point's x and y the Rust side
/// adds (a `distance_x` reads both points whole), the same.
fn reads(s: &SketchSystem, c: &SolverConstraint) -> Vec<Value> {
    use SolverConstraint as S;
    let cat = |v: &[Vec<Value>]| v.concat();
    match *c {
        S::ArcRules { arc } => all_of(s, arc),
        S::Coincident { a, b, .. }
        | S::HorizontalPoints { a, b, .. }
        | S::VerticalPoints { a, b, .. } => cat(&[pt(s, a), pt(s, b)]),
        S::DistanceX { a, b, value, .. }
        | S::DistanceY { a, b, value, .. }
        | S::Distance { a, b, value, .. } => cat(&[pt(s, a), pt(s, b), vec![value]]),
        S::PointOnLine { point, line, .. } => cat(&[pt(s, point), all_of(s, line)]),
        S::Horizontal { line, .. } | S::Vertical { line, .. } => all_of(s, line),
        S::Parallel { a, b, .. } | S::Perpendicular { a, b, .. } | S::EqualLength { a, b, .. } => {
            cat(&[all_of(s, a), all_of(s, b)])
        }
        S::Angle { a, b, value, .. } => cat(&[all_of(s, a), all_of(s, b), vec![value]]),
        S::Radius { circle, value, .. } => cat(&[radius_of(s, circle), vec![value]]),
        S::PointLineDistance {
            point, line, value, ..
        } => cat(&[pt(s, point), all_of(s, line), vec![value]]),
        S::PointOnCircle { point, circle, .. } => cat(&[pt(s, point), circle_of(s, circle)]),
        S::EqualRadius { a, b, .. } | S::TangentCircles { a, b, .. } => {
            cat(&[circle_of(s, a), circle_of(s, b)])
        }
        S::TangentLine { line, circle, .. } => cat(&[all_of(s, line), circle_of(s, circle)]),
        S::TangentOn {
            a, point, b, angle, ..
        }
        | S::AngleViaPoint {
            a, b, point, angle, ..
        } => cat(&[pt(s, point), all_of(s, a), all_of(s, b), vec![angle]]),
        S::SymmetricLine { a, b, line, .. } | S::MidpointOnLine { a, b, line, .. } => {
            cat(&[pt(s, a), pt(s, b), all_of(s, line)])
        }
        S::SymmetricPoint { a, b, center, .. } => cat(&[pt(s, a), pt(s, b), pt(s, center)]),
    }
}

/// Each unknown's part by breadth-first search: the least unknown it's joined to.
fn reference_parts(s: &SketchSystem) -> BTreeMap<Value, Value> {
    let unknowns: BTreeSet<Value> = s.unknowns.iter().copied().collect();
    let mut adjacent: BTreeMap<Value, BTreeSet<Value>> = BTreeMap::new();
    for c in &s.constraints {
        let read: Vec<Value> = reads(s, c)
            .into_iter()
            .filter(|v| unknowns.contains(v))
            .collect();
        for &a in &read {
            for &b in &read {
                adjacent.entry(a).or_default().insert(b);
            }
        }
    }
    let mut part = BTreeMap::new();
    for &start in &unknowns {
        if part.contains_key(&start) {
            continue;
        }
        // `start` is the least unknown not yet reached, so it's its part's least.
        let mut queue = VecDeque::from([start]);
        part.insert(start, start);
        while let Some(v) = queue.pop_front() {
            for &w in adjacent.get(&v).into_iter().flatten() {
                if let std::collections::btree_map::Entry::Vacant(e) = part.entry(w) {
                    e.insert(start);
                    queue.push_back(w);
                }
            }
        }
    }
    part
}

/// `parts()` as the same map: each unknown to the least unknown in its part.
fn crate_parts(s: &SketchSystem) -> BTreeMap<Value, Value> {
    let parts = s.parts();
    let mut least = BTreeMap::new();
    for &v in &s.unknowns {
        let p = parts
            .of(v)
            .unwrap_or_else(|| panic!("unknown {v:?} has no part"));
        least.entry(p).or_insert(v);
    }
    s.unknowns
        .iter()
        .map(|&v| (v, least[&parts.of(v).unwrap()]))
        .collect()
}

// ---------------------------------------------------------------------------------------
// Systems, as the core's translation lays them out, and stranger.

#[derive(Debug, Clone)]
struct Plan {
    /// Each point: whether its values are fixed (not unknowns).
    points: Vec<bool>,
    /// Lines between points (indices into all points, origin first).
    lines: Vec<(usize, usize)>,
    /// Circles about points.
    circles: Vec<usize>,
    /// Arcs: centre, start, end, and whether its rules are sent.
    arcs: Vec<(usize, usize, usize, bool)>,
    /// Constraints: a kind, three geometry picks, three point-position picks, and whether
    /// its value is an unknown (a corrupt system) rather than fixed.
    constraints: Vec<(u8, [usize; 3], [u8; 3], bool)>,
}

fn plan() -> impl Strategy<Value = Plan> {
    (
        prop::collection::vec(prop::bool::weighted(0.15), 0..10),
        prop::collection::vec((0usize..12, 0usize..12), 0..6),
        prop::collection::vec(0usize..12, 0..4),
        prop::collection::vec(
            (
                0usize..12,
                0usize..12,
                0usize..12,
                prop::bool::weighted(0.8),
            ),
            0..3,
        ),
        prop::collection::vec(
            (
                0u8..24,
                [0usize..40, 0usize..40, 0usize..40],
                [0u8..4, 0u8..4, 0u8..4],
                prop::bool::weighted(0.05),
            ),
            0..14,
        ),
    )
        .prop_map(|(points, lines, circles, arcs, constraints)| Plan {
            points,
            lines,
            circles,
            arcs,
            constraints,
        })
}

fn build(plan: &Plan) -> SketchSystem {
    // The origin and the axes' far ends, fixed.
    let mut values = vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
    let mut unknowns = Vec::new();
    let origin = PointValues {
        x: Value(0),
        y: Value(1),
    };
    let mut geometry = vec![
        SketchGeometry::Point(origin),
        SketchGeometry::Line {
            start: origin,
            end: PointValues {
                x: Value(2),
                y: Value(3),
            },
        },
        SketchGeometry::Line {
            start: origin,
            end: PointValues {
                x: Value(4),
                y: Value(5),
            },
        },
    ];
    let value = |values: &mut Vec<f64>, unknowns: &mut Vec<Value>, v: f64, unknown: bool| {
        values.push(v);
        let at = Value((values.len() - 1) as u32);
        if unknown {
            unknowns.push(at);
        }
        at
    };
    let mut pvs = vec![origin];
    for (i, &fixed) in plan.points.iter().enumerate() {
        let p = PointValues {
            x: value(&mut values, &mut unknowns, i as f64, !fixed),
            y: value(&mut values, &mut unknowns, -(i as f64), !fixed),
        };
        geometry.push(SketchGeometry::Point(p));
        pvs.push(p);
    }
    let pick = |i: usize| pvs[i % pvs.len()];
    for &(a, b) in &plan.lines {
        geometry.push(SketchGeometry::Line {
            start: pick(a),
            end: pick(b),
        });
    }
    for &c in &plan.circles {
        let radius = value(&mut values, &mut unknowns, 3.0, true);
        geometry.push(SketchGeometry::Circle {
            center: pick(c),
            radius,
        });
    }
    let mut rules = Vec::new();
    for &(c, a, b, ruled) in &plan.arcs {
        let radius = value(&mut values, &mut unknowns, 2.0, true);
        let start_angle = value(&mut values, &mut unknowns, 0.0, true);
        let end_angle = value(&mut values, &mut unknowns, 1.0, true);
        geometry.push(SketchGeometry::Arc {
            center: pick(c),
            radius,
            start_angle,
            end_angle,
            start: pick(a),
            end: pick(b),
        });
        if ruled {
            rules.push(SolverConstraint::ArcRules {
                arc: Geometry((geometry.len() - 1) as u32),
            });
        }
    }
    let n = geometry.len();
    let mut constraints = rules;
    for (i, &(kind, g, pos, unknown_value)) in plan.constraints.iter().enumerate() {
        use SolverConstraint as S;
        let tag = Tag(i as u32 + 1);
        // Mostly real geometry; now and then past the end.
        let geo = |k: usize| {
            Geometry(if g[k] < 36 {
                (g[k] % n) as u32
            } else {
                (n + g[k]) as u32
            })
        };
        let at = |k: usize| match pos[k] {
            0 => PointOf::Point(geo(k)),
            1 => PointOf::Start(geo(k)),
            2 => PointOf::End(geo(k)),
            _ => PointOf::Center(geo(k)),
        };
        let v = value(&mut values, &mut unknowns, 5.0, unknown_value);
        let c = match kind {
            0 => S::Coincident {
                tag,
                a: at(0),
                b: at(1),
            },
            1 => S::Horizontal { tag, line: geo(0) },
            2 => S::Vertical { tag, line: geo(0) },
            3 => S::DistanceX {
                tag,
                a: at(0),
                b: at(1),
                value: v,
            },
            4 => S::DistanceY {
                tag,
                a: at(0),
                b: at(1),
                value: v,
            },
            5 => S::Distance {
                tag,
                a: at(0),
                b: at(1),
                value: v,
            },
            6 => S::PointOnLine {
                tag,
                point: at(0),
                line: geo(1),
            },
            7 => S::Parallel {
                tag,
                a: geo(0),
                b: geo(1),
            },
            8 => S::Perpendicular {
                tag,
                a: geo(0),
                b: geo(1),
            },
            9 => S::Angle {
                tag,
                a: geo(0),
                b: geo(1),
                reversed: false,
                value: v,
            },
            10 => S::EqualLength {
                tag,
                a: geo(0),
                b: geo(1),
            },
            11 => S::Radius {
                tag,
                circle: geo(0),
                value: v,
            },
            12 => S::HorizontalPoints {
                tag,
                a: at(0),
                b: at(1),
            },
            13 => S::VerticalPoints {
                tag,
                a: at(0),
                b: at(1),
            },
            14 => S::PointLineDistance {
                tag,
                point: at(0),
                line: geo(1),
                value: v,
            },
            15 => S::PointOnCircle {
                tag,
                point: at(0),
                circle: geo(1),
            },
            16 => S::EqualRadius {
                tag,
                a: geo(0),
                b: geo(1),
            },
            17 => S::TangentLine {
                tag,
                line: geo(0),
                circle: geo(1),
            },
            18 => S::TangentCircles {
                tag,
                a: geo(0),
                b: geo(1),
                internal: pos[2] % 2 == 0,
            },
            19 => S::TangentOn {
                tag,
                a: geo(0),
                point: at(1),
                b: geo(2),
                angle: v,
            },
            20 => S::AngleViaPoint {
                tag,
                a: geo(0),
                b: geo(1),
                point: at(2),
                angle: v,
            },
            21 => S::SymmetricLine {
                tag,
                a: at(0),
                b: at(1),
                line: geo(2),
            },
            22 => S::MidpointOnLine {
                tag,
                a: at(0),
                b: at(1),
                line: geo(2),
            },
            _ => S::SymmetricPoint {
                tag,
                a: at(0),
                b: at(1),
                center: at(2),
            },
        };
        constraints.push(c);
    }
    SketchSystem {
        values,
        unknowns,
        geometry,
        constraints,
        algorithms: vec![Algorithm::DogLeg],
    }
}

/// The part of `s` that `points` are in, worked out from the reference partition.
fn reference_part(s: &SketchSystem, points: &[PointOf]) -> (Vec<Value>, Vec<SolverConstraint>) {
    let parts = reference_parts(s);
    let wanted: BTreeSet<Value> = points
        .iter()
        .filter_map(|&p| point_values(s, p))
        .flat_map(|v| [v.x, v.y])
        .filter_map(|v| parts.get(&v).copied())
        .collect();
    let in_part = |v: &Value| parts.get(v).is_some_and(|p| wanted.contains(p));
    let unknowns = s.unknowns.iter().copied().filter(|v| in_part(v)).collect();
    let constraints = s
        .constraints
        .iter()
        .copied()
        .filter(|c| reads(s, c).iter().any(in_part))
        .collect();
    (unknowns, constraints)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3000))]

    /// Every unknown is in the part a breadth-first search over PlaneGCS's reads puts it
    /// in; no fixed value, and no value past the system, is in any.
    #[test]
    fn parts_are_the_components_of_the_unknowns_joined_by_every_row(plan in plan()) {
        let s = build(&plan);
        prop_assert_eq!(crate_parts(&s), reference_parts(&s));
        let parts = s.parts();
        let unknowns: BTreeSet<Value> = s.unknowns.iter().copied().collect();
        for i in 0..s.values.len() as u32 + 3 {
            if !unknowns.contains(&Value(i)) {
                prop_assert_eq!(parts.of(Value(i)), None, "fixed or absent value {}", i);
            }
        }
        prop_assert_eq!(parts.of(Value(u32::MAX)), None);
    }

    /// A row's part is the part of every unknown it reads, and none when it reads none.
    #[test]
    fn a_rows_part_is_every_unknown_it_reads(plan in plan()) {
        let s = build(&plan);
        let parts = s.parts();
        let unknowns: BTreeSet<Value> = s.unknowns.iter().copied().collect();
        for c in &s.constraints {
            let read: Vec<Value> = reads(&s, c).into_iter().filter(|v| unknowns.contains(v)).collect();
            let got = parts.of_constraint(c);
            match read.first() {
                None => prop_assert_eq!(got, None, "{:?} reads no unknown", c),
                Some(_) => {
                    for v in &read {
                        prop_assert_eq!(got, parts.of(*v), "{:?} and {:?}", c, v);
                    }
                }
            }
        }
    }

    /// Parts come from the system's structure alone: no value, however wild, and no order of
    /// the constraints changes them, or the drag's part.
    #[test]
    fn parts_dont_depend_on_values_or_the_rows_order(
        plan in plan(),
        wild in prop::collection::vec(
            prop_oneof![
                Just(f64::NAN), Just(f64::INFINITY), Just(f64::NEG_INFINITY), Just(0.0),
                Just(-0.0), Just(1e300), Just(-1e-300), Just(f64::MIN_POSITIVE), -1e6f64..1e6
            ],
            1..8,
        ),
        drag in prop::collection::vec((0usize..40, 0u8..4), 0..4),
    ) {
        let s = build(&plan);
        let before = crate_parts(&s);
        let mut moved = s.clone();
        for (i, v) in moved.values.iter_mut().enumerate() {
            *v = wild[i % wild.len()];
        }
        prop_assert_eq!(&crate_parts(&moved), &before);
        let mut reversed = s.clone();
        reversed.constraints.reverse();
        prop_assert_eq!(&crate_parts(&reversed), &before);

        let n = s.geometry.len();
        let points: Vec<PointOf> = drag
            .iter()
            .map(|&(g, p)| {
                let g = Geometry((g % (n + 2)) as u32);
                match p {
                    0 => PointOf::Point(g),
                    1 => PointOf::Start(g),
                    2 => PointOf::End(g),
                    _ => PointOf::Center(g),
                }
            })
            .collect();
        let part = s.part(&points);
        let (unknowns, constraints) = reference_part(&s, &points);
        prop_assert_eq!(&part.unknowns, &unknowns);
        prop_assert_eq!(&part.constraints, &constraints);
        prop_assert_eq!(&part.values, &s.values);
        prop_assert_eq!(&part.geometry, &s.geometry);
        let part_moved = moved.part(&points);
        prop_assert_eq!(&part_moved.unknowns, &unknowns);
        prop_assert_eq!(&part_moved.constraints, &constraints);
        // A part's parts: the same unknowns, joined the same way.
        let inner = crate_parts(&part);
        for (v, least) in &inner {
            prop_assert_eq!(before.get(v), Some(least));
        }
    }
}

// ---------------------------------------------------------------------------------------
// Targeted.

fn pv(x: u32) -> PointValues {
    PointValues {
        x: Value(x),
        y: Value(x + 1),
    }
}

/// The origin and axes, then `n` points p_i at values (6 + 2i, 7 + 2i), all unknowns.
fn points(n: u32) -> SketchSystem {
    let mut geometry = vec![
        SketchGeometry::Point(pv(0)),
        SketchGeometry::Line {
            start: pv(0),
            end: pv(2),
        },
        SketchGeometry::Line {
            start: pv(0),
            end: pv(4),
        },
    ];
    geometry.extend((0..n).map(|i| SketchGeometry::Point(pv(6 + 2 * i))));
    SketchSystem {
        values: [0.0, 0.0, 1.0, 0.0, 0.0, 1.0]
            .into_iter()
            .chain((0..2 * n).map(|i| f64::from(i) + 1.0))
            .collect(),
        unknowns: (6..6 + 2 * n).map(Value).collect(),
        geometry,
        constraints: vec![],
        algorithms: vec![Algorithm::DogLeg],
    }
}

#[test]
fn a_lone_points_x_and_y_are_two_parts_and_dragging_it_holds_both() {
    // No row reads p0: PlaneGCS solves its x and y apart (two params, no constraint), so
    // they're two parts; the drag's part of the point is both.
    let s = points(1);
    let parts = s.parts();
    assert_ne!(parts.of(Value(6)), parts.of(Value(7)));
    let part = s.part(&[PointOf::Point(Geometry(3))]);
    assert_eq!(part.unknowns, [Value(6), Value(7)]);
    assert!(part.constraints.is_empty());
}

#[test]
fn rows_through_the_origin_and_axes_join_nothing_however_many() {
    // 500 points, each made coincident with the origin and put on both axes. Every row
    // reads the fixed origin or an axis besides the point, so each point stays its own part.
    let mut s = points(500);
    let mut tag = 0;
    let mut next = || {
        tag += 1;
        Tag(tag)
    };
    for i in 0..500u32 {
        let p = PointOf::Point(Geometry(3 + i));
        s.constraints.push(SolverConstraint::Coincident {
            tag: next(),
            a: PointOf::Point(Geometry(0)),
            b: p,
        });
        s.constraints.push(SolverConstraint::PointOnLine {
            tag: next(),
            point: p,
            line: Geometry(1),
        });
        s.constraints.push(SolverConstraint::PointOnLine {
            tag: next(),
            point: p,
            line: Geometry(2),
        });
    }
    let parts = s.parts();
    let distinct: BTreeSet<_> = (0..500u32).map(|i| parts.of(Value(6 + 2 * i))).collect();
    assert_eq!(distinct.len(), 500);
    for i in 0..500u32 {
        assert_eq!(parts.of(Value(6 + 2 * i)), parts.of(Value(7 + 2 * i)));
    }
    let part = s.part(&[PointOf::Point(Geometry(3 + 250))]);
    assert_eq!(part.unknowns, [Value(506), Value(507)]);
    assert_eq!(part.constraints.len(), 3);
}

#[test]
fn a_dimensions_value_shared_by_two_rows_joins_nothing() {
    // Two rows reading one fixed value (as no translation sends, but a system may): the
    // value is fixed, so p0 and p1 stay apart. Made an unknown, it joins them.
    let mut s = points(2);
    s.values.push(5.0);
    let v = Value(10);
    for (tag, p) in [(1, 3), (2, 4)] {
        s.constraints.push(SolverConstraint::Distance {
            tag: Tag(tag),
            a: PointOf::Point(Geometry(0)),
            b: PointOf::Point(Geometry(p)),
            value: v,
        });
    }
    let parts = s.parts();
    assert_ne!(parts.of(Value(6)), parts.of(Value(8)));
    s.unknowns.push(v);
    let parts = s.parts();
    assert_eq!(parts.of(Value(6)), parts.of(Value(8)));
    assert_eq!(parts.of(v), parts.of(Value(6)));
}

#[test]
fn a_radius_alone_is_a_part_without_its_centre() {
    // A circle's radius held by a radius row only: PlaneGCS's `addConstraintCircleRadius`
    // reads the radius alone, so the radius is a part without the centre's values.
    let mut s = points(1);
    s.values.push(4.0);
    s.unknowns.push(Value(8));
    s.values.push(4.0);
    s.geometry.push(SketchGeometry::Circle {
        center: pv(6),
        radius: Value(8),
    });
    s.constraints.push(SolverConstraint::Radius {
        tag: Tag(1),
        circle: Geometry(4),
        value: Value(9),
    });
    let parts = s.parts();
    assert!(parts.of(Value(8)).is_some());
    assert_ne!(parts.of(Value(8)), parts.of(Value(6)));
    assert_eq!(parts.of_constraint(&s.constraints[0]), parts.of(Value(8)));
    // Dragging the centre doesn't take the radius along.
    let part = s.part(&[PointOf::Center(Geometry(4))]);
    assert_eq!(part.unknowns, [Value(6), Value(7)]);
    assert!(part.constraints.is_empty());
}

#[test]
fn a_corrupt_system_neither_panics_nor_joins_what_it_cant_read() {
    // Unknowns out of order, repeated, and past the values; geometry naming values past
    // the end; rows naming geometry past the end and points their geometry hasn't.
    let mut s = points(3);
    s.unknowns = vec![Value(9), Value(6), Value(6), Value(500), Value(7)];
    s.geometry.push(SketchGeometry::Line {
        start: pv(1000),
        end: pv(6),
    });
    s.constraints = vec![
        SolverConstraint::Horizontal {
            tag: Tag(0),
            line: Geometry(99),
        },
        SolverConstraint::Distance {
            tag: Tag(u32::MAX),
            a: PointOf::Start(Geometry(3)),
            b: PointOf::Center(Geometry(1)),
            value: Value(u32::MAX),
        },
        SolverConstraint::ArcRules { arc: Geometry(6) },
        SolverConstraint::Vertical {
            tag: Tag(1),
            line: Geometry(6),
        },
    ];
    let parts = s.parts();
    for v in 0..1100 {
        let _ = parts.of(Value(v));
    }
    for c in &s.constraints {
        let _ = parts.of_constraint(c);
    }
    let _ = s.part(&[
        PointOf::Point(Geometry(3)),
        PointOf::End(Geometry(6)),
        PointOf::Center(Geometry(u32::MAX)),
    ]);
    // Empty.
    let empty = SketchSystem {
        values: vec![],
        unknowns: vec![],
        geometry: vec![],
        constraints: vec![],
        algorithms: vec![],
    };
    assert_eq!(empty.parts().of(Value(0)), None);
    assert!(
        empty
            .part(&[PointOf::Point(Geometry(0))])
            .unknowns
            .is_empty()
    );
}

#[test]
fn the_largest_system_splits_into_parts_and_its_drag_part_quickly() {
    // 512 points (1,024 unknowns, the most), then 10,000 rows: a chain joining them all
    // pairwise in the worst order for a union without ranks (each new row's first point
    // the chain's newest), and the rest repeats. And the same rows on 512 parts.
    let mut s = points(512);
    let mut tag = 0u32;
    while s.constraints.len() < 10_000 {
        let i = (s.constraints.len() % 511) as u32;
        tag += 1;
        s.constraints.push(SolverConstraint::HorizontalPoints {
            tag: Tag(tag),
            a: PointOf::Point(Geometry(3 + i + 1)),
            b: PointOf::Point(Geometry(3 + i)),
        });
    }
    let start = Instant::now();
    let parts = s.parts();
    let one = parts.of(Value(6));
    for v in 6..6 + 1024 {
        assert_eq!(parts.of(Value(v)), one);
    }
    for c in &s.constraints {
        assert_eq!(parts.of_constraint(c), one);
    }
    let part = s.part(&[PointOf::Point(Geometry(300))]);
    assert_eq!(part.unknowns.len(), 1024);
    assert_eq!(part.constraints.len(), 10_000);
    let begin = DragBegin {
        system: s.clone(),
        dragged: silico_protocol::Dragged::Point(PointOf::Point(Geometry(3))),
    };
    let e = begin.check().unwrap_err();
    assert!(format!("{e:?}").contains("1024 unknowns"), "{e:?}");
    let took = start.elapsed();
    assert!(took < Duration::from_secs(5), "{took:?}");
}
