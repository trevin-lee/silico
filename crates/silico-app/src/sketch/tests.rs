//! Sketch mode's own tests: what's drawn of hand-made reads (colours by state, construction
//! dashed, glyphs and dimensions placed as the prototype places them, the profile's fill,
//! the grid, a tool's preview, the selection), hit-testing by hand-worked distances, the
//! selection a frame's clicks leave, what Delete does, and what the palette and the status
//! bar say. Each expected value is worked out here, from the reads' coordinates, the hand
//! projection (sketch millimetres to screen points: ten points a millimetre, the origin at
//! (100, 500), y up), and `style`'s sizes in points (a tenth of a millimetre each here).

use std::collections::BTreeMap;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

use eframe::egui::{self, Pos2, pos2, vec2};
use silico_core::command::sketch::{Region, SketchRead};
use silico_core::document::{
    Angle, Coordinate, Direction, ElementId, ElementKind, Form, Length, OriginPlane, Reference,
    Sketch, SketchArc, SketchCircle, SketchConstraint, SketchLine, SketchPoint,
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
            mode.frame(painter, &input, &mut None);
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
    assert_eq!(mode.palette_view(r(3), None).said, None);
    assert_eq!(
        mode.palette_view(r(2), None).said.as_deref(),
        Some("the document has no feature s0000001")
    );
    assert_eq!(mode.palette_view(r(2), None).state, None);
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
        mode.palette_view(silico_core::command::Revision::OPENED, None)
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
                mode.frame(painter, &input, &mut None);
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
            mode.frame(painter, &input, &mut None);
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
    let view = mode.palette_view(silico_core::command::Revision::OPENED, None);
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

// ---- M5.11c: drags, the ghost, Smart Dimension's preview, and a placed dimension's edit ----

/// Sketch mode on the sketch s0000001, its newest read `read`, of the model at revision 2.
fn mode_at(read: SketchRead) -> SketchMode {
    let mut mode = SketchMode::new(
        1,
        Some(silico_core::document::FeatureId::new("s0000001").unwrap()),
    );
    mode.read = Some(Read {
        model: Revision::OPENED.next(),
        result: Ok(read),
    });
    mode
}

/// One frame of `events`, through the hand projection, the pointer at the last of them, with
/// the tab's drag `drag`.
fn frame_of(mode: &mut SketchMode, drag: &mut Option<drag::Drag>, events: &[PointerEvent]) {
    let pointer = events.last().map(|event| event.at);
    with_painter(|painter| {
        let input = FrameInput {
            events,
            pointer,
            project: &hand,
            to_plane: &unhand,
        };
        mode.frame(painter, &input, drag);
    });
}

fn at_mm(x: f64, y: f64) -> Pos2 {
    hand([x, y]).unwrap()
}

#[test]
fn a_press_put_on_its_curve_lands_on_a_lines_segment_an_arcs_sweep_and_a_circles_round() {
    let line = draw::Curve::Line {
        from: [0.0, 0.0],
        to: [40.0, 0.0],
    };
    // Off the segment's middle: its foot. Past its end: the end, not the line beyond.
    assert_eq!(on_curve(line, [10.0, 0.5]), Some([10.0, 0.0]));
    assert_eq!(on_curve(line, [40.6, 0.2]), Some([40.0, 0.0]));
    assert_eq!(on_curve(line, [-0.5, -0.1]), Some([0.0, 0.0]));
    // A quarter arc about the origin of radius 10, from 0 to 90°: a press at 45°, 10.5 out,
    // goes onto the arc at 45°; one just past its end, at 95°, to its end (0, 10); one just
    // before its start, at −5°, to its start (10, 0).
    let arc = draw::Curve::Round {
        centre: [0.0, 0.0],
        radius: 10.0,
        start: 0.0,
        sweep: FRAC_PI_2,
    };
    let r = 10.5 * std::f64::consts::FRAC_1_SQRT_2;
    let on = on_curve(arc, [r, r]).unwrap();
    let h = 10.0 * std::f64::consts::FRAC_1_SQRT_2;
    assert!((on[0] - h).abs() < 1e-12 && (on[1] - h).abs() < 1e-12);
    let past = on_curve(arc, direction(95f64.to_radians()).map(|c| c * 10.0)).unwrap();
    assert!(past[0].abs() < 1e-12 && (past[1] - 10.0).abs() < 1e-12);
    let before = on_curve(arc, direction(-5f64.to_radians()).map(|c| c * 10.0)).unwrap();
    assert!((before[0] - 10.0).abs() < 1e-12 && before[1].abs() < 1e-12);
    // A circle's round anywhere; its centre, or a place that isn't one, nowhere.
    let circle = draw::Curve::Round {
        centre: [5.0, 5.0],
        radius: 2.0,
        start: 0.0,
        sweep: TAU,
    };
    assert_eq!(on_curve(circle, [5.0, 9.0]), Some([5.0, 7.0]));
    assert_eq!(on_curve(circle, [5.0, 5.0]), None);
    assert_eq!(on_curve(circle, [f64::NAN, 5.0]), None);
}

#[test]
fn a_dimension_through_the_pointer_stands_its_line_there_and_its_text_at_it() {
    let read = under(rectangle(Vec::new()));
    let geometry = draw::Geometry::of(&read);
    let length = Form::Length {
        line: element("na000001"),
        value: mm("40"),
        driving: true,
    };
    // The bottom, (0, 0) to (40, 0): through (25, 7), its line at y = 7 from x 0 to 40, its
    // text at the pointer, extension lines up from a gap (4 points, 0.4 mm) to 5 points
    // (0.5 mm) past it.
    let shape = draw::measure(&length, &geometry, HAND, Some([25.0, 7.0])).unwrap();
    assert_eq!(shape.lines[2], vec![[0.0, 7.0], [40.0, 7.0]]);
    assert_eq!(shape.text_at, [25.0, 7.0]);
    assert_eq!(shape.text, "40");
    let ext = &shape.lines[0];
    assert!((ext[0][1] - 0.4).abs() < 1e-9 && (ext[1][1] - 7.5).abs() < 1e-9);
    // Below it, through (10, −3): at y = −3, the extension lines going down.
    let below = draw::measure(&length, &geometry, HAND, Some([10.0, -3.0])).unwrap();
    assert_eq!(below.lines[2], vec![[0.0, -3.0], [40.0, -3.0]]);
    assert!((below.lines[0][1][1] + 3.5).abs() < 1e-9);
    // With no pointer, as placed in the read: 26 points (2.6 mm) off, away from the middle.
    let placed = draw::measure(&length, &geometry, HAND, None).unwrap();
    assert!((placed.lines[2][0][1] + 2.6).abs() < 1e-9);
    assert_eq!(placed.text_at, [20.0, placed.lines[2][0][1]]);
    // A horizontal distance between the corners (0, 0) and (40, 20), through (30, 26): its
    // line at y = 26, its text there.
    let across = Form::DistanceX {
        a: element("pa000001"),
        b: element("pa000003"),
        value: mm("40"),
        driving: true,
    };
    let shape = draw::measure(&across, &geometry, HAND, Some([30.0, 26.0])).unwrap();
    assert_eq!(shape.lines[2], vec![[0.0, 26.0], [40.0, 26.0]]);
    assert_eq!(shape.text_at, [30.0, 26.0]);
}

#[test]
fn smart_dimensions_preview_draws_its_picks_selected_its_hover_and_its_dimension_through_the_pointer()
 {
    use silico_core::sketch_tools::DimensionPreview;
    let read = under(rectangle(Vec::new()));
    let preview = DimensionPreview {
        pointer: Some([20.0, -5.0]),
        picks: vec![element("na000001")],
        hover: Some(element("pa000003")),
        dimension: Some(Form::Length {
            line: element("na000001"),
            value: mm("40"),
            driving: true,
        }),
        locked: false,
        unavailable: None,
        modify: None,
    };
    let drawn = draw::dimension_preview(&preview, &read, &view(), None);
    // The bottom, picked: from (100, 500) to (500, 500), selected.
    let picked = of(&drawn, "na000001");
    assert!(
        matches!(picked.as_slice(), [Drawn::Stroke { points, selected: true, .. }] if points == &vec![pos2(100.0, 500.0), pos2(500.0, 500.0)])
    );
    // The corner the pointer would pick, (40, 20) at (500, 300): in the focus blue, ringed.
    assert!(of(&drawn, "pa000003").iter().any(|d| matches!(d, Drawn::Point { at, colour, selected: false, .. } if *at == pos2(500.0, 300.0) && *colour == FOCUS)));
    assert!(
        drawn
            .iter()
            .any(|d| matches!(d, Drawn::Ring { at, .. } if *at == pos2(500.0, 300.0)))
    );
    // Its length's line at y = −5, (100, 550) to (500, 550), and its value at the pointer.
    assert!(drawn.iter().any(|d| matches!(d, Drawn::Stroke { kind: StrokeKind::Dimension, points, colour, .. } if *points == vec![pos2(100.0, 550.0), pos2(500.0, 550.0)] && *colour == FOCUS)));
    let labels: Vec<_> = drawn
        .iter()
        .filter_map(|d| match d {
            Drawn::Label { text, at, .. } => Some((text.clone(), *at)),
            _ => None,
        })
        .collect();
    assert_eq!(labels, [("40".to_owned(), pos2(300.0, 550.0))]);
    // With the Modify box open, held where it opened, (10, −8): its line there, no value
    // drawn (the box stands there), and no crosshair.
    let open = DimensionPreview {
        modify: Some("40".to_owned()),
        ..preview
    };
    let drawn = draw::dimension_preview(&open, &read, &view(), Some([10.0, -8.0]));
    assert!(drawn.iter().any(|d| matches!(d, Drawn::Stroke { kind: StrokeKind::Dimension, points, .. } if *points == vec![pos2(100.0, 580.0), pos2(500.0, 580.0)])));
    assert!(!drawn.iter().any(|d| matches!(d, Drawn::Label { .. })));
    assert!(!drawn.iter().any(|d| matches!(
        d,
        Drawn::Stroke {
            kind: StrokeKind::Crosshair,
            ..
        }
    )));
}

#[test]
fn a_drags_ghost_is_the_begin_state_of_what_moved_faint_and_nothing_else() {
    let begin = under(rectangle(Vec::new()));
    let mut now = begin.clone();
    // The top right corner, pa000003, dragged from (40, 20) to (45, 25).
    if let Some(p) = now
        .members
        .points
        .iter_mut()
        .find(|p| p.id == id("pa000003"))
    {
        p.x = Coordinate::new(45.0).unwrap();
        p.y = Coordinate::new(25.0).unwrap();
    }
    let begun = drawn(&begin);
    let ghosted = draw::ghost(&begun, &begin, &now);
    let mut members: Vec<&str> = ghosted
        .iter()
        .filter_map(|d| d.of())
        .map(|id| id.as_str())
        .collect();
    members.sort_unstable();
    members.dedup();
    // The corner, and the two sides that end there: the right one and the top.
    assert_eq!(members, ["na000002", "na000003", "pa000003"]);
    let faint = SKETCH_UNDER_DEFINED.gamma_multiply(SKETCH_GHOST_OPACITY);
    assert!(ghosted.iter().all(|d| d.colour() == faint && !d.selected()));
    // Where it began: the corner's ghost at (500, 300).
    assert!(
        ghosted
            .iter()
            .any(|d| matches!(d, Drawn::Point { at, .. } if *at == pos2(500.0, 300.0)))
    );
    // Nothing moved, nothing ghosted.
    assert!(draw::ghost(&begun, &begin, &begin).is_empty());
}

#[test]
fn a_press_on_an_under_defined_corner_that_moves_past_the_threshold_is_a_drag_and_selects_nothing()
{
    let mut mode = mode_at(under(rectangle(Vec::new())));
    let mut dragged: Option<drag::Drag> = None;
    let corner = at_mm(40.0, 20.0);
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(PointerAction::Press, corner, false)],
    );
    let drag = dragged.as_ref().unwrap();
    assert_eq!(
        drag.grab,
        silico_core::command::DragGrab::Point(id("pa000003"))
    );
    assert!(!drag.moved && !drag.wants_begin());
    // 2 points: within the 3-point threshold, still a press.
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(PointerAction::Move, corner + vec2(2.0, 0.0), false)],
    );
    assert!(!dragged.as_ref().unwrap().moved);
    // 40 points right and 30 up: (44, 23) on the plane. A drag, whose begin waits for the tab.
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(
            PointerAction::Move,
            corner + vec2(40.0, -30.0),
            false,
        )],
    );
    let drag = dragged.as_ref().unwrap();
    assert!(drag.moved && drag.wants_begin() && drag.held());
    // Shown at the begin state till a frame comes.
    assert_eq!(drag.shown().map(|read| read.members.points.len()), Some(4));
    // Released: no click, so nothing selected, and the drag ends at its cursor.
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(
            PointerAction::Release,
            corner + vec2(40.0, -30.0),
            false,
        )],
    );
    assert!(mode.selection.is_empty());
    let drag = dragged.as_ref().unwrap();
    assert!(!drag.held() && !drag.over());
    // An edge: the right side pressed 0.4 mm off it at (40.4, 10) is grabbed at (40, 10).
    let mut mode = mode_at(under(rectangle(Vec::new())));
    let mut dragged: Option<drag::Drag> = None;
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(PointerAction::Press, at_mm(40.4, 10.0), false)],
    );
    assert_eq!(
        dragged.as_ref().unwrap().grab,
        silico_core::command::DragGrab::Edge {
            curve: id("na000002"),
            press: [40.0, 10.0]
        }
    );
    // A press that never moves is a click: selected, no drag.
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(PointerAction::Release, at_mm(40.4, 10.0), false)],
    );
    assert!(dragged.is_none());
    assert_eq!(
        mode.selected_ids()
            .iter()
            .map(|i| i.as_str())
            .collect::<Vec<_>>(),
        ["na000002"]
    );
}

