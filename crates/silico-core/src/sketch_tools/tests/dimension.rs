//! Smart Dimension driven by events, as a viewport sends them, against sketches written here.
//! Every expected value is worked by hand in the test's comment from the decisions (SD1 to
//! SD9, docs/design/m5.0-app.md, section 9, *Smart Dimension*), the study's §11, FreeCAD's
//! tool (SD4's distance, the parallel test), and plain geometry: never from the tool. Each
//! `sketch_add` it makes is put through the gate as the command puts it (`sketch_edit::add`),
//! and the next event sees the sketch it leaves.

use super::*;
use crate::command::sketch::{Region, SketchRead};
use crate::document::{Angle, Length, LineSide, SketchArc, SketchCircle};
use crate::sketch_states::{ConstraintState, SketchState, SketchStates};
use std::collections::{BTreeMap, BTreeSet};

/// Smart Dimension driven against a sketch, which takes each `sketch_add` it makes.
struct Dim {
    tool: SmartDimension,
    sketch: Sketch,
    revision: Revision,
    tolerance: f64,
    /// The sketch's states in the read each event comes with.
    states: SketchStates,
    ids: Counter,
    /// Each add's members, as made.
    made: Vec<Members>,
    messages: Vec<Message>,
    left: bool,
}

impl Dim {
    fn new(sketch: Sketch) -> Self {
        Dim {
            tool: SmartDimension::new(),
            sketch,
            revision: Revision::OPENED,
            tolerance: TOLERANCE,
            // Each add an independent dimension: the solve takes a freedom.
            states: SketchStates {
                dofs: 1000,
                ..under()
            },
            ids: Counter(100),
            made: Vec::new(),
            messages: Vec::new(),
            left: false,
        }
    }

    fn send(&mut self, event: DimensionEvent) -> Outcome {
        let sketch_id = sketch_id();
        let context = Context {
            sketch_id: &sketch_id,
            revision: self.revision,
            sketch: &self.sketch,
            tolerance: self.tolerance,
            automatic_relations: true,
            construction: false,
        };
        let outcome = self
            .tool
            .handle(&context, &self.states, &mut self.ids, event);
        if let Some(members) = &outcome.add {
            self.revision = self.revision.next().unwrap();
            self.tool.added(outcome.token.unwrap(), self.revision);
            let edited = sketch_edit::add(&self.sketch, members)
                .unwrap_or_else(|refused| panic!("the gate refused {members:?}: {refused:?}"));
            assert!(members.points.is_empty() && members.lines.is_empty());
            assert_eq!(members.constraints.len(), 1, "{members:?}");
            self.made.push(members.clone());
            self.sketch = edited.sketch;
            self.states.dofs -= 1;
        }
        if let Some(message) = &outcome.message {
            self.messages.push(message.clone());
        }
        self.left |= outcome.left;
        outcome
    }

    fn to(&mut self, x: f64, y: f64) -> &mut Self {
        self.send(DimensionEvent::Move([x, y]));
        self
    }
    fn click(&mut self, x: f64, y: f64) -> &mut Self {
        self.to(x, y);
        self.send(DimensionEvent::Press([x, y]));
        self.send(DimensionEvent::Release([x, y]));
        self
    }
    fn right(&mut self, x: f64, y: f64) -> &mut Self {
        self.send(DimensionEvent::RightClick([x, y]));
        self
    }
    fn typed(&mut self, text: &str) -> &mut Self {
        self.send(DimensionEvent::Typed(text.to_owned()));
        self
    }
    fn enter(&mut self) -> &mut Self {
        self.send(DimensionEvent::Enter);
        self
    }
    fn escape(&mut self) -> &mut Self {
        self.send(DimensionEvent::Escape);
        self
    }

    /// The form of the last dimension placed.
    fn placed(&self) -> Form {
        self.made.last().unwrap().constraints[0].form.clone()
    }

    /// The Modify box's text, if it's open.
    fn modify(&self) -> Option<String> {
        self.tool.preview().modify.clone()
    }

    /// The dimension the preview shows.
    fn shown(&self) -> Option<Form> {
        self.tool.preview().dimension.clone()
    }
}

/// A read's states with the sketch in `state`, nothing flagged.
fn states(state: SketchState) -> SketchStates {
    SketchStates {
        state,
        dofs: 0,
        points: BTreeMap::new(),
        lines: BTreeMap::new(),
        circles: BTreeMap::new(),
        arcs: BTreeMap::new(),
        constraints: BTreeMap::new(),
        solve_problem: None,
    }
}

fn under() -> SketchStates {
    states(SketchState::UnderDefined)
}

fn r(text: &str) -> Reference {
    Reference::Element(id(text))
}

fn mm(v: &str) -> Length {
    Length::parse(v).unwrap()
}

fn deg(v: &str) -> Angle {
    Angle::parse(v).unwrap()
}

fn with_line(mut s: Sketch, name: &str, from: [f64; 2], to: [f64; 2]) -> Sketch {
    // Its points: `f` and `t` in place of its ID's first character.
    let (f, t) = (format!("f{}", &name[1..]), format!("t{}", &name[1..]));
    s.points.push(point(&f, from[0], from[1]));
    s.points.push(point(&t, to[0], to[1]));
    s.lines.push(line(name, &f, &t));
    s
}

fn with_point(mut s: Sketch, name: &str, at: [f64; 2]) -> Sketch {
    s.points.push(point(name, at[0], at[1]));
    s
}

fn with_circle(mut s: Sketch, name: &str, centre: [f64; 2], radius: f64) -> Sketch {
    // Its centre: `c` in place of its ID's first character.
    let c = format!("c{}", &name[1..]);
    s.points.push(point(&c, centre[0], centre[1]));
    s.circles.push(SketchCircle {
        id: id(name),
        center: id(&c),
        radius: Coordinate::new(radius).unwrap(),
        construction: false,
    });
    s
}

fn construction(mut s: Sketch, name: &str) -> Sketch {
    s.lines
        .iter_mut()
        .find(|l| l.id == id(name))
        .unwrap()
        .construction = true;
    s
}

// --- SD1: one pick ---------------------------------------------------------------------

#[test]
fn a_line_is_its_length_placed_with_the_value_typed() {
    // (10, 10) to (40, 50): 30 across, 40 up, so 50 long. Picked at its middle (25, 30),
    // placed off it at (60, 5): the box opens on "50"; 12.5 typed and Enter: `length` 12.5,
    // driving (SD2).
    let mut d = Dim::new(with_line(empty(), "n0000001", [10.0, 10.0], [40.0, 50.0]));
    d.click(25.0, 30.0).click(60.0, 5.0);
    assert_eq!(d.modify().as_deref(), Some("50"));
    assert_eq!(
        d.shown(),
        Some(Form::Length {
            line: r("n0000001"),
            value: mm("50"),
            driving: true
        })
    );
    d.typed("12.5").enter();
    assert_eq!(
        d.placed(),
        Form::Length {
            line: r("n0000001"),
            value: mm("12.5"),
            driving: true
        }
    );
    assert_eq!(d.modify(), None);
    // Busy until a read of the add's revision has come: the next event's.
    assert!(d.tool.busy());
    d.to(60.0, 5.0);
    assert!(!d.tool.busy());
}

#[test]
fn a_circle_is_its_diameter_and_an_arc_its_radius() {
    // A circle about (0, 30), radius 7: its diameter, 14 (SD8: no radius). An arc about
    // (50, 50) from (60, 50) to (50, 60): its radius, 10, picked at 45° on it,
    // (50 + 10/√2, 50 + 10/√2).
    let mut s = with_circle(empty(), "k0000001", [0.0, 30.0], 7.0);
    s.points.push(point("c0000002", 50.0, 50.0));
    s.points.push(point("q0000003", 60.0, 50.0));
    s.points.push(point("q0000004", 50.0, 60.0));
    s.arcs.push(SketchArc {
        id: id("a0000001"),
        center: id("c0000002"),
        start: id("q0000003"),
        end: id("q0000004"),
        construction: false,
    });
    let mut d = Dim::new(s);
    d.click(7.0, 30.0).click(20.0, 45.0).escape();
    assert_eq!(
        d.placed(),
        Form::Diameter {
            curve: r("k0000001"),
            value: mm("14"),
            driving: true
        }
    );
    let h = 10.0 / std::f64::consts::SQRT_2;
    d.click(50.0 + h, 50.0 + h).click(80.0, 80.0).escape();
    assert_eq!(
        d.placed(),
        Form::Radius {
            curve: r("a0000001"),
            value: mm("10"),
            driving: true
        }
    );
}

#[test]
fn a_point_or_an_axis_alone_places_nothing() {
    let mut d = Dim::new(with_point(empty(), "p0000001", [10.0, 10.0]));
    d.click(10.0, 10.0).click(30.0, 30.0);
    assert_eq!(d.modify(), None);
    assert_eq!(d.tool.preview().picks, vec![r("p0000001")]);
    d.escape().click(50.0, 0.2).click(30.0, 30.0);
    assert_eq!(d.tool.preview().picks, vec![Reference::XAxis]);
    assert_eq!(d.modify(), None);
    assert!(d.made.is_empty() && d.messages.is_empty());
}

// --- SD1: two points, by the pointer; signs and order ---------------------------------

/// Two points: p (10, 10), q (40, 30). The box they span: x 10 to 40, y 10 to 30.
fn two_points() -> Sketch {
    with_point(
        with_point(empty(), "p0000001", [10.0, 10.0]),
        "p0000002",
        [40.0, 30.0],
    )
}

#[test]
fn two_points_are_horizontal_above_the_box_vertical_beside_it_and_aligned_elsewhere() {
    // Above (25, 45) or below (25, -5): x(q) − x(p) = 30. Beside (55, 20) or (-5, 20):
    // y(q) − y(p) = 20. Inside (25, 20), or off a corner (60, 50): √(30² + 20²) = √1300 =
    // 36.05551275..., to the nanometre 36.055513.
    let cases: [([f64; 2], Form); 6] = [
        (
            [25.0, 45.0],
            Form::DistanceX {
                a: r("p0000001"),
                b: r("p0000002"),
                value: mm("30"),
                driving: true,
            },
        ),
        (
            [25.0, -5.0],
            Form::DistanceX {
                a: r("p0000001"),
                b: r("p0000002"),
                value: mm("30"),
                driving: true,
            },
        ),
        (
            [55.0, 20.0],
            Form::DistanceY {
                a: r("p0000001"),
                b: r("p0000002"),
                value: mm("20"),
                driving: true,
            },
        ),
        (
            [-5.0, 20.0],
            Form::DistanceY {
                a: r("p0000001"),
                b: r("p0000002"),
                value: mm("20"),
                driving: true,
            },
        ),
        (
            [25.0, 20.0],
            Form::Distance {
                a: r("p0000001"),
                b: r("p0000002"),
                value: mm("36.055513"),
                driving: true,
            },
        ),
        (
            [60.0, 50.0],
            Form::Distance {
                a: r("p0000001"),
                b: r("p0000002"),
                value: mm("36.055513"),
                driving: true,
            },
        ),
    ];
    for (at, want) in cases {
        let mut d = Dim::new(two_points());
        d.click(10.0, 10.0).click(40.0, 30.0).to(at[0], at[1]);
        assert_eq!(d.shown(), Some(want.clone()), "{at:?}");
        d.click(at[0], at[1]).escape();
        assert_eq!(d.placed(), want, "{at:?}");
    }
}

#[test]
fn horizontal_and_vertical_distances_start_positive_whichever_point_is_picked_first() {
    // q (40, 30) picked first: `a` is still p, to the left and below, so x(b) − x(a) = 30
    // and y(b) − y(a) = 20, both positive. `distance` keeps the order picked. Points to the
    // right and above, the other way: p' (40, 10), q' (10, 30): `distance_y` from p' (below)
    // to q', 20; `distance_x` from q' (left) to p', 30.
    let mut d = Dim::new(two_points());
    d.click(40.0, 30.0)
        .click(10.0, 10.0)
        .click(25.0, 45.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::DistanceX {
            a: r("p0000001"),
            b: r("p0000002"),
            value: mm("30"),
            driving: true
        }
    );
    d.click(40.0, 30.0)
        .click(10.0, 10.0)
        .click(55.0, 20.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::DistanceY {
            a: r("p0000001"),
            b: r("p0000002"),
            value: mm("20"),
            driving: true
        }
    );
    d.click(40.0, 30.0)
        .click(10.0, 10.0)
        .click(25.0, 20.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::Distance {
            a: r("p0000002"),
            b: r("p0000001"),
            value: mm("36.055513"),
            driving: true
        }
    );
    let crossed = with_point(
        with_point(empty(), "p0000001", [40.0, 10.0]),
        "p0000002",
        [10.0, 30.0],
    );
    let mut d = Dim::new(crossed);
    d.click(40.0, 10.0)
        .click(10.0, 30.0)
        .click(25.0, 45.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::DistanceX {
            a: r("p0000002"),
            b: r("p0000001"),
            value: mm("30"),
            driving: true
        }
    );
    d.click(40.0, 10.0)
        .click(10.0, 30.0)
        .click(55.0, 20.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::DistanceY {
            a: r("p0000001"),
            b: r("p0000002"),
            value: mm("20"),
            driving: true
        }
    );
}

#[test]
fn a_negative_value_typed_for_a_horizontal_distance_is_taken_as_typed() {
    // `distance_x` is signed: -30 flips q to the left of p, as the help's negative values do.
    let mut d = Dim::new(two_points());
    d.click(10.0, 10.0)
        .click(40.0, 30.0)
        .click(25.0, 45.0)
        .typed("-30")
        .enter();
    assert_eq!(
        d.placed(),
        Form::DistanceX {
            a: r("p0000001"),
            b: r("p0000002"),
            value: mm("-30"),
            driving: true
        }
    );
}

#[test]
fn the_origin_is_a_point_to_dimension_from() {
    // The origin and p (10, 10): above the box, x(p) − x(origin) = 10.
    let mut d = Dim::new(with_point(empty(), "p0000001", [10.0, 10.0]));
    d.click(0.1, 0.1)
        .click(10.0, 10.0)
        .click(5.0, 20.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::DistanceX {
            a: Reference::Origin,
            b: r("p0000001"),
            value: mm("10"),
            driving: true
        }
    );
}

#[test]
fn two_points_at_one_place_have_no_distance_but_a_horizontal_and_vertical_offset() {
    // p (10, 10) and p' 3e-7 mm right of it and above it: neither level nor plumb within
    // 1e-7, and √2·3e-7 = 4.2e-7 apart, 0 to the nanometre: no direction to hold them apart
    // in, so no `distance`; but `distance_x`, x(p') − x(p) = 0 to the nanometre, is a value
    // a signed form holds. Each picked where it's the nearest point: (9.9999, 9.9999), and
    // (10.000001, 10.000001). The box is x 10 to 10.0000003, so (10.0000001, 20) is above it.
    let s = with_point(
        with_point(empty(), "p0000001", [10.0, 10.0]),
        "p0000002",
        [10.0000003, 10.0000003],
    );
    let mut d = Dim::new(s);
    d.click(9.9999, 9.9999).click(10.000001, 10.000001);
    assert_eq!(d.tool.preview().picks, vec![r("p0000001"), r("p0000002")]);
    d.click(30.0, 30.0);
    assert_eq!(d.messages, vec![Message::NoDistance]);
    assert_eq!(d.modify(), None);
    d.click(10.0000001, 20.0).escape();
    assert_eq!(
        d.placed(),
        Form::DistanceX {
            a: r("p0000001"),
            b: r("p0000002"),
            value: mm("0"),
            driving: true
        }
    );
}

#[test]
fn level_points_are_horizontal_and_plumb_ones_vertical_wherever_the_pointer_is() {
    // FreeCAD's 76a84f63ab. p (10, 10) and q (40, 10 + 5e-8), level within 1e-7: with the
    // pointer off the box's corner (60, 50), and inside its line, `distance_x` 30. p and
    // q' (10 + 5e-8, 40), plumb: `distance_y` 30 off its corner. q'' (40, 10 + 2e-7), not
    // level: off the corner, `distance`, √(30² + (2e-7)²) = 30 to the nanometre.
    let level = |dy: f64| {
        with_point(
            with_point(empty(), "p0000001", [10.0, 10.0]),
            "p0000002",
            [40.0, 10.0 + dy],
        )
    };
    for at in [[60.0, 50.0], [25.0, 10.0], [-5.0, 10.0]] {
        let mut d = Dim::new(level(5e-8));
        d.click(10.0, 10.0)
            .click(40.0, 10.0)
            .click(at[0], at[1])
            .escape();
        assert_eq!(
            d.placed(),
            Form::DistanceX {
                a: r("p0000001"),
                b: r("p0000002"),
                value: mm("30"),
                driving: true
            },
            "{at:?}"
        );
    }
    let plumb = with_point(
        with_point(empty(), "p0000001", [10.0, 10.0]),
        "p0000002",
        [10.0 + 5e-8, 40.0],
    );
    let mut d = Dim::new(plumb);
    d.click(10.0, 10.0)
        .click(10.0, 40.0)
        .click(60.0, 50.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::DistanceY {
            a: r("p0000001"),
            b: r("p0000002"),
            value: mm("30"),
            driving: true
        }
    );
    let mut d = Dim::new(level(2e-7));
    d.click(10.0, 10.0)
        .click(40.0, 10.0)
        .click(60.0, 50.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::Distance {
            a: r("p0000001"),
            b: r("p0000002"),
            value: mm("30"),
            driving: true
        }
    );
}

