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
use silico_core::sketch_tools::{Preview, Relation, Shown as Showing};

use super::draw::{
    Drawn, Flat, GlyphKind, LabelKind, MAX_CURVE_SEGMENTS, Scale, Shown, StrokeKind, View, drawing,
    preview, round_points, segments, signed_area, triangulate,
};
use super::hit::{Hit, hit};
use super::style::*;
use super::*;

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

fn under(sketch: Sketch) -> SketchRead {
    read(
        sketch,
        SketchState::UnderDefined,
        MemberState::UnderDefined,
        &[],
    )
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
    drawing(read, &view(), Shown::default(), &Default::default())
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
            .all(|d| d.colour() == SKETCH_DIMENSION)
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
    assert_eq!((kind, colour), (GlyphKind::Horizontal, SKETCH_GLYPH_ICON));
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
    let read = under(sketch);
    let view = View {
        chord_tolerance: 0.5,
        ..view()
    };
    let drawn = drawing(&read, &view, Shown::default(), &Default::default());
    let [Drawn::Stroke { points, .. }] = of(&drawn, "aa000001").as_slice() else {
        panic!("{drawn:?}");
    };
    assert_eq!(points.len(), 9);
    let centre = pos2(100.0, 500.0);
    assert!(near(points[0], pos2(200.0, 500.0)));
    assert!(near(points[8], pos2(100.0, 400.0)));
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
    let points = round_points([0.0, 0.0], 10.0, 0.0, TAU, &tall, 0.5);
    assert_eq!(points.len(), segments(300.0, TAU, 0.5) + 1);
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
    );
    assert!(off.iter().all(|d| !matches!(d, Drawn::Fill { .. })));
}

#[test]
fn ear_clipping_covers_a_concave_polygon_and_a_holed_one_exactly() {
    // An L: (0,0) (40,0) (40,10) (10,10) (10,30) (0,30), clockwise as given: its foot 40
    // by 10 and its upright 10 by 20, 600 mm².
    let l = vec![
        [0.0, 0.0],
        [0.0, 30.0],
        [10.0, 30.0],
        [10.0, 10.0],
        [40.0, 10.0],
        [40.0, 0.0],
    ];
    let total = |t: &[[[f64; 2]; 3]]| t.iter().map(|t| signed_area(&t[..]).abs()).sum::<f64>();
    let triangles = triangulate(l, Vec::new());
    assert_eq!(triangles.len(), 4);
    assert!(
        (total(&triangles) - 600.0).abs() < 1e-9,
        "{}",
        total(&triangles)
    );
    // Every triangle counter-clockwise, so none folds back over another.
    assert!(triangles.iter().all(|t| signed_area(&t[..]) > 0.0));
    // A square with two holes: 100 less 4 and 9.
    let square = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
    let holes = vec![
        vec![[1.0, 1.0], [3.0, 1.0], [3.0, 3.0], [1.0, 3.0]],
        vec![[5.0, 5.0], [8.0, 5.0], [8.0, 8.0], [5.0, 8.0]],
    ];
    let triangles = triangulate(square, holes);
    assert!(
        (total(&triangles) - 87.0).abs() < 1e-9,
        "{}",
        total(&triangles)
    );
    // Too few points to clip: nothing.
    assert!(triangulate(vec![[0.0, 0.0], [1.0, 0.0]], Vec::new()).is_empty());
}

#[test]
fn the_grid_steps_with_the_zoom_its_axes_in_the_planes_colours() {
    // Ten points a millimetre: a line every 5 mm. From (−1, −1) to (11, 6) in view: lines
    // at x = −5, 0, 5, 10, 15 and y = −5, 0, 5, 10, those through 0 the 50 mm ones; then
    // the two axes, the XY plane's x red and y green, at 55%.
    let read = under(rectangle(Vec::new()));
    let seen = View {
        visible: Some(([-1.0, -1.0], [11.0, 6.0])),
        ..view()
    };
    let drawn = drawing(&read, &seen, Shown::default(), &Default::default());
    let count = |kind| {
        drawn
            .iter()
            .filter(|d| matches!(d, Drawn::Stroke { kind: k, .. } if *k == kind))
            .count()
    };
    assert_eq!(count(StrokeKind::GridMinor), 7);
    assert_eq!(count(StrokeKind::GridMajor), 2);
    let axes: Vec<egui::Color32> = drawn
        .iter()
        .filter_map(|d| match d {
            Drawn::Stroke {
                kind: StrokeKind::Axis,
                colour,
                ..
            } => Some(*colour),
            _ => None,
        })
        .collect();
    assert_eq!(
        axes,
        [AXIS_X.gamma_multiply(0.55), AXIS_Y.gamma_multiply(0.55)]
    );
    // Far out (a thousand millimetres a point), across ±10 km: too many lines, none drawn.
    let far = View {
        scale: Scale(1000.0),
        visible: Some(([-1e7, -1e7], [1e7, 1e7])),
        ..view()
    };
    let drawn = drawing(&read, &far, Shown::default(), &Default::default());
    assert!(drawn.iter().all(|d| !matches!(
        d,
        Drawn::Stroke {
            kind: StrokeKind::GridMinor | StrokeKind::GridMajor | StrokeKind::Axis,
            ..
        }
    )));
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
        [(vec![pos2(200.0, 500.0), pos2(200.0, 300.0)], SKETCH_GUIDE)]
    );
    assert_eq!(
        strokes(StrokeKind::Preview),
        [(vec![pos2(100.0, 500.0), pos2(200.0, 500.0)], SKETCH_PREVIEW)]
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
            (GlyphKind::Horizontal, pos2(218.0, 484.0), SKETCH_GUIDE),
            (GlyphKind::Coincident, pos2(236.0, 484.0), SKETCH_SNAP),
        ]
    );
    assert!(drawn.contains(&Drawn::Ring {
        at: pos2(200.0, 500.0),
        colour: SKETCH_SNAP
    }));
    assert!(drawn.contains(&Drawn::Label {
        of: None,
        kind: LabelKind::Tool,
        at: pos2(214.0, 512.0),
        text: "10.00 mm".to_owned(),
        colour: SKETCH_DIMENSION,
        selected: false,
    }));
    assert!(preview(&Preview::default(), &view()).is_empty());
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
    let drawn = drawing(&read, &view, Shown::default(), &Default::default());
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
    let drawn = drawing(&read, &view, Shown::default(), &Default::default());
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
    let drawn = drawing(&read, &view, Shown::default(), &Default::default());
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
    // Ctrl on the corner again takes it away; a plain press on the right side replaces all.
    frame(&mut mode, &[press(pos2(100.0, 500.0), true)], None);
    assert_eq!(selected(&mode), ["na000003"]);
    frame(&mut mode, &[press(pos2(500.0, 400.0), false)], None);
    assert_eq!(selected(&mode), ["na000002"]);
    // A press on nothing, 10 mm from every side, clears it; a move selects nothing.
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
fn a_selected_member_is_drawn_selected_its_state_colour_kept() {
    let read = under(rectangle(vec![horizontal("ka000001", "na000001")]));
    let selected = [id("na000001"), id("ka000001"), id("pa000002")]
        .into_iter()
        .collect();
    let drawn = drawing(&read, &view(), Shown::default(), &selected);
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
    let mut drawn = drawing(&read, &view(), Shown::default(), &selected);
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