#[test]
fn a_fully_defined_member_or_a_constraint_or_a_tool_on_starts_no_drag_and_a_press_selects() {
    let full = read(
        rectangle(vec![length("ka000001", "na000001", "40", true)]),
        SketchState::FullyDefined,
        MemberState::FullyDefined,
        &[],
    );
    let mut mode = mode_at(full);
    let mut dragged: Option<drag::Drag> = None;
    let corner = at_mm(40.0, 20.0);
    frame_of(
        &mut mode,
        &mut dragged,
        &[
            event(PointerAction::Press, corner, false),
            event(PointerAction::Move, corner + vec2(40.0, -30.0), false),
            event(PointerAction::Release, corner + vec2(40.0, -30.0), false),
        ],
    );
    assert!(dragged.is_none());
    assert_eq!(
        mode.selected_ids()
            .iter()
            .map(|i| i.as_str())
            .collect::<Vec<_>>(),
        ["pa000003"]
    );
    // A tool on takes the press: no drag of an under-defined corner either.
    let mut mode = mode_at(under(rectangle(Vec::new())));
    let mut dragged: Option<drag::Drag> = None;
    mode.tools.choose(Some(tools::Choice::Dimension));
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(PointerAction::Press, corner, false)],
    );
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(PointerAction::Move, corner + vec2(40.0, 0.0), false)],
    );
    assert!(dragged.is_none());
    // A read of no model it names (a refused or older read's) drags nothing.
    let mut mode = mode_on(under(rectangle(Vec::new())));
    let mut dragged: Option<drag::Drag> = None;
    mode.sketch = Some(silico_core::document::FeatureId::new("s0000001").unwrap());
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(PointerAction::Press, corner, false)],
    );
    assert!(dragged.is_none());
}