#[test]
fn coincident_points_keep_their_pick_order() {
    // a (10, 10) and the circle about (10, 10), radius 5, its centre c: level and plumb, so
    // `distance_x`, 0, where neither is to the left: in the order picked. (10, 10) picks a,
    // whose ID sorts before c's; (15, 10) the circle, measured at its centre.
    let s = with_point(
        with_circle(empty(), "k0000001", [10.0, 10.0], 5.0),
        "a0000001",
        [10.0, 10.0],
    );
    let mut d = Dim::new(s);
    d.click(10.0, 10.0)
        .click(15.0, 10.0)
        .click(30.0, 30.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::DistanceX {
            a: r("a0000001"),
            b: r("c0000001"),
            value: mm("0"),
            driving: true
        }
    );
    d.click(15.0, 10.0)
        .click(10.0, 10.0)
        .click(30.0, 30.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::DistanceX {
            a: r("c0000001"),
            b: r("a0000001"),
            value: mm("0"),
            driving: true
        }
    );
}

// --- SD1: a point and a line; side ----------------------------------------------------

#[test]
fn a_points_distance_from_a_line_is_on_the_side_it_is_drawn() {
    // The line from (10, 0) to (50, 0) faces +x: (30, 7) is 7 to its left, (30, -4) 4 to its
    // right. The line drawn the other way, from (50, 0) to (10, 0), faces −x: (30, 7) is to
    // its right. The order picked doesn't matter.
    let s = with_point(
        with_point(
            with_line(empty(), "n0000001", [10.0, 0.0], [50.0, 0.0]),
            "p0000001",
            [30.0, 7.0],
        ),
        "p0000002",
        [30.0, -4.0],
    );
    let pld = |point: &str, value: &str, side| Form::PointLineDistance {
        point: r(point),
        line: r("n0000001"),
        value: mm(value),
        side,
        driving: true,
    };
    let mut d = Dim::new(s.clone());
    d.click(30.0, 7.0)
        .click(20.0, 0.0)
        .click(40.0, 20.0)
        .escape();
    assert_eq!(d.placed(), pld("p0000001", "7", LineSide::Left));
    d.click(20.0, 0.0)
        .click(30.0, -4.0)
        .click(40.0, -20.0)
        .escape();
    assert_eq!(d.placed(), pld("p0000002", "4", LineSide::Right));
    let mut reversed = s;
    let l = reversed.lines.iter_mut().next().unwrap();
    std::mem::swap(&mut l.from, &mut l.to);
    let mut d = Dim::new(reversed);
    d.click(30.0, 7.0)
        .click(20.0, 0.0)
        .click(40.0, 20.0)
        .escape();
    assert_eq!(d.placed(), pld("p0000001", "7", LineSide::Right));
}

#[test]
fn a_point_on_the_line_has_no_side_and_places_nothing() {
    // The line's own end, (10, 0), and a point on its extension, (70, 0).
    let s = with_point(
        with_line(empty(), "n0000001", [10.0, 0.0], [50.0, 0.0]),
        "p0000001",
        [70.0, 0.0],
    );
    let mut d = Dim::new(s);
    d.click(10.0, 0.0).click(30.0, 0.0).click(30.0, 20.0);
    assert_eq!(d.modify(), None);
    d.escape().escape().click(70.0, 0.0).click(30.0, 0.0);
    assert_eq!(d.tool.preview().unavailable, Some(Message::NoDistance));
    d.click(30.0, 20.0);
    assert_eq!(d.messages, vec![Message::NoDistance, Message::NoDistance]);
    assert!(d.made.is_empty());
}

// --- SD1: to a centre by default ------------------------------------------------------

#[test]
fn a_circle_with_anything_else_is_measured_at_its_centre() {
    // A circle about (30, 20), radius 5, picked on its edge at (35, 20):
    // - with the line (10, 0)-(50, 0): its centre 20 to the line's left;
    // - with the point (30, 50), plumb with the centre: centre to point, vertically, 30
    //   (FreeCAD's 76a84f63ab: wherever the pointer is);
    // - with the circle about (60, 60), radius 4, picked at (64, 60): centre to centre,
    //   √(30² + 40²) = 50.
    let s = with_circle(
        with_circle(
            with_point(
                with_line(empty(), "n0000001", [10.0, 0.0], [50.0, 0.0]),
                "p0000001",
                [30.0, 50.0],
            ),
            "k0000001",
            [30.0, 20.0],
            5.0,
        ),
        "k0000002",
        [60.0, 60.0],
        4.0,
    );
    let mut d = Dim::new(s);
    d.click(35.0, 20.0)
        .click(20.0, 0.0)
        .click(40.0, 30.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::PointLineDistance {
            point: r("c0000001"),
            line: r("n0000001"),
            value: mm("20"),
            side: LineSide::Left,
            driving: true
        }
    );
    d.click(35.0, 20.0)
        .click(30.0, 50.0)
        .click(10.0, 70.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::DistanceY {
            a: r("c0000001"),
            b: r("p0000001"),
            value: mm("30"),
            driving: true
        }
    );
    d.click(35.0, 20.0)
        .click(64.0, 60.0)
        .click(0.0, 100.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::Distance {
            a: r("c0000001"),
            b: r("c0000002"),
            value: mm("50"),
            driving: true
        }
    );
}

// --- SD1: a centreline's near side; SD3 -----------------------------------------------

#[test]
fn a_point_and_a_centreline_give_its_distance_on_its_side_and_nothing_across() {
    // A construction line from (0, -10) to (0, 40), facing +y; p (12, 5) is 12 to its right
    // (facing +y, +x is right). The pointer on p's side, (20, 5): `point_line_distance` 12;
    // across, (-20, 5): a diameter, not offered (SD3). On the line itself, (0, 5): its side.
    let s = construction(
        with_point(
            with_line(empty(), "n0000001", [0.0, -10.0], [0.0, 40.0]),
            "p0000001",
            [12.0, 5.0],
        ),
        "n0000001",
    );
    let want = Form::PointLineDistance {
        point: r("p0000001"),
        line: r("n0000001"),
        value: mm("12"),
        side: LineSide::Right,
        driving: true,
    };
    let mut d = Dim::new(s.clone());
    d.click(12.0, 5.0).click(0.0, 20.0).to(-20.0, 5.0);
    assert_eq!(d.shown(), None);
    assert_eq!(
        d.tool.preview().unavailable,
        Some(Message::AcrossCentreline)
    );
    d.click(-20.0, 5.0);
    assert_eq!(d.messages, vec![Message::AcrossCentreline]);
    assert_eq!(d.modify(), None);
    d.to(0.0, 5.0);
    assert_eq!(d.shown(), Some(want.clone()));
    d.click(20.0, 5.0).escape();
    assert_eq!(d.placed(), want);
    // The same line, not construction: the same distance, from either side.
    let mut plain = s;
    plain.lines[0].construction = false;
    let mut d = Dim::new(plain);
    d.click(12.0, 5.0)
        .click(0.0, 20.0)
        .click(-20.0, 5.0)
        .escape();
    assert_eq!(d.placed(), want);
}

#[test]
fn an_axis_is_a_centreline() {
    // p (12, 5) and the y axis (facing +y): 12 to its right; across it, (-5, 5), nothing.
    let mut d = Dim::new(with_point(empty(), "p0000001", [12.0, 5.0]));
    d.click(12.0, 5.0).click(0.2, 30.0).click(-5.0, 5.0);
    assert_eq!(d.messages, vec![Message::AcrossCentreline]);
    d.click(20.0, 5.0).escape();
    assert_eq!(
        d.placed(),
        Form::PointLineDistance {
            point: r("p0000001"),
            line: Reference::YAxis,
            value: mm("12"),
            side: LineSide::Right,
            driving: true
        }
    );
}

#[test]
fn a_lock_on_a_centrelines_near_side_holds_across_it() {
    // Right-clicked at (20, 5) on p's side, then placed across at (-20, 5): the lock holds
    // the kind, the pointer only moves the dimension.
    let mut d = Dim::new(with_point(empty(), "p0000001", [12.0, 5.0]));
    d.click(12.0, 5.0).click(0.2, 30.0).right(20.0, 5.0);
    assert!(d.tool.preview().locked);
    d.click(-20.0, 5.0).escape();
    assert_eq!(
        d.placed(),
        Form::PointLineDistance {
            point: r("p0000001"),
            line: Reference::YAxis,
            value: mm("12"),
            side: LineSide::Right,
            driving: true
        }
    );
}

#[test]
fn only_the_origin_and_the_axes_constrain_nothing() {
    let mut d = Dim::new(empty());
    d.click(0.1, 0.1).click(40.0, 0.2).click(30.0, 30.0);
    assert_eq!(d.messages, vec![Message::FixedOnly]);
    d.escape()
        .escape()
        .click(40.0, 0.2)
        .click(0.2, 40.0)
        .click(30.0, 30.0);
    assert_eq!(d.messages, vec![Message::FixedOnly, Message::FixedOnly]);
    assert!(d.made.is_empty());
}

// --- SD1, SD5: angles by the pointer --------------------------------------------------

/// A (`n000000a`) from (10, 10) to (30, 10), facing +x, and B (`n000000b`) from (10, 10) to
/// (20, 20), facing 45°, each with its own points: they meet at (10, 10).
fn corner() -> Sketch {
    with_line(
        with_line(empty(), "n000000a", [10.0, 10.0], [30.0, 10.0]),
        "n000000b",
        [10.0, 10.0],
        [20.0, 20.0],
    )
}

#[test]
fn a_corner_from_one_points_angle_is_the_quadrant_either_pick_order() {
    // A from (10, 10) to (30, 10), B from (10, 10) to (20, 20): both leave the vertex, so
    // their along-rays run along them, A's 0°, B's 45°, θ = 45. The pointer at (25, 13),
    // 11.3°: between, `{A, B, 45}`. At (5, 20), 116.6°: after B, before −A (180°),
    // 180 − 45 = 135 from B's ray to −A: `{B, A, reversed, 135}`. At (5, 6), 218.7°:
    // opposite, the reflex 315 from B round to A: `{B, A, 315}`. At (20, 5), 333.4°: between
    // −B (225°) and A, 135, written as its vertical twin: `{B, A, reversed, 135}`. Picked B
    // first, the same pointer gives the same forms: the rays' order is the geometry's.
    for picks in [[[20.0, 10.0], [15.0, 15.0]], [[15.0, 15.0], [20.0, 10.0]]] {
        for (at, want) in [
            ([25.0, 13.0], angle("n000000a", "n000000b", false, "45")),
            ([5.0, 20.0], angle("n000000b", "n000000a", true, "135")),
            ([5.0, 6.0], angle("n000000b", "n000000a", false, "315")),
            ([20.0, 5.0], angle("n000000b", "n000000a", true, "135")),
        ] {
            let mut d = Dim::new(corner());
            d.click(picks[0][0], picks[0][1])
                .click(picks[1][0], picks[1][1])
                .click(at[0], at[1])
                .escape();
            assert_eq!(d.placed(), want, "{picks:?} {at:?}");
        }
    }
}

#[test]
fn lines_that_dont_share_a_point_take_the_ends_nearer_where_they_meet() {
    // FreeCAD's nearest-end rule. A (10, 10)-(30, 10) and B (40, 20)-(50, 30), on
    // y = x − 20, meet at (30, 10): A's `to` is there, so A's along-ray runs back, 180°; B's
    // `from` (40, 20) is nearer than its `to`, so B's runs along it, 45°. Counter-clockwise
    // B's then A's, θ = 135, A's against its line. The pointer at (20, 20), 135° round from
    // (30, 10): between, `{B, A, reversed, 135}`. At (45, 12), 7.6°: before B's ray, beside,
    // `{A, B, 45}`. At (28, 2), 256°: opposite, the reflex from A's ray round to B's,
    // `{A, B, reversed, 225}`.
    let s = with_line(
        with_line(empty(), "n000000a", [10.0, 10.0], [30.0, 10.0]),
        "n000000b",
        [40.0, 20.0],
        [50.0, 30.0],
    );
    for (at, want) in [
        ([20.0, 20.0], angle("n000000b", "n000000a", true, "135")),
        ([45.0, 12.0], angle("n000000a", "n000000b", false, "45")),
        ([28.0, 2.0], angle("n000000a", "n000000b", true, "225")),
    ] {
        let mut d = Dim::new(s.clone());
        d.click(20.0, 10.0)
            .click(45.0, 25.0)
            .click(at[0], at[1])
            .escape();
        assert_eq!(d.placed(), want, "{at:?}");
    }
}

#[test]
fn a_line_and_an_axis_make_an_angle_and_a_skipped_box_rounds_it_to_the_nanodegree() {
    // The line (10, 10)-(30, 20) faces atan(1/2) = 26.565051177077989...°, to the nanodegree
    // 26.565051177. With the x axis, picked second, it meets it at (-10, 0); the pointer at
    // (60, 5), 4.1° from there, is in x-to-line's sweep: `{x_axis, line}` 26.565051177.
    let mut d = Dim::new(with_line(empty(), "n0000001", [10.0, 10.0], [30.0, 20.0]));
    d.click(20.0, 15.0).click(80.0, 0.2).click(60.0, 5.0);
    assert_eq!(d.modify().as_deref(), Some("26.565051177"));
    d.escape();
    assert_eq!(
        d.placed(),
        Form::Angle {
            a: Reference::XAxis,
            b: r("n0000001"),
            reversed: false,
            value: deg("26.565051177"),
            driving: true
        }
    );
}

#[test]
fn a_right_click_locks_the_angle_the_preview_shows() {
    // Locked at (25, 13) (45, A to B), then placed at (5, 20), which unlocked gives
    // `{B, A, reversed, 135}`.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0)
        .click(15.0, 15.0)
        .right(25.0, 13.0)
        .to(5.0, 20.0);
    assert!(d.tool.preview().locked);
    assert_eq!(
        d.shown(),
        Some(Form::Angle {
            a: r("n000000a"),
            b: r("n000000b"),
            reversed: false,
            value: deg("45"),
            driving: true
        })
    );
    d.click(5.0, 20.0).escape();
    assert_eq!(
        d.placed(),
        Form::Angle {
            a: r("n000000a"),
            b: r("n000000b"),
            reversed: false,
            value: deg("45"),
            driving: true
        }
    );
}

#[test]
fn a_right_click_locks_a_horizontal_distance() {
    // Above the box, (25, 45): `distance_x`, locked; placed inside it, (25, 20), where
    // unlocked it's `distance`: still `distance_x` 30.
    let mut d = Dim::new(two_points());
    d.click(10.0, 10.0)
        .click(40.0, 30.0)
        .right(25.0, 45.0)
        .click(25.0, 20.0);
    d.escape();
    assert_eq!(
        d.placed(),
        Form::DistanceX {
            a: r("p0000001"),
            b: r("p0000002"),
            value: mm("30"),
            driving: true
        }
    );
    // Unlocked, the same placement is `distance`.
    d.click(10.0, 10.0)
        .click(40.0, 30.0)
        .click(25.0, 20.0)
        .escape();
    assert_eq!(d.placed().type_name(), "distance");
}

// --- Fable's chain-corner angles (§3): the quadrant among the four rays -----------------

/// A chain's corner: a from (-10, 10) to (10, 10), facing +x; b from (10, 10) on to (20, 20),
/// facing 45°, sharing the point (10, 10) (`t000000a`, a's `to`, b's `from`).
fn chain() -> Sketch {
    let mut s = empty();
    s.points.push(point("f000000a", -10.0, 10.0));
    s.points.push(point("t000000a", 10.0, 10.0));
    s.points.push(point("t000000b", 20.0, 20.0));
    s.lines.push(line("n000000a", "f000000a", "t000000a"));
    s.lines.push(line("n000000b", "t000000a", "t000000b"));
    s
}

fn angle(a: &str, b: &str, reversed: bool, value: &str) -> Form {
    Form::Angle {
        a: r(a),
        b: r(b),
        reversed,
        value: deg(value),
        driving: true,
    }
}

#[test]
fn a_chain_corners_angle_is_the_quadrant_the_pointer_is_in() {
    // Fable's §3. The vertex V is the shared (10, 10). a's along-ray leaves the end nearer
    // V, its `to`, so runs against a: 180°; b's leaves its `from`, along b: 45°. The four
    // rays: b's 45°, a's 180°, and their opposites 225° and 0°.
    // - Inside the corner, between the along-rays, (5, 12) at 158°: 135, counter-clockwise
    //   from b's ray to a's, a's running against a: `{b, a, reversed, 135}`.
    // - Adjacent, between a's direction 0° and b's 45°, (20, 12) at 11°: 180 − 135 = 45,
    //   both rays along their lines: `{a, b, 45}`.
    // - Opposite, between 225° and 360°, (12, 5) at 292°: the reflex, 360 − 135 = 225, from
    //   a's ray (180°) round to b's (45°): `{a, b, reversed, 225}`.
    for (at, want) in [
        ([5.0, 12.0], angle("n000000b", "n000000a", true, "135")),
        ([20.0, 12.0], angle("n000000a", "n000000b", false, "45")),
        ([12.0, 5.0], angle("n000000a", "n000000b", true, "225")),
    ] {
        let mut d = Dim::new(chain());
        d.click(0.0, 10.0)
            .click(15.0, 15.0)
            .click(at[0], at[1])
            .escape();
        assert_eq!(d.placed(), want, "{at:?}");
    }
}

