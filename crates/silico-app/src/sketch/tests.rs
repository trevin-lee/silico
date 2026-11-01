//! Sketch mode's own tests: what's drawn of hand-made reads (colours by state, construction
//! dashed, glyphs, dimensions placed), hit-testing by hand-worked distances, the selection a
//! frame's clicks leave, and what Delete does. Each expected value is worked out here, from
//! the reads' coordinates and the hand projection (sketch millimetres to screen points: ten
//! points a millimetre, the origin at (100, 500), y up).

use std::collections::BTreeMap;
use std::f64::consts::{FRAC_PI_2, PI};

use eframe::egui::{self, Pos2, pos2};
use silico_core::command::sketch::SketchRead;
use silico_core::document::{
    Angle, Coordinate, Direction, ElementId, ElementKind, Form, Length, OriginPlane, Reference,
    Sketch, SketchArc, SketchCircle, SketchConstraint, SketchLine, SketchPoint,
};
use silico_core::sketch_states::{ConstraintState, MemberState, SketchState, SketchStates};

use super::draw::{
    Drawn, Flat, GlyphKind, MAX_CURVE_SEGMENTS, Scale, Shown, drawing, round_points, segments,
};
use super::hit::{Hit, hit};
use super::*;
use crate::theme::{
    SKETCH_DRIVEN, SKETCH_FULLY_DEFINED, SKETCH_LIGHT, SKETCH_OVER_DEFINED, SKETCH_REDUNDANT,
    SKETCH_SELECTED, SKETCH_UNDER_DEFINED, SketchStyle,
};

fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn element(text: &str) -> Reference {
    Reference::Element(id(text))
}

fn point(name: &str, x: f64, y: f64) -> SketchPoint {
    SketchPoint {
        id: id(name),
        x: Coordinate::new(x).unwrap(),
        y: Coordinate::new(y).unwrap(),
        construction: false,
    }
}

fn line(name: &str, from: &str, to: &str) -> SketchLine {
    SketchLine {
        id: id(name),
        from: id(from),
        to: id(to),
        construction: false,
    }
}

fn constraint(name: &str, form: Form) -> SketchConstraint {
    SketchConstraint { id: id(name), form }
}

fn mm(text: &str) -> Length {
    Length::parse(text).unwrap()
}

/// A 40 × 20 rectangle with its lower left corner at the origin: pa000001 (0, 0), pa000002
/// (40, 0), pa000003 (40, 20), pa000004 (0, 20), and its sides na000001 to na000004 from
/// each corner to the next, counter-clockwise; with `constraints`.
fn rectangle(constraints: Vec<SketchConstraint>) -> Sketch {
    Sketch {
        plane: OriginPlane::Xy.into(),
        points: vec![
            point("pa000001", 0.0, 0.0),
            point("pa000002", 40.0, 0.0),
            point("pa000003", 40.0, 20.0),
            point("pa000004", 0.0, 20.0),
        ],
        lines: vec![
            line("na000001", "pa000001", "pa000002"),
            line("na000002", "pa000002", "pa000003"),
            line("na000003", "pa000003", "pa000004"),
            line("na000004", "pa000004", "pa000001"),
        ],
        circles: Vec::new(),
        arcs: Vec::new(),
        constraints,
    }
}

/// `sketch` read as solved, in state `state`, every point and curve `members`, and each
/// constraint `satisfied` but those `flagged` says.
fn read(
    sketch: Sketch,
    state: SketchState,
    members: MemberState,
    flagged: &[(&str, ConstraintState)],
) -> SketchRead {
    let flagged: BTreeMap<ElementId, ConstraintState> =
        flagged.iter().map(|(name, s)| (id(name), *s)).collect();
    let states = SketchStates {
        state,
        dofs: 0,
        points: sketch
            .points
            .iter()
            .map(|p| (p.id.clone(), members))
            .collect(),
        lines: sketch
            .lines
            .iter()
            .map(|l| (l.id.clone(), members))
            .collect(),
        circles: sketch
            .circles
            .iter()
            .map(|c| (c.id.clone(), members))
            .collect(),
        arcs: sketch
            .arcs
            .iter()
            .map(|a| (a.id.clone(), members))
            .collect(),
        constraints: sketch
            .constraints
            .iter()
            .map(|c| {
                let state = flagged
                    .get(&c.id)
                    .copied()
                    .unwrap_or(ConstraintState::Satisfied);
                (c.id.clone(), state)
            })
            .collect(),
        solve_problem: None,
    };
    SketchRead {
        sketch: silico_core::document::FeatureId::new("s0000001").unwrap(),
        plane: Ok(OriginPlane::Xy.into()),
        members: sketch,
        states,
        regions: Vec::new(),
        profile_problems: Vec::new(),
        profile_problems_total: 0,
    }
}

/// Ten screen points a millimetre, the origin at (100, 500), y up.
fn hand(at: [f64; 2]) -> Option<Pos2> {
    Some(pos2(
        (100.0 + 10.0 * at[0]) as f32,
        (500.0 - 10.0 * at[1]) as f32,
    ))
}

/// [`hand`]'s scale: a tenth of a millimetre a point.
const HAND: Scale = Scale(0.1);

const ALL: Shown = Shown {
    dimensions: true,
    constraints: true,
};

fn drawn(read: &SketchRead) -> Vec<Drawn> {
    drawing(read, &hand, HAND, &SKETCH_LIGHT, ALL, &Default::default())
}