#[test]
fn a_read_mid_drag_leaves_the_drags_drawing_and_a_cursor_off_the_plane_or_past_the_view_is_kept_or_dropped()
 {
    let mut mode = mode_at(under(rectangle(Vec::new())));
    let mut dragged: Option<drag::Drag> = None;
    let corner = at_mm(40.0, 20.0);
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(PointerAction::Press, corner, false)],
    );
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(
            PointerAction::Move,
            corner + vec2(40.0, -30.0),
            false,
        )],
    );
    // Another client's read, its corner elsewhere, taken mid-drag.
    let mut moved = rectangle(Vec::new());
    moved.points[2] = point("pa000003", 90.0, 90.0);
    mode.took(Read {
        model: Revision::OPENED.next().and_then(Revision::next),
        result: Ok(under(moved)),
    });
    frame_of(&mut mode, &mut dragged, &[]);
    let drag = dragged.as_ref().unwrap();
    assert!(drag.held());
    // Still the drag's begin state drawn: the corner at (500, 300), not (1000, −400).
    let corner_drawn: Vec<Pos2> = mode
        .last_drawn()
        .iter()
        .filter(|d| {
            matches!(d, Drawn::Point { .. }) && d.of().is_some_and(|i| i.as_str() == "pa000003")
        })
        .flat_map(Drawn::positions)
        .collect();
    assert_eq!(corner_drawn, [pos2(500.0, 300.0)]);
    // A cursor far past the window is the plane's place there, unclamped: (−5000, 99999) on
    // the screen is (−510, −9949.9).
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(PointerAction::Move, pos2(-5000.0, 99999.0), false)],
    );
    // One that isn't a place (NaN) is dropped, and a release there ends at the last that was.
    let nowhere = pos2(f32::NAN, f32::NAN);
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(PointerAction::Move, nowhere, false)],
    );
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(PointerAction::Release, nowhere, false)],
    );
    let drag = dragged.as_ref().unwrap();
    assert!(!drag.held() && !drag.over() && drag.wants_begin());
    // Esc now has nothing held to cancel.
    assert!(!dragged.as_mut().unwrap().cancel());
}