#[test]
fn the_vertex_is_a_shared_point_exactly() {
    // Two lines ending at one point Q (-46.25, -6.635): a from P (-44.2, 0.744), b from
    // R (-43.014, -40.929). Their crossing, worked out, is (-46.25, -6.634999999999999), a
    // hair off Q; the vertex is Q itself. Both along-rays leave Q back along their lines: a's
    // toward P (74.5°), b's toward R (275.4°), so b's first, θ = 159.1°, both against (no
    // `reversed`). The pointer exactly on Q is no direction, between: `{b, a}`, 159.1. From
    // the worked-out crossing, it would be a hair straight down, 270°: beside, `{a, b,
    // reversed}`.
    let mut s = empty();
    s.points.push(point("f000000a", -44.2, 0.744));
    s.points.push(point("q0000001", -46.25, -6.635));
    s.points.push(point("f000000b", -43.014, -40.929));
    s.lines.push(line("n000000a", "f000000a", "q0000001"));
    s.lines.push(line("n000000b", "f000000b", "q0000001"));
    let mut d = Dim::new(s);
    d.click(-45.225, -2.9455)
        .click(-44.632, -23.782)
        .click(-46.25, -6.635)
        .escape();
    let Form::Angle {
        a,
        b,
        reversed,
        value,
        ..
    } = d.placed()
    else {
        panic!("{:?}", d.placed());
    };
    assert_eq!((a, b, reversed), (r("n000000b"), r("n000000a"), false));
    assert!(
        (159.0..160.0).contains(&value.degrees()),
        "{}",
        value.degrees()
    );
}

#[test]
fn the_pointer_on_the_vertex_is_between_the_along_rays() {
    // a (10, 10)-(2, 4) and b (10, 10)-(12, 2), each from its own point at (10, 10): both
    // leave it, a's ray at 216.87° (down and left), b's at 284.04°, θ = acos(32 / (10√68))
    // = 67.166°. The pointer exactly on the vertex is no direction: between, `{a, b}`. (The
    // angle of a zero vector from a's ray reads atan2(+0, −0) = 180° here, which would be
    // beside.)
    let s = with_line(
        with_line(empty(), "n000000a", [10.0, 10.0], [2.0, 4.0]),
        "n000000b",
        [10.0, 10.0],
        [12.0, 2.0],
    );
    let mut d = Dim::new(s);
    d.click(6.0, 7.0)
        .click(11.0, 6.0)
        .click(10.0, 10.0)
        .escape();
    let Form::Angle {
        a,
        b,
        reversed,
        value,
        ..
    } = d.placed()
    else {
        panic!("{:?}", d.placed());
    };
    assert_eq!((a, b, reversed), (r("n000000a"), r("n000000b"), false));
    assert!(
        (67.16..67.17).contains(&value.degrees()),
        "{}",
        value.degrees()
    );
}

// --- SD4: two parallel lines ----------------------------------------------------------

#[test]
fn two_parallel_lines_are_the_second_lines_from_to_the_first_line() {
    // A (10, 10)-(50, 10) and B (20, 30)-(60, 30), both facing +x. B picked second: its
    // `from`, (20, 30), is 20 to A's left. A picked second: A's `from`, (10, 10), is 20 to
    // B's right. With the x axis picked second, which has no `from`, the line's `from` (10,
    // 10) to the axis: 10 to its left.
    let s = with_line(
        with_line(empty(), "n000000a", [10.0, 10.0], [50.0, 10.0]),
        "n000000b",
        [20.0, 30.0],
        [60.0, 30.0],
    );
    let mut d = Dim::new(s);
    d.click(30.0, 10.0)
        .click(40.0, 30.0)
        .click(70.0, 20.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::PointLineDistance {
            point: r("f000000b"),
            line: r("n000000a"),
            value: mm("20"),
            side: LineSide::Left,
            driving: true
        }
    );
    d.click(40.0, 30.0)
        .click(30.0, 10.0)
        .click(70.0, 20.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::PointLineDistance {
            point: r("f000000a"),
            line: r("n000000b"),
            value: mm("20"),
            side: LineSide::Right,
            driving: true
        }
    );
    d.click(30.0, 10.0)
        .click(80.0, 0.2)
        .click(70.0, 5.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::PointLineDistance {
            point: r("f000000a"),
            line: Reference::XAxis,
            value: mm("10"),
            side: LineSide::Left,
            driving: true
        }
    );
}

#[test]
fn lines_are_parallel_by_freecads_test_to_its_edge() {
    // A (10, 10)-(50, 10): direction (40, 0). B from (20, 30) to (60, 30 + e): direction
    // (40, e), and A × B = 40e. FreeCAD's test is 40e < 1e-9: e = 2e-11 gives 8e-10,
    // parallel, the distance SD4 writes, B's `from` 20 to A's left. e = 3e-11 gives 1.2e-9,
    // not parallel: an angle. A to B sweeps e/40 = 7.5e-13 rad; they meet where
    // 30 + e(x − 20)/40 = 10, x = 20 − 800/e ≈ −2.67e13, each line's `from` the nearer, so
    // both along-rays run along. The pointer at (70, 31) is 21/2.67e13 ≈ 7.9e-13 rad round
    // from A, past B's ray: beside, `{B, A, reversed}`, 180 less 4.3e-11°, 180 to the
    // nanodegree.
    let s = |e: f64| {
        with_line(
            with_line(empty(), "n000000a", [10.0, 10.0], [50.0, 10.0]),
            "n000000b",
            [20.0, 30.0],
            [60.0, 30.0 + e],
        )
    };
    let mut d = Dim::new(s(2e-11));
    d.click(30.0, 10.0)
        .click(40.0, 30.0)
        .click(70.0, 31.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::PointLineDistance {
            point: r("f000000b"),
            line: r("n000000a"),
            value: mm("20"),
            side: LineSide::Left,
            driving: true
        }
    );
    let mut d = Dim::new(s(3e-11));
    d.click(30.0, 10.0)
        .click(40.0, 30.0)
        .click(70.0, 31.0)
        .escape();
    assert_eq!(
        d.placed(),
        Form::Angle {
            a: r("n000000b"),
            b: r("n000000a"),
            reversed: true,
            value: deg("180"),
            driving: true
        }
    );
}

#[test]
fn two_lines_in_line_have_no_distance() {
    // A (10, 10)-(30, 10) and B (40, 10)-(60, 10): B's `from` is on A's line.
    let s = with_line(
        with_line(empty(), "n000000a", [10.0, 10.0], [30.0, 10.0]),
        "n000000b",
        [40.0, 10.0],
        [60.0, 10.0],
    );
    let mut d = Dim::new(s);
    d.click(20.0, 10.0).click(50.0, 10.0).click(40.0, 30.0);
    assert_eq!(d.messages, vec![Message::NoDistance]);
    assert!(d.made.is_empty());
}

// --- Esc, picks, hover ----------------------------------------------------------------

#[test]
fn escape_undoes_the_last_pick_then_leaves_the_tool() {
    // A and B picked; Esc drops B, so a click on nothing places A's length, 20. Esc with
    // nothing picked leaves the tool.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(15.0, 15.0);
    assert_eq!(d.tool.preview().picks, vec![r("n000000a"), r("n000000b")]);
    d.escape();
    assert_eq!(d.tool.preview().picks, vec![r("n000000a")]);
    assert!(!d.left);
    d.click(20.0, 40.0).escape();
    assert_eq!(
        d.placed(),
        Form::Length {
            line: r("n000000a"),
            value: mm("20"),
            driving: true
        }
    );
    d.click(20.0, 10.0).escape();
    assert!(!d.left);
    d.escape();
    assert!(d.left);
}

#[test]
fn a_new_pick_unlocks_the_kind() {
    // A's length locked with A alone; B picked then: the pair's kind is the pointer's again,
    // and the preview says so.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).right(20.0, 40.0);
    assert!(d.tool.preview().locked);
    d.click(15.0, 15.0);
    assert!(!d.tool.preview().locked);
}

#[test]
fn smart_dimensions_messages_say_what_to_do() {
    for (message, text) in [
        (
            Message::NoDistance,
            "they're at no distance from each other: there's no distance to dimension",
        ),
        (
            Message::AcrossCentreline,
            "a diameter across a centreline isn't offered: place the dimension on the point's side for its distance",
        ),
        (
            Message::FixedOnly,
            "the origin and the axes are fixed: dimension a member of the sketch",
        ),
        (
            Message::NotANumber,
            "type a plain number, as a document writes one: no units, arithmetic, or expressions",
        ),
        (
            Message::ValueRefused(Error::new(ErrorCode::InvalidInput, "1e400 is too large")),
            "1e400 is too large",
        ),
        (
            Message::DimensionChanged,
            "the sketch changed what the dimension measures: place it again",
        ),
        (
            Message::Solving,
            "the last dimension is still solving: place this one once it has",
        ),
        (
            Message::OfferOpen,
            "the last dimension over-defines the sketch: make it driven, leave it, or cancel it before placing another",
        ),
    ] {
        assert_eq!(message.text(), text);
    }
}

#[test]
fn escape_mid_click_drops_the_press_with_the_pick() {
    // A and B picked; pressed on nothing at (20, 40), Esc, released there: the release is no
    // click, so A's length isn't placed.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(15.0, 15.0);
    d.send(DimensionEvent::Press([20.0, 40.0]));
    d.escape();
    d.send(DimensionEvent::Release([20.0, 40.0]));
    assert_eq!(d.modify(), None);
    assert_eq!(d.tool.preview().picks, vec![r("n000000a")]);
}

#[test]
fn escape_undoes_a_lock_with_its_pick() {
    let mut d = Dim::new(two_points());
    d.click(10.0, 10.0).click(40.0, 30.0).right(25.0, 45.0);
    assert!(d.tool.preview().locked);
    d.escape();
    assert!(!d.tool.preview().locked);
    d.click(40.0, 30.0).click(25.0, 20.0).escape();
    assert_eq!(d.placed().type_name(), "distance");
}

#[test]
fn the_preview_shows_what_a_click_would_pick_and_points_before_curves() {
    // Over A's end (10, 10), which B shares the place of: the point the nearest, ties by
    // ID, the line's `from` point A's own; over A's middle, the line.
    let mut d = Dim::new(corner());
    d.to(10.1, 10.0);
    assert_eq!(d.tool.preview().hover, Some(r("f000000a")));
    d.to(20.0, 10.2);
    assert_eq!(d.tool.preview().hover, Some(r("n000000a")));
    d.to(20.0, 12.0);
    assert_eq!(d.tool.preview().hover, None);
}

// --- SD9: the Modify box --------------------------------------------------------------

/// A 50 mm line's length in the Modify box.
fn boxed() -> Dim {
    let mut d = Dim::new(with_line(empty(), "n0000001", [10.0, 10.0], [40.0, 50.0]));
    d.click(25.0, 30.0).click(60.0, 5.0);
    assert_eq!(d.modify().as_deref(), Some("50"));
    d
}

#[test]
fn the_box_takes_no_units_arithmetic_or_words_and_stays_open() {
    for text in [
        "", "abc", "10mm", "10 mm", "10/3", "3+4", "=10", "NaN", "nan", "Infinity", "-inf", "+5",
        ".5", "5.", "05", "\"10\"", "true", "null", "[10]", "10,5", "1_000", "0x10", "1e", "10 10",
    ] {
        let mut d = boxed();
        d.typed(text).enter();
        assert_eq!(d.messages, vec![Message::NotANumber], "{text:?}");
        assert_eq!(d.modify().as_deref(), Some(text), "{text:?}");
        assert!(d.made.is_empty(), "{text:?}");
        // Still open: a number typed then is placed.
        d.typed("12").enter();
        assert_eq!(
            d.placed(),
            Form::Length {
                line: r("n0000001"),
                value: mm("12"),
                driving: true
            },
            "{text:?}"
        );
    }
}

#[test]
fn the_box_reads_a_number_as_the_document_does() {
    // JSON's numbers, with whitespace about them, and exponents read exactly.
    for (text, want) in [
        ("12.5", "12.5"),
        (" 7 ", "7"),
        ("1e1", "10"),
        ("1.05E1", "10.5"),
        ("2.5e-1", "0.25"),
        ("0.000001", "0.000001"),
        ("1000000", "1000000"),
    ] {
        let mut d = boxed();
        d.typed(text).enter();
        assert_eq!(d.messages, vec![], "{text:?}");
        assert_eq!(
            d.placed(),
            Form::Length {
                line: r("n0000001"),
                value: mm(want),
                driving: true
            },
            "{text:?}"
        );
    }
}

#[test]
fn the_box_refuses_what_the_reader_or_the_gate_refuses_in_their_words() {
    // Past a nanometre, past what a length holds, past a kilometre (the gate's driving
    // length), zero and negative for a length (above 0), and zero written with a minus.
    for (text, says) in [
        ("0.0000001", "0.0000001 has more than 6 decimals"),
        ("1e400", "1e400 is too large"),
        ("1e13", "1e13 is too large"),
        ("-0", "-0 is zero written with a minus sign"),
        (
            "1000000.000001",
            "but it must be above 0 and at most 1000000 mm",
        ),
        ("1e7", "but it must be above 0 and at most 1000000 mm"),
        ("0", "but it must be above 0 and at most 1000000 mm"),
        ("-5", "but it must be above 0 and at most 1000000 mm"),
    ] {
        let mut d = boxed();
        d.typed(text).enter();
        assert_eq!(d.messages.len(), 1, "{text:?}");
        let Message::ValueRefused(error) = &d.messages[0] else {
            panic!("{text:?}: {:?}", d.messages);
        };
        assert!(error.message().contains(says), "{text:?}: {error:?}");
        assert_eq!(d.modify().as_deref(), Some(text));
        assert!(d.made.is_empty());
    }
}

#[test]
fn an_angles_box_reads_degrees_to_the_nanodegree_signed_within_a_turn() {
    // The corner's 45°: -30 and 360 taken (signed, at most a turn), 360.000000001 and 400
    // refused by the gate, 1e-10 past a nanodegree by the reader.
    for (text, ok) in [
        ("-30", true),
        ("360", true),
        ("30.000000001", true),
        ("360.000000001", false),
        ("400", false),
        ("0.0000000001", false),
    ] {
        let mut d = Dim::new(corner());
        d.click(20.0, 10.0).click(15.0, 15.0).click(25.0, 13.0);
        assert_eq!(d.modify().as_deref(), Some("45"));
        d.typed(text).enter();
        if ok {
            assert_eq!(
                d.placed(),
                Form::Angle {
                    a: r("n000000a"),
                    b: r("n000000b"),
                    reversed: false,
                    value: deg(text),
                    driving: true
                },
                "{text}"
            );
        } else {
            assert!(d.made.is_empty(), "{text}");
            assert!(
                matches!(d.messages[..], [Message::ValueRefused(_)]),
                "{text}: {:?}",
                d.messages
            );
        }
    }
}

#[test]
fn a_diameter_may_be_two_kilometres() {
    // The gate's driving diameter: at most twice a kilometre.
    let mut d = Dim::new(with_circle(empty(), "k0000001", [0.0, 30.0], 7.0));
    d.click(7.0, 30.0)
        .click(20.0, 45.0)
        .typed("2000000")
        .enter();
    assert_eq!(
        d.placed(),
        Form::Diameter {
            curve: r("k0000001"),
            value: mm("2000000"),
            driving: true
        }
    );
    d.click(7.0, 30.0)
        .click(20.0, 45.0)
        .typed("2000000.000001")
        .enter();
    assert_eq!(d.made.len(), 1);
    assert!(matches!(d.messages[..], [Message::ValueRefused(_)]));
}

// --- SD2: a skipped box ---------------------------------------------------------------

#[test]
fn a_skipped_box_writes_the_measured_value_rounded_to_the_nanometre() {
    // (10, 10) to (11, 11): √2 = 1.41421356237..., to the nanometre 1.414214. Enter on the
    // untouched box writes the same.
    let s = with_line(empty(), "n0000001", [10.0, 10.0], [11.0, 11.0]);
    let want = Form::Length {
        line: r("n0000001"),
        value: Length::from_nm(1_414_214),
        driving: true,
    };
    let mut d = Dim::new(s.clone());
    d.click(10.5, 10.5).click(20.0, 5.0);
    assert_eq!(d.modify().as_deref(), Some("1.414214"));
    d.escape();
    assert_eq!(d.placed(), want);
    let mut d = Dim::new(s);
    d.click(10.5, 10.5).click(20.0, 5.0).enter();
    assert_eq!(d.placed(), want);
}

#[test]
fn a_skipped_box_whose_measure_the_gate_refuses_places_nothing_and_keeps_the_picks() {
    // Points 1.8 km apart: a driving distance is at most a kilometre. Skipped, nothing is
    // placed, the box closes, the picks stay; placed again, 500000 typed is taken.
    let s = with_point(
        with_point(empty(), "p0000001", [-900_000.0, 0.0]),
        "p0000002",
        [900_000.0, 0.0],
    );
    let mut d = Dim::new(s);
    d.click(-900_000.0, 0.0)
        .click(900_000.0, 0.0)
        .click(0.0, 50.0);
    assert_eq!(d.modify().as_deref(), Some("1800000"));
    d.escape();
    assert!(d.made.is_empty());
    assert_eq!(d.modify(), None);
    assert!(matches!(d.messages[..], [Message::ValueRefused(_)]));
    assert_eq!(d.tool.preview().picks.len(), 2);
    d.click(0.0, 50.0).typed("500000").enter();
    assert_eq!(
        d.placed(),
        Form::DistanceX {
            a: r("p0000001"),
            b: r("p0000002"),
            value: mm("500000"),
            driving: true
        }
    );
}

// --- The sketch under the tool ---------------------------------------------------------

#[test]
fn a_pick_the_read_no_longer_has_is_dropped_and_the_box_closed() {
    let mut d = boxed();
    d.sketch = with_line(empty(), "n0000002", [10.0, 10.0], [40.0, 50.0]);
    d.typed("12");
    assert_eq!(d.modify(), None);
    assert!(d.tool.preview().picks.is_empty());
    d.enter();
    assert!(d.made.is_empty());
}

/// IDs `t0000001` unless it's taken, then `t0000002`, and so on: a source that gives the
/// same ID again unless told it's taken.
struct Lowest;