/// What's drawn of the member `name`.
fn of<'a>(drawn: &'a [Drawn], name: &str) -> Vec<&'a Drawn> {
    drawn.iter().filter(|d| d.of().as_str() == name).collect()
}

#[test]
fn an_under_defined_rectangle_draws_blue_each_side_from_corner_to_corner() {
    let read = read(
        rectangle(Vec::new()),
        SketchState::UnderDefined,
        MemberState::UnderDefined,
        &[],
    );
    let drawn = drawn(&read);
    // Four sides and four corners, nothing else.
    assert_eq!(drawn.len(), 8);
    let corners = [
        (100.0, 500.0),
        (500.0, 500.0),
        (500.0, 300.0),
        (100.0, 300.0),
    ];
    for (n, side) in ["na000001", "na000002", "na000003", "na000004"]
        .iter()
        .enumerate()
    {
        let (from, to) = (corners[n], corners[(n + 1) % 4]);
        assert_eq!(
            of(&drawn, side),
            [&Drawn::Stroke {
                of: id(side),
                points: vec![pos2(from.0, from.1), pos2(to.0, to.1)],
                colour: SKETCH_UNDER_DEFINED,
                dashed: false,
                thin: false,
            }],
            "{side}"
        );
    }
    for corner in ["pa000001", "pa000002", "pa000003", "pa000004"] {
        let dots = of(&drawn, corner);
        assert_eq!(dots.len(), 1);
        assert_eq!(dots[0].colour(), SKETCH_UNDER_DEFINED, "{corner}");
    }
}

#[test]
fn a_fully_defined_rectangle_draws_black_and_a_member_over_defined_red() {
    let read = read(
        rectangle(Vec::new()),
        SketchState::FullyDefined,
        MemberState::FullyDefined,
        &[],
    );
    assert!(
        drawn(&read)
            .iter()
            .all(|d| d.colour() == SKETCH_FULLY_DEFINED)
    );
    let mut over = read.clone();
    over.states.state = SketchState::OverDefined;
    over.states
        .lines
        .insert(id("na000002"), MemberState::OverDefined);
    let drawn = drawn(&over);
    assert_eq!(of(&drawn, "na000002")[0].colour(), SKETCH_OVER_DEFINED);
    assert_eq!(of(&drawn, "na000001")[0].colour(), SKETCH_FULLY_DEFINED);
}

#[test]
fn a_sketch_with_no_solution_or_an_invalid_one_colours_every_member_its_way() {
    for (state, colour) in [
        (SketchState::NoSolution, SKETCH_LIGHT.no_solution),
        (SketchState::Invalid, SKETCH_LIGHT.invalid),
    ] {
        let read = read(rectangle(Vec::new()), state, MemberState::FullyDefined, &[]);
        assert!(
            drawn(&read).iter().all(|d| d.colour() == colour),
            "{state:?}"
        );
    }
}

