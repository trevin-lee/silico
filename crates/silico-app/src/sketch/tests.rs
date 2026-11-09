//! Sketch mode's own tests: what's drawn of hand-made reads (colours by state, construction
//! dashed, glyphs and dimensions placed as the prototype places them, the profile's fill,
//! the grid, a tool's preview, the selection), hit-testing by hand-worked distances, the
//! selection a frame's clicks leave, what Delete does, and what the palette and the status
//! bar say. Each expected value is worked out here, from the reads' coordinates, the hand
//! projection (sketch millimetres to screen points: ten points a millimetre, the origin at
//! (100, 500), y up), and `style`'s sizes in points (a tenth of a millimetre each here).

use std::collections::BTreeMap;
use std::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_2, PI, TAU};

use eframe::egui::{self, Pos2, pos2, vec2};
use silico_core::command::sketch::{Region, SketchRead};
use silico_core::document::{
    Angle, Coordinate, Direction, ElementId, ElementKind, Form, Length, LineSide, OriginPlane,
    Reference, Sketch, SketchArc, SketchCircle, SketchConstraint, SketchLine, SketchPoint,
};
use silico_core::sketch_states::{ConstraintState, MemberState, SketchState, SketchStates};
use silico_core::sketch_tools::{MeasureKind, Preview, PreviewMeasure, Relation, Shown as Showing};

use super::draw::{
    Drawn, Flat, GlyphKind, LabelKind, MAX_CURVE_SEGMENTS, Scale, Shown, StrokeKind, View, drawing,
    preview, region_fills, round_points, segments, tessellate,
};
use super::draw::{add, direction, mul};
use super::hit::{Hit, hit};
use super::style::*;
use super::*;

pub(crate) fn id(text: &str) -> ElementId {
    ElementId::new(text).unwrap()
}

fn element(text: &str) -> Reference {
    Reference::Element(id(text))
}

pub(crate) fn point(name: &str, x: f64, y: f64) -> SketchPoint {
    SketchPoint {
        id: id(name),
        x: Coordinate::new(x).unwrap(),
        y: Coordinate::new(y).unwrap(),
        construction: false,
    }
}

pub(crate) fn line(name: &str, from: &str, to: &str) -> SketchLine {
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

pub(crate) fn under(sketch: Sketch) -> SketchRead {
    read(
        sketch,
        SketchState::UnderDefined,
        MemberState::UnderDefined,
        &[],
    )
}

/// Ten screen points a millimetre, the origin at (100, 500), y up.
pub(crate) fn hand(at: [f64; 2]) -> Option<Pos2> {
    Some(pos2(
        (100.0 + 10.0 * at[0]) as f32,
        (500.0 - 10.0 * at[1]) as f32,
    ))
}

/// [`hand`]'s scale: a tenth of a millimetre a point.
const HAND: Scale = Scale(0.1);

/// The hand view, with no grid.
fn view() -> View<'static> {
    View {
        project: &hand,
        scale: HAND,
        visible: None,
        chord_tolerance: SKETCH_CHORD_TOLERANCE,
    }
}

fn drawn(read: &SketchRead) -> Vec<Drawn> {
    drawing(
        read,
        &view(),
        Shown::default(),
        &Default::default(),
        &region_fills(read, HAND, SKETCH_CHORD_TOLERANCE),
    )
}

/// What's drawn of the member `name`.
fn of<'a>(drawn: &'a [Drawn], name: &str) -> Vec<&'a Drawn> {
    drawn
        .iter()
        .filter(|d| d.of().is_some_and(|of| of.as_str() == name))
        .collect()
}

fn near(a: Pos2, b: Pos2) -> bool {
    (a - b).length() < 1e-3
}

#[test]
fn an_under_defined_rectangle_draws_blue_each_side_from_corner_to_corner_and_the_origin() {
    let read = under(rectangle(Vec::new()));
    let drawn = drawn(&read);
    // The origin, four sides, and four corners; no grid without a view of the plane.
    assert_eq!(drawn.len(), 9);
    assert_eq!(
        drawn[0],
        Drawn::Origin {
            at: pos2(100.0, 500.0)
        }
    );
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
                of: Some(id(side)),
                kind: StrokeKind::Curve { dashed: false },
                points: vec![pos2(from.0, from.1), pos2(to.0, to.1)],
                colour: SKETCH_UNDER_DEFINED,
                selected: false,
            }],
            "{side}"
        );
    }
    for corner in ["pa000001", "pa000002", "pa000003", "pa000004"] {
        let points = of(&drawn, corner);
        assert!(
            matches!(points.as_slice(), [Drawn::Point { colour, .. }] if *colour == SKETCH_UNDER_DEFINED),
            "{corner}"
        );
    }
    // Show Points off: the corners go, the sides stay.
    let hidden = drawing(
        &read,
        &view(),
        Shown {
            points: false,
            ..Shown::default()
        },
        &Default::default(),
        &[],
    );
    assert_eq!(hidden.len(), 5);
}