impl NewIds for Lowest {
    fn next(&mut self, taken: &dyn Fn(&ElementId) -> bool) -> Result<ElementId, Error> {
        (1..)
            .map(|n| id(&format!("t{n:07}")))
            .find(|id| !taken(id))
            .ok_or_else(|| Error::new(ErrorCode::Internal, "none"))
    }
}

#[test]
fn a_dimension_placed_before_a_read_shows_it_keeps_its_id_taken() {
    // Two lengths placed against the same read, the first unanswered: the second's ID isn't
    // the first's. Without the add kept, `Lowest` gives t0000001 twice.
    let sketch = with_line(
        with_line(empty(), "n000000a", [10.0, 10.0], [30.0, 10.0]),
        "n000000b",
        [10.0, 40.0],
        [30.0, 40.0],
    );
    let sketch_id = sketch_id();
    let context = Context {
        sketch_id: &sketch_id,
        revision: Revision::OPENED,
        sketch: &sketch,
        tolerance: TOLERANCE,
        automatic_relations: true,
        construction: false,
    };
    let mut tool = SmartDimension::new();
    let mut ids = Lowest;
    let mut made = Vec::new();
    for (pick, place) in [([20.0, 10.0], [20.0, 20.0]), ([20.0, 40.0], [20.0, 50.0])] {
        for at in [pick, place] {
            tool.handle(&context, &under(), &mut ids, DimensionEvent::Press(at));
            tool.handle(&context, &under(), &mut ids, DimensionEvent::Release(at));
        }
        let out = tool.handle(&context, &under(), &mut ids, DimensionEvent::Escape);
        made.push(out.add.unwrap().constraints[0].id.clone());
        // No read of its revision comes: its offer dismissed, so the next is placed.
        tool.dismiss_offer();
    }
    assert_eq!(made, vec![id("t0000001"), id("t0000002")]);
}

#[test]
fn another_sketch_starts_the_tool_afresh() {
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0);
    let other = FeatureId::new("s0000002").unwrap();
    let context = Context {
        sketch_id: &other,
        revision: d.revision,
        sketch: &d.sketch,
        tolerance: TOLERANCE,
        automatic_relations: true,
        construction: false,
    };
    d.tool.handle(
        &context,
        &under(),
        &mut d.ids,
        DimensionEvent::Move([50.0, 50.0]),
    );
    assert!(d.tool.preview().picks.is_empty());
    assert!(!d.tool.busy());
}

#[test]
fn a_position_that_isnt_a_number_is_refused_and_changes_nothing() {
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0);
    for event in [
        DimensionEvent::Press([f64::NAN, 0.0]),
        DimensionEvent::Release([0.0, f64::INFINITY]),
        DimensionEvent::RightClick([f64::NAN, f64::NAN]),
        DimensionEvent::Move([f64::NEG_INFINITY, 0.0]),
    ] {
        let out = d.send(event.clone());
        assert_eq!(out.message, Some(Message::NotFinite), "{event:?}");
    }
    assert_eq!(d.tool.preview().picks, vec![r("n000000a")]);
}

#[test]
fn a_tolerance_that_isnt_a_positive_number_picks_only_what_is_exactly_there() {
    for tolerance in [f64::NAN, -1.0, 0.0] {
        let mut d = Dim::new(corner());
        d.tolerance = tolerance;
        d.click(20.0, 10.2);
        assert!(d.tool.preview().picks.is_empty(), "{tolerance}");
        d.click(20.0, 10.0);
        assert_eq!(d.tool.preview().picks, vec![r("n000000a")], "{tolerance}");
    }
}

#[test]
fn a_problem_the_gate_finds_elsewhere_is_the_cores_to_answer() {
    // The read holds a driving `length` of 0 on B (which no read would, but a merged one
    // might): the gate refuses that at B's constraint, not the new one, so A's length is
    // added and the core answers it.
    let mut sketch = corner();
    sketch.constraints.push(SketchConstraint {
        id: id("k0000001"),
        form: Form::Length {
            line: r("n000000b"),
            value: Length::from_nm(0),
            driving: true,
        },
    });
    let sketch_id = sketch_id();
    let context = Context {
        sketch_id: &sketch_id,
        revision: Revision::OPENED,
        sketch: &sketch,
        tolerance: TOLERANCE,
        automatic_relations: true,
        construction: false,
    };
    let mut tool = SmartDimension::new();
    let mut ids = Counter(0);
    for at in [[20.0, 10.0], [20.0, 30.0]] {
        tool.handle(&context, &under(), &mut ids, DimensionEvent::Press(at));
        tool.handle(&context, &under(), &mut ids, DimensionEvent::Release(at));
    }
    let out = tool.handle(&context, &under(), &mut ids, DimensionEvent::Escape);
    assert_eq!(out.message, None);
    assert_eq!(
        out.add.unwrap().constraints[0].form,
        Form::Length {
            line: r("n000000a"),
            value: mm("20"),
            driving: true
        }
    );
    // Its own value refused is the tool's to say: the box stays open. (The first's offer
    // dismissed, as no read of its revision comes.)
    tool.dismiss_offer();
    for at in [[20.0, 10.0], [20.0, 30.0]] {
        tool.handle(&context, &under(), &mut ids, DimensionEvent::Press(at));
        tool.handle(&context, &under(), &mut ids, DimensionEvent::Release(at));
    }
    tool.handle(
        &context,
        &under(),
        &mut ids,
        DimensionEvent::Typed("0".to_owned()),
    );
    let out = tool.handle(&context, &under(), &mut ids, DimensionEvent::Enter);
    assert_eq!(out.add, None);
    let Some(Message::ValueRefused(error)) = out.message else {
        panic!("{out:?}");
    };
    assert_eq!(error.parameter(), Some("constraints.t0000002.value"));
}

// --- Review #345: the box, the clicks, busy -------------------------------------------

/// p (30, 7) above the line from (10, 0) to (50, 0), facing +x.
fn point_over_line() -> Sketch {
    with_point(
        with_line(empty(), "n0000001", [10.0, 0.0], [50.0, 0.0]),
        "p0000001",
        [30.0, 7.0],
    )
}

#[test]
fn enter_on_an_untouched_box_places_the_measured_distance_from_a_line() {
    // B1: the box holds the value alone, "7", never the form's "7 left". The same for SD4's
    // lines (B's `from` 20 to A's left), a circle's centre and a line (20), and a centreline
    // (12 to its right).
    let mut d = Dim::new(point_over_line());
    d.click(30.0, 7.0).click(20.0, 0.0).click(40.0, 20.0);
    assert_eq!(d.modify().as_deref(), Some("7"));
    d.enter();
    assert_eq!(
        d.placed(),
        Form::PointLineDistance {
            point: r("p0000001"),
            line: r("n0000001"),
            value: mm("7"),
            side: LineSide::Left,
            driving: true
        }
    );
    let parallel = with_line(
        with_line(empty(), "n000000a", [10.0, 10.0], [50.0, 10.0]),
        "n000000b",
        [20.0, 30.0],
        [60.0, 30.0],
    );
    let mut d = Dim::new(parallel);
    d.click(30.0, 10.0).click(40.0, 30.0).click(70.0, 20.0);
    assert_eq!(d.modify().as_deref(), Some("20"));
    d.enter();
    assert_eq!(d.placed().type_name(), "point_line_distance");
    let circle_line = with_circle(
        with_line(empty(), "n0000001", [10.0, 0.0], [50.0, 0.0]),
        "k0000001",
        [30.0, 20.0],
        5.0,
    );
    let mut d = Dim::new(circle_line);
    d.click(35.0, 20.0).click(20.0, 0.0).click(40.0, 30.0);
    assert_eq!(d.modify().as_deref(), Some("20"));
    d.enter();
    assert_eq!(d.placed().type_name(), "point_line_distance");
    let mut d = Dim::new(centreline(12.0));
    d.click(12.0, 5.0).click(0.0, 20.0).click(20.0, 5.0);
    assert_eq!(d.modify().as_deref(), Some("12"));
    d.enter();
    assert_eq!(d.placed().type_name(), "point_line_distance");
}

/// A construction line from (0, -10) to (0, 40), facing +y, and p at (x, 5).
fn centreline(x: f64) -> Sketch {
    construction(
        with_point(
            with_line(empty(), "n0000001", [0.0, -10.0], [0.0, 40.0]),
            "p0000001",
            [x, 5.0],
        ),
        "n0000001",
    )
}

#[test]
fn an_untouched_box_follows_a_newer_read() {
    // R1: the line's end moved from (40, 50) to (70, 90) while the box is open: 60 across,
    // 80 up, 100 long. The untouched box now reads "100", and Enter places 100.
    let mut d = boxed();
    d.sketch.points[1].x = Coordinate::new(70.0).unwrap();
    d.sketch.points[1].y = Coordinate::new(90.0).unwrap();
    d.to(60.0, 5.0);
    assert_eq!(d.modify().as_deref(), Some("100"));
    d.enter();
    assert_eq!(
        d.placed(),
        Form::Length {
            line: r("n0000001"),
            value: mm("100"),
            driving: true
        }
    );
}

#[test]
fn a_typed_box_keeps_its_text_on_the_newest_side_and_order() {
    // R1: 5 typed for p's distance from the line, then p moved to (30, -7), below it: the
    // box keeps "5", and Enter places 5 on the line's right, the newest read's side.
    let mut d = Dim::new(point_over_line());
    d.click(30.0, 7.0)
        .click(20.0, 0.0)
        .click(40.0, 20.0)
        .typed("5");
    d.sketch.points[2].y = Coordinate::new(-7.0).unwrap();
    d.to(40.0, 20.0);
    assert_eq!(d.modify().as_deref(), Some("5"));
    d.enter();
    assert_eq!(
        d.placed(),
        Form::PointLineDistance {
            point: r("p0000001"),
            line: r("n0000001"),
            value: mm("5"),
            side: LineSide::Right,
            driving: true
        }
    );
    // 20 typed for p (10, 10) and q (40, 30) horizontally, then q moved to (-20, 30), left
    // of p: the box keeps "20", and the order is q to p, so the value starts positive.
    let mut d = Dim::new(two_points());
    d.click(10.0, 10.0)
        .click(40.0, 30.0)
        .click(25.0, 45.0)
        .typed("20");
    d.sketch.points[1].x = Coordinate::new(-20.0).unwrap();
    d.enter();
    assert_eq!(
        d.placed(),
        Form::DistanceX {
            a: r("p0000002"),
            b: r("p0000001"),
            value: mm("20"),
            driving: true
        }
    );
}

#[test]
fn a_box_the_newer_read_leaves_no_dimension_closes_saying_why() {
    // R1: p moved onto the line, (30, 0): no distance. And p, measured from a centreline on
    // its side with the box placed at (20, 5), moved across it to (-12, 5): the box at
    // (20, 5) is then across, a diameter.
    let mut d = Dim::new(point_over_line());
    d.click(30.0, 7.0).click(20.0, 0.0).click(40.0, 20.0);
    d.sketch.points[2].y = Coordinate::new(0.0).unwrap();
    d.to(40.0, 20.0);
    assert_eq!(d.modify(), None);
    assert_eq!(d.messages, vec![Message::NoDistance]);
    d.enter();
    assert!(d.made.is_empty());
    let mut d = Dim::new(centreline(12.0));
    d.click(12.0, 5.0).click(0.0, 20.0).click(20.0, 5.0);
    assert_eq!(d.modify().as_deref(), Some("12"));
    d.sketch.points[2].x = Coordinate::new(-12.0).unwrap();
    d.typed("12");
    assert_eq!(d.modify(), None);
    assert_eq!(d.messages, vec![Message::AcrossCentreline]);
}

#[test]
fn a_long_refused_number_is_cut_to_twenty_characters() {
    // 0.00000000000000000000000001 (28 characters) has more than 6 decimals: said as its
    // first 20, "0." and 18 zeros, then "…".
    let mut d = boxed();
    d.typed("0.00000000000000000000000001").enter();
    let Message::ValueRefused(error) = &d.messages[0] else {
        panic!("{:?}", d.messages);
    };
    assert_eq!(
        error.message(),
        "0.000000000000000000… has more than 6 decimals"
    );
}

#[test]
fn a_placing_click_may_land_on_something() {
    // T1: p and q picked, then a click on r (25, 45), above their box: it places, the
    // horizontal distance 30. The line picked, then clicked again: its length, 50.
    let mut d = Dim::new(with_point(two_points(), "p0000003", [25.0, 45.0]));
    d.click(10.0, 10.0).click(40.0, 30.0).click(25.0, 45.0);
    assert_eq!(
        d.shown(),
        Some(Form::DistanceX {
            a: r("p0000001"),
            b: r("p0000002"),
            value: mm("30"),
            driving: true
        })
    );
    assert_eq!(d.modify().as_deref(), Some("30"));
    let mut d = Dim::new(with_line(empty(), "n0000001", [10.0, 10.0], [40.0, 50.0]));
    d.click(25.0, 30.0).click(25.0, 30.0);
    assert_eq!(d.modify().as_deref(), Some("50"));
    d.enter();
    assert_eq!(
        d.placed(),
        Form::Length {
            line: r("n0000001"),
            value: mm("50"),
            driving: true
        }
    );
}

#[test]
fn clicks_while_the_box_is_open_change_nothing() {
    // T2.
    let mut d = boxed();
    d.typed("12").click(60.0, 60.0).right(60.0, 60.0);
    assert_eq!(d.modify().as_deref(), Some("12"));
    d.enter();
    assert_eq!(
        d.placed(),
        Form::Length {
            line: r("n0000001"),
            value: mm("12"),
            driving: true
        }
    );
}

#[test]
fn busy_while_a_pick_a_press_or_the_box_is_held() {
    // T3.
    let mut d = Dim::new(corner());
    assert!(!d.tool.busy());
    d.click(20.0, 10.0);
    assert!(d.tool.busy());
    d.escape();
    assert!(!d.tool.busy());
    d.send(DimensionEvent::Press([50.0, 50.0]));
    assert!(d.tool.busy());
    d.send(DimensionEvent::Release([50.0, 50.0]));
    assert!(!d.tool.busy());
    d.click(20.0, 10.0).click(20.0, 40.0).escape();
    assert!(d.tool.busy(), "the box open");
    d.escape();
    assert!(d.left);
    assert!(!d.tool.busy());
}

#[test]
fn an_angle_is_about_where_the_lines_meet_in_x() {
    // T4, adjusted to §3: A (0, 0)-(10, 0) and B (110, 10)-(120, 20), on y = x − 100, meet
    // at (100, 0). A's `to` (10, 0) is the nearer end, so A's along-ray runs back, 180°; B's
    // `from`, along it, 45°: θ = 135, A's against. The pointer at (99, 0.5), 153.4° round
    // from (100, 0): between, `{B, A, reversed, 135}` (SD5 had the reflex, `{B, A, 315}`); at
    // (101, 0.5), 26.6°: beside, `{A, B, 45}`, as before.
    let s = with_line(
        with_line(empty(), "n000000a", [0.0, 0.0], [10.0, 0.0]),
        "n000000b",
        [110.0, 10.0],
        [120.0, 20.0],
    );
    for (at, want) in [
        ([99.0, 0.5], angle("n000000b", "n000000a", true, "135")),
        ([101.0, 0.5], angle("n000000a", "n000000b", false, "45")),
    ] {
        let mut d = Dim::new(s.clone());
        d.click(5.0, 0.0)
            .click(115.0, 15.0)
            .click(at[0], at[1])
            .escape();
        assert_eq!(d.placed(), want, "{at:?}");
    }
}

#[test]
fn crossing_lines_take_their_ends_by_freecads_tie() {
    // A (10, 20)-(30, 20) and B (10, 10)-(30, 30) cross at (20, 20), each at its middle:
    // each end as near, so FreeCAD's rule (its start only where strictly nearer) takes each
    // `to`, and both along-rays run back: A's 180°, B's 225°, θ = 45, both against (no
    // `reversed`). The pointer at (12, 18), 194°: between, `{A, B, 45}`. At (28, 22), 14°:
    // opposite, which crossing lines draw as θ too (Fable's §4), `{A, B, 45}`. At (12, 25),
    // 148°, and (28, 15), 328°: beside, `{B, A, reversed, 135}`.
    let s = with_line(
        with_line(empty(), "n000000a", [10.0, 20.0], [30.0, 20.0]),
        "n000000b",
        [10.0, 10.0],
        [30.0, 30.0],
    );
    for (at, want) in [
        ([12.0, 18.0], angle("n000000a", "n000000b", false, "45")),
        ([28.0, 22.0], angle("n000000a", "n000000b", false, "45")),
        ([12.0, 25.0], angle("n000000b", "n000000a", true, "135")),
        ([28.0, 15.0], angle("n000000b", "n000000a", true, "135")),
    ] {
        // Either pick order: the crossing test holds whichever sign `crossed` takes.
        for picks in [[[14.0, 20.0], [26.0, 26.0]], [[26.0, 26.0], [14.0, 20.0]]] {
            let mut d = Dim::new(s.clone());
            d.click(picks[0][0], picks[0][1])
                .click(picks[1][0], picks[1][1])
                .click(at[0], at[1])
                .escape();
            assert_eq!(d.placed(), want, "{picks:?} {at:?}");
        }
    }
}