#[test]
fn an_over_defining_dimension_draws_red_a_driven_one_gray_in_parentheses() {
    let read = read(
        rectangle(vec![
            constraint(
                "ka000001",
                Form::Length {
                    line: element("na000001"),
                    value: mm("40"),
                    driving: true,
                },
            ),
            constraint(
                "ka000002",
                Form::Length {
                    line: element("na000003"),
                    value: mm("40"),
                    driving: false,
                },
            ),
            constraint(
                "ka000003",
                Form::Length {
                    line: element("na000002"),
                    value: mm("20"),
                    driving: true,
                },
            ),
        ]),
        SketchState::OverDefined,
        MemberState::FullyDefined,
        &[("ka000001", ConstraintState::Conflicting)],
    );
    let drawn = drawn(&read);
    let over = of(&drawn, "ka000001");
    // Two extension lines, the dimension line, and the text.
    assert_eq!(over.len(), 4);
    assert!(over.iter().all(|d| d.colour() == SKETCH_OVER_DEFINED));
    let driven = of(&drawn, "ka000002");
    assert!(driven.iter().all(|d| d.colour() == SKETCH_DRIVEN));
    let text = |items: &[&Drawn]| {
        items.iter().find_map(|d| match d {
            Drawn::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
    };
    assert_eq!(text(&over).as_deref(), Some("40"));
    assert_eq!(text(&driven).as_deref(), Some("(40)"));
    // A driving dimension, satisfied: as fully defined geometry.
    assert!(
        of(&drawn, "ka000003")
            .iter()
            .all(|d| d.colour() == SKETCH_FULLY_DEFINED)
    );
}

#[test]
fn a_length_dimension_stands_outward_of_the_sketchs_middle_its_text_beyond_its_line() {
    // The bottom side, (0, 0) to (40, 0); the sketch's middle (20, 10) is above it, so the
    // dimension stands below: 24 points (2.4 mm) down, its text 8 points (0.8 mm) further,
    // at (20, −3.2) mm, (300, 532) on the screen.
    let read = read(
        rectangle(vec![constraint(
            "ka000001",
            Form::Length {
                line: element("na000001"),
                value: mm("40"),
                driving: true,
            },
        )]),
        SketchState::FullyDefined,
        MemberState::FullyDefined,
        &[],
    );
    let drawn = drawn(&read);
    let strokes: Vec<Vec<Pos2>> = of(&drawn, "ka000001")
        .into_iter()
        .filter_map(|d| match d {
            Drawn::Stroke { points, thin, .. } => {
                assert!(thin);
                Some(points.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        strokes,
        [
            vec![pos2(100.0, 500.0), pos2(100.0, 524.0)],
            vec![pos2(500.0, 500.0), pos2(500.0, 524.0)],
            vec![pos2(100.0, 524.0), pos2(500.0, 524.0)],
        ]
    );
    let text = of(&drawn, "ka000001")
        .into_iter()
        .find_map(|d| match d {
            Drawn::Text { at, .. } => Some(*at),
            _ => None,
        })
        .unwrap();
    assert!((text - pos2(300.0, 532.0)).length() < 1e-3, "{text:?}");
}

#[test]
fn a_construction_line_is_dashed_and_the_others_are_not() {
    let mut sketch = rectangle(Vec::new());
    sketch.lines[2].construction = true;
    let read = read(
        sketch,
        SketchState::UnderDefined,
        MemberState::UnderDefined,
        &[],
    );
    for d in drawn(&read) {
        if let Drawn::Stroke { of, dashed, .. } = d {
            assert_eq!(dashed, of.as_str() == "na000003", "{of}");
        }
    }
}

#[test]
fn a_redundant_relation_gets_its_own_glyph_its_line_untouched() {
    // Two horizontals on the bottom side, the second redundant, and one on the top.
    let horizontal = |name: &str, side: &str| {
        constraint(
            name,
            Form::Horizontal {
                line: element(side),
            },
        )
    };
    let read = read(
        rectangle(vec![
            horizontal("ka000001", "na000001"),
            horizontal("ka000002", "na000001"),
            horizontal("ka000003", "na000003"),
        ]),
        SketchState::UnderDefined,
        MemberState::UnderDefined,
        &[("ka000002", ConstraintState::Redundant)],
    );
    let drawn = drawn(&read);
    let glyph = |name: &str| match of(&drawn, name).as_slice() {
        [Drawn::Glyph { kind, colour, .. }] => (*kind, *colour),
        other => panic!("{name}: {other:?}"),
    };
    assert_eq!(
        glyph("ka000001"),
        (GlyphKind::Horizontal, SKETCH_LIGHT.glyph)
    );
    assert_eq!(glyph("ka000002"), (GlyphKind::Redundant, SKETCH_REDUNDANT));
    assert_eq!(
        glyph("ka000003"),
        (GlyphKind::Horizontal, SKETCH_LIGHT.glyph)
    );
    assert_eq!(of(&drawn, "na000001")[0].colour(), SKETCH_UNDER_DEFINED);
    // Two glyphs beside the bottom side, a step (16 points) apart along it.
    let at = |name: &str| match of(&drawn, name)[0] {
        Drawn::Glyph { at, .. } => *at,
        _ => unreachable!(),
    };
    // The bottom side runs +x; its left is +y, up the screen: 12 points above its middle.
    assert!((at("ka000001") - pos2(300.0, 488.0)).length() < 1e-3);
    assert!((at("ka000002") - pos2(316.0, 488.0)).length() < 1e-3);
    // Hidden with Show Constraints off; the geometry stays.
    let hidden = drawing(
        &read,
        &hand,
        HAND,
        &SKETCH_LIGHT,
        Shown {
            dimensions: true,
            constraints: false,
        },
        &Default::default(),
    );
    assert!(hidden.iter().all(|d| !matches!(d, Drawn::Glyph { .. })));
    assert_eq!(hidden.len(), 8);
}

#[test]
fn each_relation_draws_its_glyph_beside_each_member_it_names() {
    let read = read(
        rectangle(vec![
            constraint(
                "ka000001",
                Form::Vertical {
                    line: element("na000002"),
                },
            ),
            constraint(
                "ka000002",
                Form::Parallel {
                    a: element("na000001"),
                    b: element("na000003"),
                },
            ),
            constraint(
                "ka000003",
                Form::Perpendicular {
                    a: element("na000001"),
                    b: element("na000002"),
                },
            ),
            constraint(
                "ka000004",
                Form::Equal {
                    a: element("na000002"),
                    b: element("na000004"),
                },
            ),
            constraint(
                "ka000005",
                Form::Symmetric {
                    a: element("pa000001"),
                    b: element("pa000003"),
                    about: element("pa000002"),
                },
            ),
            constraint(
                "ka000006",
                Form::HorizontalPoints {
                    a: element("pa000001"),
                    b: Reference::Origin,
                },
            ),
            constraint(
                "ka000007",
                Form::On {
                    point: element("pa000004"),
                    curve: Reference::YAxis,
                },
            ),
        ]),
        SketchState::UnderDefined,
        MemberState::UnderDefined,
        &[],
    );
    let drawn = drawn(&read);
    let kinds = |name: &str| -> Vec<GlyphKind> {
        of(&drawn, name)
            .into_iter()
            .map(|d| match d {
                Drawn::Glyph { kind, .. } => *kind,
                other => panic!("{other:?}"),
            })
            .collect()
    };
    assert_eq!(kinds("ka000001"), [GlyphKind::Vertical]);
    assert_eq!(kinds("ka000002"), [GlyphKind::Parallel; 2]);
    assert_eq!(kinds("ka000003"), [GlyphKind::Perpendicular; 2]);
    assert_eq!(kinds("ka000004"), [GlyphKind::Equal; 2]);
    assert_eq!(kinds("ka000005"), [GlyphKind::Symmetric; 2]);
    // The origin has no glyph of its own: one, beside the point.
    assert_eq!(kinds("ka000006"), [GlyphKind::Horizontal]);
    assert_eq!(kinds("ka000007"), [GlyphKind::On]);
}

#[test]
fn a_quarter_arcs_polyline_is_eight_chords_each_within_the_tolerance_of_the_arc() {
    // Radius 10 mm, 100 points on the screen; tolerance 0.5: each chord's angle at most
    // 2·acos(1 − 0.005) = 0.20008 rad, so π/2 needs ⌈7.85⌉ = 8. Eight chords of π/16 stray
    // 100·(1 − cos π/32) = 0.4815 points; seven, 100·(1 − cos π/28) = 0.6288, too far.
    assert_eq!(segments(100.0, FRAC_PI_2, 0.5), 8);
    assert!(100.0 * (1.0 - libm::cos(PI / 32.0)) <= 0.5);
    assert!(100.0 * (1.0 - libm::cos(PI / 28.0)) > 0.5);
    let sketch = Sketch {
        plane: OriginPlane::Xy.into(),
        points: vec![
            point("pa000001", 0.0, 0.0),
            point("pa000002", 10.0, 0.0),
            point("pa000003", 0.0, 10.0),
        ],
        lines: Vec::new(),
        circles: Vec::new(),
        arcs: vec![SketchArc {
            id: id("aa000001"),
            center: id("pa000001"),
            start: id("pa000002"),
            end: id("pa000003"),
            construction: false,
        }],
        constraints: Vec::new(),
    };
    let read = read(
        sketch,
        SketchState::UnderDefined,
        MemberState::UnderDefined,
        &[],
    );
    let style = SketchStyle {
        chord_tolerance: 0.5,
        ..SKETCH_LIGHT
    };
    let drawn = drawing(&read, &hand, HAND, &style, ALL, &Default::default());
    let [Drawn::Stroke { points, .. }] = of(&drawn, "aa000001").as_slice() else {
        panic!("{drawn:?}");
    };
    assert_eq!(points.len(), 9);
    let centre = pos2(100.0, 500.0);
    assert!((points[0] - pos2(200.0, 500.0)).length() < 1e-3);
    assert!((points[8] - pos2(100.0, 400.0)).length() < 1e-3);
    for p in points {
        assert!(((*p - centre).length() - 100.0).abs() < 1e-3, "{p:?}");
    }
    for pair in points.windows(2) {
        let chord_middle = pair[0] + (pair[1] - pair[0]) / 2.0;
        let strays = 100.0 - (chord_middle - centre).length();
        assert!(strays <= 0.5 && strays > 0.45, "{strays}");
    }
}

#[test]
fn a_circle_is_closed_its_segments_set_by_the_projections_largest_stretch() {
    // A projection three times as tall as wide: the circle of radius 10 stretches to 300
    // points up and down, 100 across; its chords are counted for 300.
    let tall = |at: [f64; 2]| Some(pos2((10.0 * at[0]) as f32, (-30.0 * at[1]) as f32));
    let points = round_points([0.0, 0.0], 10.0, 0.0, std::f64::consts::TAU, &tall, 0.5);
    assert_eq!(
        points.len(),
        segments(300.0, std::f64::consts::TAU, 0.5) + 1
    );
    assert_eq!(points.first(), points.last());
}

#[test]
fn hit_takes_a_point_before_a_curve_and_the_nearest_curve_before_a_farther_one() {
    let read = read(
        rectangle(Vec::new()),
        SketchState::UnderDefined,
        MemberState::UnderDefined,
        &[],
    );
    let found = |at: [f64; 2], tolerance: f64| hit(&read, at, tolerance, HAND, &SKETCH_LIGHT);
    let hit_of = |name: &str, kind: ElementKind| Some(Hit { id: id(name), kind });
    // (1, 0.5): the corner pa000001 is √1.25 = 1.118 off, past 1; the bottom side 0.5,
    // the left side 1.0: the bottom.
    assert_eq!(
        found([1.0, 0.5], 1.0),
        hit_of("na000001", ElementKind::Line)
    );
    // (0.3, 0.3): the corner 0.424 off, a side 0.3: the point first.
    assert_eq!(
        found([0.3, 0.3], 1.0),
        hit_of("pa000001", ElementKind::Point)
    );
    // (0.5, 0.5) within 0.6: the corner is 0.707 off; both sides 0.5: the lower ID.
    assert_eq!(
        found([0.5, 0.5], 0.6),
        hit_of("na000001", ElementKind::Line)
    );
    // The middle, 10 from every side: nothing.
    assert_eq!(found([20.0, 10.0], 1.0), None);
    // Nothing for a negative tolerance, or a place that isn't one.
    assert_eq!(found([0.0, 0.0], -1.0), None);
    assert_eq!(found([f64::NAN, 0.0], 1.0), None);
    assert_eq!(found([0.0, 0.0], f64::NAN), None);
}

#[test]
fn hit_ties_between_points_go_to_the_lower_id() {
    let mut sketch = rectangle(Vec::new());
    sketch.lines.clear();
    sketch.points = vec![point("pb000002", 2.0, 0.0), point("pb000001", 0.0, 0.0)];
    let read = read(
        sketch,
        SketchState::UnderDefined,
        MemberState::UnderDefined,
        &[],
    );
    assert_eq!(
        hit(&read, [1.0, 0.0], 1.5, HAND, &SKETCH_LIGHT),
        Some(Hit {
            id: id("pb000001"),
            kind: ElementKind::Point
        })
    );
}

#[test]
fn hit_finds_a_glyph_and_a_dimension_where_theyre_drawn() {
    // The top side's horizontal: the side runs from (40, 20) to (0, 20), −x, so its left is
    // −y: the glyph 12 points (1.2 mm) below its middle, at (20, 18.8). The bottom side's
    // length, its text at (20, −3.2).
    let read = read(
        rectangle(vec![
            constraint(
                "ka000001",
                Form::Horizontal {
                    line: element("na000003"),
                },
            ),
            constraint(
                "ka000002",
                Form::Length {
                    line: element("na000001"),
                    value: mm("40"),
                    driving: true,
                },
            ),
        ]),
        SketchState::FullyDefined,
        MemberState::FullyDefined,
        &[],
    );
    let found = |at: [f64; 2]| hit(&read, at, 0.1, HAND, &SKETCH_LIGHT).map(|h| h.id);
    assert_eq!(found([20.0, 18.8]), Some(id("ka000001")));
    assert_eq!(found([20.0, -3.2]), Some(id("ka000002")));
    // On its dimension line, (10, −2.4).
    assert_eq!(found([10.0, -2.4]), Some(id("ka000002")));
    assert_eq!(found([20.0, 15.0]), None);
}

#[test]
fn an_arc_is_hit_on_its_sweep_and_past_it_only_near_its_ends() {
    let sketch = Sketch {
        plane: OriginPlane::Xy.into(),
        points: vec![
            point("pa000001", 0.0, 0.0),
            point("pa000002", 10.0, 0.0),
            point("pa000003", 0.0, 10.0),
        ],
        lines: Vec::new(),
        circles: Vec::new(),
        arcs: vec![SketchArc {
            id: id("aa000001"),
            center: id("pa000001"),
            start: id("pa000002"),
            end: id("pa000003"),
            construction: false,
        }],
        constraints: Vec::new(),
    };
    let read = read(
        sketch,
        SketchState::UnderDefined,
        MemberState::UnderDefined,
        &[],
    );
    let found = |at: [f64; 2]| hit(&read, at, 0.5, HAND, &SKETCH_LIGHT).map(|h| h.kind);
    // On the sweep, at 45°, 0.2 outside the rim.
    let r = 10.2 * std::f64::consts::FRAC_1_SQRT_2;
    assert_eq!(found([r, r]), Some(ElementKind::Arc));
    // At 225°, on the circle but off the sweep, far from either end.
    assert_eq!(found([-r, -r]), None);
}

#[test]
fn an_empty_sketch_draws_nothing_and_hits_nothing() {
    let empty = Sketch {
        plane: OriginPlane::Xy.into(),
        points: Vec::new(),
        lines: Vec::new(),
        circles: Vec::new(),
        arcs: Vec::new(),
        constraints: Vec::new(),
    };
    let read = read(
        empty,
        SketchState::FullyDefined,
        MemberState::FullyDefined,
        &[],
    );
    assert!(drawn(&read).is_empty());
    assert_eq!(hit(&read, [0.0, 0.0], 100.0, HAND, &SKETCH_LIGHT), None);
}

#[test]
fn places_the_projection_cant_show_break_a_curve_and_nothing_drawn_isnt_a_number() {
    // Everything left of x = 20 behind the camera, and x = 30 a NaN.
    let half = |at: [f64; 2]| {
        if at[0] < 20.0 {
            None
        } else if at[0] == 30.0 {
            Some(pos2(f32::NAN, 0.0))
        } else {
            hand(at)
        }
    };
    let mut sketch = rectangle(Vec::new());
    sketch.points.push(point("pa000005", 30.0, 5.0));
    sketch.points.push(point("pa000006", 25.0, 5.0));
    sketch.circles.push(SketchCircle {
        id: id("ca000001"),
        center: id("pa000006"),
        radius: Coordinate::new(10.0).unwrap(),
        construction: false,
    });
    let read = read(
        sketch,
        SketchState::UnderDefined,
        MemberState::UnderDefined,
        &[],
    );
    let drawn = drawing(&read, &half, HAND, &SKETCH_LIGHT, ALL, &Default::default());
    for d in &drawn {
        for p in d.positions() {
            assert!(p.x.is_finite() && p.y.is_finite(), "{d:?}");
        }
        if let Drawn::Stroke { points, .. } = d {
            assert!(points.len() >= 2);
        }
    }
    // Only the right side (x = 40) is whole; the bottom and top lose an end; the circle
    // keeps its right part, unbroken.
    let sides: Vec<&str> = drawn
        .iter()
        .filter_map(|d| match d {
            Drawn::Stroke { of, .. } if of.as_str().starts_with("na") => Some(of.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(sides, ["na000002"]);
    assert!(!of(&drawn, "ca000001").is_empty());
    assert!(of(&drawn, "pa000005").is_empty());
}

#[test]
fn huge_coordinates_draw_finite_and_a_huge_circle_takes_at_most_the_cap() {
    // A circle of 1 km radius at the reach, viewed at a thousand points a millimetre: a
    // billion points across, capped.
    let sketch = Sketch {
        plane: OriginPlane::Xy.into(),
        points: vec![point("pa000001", 1e6, -1e6), point("pa000002", -1e6, 1e6)],
        lines: vec![line("na000001", "pa000001", "pa000002")],
        circles: vec![SketchCircle {
            id: id("ca000001"),
            center: id("pa000001"),
            radius: Coordinate::new(1e6).unwrap(),
            construction: false,
        }],
        arcs: Vec::new(),
        constraints: vec![constraint(
            "ka000001",
            Form::Diameter {
                curve: element("ca000001"),
                value: mm("2000000"),
                driving: true,
            },
        )],
    };
    let read = read(
        sketch,
        SketchState::UnderDefined,
        MemberState::UnderDefined,
        &[],
    );
    let near = |at: [f64; 2]| Some(pos2((1000.0 * at[0]) as f32, (-1000.0 * at[1]) as f32));
    let drawn = drawing(
        &read,
        &near,
        Scale(0.001),
        &SKETCH_LIGHT,
        ALL,
        &Default::default(),
    );
    let [Drawn::Stroke { points, .. }] = of(&drawn, "ca000001").as_slice() else {
        panic!("{drawn:?}");
    };
    assert_eq!(points.len(), MAX_CURVE_SEGMENTS + 1);
    for d in &drawn {
        assert!(
            d.positions()
                .iter()
                .all(|p| p.x.is_finite() && p.y.is_finite())
        );
    }
    assert!(hit(&read, [1e6, 0.0], 1.0, Scale(0.001), &SKETCH_LIGHT).is_some());
}

#[test]
fn degenerate_members_draw_without_a_number_that_isnt_one() {
    // A zero-length line, a circle of radius 0, and an arc whose start and end are one.
    let sketch = Sketch {
        plane: OriginPlane::Xy.into(),
        points: vec![point("pa000001", 5.0, 5.0), point("pa000002", 6.0, 5.0)],
        lines: vec![line("na000001", "pa000001", "pa000001")],
        circles: vec![SketchCircle {
            id: id("ca000001"),
            center: id("pa000001"),
            radius: Coordinate::new(0.0).unwrap(),
            construction: false,
        }],
        arcs: vec![SketchArc {
            id: id("aa000001"),
            center: id("pa000001"),
            start: id("pa000002"),
            end: id("pa000002"),
            construction: false,
        }],
        constraints: vec![
            constraint(
                "ka000001",
                Form::Length {
                    line: element("na000001"),
                    value: mm("0"),
                    driving: true,
                },
            ),
            constraint(
                "ka000002",
                Form::Radius {
                    curve: element("ca000001"),
                    value: mm("0"),
                    driving: true,
                },
            ),
            constraint(
                "ka000003",
                Form::Angle {
                    a: element("na000001"),
                    b: Reference::XAxis,
                    reversed: false,
                    value: Angle::parse("30").unwrap(),
                    driving: true,
                },
            ),
            constraint(
                "ka000004",
                Form::Horizontal {
                    line: element("na000001"),
                },
            ),
        ],
    };
    let read = read(sketch, SketchState::Invalid, MemberState::UnderDefined, &[]);
    let drawn = drawn(&read);
    for d in &drawn {
        assert!(
            d.positions()
                .iter()
                .all(|p| p.x.is_finite() && p.y.is_finite()),
            "{d:?}"
        );
    }
    assert!(hit(&read, [5.0, 5.0], 0.1, HAND, &SKETCH_LIGHT).is_some());
}

#[test]
fn thousands_of_members_draw_and_hit_within_a_frames_budget() {
    // 4,000 lines in chains, 1,000 circles, and 2,000 relations and dimensions: what's
    // drawn is bounded by the members (each circle at most the cap), and the time is said.
    let mut points = Vec::new();
    let mut lines = Vec::new();
    let mut circles = Vec::new();
    let mut constraints = Vec::new();
    let name = |prefix: &str, n: usize| format!("{prefix}{n:07}");
    for n in 0..4001 {
        points.push(point(&name("p", n), (n % 100) as f64, (n / 100) as f64));
    }
    for n in 0..4000 {
        lines.push(line(&name("n", n), &name("p", n), &name("p", n + 1)));
    }
    for n in 0..1000 {
        circles.push(SketchCircle {
            id: id(&name("c", n)),
            center: id(&name("p", n)),
            radius: Coordinate::new(0.3).unwrap(),
            construction: false,
        });
    }
    for n in 0..1000 {
        constraints.push(constraint(
            &name("k", 2 * n),
            Form::Horizontal {
                line: element(&name("n", n)),
            },
        ));
        constraints.push(constraint(
            &name("k", 2 * n + 1),
            Form::Length {
                line: element(&name("n", n + 1000)),
                value: mm("1"),
                driving: true,
            },
        ));
    }
    let sketch = Sketch {
        plane: OriginPlane::Xy.into(),
        points,
        lines,
        circles,
        arcs: Vec::new(),
        constraints,
    };
    let read = read(
        sketch,
        SketchState::UnderDefined,
        MemberState::UnderDefined,
        &[],
    );
    let started = std::time::Instant::now();
    let drawn = drawn(&read);
    let drawing_took = started.elapsed();
    let started = std::time::Instant::now();
    let found = hit(&read, [50.5, 20.5], 0.6, HAND, &SKETCH_LIGHT);
    let hit_took = started.elapsed();
    // 4,000 sides, 1,000 circles, 4,001 dots, 1,000 glyphs, and 1,000 dimensions of four.
    assert_eq!(drawn.len(), 4000 + 1000 + 4001 + 1000 + 4000);
    let vertices: usize = drawn.iter().map(|d| d.positions().len()).sum();
    assert!(vertices < 200_000, "{vertices}");
    assert!(found.is_some());
    // Measured on a laptop in a debug build (M5.11a's report); no bound here, as nothing
    // here holds to a clock.
    let _ = (drawing_took, hit_took);
}

#[test]
fn a_flat_sketch_fills_its_rectangle_but_the_margin_and_maps_back() {
    let read = read(
        rectangle(Vec::new()),
        SketchState::UnderDefined,
        MemberState::UnderDefined,
        &[],
    );
    let rect = egui::Rect::from_min_size(pos2(10.0, 20.0), egui::vec2(500.0, 300.0));
    let flat = Flat::fit(rect, &read);
    // Bounds (0, 0) to (40, 20), the origin among them; the shorter side, 300, less a tenth
    // each side, 240 for the 40 mm span: 6 points a millimetre, centred at (260, 170).
    assert_eq!(flat.project([20.0, 10.0]), Some(pos2(260.0, 170.0)));
    assert_eq!(flat.project([0.0, 0.0]), Some(pos2(140.0, 230.0)));
    assert_eq!(flat.to_plane(pos2(140.0, 230.0)), Some([0.0, 0.0]));
    let project = |at| flat.project(at);
    let drawn = drawing(
        &read,
        &project,
        Scale(1.0 / 6.0),
        &SKETCH_LIGHT,
        ALL,
        &Default::default(),
    );
    for d in &drawn {
        assert!(d.positions().iter().all(|p| rect.contains(*p)), "{d:?}");
    }
}

/// `f` with a painter, in one pass of a context with the app's fonts.
fn with_painter(f: impl FnOnce(&egui::Painter)) {
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::theme::fonts());
    let mut f = Some(f);
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        if let Some(f) = f.take() {
            f(&ui.painter().clone());
        }
    });
    // Nothing draws them: let go of the font atlas's upload.
    output.textures_delta.clear();
}

/// [`hand`]'s way back: screen points to the sketch's millimetres.
fn unhand(at: Pos2) -> Option<[f64; 2]> {
    Some([
        (f64::from(at.x) - 100.0) / 10.0,
        (500.0 - f64::from(at.y)) / 10.0,
    ])
}

fn press(at: Pos2, ctrl: bool) -> PointerEvent {
    PointerEvent {
        action: PointerAction::Press,
        at,
        shift: false,
        ctrl,
    }
}

#[test]
fn a_frames_presses_select_ctrl_adds_and_a_press_on_nothing_clears() {
    let mut mode = SketchMode::new(1, None);
    mode.read = Some(Read {
        model: None,
        result: Ok(read(
            rectangle(Vec::new()),
            SketchState::UnderDefined,
            MemberState::UnderDefined,
            &[],
        )),
    });
    let frame = |mode: &mut SketchMode, events: &[PointerEvent], pointer: Option<Pos2>| {
        with_painter(|painter| {
            let input = FrameInput {
                events,
                pointer,
                project: &hand,
                to_plane: &unhand,
            };
            mode.frame(painter, &input, &SKETCH_LIGHT);
        });
    };
    let selected = |mode: &SketchMode| -> Vec<String> {
        mode.selected_ids()
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect()
    };
    // The corner (0, 0) is at (100, 500); a press 2 points off is within the 6-point pick.
    frame(
        &mut mode,
        &[press(pos2(102.0, 500.0), false)],
        Some(pos2(250.0, 450.0)),
    );
    assert_eq!(selected(&mode), ["pa000001"]);
    // The pointer at (250, 450) is (15, 5) on the plane.
    assert_eq!(mode.pointer, Some([15.0, 5.0]));
    // Ctrl on the top side's middle, (20, 20): (300, 300), 3 points off it.
    frame(&mut mode, &[press(pos2(300.0, 303.0), true)], None);
    assert_eq!(selected(&mode), ["na000003", "pa000001"]);
    assert_eq!(mode.pointer, None);
    // Ctrl on the corner again takes it away; a plain press on the top side replaces all.
    frame(&mut mode, &[press(pos2(100.0, 500.0), true)], None);
    assert_eq!(selected(&mode), ["na000003"]);
    frame(&mut mode, &[press(pos2(500.0, 400.0), false)], None);
    assert_eq!(selected(&mode), ["na000002"]);
    // A press on nothing, 7 points off every side, clears it; a move selects nothing.
    frame(&mut mode, &[press(pos2(300.0, 400.0), false)], None);
    assert!(mode.selection.is_empty());
    let moved = PointerEvent {
        action: PointerAction::Move,
        ..press(pos2(100.0, 500.0), false)
    };
    frame(&mut mode, &[moved], None);
    assert!(mode.selection.is_empty());
}

#[test]
fn delete_joins_a_lone_point_two_curves_end_at_and_removes_anything_else() {
    let mut mode = SketchMode::new(1, None);
    let mut sketch = rectangle(Vec::new());
    sketch.lines.pop();
    mode.read = Some(Read {
        model: None,
        result: Ok(read(
            sketch,
            SketchState::UnderDefined,
            MemberState::UnderDefined,
            &[],
        )),
    });
    let hit = |name: &str, kind| Hit { id: id(name), kind };
    assert_eq!(mode.deletion(), None);
    // pa000002: na000001 ends there, na000002 starts there.
    mode.click(Some(hit("pa000002", ElementKind::Point)), false);
    assert_eq!(mode.deletion(), Some(Deletion::Join(id("pa000002"))));
    // pa000001, its one side (na000004) gone: removed.
    mode.click(Some(hit("pa000001", ElementKind::Point)), false);
    assert_eq!(
        mode.deletion(),
        Some(Deletion::Remove(vec![element("pa000001")]))
    );
    // A point and a line: removed together, by ID.
    mode.click(Some(hit("na000001", ElementKind::Line)), true);
    assert_eq!(
        mode.deletion(),
        Some(Deletion::Remove(vec![
            element("na000001"),
            element("pa000001")
        ]))
    );
}

#[test]
fn a_new_read_lets_go_of_selected_members_it_no_longer_holds() {
    let mut mode = SketchMode::new(1, None);
    mode.click(
        Some(Hit {
            id: id("na000002"),
            kind: ElementKind::Line,
        }),
        false,
    );
    mode.click(
        Some(Hit {
            id: id("nz000009"),
            kind: ElementKind::Line,
        }),
        true,
    );
    mode.took(Read {
        model: None,
        result: Ok(read(
            rectangle(Vec::new()),
            SketchState::UnderDefined,
            MemberState::UnderDefined,
            &[],
        )),
    });
    let kept: Vec<&str> = mode.selection.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(kept, ["na000002"]);
}

#[test]
fn the_palette_says_a_refused_read_of_the_tabs_revision_and_waits_on_an_older_ones() {
    let r = |n| silico_core::command::Revision::from_number(n).unwrap();
    let refused = Problems::from(silico_core::Error::new(
        silico_core::ErrorCode::InvalidInput,
        "the document has no feature s0000001",
    ));
    let mut mode = SketchMode::new(1, None);
    mode.read = Some(Read {
        model: Some(r(2)),
        result: Err(refused.clone()),
    });
    assert_eq!(mode.palette_view(r(3)).said, None);
    assert_eq!(
        mode.palette_view(r(2)).said.as_deref(),
        Some("the document has no feature s0000001")
    );
    assert_eq!(mode.palette_view(r(2)).status, None);
}

#[test]
fn the_words_and_prefixes_are_solidworks() {
    let words: Vec<(&str, Option<&str>)> = SketchState::ALL
        .iter()
        .map(|&s| (state_words(s), prefix(s)))
        .collect();
    assert_eq!(
        words,
        [
            ("Under Defined", Some("(–)")),
            ("Fully Defined", None),
            ("Over Defined", Some("(+)")),
            ("No Solution Found", Some("(?)")),
            ("Invalid Solution Found", Some("(?)")),
        ]
    );
}

#[test]
fn a_selected_member_draws_in_the_selections_colour() {
    let read = read(
        rectangle(vec![constraint(
            "ka000001",
            Form::Horizontal {
                line: element("na000001"),
            },
        )]),
        SketchState::UnderDefined,
        MemberState::UnderDefined,
        &[],
    );
    let selected = [id("na000001"), id("ka000001")].into_iter().collect();
    let drawn = drawing(&read, &hand, HAND, &SKETCH_LIGHT, ALL, &selected);
    assert_eq!(of(&drawn, "na000001")[0].colour(), SKETCH_SELECTED);
    assert_eq!(of(&drawn, "ka000001")[0].colour(), SKETCH_SELECTED);
    assert_eq!(of(&drawn, "na000002")[0].colour(), SKETCH_UNDER_DEFINED);
}

#[test]
fn a_tangency_draws_at_its_point_and_an_angle_its_value_in_degrees() {
    let read = read(
        rectangle(vec![
            constraint(
                "ka000001",
                Form::TangentAt {
                    a: element("na000001"),
                    b: element("na000002"),
                    at: element("pa000002"),
                    direction: Direction::Same,
                },
            ),
            constraint(
                "ka000002",
                Form::Angle {
                    a: element("na000001"),
                    b: element("na000002"),
                    reversed: false,
                    value: Angle::parse("90").unwrap(),
                    driving: true,
                },
            ),
        ]),
        SketchState::UnderDefined,
        MemberState::UnderDefined,
        &[],
    );
    let drawn = drawn(&read);
    // Beside pa000002, (40, 0): up and right by 1.2 mm, (41.2, 1.2), (512, 488).
    match of(&drawn, "ka000001").as_slice() {
        [Drawn::Glyph { kind, at, .. }] => {
            assert_eq!(*kind, GlyphKind::Tangent);
            assert!((*at - pos2(512.0, 488.0)).length() < 1e-3);
        }
        other => panic!("{other:?}"),
    }
    // The sides meet at (40, 0); the arc from +x to +y, its text at 45°, 3.2 mm out.
    let text = of(&drawn, "ka000002")
        .into_iter()
        .find_map(|d| match d {
            Drawn::Text { text, at, .. } => Some((text.clone(), *at)),
            _ => None,
        })
        .unwrap();
    assert_eq!(text.0, "90°");
    let out = 3.2 * std::f64::consts::FRAC_1_SQRT_2;
    let want = hand([40.0 + out, out]).unwrap();
    assert!((text.1 - want).length() < 1e-3, "{text:?}");
}