#[test]
fn a_cancel_ends_a_drag_from_its_begin_and_one_that_never_moved_is_let_go() {
    let corner = at_mm(40.0, 20.0);
    let mut mode = mode_at(under(rectangle(Vec::new())));
    let mut dragged: Option<drag::Drag> = None;
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(PointerAction::Press, corner, false)],
    );
    // Cancelled before it moved: no drag, nothing to send.
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(PointerAction::Cancel, corner, false)],
    );
    assert!(dragged.is_none());
    // Moved, then cancelled: it's to end from its begin state, its begin never sent, so
    // it's over with nothing sent.
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(PointerAction::Press, corner, false)],
    );
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(PointerAction::Move, corner + vec2(40.0, 0.0), false)],
    );
    frame_of(
        &mut mode,
        &mut dragged,
        &[event(
            PointerAction::Cancel,
            corner + vec2(40.0, 0.0),
            false,
        )],
    );
    let drag = dragged.as_ref().unwrap();
    assert!(!drag.held() && !drag.wants_begin() && drag.over());
    frame_of(&mut mode, &mut dragged, &[]);
    assert!(dragged.is_none());
}

#[test]
fn a_double_click_on_a_dimension_opens_its_box_with_its_value_whose_text_is_asked_as_read() {
    use dimension::{Ask, ModifyAction};
    use silico_core::document::json::Value;
    let read = under(rectangle(vec![length("ka000001", "na000001", "40", true)]));
    let mut mode = mode_at(read);
    let mut dragged: Option<drag::Drag> = None;
    frame_of(&mut mode, &mut dragged, &[]);
    // Its value's label, where the read's drawing put it.
    let label = mode
        .last_drawn()
        .iter()
        .find_map(|d| match d {
            Drawn::Label {
                of: Some(of), at, ..
            } if of.as_str() == "ka000001" => Some(*at),
            _ => None,
        })
        .unwrap();
    let mut clicks = press(label, false).to_vec();
    clicks.extend(press(label, false));
    clicks.push(event(PointerAction::DoubleClick, label, false));
    frame_of(&mut mode, &mut dragged, &clicks);
    let edit = mode.edit_box.as_ref().unwrap();
    assert_eq!((edit.text.as_str(), edit.unit), ("40", "mm"));
    assert_eq!(edit.constraint, id("ka000001"));
    let view = mode.modify_view().unwrap();
    assert!(!view.placing && !view.waiting);
    // "25mm": the core's reader reads no JSON of it; said, nothing asked.
    mode.modify(ModifyAction::Typed("25mm".to_owned()));
    mode.modify(ModifyAction::Accept);
    assert!(mode.take_asks().is_empty());
    assert!(mode.edit_box.as_ref().unwrap().said.is_some());
    // " 1e1 ": asked whole, as the reader reads it: the number's own text, "1e1".
    mode.modify(ModifyAction::Typed(" 1e1 ".to_owned()));
    assert!(mode.edit_box.as_ref().unwrap().said.is_none());
    mode.modify(ModifyAction::Accept);
    match mode.take_asks().as_slice() {
        [
            Ask::Value {
                constraint, value, ..
            },
        ] => {
            assert_eq!(*constraint, id("ka000001"));
            assert_eq!(value.value, Value::Number("1e1".to_owned()));
        }
        other => panic!("{other:?}"),
    }
    assert!(mode.modify_view().unwrap().waiting);
    // Refused by the core: said in the box, which stays open; then taken, which closes it.
    mode.value_answered(Revision::OPENED, Err("refused".to_owned()));
    let edit = mode.edit_box.as_ref().unwrap();
    assert_eq!((edit.said.as_deref(), edit.sent), (Some("refused"), false));
    mode.modify(ModifyAction::Accept);
    mode.take_asks();
    mode.value_answered(Revision::OPENED.next().unwrap(), Ok(false));
    assert!(mode.edit_box.is_none() && mode.edited.is_none());
    // Esc's way: a box closed with nothing asked.
    frame_of(&mut mode, &mut dragged, &clicks);
    mode.modify(ModifyAction::Skip);
    assert!(mode.edit_box.is_none());
    assert!(mode.take_asks().is_empty());
}