#[test]
fn crossing_lines_read_their_angle_or_its_supplement_in_every_sector() {
    // Fable's §4, with no tie: A (10, 20)-(31, 20) and B (10, 10)-(31, 31) cross at
    // (20, 20), each `from` the nearer end (10 against 11, √200 against √242), so both
    // along-rays run along: A's 0°, B's 45°, θ = 45. Every sector reads θ or 180 − θ, never
    // a reflex: (28, 22) at 14°, between, `{A, B, 45}`; (12, 25) at 148°, beside,
    // `{B, A, reversed, 135}`; (12, 18) at 194°, opposite, θ again, `{A, B, 45}` (the
    // quadrant rule alone would give the reflex `{B, A, 315}`); (28, 15) at 328°, beside,
    // `{B, A, reversed, 135}`.
    let s = with_line(
        with_line(empty(), "n000000a", [10.0, 20.0], [31.0, 20.0]),
        "n000000b",
        [10.0, 10.0],
        [31.0, 31.0],
    );
    for (at, want) in [
        ([28.0, 22.0], angle("n000000a", "n000000b", false, "45")),
        ([12.0, 25.0], angle("n000000b", "n000000a", true, "135")),
        ([12.0, 18.0], angle("n000000a", "n000000b", false, "45")),
        ([28.0, 15.0], angle("n000000b", "n000000a", true, "135")),
    ] {
        // Either pick order: the crossing test holds whichever sign `crossed` takes.
        for picks in [[[14.0, 20.0], [26.0, 26.0]], [[26.0, 26.0], [14.0, 20.0]]] {
            let mut d = Dim::new(s.clone());
            d.click(picks[0][0], picks[0][1])
                .click(picks[1][0], picks[1][1])
                .click(at[0], at[1])
                .escape();
            assert_eq!(d.placed(), want, "{picks:?} {at:?}");
        }
    }
}

#[test]
fn a_crossing_a_nanometre_inside_an_end_counts_and_one_outside_doesnt() {
    // Main's exact test. A (10, 20)-(30, 20), and B upright at x = 30 ∓ 1 nm, from y 10 to
    // 31, meeting A at (x, 20). A's `to` is the nearer end, so A's along-ray runs back,
    // 180°; B's `from`, 90°. B's ray first, θ = 90, A's against. The pointer at (35, 15), 315°
    // round, is opposite. 1 nm inside A's end, the lines cross: θ, `{B, A, reversed, 90}`.
    // 1 nm outside it, they don't: the reflex from A's ray round to B's,
    // `{A, B, reversed, 270}`.
    for (x, want) in [
        (29.999999, angle("n000000b", "n000000a", true, "90")),
        (30.000001, angle("n000000a", "n000000b", true, "270")),
        // One ulp either side of 30: 21(x − 10) is 419.99999999999994 below 420, inside,
        // and 420.00000000000006 above it, outside. No tolerance, however small.
        (
            29.999999999999996,
            angle("n000000b", "n000000a", true, "90"),
        ),
        (
            30.000000000000004,
            angle("n000000a", "n000000b", true, "270"),
        ),
    ] {
        let s = with_line(
            with_line(empty(), "n000000a", [10.0, 20.0], [30.0, 20.0]),
            "n000000b",
            [x, 10.0],
            [x, 31.0],
        );
        let mut d = Dim::new(s);
        d.click(20.0, 20.0)
            .click(x, 25.0)
            .click(35.0, 15.0)
            .escape();
        assert_eq!(d.placed(), want, "{x}");
    }
}

#[test]
fn lines_that_only_meet_keep_the_quadrant_rule_opposite() {
    // Strictly inside both is exact: where the vertex is on one line's end, behind its start,
    // or on an axis, they don't cross, and the sector opposite the along-rays reads the
    // reflex. A is (10, 10)-(30, 10) in the first three.
    // - B from (30, 20) to A's middle (20, 10): B's `to` there, its ray back, 45°; A's tie,
    //   its `to`, 180°. θ = 135, both against. At (22.5, 5.67), 300°, opposite:
    //   `{A, B, 225}`.
    // - B from A's middle (20, 10) to (30, 0): its `from` there, 315°; A's 180°. θ = 135
    //   (A's first), A's against. At (22.5, 14.33), 60°, opposite: `{B, A, reversed, 225}`.
    // - B from (20, 15) to (30, 25), on y = x − 5, meeting A at (15, 10), half B's length
    //   behind its start: A's `from` nearer, 0°; B's `from`, 45°. θ = 45. At (10.3, 8.29),
    //   200°, opposite: `{B, A, 315}`.
    // - The line (-9.5, -10)-(11.5, 11), on y = x − 0.5, across the x axis at (0.5, 0),
    //   within one unit of the origin: its `from` nearer, 45°; the axis's +x. θ = 45. At
    //   (-4.2, -1.71), 200°, opposite: `{line, x_axis, 315}`.
    let a = || with_line(empty(), "n000000a", [10.0, 10.0], [30.0, 10.0]);
    let x_axis = Reference::XAxis;
    let cases = vec![
        (
            with_line(a(), "n000000b", [30.0, 20.0], [20.0, 10.0]),
            [[14.0, 10.0], [25.0, 15.0]],
            [22.5, 5.67],
            angle("n000000a", "n000000b", false, "225"),
        ),
        (
            with_line(a(), "n000000b", [20.0, 10.0], [30.0, 0.0]),
            [[14.0, 10.0], [25.0, 5.0]],
            [22.5, 14.33],
            angle("n000000b", "n000000a", true, "225"),
        ),
        (
            with_line(a(), "n000000b", [20.0, 15.0], [30.0, 25.0]),
            [[14.0, 10.0], [25.0, 20.0]],
            [10.3, 8.29],
            angle("n000000b", "n000000a", false, "315"),
        ),
        (
            with_line(empty(), "n0000001", [-9.5, -10.0], [11.5, 11.0]),
            [[5.0, 4.5], [40.0, 0.2]],
            [-4.2, -1.71],
            Form::Angle {
                a: r("n0000001"),
                b: x_axis,
                reversed: false,
                value: deg("315"),
                driving: true,
            },
        ),
    ];
    for (s, picks, at, want) in cases {
        let mut d = Dim::new(s);
        d.click(picks[0][0], picks[0][1])
            .click(picks[1][0], picks[1][1])
            .click(at[0], at[1])
            .escape();
        assert_eq!(d.placed(), want, "{picks:?} {at:?}");
    }
}

#[test]
fn lines_ending_at_one_place_dont_cross() {
    // A (8.425, 40.42)-(18.198, 42.895) and B (35.64, 49.099)-(18.198, 42.895) both end at
    // Q (18.198, 42.895): at one point the two share, or each `to` a point of its own there.
    // Either way they meet at an end, so they don't cross, though their crosses round (each
    // -17.462741999999906 against -17.462741999999913) to strictly inside both: the vertex
    // is found where an end of one is where an end of the other is, by coordinates, so a
    // single point and two coincident points run the same code. Both rays leave Q back
    // along their lines: B's to (35.64, 49.099), atan2(6.204, 17.442) = 19.5801245°; A's to
    // (8.425, 40.42), atan2(-2.475, -9.773) = 194.2112992°. B's first, θ = 174.6311746°,
    // both against. The pointer at (19.066, 37.971), 280° round from Q, is opposite: the
    // reflex, 360 − θ = 185.368825354, from A's ray round to B's: `{A, B, 185.368825354}`.
    for one_point in [true, false] {
        let b_end = if one_point { "t000000a" } else { "t000000b" };
        let mut s = empty();
        s.points.push(point("f000000a", 8.425, 40.42));
        s.points.push(point("t000000a", 18.198, 42.895));
        s.points.push(point("f000000b", 35.64, 49.099));
        if !one_point {
            s.points.push(point("t000000b", 18.198, 42.895));
        }
        s.lines.push(line("n000000a", "f000000a", "t000000a"));
        s.lines.push(line("n000000b", "f000000b", b_end));
        let mut d = Dim::new(s);
        d.click(13.3115, 41.6575)
            .click(26.919, 45.997)
            .click(19.066, 37.971)
            .escape();
        assert_eq!(
            d.placed(),
            angle("n000000a", "n000000b", false, "185.368825354"),
            "one point: {one_point}"
        );
    }
}

/// The point `length` from `from` at `deg`°.
fn toward(from: Xy, deg: f64, length: f64) -> Xy {
    let a = deg.to_radians();
    [
        from[0] + length * libm::cos(a),
        from[1] + length * libm::sin(a),
    ]
}

/// The slanted T's line A, (0, 0) to (7, 3), and B from `start`, perpendicular to A, to
/// its left (or right), 0.5 of (-3, 7) (or (3, -7)) long; B's start held on A by `on`, or
/// not.
fn slanted_t(start: Xy, left: bool, held: bool) -> Sketch {
    let off = if left { [-1.5, 3.5] } else { [1.5, -3.5] };
    let mut s = with_line(
        with_line(empty(), "n000000a", [0.0, 0.0], [7.0, 3.0]),
        "n000000b",
        start,
        [start[0] + off[0], start[1] + off[1]],
    );
    if held {
        s.constraints.push(SketchConstraint {
            id: id("k0000001"),
            form: Form::On {
                point: r("f000000b"),
                curve: r("n000000a"),
            },
        });
    }
    s
}

#[test]
fn a_slanted_t_held_on_its_line_never_crosses() {
    // Main's ruling: structure first. A (0, 0)-(7, 3) at φ = atan2(3, 7) = 23.2°; B starts
    // at A's k-th ninth, (7k/9, 3k/9), held there by `on`, and leaves perpendicular, to the
    // left (φ + 90) or the right (φ − 90). B's along-ray runs along it; A's from its
    // nearer end: `from` for k ≤ 4, `to` (φ + 180) for k ≥ 5. θ = 90 throughout. The
    // pointer 3 from B's start in the sector opposite the along-rays reads the reflex,
    // 270, never θ, though the bare cross test calls five of these crossing, as B's start
    // rounds a hair to one side of A or the other (k = 1, 2, 4, 8 left, k = 5 right):
    // - left, k ≤ 4: rays A φ, B φ + 90; opposite at φ + 225: `{B, A, 270}`;
    // - left, k ≥ 5: B φ + 90, A φ + 180, A's against; opposite at φ + 315:
    //   `{A, B, reversed, 270}`;
    // - right, k ≤ 4: B φ − 90, A φ; opposite at φ + 135: `{A, B, 270}`;
    // - right, k ≥ 5: A φ + 180, B φ + 270, A's against; opposite at φ + 45:
    //   `{B, A, reversed, 270}`.
    // Picked either way round: an end of the line picked first held on the second, or of the
    // second on the first.
    let phi = libm::atan2(3.0, 7.0).to_degrees();
    for k in 1..=8 {
        let start = [7.0 * f64::from(k) / 9.0, 3.0 * f64::from(k) / 9.0];
        let near_from = k <= 4;
        for left in [true, false] {
            let (turn, want) = match (left, near_from) {
                (true, true) => (225.0, angle("n000000b", "n000000a", false, "270")),
                (true, false) => (315.0, angle("n000000a", "n000000b", true, "270")),
                (false, true) => (135.0, angle("n000000a", "n000000b", false, "270")),
                (false, false) => (45.0, angle("n000000b", "n000000a", true, "270")),
            };
            let at = toward(start, phi + turn, 3.0);
            // A picked well away from B's start, B at its middle.
            let along_a = if near_from { 0.8 } else { 0.2 };
            let pick_a = [7.0 * along_a, 3.0 * along_a];
            let pick_b = if left {
                [start[0] - 0.75, start[1] + 1.75]
            } else {
                [start[0] + 0.75, start[1] - 1.75]
            };
            for picks in [[pick_a, pick_b], [pick_b, pick_a]] {
                let mut d = Dim::new(slanted_t(start, left, true));
                d.click(picks[0][0], picks[0][1])
                    .click(picks[1][0], picks[1][1])
                    .click(at[0], at[1])
                    .escape();
                assert_eq!(d.placed(), want, "k {k}, left {left}, {picks:?}");
            }
        }
    }
}

#[test]
fn a_t_held_at_a_lines_middle_never_crosses() {
    // A (0.1, 0.1)-(2.3, 5.9), at φ; B from A's middle M, held there by `midpoint` (the line
    // tool's middle snap), or by a `symmetric` of A's two ends about B's start, which holds
    // it at the middle as well; B leaves to the left, perpendicular, to M + (-2.9, 1.1), half
    // of A's direction turned. M's coordinates round so that the bare cross test calls them
    // crossing. Held, they never cross. A's own ends are equally near M, so which way A's
    // along-ray runs is the distances' rounding; either way B's runs along it (φ + 90), and
    // of the two sectors φ + 225 and φ + 315 either side of B's opposite, one is opposite
    // the along-rays, the reflex 270, and the other beside, 90. Were they crossing, the
    // opposite one would read θ, 90, too.
    let (p, q) = ([0.1, 0.1], [2.3, 5.9]);
    let d = [q[0] - p[0], q[1] - p[1]];
    let m = [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0];
    let holds = [
        Form::Midpoint {
            point: r("f000000b"),
            line: r("n000000a"),
        },
        Form::Symmetric {
            a: r("f000000a"),
            b: r("t000000a"),
            about: r("f000000b"),
        },
        Form::Symmetric {
            a: r("t000000a"),
            b: r("f000000a"),
            about: r("f000000b"),
        },
    ];
    let pick_a = [p[0] + d[0] * 0.15, p[1] + d[1] * 0.15];
    let pick_b = [m[0] - d[1] * 0.25, m[1] + d[0] * 0.25];
    for (hold, picks) in holds
        .into_iter()
        .flat_map(|hold| [[pick_a, pick_b], [pick_b, pick_a]].map(|picks| (hold.clone(), picks)))
    {
        let mut s = with_line(
            with_line(empty(), "n000000a", p, q),
            "n000000b",
            m,
            [m[0] - d[1] * 0.5, m[1] + d[0] * 0.5],
        );
        s.constraints.push(SketchConstraint {
            id: id("k0000001"),
            form: hold.clone(),
        });
        let phi = libm::atan2(d[1], d[0]).to_degrees();
        let mut values: Vec<String> = [225.0, 315.0]
            .into_iter()
            .map(|turn| {
                let at = toward(m, phi + turn, 2.0);
                let mut dim = Dim::new(s.clone());
                dim.click(picks[0][0], picks[0][1])
                    .click(picks[1][0], picks[1][1])
                    .click(at[0], at[1])
                    .escape();
                match dim.placed() {
                    Form::Angle { value, .. } => value.to_string(),
                    other => panic!("{other:?}"),
                }
            })
            .collect();
        values.sort();
        assert_eq!(values, ["270", "90"], "{hold:?} {picks:?}");
    }
}

#[test]
fn only_a_hold_of_one_lines_end_on_the_other_ties_them() {
    // A (10, 10)-(20, 10) and B (15, 5)-(15, 15) truly cross at (15, 10), each at its
    // middle, so each `to` (FreeCAD's tie): A's ray 180°, B's 270°, θ = 90, both against.
    // The pointer at (18, 13), 45°, is opposite: crossing, θ, `{A, B, 90}`. Holds that tie
    // nothing between them leave it so:
    // - a point P (12, 10) of its own held `on` A (not an end of B);
    // - B's `to` held `on` a circle about (15, 20), radius 5 (not on A);
    // - a point P (15, 10) of its own held at A's middle by `symmetric` of A's ends (not an
    //   end of B);
    // - B's `to` (15, 15) the middle of A's `from` (10, 10) and a point Z (20, 20) by
    //   `symmetric` (not of A's two ends);
    // - a point P (15, 10) of its own held at A's middle by `midpoint` (not an end of B);
    // - B's `to` (15, 15) held at the middle of a third line C (10, 15)-(20, 15) by
    //   `midpoint` (not A's).
    let crossing = || {
        with_line(
            with_line(empty(), "n000000a", [10.0, 10.0], [20.0, 10.0]),
            "n000000b",
            [15.0, 5.0],
            [15.0, 15.0],
        )
    };
    let mut on_a = with_point(crossing(), "p0000001", [12.0, 10.0]);
    on_a.constraints.push(SketchConstraint {
        id: id("k0000001"),
        form: Form::On {
            point: r("p0000001"),
            curve: r("n000000a"),
        },
    });
    let mut on_circle = with_circle(crossing(), "k0000002", [15.0, 20.0], 5.0);
    on_circle.constraints.push(SketchConstraint {
        id: id("k0000001"),
        form: Form::On {
            point: r("t000000b"),
            curve: r("k0000002"),
        },
    });
    let mut about_p = with_point(crossing(), "p0000001", [15.0, 10.0]);
    about_p.constraints.push(SketchConstraint {
        id: id("k0000001"),
        form: Form::Symmetric {
            a: r("f000000a"),
            b: r("t000000a"),
            about: r("p0000001"),
        },
    });
    let mut half_on_a = with_point(crossing(), "z0000001", [20.0, 20.0]);
    half_on_a.constraints.push(SketchConstraint {
        id: id("k0000001"),
        form: Form::Symmetric {
            a: r("f000000a"),
            b: r("z0000001"),
            about: r("t000000b"),
        },
    });
    let mut middle_p = with_point(crossing(), "p0000001", [15.0, 10.0]);
    middle_p.constraints.push(SketchConstraint {
        id: id("k0000001"),
        form: Form::Midpoint {
            point: r("p0000001"),
            line: r("n000000a"),
        },
    });
    let mut middle_c = with_line(crossing(), "n000000c", [10.0, 15.0], [20.0, 15.0]);
    middle_c.constraints.push(SketchConstraint {
        id: id("k0000001"),
        form: Form::Midpoint {
            point: r("t000000b"),
            line: r("n000000c"),
        },
    });
    for s in [on_a, on_circle, about_p, half_on_a, middle_p, middle_c] {
        let mut d = Dim::new(s.clone());
        d.click(17.0, 10.0)
            .click(15.0, 7.0)
            .click(18.0, 13.0)
            .escape();
        assert_eq!(
            d.placed(),
            angle("n000000a", "n000000b", false, "90"),
            "{:?}",
            s.constraints
        );
    }
}