#[test]
fn a_fully_defined_rectangle_draws_in_its_colour_and_a_member_over_defined_in_red() {
    let read = read(
        rectangle(Vec::new()),
        SketchState::FullyDefined,
        MemberState::FullyDefined,
        &[],
    );
    let members = |drawn: &[Drawn]| -> Vec<Color> {
        drawn
            .iter()
            .filter(|d| d.of().is_some())
            .map(|d| Color(d.colour()))
            .collect()
    };
    assert!(
        members(&drawn(&read))
            .iter()
            .all(|c| c.0 == SKETCH_FULLY_DEFINED)
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

/// A colour, compared.
#[derive(Debug, PartialEq)]
struct Color(egui::Color32);

#[test]
fn a_sketch_with_no_solution_or_an_invalid_one_colours_every_member_its_way() {
    for (state, colour) in [
        (SketchState::NoSolution, SKETCH_OVER_DEFINED),
        (SketchState::Invalid, SKETCH_INVALID),
    ] {
        let read = read(rectangle(Vec::new()), state, MemberState::FullyDefined, &[]);
        for d in drawn(&read).iter().filter(|d| d.of().is_some()) {
            assert_eq!(d.colour(), colour, "{state:?}: {d:?}");
        }
    }
}

fn length(name: &str, side: &str, value: &str, driving: bool) -> SketchConstraint {
    constraint(
        name,
        Form::Length {
            line: element(side),
            value: mm(value),
            driving,
        },
    )
}

/// The text a dimension's label says, and where.
fn label_of(drawn: &[&Drawn]) -> Option<(String, Pos2)> {
    drawn.iter().find_map(|d| match d {
        Drawn::Label { text, at, kind, .. } => {
            assert_eq!(*kind, LabelKind::Dimension);
            Some((text.clone(), *at))
        }
        _ => None,
    })
}

#[test]
fn an_over_defining_dimension_draws_red_a_driven_one_gray_in_parentheses() {
    let read = read(
        rectangle(vec![
            length("ka000001", "na000001", "40", true),
            length("ka000002", "na000003", "40", false),
            length("ka000003", "na000002", "20", true),
        ]),
        SketchState::OverDefined,
        MemberState::FullyDefined,
        &[("ka000001", ConstraintState::Conflicting)],
    );
    let drawn = drawn(&read);
    let over = of(&drawn, "ka000001");
    // Two extension lines, the dimension line, two arrowheads, and the value.
    assert_eq!(over.len(), 6);
    assert!(over.iter().all(|d| d.colour() == SKETCH_OVER_DEFINED));
    let driven = of(&drawn, "ka000002");
    assert!(driven.iter().all(|d| d.colour() == SKETCH_DRIVEN));
    assert_eq!(label_of(&over).unwrap().0, "40");
    assert_eq!(label_of(&driven).unwrap().0, "(40)");
    // A driving dimension, satisfied: in the dimensions' colour.
    assert!(
        of(&drawn, "ka000003")
            .iter()
            .all(|d| d.colour() == SKETCH_DIMENSION_TEXT)
    );
}

#[test]
fn a_length_dimension_stands_outward_of_the_sketchs_middle_with_arrows_and_its_value_on_its_line() {
    // The bottom side, (0, 0) to (40, 0); the sketch's middle (20, 10) is above it, so the
    // dimension stands below: its line 26 points (2.6 mm) down, its extension lines from 4
    // points off the side to 5 past the line, an arrowhead at each end pointing out, and its
    // value at the line's middle: (300, 526) on the screen.
    let read = read(
        rectangle(vec![length("ka000001", "na000001", "40", true)]),
        SketchState::FullyDefined,
        MemberState::FullyDefined,
        &[],
    );
    let drawn = drawn(&read);
    let items = of(&drawn, "ka000001");
    let strokes: Vec<Vec<Pos2>> = items
        .iter()
        .filter_map(|d| match d {
            Drawn::Stroke { points, kind, .. } => {
                assert_eq!(*kind, StrokeKind::Dimension);
                Some(points.clone())
            }
            _ => None,
        })
        .collect();
    let close =
        |a: &[Pos2], b: &[Pos2]| a.len() == b.len() && a.iter().zip(b).all(|(p, q)| near(*p, *q));
    let want = [
        vec![pos2(100.0, 504.0), pos2(100.0, 531.0)],
        vec![pos2(500.0, 504.0), pos2(500.0, 531.0)],
        vec![pos2(100.0, 526.0), pos2(500.0, 526.0)],
    ];
    assert_eq!(strokes.len(), 3);
    for (got, want) in strokes.iter().zip(&want) {
        assert!(close(got, want), "{got:?} for {want:?}");
    }
    let arrows: Vec<(Pos2, egui::Vec2)> = items
        .iter()
        .filter_map(|d| match d {
            Drawn::Arrow { tip, toward, .. } => Some((*tip, *toward)),
            _ => None,
        })
        .collect();
    assert_eq!(arrows.len(), 2);
    assert!(
        near(arrows[0].0, pos2(100.0, 526.0)) && (arrows[0].1 - vec2(1.0, 0.0)).length() < 1e-5
    );
    assert!(
        near(arrows[1].0, pos2(500.0, 526.0)) && (arrows[1].1 - vec2(-1.0, 0.0)).length() < 1e-5
    );
    let (text, at) = label_of(&items).unwrap();
    assert_eq!(text, "40");
    assert!(near(at, pos2(300.0, 526.0)), "{at:?}");
}

#[test]
fn a_construction_line_is_dashed_and_the_others_are_not() {
    let mut sketch = rectangle(Vec::new());
    sketch.lines[2].construction = true;
    for d in drawn(&under(sketch)) {
        if let Drawn::Stroke {
            of: Some(of), kind, ..
        } = d
        {
            assert_eq!(
                kind,
                StrokeKind::Curve {
                    dashed: of.as_str() == "na000003"
                },
                "{of}"
            );
        }
    }
}

fn horizontal(name: &str, side: &str) -> SketchConstraint {
    constraint(
        name,
        Form::Horizontal {
            line: element(side),
        },
    )
}

/// The kind, place, and colour of `name`'s one glyph.
fn glyph_of(drawn: &[Drawn], name: &str) -> (GlyphKind, Pos2, egui::Color32) {
    match of(drawn, name).as_slice() {
        [
            Drawn::Glyph {
                kind, at, colour, ..
            },
        ] => (*kind, *at, *colour),
        other => panic!("{name}: {other:?}"),
    }
}

#[test]
fn a_redundant_relation_gets_its_own_glyph_its_line_untouched_and_glyphs_stack_along_it() {
    // Two horizontals on the bottom side, the second redundant, and one on the top.
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
    let (kind, first, colour) = glyph_of(&drawn, "ka000001");
    assert_eq!((kind, colour), (GlyphKind::Horizontal, GRAY_700));
    let (kind, second, colour) = glyph_of(&drawn, "ka000002");
    assert_eq!((kind, colour), (GlyphKind::Redundant, SKETCH_REDUNDANT));
    assert_eq!(glyph_of(&drawn, "ka000003").0, GlyphKind::Horizontal);
    assert_eq!(of(&drawn, "na000001")[0].colour(), SKETCH_UNDER_DEFINED);
    // The bottom side runs +x: its glyphs 0.28 of the way along, (11.2, 0), off its right,
    // 13 points below, (212, 513); the next 18 points on, (230, 513).
    assert!(near(first, pos2(212.0, 513.0)), "{first:?}");
    assert!(near(second, pos2(230.0, 513.0)), "{second:?}");
    // Hidden with Show Constraints off; the geometry stays.
    let hidden = drawing(
        &read,
        &view(),
        Shown {
            constraints: false,
            ..Shown::default()
        },
        &Default::default(),
        &[],
    );
    assert!(hidden.iter().all(|d| !matches!(d, Drawn::Glyph { .. })));
    assert_eq!(hidden.len(), 9);
}

#[test]
fn each_relation_draws_its_glyph_beside_each_member_it_names() {
    let read = under(rectangle(vec![
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
        constraint(
            "ka000008",
            Form::Midpoint {
                point: element("pa000002"),
                line: element("na000003"),
            },
        ),
    ]));
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
    // A point on a curve shows the coincident glyph (fusion.md, *Sketch mode*).
    assert_eq!(kinds("ka000007"), [GlyphKind::Coincident]);
    assert_eq!(kinds("ka000008"), [GlyphKind::Midpoint]);
}

#[test]
fn a_tangency_draws_at_its_point_and_an_angle_its_value_in_degrees_on_its_arc() {
    let read = under(rectangle(vec![
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
    ]));
    let drawn = drawn(&read);
    // Beside pa000002, (40, 0): down and left by 1.3 mm, (38.7, −1.3), (487, 513).
    let (kind, at, _) = glyph_of(&drawn, "ka000001");
    assert_eq!(kind, GlyphKind::Tangent);
    assert!(near(at, pos2(487.0, 513.0)), "{at:?}");
    // The sides meet at (40, 0); the arc from +x to +y, 2.6 mm out, its value at 45°.
    let (text, at) = label_of(&of(&drawn, "ka000002")).unwrap();
    assert_eq!(text, "90°");
    let out = 2.6 * std::f64::consts::FRAC_1_SQRT_2;
    assert!(near(at, hand([40.0 + out, out]).unwrap()), "{at:?}");
}

#[test]
fn a_quarter_arcs_polyline_is_ten_chords_each_within_the_tolerance_of_the_arc() {
    // Radius 10 mm, 100 points on the screen; tolerance 0.5: each chord's angle at most
    // 2·acos(1 − 0.005) = 0.20008 rad, so π/2 needs ⌈7.85⌉ = 8. Eight chords of π/16 stray
    // 100·(1 − cos π/32) = 0.4815 points; seven, 100·(1 − cos π/28) = 0.6288, too far. But
    // the outline takes the profile fill's count, worked out at the power of two below the
    // view's scale, 1/16 mm a point: 160 points, 2·acos(1 − 0.5/160) = 0.15815 rad a chord,
    // ⌈9.93⌉ = 10, each straying 100·(1 − cos π/40) = 0.3084.
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
    let read = under(sketch);
    let view = View {
        chord_tolerance: 0.5,
        ..view()
    };
    let drawn = drawing(&read, &view, Shown::default(), &Default::default(), &[]);
    let [Drawn::Stroke { points, .. }] = of(&drawn, "aa000001").as_slice() else {
        panic!("{drawn:?}");
    };
    assert_eq!(segments(160.0, FRAC_PI_2, 0.5), 10);
    assert_eq!(points.len(), 11);
    let centre = pos2(100.0, 500.0);
    assert!(near(points[0], pos2(200.0, 500.0)));
    assert!(near(points[10], pos2(100.0, 400.0)));
    for p in points {
        assert!(((*p - centre).length() - 100.0).abs() < 1e-3, "{p:?}");
    }
    for pair in points.windows(2) {
        let chord_middle = pair[0] + (pair[1] - pair[0]) / 2.0;
        let strays = 100.0 - (chord_middle - centre).length();
        assert!((strays - 0.3084).abs() < 1e-3, "{strays}");
    }
}

#[test]
fn a_circle_is_closed_its_segments_set_by_the_projections_largest_stretch() {
    // A projection three times as tall as wide: the circle of radius 10 stretches to 300
    // points up and down, 100 across; its chords are counted for 300.
    let tall = |at: [f64; 2]| Some(pos2((10.0 * at[0]) as f32, (-30.0 * at[1]) as f32));
    let points = round_points([0.0, 0.0], 10.0, 0.0, TAU, &tall, 0.5, 0);
    // 2·acos(1 − 0.5/300) = 0.11549 rad a chord: 2π takes ⌈54.4⌉ = 55.
    assert_eq!(segments(300.0, TAU, 0.5), 55);
    assert_eq!(points.len(), 56);
    assert_eq!(points.first(), points.last());
}

/// The screen area `triangles` cover.
fn area(triangles: &[[Pos2; 3]]) -> f32 {
    triangles
        .iter()
        .map(|[a, b, c]| ((*b - *a).x * (*c - *a).y - (*b - *a).y * (*c - *a).x).abs() / 2.0)
        .sum()
}

#[test]
fn the_profile_is_filled_less_its_holes_and_not_when_over_defined() {
    // The rectangle, 800 mm², and a 10 × 10 square hole in it from (10, 5): 700 mm², 70,000
    // square points at a hundred to the square millimetre.
    let mut sketch = rectangle(Vec::new());
    for (name, x, y) in [
        ("pb000001", 10.0, 5.0),
        ("pb000002", 20.0, 5.0),
        ("pb000003", 20.0, 15.0),
        ("pb000004", 10.0, 15.0),
    ] {
        sketch.points.push(point(name, x, y));
    }
    for (name, from, to) in [
        ("nb000001", "pb000001", "pb000004"),
        ("nb000002", "pb000004", "pb000003"),
        ("nb000003", "pb000003", "pb000002"),
        ("nb000004", "pb000002", "pb000001"),
    ] {
        sketch.lines.push(line(name, from, to));
    }
    let mut read = under(sketch);
    let sides = |names: [&str; 4]| names.iter().map(|n| id(n)).collect::<Vec<_>>();
    read.regions = vec![Region {
        outer: sides(["na000001", "na000002", "na000003", "na000004"]),
        holes: vec![sides(["nb000001", "nb000002", "nb000003", "nb000004"])],
    }];
    let fill = |read: &SketchRead| -> Option<f32> {
        drawn(read).iter().find_map(|d| match d {
            Drawn::Fill {
                triangles,
                colour,
                of,
            } => {
                assert_eq!(*colour, SKETCH_PROFILE_FILL);
                assert_eq!(of.as_ref(), Some(&id("na000001")));
                Some(area(triangles))
            }
            _ => None,
        })
    };
    let holed = fill(&read).unwrap();
    assert!((holed - 70_000.0).abs() < 1.0, "{holed}");
    // Without its hole: the whole rectangle, in two triangles.
    let mut whole = read.clone();
    whole.regions[0].holes.clear();
    assert!((fill(&whole).unwrap() - 80_000.0).abs() < 1.0);
    // Over defined, or with Show Profile off: none.
    let mut over = read.clone();
    over.states.state = SketchState::OverDefined;
    assert_eq!(fill(&over), None);
    let off = drawing(
        &read,
        &view(),
        Shown {
            profile: false,
            ..Shown::default()
        },
        &Default::default(),
        &region_fills(&read, HAND, SKETCH_CHORD_TOLERANCE),
    );
    assert!(off.iter().all(|d| !matches!(d, Drawn::Fill { .. })));
}

#[test]
fn the_tessellation_covers_a_concave_polygon_and_a_holed_one_exactly() {
    // An L: (0,0) (40,0) (40,10) (10,10) (10,30) (0,30), clockwise as given: its foot 40
    // by 10 and its upright 10 by 20, 600 mm². The tessellator works in f32, about the
    // loops' middle: areas to a thousandth.
    let l = vec![
        [0.0, 0.0],
        [0.0, 30.0],
        [10.0, 30.0],
        [10.0, 10.0],
        [40.0, 10.0],
        [40.0, 0.0],
    ];
    let total = |t: &[[[f64; 2]; 3]]| t.iter().map(|t| signed_area(&t[..]).abs()).sum::<f64>();
    assert!((total(&tessellate(&[l]).unwrap()) - 600.0).abs() < 1e-3);
    // A square with two holes: 100 less 4 and 9.
    let square = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
    let loops = [
        square,
        vec![[1.0, 1.0], [3.0, 1.0], [3.0, 3.0], [1.0, 3.0]],
        vec![[5.0, 5.0], [8.0, 5.0], [8.0, 8.0], [5.0, 8.0]],
    ];
    let triangles = tessellate(&loops).unwrap();
    assert!(
        (total(&triangles) - 87.0).abs() < 1e-3,
        "{}",
        total(&triangles)
    );
    // Too few points to fill: nothing.
    assert!(
        tessellate(&[vec![[0.0, 0.0], [1.0, 0.0]]])
            .unwrap_or_default()
            .is_empty()
    );
}

#[test]
fn the_grid_is_the_viewports_its_steps_with_the_zoom_its_fade_and_its_axes_in_the_planes_colours() {
    // Ten points a millimetre: the minor step, 10 mm, is 100 points apart. From (−1, −1) to
    // (11, 6) in view: lines at x = 0 and 10, and y = 0; those through 0 the 50 mm ones.
    // Then the two axes, the XY plane's x red and y green, whole.
    let read = under(rectangle(Vec::new()));
    let seen = View {
        visible: Some(([-1.0, -1.0], [11.0, 6.0])),
        ..view()
    };
    let drawn = drawing(&read, &seen, Shown::default(), &Default::default(), &[]);
    let lines = |kind| {
        drawn
            .iter()
            .filter_map(|d| match d {
                Drawn::Stroke {
                    kind: k,
                    points,
                    colour,
                    ..
                } if *k == kind => Some((points.clone(), *colour)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let minor = lines(StrokeKind::GridMinor);
    assert_eq!(minor.len(), 1);
    // x = 10 runs (10, −1) to (10, 6): (200, 510) to (200, 440); 10 mm out of 200, faded to
    // 95%.
    assert_eq!(minor[0].0, vec![pos2(200.0, 510.0), pos2(200.0, 440.0)]);
    assert_eq!(
        minor[0].1,
        color32(VIEWPORT_GRID_MINOR).gamma_multiply(0.95)
    );
    assert_eq!(lines(StrokeKind::GridMajor).len(), 2);
    let axes: Vec<egui::Color32> = lines(StrokeKind::Axis)
        .into_iter()
        .map(|(_, c)| c)
        .collect();
    assert_eq!(axes, [color32(AXIS_X), color32(AXIS_Y)]);
    // At 2 mm a point the minor lines (5 points apart, under 8) go, the major (25) stay:
    // x = −100, −50, 0, 50, 100 and y = 0. At 10 mm a point, all go. Past the fade, at
    // 200 mm, none.
    let out = |scale: f64, visible| {
        let seen = View {
            scale: Scale(scale),
            visible: Some(visible),
            ..view()
        };
        let drawn = drawing(&read, &seen, Shown::default(), &Default::default(), &[]);
        let count = |kind| {
            drawn
                .iter()
                .filter(|d| matches!(d, Drawn::Stroke { kind: k, .. } if *k == kind))
                .count()
        };
        (count(StrokeKind::GridMinor), count(StrokeKind::GridMajor))
    };
    assert_eq!(out(2.0, ([-120.0, -1.0], [120.0, 1.0])), (0, 6));
    assert_eq!(out(10.0, ([-120.0, -1.0], [120.0, 1.0])), (0, 0));
    assert_eq!(out(0.1, ([200.5, 200.5], [230.0, 230.0])), (0, 0));
}

#[test]
fn a_tools_preview_draws_its_line_dashed_its_guides_dotted_and_what_the_pointer_shows() {
    let pv = Preview {
        pointer: Some([10.0, 0.0]),
        points: vec![[0.0, 0.0]],
        lines: vec![[[0.0, 0.0], [10.0, 0.0]]],
        shown: vec![
            Showing {
                at: [10.0, 0.0],
                relation: Relation::Horizontal,
            },
            Showing {
                at: [10.0, 0.0],
                relation: Relation::Shared(id("pa000002")),
            },
        ],
        guides: vec![[[10.0, 0.0], [10.0, 20.0]]],
        measures: vec![PreviewMeasure {
            from: [0.0, 0.0],
            to: [10.0, 0.0],
            kind: MeasureKind::Length,
        }],
        ..Preview::default()
    };
    let drawn = preview(&pv, &view());
    let strokes = |kind| {
        drawn
            .iter()
            .filter_map(|d| match d {
                Drawn::Stroke {
                    kind: k,
                    points,
                    colour,
                    ..
                } if *k == kind => Some((points.clone(), *colour)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        strokes(StrokeKind::Guide),
        [(
            vec![pos2(200.0, 500.0), pos2(200.0, 300.0)],
            SKETCH_GUIDE_STROKE.color
        )]
    );
    assert_eq!(
        strokes(StrokeKind::Preview {
            construction: false
        }),
        [(
            vec![pos2(100.0, 500.0), pos2(200.0, 500.0)],
            SKETCH_PREVIEW_STROKE.color
        )]
    );
    assert_eq!(strokes(StrokeKind::Crosshair).len(), 2);
    let glyphs: Vec<(GlyphKind, Pos2, egui::Color32)> = drawn
        .iter()
        .filter_map(|d| match d {
            Drawn::Glyph {
                kind, at, colour, ..
            } => Some((*kind, *at, *colour)),
            _ => None,
        })
        .collect();
    // Beside the pointer, (200, 500): 18 right and 16 up, the next a step on.
    assert_eq!(
        glyphs,
        [
            (GlyphKind::Horizontal, pos2(218.0, 484.0), FOCUS),
            (
                GlyphKind::Coincident,
                pos2(236.0, 484.0),
                SKETCH_SNAP_STROKE.color
            ),
        ]
    );
    assert!(drawn.contains(&Drawn::Ring {
        at: pos2(200.0, 500.0),
        colour: SKETCH_SNAP_STROKE.color
    }));
    // Its length, 10, beside its segment's middle, (150, 500): 12 points below.
    assert!(drawn.contains(&Drawn::Label {
        of: None,
        kind: LabelKind::Tool,
        at: pos2(150.0, 512.0),
        text: "10.00 mm".to_owned(),
        colour: SKETCH_DIMENSION_TEXT,
        selected: false,
    }));
    assert!(preview(&Preview::default(), &view()).is_empty());
}

/// The tool labels of `drawn`: each one's place and text.
fn tool_labels(drawn: &[Drawn]) -> Vec<(Pos2, String)> {
    drawn
        .iter()
        .filter_map(|d| match d {
            Drawn::Label {
                kind: LabelKind::Tool,
                at,
                text,
                ..
            } => Some((*at, text.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_previews_measures_are_labelled_beside_their_segments_and_only_they_are() {
    let measure = |from, to, kind| PreviewMeasure { from, to, kind };
    let pv = Preview {
        pointer: Some([40.0, 20.0]),
        lines: vec![[[0.0, 0.0], [40.0, 0.0]], [[40.0, 0.0], [40.0, 20.0]]],
        measures: vec![
            measure([0.0, 0.0], [40.0, 0.0], MeasureKind::Length),
            measure([40.0, 0.0], [40.0, 20.0], MeasureKind::Length),
            measure([0.0, 0.0], [3.0, 4.0], MeasureKind::Radius),
        ],
        ..Preview::default()
    };
    let labels = tool_labels(&preview(&pv, &view()));
    // The width's segment, (100, 500) to (500, 500): 12 below its middle, (300, 500). The
    // height's, (500, 500) up to (500, 300): 12 right of its middle, (500, 400). The
    // radius's, (100, 500) to (130, 460), 5 long: 12 along (0.8, 0.6) from (115, 480).
    assert_eq!(labels.len(), 3);
    assert_eq!(labels[0], (pos2(300.0, 512.0), "40.00 mm".to_owned()));
    assert_eq!(labels[1], (pos2(512.0, 400.0), "20.00 mm".to_owned()));
    assert_eq!(labels[2].1, "R 5.00 mm");
    assert!(near(labels[2].0, pos2(124.6, 487.2)), "{:?}", labels[2].0);
    // A line under way with no measure has no label.
    let unmeasured = Preview {
        measures: Vec::new(),
        ..pv
    };
    assert!(tool_labels(&preview(&unmeasured, &view())).is_empty());
}

#[test]
fn a_construction_previews_curves_are_drawn_as_construction() {
    for construction in [false, true] {
        let pv = Preview {
            lines: vec![[[0.0, 0.0], [10.0, 0.0]]],
            circles: vec![([0.0, 0.0], 5.0)],
            arcs: vec![silico_core::sketch_tools::PreviewArc {
                center: [0.0, 0.0],
                start: [5.0, 0.0],
                end: [0.0, 5.0],
            }],
            construction,
            ..Preview::default()
        };
        let kinds: Vec<StrokeKind> = preview(&pv, &view())
            .iter()
            .filter_map(|d| match d {
                Drawn::Stroke { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect();
        assert_eq!(kinds, [StrokeKind::Preview { construction }; 3]);
    }
}

#[test]
fn hit_takes_a_point_before_a_curve_and_the_nearest_curve_before_a_farther_one() {
    let read = under(rectangle(Vec::new()));
    let found = |at: [f64; 2], tolerance: f64| hit(&read, at, tolerance, HAND);
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
    assert_eq!(
        hit(&under(sketch), [1.0, 0.0], 1.5, HAND),
        Some(Hit {
            id: id("pb000001"),
            kind: ElementKind::Point
        })
    );
}

#[test]
fn hit_finds_a_glyph_and_a_dimension_where_theyre_drawn() {
    // The top side's horizontal: the side runs from (40, 20) to (0, 20), −x, so its right is
    // +y: the glyph 0.28 along it, (28.8, 20), 1.3 mm above, (28.8, 21.3). The bottom side's
    // length, its value at (20, −2.6).
    let read = read(
        rectangle(vec![
            horizontal("ka000001", "na000003"),
            length("ka000002", "na000001", "40", true),
        ]),
        SketchState::FullyDefined,
        MemberState::FullyDefined,
        &[],
    );
    let found = |at: [f64; 2]| hit(&read, at, 0.1, HAND).map(|h| h.id);
    assert_eq!(found([28.8, 21.3]), Some(id("ka000001")));
    assert_eq!(found([20.0, -2.6]), Some(id("ka000002")));
    // On an extension line, (40, −1.5).
    assert_eq!(found([40.0, -1.5]), Some(id("ka000002")));
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
    let read = under(sketch);
    let found = |at: [f64; 2]| hit(&read, at, 0.5, HAND).map(|h| h.kind);
    // On the sweep, at 45°, 0.2 outside the rim.
    let r = 10.2 * std::f64::consts::FRAC_1_SQRT_2;
    assert_eq!(found([r, r]), Some(ElementKind::Arc));
    // At 225°, on the circle but off the sweep, far from either end.
    assert_eq!(found([-r, -r]), None);
}

#[test]
fn an_empty_sketch_draws_only_the_origin_and_hits_nothing() {
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
    assert_eq!(
        drawn(&read),
        [Drawn::Origin {
            at: pos2(100.0, 500.0)
        }]
    );
    assert_eq!(hit(&read, [0.0, 0.0], 100.0, HAND), None);
    assert_eq!(state_shown(&read).words, "Empty");
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
    let read = under(sketch);
    let view = View {
        project: &half,
        ..view()
    };
    let drawn = drawing(&read, &view, Shown::default(), &Default::default(), &[]);
    for d in &drawn {
        for p in d.positions() {
            assert!(p.x.is_finite() && p.y.is_finite(), "{d:?}");
        }
        if let Drawn::Stroke { points, .. } = d {
            assert!(points.len() >= 2);
        }
    }
    // Only the right side (x = 40) is whole; the bottom and top lose an end; the circle
    // keeps what's right of x = 20; the NaN point and the origin aren't drawn.
    let sides: Vec<&str> = drawn
        .iter()
        .filter_map(|d| match d {
            Drawn::Stroke { of: Some(of), .. } if of.as_str().starts_with("na") => {
                Some(of.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(sides, ["na000002"]);
    assert!(!of(&drawn, "ca000001").is_empty());
    assert!(of(&drawn, "pa000005").is_empty());
    assert!(!drawn.iter().any(|d| matches!(d, Drawn::Origin { .. })));
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
    let read = under(sketch);
    let close = |at: [f64; 2]| Some(pos2((1000.0 * at[0]) as f32, (-1000.0 * at[1]) as f32));
    let view = View {
        project: &close,
        scale: Scale(0.001),
        visible: Some(([1e6 - 1.0, -1e6 - 1.0], [1e6 + 1.0, -1e6 + 1.0])),
        chord_tolerance: SKETCH_CHORD_TOLERANCE,
    };
    let drawn = drawing(&read, &view, Shown::default(), &Default::default(), &[]);
    let [Drawn::Stroke { points, .. }] = of(&drawn, "ca000001")
        .into_iter()
        .filter(|d| matches!(d, Drawn::Stroke { .. }))
        .collect::<Vec<_>>()
        .as_slice()
    else {
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
    assert!(hit(&read, [1e6, 0.0], 1.0, Scale(0.001)).is_some());
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
            length("ka000001", "na000001", "0", true),
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
            horizontal("ka000004", "na000001"),
        ],
    };
    let mut read = read(sketch, SketchState::Invalid, MemberState::UnderDefined, &[]);
    read.regions = vec![Region {
        outer: vec![id("na000001")],
        holes: Vec::new(),
    }];
    let drawn = drawn(&read);
    for d in &drawn {
        assert!(
            d.positions()
                .iter()
                .all(|p| p.x.is_finite() && p.y.is_finite()),
            "{d:?}"
        );
        if let Drawn::Arrow { toward, .. } = d {
            assert!(toward.x.is_finite() && toward.y.is_finite());
        }
    }
    assert!(hit(&read, [5.0, 5.0], 0.1, HAND).is_some());
}

#[test]
fn thousands_of_members_draw_and_hit_within_a_frames_budget() {
    // 4,000 lines in chains, 1,000 circles, and 2,000 relations and dimensions: what's
    // drawn is bounded by the members (each circle at most the cap).
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
        constraints.push(horizontal(&name("k", 2 * n), &name("n", n)));
        constraints.push(length(
            &name("k", 2 * n + 1),
            &name("n", n + 1000),
            "1",
            true,
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
    let read = under(sketch);
    let started = std::time::Instant::now();
    let drawn = drawn(&read);
    let drawing_took = started.elapsed();
    let started = std::time::Instant::now();
    let found = hit(&read, [50.5, 20.5], 0.7, HAND);
    let hit_took = started.elapsed();
    // The origin, 4,000 sides, 1,000 circles, 4,001 points, 1,000 glyphs, and 1,000
    // dimensions of six.
    assert_eq!(drawn.len(), 1 + 4000 + 1000 + 4001 + 1000 + 6000);
    let vertices: usize = drawn.iter().map(|d| d.positions().len()).sum();
    assert!(vertices < 200_000, "{vertices}");
    assert!(found.is_some());
    // Measured on a laptop (M5.11a's report); no bound here, as nothing here holds to a
    // clock.
    let _ = (drawing_took, hit_took);
}

#[test]
fn a_flat_sketch_fills_its_rectangle_but_the_margin_and_maps_back() {
    let read = under(rectangle(Vec::new()));
    let rect = egui::Rect::from_min_size(pos2(10.0, 20.0), egui::vec2(500.0, 300.0));
    let flat = Flat::fit(rect, &read);
    // Bounds (0, 0) to (40, 20), the origin among them; the shorter side, 300, less a tenth
    // each side, 240 for the 40 mm span: 6 points a millimetre, centred at (260, 170).
    assert_eq!(flat.project([20.0, 10.0]), Some(pos2(260.0, 170.0)));
    assert_eq!(flat.project([0.0, 0.0]), Some(pos2(140.0, 230.0)));
    assert_eq!(flat.to_plane(pos2(140.0, 230.0)), Some([0.0, 0.0]));
    assert_eq!(flat.scale(), Scale(1.0 / 6.0));
    let project = |at| flat.project(at);
    let view = View {
        project: &project,
        scale: flat.scale(),
        visible: None,
        chord_tolerance: SKETCH_CHORD_TOLERANCE,
    };
    let drawn = drawing(&read, &view, Shown::default(), &Default::default(), &[]);
    for d in &drawn {
        assert!(d.positions().iter().all(|p| rect.contains(*p)), "{d:?}");
    }
}

/// `f` with a painter, in one pass of a context with the app's fonts.
pub(crate) fn with_painter(f: impl FnOnce(&egui::Painter)) {
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
pub(crate) fn unhand(at: Pos2) -> Option<[f64; 2]> {
    Some([
        (f64::from(at.x) - 100.0) / 10.0,
        (500.0 - f64::from(at.y)) / 10.0,
    ])
}

fn event(action: PointerAction, at: Pos2, ctrl: bool) -> PointerEvent {
    PointerEvent {
        action,
        at,
        shift: false,
        ctrl,
    }
}

/// A click at `at`: a press, then its release.
fn press(at: Pos2, ctrl: bool) -> [PointerEvent; 2] {
    [
        event(PointerAction::Press, at, ctrl),
        event(PointerAction::Release, at, ctrl),
    ]
}

fn mode_on(read: SketchRead) -> SketchMode {
    let mut mode = SketchMode::new(1, None);
    mode.read = Some(Read {
        model: None,
        result: Ok(read),
    });
    mode
}

#[test]
fn a_frames_presses_select_ctrl_adds_and_a_press_on_nothing_clears() {
    let mut mode = mode_on(under(rectangle(Vec::new())));
    let frame = |mode: &mut SketchMode, events: &[PointerEvent], pointer: Option<Pos2>| {
        with_painter(|painter| {
            let input = FrameInput {
                events,
                pointer,
                project: &hand,
                to_plane: &unhand,
            };
            mode.frame(painter, &input);
        });
    };
    let selected = |mode: &SketchMode| -> Vec<String> {
        mode.selected_ids()
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect()
    };
    // The corner (0, 0) is at (100, 500); a press 2 points off is within the 7-point pick.
    frame(
        &mut mode,
        &press(pos2(102.0, 500.0), false),
        Some(pos2(250.0, 450.0)),
    );
    assert_eq!(selected(&mode), ["pa000001"]);
    // The pointer at (250, 450) is (15, 5) on the plane.
    assert_eq!(mode.pointer, Some([15.0, 5.0]));
    // Ctrl on the top side's middle, (20, 20): (300, 300), 3 points off it.
    frame(&mut mode, &press(pos2(300.0, 303.0), true), None);
    assert_eq!(selected(&mode), ["na000003", "pa000001"]);
    assert_eq!(mode.pointer, None);
    // Ctrl on the corner again takes it away; a plain press on the right side replaces all.
    frame(&mut mode, &press(pos2(100.0, 500.0), true), None);
    assert_eq!(selected(&mode), ["na000003"]);
    frame(&mut mode, &press(pos2(500.0, 400.0), false), None);
    assert_eq!(selected(&mode), ["na000002"]);
    // A press on nothing, 10 mm from every side, clears it; a move selects nothing.
    frame(&mut mode, &press(pos2(300.0, 400.0), false), None);
    assert!(mode.selection.is_empty());
    let moved = event(PointerAction::Move, pos2(100.0, 500.0), false);
    frame(&mut mode, &[moved], None);
    assert!(mode.selection.is_empty());
    // Selected on the release: a press alone selects nothing yet, and a press cancelled
    // (the pointer gone, a navigation tool taking over) leaves the selection as it was.
    frame(&mut mode, &press(pos2(500.0, 400.0), false), None);
    assert_eq!(selected(&mode), ["na000002"]);
    frame(
        &mut mode,
        &[event(PointerAction::Press, pos2(100.0, 500.0), false)],
        None,
    );
    assert_eq!(selected(&mode), ["na000002"]);
    frame(
        &mut mode,
        &[event(PointerAction::Cancel, pos2(100.0, 500.0), false)],
        None,
    );
    frame(
        &mut mode,
        &[event(PointerAction::Release, pos2(100.0, 500.0), false)],
        None,
    );
    assert_eq!(selected(&mode), ["na000002"]);
    // A press 8 points (0.8 mm) off the top side, past the 7-point pick: nothing under it,
    // so its click clears the selection.
    frame(&mut mode, &press(pos2(300.0, 308.0), false), None);
    assert!(mode.selection.is_empty());
    // And 7 points (0.7 mm) off it, on the pick's edge: selected.
    frame(&mut mode, &press(pos2(300.0, 307.0), false), None);
    assert_eq!(selected(&mode), ["na000003"]);
    // A press on a member that a read taken before its release no longer holds: a click on
    // nothing.
    frame(
        &mut mode,
        &[event(PointerAction::Press, pos2(500.0, 400.0), false)],
        None,
    );
    let mut fewer = rectangle(Vec::new());
    fewer.lines.remove(1);
    mode.took(Read {
        model: None,
        result: Ok(under(fewer)),
    });
    frame(
        &mut mode,
        &[event(PointerAction::Release, pos2(500.0, 400.0), false)],
        None,
    );
    assert!(mode.selection.is_empty()); // One taken between that still holds it: selected.
    frame(
        &mut mode,
        &[event(PointerAction::Press, pos2(300.0, 300.0), false)],
        None,
    );
    mode.took(Read {
        model: None,
        result: Ok(under(rectangle(Vec::new()))),
    });
    frame(
        &mut mode,
        &[event(PointerAction::Release, pos2(300.0, 300.0), false)],
        None,
    );
    assert_eq!(selected(&mode), ["na000003"]);
}

#[test]
fn a_selected_member_is_drawn_selected_its_state_colour_kept() {
    let read = under(rectangle(vec![horizontal("ka000001", "na000001")]));
    let selected = [id("na000001"), id("ka000001"), id("pa000002")]
        .into_iter()
        .collect();
    let drawn = drawing(&read, &view(), Shown::default(), &selected, &[]);
    for name in ["na000001", "ka000001", "pa000002"] {
        assert!(of(&drawn, name).iter().all(|d| d.selected()), "{name}");
    }
    assert_eq!(of(&drawn, "na000001")[0].colour(), SKETCH_UNDER_DEFINED);
    assert!(!of(&drawn, "na000002")[0].selected());
}

#[test]
fn every_kind_turns_into_shapes_none_left_out() {
    // What's drawn of a sketch with every kind of item, selected and not, and a preview:
    // each item makes at least one shape.
    let mut read = under(rectangle(vec![
        horizontal("ka000001", "na000001"),
        length("ka000002", "na000002", "20", true),
    ]));
    read.regions = vec![Region {
        outer: ["na000001", "na000002", "na000003", "na000004"]
            .iter()
            .map(|n| id(n))
            .collect(),
        holes: Vec::new(),
    }];
    read.members.lines[3].construction = true;
    let selected = [
        id("na000002"),
        id("ka000002"),
        id("pa000001"),
        id("ka000001"),
    ]
    .into_iter()
    .collect();
    let mut drawn = drawing(&read, &view(), Shown::default(), &selected, &[]);
    drawn.extend(preview(
        &Preview {
            pointer: Some([1.0, 1.0]),
            lines: vec![[[0.0, 0.0], [1.0, 1.0]]],
            shown: vec![Showing {
                at: [1.0, 1.0],
                relation: Relation::Origin,
            }],
            ..Preview::default()
        },
        &view(),
    ));
    with_painter(|painter| {
        for item in &drawn {
            assert!(
                !super::draw::to_shapes(painter, std::slice::from_ref(item)).is_empty(),
                "{item:?}"
            );
        }
    });
}

#[test]
fn delete_joins_a_lone_point_two_curves_end_at_and_removes_anything_else() {
    let mut sketch = rectangle(Vec::new());
    sketch.lines.pop();
    let mut mode = mode_on(under(sketch));
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
    for (name, ctrl) in [("na000002", false), ("nz000009", true)] {
        mode.click(
            Some(Hit {
                id: id(name),
                kind: ElementKind::Line,
            }),
            ctrl,
        );
    }
    mode.took(Read {
        model: None,
        result: Ok(under(rectangle(Vec::new()))),
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
        result: Err(refused),
    });
    assert_eq!(mode.palette_view(r(3)).said, None);
    assert_eq!(
        mode.palette_view(r(2)).said.as_deref(),
        Some("the document has no feature s0000001")
    );
    assert_eq!(mode.palette_view(r(2)).state, None);
}

#[test]
fn the_states_words_are_the_prototypes_and_the_prefixes_solidworks() {
    let mut read = under(rectangle(Vec::new()));
    read.states.dofs = 3;
    let words: Vec<(String, egui::Color32, Option<&str>)> = SketchState::ALL
        .iter()
        .map(|&s| {
            read.states.state = s;
            let shown = state_shown(&read);
            (shown.words, shown.colour, prefix(s))
        })
        .collect();
    assert_eq!(
        words,
        [
            (
                "Under defined · 3 DOF".to_owned(),
                SKETCH_UNDER_DEFINED,
                Some("(–)")
            ),
            ("Fully defined".to_owned(), SKETCH_FULLY_DEFINED, None),
            ("Over defined".to_owned(), SKETCH_OVER_DEFINED, Some("(+)")),
            (
                "No solution found".to_owned(),
                SKETCH_OVER_DEFINED,
                Some("(?)")
            ),
            (
                "Invalid solution found".to_owned(),
                SKETCH_INVALID,
                Some("(?)")
            ),
        ]
    );
}

#[test]
fn the_status_line_names_the_sketch_its_pointer_by_the_planes_axes_and_its_state() {
    let mut read = under(rectangle(Vec::new()));
    read.states.dofs = 2;
    read.plane = Ok(OriginPlane::Xz.into());
    let mut mode = mode_on(read);
    let line = mode.status("Sketch1");
    assert_eq!(line.editing, "Editing Sketch1");
    assert_eq!(line.coordinates, None);
    assert_eq!(line.state.unwrap().words, "Under defined · 2 DOF");
    mode.pointer = Some([15.0, -5.25]);
    assert_eq!(
        mode.status("Sketch1").coordinates.as_deref(),
        Some("x 15.000 · z -5.250 mm")
    );
    let bar = mode.bar_view("Sketch1", true);
    assert_eq!((bar.name.as_str(), bar.plane.as_str()), ("Sketch1", "XZ"));
    // A face whose frame broke: by u and v.
    if let Some(Read {
        result: Ok(read), ..
    }) = &mut mode.read
    {
        read.plane = Err(silico_core::Error::new(
            silico_core::ErrorCode::ReferenceBroken,
            "gone",
        ));
    }
    assert_eq!(
        mode.status("Top").coordinates.as_deref(),
        Some("u 15.000 · v -5.250 mm")
    );
    assert_eq!(
        mode.palette_view(silico_core::command::Revision::OPENED)
            .plane_problem,
        Some(("gone".to_owned(), None))
    );
}

#[test]
fn each_constraint_states_colour_on_each_sketch_state_is_its_own() {
    use ConstraintState::{Conflicting, PartiallyRedundant, Redundant, Satisfied, Unsolved};
    let dimension = Form::Length {
        line: element("na000001"),
        value: mm("40"),
        driving: true,
    };
    let driven = Form::Length {
        line: element("na000001"),
        value: mm("40"),
        driving: false,
    };
    let relation = Form::Horizontal {
        line: element("na000001"),
    };
    let (dim, rel, over, red, inv) = (
        SKETCH_DIMENSION_TEXT,
        GRAY_700,
        SKETCH_OVER_DEFINED,
        SKETCH_REDUNDANT,
        SKETCH_INVALID,
    );
    // Each row: the constraint's state and form, then its colour on a sketch under defined,
    // fully defined, over defined, with no solution, and invalid.
    let table: [(ConstraintState, &Form, [egui::Color32; 5]); 15] = [
        (Satisfied, &dimension, [dim; 5]),
        (Satisfied, &relation, [rel; 5]),
        (Satisfied, &driven, [SKETCH_DRIVEN; 5]),
        (Conflicting, &dimension, [over; 5]),
        (Conflicting, &relation, [over; 5]),
        (Redundant, &dimension, [over; 5]),
        (Redundant, &relation, [red; 5]),
        (PartiallyRedundant, &dimension, [over; 5]),
        (PartiallyRedundant, &relation, [red; 5]),
        (Unsolved, &dimension, [dim, dim, dim, over, inv]),
        (Unsolved, &relation, [rel, rel, rel, over, inv]),
        (
            Unsolved,
            &driven,
            [SKETCH_DRIVEN, SKETCH_DRIVEN, SKETCH_DRIVEN, over, inv],
        ),
        (Conflicting, &driven, [over; 5]),
        (Redundant, &driven, [over; 5]),
        (PartiallyRedundant, &driven, [over; 5]),
    ];
    for (state, form, colours) in table {
        for (sketch, want) in SketchState::ALL.into_iter().zip(colours) {
            assert_eq!(
                draw::constraint_colour(sketch, form, Some(state)),
                want,
                "{state:?} {} on {sketch:?}",
                form.type_name()
            );
        }
    }
    // A constraint given no state: as one satisfied.
    for (form, want) in [
        (&dimension, dim),
        (&driven, SKETCH_DRIVEN),
        (&relation, rel),
    ] {
        for sketch in SketchState::ALL {
            assert_eq!(
                draw::constraint_colour(sketch, form, None),
                want,
                "{sketch:?}"
            );
        }
    }
    // Every constraint state is in the table.
    assert_eq!(
        ConstraintState::ALL.len(),
        table
            .iter()
            .map(|row| row.0)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );
    // A member with no state given reads under defined, unless the sketch has no solution
    // or an invalid one.
    use MemberState::{FullyDefined, OverDefined};
    for (sketch, state, want) in [
        (SketchState::UnderDefined, None, SKETCH_UNDER_DEFINED),
        (SketchState::FullyDefined, None, SKETCH_UNDER_DEFINED),
        (SketchState::NoSolution, None, SKETCH_OVER_DEFINED),
        (SketchState::Invalid, None, SKETCH_INVALID),
        (SketchState::Invalid, Some(FullyDefined), SKETCH_INVALID),
        (
            SketchState::OverDefined,
            Some(OverDefined),
            SKETCH_OVER_DEFINED,
        ),
        (
            SketchState::FullyDefined,
            Some(FullyDefined),
            SKETCH_FULLY_DEFINED,
        ),
    ] {
        assert_eq!(
            draw::member_colour(sketch, state),
            want,
            "{sketch:?} {state:?}"
        );
    }
}

fn arc(name: &str, center: &str, start: &str, end: &str) -> SketchArc {
    SketchArc {
        id: id(name),
        center: id(center),
        start: id(start),
        end: id(end),
        construction: false,
    }
}

fn sketch_of(points: Vec<SketchPoint>, lines: Vec<SketchLine>, arcs: Vec<SketchArc>) -> Sketch {
    Sketch {
        plane: OriginPlane::Xy.into(),
        points,
        lines,
        circles: Vec::new(),
        arcs,
        constraints: Vec::new(),
    }
}

#[test]
fn delete_joins_two_arcs_or_a_line_and_an_arc_at_their_point_and_removes_a_point_three_lines_meet()
{
    let point_hit = |name: &str| Hit {
        id: id(name),
        kind: ElementKind::Point,
    };
    let deleting = |sketch: Sketch, point: &str| {
        let mut mode = mode_on(under(sketch));
        mode.click(Some(point_hit(point)), false);
        mode.deletion()
    };
    // A split arc's two pieces about pc000000: the first ends at pm000000, the second starts
    // there.
    let arcs = sketch_of(
        vec![
            point("pc000000", 0.0, 0.0),
            point("ps000000", 10.0, 0.0),
            point("pm000000", 0.0, 10.0),
            point("pe000000", -10.0, 0.0),
        ],
        Vec::new(),
        vec![
            arc("aa000001", "pc000000", "ps000000", "pm000000"),
            arc("aa000002", "pc000000", "pm000000", "pe000000"),
        ],
    );
    assert_eq!(
        deleting(arcs, "pm000000"),
        Some(Deletion::Join(id("pm000000")))
    );
    // A line ending where an arc starts.
    let line_and_arc = sketch_of(
        vec![
            point("pc000000", 0.0, 0.0),
            point("pq000000", 10.0, -10.0),
            point("ps000000", 10.0, 0.0),
            point("pm000000", 0.0, 10.0),
        ],
        vec![line("na000001", "pq000000", "ps000000")],
        vec![arc("aa000001", "pc000000", "ps000000", "pm000000")],
    );
    assert_eq!(
        deleting(line_and_arc, "ps000000"),
        Some(Deletion::Join(id("ps000000")))
    );
    // Three lines at pp000000: not a split's two pieces, so removed (the core refuses a point
    // curves use, and says so).
    let three = sketch_of(
        vec![
            point("pp000000", 0.0, 0.0),
            point("pa000000", 10.0, 0.0),
            point("pb000000", 0.0, 10.0),
            point("pd000000", -10.0, 0.0),
        ],
        vec![
            line("na000001", "pa000000", "pp000000"),
            line("na000002", "pp000000", "pb000000"),
            line("na000003", "pp000000", "pd000000"),
        ],
        Vec::new(),
    );
    assert_eq!(
        deleting(three, "pp000000"),
        Some(Deletion::Remove(vec![element("pp000000")]))
    );
}

/// Twice a polygon's signed area, halved: positive counter-clockwise.
fn signed_area(ring: &[[f64; 2]]) -> f64 {
    (0..ring.len())
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            a[0] * b[1] - a[1] * b[0]
        })
        .sum::<f64>()
        / 2.0
}

/// The area of `fills`' triangles, in square millimetres.
fn fill_area(fills: &[draw::RegionFill]) -> f64 {
    fills
        .iter()
        .flat_map(|f| f.triangles.iter())
        .map(|t| signed_area(&t[..]).abs())
        .sum()
}

fn region(outer: &[&str], holes: &[&[&str]]) -> Region {
    Region {
        outer: outer.iter().map(|n| id(n)).collect(),
        holes: holes
            .iter()
            .map(|h| h.iter().map(|n| id(n)).collect())
            .collect(),
    }
}

#[test]
fn a_profile_with_a_side_written_backwards_fills_the_same() {
    let mut sketch = rectangle(Vec::new());
    // The top side from pa000004 to pa000003: against the loop's way round.
    sketch.lines[2] = line("na000003", "pa000004", "pa000003");
    let mut read = under(sketch);
    read.regions = vec![region(
        &["na000001", "na000002", "na000003", "na000004"],
        &[],
    )];
    let fills = region_fills(&read, HAND, SKETCH_CHORD_TOLERANCE);
    assert!((fill_area(&fills) - 800.0).abs() < 1e-3);
    let drawn = drawing(
        &read,
        &view(),
        Shown::default(),
        &Default::default(),
        &fills,
    );
    let filled = drawn.iter().find_map(|d| match d {
        Drawn::Fill { triangles, .. } => Some(area(triangles)),
        _ => None,
    });
    assert!(
        filled.is_some_and(|a| (a - 80_000.0).abs() < 1.0),
        "{filled:?}"
    );
}

#[test]
fn a_circles_and_an_arcs_fill_stay_within_the_chord_bound_of_their_area() {
    // At a tenth of a millimetre a point, the chords stray at most 0.25 points, 0.025 mm:
    // a fill falls short of the true area by at most its perimeter times that.
    let stray = 0.25 * 0.1;
    let mut circle = under(sketch_of(
        vec![point("pa000001", 0.0, 0.0)],
        Vec::new(),
        Vec::new(),
    ));
    circle.members.circles.push(SketchCircle {
        id: id("ca000001"),
        center: id("pa000001"),
        radius: Coordinate::new(10.0).unwrap(),
        construction: false,
    });
    circle.regions = vec![region(&["ca000001"], &[])];
    let disc = fill_area(&region_fills(&circle, HAND, SKETCH_CHORD_TOLERANCE));
    let full = PI * 100.0;
    assert!(disc <= full && disc >= full - TAU * 10.0 * stray, "{disc}");
    // A 20 × 10 rectangle whose top is an arc about (10, 10), bulging up through a half turn:
    // the arc in the middle of the loop. 200 + 50π.
    let mut arched = under(sketch_of(
        vec![
            point("pa000001", 0.0, 0.0),
            point("pa000002", 20.0, 0.0),
            point("pa000003", 20.0, 10.0),
            point("pa000004", 0.0, 10.0),
            point("pc000000", 10.0, 10.0),
        ],
        vec![
            line("na000001", "pa000001", "pa000002"),
            line("na000002", "pa000002", "pa000003"),
            line("na000003", "pa000004", "pa000001"),
        ],
        vec![arc("aa000001", "pc000000", "pa000003", "pa000004")],
    ));
    arched.regions = vec![region(
        &["na000001", "na000002", "aa000001", "na000003"],
        &[],
    )];
    let got = fill_area(&region_fills(&arched, HAND, SKETCH_CHORD_TOLERANCE));
    let want = 200.0 + 50.0 * PI;
    assert!(
        got <= want + 1e-9 && got >= want - PI * 10.0 * stray,
        "{got}"
    );
}

/// Whether `p` lies inside any of `triangles` (or on one's edge).
fn covered(triangles: &[[[f64; 2]; 3]], p: [f64; 2]) -> bool {
    triangles.iter().any(|&[a, b, c]| {
        let side = |u: [f64; 2], v: [f64; 2]| {
            (v[0] - u[0]) * (p[1] - u[1]) - (v[1] - u[1]) * (p[0] - u[0])
        };
        let (d1, d2, d3) = (side(a, b), side(b, c), side(c, a));
        (d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0) || (d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0)
    })
}

#[test]
fn a_profiles_hole_is_left_uncovered_and_the_rest_covered() {
    let outer = vec![[0.0, 0.0], [40.0, 0.0], [40.0, 20.0], [0.0, 20.0]];
    let hole = vec![[10.0, 5.0], [20.0, 5.0], [20.0, 15.0], [10.0, 15.0]];
    let triangles = tessellate(&[outer, hole]).unwrap();
    // The hole's middle, and a point just inside its edge, uncovered.
    assert!(!covered(&triangles, [15.0, 10.0]));
    assert!(!covered(&triangles, [10.5, 14.5]));
    // Around it, and either side, covered.
    for p in [
        [5.0, 5.0],
        [30.0, 10.0],
        [15.0, 2.0],
        [15.0, 18.0],
        [9.5, 10.0],
    ] {
        assert!(covered(&triangles, p), "{p:?}");
    }
}

/// A 100 × 60 plate with 60 holes of 5 mm across, in ten columns and six rows 10 mm apart,
/// its profile one region.
fn plate() -> SketchRead {
    let mut points = vec![
        point("pa000001", 0.0, 0.0),
        point("pa000002", 100.0, 0.0),
        point("pa000003", 100.0, 60.0),
        point("pa000004", 0.0, 60.0),
    ];
    let mut circles = Vec::new();
    for row in 0..6 {
        for column in 0..10 {
            let n = row * 10 + column;
            points.push(point(
                &format!("pc{n:06}"),
                5.0 + 10.0 * column as f64,
                5.0 + 10.0 * row as f64,
            ));
            circles.push(SketchCircle {
                id: id(&format!("cc{n:06}")),
                center: id(&format!("pc{n:06}")),
                radius: Coordinate::new(2.5).unwrap(),
                construction: false,
            });
        }
    }
    let mut sketch = rectangle(Vec::new());
    sketch.points = points;
    sketch.circles = circles;
    let holes: Vec<Vec<ElementId>> = (0..60).map(|n| vec![id(&format!("cc{n:06}"))]).collect();
    let mut read = under(sketch);
    read.regions = vec![Region {
        outer: ["na000001", "na000002", "na000003", "na000004"]
            .iter()
            .map(|n| id(n))
            .collect(),
        holes,
    }];
    read
}

#[test]
fn a_plate_with_sixty_holes_fills_less_its_holes_and_steady_frames_reuse_the_drawing_and_fill() {
    // The plate's area less 60 holes of radius 2.5, 6,000 − 375π: each hole's chords stray
    // inward from its circle, so the fill covers a little more than that, by at most the
    // holes' perimeters times the chord bound.
    let read = plate();
    let fills = region_fills(&read, Scale(0.125), SKETCH_CHORD_TOLERANCE);
    assert_eq!(fills.len(), 1);
    let want = 6000.0 - 60.0 * PI * 6.25;
    let stray = 0.25 * 0.125;
    let got = fill_area(&fills);
    assert!(
        got >= want && got <= want + 60.0 * TAU * 2.5 * stray,
        "{got} for {want}"
    );
    // Steady frames, nothing changing, draw what the first worked out: one drawing, one
    // fill. A selection redraws, and reuses the fill; a zoom within a power of two
    // redraws, and reuses it; past one, it's worked out again.
    let mut mode = mode_on(read);
    let at = |zoom: f32| {
        move |p: [f64; 2]| {
            Some(pos2(
                (100.0 + f64::from(zoom) * p[0]) as f32,
                (500.0 - f64::from(zoom) * p[1]) as f32,
            ))
        }
    };
    let back = |zoom: f32| {
        move |p: Pos2| {
            Some([
                (f64::from(p.x) - 100.0) / f64::from(zoom),
                (500.0 - f64::from(p.y)) / f64::from(zoom),
            ])
        }
    };
    let frames = |mode: &mut SketchMode, zoom: f32, n: usize| {
        let (project, to_plane) = (at(zoom), back(zoom));
        with_painter(|painter| {
            for _ in 0..n {
                let input = FrameInput {
                    events: &[],
                    pointer: None,
                    project: &project,
                    to_plane: &to_plane,
                };
                mode.frame(painter, &input);
            }
        });
    };
    frames(&mut mode, 10.0, 20);
    assert_eq!((mode.drawings, mode.fill_builds), (1, 1));
    mode.click(
        Some(Hit {
            id: id("na000001"),
            kind: ElementKind::Line,
        }),
        false,
    );
    frames(&mut mode, 10.0, 5);
    assert_eq!((mode.drawings, mode.fill_builds), (2, 1));
    // 10 points a mm is 0.1 mm a point, in [1/16, 1/8); 12 is 0.0833, too.
    frames(&mut mode, 12.0, 5);
    assert_eq!((mode.drawings, mode.fill_builds), (3, 1));
    frames(&mut mode, 20.0, 5);
    assert_eq!((mode.drawings, mode.fill_builds), (4, 2));
}

#[test]
fn the_yz_planes_words_and_axes() {
    let yz = silico_core::kernel::SketchPlane::Yz;
    assert_eq!(plane_words(Some(&yz)), ("YZ", ["y", "z"]));
    assert_eq!(
        draw::axis_colours(Some(&yz)),
        [color32(AXIS_Y), color32(AXIS_Z)]
    );
}

/// WCAG's contrast ratio of two colours.
fn contrast(a: egui::Color32, b: egui::Color32) -> f64 {
    let luminance = |c: egui::Color32| {
        let channel = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                libm::pow((v + 0.055) / 1.055, 2.4)
            }
        };
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    };
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[test]
fn the_states_words_stand_out_from_the_panels_they_stand_on() {
    let mut read = under(rectangle(Vec::new()));
    let mut reads = Vec::new();
    for state in SketchState::ALL {
        read.states.state = state;
        reads.push(read.clone());
    }
    let mut empty = read.clone();
    empty.members = sketch_of(Vec::new(), Vec::new(), Vec::new());
    reads.push(empty);
    for read in &reads {
        let shown = state_shown(read);
        let ratio = contrast(shown.colour, FLOATING_BG);
        assert!(ratio >= 3.0, "{}: {ratio}", shown.words);
    }
}

#[test]
fn a_fills_scale_is_the_power_of_two_at_or_below_the_views() {
    assert_eq!(draw::fill_scale(Scale(0.1)), (-4, Scale(0.0625)));
    assert_eq!(draw::fill_scale(Scale(0.125)), (-3, Scale(0.125)));
    for odd in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(draw::fill_scale(Scale(odd)), (0, Scale(1.0)), "{odd}");
    }
}

/// A projection to the screen, and its way back.
type Projection = Box<dyn Fn([f64; 2]) -> Option<Pos2>>;
type Unprojection = Box<dyn Fn(Pos2) -> Option<[f64; 2]>>;

/// A view of the plane at `zoom` points a millimetre, panned `pan` points right: the
/// projection, and its way back.
fn zoomed(zoom: f64, pan: f64) -> (Projection, Unprojection) {
    (
        Box::new(move |p: [f64; 2]| {
            Some(pos2(
                (100.0 + pan + zoom * p[0]) as f32,
                (500.0 - zoom * p[1]) as f32,
            ))
        }),
        Box::new(move |p: Pos2| {
            Some([
                (f64::from(p.x) - 100.0 - pan) / zoom,
                (500.0 - f64::from(p.y)) / zoom,
            ])
        }),
    )
}

/// `n` frames of `mode` seen at `zoom`, panned `pan`.
fn frames_at(mode: &mut SketchMode, zoom: f64, pan: f64, n: usize) {
    let (project, to_plane) = zoomed(zoom, pan);
    with_painter(|painter| {
        for _ in 0..n {
            let input = FrameInput {
                events: &[],
                pointer: None,
                project: &*project,
                to_plane: &*to_plane,
            };
            mode.frame(painter, &input);
        }
    });
}

/// The area of the last frame's fill, in square millimetres at `zoom`.
fn drawn_fill(mode: &SketchMode, zoom: f64) -> Option<f64> {
    mode.last_drawn().iter().find_map(|d| match d {
        Drawn::Fill { triangles, .. } => Some(f64::from(area(triangles)) / (zoom * zoom)),
        _ => None,
    })
}

#[test]
fn the_plates_cache_is_keyed_on_the_read_the_scale_the_view_and_whats_shown() {
    let mut mode = mode_on(plate());
    // At 10 points a millimetre, 0.1 mm a point: the fill worked out at 1/16.
    frames_at(&mut mode, 10.0, 0.0, 3);
    assert_eq!((mode.drawings, mode.fill_builds), (1, 1));
    let cached = mode.fill_at(-4).unwrap().to_vec();
    let read = mode.drawable().unwrap().clone();
    assert_eq!(
        cached,
        region_fills(&read, Scale(0.0625), SKETCH_CHORD_TOLERANCE)
    );
    // To 20 (another power of two) and back, and again: the two kept, neither worked out
    // again.
    frames_at(&mut mode, 20.0, 0.0, 2);
    frames_at(&mut mode, 10.0, 0.0, 2);
    frames_at(&mut mode, 20.0, 0.0, 2);
    assert_eq!((mode.drawings, mode.fill_builds), (4, 2));
    frames_at(&mut mode, 10.0, 0.0, 2);
    let before = drawn_fill(&mode, 10.0).unwrap();
    assert_eq!((mode.drawings, mode.fill_builds), (5, 2));
    // A new read, one hole's circle gone: both worked out again; the fill grows by the
    // hole as its outline draws it: at 1/16, r / scale = 40 points, 2·acos(1 − 0.25/40) =
    // 0.2238 rad a chord, 29 to a turn, an inscribed 29-gon of 14.5 · 6.25 · sin(2π/29)
    // (to the screen triangles' f32 rounding).
    let mut fewer = read.clone();
    fewer
        .members
        .circles
        .retain(|c| c.id.as_str() != "cc000000");
    fewer.regions[0]
        .holes
        .retain(|h| h[0].as_str() != "cc000000");
    mode.took(Read {
        model: None,
        result: Ok(fewer),
    });
    frames_at(&mut mode, 10.0, 0.0, 2);
    assert_eq!((mode.drawings, mode.fill_builds), (6, 3));
    let grown = drawn_fill(&mode, 10.0).unwrap() - before;
    let gon = 14.5 * 6.25 * libm::sin(TAU / 29.0);
    assert!((grown - gon).abs() < 2e-3, "{grown} for {gon}");
    // Show Profile off: drawn again, the fill kept but not drawn.
    mode.settings.show_profile = false;
    frames_at(&mut mode, 10.0, 0.0, 2);
    assert_eq!((mode.drawings, mode.fill_builds), (7, 3));
    assert_eq!(drawn_fill(&mode, 10.0), None);
    mode.settings.show_profile = true;
    frames_at(&mut mode, 10.0, 0.0, 1);
    // A pan of 50 points at the same zoom: drawn again, the fill kept.
    frames_at(&mut mode, 10.0, 50.0, 2);
    assert_eq!((mode.drawings, mode.fill_builds), (9, 3));
    assert!(drawn_fill(&mode, 10.0).is_some());
}

#[test]
fn the_plate_zoomed_far_in_fills_at_its_outlines_fidelity() {
    // 80 points a millimetre: the fill at 1/128, each hole r / scale = 320 points, so
    // 2·acos(1 − 0.25/320) = 0.0791 rad a chord, 80 to a turn: the plate less 60 inscribed
    // 80-gons, 40 · 6.25 · sin(π/40) each.
    let mut mode = mode_on(plate());
    frames_at(&mut mode, 80.0, 0.0, 2);
    let fills = mode.fill_at(-7).unwrap();
    assert_eq!(fills.len(), 1);
    let gon = 40.0 * 6.25 * libm::sin(PI / 40.0);
    let got = fill_area(fills);
    assert!((got - (6000.0 - 60.0 * gon)).abs() < 1e-2, "{got}");
    assert!(drawn_fill(&mode, 80.0).is_some());
}

#[test]
fn three_holes_in_a_row_are_each_left_uncovered() {
    // A tall slot, A, between two small holes, B and C: none filled over.
    let outer = vec![[0.0, 0.0], [40.0, 0.0], [40.0, 20.0], [0.0, 20.0]];
    let square = |x0: f64, x1: f64, y0: f64, y1: f64| vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    let c = square(30.0, 31.0, 9.5, 10.5);
    let a = square(24.0, 25.0, 1.0, 19.0);
    let b = square(20.0, 21.0, 9.5, 10.5);
    let triangles = tessellate(&[outer, c, a, b]).unwrap();
    for p in [
        [24.5, 5.0],
        [24.5, 10.2],
        [24.5, 15.0],
        [20.5, 10.0],
        [30.5, 10.0],
    ] {
        assert!(!covered(&triangles, p), "{p:?} covered");
    }
    for p in [[22.5, 10.0], [27.5, 10.0]] {
        assert!(covered(&triangles, p), "{p:?} uncovered");
    }
    // 800 less C's 1, A's 18, and B's 1.
    let total: f64 = triangles.iter().map(|t| signed_area(&t[..]).abs()).sum();
    assert!((total - 780.0).abs() < 1e-3, "{total}");
}

/// A `size` × `size` grid of holes of 5 mm across, 10 mm apart, in a plate `10 · size`
/// square, its profile one region; and, if `round`, the outer boundary a circle about the
/// plate's middle instead, of radius `round`.
fn holed(size: usize, round: Option<f64>) -> SketchRead {
    let side = 10.0 * size as f64;
    let mut points = vec![
        point("pa000001", 0.0, 0.0),
        point("pa000002", side, 0.0),
        point("pa000003", side, side),
        point("pa000004", 0.0, side),
        point("pm000000", side / 2.0, side / 2.0),
    ];
    let mut circles = Vec::new();
    for n in 0..size * size {
        let (row, column) = (n / size, n % size);
        points.push(point(
            &format!("pc{n:06}"),
            5.0 + 10.0 * column as f64,
            5.0 + 10.0 * row as f64,
        ));
        circles.push(SketchCircle {
            id: id(&format!("cc{n:06}")),
            center: id(&format!("pc{n:06}")),
            radius: Coordinate::new(2.5).unwrap(),
            construction: false,
        });
    }
    let outer: Vec<ElementId> = match round {
        Some(radius) => {
            circles.push(SketchCircle {
                id: id("cr000000"),
                center: id("pm000000"),
                radius: Coordinate::new(radius).unwrap(),
                construction: false,
            });
            vec![id("cr000000")]
        }
        None => ["na000001", "na000002", "na000003", "na000004"]
            .iter()
            .map(|n| id(n))
            .collect(),
    };
    let mut sketch = rectangle(Vec::new());
    sketch.lines = vec![
        line("na000001", "pa000001", "pa000002"),
        line("na000002", "pa000002", "pa000003"),
        line("na000003", "pa000003", "pa000004"),
        line("na000004", "pa000004", "pa000001"),
    ];
    sketch.points = points;
    sketch.circles = circles;
    let mut read = under(sketch);
    read.regions = vec![Region {
        outer,
        holes: (0..size * size)
            .map(|n| vec![id(&format!("cc{n:06}"))])
            .collect(),
    }];
    read
}

#[test]
fn a_plate_of_225_holes_fills_within_the_chord_bound() {
    // Each hole at 1/16 mm a point is a 29-gon inscribed in it (r / scale = 40 points,
    // 2·acos(1 − 0.25/40) = 0.2238 rad a chord): the fill exceeds 150² − 225 · π · 6.25 by
    // at most the holes' perimeters times the chords' sagitta, 0.25 / 16 mm.
    let read = holed(15, None);
    let at = draw::fill_scale(Scale(0.1)).1;
    let fills = region_fills(&read, at, SKETCH_CHORD_TOLERANCE);
    assert_eq!(fills.len(), 1);
    let want = 150.0 * 150.0 - 225.0 * PI * 6.25;
    let got = fill_area(&fills);
    let bound = 225.0 * TAU * 2.5 * 0.25 * at.0;
    assert!(got >= want - 1e-2 && got <= want + bound, "{got}");
}

#[test]
fn a_close_fill_keeps_outside_each_hole_and_inside_a_round_outer_boundary() {
    // The 60-hole grid inside a circle of radius 70 about (50, 50), at 1/128: every fill
    // vertex at a hole's radius or past it, and within the outer circle.
    let mut read = holed(10, Some(70.0));
    read.members.circles.retain(|c| {
        c.id.as_str() == "cr000000" || c.id.as_str()[2..].parse::<usize>().is_ok_and(|n| n < 60)
    });
    read.regions[0].holes.truncate(60);
    let fills = region_fills(&read, Scale(1.0 / 128.0), SKETCH_CHORD_TOLERANCE);
    let vertices: Vec<[f64; 2]> = fills[0].triangles.iter().flatten().copied().collect();
    for v in &vertices {
        assert!(
            libm::hypot(v[0] - 50.0, v[1] - 50.0) <= 70.0 + 1e-3,
            "{v:?} outside"
        );
        for n in 0..60 {
            let centre = [5.0 + 10.0 * (n % 10) as f64, 5.0 + 10.0 * (n / 10) as f64];
            assert!(
                libm::hypot(v[0] - centre[0], v[1] - centre[1]) >= 2.5 - 1e-3,
                "{v:?} in {n}"
            );
        }
    }
}

#[test]
fn a_fill_meets_its_outline_vertex_for_vertex_at_every_zoom() {
    let mut read = under(sketch_of(
        vec![point("pa000001", 0.0, 0.0)],
        Vec::new(),
        Vec::new(),
    ));
    read.members.circles.push(SketchCircle {
        id: id("ca000001"),
        center: id("pa000001"),
        radius: Coordinate::new(10.0).unwrap(),
        construction: false,
    });
    read.regions = vec![region(&["ca000001"], &[])];
    // At 10 points a millimetre (the fill at 1/16, 160 points of radius: 57 chords), at 80
    // (1/128, 1,280 points: 159), and at 10 with a chord tolerance of 0.1 point, the view's
    // and the fill's alike (2·acos(1 − 0.1/160) = 0.0707 rad a chord: 89).
    for (zoom, tolerance, chords) in [
        (10.0, SKETCH_CHORD_TOLERANCE, 57),
        (80.0, SKETCH_CHORD_TOLERANCE, 159),
        (10.0, 0.1, 89),
    ] {
        let (project, _) = zoomed(zoom, 0.0);
        let view = View {
            project: &*project,
            scale: Scale(1.0 / zoom),
            visible: None,
            chord_tolerance: tolerance,
        };
        let fills = region_fills(&read, draw::fill_scale(view.scale).1, tolerance);
        let drawn = drawing(&read, &view, Shown::default(), &Default::default(), &fills);
        let key = |p: Pos2| ((p.x * 16.0).round() as i64, (p.y * 16.0).round() as i64);
        let outline: std::collections::BTreeSet<(i64, i64)> = drawn
            .iter()
            .find_map(|d| match d {
                Drawn::Stroke {
                    of: Some(of),
                    points,
                    ..
                } if of.as_str() == "ca000001" => Some(points.iter().map(|&p| key(p)).collect()),
                _ => None,
            })
            .unwrap();
        let filled: std::collections::BTreeSet<(i64, i64)> = drawn
            .iter()
            .find_map(|d| match d {
                Drawn::Fill { triangles, .. } => {
                    Some(triangles.iter().flatten().map(|&p| key(p)).collect())
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(outline.len(), chords, "at {zoom}");
        assert_eq!(outline, filled, "at {zoom}");
    }
}

/// `columns` × `rows` holes of 5 mm across at `pitch`, in a plate just holding them, its
/// profile one region.
fn pitched(columns: usize, rows: usize, pitch: f64) -> SketchRead {
    let (width, height) = (pitch * columns as f64, pitch * rows as f64);
    let mut points = vec![
        point("pa000001", 0.0, 0.0),
        point("pa000002", width, 0.0),
        point("pa000003", width, height),
        point("pa000004", 0.0, height),
    ];
    let mut circles = Vec::new();
    for n in 0..columns * rows {
        let (row, column) = (n / columns, n % columns);
        points.push(point(
            &format!("pc{n:06}"),
            pitch / 2.0 + pitch * column as f64,
            pitch / 2.0 + pitch * row as f64,
        ));
        circles.push(SketchCircle {
            id: id(&format!("cc{n:06}")),
            center: id(&format!("pc{n:06}")),
            radius: Coordinate::new(2.5).unwrap(),
            construction: false,
        });
    }
    let mut sketch = rectangle(Vec::new());
    sketch.lines = vec![
        line("na000001", "pa000001", "pa000002"),
        line("na000002", "pa000002", "pa000003"),
        line("na000003", "pa000003", "pa000004"),
        line("na000004", "pa000004", "pa000001"),
    ];
    sketch.points = points;
    sketch.circles = circles;
    let mut read = under(sketch);
    read.regions = vec![Region {
        outer: ["na000001", "na000002", "na000003", "na000004"]
            .iter()
            .map(|n| id(n))
            .collect(),
        holes: (0..columns * rows)
            .map(|n| vec![id(&format!("cc{n:06}"))])
            .collect(),
    }];
    read
}

#[test]
fn a_perforated_plate_at_a_close_pitch_covers_no_hole_and_fills_within_the_chord_bound() {
    // 300 holes of r 2.5 on a 7 mm pitch, 2 mm of web between them, in a 140 × 105 plate.
    let read = pitched(20, 15, 7.0);
    let at = draw::fill_scale(Scale(0.1)).1;
    let fills = region_fills(&read, at, SKETCH_CHORD_TOLERANCE);
    assert_eq!(fills.len(), 1);
    let triangles = &fills[0].triangles;
    // Each hole inscribed (29 chords at 1/16): the fill exceeds the true area by at most the
    // holes' perimeters times the chords' sagitta.
    let want = 140.0 * 105.0 - 300.0 * PI * 6.25;
    let got = fill_area(&fills);
    assert!(
        got >= want - 1e-2 && got <= want + 300.0 * TAU * 2.5 * 0.25 * at.0,
        "{got}"
    );
    // No hole covered: 16 points at r 2.4 round each, inside its inscribed 29-gon (whose
    // chords come no nearer its centre than 2.5 · cos(π/29) = 2.485). Triangles found by a
    // grid of the pitch's cells.
    let cell = |p: [f64; 2]| ((p[0] / 7.0).floor() as i64, (p[1] / 7.0).floor() as i64);
    let mut grid: BTreeMap<(i64, i64), Vec<[[f64; 2]; 3]>> = BTreeMap::new();
    for t in triangles {
        let (lo, hi) = (
            [
                t[0][0].min(t[1][0]).min(t[2][0]),
                t[0][1].min(t[1][1]).min(t[2][1]),
            ],
            [
                t[0][0].max(t[1][0]).max(t[2][0]),
                t[0][1].max(t[1][1]).max(t[2][1]),
            ],
        );
        let (a, b) = (cell(lo), cell(hi));
        for x in a.0..=b.0 {
            for y in a.1..=b.1 {
                grid.entry((x, y)).or_default().push(*t);
            }
        }
    }
    for n in 0..300 {
        let centre = [3.5 + 7.0 * (n % 20) as f64, 3.5 + 7.0 * (n / 20) as f64];
        for k in 0..16 {
            let p = add(centre, mul(direction(TAU * k as f64 / 16.0), 2.4));
            let near = grid.get(&cell(p)).map_or(&[][..], Vec::as_slice);
            assert!(!covered(near, p), "hole {n} covered at {p:?}");
        }
        // And the web between holes, 3.5 mm off a centre, covered (but past the plate's end).
        let web = add(centre, [3.5, 0.0]);
        if web[0] < 140.0 {
            let near = grid.get(&cell(web)).map_or(&[][..], Vec::as_slice);
            assert!(covered(near, web), "web at {web:?}");
        }
    }
}

#[test]
fn a_slot_hole_and_a_concave_notch_fill_exactly_within_the_chord_bound() {
    // A 60 × 30 plate whose top has a notch: a half circle of radius 10 about (30, 30),
    // bulging down into it. A slot hole: two lines and two half circles of radius 3 about
    // (15, 8) and (25, 8), its lines written backwards.
    let sketch = Sketch {
        plane: OriginPlane::Xy.into(),
        points: vec![
            point("pa000001", 0.0, 0.0),
            point("pa000002", 60.0, 0.0),
            point("pa000003", 60.0, 30.0),
            point("pa000004", 40.0, 30.0),
            point("pa000005", 20.0, 30.0),
            point("pa000006", 0.0, 30.0),
            point("pn000000", 30.0, 30.0),
            point("pq000001", 15.0, 8.0),
            point("pq000002", 25.0, 8.0),
            point("ps000001", 15.0, 5.0),
            point("ps000002", 25.0, 5.0),
            point("ps000003", 25.0, 11.0),
            point("ps000004", 15.0, 11.0),
        ],
        lines: vec![
            line("na000001", "pa000001", "pa000002"),
            line("na000002", "pa000002", "pa000003"),
            line("na000003", "pa000003", "pa000004"),
            line("na000004", "pa000005", "pa000006"),
            line("na000005", "pa000006", "pa000001"),
            line("nm000001", "ps000002", "ps000001"),
            line("nm000002", "ps000004", "ps000003"),
        ],
        circles: Vec::new(),
        arcs: vec![
            // Counter-clockwise from (20, 30) by the bottom to (40, 30).
            arc("an000000", "pn000000", "pa000005", "pa000004"),
            arc("ab000001", "pq000001", "ps000004", "ps000001"),
            arc("ab000002", "pq000002", "ps000002", "ps000003"),
        ],
        constraints: Vec::new(),
    };
    let mut read = under(sketch);
    read.regions = vec![region(
        &[
            "na000001", "na000002", "na000003", "an000000", "na000004", "na000005",
        ],
        &[&["nm000001", "ab000001", "nm000002", "ab000002"]],
    )];
    let at = draw::fill_scale(HAND).1;
    let fills = region_fills(&read, at, SKETCH_CHORD_TOLERANCE);
    assert_eq!(fills.len(), 1);
    // 1,800 less the notch's half disc, 50π, and the slot, 60 + 9π. Both inscribed, so the
    // fill exceeds it, by at most their arcs' lengths (10π and 6π) times the sagitta.
    let want = 1800.0 - 50.0 * PI - (60.0 + 9.0 * PI);
    let got = fill_area(&fills);
    assert!(
        got >= want - 1e-2 && got <= want + 16.0 * PI * 0.25 * at.0,
        "{got} for {want}"
    );
    let triangles = &fills[0].triangles;
    // The slot's inside, and just inside the notch's circle, uncovered; just below it, and
    // the plate's corner, covered.
    for p in [
        [20.0, 8.0],
        [13.0, 8.0],
        [27.0, 8.0],
        [30.0, 20.5],
        [30.0, 29.0],
    ] {
        assert!(!covered(triangles, p), "{p:?} covered");
    }
    for p in [[30.0, 19.5], [5.0, 5.0], [20.0, 4.0], [20.0, 12.0]] {
        assert!(covered(triangles, p), "{p:?} uncovered");
    }
}

#[test]
fn the_same_read_fills_with_the_same_triangles_every_time() {
    let read = plate();
    let first = region_fills(&read, Scale(0.0625), SKETCH_CHORD_TOLERANCE);
    for _ in 0..3 {
        assert_eq!(
            region_fills(&read, Scale(0.0625), SKETCH_CHORD_TOLERANCE),
            first
        );
    }
}

/// Every text `draw` lays out in a context showing egui's light theme, after two passes so
/// its layout has settled: each text and its colour.
fn texts_on_light(mut draw: impl FnMut(&mut egui::Ui)) -> Vec<(String, egui::Color32)> {
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::theme::fonts());
    ctx.set_theme(egui::Theme::Light);
    let mut texts = Vec::new();
    for pass in 0..2 {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| draw(ui));
        output.textures_delta.clear();
        if pass == 1 {
            let mut shapes: Vec<egui::Shape> = output.shapes.into_iter().map(|c| c.shape).collect();
            while let Some(shape) = shapes.pop() {
                match shape {
                    egui::Shape::Vec(inner) => shapes.extend(inner),
                    egui::Shape::Text(text) => {
                        for section in &text.galley.job.sections {
                            let colour = match (text.override_text_color, section.format.color) {
                                (Some(colour), _) => colour,
                                (None, c) if c == egui::Color32::PLACEHOLDER => text.fallback_color,
                                (None, c) => c,
                            };
                            let range = section.byte_range.start.0..section.byte_range.end.0;
                            let words = text.galley.job.text[range].to_owned();
                            texts.push((words, colour));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    texts
}

#[test]
fn the_bar_and_the_palette_are_drawn_dark_on_a_window_showing_the_light_theme() {
    let mut read = under(rectangle(Vec::new()));
    read.states.dofs = 4;
    let mode = mode_on(read);
    let bar = mode.bar_view("Sketch1", true);
    let view = mode.palette_view(silico_core::command::Revision::OPENED);
    let mut settings = bar::Settings::default();
    let mut texts = texts_on_light(|ui| {
        bar::sketch_bar(ui, &bar);
    });
    texts.extend(texts_on_light(|ui| bar::palette(ui, &view, &mut settings)));
    assert!(texts.iter().any(|(words, _)| words == "Cancel"));
    assert!(texts.iter().any(|(words, _)| words == "Profile"));
    // Every text but Look At, which is off (faded on purpose till the viewport's camera),
    // stands out from the panels' dark fill.
    for (words, colour) in texts.iter().filter(|(words, _)| words != "Look At") {
        let ratio = contrast(*colour, FLOATING_BG);
        assert!(ratio >= 3.0, "{words:?} in {colour:?}: {ratio}");
    }
}

#[test]
fn a_profile_far_from_the_origin_fills_at_its_own_precision() {
    // A 10 mm square about (999 990, 999 990), a kilometre out, less a 29-gon of radius 2.5
    // about its middle. In f32 there, a coordinate is a sixteenth of a millimetre apart; the
    // tessellation, taken about the loops' middle, keeps each vertex where it was.
    let (cx, cy) = (999_990.0, 999_990.0);
    let square = vec![
        [cx - 5.0, cy - 5.0],
        [cx + 5.0, cy - 5.0],
        [cx + 5.0, cy + 5.0],
        [cx - 5.0, cy + 5.0],
    ];
    let hole: Vec<[f64; 2]> = (0..29)
        .map(|k| add([cx, cy], mul(direction(TAU * k as f64 / 29.0), 2.5)))
        .collect();
    let loops = [square, hole];
    let triangles = tessellate(&loops).unwrap();
    for v in triangles.iter().flatten() {
        let off = loops
            .iter()
            .flatten()
            .map(|p| libm::hypot(v[0] - p[0], v[1] - p[1]))
            .fold(f64::INFINITY, f64::min);
        assert!(off <= 1e-5, "{v:?} is {off} from the loops' points");
    }
    let area: f64 = triangles.iter().map(|t| signed_area(&t[..]).abs()).sum();
    let want = 100.0 - 14.5 * 6.25 * libm::sin(TAU / 29.0);
    assert!((area - want).abs() < 1e-3, "{area} for {want}");
}

#[test]
fn a_loop_whose_first_curve_runs_against_it_fills_whole() {
    // The rectangle with its first side, na000001, written from pa000002 to pa000001.
    let mut sketch = rectangle(Vec::new());
    sketch.lines[0] = line("na000001", "pa000002", "pa000001");
    let mut read = under(sketch);
    read.regions = vec![region(
        &["na000001", "na000002", "na000003", "na000004"],
        &[],
    )];
    let fills = region_fills(&read, HAND, SKETCH_CHORD_TOLERANCE);
    assert!(
        (fill_area(&fills) - 800.0).abs() < 1e-3,
        "{}",
        fill_area(&fills)
    );
    // A loop that starts with an arc walked against its own way: the upper half circle of
    // radius 10 about the origin, counter-clockwise from (10, 0) to (−10, 0), walked back from
    // (−10, 0) to (10, 0), then down, across, and up a 20 × 10 rectangle below it. 200 +
    // 50π, less at most its 10π of arc times the chords' sagitta.
    let arched = sketch_of(
        vec![
            point("pc000000", 0.0, 0.0),
            point("pa000001", 10.0, 0.0),
            point("pa000002", -10.0, 0.0),
            point("pa000003", 10.0, -10.0),
            point("pa000004", -10.0, -10.0),
        ],
        vec![
            line("na000001", "pa000001", "pa000003"),
            line("na000002", "pa000003", "pa000004"),
            line("na000003", "pa000004", "pa000002"),
        ],
        vec![arc("aa000001", "pc000000", "pa000001", "pa000002")],
    );
    let mut read = under(arched);
    read.regions = vec![region(
        &["aa000001", "na000001", "na000002", "na000003"],
        &[],
    )];
    let at = draw::fill_scale(HAND).1;
    let got = fill_area(&region_fills(&read, at, SKETCH_CHORD_TOLERANCE));
    let want = 200.0 + 50.0 * PI;
    assert!(
        got <= want + 1e-3 && got >= want - 10.0 * PI * 0.25 * at.0,
        "{got} for {want}"
    );
}

/// Counts the warnings logged while it's the subscriber.
struct Warnings(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl tracing::Subscriber for Warnings {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if *event.metadata().level() == tracing::Level::WARN {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[test]
fn a_region_past_the_vertex_bound_is_left_unfilled_and_said() {
    // 200 holes at a millionth of a millimetre a point: each circle at the most segments a
    // curve takes, 1,024, so 200 · 1,024 + 4 = 204,804 vertices, past 200,000.
    let read = pitched(20, 10, 7.0);
    let warnings = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fills = tracing::subscriber::with_default(Warnings(warnings.clone()), || {
        region_fills(&read, Scale(1e-6), SKETCH_CHORD_TOLERANCE)
    });
    assert!(fills.is_empty());
    assert_eq!(warnings.load(std::sync::atomic::Ordering::SeqCst), 1);
    // 195 holes, 195 · 1,024 + 4 = 199,684 vertices, within it: filled, said nothing of.
    let fewer = pitched(15, 13, 7.0);
    let warnings = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let filled = tracing::subscriber::with_default(Warnings(warnings.clone()), || {
        region_fills(&fewer, Scale(1e-6), SKETCH_CHORD_TOLERANCE)
    });
    assert_eq!(filled.len(), 1);
    assert_eq!(warnings.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn a_region_whose_loop_doesnt_run_end_to_end_is_left_unfilled_and_said() {
    // The rectangle's bottom and top sides alone: they meet at no end.
    let mut read = under(rectangle(Vec::new()));
    read.regions = vec![region(&["na000001", "na000003"], &[])];
    let warnings = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fills = tracing::subscriber::with_default(Warnings(warnings.clone()), || {
        region_fills(&read, HAND, SKETCH_CHORD_TOLERANCE)
    });
    assert!(fills.is_empty());
    assert_eq!(warnings.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn the_palette_says_how_the_tool_on_is_used_and_how_trim_trims_each_way() {
    // The palette's Tool section, under the tool's name: how it's used, in the prototype's
    // words for the line, and the help's for trim's two ways (docs/ux/solidworks.md, §4).
    let mut mode = mode_on(under(rectangle(Vec::new())));
    let words = |mode: &SketchMode, settings: &mut bar::Settings| -> Vec<String> {
        let view = mode.palette_view(silico_core::command::Revision::OPENED);
        texts_on_light(|ui| bar::palette(ui, &view, settings))
            .into_iter()
            .map(|(words, _)| words)
            .collect()
    };
    let mut settings = bar::Settings::default();
    mode.tools.choose(Some(tools::Choice::Draw(
        silico_core::sketch_tools::ToolKind::Line,
    )));
    let said = words(&mode, &mut settings);
    assert!(
        said.iter()
            .any(|w| w == "Click, click: a chain. Double-click or Enter ends it. Drag: one line."),
        "{said:?}"
    );
    mode.tools.choose(Some(tools::Choice::Trim));
    let said = words(&mode, &mut settings);
    assert!(
        said.iter()
            .any(|w| w == "Drag across curves to trim them. Drag along one to extend it."),
        "{said:?}"
    );
    settings.trim = tools::TrimMode::Closest;
    let said = words(&mode, &mut settings);
    assert!(
        said.iter()
            .any(|w| w
                == "Click a curve to trim it to its closest cuts. Drag from one to extend it."),
        "{said:?}"
    );
}

// The mutants #351's run left alive in `draw`: each placement, guard, and shape worked by
// hand.

/// Where each dimension of `read` stands, in the sketch's millimetres, at [`HAND`]'s scale:
/// 2.6 mm off (26 points), its extension lines from 0.4 mm off the geometry to 0.5 mm past
/// the dimension's line.
fn dimensions(read: &SketchRead) -> Vec<draw::PlacedDimension> {
    draw::placed(read, &draw::Geometry::of(read), HAND).dimensions
}

fn glyphs(read: &SketchRead) -> Vec<draw::PlacedGlyph> {
    draw::placed(read, &draw::Geometry::of(read), HAND).glyphs
}

fn same(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
}

/// That `got` is `want`, a dimension's lines, arrows, text, and the text's place.
fn assert_placed(
    got: &draw::PlacedDimension,
    lines: &[&[[f64; 2]]],
    arrows: &[([f64; 2], [f64; 2])],
    text: &str,
    text_at: [f64; 2],
) {
    assert_eq!(got.lines.len(), lines.len(), "{got:?}");
    for (got_line, want) in got.lines.iter().zip(lines) {
        assert_eq!(got_line.len(), want.len(), "{got:?}");
        assert!(
            got_line.iter().zip(*want).all(|(p, q)| same(*p, *q)),
            "{got_line:?} for {want:?}"
        );
    }
    assert_eq!(got.arrows.len(), arrows.len(), "{got:?}");
    for (got_arrow, want) in got.arrows.iter().zip(arrows) {
        assert!(
            same(got_arrow.0, want.0) && same(got_arrow.1, want.1),
            "{got_arrow:?} for {want:?}"
        );
    }
    assert_eq!(got.text, text);
    assert!(same(got.text_at, text_at), "{:?}", got.text_at);
}

#[test]
fn a_distance_stands_square_to_its_points_off_the_side_its_normal_takes_at_the_middle() {
    // From (0, 0) to (40, 20), through the sketch's middle (20, 10): t = (2, 1)/√5, its
    // normal (−t.y, t.x) = (−1, 2)/√5 kept, the middle neither side of it.
    let read = under(rectangle(vec![constraint(
        "ka000001",
        Form::Distance {
            a: element("pa000001"),
            b: element("pa000003"),
            value: mm("44.72"),
            driving: true,
        },
    )]));
    let s = 5f64.sqrt();
    let (t, n) = ([2.0 / s, 1.0 / s], [-1.0 / s, 2.0 / s]);
    let at = |p: [f64; 2], k: f64| add(p, mul(n, k));
    let (a, b) = ([0.0, 0.0], [40.0, 20.0]);
    let got = dimensions(&read);
    assert_eq!(got.len(), 1);
    assert_placed(
        &got[0],
        &[
            &[at(a, 0.4), at(a, 3.1)],
            &[at(b, 0.4), at(b, 3.1)],
            &[at(a, 2.6), at(b, 2.6)],
        ],
        &[(at(a, 2.6), t), (at(b, 2.6), mul(t, -1.0))],
        "44.72",
        at([20.0, 10.0], 2.6),
    );
}

#[test]
fn a_length_on_a_slanted_side_stands_off_square_to_it_away_from_the_middle() {
    // The side (0, 0) to (30, 40), t = (0.6, 0.8), its normal (−0.8, 0.6); the sketch's
    // middle (20, 20) is on the other side of its middle (15, 20), so it stands along it.
    let sketch = sketch_of(
        vec![
            point("pa000001", 0.0, 0.0),
            point("pa000002", 30.0, 40.0),
            point("pa000003", 40.0, 0.0),
        ],
        vec![line("na000001", "pa000001", "pa000002")],
        Vec::new(),
    );
    let mut read = under(sketch);
    read.members
        .constraints
        .push(length("ka000001", "na000001", "50", true));
    let n = [-0.8, 0.6];
    let at = |p: [f64; 2], k: f64| add(p, mul(n, k));
    let (a, b) = ([0.0, 0.0], [30.0, 40.0]);
    let got = dimensions(&read);
    assert_eq!(got.len(), 1);
    assert_placed(
        &got[0],
        &[
            &[at(a, 0.4), at(a, 3.1)],
            &[at(b, 0.4), at(b, 3.1)],
            &[at(a, 2.6), at(b, 2.6)],
        ],
        &[(at(a, 2.6), [0.6, 0.8]), (at(b, 2.6), [-0.6, -0.8])],
        "50",
        at([15.0, 20.0], 2.6),
    );
}

#[test]
fn a_lone_lines_length_stands_on_its_normals_side_the_middle_on_its_line() {
    // The one line (0, 0) to (40, 0) is the sketch: its middle (20, 0) is the line's, so
    // the dimension takes the normal (0, 1) as it is, above.
    let sketch = sketch_of(
        vec![point("pa000001", 0.0, 0.0), point("pa000002", 40.0, 0.0)],
        vec![line("na000001", "pa000001", "pa000002")],
        Vec::new(),
    );
    let mut read = under(sketch);
    read.members
        .constraints
        .push(length("ka000001", "na000001", "40", true));
    let got = dimensions(&read);
    assert_eq!(got.len(), 1);
    assert_placed(
        &got[0],
        &[
            &[[0.0, 0.4], [0.0, 3.1]],
            &[[40.0, 0.4], [40.0, 3.1]],
            &[[0.0, 2.6], [40.0, 2.6]],
        ],
        &[([0.0, 2.6], [1.0, 0.0]), ([40.0, 2.6], [-1.0, 0.0])],
        "40",
        [20.0, 2.6],
    );
}

#[test]
fn a_horizontal_distance_stands_above_or_below_and_a_vertical_one_beside() {
    // Across, (0, 0) to (40, 20): its middle (20, 10) at the sketch's, not below it, so it
    // stands above the higher point, at 20 + 2.6 = 22.6.
    let read = under(rectangle(vec![
        constraint(
            "ka000001",
            Form::DistanceX {
                a: element("pa000001"),
                b: element("pa000003"),
                value: mm("40"),
                driving: true,
            },
        ),
        // Up the left side, (0, 0) to (0, 20): its middle (0, 10) left of the sketch's,
        // so it stands left of the leftmost point, at 0 − 2.6.
        constraint(
            "ka000002",
            Form::DistanceY {
                a: element("pa000001"),
                b: element("pa000004"),
                value: mm("20"),
                driving: true,
            },
        ),
    ]));
    let got = dimensions(&read);
    assert_eq!(got.len(), 2);
    assert_placed(
        &got[0],
        &[
            &[[0.0, 0.4], [0.0, 23.1]],
            &[[40.0, 20.4], [40.0, 23.1]],
            &[[0.0, 22.6], [40.0, 22.6]],
        ],
        &[([0.0, 22.6], [1.0, 0.0]), ([40.0, 22.6], [-1.0, 0.0])],
        "40",
        [20.0, 22.6],
    );
    assert_placed(
        &got[1],
        &[
            &[[-0.4, 0.0], [-3.1, 0.0]],
            &[[-0.4, 20.0], [-3.1, 20.0]],
            &[[-2.6, 0.0], [-2.6, 20.0]],
        ],
        &[([-2.6, 0.0], [0.0, 1.0]), ([-2.6, 20.0], [0.0, -1.0])],
        "20",
        [-2.6, 10.0],
    );
}

#[test]
fn a_point_to_line_distance_runs_to_its_foot_and_points_down_for_a_point_on_the_line() {
    let read = under(rectangle(vec![
        // (0, 20) to the right side, x = 40: its foot (40, 20).
        constraint(
            "ka000001",
            Form::PointLineDistance {
                point: element("pa000004"),
                line: element("na000002"),
                value: mm("40"),
                side: LineSide::Left,
                driving: true,
            },
        ),
        // (0, 0) on the bottom side: its foot itself, so no direction of its own: down.
        constraint(
            "ka000002",
            Form::PointLineDistance {
                point: element("pa000001"),
                line: element("na000001"),
                value: mm("0"),
                side: LineSide::Left,
                driving: true,
            },
        ),
    ]));
    let got = dimensions(&read);
    assert_eq!(got.len(), 2);
    assert_placed(
        &got[0],
        &[&[[0.0, 20.0], [40.0, 20.0]]],
        &[([0.0, 20.0], [1.0, 0.0]), ([40.0, 20.0], [-1.0, 0.0])],
        "40",
        [20.0, 20.0],
    );
    assert_placed(
        &got[1],
        &[&[[0.0, 0.0], [0.0, 0.0]]],
        &[([0.0, 0.0], [0.0, -1.0]), ([0.0, 0.0], [0.0, 1.0])],
        "0",
        [0.0, 0.0],
    );
}

#[test]
fn a_radius_runs_out_through_an_arcs_middle_and_a_lone_circles_diameter_at_45_degrees() {
    // The arc about (0, 0) from (10, 0) to (0, 10): out through its middle, at 45°,
    // u = (1, 1)/√2, from its centre to 2.6 mm past its rim, the arrow on the rim
    // pointing in.
    let sketch = sketch_of(
        vec![
            point("pa000001", 0.0, 0.0),
            point("pa000002", 10.0, 0.0),
            point("pa000003", 0.0, 10.0),
        ],
        Vec::new(),
        vec![arc("aa000001", "pa000001", "pa000002", "pa000003")],
    );
    let mut read = under(sketch);
    read.members.constraints.push(constraint(
        "ka000001",
        Form::Radius {
            curve: element("aa000001"),
            value: mm("10"),
            driving: true,
        },
    ));
    let h = FRAC_1_SQRT_2;
    let u = [h, h];
    let got = dimensions(&read);
    assert_eq!(got.len(), 1);
    assert_placed(
        &got[0],
        &[&[[0.0, 0.0], mul(u, 12.6)]],
        &[(mul(u, 10.0), [-h, -h])],
        "R10",
        mul(u, 12.6),
    );
    let mut circle = sketch_of(vec![point("pa000001", 5.0, 5.0)], Vec::new(), Vec::new());
    circle.circles.push(SketchCircle {
        id: id("ca000001"),
        center: id("pa000001"),
        radius: Coordinate::new(3.0).unwrap(),
        construction: false,
    });
    let mut read = under(circle);
    read.members.constraints.push(constraint(
        "ka000001",
        Form::Diameter {
            curve: element("ca000001"),
            value: mm("6"),
            driving: true,
        },
    ));
    let c = [5.0, 5.0];
    let got = dimensions(&read);
    assert_eq!(got.len(), 1);
    assert_placed(
        &got[0],
        &[&[add(c, mul(u, -3.0)), add(c, mul(u, 5.6))]],
        &[(add(c, mul(u, 3.0)), [-h, -h]), (add(c, mul(u, -3.0)), u)],
        "Ø6",
        add(c, mul(u, 5.6)),
    );
}

#[test]
fn a_negative_angle_runs_its_arc_clockwise_its_arrows_along_it() {
    // The bottom side, +x from (0, 0), and the right side, +y from (40, 0), meet at
    // (40, 0); −90° runs from +x clockwise to −y, 2.6 mm out: its k-th of 24 points at
    // −90°·k/24. At its start the arc runs down, (0, −1); at its end, (40, −2.6), it
    // arrives running −x, so the arrow there points back along it, +x.
    let read = under(rectangle(vec![constraint(
        "ka000001",
        Form::Angle {
            a: element("na000001"),
            b: element("na000002"),
            reversed: false,
            value: Angle::parse("-90").unwrap(),
            driving: true,
        },
    )]));
    let on = |degrees: f64| {
        let r = degrees.to_radians();
        [40.0 + 2.6 * libm::cos(r), 2.6 * libm::sin(r)]
    };
    let arc: Vec<[f64; 2]> = (0..=24).map(|k| on(-90.0 * f64::from(k) / 24.0)).collect();
    let got = dimensions(&read);
    assert_eq!(got.len(), 1);
    assert_placed(
        &got[0],
        &[&arc],
        &[(on(0.0), [0.0, -1.0]), (on(-90.0), [1.0, 0.0])],
        "-90°",
        on(-45.0),
    );
}

#[test]
fn a_zero_angle_points_its_arrows_as_a_positive_angles_do() {
    // A reference 0° between the bottom side, +x from (0, 0), and the top side reversed,
    // +x from (0, 20) (the gate takes any angle up to 360° either way): parallel, so the
    // corner is the bottom side's start, (0, 0). The arc runs from 0° through nothing, each
    // of its 25 points at (2.6, 0). As a positive angle's, its first arrow runs
    // counter-clockwise from there, +y, and its last back, −y.
    let read = under(rectangle(vec![constraint(
        "ka000001",
        Form::Angle {
            a: element("na000001"),
            b: element("na000003"),
            reversed: true,
            value: Angle::parse("0").unwrap(),
            driving: false,
        },
    )]));
    let got = dimensions(&read);
    assert_eq!(got.len(), 1);
    assert_placed(
        &got[0],
        &[&[[2.6, 0.0]; 25]],
        &[([2.6, 0.0], [0.0, 1.0]), ([2.6, 0.0], [0.0, -1.0])],
        "(0°)",
        [2.6, 0.0],
    );
}

#[test]
fn glyphs_on_a_point_stack_leftward_and_one_on_an_arc_stands_out_through_its_middle() {
    // Two relations on (40, 0): the first 1.3 mm down and left, (38.7, −1.3); the next a
    // step (1.8 mm) further left, (36.9, −1.3).
    let read = under(rectangle(vec![
        constraint(
            "ka000001",
            Form::HorizontalPoints {
                a: element("pa000002"),
                b: Reference::Origin,
            },
        ),
        constraint(
            "ka000002",
            Form::VerticalPoints {
                a: element("pa000002"),
                b: Reference::Origin,
            },
        ),
    ]));
    let got = glyphs(&read);
    assert_eq!(got.len(), 2);
    assert!(same(got[0].at, [38.7, -1.3]), "{got:?}");
    assert!(same(got[1].at, [36.9, -1.3]), "{got:?}");
    // The arc about (0, 0) from (10, 0) to (0, 10): its glyph at 45°, 10 + 1.3 out.
    let sketch = sketch_of(
        vec![
            point("pa000001", 0.0, 0.0),
            point("pa000002", 10.0, 0.0),
            point("pa000003", 0.0, 10.0),
        ],
        vec![line("na000001", "pa000002", "pa000003")],
        vec![arc("aa000001", "pa000001", "pa000002", "pa000003")],
    );
    let mut read = under(sketch);
    read.members.constraints.push(constraint(
        "ka000001",
        Form::Equal {
            a: element("aa000001"),
            b: element("na000001"),
        },
    ));
    let got = glyphs(&read);
    assert_eq!(got.len(), 2);
    let out = 11.3 * FRAC_1_SQRT_2;
    assert!(same(got[0].at, [out, out]), "{got:?}");
}

#[test]
fn a_circles_glyph_stands_out_at_45_degrees_and_a_half_arcs_at_its_middle_straight_up() {
    // An equal relation on a circle about (30, 0) of radius 4 and on the arc about (0, 0)
    // from (10, 0) to (−10, 0), counter-clockwise from 0° through 180°.
    let mut sketch = sketch_of(
        vec![
            point("pa000001", 0.0, 0.0),
            point("pa000002", 10.0, 0.0),
            point("pa000003", -10.0, 0.0),
            point("pa000004", 30.0, 0.0),
        ],
        Vec::new(),
        vec![arc("aa000001", "pa000001", "pa000002", "pa000003")],
    );
    sketch.circles.push(SketchCircle {
        id: id("ca000001"),
        center: id("pa000004"),
        radius: Coordinate::new(4.0).unwrap(),
        construction: false,
    });
    let mut read = under(sketch);
    read.members.constraints.push(constraint(
        "ka000001",
        Form::Equal {
            a: element("ca000001"),
            b: element("aa000001"),
        },
    ));
    let got = glyphs(&read);
    assert_eq!(got.len(), 2, "{got:?}");
    // The circle's at 45°, 4 + 1.3 out: (30 + 5.3/√2, 5.3/√2), about (33.748, 3.748); not
    // at its 180°, (24.7, 0).
    let out = 5.3 * FRAC_1_SQRT_2;
    assert!(same(got[0].at, [30.0 + out, out]), "{got:?}");
    // The arc's at its middle, 90°, 10 + 1.3 out: straight up, (0, 11.3); not at 45°.
    assert!(same(got[1].at, [0.0, 11.3]), "{got:?}");
}

#[test]
fn the_axes_run_along_zero_only_where_its_in_view_and_no_grid_at_no_scale() {
    let read = under(rectangle(Vec::new()));
    let axes = |seen: &View<'_>| -> Vec<(Vec<Pos2>, egui::Color32)> {
        drawing(&read, seen, Shown::default(), &Default::default(), &[])
            .into_iter()
            .filter_map(|d| match d {
                Drawn::Stroke {
                    kind: StrokeKind::Axis,
                    points,
                    colour,
                    ..
                } => Some((points, colour)),
                _ => None,
            })
            .collect()
    };
    // From (−1, −1) to (11, 6): x along y = 0, (−1, 0) to (11, 0), (90, 500) to (210, 500);
    // y along x = 0, (0, −1) to (0, 6), (100, 510) to (100, 440).
    let seen = View {
        visible: Some(([-1.0, -1.0], [11.0, 6.0])),
        ..view()
    };
    assert_eq!(
        axes(&seen),
        [
            (vec![pos2(90.0, 500.0), pos2(210.0, 500.0)], color32(AXIS_X)),
            (
                vec![pos2(100.0, 510.0), pos2(100.0, 440.0)],
                color32(AXIS_Y)
            ),
        ]
    );
    // From (10, 10) to (60, 60): neither axis in view.
    let away = View {
        visible: Some(([10.0, 10.0], [60.0, 60.0])),
        ..view()
    };
    assert!(axes(&away).is_empty());
    // No millimetres a point: no grid lines, their spacing on the screen unknown (a 10 mm
    // step at no scale isn't 8 points apart, however far it reaches).
    let flat = View {
        scale: Scale(0.0),
        ..seen
    };
    let drawn = drawing(&read, &flat, Shown::default(), &Default::default(), &[]);
    assert!(!drawn.iter().any(|d| matches!(
        d,
        Drawn::Stroke {
            kind: StrokeKind::GridMinor | StrokeKind::GridMajor,
            ..
        }
    )));
}

#[test]
fn a_view_past_the_fade_on_one_axis_alone_draws_no_grid_and_no_axes() {
    // Ten points a millimetre, the minor step 100 points apart. In view, x from 300 to
    // 400 mm and y from −10 to 10 mm: y = −10, 0, and 10 cross it, and so does the x axis,
    // but every x in it lies past the grid's 200 mm, so none of them is drawn. So too with
    // the axes swapped: x from −10 to 10, y from −400 to −300.
    let read = under(rectangle(Vec::new()));
    for visible in [
        ([300.0, -10.0], [400.0, 10.0]),
        ([-10.0, -400.0], [10.0, -300.0]),
    ] {
        let seen = View {
            visible: Some(visible),
            ..view()
        };
        let drawn = drawing(&read, &seen, Shown::default(), &Default::default(), &[]);
        let grid: Vec<&Drawn> = drawn
            .iter()
            .filter(|d| {
                matches!(
                    d,
                    Drawn::Stroke {
                        kind: StrokeKind::GridMinor | StrokeKind::GridMajor | StrokeKind::Axis,
                        ..
                    }
                )
            })
            .collect();
        assert!(grid.is_empty(), "{visible:?}: {grid:?}");
    }
}

#[test]
fn an_arc_previews_from_its_start_to_its_end_and_a_round_place_not_a_number_is_none() {
    // About (0, 0) from (0, 10) to (−10, 0): a quarter, counter-clockwise, from 90° to 180°,
    // (100, 400) to (0, 500) on the screen.
    let drawn = preview(
        &Preview {
            arcs: vec![silico_core::sketch_tools::PreviewArc {
                center: [0.0, 0.0],
                start: [0.0, 10.0],
                end: [-10.0, 0.0],
            }],
            ..Preview::default()
        },
        &view(),
    );
    let Some(Drawn::Stroke { points, .. }) = drawn.first() else {
        panic!("{drawn:?}");
    };
    assert!(near(points[0], pos2(100.0, 400.0)), "{points:?}");
    assert!(
        near(*points.last().unwrap(), pos2(0.0, 500.0)),
        "{points:?}"
    );
    // Every place on the round projected to a y that's no number: none of them shown.
    let nan = |at: [f64; 2]| Some(pos2(at[0] as f32, f32::NAN));
    let points = round_points([0.0, 0.0], 1.0, 0.0, TAU, &nan, 0.25, 0);
    assert!(!points.is_empty());
    assert!(points.iter().all(Option::is_none), "{points:?}");
}

#[test]
fn a_tiny_circle_still_takes_four_fill_segments_and_an_arc_its_share() {
    // A hundredth of a millimetre at a tenth a point: 0.1 points, within the tolerance in
    // one chord, but a circle takes at least 4, a half 2, a quarter 1.
    let at = |sweep: f64| draw::curve_segments(0.01, sweep, HAND, 0.25);
    assert_eq!(at(TAU), 4);
    assert_eq!(at(PI), 2);
    assert_eq!(at(FRAC_PI_2), 1);
}

#[test]
fn segments_refuse_what_isnt_a_curve_and_take_one_for_a_curve_within_its_tolerance() {
    // No number for a sweep, a radius below zero, or no tolerance: the most.
    assert_eq!(segments(0.1, f64::INFINITY, 0.25), MAX_CURVE_SEGMENTS);
    assert_eq!(segments(f64::NAN, 1.0, 0.25), MAX_CURVE_SEGMENTS);
    assert_eq!(segments(-1.0, 1.0, 0.25), MAX_CURVE_SEGMENTS);
    assert_eq!(segments(0.0, 1.0, 0.0), MAX_CURVE_SEGMENTS);
    // A radius of nothing, or within the tolerance, is one chord, whatever its sweep.
    assert_eq!(segments(0.0, 1.0, 0.25), 1);
    assert_eq!(segments(0.1, PI, 0.25), 1);
    assert_eq!(segments(10.0, 0.0, 0.25), 1);
}

#[test]
fn a_region_of_exactly_the_vertex_bound_is_filled() {
    // 195 holes at a millionth of a millimetre a point, each at the most segments, 1,024,
    // and the outer rectangle's 4: 199,684. One more hole, of radius 0.005043 mm, 5,043
    // points: a chord's angle 2·acos(1 − 0.25/5043) lies between 2π/316 (radius 5,058.8)
    // and 2π/315 (radius 5,026.8), so 316 segments: 200,000 vertices, the bound, filled.
    let mut read = pitched(15, 13, 7.0);
    read.members.points.push(point("pz000001", 7.0, 7.0));
    read.members.circles.push(SketchCircle {
        id: id("cz000001"),
        center: id("pz000001"),
        radius: Coordinate::new(0.005043).unwrap(),
        construction: false,
    });
    read.regions[0].holes.push(vec![id("cz000001")]);
    assert_eq!(
        draw::curve_segments(0.005043, TAU, Scale(1e-6), SKETCH_CHORD_TOLERANCE),
        316
    );
    let fills = region_fills(&read, Scale(1e-6), SKETCH_CHORD_TOLERANCE);
    assert_eq!(fills.len(), 1);
}

#[test]
fn a_flat_sketch_takes_its_larger_span_and_a_scale_of_one_for_no_span_or_no_room() {
    let rect = egui::Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(100.0, 100.0));
    let fit = |points: Vec<SketchPoint>, rect: egui::Rect| {
        Flat::fit(rect, &under(sketch_of(points, Vec::new(), Vec::new())))
    };
    // From (−10, −5) to (30, 15), the origin within: the 40 mm across fills the 100
    // points less a tenth each side, 80: half a millimetre a point.
    let close = |flat: Flat, want: f64| (flat.scale().0 - want).abs() < 1e-12;
    let wide = fit(
        vec![
            point("pa000001", -10.0, -5.0),
            point("pa000002", 30.0, 15.0),
        ],
        rect,
    );
    assert!(close(wide, 0.5), "{wide:?}");
    // From (−5, −10) to (15, 30): the 40 mm up.
    let tall = fit(
        vec![
            point("pa000001", -5.0, -10.0),
            point("pa000002", 15.0, 30.0),
        ],
        rect,
    );
    assert!(close(tall, 0.5), "{tall:?}");
    // Its middle, (5, 10), at the rectangle's centre; 20 points right of it, 10 mm right.
    assert_eq!(tall.project([5.0, 10.0]), Some(pos2(50.0, 50.0)));
    assert_eq!(tall.to_plane(pos2(70.0, 50.0)), Some([15.0, 10.0]));
    // No points but the origin: no span, a point a millimetre.
    assert!(close(fit(Vec::new(), rect), 1.0));
    // No room: a point a millimetre too.
    let none = egui::Rect::from_min_size(pos2(5.0, 5.0), egui::vec2(0.0, 0.0));
    assert!(close(fit(vec![point("pa000001", 40.0, 20.0)], none), 1.0));
}

#[test]
fn each_drawn_item_holds_its_own_positions() {
    let (a, b, c) = (pos2(1.0, 2.0), pos2(3.0, 4.0), pos2(5.0, 6.0));
    let colour = SKETCH_FULLY_DEFINED;
    let stroke = Drawn::Stroke {
        of: None,
        kind: StrokeKind::Dimension,
        points: vec![a, b],
        colour,
        selected: false,
    };
    assert_eq!(stroke.positions(), [a, b]);
    let fill = Drawn::Fill {
        of: None,
        triangles: vec![[a, b, c]],
        colour,
    };
    assert_eq!(fill.positions(), [a, b, c]);
    let arrow = Drawn::Arrow {
        of: id("ka000001"),
        tip: c,
        toward: vec2(1.0, 0.0),
        colour,
        selected: false,
    };
    assert_eq!(arrow.positions(), [c]);
    assert_eq!(Drawn::Origin { at: b }.positions(), [b]);
}

/// The shapes `drawn` turns into.
fn shapes_of(drawn: Drawn) -> Vec<egui::Shape> {
    let mut shapes = Vec::new();
    with_painter(|painter| shapes = super::draw::to_shapes(painter, &[drawn]));
    shapes
}

#[test]
fn a_fill_a_selected_point_and_an_arrowhead_turn_into_their_shapes() {
    // Two triangles: one mesh, its indices each triangle's own three vertices.
    let (a, b, c) = (pos2(0.0, 0.0), pos2(10.0, 0.0), pos2(0.0, 10.0));
    let shapes = shapes_of(Drawn::Fill {
        of: None,
        triangles: vec![[a, b, c], [b, c, a]],
        colour: SKETCH_PROFILE_FILL,
    });
    let [egui::Shape::Mesh(mesh)] = shapes.as_slice() else {
        panic!("{shapes:?}");
    };
    assert_eq!(mesh.indices, [0, 1, 2, 3, 4, 5]);
    // A selected point: its 5-point square over a halo 3 points wider each side, 11.
    let shapes = shapes_of(Drawn::Point {
        of: None,
        at: pos2(50.0, 50.0),
        colour: SKETCH_FULLY_DEFINED,
        selected: true,
    });
    let rects: Vec<egui::Rect> = shapes
        .iter()
        .map(|s| match s {
            egui::Shape::Rect(r) => r.rect,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        rects,
        [
            egui::Rect::from_center_size(pos2(50.0, 50.0), vec2(11.0, 11.0)),
            egui::Rect::from_center_size(pos2(50.0, 50.0), vec2(5.0, 5.0)),
        ]
    );
    // An arrowhead at (10, 10) along +x: its base 7 points along, (17, 10), 6 wide.
    let shapes = shapes_of(Drawn::Arrow {
        of: id("ka000001"),
        tip: pos2(10.0, 10.0),
        toward: vec2(1.0, 0.0),
        colour: SKETCH_DIMENSION_TEXT,
        selected: false,
    });
    let [egui::Shape::Path(path)] = shapes.as_slice() else {
        panic!("{shapes:?}");
    };
    let mut corners = path.points.clone();
    corners[1..].sort_by(|p, q| p.y.total_cmp(&q.y));
    assert_eq!(
        corners,
        [pos2(10.0, 10.0), pos2(17.0, 7.0), pos2(17.0, 13.0)]
    );
}

#[test]
fn a_dimensions_value_is_boxed_about_it_its_text_centred() {
    // The box: the text's width and 5 points each side, 18 high, centred at (200, 200);
    // the text centred in it.
    let mut text = egui::Vec2::ZERO;
    with_painter(|painter| {
        text = painter
            .layout_no_wrap(
                "40".to_owned(),
                egui::FontId::proportional(CANVAS_TEXT_SIZE),
                SKETCH_DIMENSION_TEXT,
            )
            .size();
    });
    assert!(text.x > 0.0);
    let at = pos2(200.0, 200.0);
    let shapes = shapes_of(Drawn::Label {
        of: None,
        kind: LabelKind::Dimension,
        at,
        text: "40".to_owned(),
        colour: SKETCH_DIMENSION_TEXT,
        selected: false,
    });
    let [egui::Shape::Rect(frame), egui::Shape::Text(words)] = shapes.as_slice() else {
        panic!("{shapes:?}");
    };
    assert_eq!(
        frame.rect,
        egui::Rect::from_center_size(at, vec2(text.x + 10.0, 18.0))
    );
    assert!(near(words.pos, at - text / 2.0), "{:?}", words.pos);
}

/// The shapes of a glyph of `kind` at (100, 100).
fn glyph_shapes(kind: GlyphKind, colour: egui::Color32, selected: bool) -> Vec<egui::Shape> {
    shapes_of(Drawn::Glyph {
        of: None,
        kind,
        at: pos2(100.0, 100.0),
        colour,
        selected,
    })
}

#[test]
fn a_glyph_is_its_box_its_border_by_state_and_its_icon_inside() {
    let side = egui::Rect::from_center_size(pos2(100.0, 100.0), vec2(16.0, 16.0));
    let rect = |s: &egui::Shape| match s {
        egui::Shape::Rect(r) => r.clone(),
        other => panic!("{other:?}"),
    };
    // Selected: a halo 3 points wider each side first, then the box, its border white.
    let shapes = glyph_shapes(GlyphKind::Horizontal, FOCUS, true);
    assert_eq!(rect(&shapes[0]).rect, side.expand(3.0));
    assert_eq!(rect(&shapes[1]).rect, side);
    assert_eq!(rect(&shapes[2]).stroke.color, SKETCH_SELECTED_STROKE);
    // The plain glyph's gray border is the glyph's stroke; a state's colour its own.
    let plain = glyph_shapes(GlyphKind::Horizontal, GRAY_700, false);
    assert_eq!(rect(&plain[1]).stroke.color, SKETCH_GLYPH_STROKE);
    let focus = glyph_shapes(GlyphKind::Horizontal, FOCUS, false);
    assert_eq!(rect(&focus[1]).stroke.color, FOCUS);
    // The horizontal icon's bar, the icon square's (3.5, 10) to (16.5, 10), at 0.6 about
    // its middle (10, 10): (96.1, 100) to (103.9, 100); its left end's tick, (3.5, 7) to
    // (3.5, 13), (96.1, 98.2) to (96.1, 101.8).
    let line = |s: &egui::Shape| match s {
        egui::Shape::LineSegment { points, stroke } => (*points, *stroke),
        other => panic!("{other:?}"),
    };
    let (bar, stroke) = line(&focus[2]);
    assert!(near(bar[0], pos2(96.1, 100.0)) && near(bar[1], pos2(103.9, 100.0)));
    assert_eq!(stroke, egui::Stroke::new(1.3, FOCUS));
    let (tick, _) = line(&focus[3]);
    assert!(near(tick[0], pos2(96.1, 98.2)) && near(tick[1], pos2(96.1, 101.8)));
    // The coincident icon: a ring of 5.5 at 0.6, 3.3, in the icon's stroke, and a dot of
    // 1.8, 1.08.
    let coincident = glyph_shapes(GlyphKind::Coincident, FOCUS, false);
    let circle = |s: &egui::Shape| match s {
        egui::Shape::Circle(c) => *c,
        other => panic!("{other:?}"),
    };
    let ring = circle(&coincident[2]);
    assert_eq!(ring.center, pos2(100.0, 100.0));
    assert!((ring.radius - 3.3).abs() < 1e-5, "{ring:?}");
    assert_eq!(ring.stroke, egui::Stroke::new(1.3, FOCUS));
    let dot = circle(&coincident[3]);
    assert!((dot.radius - 1.08).abs() < 1e-5, "{dot:?}");
    // Every kind's icon, each of its parts as the prototype's icons.js draws it, inside its
    // box.
    for (kind, parts) in prototype_icons() {
        let icon = &glyph_shapes(kind, FOCUS, false)[2..];
        assert_eq!(icon.len(), parts.len(), "{kind:?}: {icon:?}");
        for (shape, part) in icon.iter().zip(&parts) {
            assert_icon_part(shape, part, kind);
            assert!(
                side.contains_rect(shape.visual_bounding_rect()),
                "{kind:?}: {shape:?}"
            );
        }
    }
    // The symmetric icon: its axis dashed down the middle, (10, 3) to (10, 17), 8.4 points
    // long on the screen, 2 on and 1.6 off at 0.6 (egui's dash 1.2, its gap 1): dashes from
    // 0, 2.2, 4.4, and 6.6 down it, each 1.2. Then its six strokes.
    let icon = &glyph_shapes(GlyphKind::Symmetric, FOCUS, false)[2..];
    let (axis, strokes) = icon.split_at(icon.len().saturating_sub(6));
    let top = icon_at(10.0, 3.0).y;
    let dashes: Vec<(f32, f32)> = axis
        .iter()
        .map(|dash| {
            let egui::Shape::LineSegment { points, stroke } = dash else {
                panic!("{dash:?}");
            };
            assert_eq!(*stroke, egui::Stroke::new(1.3, FOCUS));
            assert!(
                points.iter().all(|p| (p.x - 100.0).abs() < 1e-4),
                "{points:?}"
            );
            (points[0].y - top, points[1].y - top)
        })
        .collect();
    assert_eq!(dashes.len(), 4, "{dashes:?}");
    for (dash, from) in dashes.iter().zip([0.0, 2.2, 4.4, 6.6]) {
        assert!(
            (dash.0 - from).abs() < 1e-4 && (dash.1 - from - 1.2).abs() < 1e-4,
            "{dashes:?}"
        );
    }
    for (shape, (a, b)) in strokes.iter().zip([
        ((6.0, 6.0), (6.0, 14.0)),
        ((14.0, 6.0), (14.0, 14.0)),
        ((3.5, 6.0), (6.0, 6.0)),
        ((3.5, 14.0), (6.0, 14.0)),
        ((14.0, 6.0), (16.5, 6.0)),
        ((14.0, 14.0), (16.5, 14.0)),
    ]) {
        assert_icon_part(shape, &IconPart::Line(a, b, 1.0), GlyphKind::Symmetric);
    }
}

/// One part of a relation's icon, in the prototype's 20-unit icon square (icons.js's
/// `cHorizontal` and the rest): a line at an opacity, a ring, a dot, or a filled triangle.
enum IconPart {
    Line((f32, f32), (f32, f32), f32),
    Ring((f32, f32), f32),
    Dot((f32, f32), f32),
    Triangle([(f32, f32); 3]),
}

/// The icon square's `(x, y)` on the screen, for a glyph at (100, 100): the square's middle,
/// (10, 10), at the glyph's centre, scaled to 0.6 about it.
fn icon_at(x: f32, y: f32) -> Pos2 {
    pos2(100.0 + (x - 10.0) * 0.6, 100.0 + (y - 10.0) * 0.6)
}

/// Each kind's icon, part by part, typed from the prototype's icons.js (its `path`s split at
/// each `M`, its `circle`s by their `fill`); the redundant one, an exclamation mark, is
/// Silico's own (open item 28): a bar down the middle over a dot below it.
fn prototype_icons() -> Vec<(GlyphKind, Vec<IconPart>)> {
    use IconPart::{Dot, Line, Ring, Triangle};
    vec![
        (
            GlyphKind::Horizontal,
            vec![
                Line((3.5, 10.0), (16.5, 10.0), 1.0),
                Line((3.5, 7.0), (3.5, 13.0), 0.5),
                Line((16.5, 7.0), (16.5, 13.0), 0.5),
            ],
        ),
        (
            GlyphKind::Vertical,
            vec![
                Line((10.0, 3.5), (10.0, 16.5), 1.0),
                Line((7.0, 3.5), (13.0, 3.5), 0.5),
                Line((7.0, 16.5), (13.0, 16.5), 0.5),
            ],
        ),
        (
            GlyphKind::Coincident,
            vec![Ring((10.0, 10.0), 5.5), Dot((10.0, 10.0), 1.8)],
        ),
        (
            GlyphKind::Midpoint,
            vec![
                Line((3.0, 13.0), (17.0, 13.0), 1.0),
                Triangle([(10.0, 7.5), (13.5, 13.0), (6.5, 13.0)]),
            ],
        ),
        (
            GlyphKind::Tangent,
            vec![Ring((10.0, 12.0), 4.5), Line((3.0, 7.5), (17.0, 7.5), 1.0)],
        ),
        (
            GlyphKind::Perpendicular,
            vec![
                Line((4.0, 16.0), (16.0, 16.0), 1.0),
                Line((10.0, 16.0), (10.0, 4.0), 1.0),
            ],
        ),
        (
            GlyphKind::Parallel,
            vec![
                Line((6.0, 16.0), (11.0, 4.0), 1.0),
                Line((10.0, 16.0), (15.0, 4.0), 1.0),
            ],
        ),
        (
            GlyphKind::Equal,
            vec![
                Line((4.5, 7.5), (15.5, 7.5), 1.0),
                Line((4.5, 12.5), (15.5, 12.5), 1.0),
            ],
        ),
        (
            GlyphKind::Redundant,
            vec![Line((10.0, 4.0), (10.0, 12.0), 1.0), Dot((10.0, 15.5), 1.4)],
        ),
    ]
}

/// That `shape` is `part` of `kind`'s icon, drawn in FOCUS at (100, 100): its places at 0.6
/// of the icon square about the glyph's centre, its lines and rings 1.3 wide (the
/// prototype's icon stroke), at the part's opacity.
#[track_caller]
fn assert_icon_part(shape: &egui::Shape, part: &IconPart, kind: GlyphKind) {
    let at = |(x, y): (f32, f32)| icon_at(x, y);
    let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
    match (shape, part) {
        (egui::Shape::LineSegment { points, stroke }, IconPart::Line(a, b, opacity)) => {
            assert!(
                near(points[0], at(*a)) && near(points[1], at(*b)),
                "{kind:?}: {points:?} for {a:?} to {b:?}"
            );
            assert_eq!(
                *stroke,
                egui::Stroke::new(1.3, FOCUS.gamma_multiply(*opacity)),
                "{kind:?}"
            );
        }
        (egui::Shape::Circle(circle), IconPart::Ring(centre, radius)) => {
            assert!(near(circle.center, at(*centre)), "{kind:?}: {circle:?}");
            assert!(close(circle.radius, radius * 0.6), "{kind:?}: {circle:?}");
            assert_eq!(circle.stroke, egui::Stroke::new(1.3, FOCUS), "{kind:?}");
            assert_eq!(circle.fill, egui::Color32::TRANSPARENT, "{kind:?}");
        }
        (egui::Shape::Circle(circle), IconPart::Dot(centre, radius)) => {
            assert!(near(circle.center, at(*centre)), "{kind:?}: {circle:?}");
            assert!(close(circle.radius, radius * 0.6), "{kind:?}: {circle:?}");
            assert_eq!(circle.fill, FOCUS, "{kind:?}");
        }
        (egui::Shape::Path(path), IconPart::Triangle(corners)) => {
            assert_eq!(path.points.len(), 3, "{kind:?}: {path:?}");
            assert!(
                path.points
                    .iter()
                    .zip(corners)
                    .all(|(p, corner)| near(*p, at(*corner))),
                "{kind:?}: {path:?}"
            );
            assert_eq!(path.fill, FOCUS, "{kind:?}");
        }
        (shape, _) => panic!("{kind:?}: {shape:?} isn't its part"),
    }
}

// The mutants #351's run left alive in `hit`.

#[test]
fn hit_takes_no_place_thats_no_number_even_at_any_tolerance_and_an_exact_one_at_none() {
    let read = under(rectangle(Vec::new()));
    assert_eq!(hit(&read, [f64::INFINITY, 0.0], f64::INFINITY, HAND), None);
    assert_eq!(hit(&read, [0.0, f64::NAN], f64::INFINITY, HAND), None);
    // At a point exactly, with no tolerance: that point.
    assert_eq!(
        hit(&read, [40.0, 20.0], 0.0, HAND),
        Some(Hit {
            id: id("pa000003"),
            kind: ElementKind::Point,
        })
    );
}

#[test]
fn a_glyphs_and_a_values_boxes_are_hit_to_their_edges_and_not_past_them() {
    // The bottom side's horizontal: the side runs +x, so its right is −y: the glyph 0.28
    // along it, (11.2, 0), 1.3 mm below, (11.2, −1.3), its box 16 points, 0.8 mm either
    // way. Its length's value, "40", at (20, −2.6): two characters of 0.574 × 11.5 points,
    // 13.202, and 5 each side, so 1.16011 mm either way across; 18 points high, 0.9 mm
    // either way up.
    let read = under(rectangle(vec![
        horizontal("ka000001", "na000001"),
        length("ka000002", "na000001", "40", true),
    ]));
    let found = |at: [f64; 2], tolerance: f64| hit(&read, at, tolerance, HAND).map(|h| h.id);
    // Inside the glyph's box, off its middle both ways; 0.2 below it, out of a 0.1 reach.
    assert_eq!(found([11.7, -1.8], 0.1), Some(id("ka000001")));
    assert_eq!(found([11.2, -2.3], 0.1), None);
    // Inside the value's box, 0.85 above its line, off its middle; 0.1 past either edge,
    // out of a 0.05 reach.
    assert_eq!(found([21.1, -1.75], 0.05), Some(id("ka000002")));
    assert_eq!(found([21.26, -1.75], 0.05), None);
    assert_eq!(found([20.0, -1.6], 0.05), None);
}

#[test]
fn an_arcs_distance_is_its_rims_on_its_sweep_and_its_nearer_ends_past_it() {
    // About (0, 0), radius 10, from 90° through 90°: from (0, 10) to (−10, 0).
    let quarter = super::draw::Curve::Round {
        centre: [0.0, 0.0],
        radius: 10.0,
        start: FRAC_PI_2,
        sweep: FRAC_PI_2,
    };
    let distance = |at| super::hit::distance(quarter, at);
    // At 135°, 1 outside the rim: 1.
    let out = 11.0 * std::f64::consts::FRAC_1_SQRT_2;
    assert!((distance([-out, out]) - 1.0).abs() < 1e-9);
    // (−10, −1), just past its end: 1 from (−10, 0).
    assert!((distance([-10.0, -1.0]) - 1.0).abs() < 1e-9);
}

// The mutants #351's run left alive in the panels (`bar`): their look, the tools' names,
// the legend, the state's dot, and the status bar's words, each against the prototype
// (docs/design/m5-ui/: styles.css's `:root`, `.state-dot`, and `.legend`; sketch.js's
// palette).

/// Every shape `draw` paints in a context showing egui's light theme, after two passes so
/// its layout has settled, nested ones flattened, in paint order.
fn painted_on_light(mut draw: impl FnMut(&mut egui::Ui)) -> Vec<egui::Shape> {
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::theme::fonts());
    ctx.set_theme(egui::Theme::Light);
    let mut painted = Vec::new();
    for pass in 0..2 {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| draw(ui));
        output.textures_delta.clear();
        if pass == 1 {
            let mut shapes: Vec<egui::Shape> =
                output.shapes.into_iter().map(|c| c.shape).rev().collect();
            while let Some(shape) = shapes.pop() {
                match shape {
                    egui::Shape::Vec(inner) => shapes.extend(inner.into_iter().rev()),
                    shape => painted.push(shape),
                }
            }
        }
    }
    painted
}

/// The palette of an under-defined rectangle with no degrees of freedom left to say, and
/// `tool` on.
fn palette_painted(tool: Option<silico_core::sketch_tools::ToolKind>) -> Vec<egui::Shape> {
    let mut mode = mode_on(under(rectangle(Vec::new())));
    mode.tools.choose(tool.map(tools::Choice::Draw));
    let view = mode.palette_view(silico_core::command::Revision::OPENED);
    let mut settings = bar::Settings::default();
    painted_on_light(|ui| bar::palette(ui, &view, &mut settings))
}

/// The one text painted that says `words`.
#[track_caller]
fn painted_text<'a>(painted: &'a [egui::Shape], words: &str) -> &'a egui::epaint::TextShape {
    let found: Vec<&egui::epaint::TextShape> = painted
        .iter()
        .filter_map(|shape| match shape {
            egui::Shape::Text(text) if text.galley.text() == words => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(found.len(), 1, "{words:?} painted {} times", found.len());
    found[0]
}

#[test]
fn the_panels_are_the_prototypes_floating_panels_whatever_the_windows_theme() {
    // `--g200` (#212121) panels, `--g250` (#272626) borders, the selection `--blue`
    // (#186add) under `--blue-text` (#f0f2ff), errors `--red` (#ff3865), and text
    // `--g750` (#cfcfcf), `--g1000` (#ffffff) hovered or pressed.
    let rgb = |hex: u32| {
        let [_, r, g, b] = hex.to_be_bytes();
        egui::Color32::from_rgb(r, g, b)
    };
    let visuals = bar::panel_visuals();
    assert!(visuals.dark_mode);
    assert_eq!(visuals.window_fill, rgb(0x212121));
    assert_eq!(visuals.panel_fill, rgb(0x212121));
    assert_eq!(visuals.window_stroke, egui::Stroke::new(1.0, rgb(0x272626)));
    assert_eq!(visuals.selection.bg_fill, rgb(0x186add));
    assert_eq!(
        visuals.selection.stroke,
        egui::Stroke::new(1.0, rgb(0xf0f2ff))
    );
    assert_eq!(visuals.error_fg_color, rgb(0xff3865));
    let widgets = &visuals.widgets;
    assert_eq!(
        [
            widgets.noninteractive.fg_stroke.color,
            widgets.inactive.fg_stroke.color,
            widgets.open.fg_stroke.color,
        ],
        [rgb(0xcfcfcf); 3]
    );
    assert_eq!(
        [
            widgets.hovered.fg_stroke.color,
            widgets.active.fg_stroke.color,
        ],
        [rgb(0xffffff); 2]
    );
    // As the palette draws on a light window: its Show checkboxes' words in `--g750`.
    let painted = palette_painted(None);
    assert_eq!(
        painted_text(&painted, "Profile").fallback_color,
        rgb(0xcfcfcf)
    );
}

#[test]
fn each_tool_is_named_as_the_toolbars_name_it_and_the_palette_names_the_one_on() {
    use silico_core::sketch_tools::ToolKind;
    // The prototype's Line, Circle, and Point (app.js); Fusion's Center Rectangle, Center
    // Point Arc, 3-Point Arc, and Tangent Arc; and section 9's Corner Rectangle (SolidWorks'
    // name; Fusion's 2-Point Rectangle).
    assert_eq!(
        [
            ToolKind::Line,
            ToolKind::CornerRectangle,
            ToolKind::CenterRectangle,
            ToolKind::Circle,
            ToolKind::CenterpointArc,
            ToolKind::ThreePointArc,
            ToolKind::TangentArc,
            ToolKind::Point,
        ]
        .map(bar::tool_name),
        [
            "Line",
            "Corner Rectangle",
            "Center Rectangle",
            "Circle",
            "Center Point Arc",
            "3-Point Arc",
            "Tangent Arc",
            "Point",
        ]
    );
    // The palette's Tool: the one on, strong (`--g1000`), and its Automatic relations.
    let painted = palette_painted(Some(ToolKind::TangentArc));
    let name = painted_text(&painted, "Tangent Arc");
    assert_eq!(
        name.override_text_color
            .unwrap_or(name.galley.job.sections[0].format.color),
        egui::Color32::from_rgb(0xff, 0xff, 0xff)
    );
    painted_text(&painted, "Automatic relations");
    // None on: the keys that choose one.
    let painted = palette_painted(None);
    painted_text(
        &painted,
        "None. Line L, Rectangle R, Circle C, Arc A, Point P, Trim T",
    );
}

#[test]
fn the_palettes_legend_is_the_prototypes_each_colour_on_its_swatch_and_construction_dashed() {
    // sketch.js's Colours: a 10 × 3 swatch (`.legend i`) in each of `COL`'s colours beside
    // its words, then Construction's dashed rule.
    let painted = palette_painted(None);
    let rgb = |hex: u32| {
        let [_, r, g, b] = hex.to_be_bytes();
        egui::Color32::from_rgb(r, g, b)
    };
    let legend = [
        (0x7a95ff, "Under defined"),
        (0xeeeeed, "Fully defined"),
        (0xff3865, "Over defined, unsolvable"),
        (0xf2c94c, "Invalid (zero length)"),
        (0xb5b84a, "Dangling reference"),
        (0x827f7f, "Driven dimension"),
    ];
    let swatches: Vec<(egui::Rect, egui::Color32)> = painted
        .iter()
        .filter_map(|shape| match shape {
            egui::Shape::Rect(rect) if rect.rect.size() == vec2(10.0, 3.0) => {
                Some((rect.rect, rect.fill))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        swatches.iter().map(|(_, fill)| *fill).collect::<Vec<_>>(),
        legend.map(|(hex, _)| rgb(hex))
    );
    for ((swatch, _), (_, words)) in swatches.iter().zip(legend) {
        // Its words beside it, on its line.
        let text = painted_text(&painted, words);
        let middle = text.pos.y + text.galley.size().y / 2.0;
        assert!(text.pos.x > swatch.right(), "{words}");
        assert!((middle - swatch.center().y).abs() < 1.5, "{words}");
    }
    // Construction: a rule 2 points wide across its 10-point swatch, dashed as the canvas
    // dashes construction (5 on, 3 off) at half that, so dashes 2.5 long from 0, 4, and 8
    // along it, the last cut off at 10.
    let construction = painted_text(&painted, "Construction");
    let dashes: Vec<[Pos2; 2]> = painted
        .iter()
        .filter_map(|shape| match shape {
            egui::Shape::LineSegment { points, stroke }
                if stroke.width == 2.0 && stroke.color == TEXT_SUBDUED =>
            {
                Some(*points)
            }
            _ => None,
        })
        .collect();
    assert_eq!(dashes.len(), 3, "{dashes:?}");
    let left = dashes[0][0].x;
    let along: Vec<(f32, f32)> = dashes
        .iter()
        .map(|[a, b]| (a.x - left, b.x - left))
        .collect();
    for (got, want) in along.iter().zip([(0.0, 2.5), (4.0, 6.5), (8.0, 10.0)]) {
        assert!(
            (got.0 - want.0).abs() < 1e-4 && (got.1 - want.1).abs() < 1e-4,
            "{along:?}"
        );
    }
    let middle = construction.pos.y + construction.galley.size().y / 2.0;
    assert!(
        dashes
            .iter()
            .all(|[a, b]| a.y == b.y && (a.y - middle).abs() < 1.5)
    );
    assert!(construction.pos.x > left + 10.0);
}

#[test]
fn a_states_dot_is_the_prototypes_8_point_dot_its_words_a_gap_after_it() {
    // `.state-dot`: 8 × 8, round, in the state's colour, its room in the row its own; the
    // words then the row's gap on (egui's item spacing).
    let painted = palette_painted(None);
    let dots: Vec<egui::epaint::CircleShape> = painted
        .iter()
        .filter_map(|shape| match shape {
            egui::Shape::Circle(circle) if circle.fill == SKETCH_UNDER_DEFINED => Some(*circle),
            _ => None,
        })
        .collect();
    let [dot] = dots.as_slice() else {
        panic!("{dots:?}");
    };
    assert_eq!(dot.radius, 4.0);
    let words = painted_text(&painted, "Under defined · 0 DOF");
    let gap = egui::Style::default().spacing.item_spacing.x;
    assert!(
        (words.pos.x - (dot.center.x + 4.0 + gap)).abs() < 1e-3,
        "{:?} after {dot:?}",
        words.pos
    );
}

#[test]
fn the_status_bars_sketch_words_say_whats_edited_the_pointer_and_the_state() {
    let line = bar::StatusLine {
        editing: "Editing Sketch1".to_owned(),
        coordinates: Some("x 15.000 · y -5.250 mm".to_owned()),
        state: Some(bar::StateShown {
            words: "Fully defined".to_owned(),
            colour: SKETCH_FULLY_DEFINED,
        }),
    };
    let texts = texts_on_light(|ui| {
        ui.horizontal(|ui| bar::status_ui(ui, &line));
    });
    let words: Vec<&str> = texts.iter().map(|(words, _)| words.as_str()).collect();
    for said in ["Editing Sketch1", "x 15.000 · y -5.250 mm", "Fully defined"] {
        assert!(words.contains(&said), "{said:?} in {words:?}");
    }
    assert!(texts.contains(&("Fully defined".to_owned(), SKETCH_FULLY_DEFINED)));
    // The pointer off the canvas, and the sketch not read: a dash, and no state.
    let off = bar::StatusLine {
        coordinates: None,
        state: None,
        ..line
    };
    let texts = texts_on_light(|ui| {
        ui.horizontal(|ui| bar::status_ui(ui, &off));
    });
    let words: Vec<&str> = texts.iter().map(|(words, _)| words.as_str()).collect();
    assert!(words.contains(&"Editing Sketch1"), "{words:?}");
    assert!(words.contains(&"—"), "{words:?}");
    assert!(!words.contains(&"Fully defined"), "{words:?}");
}

// The mutants #351's run left alive in sketch mode's own (`mod.rs`): the fill found by its
// scale, the state and its hint, and the scale at a place.

#[test]
fn a_fill_is_found_only_at_the_power_of_two_scale_it_was_worked_out_at() {
    // At 10 points a millimetre (0.1 mm a point, the fill at 1/16: bucket −4), then 20
    // (0.05, at 1/32: −5): each its own fill, both kept; 1/64 (−6) never worked out.
    let mut mode = mode_on(plate());
    frames_at(&mut mode, 10.0, 0.0, 1);
    frames_at(&mut mode, 20.0, 0.0, 1);
    let read = mode.drawable().unwrap().clone();
    let at = |scale| region_fills(&read, Scale(scale), SKETCH_CHORD_TOLERANCE);
    let (sixteenth, thirty_second) = (at(0.0625), at(0.03125));
    assert_ne!(sixteenth, thirty_second);
    assert_eq!(mode.fill_at(-4), Some(sixteenth.as_slice()));
    assert_eq!(mode.fill_at(-5), Some(thirty_second.as_slice()));
    assert_eq!(mode.fill_at(-6), None);
}

#[test]
fn the_palette_hints_at_what_an_under_defined_sketch_needs_as_the_prototype_does() {
    // The prototype's palette (sketch.js, its Status): under defined, "Blue curves can still
    // move. Add relations or dimensions."; fully defined, nothing more.
    let opened = silico_core::command::Revision::OPENED;
    let mut read = under(rectangle(Vec::new()));
    let mode = mode_on(read.clone());
    assert_eq!(mode.state(), Some(SketchState::UnderDefined));
    assert_eq!(
        mode.palette_view(opened).hint,
        Some("Blue curves can still move. Add relations or dimensions.")
    );
    read.states.state = SketchState::FullyDefined;
    let mode = mode_on(read.clone());
    assert_eq!(mode.state(), Some(SketchState::FullyDefined));
    assert_eq!(mode.palette_view(opened).hint, None);
    // Over defined, with no solution, or invalid: each a word of its own on what to do.
    let mut said = std::collections::BTreeSet::new();
    for state in [
        SketchState::OverDefined,
        SketchState::NoSolution,
        SketchState::Invalid,
    ] {
        read.states.state = state;
        let mode = mode_on(read.clone());
        assert_eq!(mode.state(), Some(state));
        let hint = mode.palette_view(opened).hint.unwrap_or_default();
        assert!(
            hint.ends_with('.') && hint.len() > 20,
            "{state:?}: {hint:?}"
        );
        said.insert(hint);
    }
    assert_eq!(said.len(), 3, "{said:?}");
    // Nothing read yet: no state, and no hint.
    let unread = SketchMode::new(1, None);
    assert_eq!(unread.state(), None);
    assert_eq!(unread.palette_view(opened).hint, None);
}

#[test]
fn the_scale_at_a_place_is_the_mean_step_of_a_point_right_and_a_point_down_or_none() {
    // A plane whose x is the square of the screen's: at x = 10, a point right moves
    // 11² − 10² = 21 mm (a point left, 19), a point down 1: their mean, 11 mm a point.
    let squared = |p: Pos2| Some([f64::from(p.x) * f64::from(p.x), f64::from(p.y)]);
    assert_eq!(scale_at(&squared, pos2(10.0, 0.0)), Some(Scale(11.0)));
    // Every point to one place: no step, none.
    let still = |_: Pos2| Some([3.0, 4.0]);
    assert_eq!(scale_at(&still, pos2(10.0, 0.0)), None);
    // A point right going past any length: none.
    let edge = |p: Pos2| Some([if p.x > 10.5 { f64::INFINITY } else { 0.0 }, 0.0]);
    assert_eq!(scale_at(&edge, pos2(10.0, 0.0)), None);
    // A ray that misses the plane: none.
    let misses = |_: Pos2| -> Option<[f64; 2]> { None };
    assert_eq!(scale_at(&misses, pos2(10.0, 0.0)), None);
}