#[test]
fn a_driven_dimension_double_clicked_opens_no_modify_box() {
    let read = under(rectangle(vec![length("ka000001", "na000001", "40", false)]));
    let mut mode = mode_at(read);
    let mut dragged: Option<drag::Drag> = None;
    frame_of(&mut mode, &mut dragged, &[]);
    let label = mode
        .last_drawn()
        .iter()
        .find_map(|d| match d {
            Drawn::Label {
                of: Some(of),
                at,
                text,
                ..
            } if of.as_str() == "ka000001" => {
                assert_eq!(text, "(40)");
                Some(*at)
            }
            _ => None,
        })
        .unwrap();
    let mut clicks = press(label, false).to_vec();
    clicks.extend(press(label, false));
    clicks.push(event(PointerAction::DoubleClick, label, false));
    frame_of(&mut mode, &mut dragged, &clicks);
    assert!(mode.edit_box.is_none());
    assert!(mode.modify_view().is_none());
    // Its click selected it all the same.
    assert_eq!(
        mode.selected_ids()
            .iter()
            .map(|i| i.as_str())
            .collect::<Vec<_>>(),
        ["ka000001"]
    );
}

#[test]
fn after_a_drag_ends_a_press_hits_what_its_last_frame_drew_until_the_read_of_its_edit() {
    let begin = under(rectangle(Vec::new()));
    let mut moved = begin.clone();
    if let Some(p) = moved
        .members
        .points
        .iter_mut()
        .find(|p| p.id == id("pa000003"))
    {
        p.x = Coordinate::new(45.0).unwrap();
        p.y = Coordinate::new(25.0).unwrap();
    }
    let mut mode = mode_at(begin.clone());
    // A drag of the corner, ended at revision 3, its last frame the corner at (45, 25); the
    // read of revision 3 not yet come.
    let mut dragged = Some(drag::Drag::ended_at(
        silico_core::document::FeatureId::new("s0000001").unwrap(),
        silico_core::command::DragGrab::Point(id("pa000003")),
        begin,
        moved,
        Revision::OPENED.next().and_then(Revision::next).unwrap(),
    ));
    // A click at (45, 25), where the corner is drawn, 7 mm from anything the old read has.
    frame_of(&mut mode, &mut dragged, &press(at_mm(45.0, 25.0), false));
    assert_eq!(
        mode.selected_ids()
            .iter()
            .map(|i| i.as_str())
            .collect::<Vec<_>>(),
        ["pa000003"]
    );
    // And it's drawn there.
    let corner: Vec<Pos2> = mode
        .last_drawn()
        .iter()
        .filter(|d| {
            matches!(d, Drawn::Point { selected: true, .. })
                && d.of().is_some_and(|i| i.as_str() == "pa000003")
        })
        .flat_map(Drawn::positions)
        .collect();
    assert_eq!(corner, [at_mm(45.0, 25.0)]);
}