#[test]
fn a_t_merely_near_its_line_is_geometry() {
    let phi = libm::atan2(3.0, 7.0).to_degrees();
    // B from A's fourth ninth, (28/9, 12/9), to the left, with no `on`: the sketch doesn't
    // tie them, so the cross test decides, on the bits. B's start rounds 1.8e-15 to A's
    // left, inside B, so they cross: the sector opposite (φ + 225) reads θ, `{A, B, 90}`.
    let start = [7.0 * 4.0 / 9.0, 3.0 * 4.0 / 9.0];
    let at = toward(start, phi + 225.0, 3.0);
    let mut d = Dim::new(slanted_t(start, true, false));
    d.click(5.6, 2.4)
        .click(start[0] - 0.75, start[1] + 1.75)
        .click(at[0], at[1])
        .escape();
    assert_eq!(d.placed(), angle("n000000a", "n000000b", false, "90"));
}

#[test]
fn a_right_click_locks_a_quadrant_beside_the_along_rays() {
    // The corner: at (5, 20), beside, `{B, A, reversed, 135}`, locked; placed at (25, 13),
    // where unlocked it's `{A, B, 45}`: the reversed 135 is kept.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(15.0, 15.0).right(5.0, 20.0);
    assert!(d.tool.preview().locked);
    d.click(25.0, 13.0).escape();
    assert_eq!(d.placed(), angle("n000000b", "n000000a", true, "135"));
}

#[test]
fn a_line_ending_on_anothers_middle_takes_the_others_to_end() {
    // A T: A (10, 10)-(30, 10) and B from A's middle (20, 10) to (30, 20). B's `from` is the
    // vertex, along it, 45°; A's ends are as near, so its `to` (FreeCAD's tie), 180°.
    // θ = 135, A's against. The pointer at (14, 5), 219.8°: after A's ray, beside,
    // `{A, B, 45}`. And, B ending on A, the lines don't cross (the vertex is inside A, not
    // B), so the sector opposite the along-rays keeps the quadrant rule's reflex: at
    // (22.5, 5.67), 300°, from A's ray round to B's, `{A, B, reversed, 225}`.
    let s = with_line(
        with_line(empty(), "n000000a", [10.0, 10.0], [30.0, 10.0]),
        "n000000b",
        [20.0, 10.0],
        [30.0, 20.0],
    );
    for (at, want) in [
        ([14.0, 5.0], angle("n000000a", "n000000b", false, "45")),
        ([22.5, 5.67], angle("n000000a", "n000000b", true, "225")),
    ] {
        let mut d = Dim::new(s.clone());
        d.click(14.0, 10.0)
            .click(25.0, 15.0)
            .click(at[0], at[1])
            .escape();
        assert_eq!(d.placed(), want, "{at:?}");
    }
}

#[test]
fn a_line_against_an_axis_is_measured_by_the_quadrant_too() {
    // The line (10, 10)-(0, 20), on y = 20 − x, meets the x axis at (20, 0); its `from` is
    // nearer, so its along-ray runs along it, 135°; the axis's, +x. θ = 135. The pointer at
    // (20, 5), 90°: between, `{x_axis, line, 135}`. At (5, 5), 161.6°: beside, SolidWorks'
    // "45 from horizontal", from the line's ray to −x: `{line, x_axis, reversed, 45}`, the
    // axis taken the other way. At (17, -9), 251.6°, opposite (between −x, 180°, and the
    // line's opposite, 315°): the reflex from the line's ray round to +x,
    // `{line, x_axis, 225}`. (The axis's ray is its own direction, whichever way the vertex
    // lies along it.)
    let s = with_line(empty(), "n0000001", [10.0, 10.0], [0.0, 20.0]);
    for (at, a, b, reversed, value) in [
        ([20.0, 5.0], "x_axis", "n0000001", false, "135"),
        ([5.0, 5.0], "n0000001", "x_axis", true, "45"),
        ([17.0, -9.0], "n0000001", "x_axis", false, "225"),
    ] {
        let mut d = Dim::new(s.clone());
        d.click(5.0, 15.0)
            .click(40.0, 0.2)
            .click(at[0], at[1])
            .escape();
        let reference = |text: &str| Reference::new(text).unwrap();
        assert_eq!(
            d.placed(),
            Form::Angle {
                a: reference(a),
                b: reference(b),
                reversed,
                value: deg(value),
                driving: true
            },
            "{at:?}"
        );
    }
}

#[test]
fn the_origin_is_picked_where_its_nearer_than_a_point_and_a_point_on_it_otherwise() {
    // T5. p (0.3, 0.3): at (0.1, 0.1) the origin is 0.14 away, p 0.28: the origin. At
    // (0.25, 0.25), p is nearer. A point exactly at (0, 0) is picked there, not the origin.
    let mut d = Dim::new(with_point(empty(), "p0000001", [0.3, 0.3]));
    d.click(0.1, 0.1);
    assert_eq!(d.tool.preview().picks, vec![Reference::Origin]);
    d.escape().click(0.25, 0.25);
    assert_eq!(d.tool.preview().picks, vec![r("p0000001")]);
    let mut d = Dim::new(with_point(empty(), "p0000001", [0.0, 0.0]));
    d.click(0.0, 0.0);
    assert_eq!(d.tool.preview().picks, vec![r("p0000001")]);
}

#[test]
fn a_point_on_a_centrelines_left_is_measured_from_its_left() {
    // T5. p (-12, 5), left of the construction line facing +y: 12 to its left with the
    // pointer at (-20, 5); at (20, 5), across, nothing.
    let mut d = Dim::new(centreline(-12.0));
    d.click(-12.0, 5.0).click(0.0, 20.0).click(20.0, 5.0);
    assert_eq!(d.messages, vec![Message::AcrossCentreline]);
    // On the centreline itself, (0, 5): p's side, not across.
    d.to(0.0, 5.0);
    assert_eq!(d.tool.preview().unavailable, None);
    assert_eq!(d.shown().unwrap().type_name(), "point_line_distance");
    d.click(-20.0, 5.0).escape();
    assert_eq!(
        d.placed(),
        Form::PointLineDistance {
            point: r("p0000001"),
            line: r("n0000001"),
            value: mm("12"),
            side: LineSide::Left,
            driving: true
        }
    );
}

#[test]
fn a_centreline_stays_picked_for_the_next_dimension() {
    // The help's "additional dimensions": after p's 12 from the centreline, it stays picked;
    // q (25, 30) picked next is 25 from it. A plain line doesn't stay.
    let mut d = Dim::new(with_point(centreline(12.0), "p0000002", [25.0, 30.0]));
    d.click(12.0, 5.0)
        .click(0.0, 20.0)
        .click(20.0, 5.0)
        .escape();
    assert_eq!(d.tool.preview().picks, vec![r("n0000001")]);
    d.click(25.0, 30.0).click(30.0, 30.0).escape();
    assert_eq!(
        d.placed(),
        Form::PointLineDistance {
            point: r("p0000002"),
            line: r("n0000001"),
            value: mm("25"),
            side: LineSide::Right,
            driving: true
        }
    );
    let mut d = Dim::new(point_over_line());
    d.click(30.0, 7.0)
        .click(20.0, 0.0)
        .click(40.0, 20.0)
        .escape();
    assert!(d.tool.preview().picks.is_empty());
}

#[test]
fn a_parallel_line_across_a_centreline_is_refused_as_a_point_is() {
    // The construction line (0, -10)-(0, 40) and L (12, 0)-(12, 30), parallel. L picked
    // first, the centreline second: SD4, the centreline's `from` (0, -10) to L, 12 to its
    // left (L faces +y). The pointer on L's side, (20, 5): placed; across, (-20, 5): not.
    let s = with_line(centreline(50.0), "n0000002", [12.0, 0.0], [12.0, 30.0]);
    let mut d = Dim::new(s);
    d.click(12.0, 15.0).click(0.0, 20.0).click(-20.0, 5.0);
    assert_eq!(d.messages, vec![Message::AcrossCentreline]);
    d.click(20.0, 5.0).escape();
    assert_eq!(
        d.placed(),
        Form::PointLineDistance {
            point: r("f0000001"),
            line: r("n0000002"),
            value: mm("12"),
            side: LineSide::Left,
            driving: true
        }
    );
    // The centreline picked first, L second: SD4, L's `from` (12, 0) to the centreline
    // (facing +y), 12 to its right; across from L, (-20, 5), refused alike.
    let s = with_line(centreline(50.0), "n0000002", [12.0, 0.0], [12.0, 30.0]);
    let mut d = Dim::new(s);
    d.click(0.0, 20.0).click(12.0, 15.0).click(-20.0, 5.0);
    assert_eq!(d.messages, vec![Message::AcrossCentreline]);
    d.click(20.0, 5.0).escape();
    assert_eq!(
        d.placed(),
        Form::PointLineDistance {
            point: r("f0000002"),
            line: r("n0000001"),
            value: mm("12"),
            side: LineSide::Right,
            driving: true
        }
    );
}

#[test]
fn a_pointer_a_subnormal_distance_across_a_centreline_is_across() {
    // p a nanometre left of the y axis, (-1e-6, 5): 0.000001 to its left. The pointer at
    // (1e-320, 5), a subnormal distance to its right, is across: refused. (The product of the
    // two sides, -1e-326, underflows to -0, which isn't below 0: the sides are compared by
    // sign.) At (-1e-320, 5), on p's side, it's placed.
    let mut d = Dim::new(with_point(empty(), "p0000001", [-1e-6, 5.0]));
    d.click(-1e-6, 5.0).click(0.2, 30.0).click(1e-320, 5.0);
    assert_eq!(d.messages, vec![Message::AcrossCentreline]);
    assert_eq!(d.modify(), None);
    d.click(-1e-320, 5.0).escape();
    assert_eq!(
        d.placed(),
        Form::PointLineDistance {
            point: r("p0000001"),
            line: Reference::YAxis,
            value: mm("0.000001"),
            side: LineSide::Left,
            driving: true
        }
    );
    // A point a kilometre left of the y axis, (-1000000, 5), and the pointer at (1e-320, 5):
    // across. (Their quotient, -1e-326, underflows to -0 too.)
    let mut d = Dim::new(with_point(empty(), "p0000001", [-1_000_000.0, 5.0]));
    d.click(-1_000_000.0, 5.0)
        .click(0.2, 30.0)
        .click(1e-320, 5.0);
    assert_eq!(d.messages, vec![Message::AcrossCentreline]);
    assert_eq!(d.modify(), None);
}

#[test]
fn the_box_edge_is_inside_the_box_s_span() {
    // T5: (10, 45) is above the box, on its left edge's line: `distance_x`.
    let mut d = Dim::new(two_points());
    d.click(10.0, 10.0).click(40.0, 30.0).to(10.0, 45.0);
    assert_eq!(d.shown().unwrap().type_name(), "distance_x");
}

#[test]
fn the_lock_goes_with_a_pick_the_read_drops() {
    // T5: p and q, locked horizontal; q gone from the newer read: the lock goes with it.
    let mut d = Dim::new(two_points());
    d.click(10.0, 10.0).click(40.0, 30.0).right(25.0, 45.0);
    assert!(d.tool.preview().locked);
    d.sketch.points.pop();
    d.to(25.0, 45.0);
    assert_eq!(d.tool.preview().picks, vec![r("p0000001")]);
    assert!(!d.tool.preview().locked);
}

#[test]
fn another_sketch_forgets_the_placement() {
    // T5.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(20.0, 30.0).escape();
    assert!(d.tool.placement().is_some());
    let other = FeatureId::new("s0000002").unwrap();
    let context = Context {
        sketch_id: &other,
        revision: d.revision,
        sketch: &d.sketch,
        tolerance: TOLERANCE,
        automatic_relations: true,
        construction: false,
    };
    d.tool.handle(
        &context,
        &under(),
        &mut d.ids,
        DimensionEvent::Move([50.0, 50.0]),
    );
    assert_eq!(d.tool.placement(), None);
}

// --- Review #345, round 2 --------------------------------------------------------------

#[test]
fn a_box_whose_pick_the_read_drops_closes() {
    // Item 2: p and q measured horizontally, 20 typed; q gone from the newer read. With one
    // pick there's no distance; the 20 mustn't be carried onto anything else.
    let mut d = Dim::new(two_points());
    d.click(10.0, 10.0)
        .click(40.0, 30.0)
        .click(25.0, 45.0)
        .typed("20");
    d.sketch.points.pop();
    d.to(25.0, 45.0);
    assert_eq!(d.modify(), None);
    assert_eq!(d.messages, vec![Message::DimensionChanged]);
    d.enter();
    assert!(d.made.is_empty());
}

#[test]
fn a_box_whose_lines_the_read_makes_parallel_closes() {
    // Item 2: the corner's 45° angle in the box, 30 typed; B's start moved to (10, 20), so B
    // runs (10, 20)-(20, 20), parallel to A: SD4's distance, another kind than the angle.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0)
        .click(15.0, 15.0)
        .click(25.0, 13.0)
        .typed("30");
    assert_eq!(d.shown().unwrap().type_name(), "angle");
    d.sketch.points[2].y = Coordinate::new(20.0).unwrap();
    d.to(25.0, 13.0);
    assert_eq!(d.modify(), None);
    assert_eq!(d.messages, vec![Message::DimensionChanged]);
    d.enter();
    assert!(d.made.is_empty());
}

#[test]
fn a_box_kept_through_a_newer_read_keeps_its_angles_order_as_one_kind() {
    // Item 2: Angle's order is one kind. The corner's box placed at (25, 13), {A, B} 45; B's
    // end moved to (20, 30), so A to B sweeps atan2(20, 10) = 63.43494882°: still an
    // angle, the box open on the new value.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(15.0, 15.0).click(25.0, 13.0);
    d.sketch.points[3].y = Coordinate::new(30.0).unwrap();
    d.to(25.0, 13.0);
    assert_eq!(d.modify().as_deref(), Some("63.434948823"));
    assert!(d.messages.is_empty());
}

#[test]
fn a_second_placement_waits_for_the_firsts_read() {
    // Item 3: the first placed, made revision 2; the reads still of revision 1, a second
    // placing click is refused, saying so, its picks kept. Once a read of revision 2 comes,
    // it's placed.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(20.0, 30.0).escape();
    let made = d.revision;
    d.revision = Revision::OPENED;
    d.click(15.0, 15.0).click(30.0, 30.0);
    assert_eq!(d.messages, vec![Message::Solving]);
    assert_eq!(d.modify(), None);
    assert_eq!(d.tool.preview().picks, vec![r("n000000b")]);
    assert_eq!(d.made.len(), 1);
    d.revision = made;
    d.click(30.0, 30.0);
    assert_eq!(d.modify().as_deref(), Some("14.142136"));
    d.escape();
    assert_eq!(d.made.len(), 2);
}

#[test]
fn settle_takes_a_read_with_no_event() {
    // Item 5: placed, then the read of its revision given with no pointer event: settled,
    // so not busy. And a box the newer read changes the kind of closes there, saying why.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(20.0, 30.0).escape();
    assert!(d.tool.busy());
    let sketch_id = sketch_id();
    let context = Context {
        sketch_id: &sketch_id,
        revision: d.revision,
        sketch: &d.sketch,
        tolerance: TOLERANCE,
        automatic_relations: true,
        construction: false,
    };
    let out = d.tool.settle(&context, &under());
    assert_eq!(out, Outcome::default());
    assert!(!d.tool.busy());
    d.click(20.0, 10.0).click(15.0, 15.0).click(25.0, 13.0);
    let mut newer = d.sketch.clone();
    newer.points[2].y = Coordinate::new(20.0).unwrap();
    let context = Context {
        sketch_id: &sketch_id,
        revision: d.revision,
        sketch: &newer,
        tolerance: TOLERANCE,
        automatic_relations: true,
        construction: false,
    };
    let out = d.tool.settle(&context, &under());
    assert_eq!(out.message, Some(Message::DimensionChanged));
    assert_eq!(out.add, None);
    assert_eq!(d.tool.preview().modify, None);
}

#[test]
fn a_box_typed_with_its_own_value_is_touched() {
    // Item 4: "50" typed over the measured 50; the line then 100 long: the box keeps "50",
    // as typed, and Enter places 50.
    let mut d = boxed();
    d.typed("50");
    d.sketch.points[1].x = Coordinate::new(70.0).unwrap();
    d.sketch.points[1].y = Coordinate::new(90.0).unwrap();
    d.to(60.0, 5.0);
    assert_eq!(d.modify().as_deref(), Some("50"));
    d.enter();
    assert_eq!(
        d.placed(),
        Form::Length {
            line: r("n0000001"),
            value: mm("50"),
            driving: true
        }
    );
}

#[test]
fn level_is_within_a_tenth_of_a_micrometre_not_at_it() {
    // Item 6: p (10, 0) and q (40, dy), the pointer off the box's corner (60, 50).
    // dy = 0.99e-7 < 1e-7: level, `distance_x` 30. dy = 1e-7 exactly: not, `distance`,
    // √(30² + 1e-14) = 30 to the nanometre.
    for (dy, kind) in [(0.99e-7, "distance_x"), (1e-7, "distance")] {
        let s = with_point(
            with_point(empty(), "p0000001", [10.0, 0.0]),
            "p0000002",
            [40.0, dy],
        );
        let mut d = Dim::new(s);
        d.click(10.0, 0.0)
            .click(40.0, 0.0)
            .click(60.0, 50.0)
            .escape();
        assert_eq!(d.placed().type_name(), kind, "{dy}");
    }
    // And plumb: p (0, 10) and q (dx, 40), off the corner (60, 50). dx = 1e-7 exactly: not
    // plumb, `distance`; 0.99e-7: plumb, `distance_y`.
    for (dx, kind) in [(0.99e-7, "distance_y"), (1e-7, "distance")] {
        let s = with_point(
            with_point(empty(), "p0000001", [0.0, 10.0]),
            "p0000002",
            [dx, 40.0],
        );
        let mut d = Dim::new(s);
        d.click(0.0, 10.0)
            .click(0.0, 40.0)
            .click(60.0, 50.0)
            .escape();
        assert_eq!(d.placed().type_name(), kind, "{dx}");
    }
}

// --- A press that ends with no release (DimensionEvent::Cancel) ----------------------------

#[test]
fn a_cancelled_press_picks_nothing_and_what_is_picked_stays() {
    // p (10, 10) picked; a press on q (40, 30), alone or moved off it, cancelled: q isn't
    // picked, by the cancel or by a release after it (whose press was cancelled), p stays
    // picked, and the pointer is where it was before the press. Then q clicked and the
    // dimension placed above the box: `distance_x` p to q, 40 − 10 = 30, as a twin that
    // never saw the press places it.
    let p = r("p0000001");
    let q = r("p0000002");
    let moves: [&[Xy]; 2] = [&[], &[[42.0, 33.0], [60.0, 55.0]]];
    for moves in moves {
        let mut d = Dim::new(two_points());
        d.click(10.0, 10.0).to(40.0, 30.0);
        let (preview, busy) = (d.tool.preview().clone(), d.tool.busy());
        d.send(DimensionEvent::Press([40.0, 30.0]));
        for &m in moves {
            d.to(m[0], m[1]);
        }
        assert_eq!(
            d.send(DimensionEvent::Cancel),
            Outcome::default(),
            "{moves:?}"
        );
        assert_eq!(d.tool.preview(), &preview, "{moves:?}");
        assert_eq!(d.tool.busy(), busy, "{moves:?}");
        d.send(DimensionEvent::Release([40.0, 30.0]));
        assert_eq!(d.tool.preview().picks, vec![p.clone()], "{moves:?}");
        d.click(40.0, 30.0).click(25.0, 45.0).escape();

        let mut twin = Dim::new(two_points());
        twin.click(10.0, 10.0).to(40.0, 30.0);
        twin.send(DimensionEvent::Release([40.0, 30.0]));
        twin.click(40.0, 30.0).click(25.0, 45.0).escape();
        assert_eq!(
            twin.placed(),
            Form::DistanceX {
                a: p.clone(),
                b: q.clone(),
                value: mm("30"),
                driving: true
            }
        );
        assert_eq!(d.made, twin.made, "{moves:?}");
        assert_eq!(d.messages, twin.messages, "{moves:?}");
        assert_eq!(d.tool.preview(), twin.tool.preview(), "{moves:?}");
        assert!(!d.left);

        // Not cancelled, the release would have picked q.
        let mut kept = Dim::new(two_points());
        kept.click(10.0, 10.0).to(40.0, 30.0);
        kept.send(DimensionEvent::Press([40.0, 30.0]));
        for &m in moves {
            kept.to(m[0], m[1]);
        }
        kept.send(DimensionEvent::Release([40.0, 30.0]));
        assert_eq!(kept.tool.preview().picks, vec![p.clone(), q.clone()]);
    }
}

#[test]
fn a_cancel_with_no_press_in_progress_changes_nothing() {
    // p clicked (its press released), the pointer moved on to (50, 50): a cancel has no
    // press to put back, so the pointer stays at (50, 50) and p picked.
    let mut d = Dim::new(two_points());
    d.click(10.0, 10.0).to(50.0, 50.0);
    let preview = d.tool.preview().clone();
    assert_eq!(d.send(DimensionEvent::Cancel), Outcome::default());
    assert_eq!(d.tool.preview(), &preview);
    assert_eq!(d.tool.preview().pointer, Some([50.0, 50.0]));
    assert_eq!(d.tool.preview().picks, vec![r("p0000001")]);
    // The Modify box open and typed in; a press (which the box ignores) cancelled: the box
    // and its text stay, and Enter places what was typed.
    d.click(40.0, 30.0).click(25.0, 45.0).typed("12");
    d.send(DimensionEvent::Press([70.0, 70.0]));
    assert_eq!(d.send(DimensionEvent::Cancel), Outcome::default());
    assert_eq!(d.modify().as_deref(), Some("12"));
    d.enter();
    assert_eq!(
        d.placed(),
        Form::DistanceX {
            a: r("p0000001"),
            b: r("p0000002"),
            value: mm("12"),
            driving: true
        }
    );
    // Esc mid-press undoes the last pick; a cancel after it puts nothing back.
    let mut d = Dim::new(two_points());
    d.click(10.0, 10.0).to(40.0, 30.0);
    d.send(DimensionEvent::Press([40.0, 30.0]));
    d.escape();
    d.send(DimensionEvent::Cancel);
    assert!(d.tool.preview().picks.is_empty());
    assert!(!d.tool.busy());
}

#[test]
fn a_cancel_puts_back_what_was_before_the_first_press_and_nothing_once_forgotten() {
    // Two presses on q (40, 30) with no release between (the first's lost), then a cancel:
    // as before the first, no press is down, so a release after it picks nothing.
    let mut d = Dim::new(two_points());
    d.to(70.0, 70.0);
    d.send(DimensionEvent::Press([40.0, 30.0]));
    d.send(DimensionEvent::Press([40.0, 30.0]));
    d.send(DimensionEvent::Cancel);
    assert_eq!(d.tool.preview().pointer, Some([70.0, 70.0]));
    d.send(DimensionEvent::Release([40.0, 30.0]));
    assert!(d.tool.preview().picks.is_empty());
    assert!(!d.tool.busy());
    // A press, then everything forgotten (`forget_pending`, as another sketch does): no
    // press to cancel, so the pointer stays where the press put it.
    let mut d = Dim::new(two_points());
    d.to(70.0, 70.0);
    d.send(DimensionEvent::Press([40.0, 30.0]));
    d.tool.forget_pending();
    d.send(DimensionEvent::Cancel);
    assert_eq!(d.tool.preview().pointer, Some([40.0, 30.0]));
}

#[test]
fn a_cancelled_press_leaves_the_offer_as_the_read_makes_it() {
    // SD7: the add's read flags the dimension, so it's offered. A press on B, cancelled:
    // the offer stays open and holds the tool, a release after it picks nothing, and once
    // the app dismisses it the next dimension is placed.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(20.0, 30.0).escape();
    let k = d.tool.placement().unwrap().constraint;
    d.states.state = SketchState::OverDefined;
    d.states
        .constraints
        .insert(k.clone(), ConstraintState::Conflicting);
    d.to(15.0, 15.0);
    let open = d.tool.open_offer().cloned().unwrap();
    d.send(DimensionEvent::Press([15.0, 15.0]));
    d.send(DimensionEvent::Cancel);
    assert_eq!(d.tool.open_offer(), Some(&open));
    assert!(d.tool.busy());
    d.send(DimensionEvent::Release([15.0, 15.0]));
    assert!(d.tool.preview().picks.is_empty());
    d.tool.dismiss_offer();
    d.click(15.0, 15.0).click(30.0, 30.0);
    assert_eq!(d.modify().as_deref(), Some("14.142136"));
    // A placement the core hasn't answered: a cancelled press leaves it unsettled, so the
    // next placing click waits for it.
    let sketch = corner();
    let sketch_id = sketch_id();
    let context = Context {
        sketch_id: &sketch_id,
        revision: Revision::OPENED,
        sketch: &sketch,
        tolerance: TOLERANCE,
        automatic_relations: true,
        construction: false,
    };
    let mut tool = SmartDimension::new();
    let mut ids = Counter(0);
    let mut send = |tool: &mut SmartDimension, event: DimensionEvent| {
        tool.handle(&context, &under(), &mut ids, event)
    };
    for at in [[20.0, 10.0], [20.0, 30.0]] {
        send(&mut tool, DimensionEvent::Press(at));
        send(&mut tool, DimensionEvent::Release(at));
    }
    assert!(send(&mut tool, DimensionEvent::Escape).add.is_some());
    send(&mut tool, DimensionEvent::Press([15.0, 15.0]));
    assert_eq!(send(&mut tool, DimensionEvent::Cancel), Outcome::default());
    assert!(tool.busy());
    for at in [[15.0, 15.0], [30.0, 30.0]] {
        send(&mut tool, DimensionEvent::Press(at));
        let out = send(&mut tool, DimensionEvent::Release(at));
        if at == [30.0, 30.0] {
            assert_eq!(out.message, Some(Message::Solving));
        }
    }
}

// --- Review #345, round 3 --------------------------------------------------------------

#[test]
fn a_dimension_that_leaves_a_solved_sketchs_freedoms_where_they_were_is_offered() {
    // Item 1: placed on a read with 1000 freedoms; the add's read solved, nothing flagged,
    // its state the same: 1000 or more freedoms is a dependent dimension, offered; 999
    // isn't. A refused solve's freedoms say nothing: not offered on them (state Invalid, no
    // prompt either).
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(20.0, 30.0).typed("25").enter();
    let placement = d.tool.placement().unwrap();
    assert_eq!(placement.dofs, 1000);
    let made = placement.revision;
    let read = |dofs: i32, refused: bool| {
        let mut read = read_of(
            d.sketch.clone(),
            "t0000101",
            ConstraintState::Satisfied,
            if refused {
                SketchState::Invalid
            } else {
                SketchState::UnderDefined
            },
        );
        read.states.dofs = dofs;
        if refused {
            read.states.solve_problem = Some(Error::new(ErrorCode::InvalidResult, "invalid"));
        }
        read
    };
    assert!(offer(&read(1000, false), made, &placement).is_some());
    assert!(offer(&read(1001, false), made, &placement).is_some());
    assert_eq!(offer(&read(999, false), made, &placement), None);
    assert_eq!(offer(&read(1000, true), made, &placement), None);
}

#[test]
fn a_later_read_settles_a_placement_and_the_next_is_placed() {
    // Item 2: the add made revision 2; the first read the tool sees is revision 3 (another
    // edit came first). It can't offer, so it settles: the next dimension is placed.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(20.0, 30.0).escape();
    d.revision = d.revision.next().unwrap();
    d.click(15.0, 15.0).click(30.0, 30.0);
    assert_eq!(d.modify().as_deref(), Some("14.142136"));
    assert!(d.messages.is_empty());
}

#[test]
fn a_later_read_that_flags_the_dimension_still_settles_it() {
    // A read past the add's revision can't offer (the moment's past: it may hold another
    // edit), so even one flagging the dimension settles the placement: the next is placed.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(20.0, 30.0).escape();
    let k = d.tool.placement().unwrap().constraint;
    d.revision = d.revision.next().unwrap();
    d.states.state = SketchState::OverDefined;
    d.states.constraints.insert(k, ConstraintState::Conflicting);
    d.click(15.0, 15.0).click(30.0, 30.0);
    assert_eq!(d.modify().as_deref(), Some("14.142136"));
    assert!(d.messages.is_empty());
}

#[test]
fn a_placement_not_yet_answered_holds_the_next() {
    // Item 2: the add issued, the core's answer (`added`) not yet come: the next placing
    // click waits.
    let sketch = corner();
    let sketch_id = sketch_id();
    let context = Context {
        sketch_id: &sketch_id,
        revision: Revision::OPENED,
        sketch: &sketch,
        tolerance: TOLERANCE,
        automatic_relations: true,
        construction: false,
    };
    let mut tool = SmartDimension::new();
    let mut ids = Counter(0);
    let click = |tool: &mut SmartDimension, ids: &mut Counter, at: [f64; 2]| {
        tool.handle(&context, &under(), ids, DimensionEvent::Press(at));
        tool.handle(&context, &under(), ids, DimensionEvent::Release(at))
    };
    click(&mut tool, &mut ids, [20.0, 10.0]);
    click(&mut tool, &mut ids, [20.0, 30.0]);
    let out = tool.handle(&context, &under(), &mut ids, DimensionEvent::Escape);
    assert!(out.add.is_some());
    click(&mut tool, &mut ids, [15.0, 15.0]);
    let out = click(&mut tool, &mut ids, [30.0, 30.0]);
    assert_eq!(out.message, Some(Message::Solving));
    assert_eq!(tool.preview().modify, None);
}

#[test]
fn an_offer_holds_the_tool_until_its_dismissed() {
    // Item 3: the add's read flags the dimension: offered, so no other dimension is placed
    // (busy, and a placing click says why) until the app dismisses the offer.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(20.0, 30.0).escape();
    let k = d.tool.placement().unwrap().constraint;
    d.states.state = SketchState::OverDefined;
    d.states
        .constraints
        .insert(k.clone(), ConstraintState::Conflicting);
    d.click(15.0, 15.0).click(30.0, 30.0);
    assert_eq!(d.messages, vec![Message::OfferOpen]);
    assert_eq!(d.modify(), None);
    assert!(d.tool.busy());
    let open = d.tool.open_offer().unwrap();
    assert_eq!(open.constraint, k);
    assert_eq!(open.state, ConstraintState::Conflicting);
    // Another placing click on the same read: still held.
    d.click(30.0, 30.0);
    assert_eq!(d.messages, vec![Message::OfferOpen, Message::OfferOpen]);
    d.tool.dismiss_offer();
    assert_eq!(d.tool.open_offer(), None);
    d.click(30.0, 30.0);
    assert_eq!(d.modify().as_deref(), Some("14.142136"));
}

#[test]
fn a_later_read_releases_an_open_offer() {
    // Round 5, item 1: offered; then a read past the add's revision, the dimension still in
    // it and the sketch still over defined: the moment has passed (Driven and Cancel are
    // made against the add's revision), so the offer is released, and the next dimension is
    // placed. The sketch's state shows it's still over defined.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(20.0, 30.0).escape();
    let k = d.tool.placement().unwrap().constraint;
    d.states.state = SketchState::OverDefined;
    d.states
        .constraints
        .insert(k.clone(), ConstraintState::Conflicting);
    d.to(50.0, 50.0);
    assert!(d.tool.open_offer().is_some());
    d.revision = d.revision.next().unwrap();
    d.to(50.0, 50.0);
    assert!(d.sketch.constraints.iter().any(|c| c.id == k));
    assert_eq!(d.tool.open_offer(), None);
    assert!(!d.tool.busy());
    d.click(15.0, 15.0).click(30.0, 30.0);
    assert_eq!(d.modify().as_deref(), Some("14.142136"));
}

#[test]
fn an_add_with_an_edit_before_it_is_judged_through_the_tool_by_its_own_state() {
    // Round 5, item 2: placed on revision 1's read; the core answers the add as revision 3
    // (an edit came between); the tool given revision 3's read, its freedoms not dropped
    // (which across one add would offer). Satisfied: no offer, settled. Redundant, partially
    // redundant, or conflicting: offered.
    for (state, offered) in [
        (ConstraintState::Satisfied, false),
        (ConstraintState::Redundant, true),
        (ConstraintState::PartiallyRedundant, true),
        (ConstraintState::Conflicting, true),
    ] {
        let sketch = corner();
        let sketch_id = sketch_id();
        let one = Revision::OPENED;
        let three = one.next().unwrap().next().unwrap();
        let context = Context {
            sketch_id: &sketch_id,
            revision: one,
            sketch: &sketch,
            tolerance: TOLERANCE,
            automatic_relations: true,
            construction: false,
        };
        let mut tool = SmartDimension::new();
        let mut ids = Counter(0);
        for at in [[20.0, 10.0], [20.0, 30.0]] {
            tool.handle(&context, &under(), &mut ids, DimensionEvent::Press(at));
            tool.handle(&context, &under(), &mut ids, DimensionEvent::Release(at));
        }
        let out = tool.handle(&context, &under(), &mut ids, DimensionEvent::Escape);
        let members = out.add.unwrap();
        let k = members.constraints[0].id.clone();
        tool.added(out.token.unwrap(), three);
        let after = sketch_edit::add(&sketch, &members).unwrap().sketch;
        let mut states = under();
        states.constraints.insert(k, state);
        let context = Context {
            sketch_id: &sketch_id,
            revision: three,
            sketch: &after,
            tolerance: TOLERANCE,
            automatic_relations: true,
            construction: false,
        };
        tool.settle(&context, &states);
        let placement = tool.placement().unwrap();
        assert_eq!(placement.read, one, "{state:?}");
        assert_eq!(placement.revision, three, "{state:?}");
        assert_eq!(tool.open_offer().is_some(), offered, "{state:?}");
        assert_eq!(tool.busy(), offered, "{state:?}");
    }
}

#[test]
fn a_placement_knows_the_read_it_was_placed_on() {
    // Placed on a read of revision 1, its add made revision 2; the next placed on revision
    // 2's read, its add revision 3.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(20.0, 30.0).escape();
    let placement = d.tool.placement().unwrap();
    let two = Revision::OPENED.next().unwrap();
    assert_eq!(placement.read, Revision::OPENED);
    assert_eq!(placement.revision, two);
    d.click(15.0, 15.0).click(30.0, 30.0).escape();
    let placement = d.tool.placement().unwrap();
    assert_eq!(placement.read, two);
    assert_eq!(placement.revision, two.next().unwrap());
}

#[test]
fn with_an_edit_between_its_read_and_its_add_a_dimension_is_judged_by_its_own_state() {
    // Round 4, item 1. Placed on revision 1's read (1000 freedoms, nothing flagged, under
    // defined); an agent's edit made revision 2, the add revision 3.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(20.0, 30.0).typed("25").enter();
    let adjacent = d.tool.placement().unwrap();
    let three = adjacent.revision.next().unwrap();
    let between = Placement {
        revision: three,
        ..adjacent.clone()
    };
    let read = |states: &[(&str, ConstraintState)], dofs: i32, of: SketchState| {
        let mut read = read_with(d.sketch.clone(), states, of);
        read.states.dofs = dofs;
        read
    };
    // An agent's line came between: 4 freedoms more, the dimension took 1, so 1003, and
    // nothing's flagged. Compared with the read before, the freedoms didn't drop, which
    // with nothing between would offer it; with the line between, the dimension is
    // satisfied: not offered.
    let line = read(
        &[("t0000101", ConstraintState::Satisfied)],
        1003,
        SketchState::UnderDefined,
    );
    assert!(offer(&line, adjacent.revision, &adjacent).is_some());
    assert_eq!(offer(&line, three, &between), None);
    // An agent's constraint came between, taking a freedom, and the dimension repeats it:
    // 999, lower than before, which with nothing between wouldn't count it dependent. Its
    // own state, redundant, offers it.
    let constraint = read(
        &[("t0000101", ConstraintState::Redundant)],
        999,
        SketchState::FullyDefined,
    );
    assert!(offer(&constraint, three, &between).is_some());
    // And with the agent's constraint the one flagged, the sketch newly over defined, the
    // dimension satisfied: not the dimension's doing, not offered.
    let theirs = read(
        &[
            ("t0000101", ConstraintState::Satisfied),
            ("k0000009", ConstraintState::Conflicting),
        ],
        999,
        SketchState::OverDefined,
    );
    assert!(offer(&theirs, adjacent.revision, &adjacent).is_some());
    assert_eq!(offer(&theirs, three, &between), None);
}

// --- #345's mutants -----------------------------------------------------------------------

#[test]
fn lines_crossing_by_exactly_freecads_bound_are_not_parallel() {
    // A (0, 5)-(D, 5), D = 1e-9 · 2^30, direction (D, 0); B (20, 1)-(30, 1 + 2^-30), direction
    // (10, 2^-30): each exact (2^30 a power of two, 1 + 2^-30 a double). A × B = D · 2^-30, the
    // double 1e-9 exactly, FreeCAD's `Precision::Intersection()` itself: parallel only below
    // it, so an angle, not SD4's distance. (B is a unit off the x axis, so a pick finds it.)
    let turn = 1_073_741_824.0;
    let s = with_line(
        with_line(empty(), "n000000a", [0.0, 5.0], [1e-9 * turn, 5.0]),
        "n000000b",
        [20.0, 1.0],
        [30.0, 1.0 + 1.0 / turn],
    );
    // A picked 0.3 off its middle, more than the tolerance from both its ends and the y axis.
    let mut d = Dim::new(s);
    d.click(0.537, 5.3)
        .click(25.0, 1.0)
        .click(25.0, 3.0)
        .escape();
    assert_eq!(d.placed().type_name(), "angle");
}

#[test]
fn a_pointer_on_the_first_along_ray_is_between() {
    // The corner: A's ray +x, B's 45°. The pointer at (25, 10), on A's ray, 0° round from
    // it: between, `{A, B, 45}`, not 360° round, beside.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0)
        .click(15.0, 15.0)
        .click(25.0, 10.0)
        .escape();
    assert_eq!(d.placed(), angle("n000000a", "n000000b", false, "45"));
}

#[test]
fn a_refused_number_of_twenty_characters_is_said_whole() {
    // 0.000000000000000001, "0." and 18 digits: 20 characters, said whole, with no "…".
    let mut d = boxed();
    d.typed("0.000000000000000001").enter();
    let Message::ValueRefused(error) = &d.messages[0] else {
        panic!("{:?}", d.messages);
    };
    assert_eq!(
        error.message(),
        "0.000000000000000001 has more than 6 decimals"
    );
}

#[test]
fn the_preview_has_the_pointer_and_hovers_nothing_once_two_are_picked() {
    // p and q picked; the pointer over a third point r (25, 45): the preview is there, and
    // shows no hover, since a click there places rather than picks.
    let mut d = Dim::new(with_point(two_points(), "p0000003", [25.0, 45.0]));
    d.to(10.0, 10.0);
    assert_eq!(d.tool.preview().pointer, Some([10.0, 10.0]));
    assert_eq!(d.tool.preview().hover, Some(r("p0000001")));
    d.click(10.0, 10.0).click(40.0, 30.0).to(25.0, 45.0);
    assert_eq!(d.tool.preview().pointer, Some([25.0, 45.0]));
    assert_eq!(d.tool.preview().hover, None);
}

// --- SD7: the offer ------------------------------------------------------------------

/// A read of the sketch `members` with the constraint `k` in `state`, the sketch in `of`.
fn read_of(members: Sketch, k: &str, state: ConstraintState, of: SketchState) -> SketchRead {
    read_with(members, &[(k, state)], of)
}

/// A read of the sketch `members` with each constraint named in its state, the sketch in
/// `of`.
fn read_with(
    members: Sketch,
    constraints: &[(&str, ConstraintState)],
    of: SketchState,
) -> SketchRead {
    let mut states = states(of);
    for (k, state) in constraints {
        states.constraints.insert(id(k), *state);
    }
    SketchRead {
        sketch: sketch_id(),
        // Every sketch here is on XY (`tests::empty`).
        plane: Ok(silico_protocol::SketchPlane::Xy),
        members,
        states,
        regions: Vec::<Region>::new(),
        profile_problems: Vec::new(),
        profile_problems_total: 0,
    }
}

#[test]
fn a_placement_is_known_once_its_add_is_answered_and_forgotten_if_refused() {
    let mut d = Dim::new(corner());
    let sketch_id = sketch_id();
    let context = Context {
        sketch_id: &sketch_id,
        revision: d.revision,
        sketch: &d.sketch,
        tolerance: TOLERANCE,
        automatic_relations: true,
        construction: false,
    };
    let mut tool = SmartDimension::new();
    for at in [[20.0, 10.0], [20.0, 30.0]] {
        tool.handle(&context, &under(), &mut d.ids, DimensionEvent::Press(at));
        tool.handle(&context, &under(), &mut d.ids, DimensionEvent::Release(at));
    }
    let out = tool.handle(&context, &under(), &mut d.ids, DimensionEvent::Escape);
    let token = out.token.unwrap();
    let k = out.add.unwrap().constraints[0].id.clone();
    assert_eq!(tool.placement(), None);
    let made = Revision::OPENED.next().unwrap();
    tool.added(token, made);
    assert_eq!(
        tool.placement(),
        Some(Placement {
            sketch: sketch_id.clone(),
            constraint: k,
            revision: made,
            read: Revision::OPENED,
            before: SketchState::UnderDefined,
            flagged: BTreeSet::new(),
            dofs: 0,
        })
    );
    tool.refused(token);
    assert_eq!(tool.placement(), None);
}

#[test]
fn an_add_that_over_defines_or_unsolves_the_sketch_is_offered_driven_leave_or_cancel() {
    // The corner with `length` of A, 25, driving, placed as t0000101 at revision 2 on a read
    // under defined. A read of revision 2 over defined, or with no solution, offers it,
    // whatever state the solve gave the dimension itself (PlaneGCS may flag another of its
    // group).
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(20.0, 30.0).typed("25").enter();
    let placement = d.tool.placement().unwrap();
    assert_eq!(placement.constraint, id("t0000101"));
    assert_eq!(placement.before, SketchState::UnderDefined);
    let made = Revision::OPENED.next().unwrap();
    assert_eq!(placement.revision, made);
    for sketch_state in [SketchState::OverDefined, SketchState::NoSolution] {
        for state in [
            ConstraintState::Conflicting,
            ConstraintState::Redundant,
            ConstraintState::Satisfied,
            ConstraintState::Unsolved,
        ] {
            let read = read_of(d.sketch.clone(), "t0000101", state, sketch_state);
            let offer = offer(&read, made, &placement).unwrap();
            assert_eq!(offer.constraint, id("t0000101"));
            assert_eq!(offer.state, state);
            assert_eq!(offer.sketch_state, sketch_state);
            assert_eq!(
                offer.request(OfferChoice::Driven),
                Some(OfferRequest::SetField {
                    revision: made,
                    feature: sketch_id(),
                    field: "constraints.t0000101.driving".to_owned(),
                    value: json::read(b"false")
                        .map(|mut n| {
                            json::unplace(&mut n);
                            n
                        })
                        .unwrap(),
                })
            );
            assert_eq!(offer.request(OfferChoice::Leave), None);
            assert_eq!(
                offer.request(OfferChoice::Cancel),
                Some(OfferRequest::Undo {
                    revision: made,
                    to: None
                })
            );
        }
    }
    assert_eq!(
        OfferChoice::ALL,
        [OfferChoice::Driven, OfferChoice::Leave, OfferChoice::Cancel]
    );
}

#[test]
fn an_add_that_flags_something_new_is_offered_whatever_the_state_before() {
    // Round 2, item 1: SolidWorks prompts for each dimension. The sketch over defined or
    // unsolvable before it (left so, or by an agent's edit), the add's read flags the
    // dimension, which the read before couldn't: offered. So is a flag that leaves the
    // sketch fully defined (PlaneGCS may flag a relation of the group, item 32's mixed
    // group), and one of another constraint than the dimension.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(20.0, 30.0).typed("25").enter();
    let placement = d.tool.placement().unwrap();
    let made = placement.revision;
    let flags_it = read_of(
        d.sketch.clone(),
        "t0000101",
        ConstraintState::Conflicting,
        SketchState::OverDefined,
    );
    for before in [SketchState::OverDefined, SketchState::NoSolution] {
        let earlier = Placement {
            before,
            ..placement.clone()
        };
        let offer = offer(&flags_it, made, &earlier).unwrap();
        assert_eq!(offer.state, ConstraintState::Conflicting, "{before:?}");
    }
    let redundant_relation = read_with(
        d.sketch.clone(),
        &[
            ("t0000101", ConstraintState::Satisfied),
            ("k0000009", ConstraintState::Redundant),
        ],
        SketchState::FullyDefined,
    );
    assert!(offer(&redundant_relation, made, &placement).is_some());
}

#[test]
fn no_offer_where_the_add_flagged_nothing_new_nor_changed_the_state_to_one_prompted() {
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(20.0, 30.0).typed("25").enter();
    let placement = d.tool.placement().unwrap();
    let made = placement.revision;
    let read = |of| read_of(d.sketch.clone(), "t0000101", ConstraintState::Satisfied, of);
    // Nothing flagged, the sketch still solved, or invalid geometry (no prompt in
    // SolidWorks).
    for of in [
        SketchState::UnderDefined,
        SketchState::FullyDefined,
        SketchState::Invalid,
    ] {
        assert_eq!(offer(&read(of), made, &placement), None, "{of:?}");
    }
    // Over defined before it, by k0000009, which the add's read flags still, and nothing
    // more: not this one's doing.
    let same = read_with(
        d.sketch.clone(),
        &[
            ("t0000101", ConstraintState::Unsolved),
            ("k0000009", ConstraintState::Conflicting),
        ],
        SketchState::OverDefined,
    );
    let earlier = Placement {
        before: SketchState::OverDefined,
        flagged: [id("k0000009")].into_iter().collect(),
        ..placement.clone()
    };
    assert_eq!(offer(&same, made, &earlier), None);
    let over = read_of(
        d.sketch.clone(),
        "t0000101",
        ConstraintState::Conflicting,
        SketchState::OverDefined,
    );
    // A read of an older model, or a newer one: not the add's.
    assert_eq!(offer(&over, Revision::OPENED, &placement), None);
    assert_eq!(offer(&over, made.next().unwrap(), &placement), None);
    // Another sketch's read.
    let mut elsewhere = over.clone();
    elsewhere.sketch = FeatureId::new("s0000002").unwrap();
    assert_eq!(offer(&elsewhere, made, &placement), None);
    // Already driven, or gone.
    let mut driven = over.clone();
    if let Form::Length { driving, .. } = &mut driven.members.constraints.last_mut().unwrap().form {
        *driving = false;
    }
    assert_eq!(offer(&driven, made, &placement), None);
    let mut gone = over;
    gone.members.constraints.clear();
    assert_eq!(offer(&gone, made, &placement), None);
}

#[test]
fn the_state_a_dimension_is_placed_on_is_the_reads() {
    let mut d = Dim::new(corner());
    d.states.state = SketchState::NoSolution;
    d.states
        .constraints
        .insert(id("k0000009"), ConstraintState::Conflicting);
    d.states
        .constraints
        .insert(id("k0000008"), ConstraintState::Satisfied);
    d.states
        .constraints
        .insert(id("k0000007"), ConstraintState::PartiallyRedundant);
    d.click(20.0, 10.0).click(20.0, 30.0).escape();
    let placement = d.tool.placement().unwrap();
    assert_eq!(placement.before, SketchState::NoSolution);
    assert_eq!(
        placement.flagged,
        [id("k0000007"), id("k0000009")].into_iter().collect()
    );
}

#[test]
fn a_placement_is_busy_until_its_read_comes_or_it_is_dismissed() {
    // Placed against revision 1, answered as revision 2: busy while the reads are of 1.
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(20.0, 30.0).escape();
    let made = d.revision;
    d.revision = Revision::OPENED;
    d.to(50.0, 50.0);
    assert!(d.tool.busy());
    d.tool.dismiss_offer();
    assert!(!d.tool.busy());
    assert_eq!(d.tool.placement(), None);
    d.revision = made;
    d.click(20.0, 10.0).click(20.0, 30.0).escape();
    assert!(d.tool.busy());
    d.to(50.0, 50.0);
    assert!(!d.tool.busy());
    assert!(d.tool.placement().is_some());
}

#[test]
fn forget_pending_forgets_the_placement_and_what_is_under_way() {
    let mut d = Dim::new(corner());
    d.click(20.0, 10.0).click(20.0, 30.0).escape();
    d.click(20.0, 10.0);
    d.tool.forget_pending();
    assert_eq!(d.tool.placement(), None);
    assert!(!d.tool.busy());
    d.to(50.0, 50.0);
    assert!(d.tool.preview().picks.is_empty());
}

// --- Any events at all -----------------------------------------------------------------

mod property {
    use proptest::prelude::*;
    use proptest::test_runner::RngSeed;

    use super::*;

    /// Something of every kind to pick: two lines meeting, a parallel one, a construction
    /// line, a circle, an arc, lone points (two at one place), and points far apart.
    fn busy() -> Sketch {
        let mut s = construction(
            with_line(corner(), "n000000c", [-20.0, -20.0], [-20.0, 40.0]),
            "n000000c",
        );
        s = with_line(s, "n000000d", [15.0, 30.0], [35.0, 30.0]);
        s = with_circle(s, "k0000001", [40.0, -20.0], 6.0);
        s.points.push(point("c0000009", 50.0, 50.0));
        s.points.push(point("q0000003", 60.0, 50.0));
        s.points.push(point("q0000004", 50.0, 60.0));
        s.arcs.push(SketchArc {
            id: id("a0000001"),
            center: id("c0000009"),
            start: id("q0000003"),
            end: id("q0000004"),
            construction: false,
        });
        s = with_point(s, "p0000001", [5.0, 45.0]);
        s = with_point(s, "p0000002", [5.0, 45.0]);
        s = with_point(s, "p0000003", [-900_000.0, 0.0]);
        with_point(s, "p0000004", [900_000.0, 1.0])
    }

    fn coordinate() -> impl Strategy<Value = f64> {
        prop_oneof![
            8 => -40.0..70.0f64,
            3 => prop::sample::select(vec![
                0.0, 5.0, 10.0, 15.0, 20.0, 30.0, 40.0, 45.0, 50.0, 60.0, -20.0,
            ]),
            1 => prop::sample::select(vec![
                -900_000.0, 900_000.0, 1e300, f64::NAN, f64::INFINITY, 1e-300,
            ]),
        ]
    }

    fn text() -> impl Strategy<Value = String> {
        prop_oneof![
            3 => prop::sample::select(vec![
                "10", "-10", "0", "-0", "45", "360", "400", "1e400", "0.0000001", "abc", "",
                "10/3", "1e7", "2.5", "1000000", "0.000001",
            ])
            .prop_map(str::to_owned),
            1 => ".{0,12}",
        ]
    }

    fn event() -> impl Strategy<Value = DimensionEvent> {
        let at = || (coordinate(), coordinate()).prop_map(|(x, y)| [x, y]);
        prop_oneof![
            4 => at().prop_map(DimensionEvent::Press),
            4 => at().prop_map(DimensionEvent::Move),
            4 => at().prop_map(DimensionEvent::Release),
            1 => at().prop_map(DimensionEvent::RightClick),
            2 => Just(DimensionEvent::Escape),
            2 => Just(DimensionEvent::Enter),
            2 => text().prop_map(DimensionEvent::Typed),
            1 => Just(DimensionEvent::Cancel),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 600,
            failure_persistence: None,
            rng_seed: RngSeed::Fixed(0x5d_1300),
            ..ProptestConfig::default()
        })]

        /// Whatever the events and the tolerance: nothing panics; every add the tool makes
        /// is one driving dimension the gate takes (`Dim::send` puts each through it, the
        /// next event against the sketch it leaves), and writes as a request's lists.
        #[test]
        fn any_events_make_only_driving_dimensions_the_gate_takes(
            events in prop::collection::vec(event(), 0..48),
            tolerance in prop::sample::select(vec![0.0, 1e-9, 0.5, 5.0, 1e3, f64::NAN, -1.0]),
        ) {
            let mut d = Dim::new(busy());
            d.tolerance = tolerance;
            for event in events {
                d.send(event);
            }
            for members in &d.made {
                prop_assert!(crate::command::NewMembers::of(members).is_ok());
                prop_assert_eq!(members.constraints[0].form.driving(), Some(true));
            }
        }
    }
}
