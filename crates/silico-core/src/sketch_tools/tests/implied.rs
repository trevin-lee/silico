//! The relations a sketch already implies, which a tool leaves out and the pointer never
//! shows (#278's G as amended, Fable, 2026-11-08, g5-redundant-autoconstraints-fable.md;
//! `sketch_tools::implied`). Every expected value is worked by hand from its two rules: an
//! inferred relation asserting two points level (or plumb) goes when they're already in one
//! class, or both their classes are fixed (Rule E); an inferred `perpendicular`, `parallel`,
//! or `tangent_at` goes when every point it reads is fixed both ways (Rule F). A class is
//! fixed when it holds the origin or a point the read marks fully defined.

use super::*;
use crate::sketch_states::MemberState;
use crate::sketch_tools::implied_by;

/// The forms of each constraint the adds made, by type, sorted.
fn types(d: &Drive) -> Vec<&'static str> {
    let mut types: Vec<&'static str> = d
        .made
        .iter()
        .flat_map(|m| &m.constraints)
        .map(|c| c.form.type_name())
        .collect();
    types.sort();
    types
}

/// `d`'s read marks the points `ids` fully defined.
fn fixed(d: &mut Drive, ids: &[&str]) {
    for p in ids {
        d.states.points.insert(id(p), MemberState::FullyDefined);
    }
}

/// Whether the preview shows a relation like `relation` anywhere.
fn shows(d: &Drive, relation: &Relation) -> bool {
    d.tool
        .preview()
        .shown
        .iter()
        .any(|s| &s.relation == relation)
}

/// The L the brief draws from the origin, by six clicks and a seventh back on its first
/// point: (0, 0), (40, 0), (40, 10), (10, 10), (10, 30), (0, 30), (0, 0).
fn the_l(d: &mut Drive) {
    for (x, y) in [
        (0.0, 0.0),
        (40.0, 0.0),
        (40.0, 10.0),
        (10.0, 10.0),
        (10.0, 30.0),
        (0.0, 30.0),
        (0.0, 0.0),
    ] {
        d.click(x, y);
    }
}

#[test]
fn the_l_from_the_origin_sends_no_direction_its_points_already_fix() {
    // Segment 1: (0, 0) level and plumb with the origin, (40, 0) on the x axis; both are in
    // the origin's Y class, so the line's `horizontal` goes. Segments 2 to 5 each join two
    // classes of their own: the direction stays. The closing segment: (0, 30) on the y axis,
    // (0, 0) plumb with the origin, one X class, so its `vertical` goes. Two relations fewer
    // than every one the pointer finds.
    let mut d = Drive::new(ToolKind::Line, empty());
    the_l(&mut d);
    assert_eq!(
        d.adds(),
        [
            sorted(&[
                "point (0, 0)",
                "point (40, 0)",
                "line (0, 0)-(40, 0)",
                "horizontal_points (0, 0) origin",
                "vertical_points (0, 0) origin",
                "on (40, 0) x_axis",
            ]),
            sorted(&[
                "point (40, 10)",
                "line #(40, 0)-(40, 10)",
                "vertical line #(40, 0)-(40, 10)",
            ]),
            sorted(&[
                "point (10, 10)",
                "line #(40, 10)-(10, 10)",
                "horizontal line #(40, 10)-(10, 10)",
            ]),
            sorted(&[
                "point (10, 30)",
                "line #(10, 10)-(10, 30)",
                "vertical line #(10, 10)-(10, 30)",
            ]),
            sorted(&[
                "point (0, 30)",
                "line #(10, 30)-(0, 30)",
                "horizontal line #(10, 30)-(0, 30)",
                "on (0, 30) y_axis",
            ]),
            sorted(&["line #(0, 30)-#(0, 0)"]),
        ]
    );
}

#[test]
fn a_chain_along_the_x_axis_from_the_origin_has_no_direction_at_all() {
    // (0, 0), (10, 0), (20, 0), (30, 0), then Enter: each new point on the x axis, so each
    // segment's ends are in the origin's Y class before its `horizontal` is judged.
    let mut d = Drive::new(ToolKind::Line, empty());
    for x in [0.0, 10.0, 20.0, 30.0] {
        d.click(x, 0.0);
    }
    d.send(Event::Enter);
    assert_eq!(
        d.adds(),
        [
            sorted(&[
                "point (0, 0)",
                "point (10, 0)",
                "line (0, 0)-(10, 0)",
                "horizontal_points (0, 0) origin",
                "vertical_points (0, 0) origin",
                "on (10, 0) x_axis",
            ]),
            sorted(&[
                "point (20, 0)",
                "line #(10, 0)-(20, 0)",
                "on (20, 0) x_axis"
            ]),
            sorted(&[
                "point (30, 0)",
                "line #(20, 0)-(30, 0)",
                "on (30, 0) x_axis"
            ]),
        ]
    );
}

#[test]
fn a_line_between_two_fixed_points_gets_no_direction_level_or_not() {
    // (10, 10) and (40, 10.785), both fully defined: 0.785 over 30 is 1.499°, within the 2°
    // that shows `horizontal`. Both Y classes are fixed, so it goes, agreeing or not (here
    // not: added, it would conflict), and the pointer shows no glyph. The same between
    // (10, 10) and (40, 10), level: both fixed, it goes too.
    for to in [10.785, 10.0] {
        let mut s = empty();
        s.points.push(point("q0000001", 10.0, 10.0));
        s.points.push(point("q0000002", 40.0, to));
        let mut d = Drive::new(ToolKind::Line, s);
        fixed(&mut d, &["q0000001", "q0000002"]);
        d.to(10.0, 10.0).press(10.0, 10.0).to(40.0, to);
        assert!(!shows(&d, &Relation::Horizontal), "to {to}");
        d.release(40.0, to);
        assert_eq!(d.made.len(), 1, "to {to}");
        assert_eq!(types(&d), Vec::<&str>::new(), "to {to}");
    }
}

#[test]
fn a_line_from_a_fixed_point_to_a_free_end_keeps_its_direction_and_the_end_is_moved_onto_it() {
    // From (10, 10), fully defined, to a free end at (40, 10.785), 1.499° off level: a free
    // end is a class of its own, never fixed, so `horizontal` stays, and the end is moved
    // onto the level, (40, 10).
    let mut s = empty();
    s.points.push(point("q0000001", 10.0, 10.0));
    let mut d = Drive::new(ToolKind::Line, s);
    fixed(&mut d, &["q0000001"]);
    d.drag([10.0, 10.0], [40.0, 10.785]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (40, 10)",
            "line #(10, 10)-(40, 10)",
            "horizontal line #(10, 10)-(40, 10)",
        ])]
    );
}

#[test]
fn a_line_from_a_point_on_the_axis_to_a_fixed_point_on_it_has_no_horizontal() {
    // (10, 0), `on` the x axis (its y fixed, its x free: under defined), and (40, 0), fully
    // defined: the first's Y class is the origin's, fixed, and so is the second's, so the
    // `horizontal` goes. (Every point fully defined, the study's P2, wouldn't catch this.)
    let mut s = empty();
    s.points.push(point("q0000001", 10.0, 0.0));
    s.points.push(point("q0000002", 40.0, 0.0));
    s.constraints.push(SketchConstraint {
        id: id("k0000001"),
        form: Form::On {
            point: Reference::Element(id("q0000001")),
            curve: Reference::XAxis,
        },
    });
    let mut d = Drive::new(ToolKind::Line, s);
    fixed(&mut d, &["q0000002"]);
    d.drag([10.0, 0.0], [40.0, 0.0]);
    assert_eq!(d.adds(), [sorted(&["line #(10, 0)-#(40, 0)"])]);
}

#[test]
fn a_point_on_a_level_line_is_level_with_it_through_the_line() {
    // M from (10, 20) to (50, 20), `horizontal`; (20, 20) `on` M; (70, 20) level with M's
    // first end by `horizontal_points`. The point on M is in M's Y class (a point on the
    // line through a level line is level with it), and so is (70, 20): the line between them
    // gets no `horizontal`.
    let mut s = with_line(empty(), "m0000001", [10.0, 20.0], [50.0, 20.0]);
    s.points.push(point("q0000001", 20.0, 20.0));
    s.points.push(point("q0000002", 70.0, 20.0));
    let r = |text: &str| Reference::Element(id(text));
    for (k, form) in [
        Form::Horizontal {
            line: r("m0000001"),
        },
        Form::On {
            point: r("q0000001"),
            curve: r("m0000001"),
        },
        Form::HorizontalPoints {
            a: r("q0000002"),
            b: r("f0000001"),
        },
    ]
    .into_iter()
    .enumerate()
    {
        s.constraints.push(SketchConstraint {
            id: id(&format!("k000000{k}")),
            form,
        });
    }
    let mut d = Drive::new(ToolKind::Line, s);
    d.drag([20.0, 20.0], [70.0, 20.0]);
    assert_eq!(d.adds(), [sorted(&["line #(20, 20)-#(70, 20)"])]);
}

#[test]
fn a_point_at_the_middle_of_a_line_on_the_axis_is_its_midpoint_alone() {
    // The Point tool at (30, 0.1), the middle of (10, 0)-(50, 0), which lies along the x
    // axis: `midpoint`, and no `on` of the axis (the line covers it there, and its middle is
    // level with the origin through it). At the middle of (30, -10)-(30, 10), which the x
    // axis crosses there, (30, 0): both, `midpoint` and `on` the axis, each joining classes.
    let mut d = Drive::new(
        ToolKind::Point,
        with_line(empty(), "n0000001", [10.0, 0.0], [50.0, 0.0]),
    );
    d.click(30.0, 0.1);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (30, 0)",
            "midpoint (30, 0) line #(10, 0)-#(50, 0)",
        ])]
    );
    let mut d = Drive::new(
        ToolKind::Point,
        with_line(empty(), "n0000001", [30.0, -10.0], [30.0, 10.0]),
    );
    d.click(30.1, 0.0);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (30, 0)",
            "midpoint (30, 0) line #(30, -10)-#(30, 10)",
            "on (30, 0) x_axis",
        ])]
    );
}

#[test]
fn a_point_at_the_middle_of_a_level_line_is_on_a_line_level_with_it_only_once() {
    // M (10, 20)-(50, 20) and N (0, 20)-(60, 20), both `horizontal`, and N's first end level
    // with M's by `horizontal_points`: one Y class. The Point tool at M's middle, (30, 20),
    // which N runs through exactly: `midpoint` of M, and no `on` of N (the middle is level
    // with N's ends already, through M). Without the `horizontal_points`, M and N are two
    // classes, and the `on` of N joins them: kept.
    for joined in [true, false] {
        let mut s = with_line(empty(), "m0000001", [10.0, 20.0], [50.0, 20.0]);
        s = with_line(s, "n0000002", [0.0, 20.0], [60.0, 20.0]);
        let r = |text: &str| Reference::Element(id(text));
        let mut forms = vec![
            Form::Horizontal {
                line: r("m0000001"),
            },
            Form::Horizontal {
                line: r("n0000002"),
            },
        ];
        if joined {
            forms.push(Form::HorizontalPoints {
                a: r("f0000002"),
                b: r("f0000001"),
            });
        }
        for (k, form) in forms.into_iter().enumerate() {
            s.constraints.push(SketchConstraint {
                id: id(&format!("k000000{k}")),
                form,
            });
        }
        let mut d = Drive::new(ToolKind::Point, s);
        d.click(30.0, 20.1);
        let types = types(&d);
        let count = |t: &str| types.iter().filter(|u| **u == t).count();
        assert_eq!(
            (count("midpoint"), count("on")),
            (1, usize::from(!joined)),
            "joined: {joined}: {:?}",
            d.adds()
        );
    }
}

#[test]
fn a_perpendicular_between_fixed_points_goes_and_to_a_free_end_stays() {
    // K from (10, 10) to (30, 20), both ends fully defined; from K's end (30, 20) a line to
    // (20, 40), along (-10, 20), at right angles to K's (20, 10): `perpendicular` to K shows.
    // To (20, 40) fully defined, every point it reads is fixed: it goes (Rule F). To a free
    // end there, it stays.
    for end_fixed in [true, false] {
        let mut s = with_line(empty(), "n0000001", [10.0, 10.0], [30.0, 20.0]);
        if end_fixed {
            s.points.push(point("q0000001", 20.0, 40.0));
        }
        let mut d = Drive::new(ToolKind::Line, s);
        fixed(&mut d, &["f0000001", "t0000001"]);
        if end_fixed {
            fixed(&mut d, &["q0000001"]);
        }
        d.drag([30.0, 20.0], [20.0, 40.0]);
        assert_eq!(d.made.len(), 1, "end fixed: {end_fixed}");
        let want: &[&str] = if end_fixed { &[] } else { &["perpendicular"] };
        assert_eq!(types(&d), want, "end fixed: {end_fixed}");
    }
}

#[test]
fn a_tangent_from_an_arcs_fixed_end_to_a_fixed_point_goes_and_to_a_free_end_stays() {
    // The arc about (50, 50), radius 10, from 0° (60, 50) to 45° (50 + 5√2, 50 + 5√2): past
    // its end it runs on along 135°. A line from its end along 135°, 20 long, to
    // (50 + 5√2 − 10√2, 50 + 5√2 + 10√2): `tangent_at` shows. With the arc's centre and end
    // and the line's far end fully defined, every point it reads is fixed: it goes (Rule F).
    // To a free far end, it stays.
    let h = 5.0 * std::f64::consts::SQRT_2;
    let end = [50.0 + h, 50.0 + h];
    let far = [end[0] - 2.0 * h, end[1] + 2.0 * h];
    for far_fixed in [true, false] {
        let mut s = empty();
        s.points.push(point("c0000001", 50.0, 50.0));
        s.points.push(point("q0000101", 60.0, 50.0));
        s.points.push(point("q0000102", end[0], end[1]));
        s.arcs.push(crate::document::SketchArc {
            id: id("a0000001"),
            center: id("c0000001"),
            start: id("q0000101"),
            end: id("q0000102"),
            construction: false,
        });
        if far_fixed {
            s.points.push(point("q0000001", far[0], far[1]));
        }
        let mut d = Drive::new(ToolKind::Line, s);
        fixed(&mut d, &["c0000001", "q0000101", "q0000102"]);
        if far_fixed {
            fixed(&mut d, &["q0000001"]);
        }
        d.drag(end, far);
        assert_eq!(d.made.len(), 1, "far end fixed: {far_fixed}");
        let want: &[&str] = if far_fixed { &[] } else { &["tangent_at"] };
        assert_eq!(types(&d), want, "far end fixed: {far_fixed}");
    }
}

#[test]
fn a_rectangle_and_a_centre_rectangle_at_the_origin_send_all_their_own_relations() {
    // A corner rectangle from the origin to (40, 20): its four sides' `horizontal` and
    // `vertical` (its own, never judged), and the corner's two to the origin. A centre
    // rectangle about the origin to (20, 10): its sides' four, its `symmetric`, and the
    // centre's two.
    let mut d = Drive::new(ToolKind::CornerRectangle, empty());
    d.drag([0.0, 0.0], [40.0, 20.0]);
    assert_eq!(
        types(&d),
        [
            "horizontal",
            "horizontal",
            "horizontal_points",
            "vertical",
            "vertical",
            "vertical_points"
        ]
    );
    let mut d = Drive::new(ToolKind::CenterRectangle, empty());
    d.drag([0.0, 0.0], [20.0, 10.0]);
    let types = types(&d);
    assert_eq!(
        types.iter().filter(|t| **t == "horizontal").count(),
        2,
        "{types:?}"
    );
    assert_eq!(
        types.iter().filter(|t| **t == "vertical").count(),
        2,
        "{types:?}"
    );
    assert_eq!(
        types.iter().filter(|t| **t == "symmetric").count(),
        1,
        "{types:?}"
    );
}

#[test]
fn with_automatic_relations_off_or_ctrl_held_the_l_has_no_relations_to_leave_out() {
    // Nothing inferred, nothing judged: the L's six lines and points alone.
    for ctrl in [false, true] {
        let mut d = Drive::new(ToolKind::Line, empty());
        if ctrl {
            d.keys.ctrl = true;
        } else {
            d.automatic = false;
        }
        the_l(&mut d);
        assert_eq!(d.made.len(), 6, "ctrl: {ctrl}");
        assert_eq!(types(&d), Vec::<&str>::new(), "ctrl: {ctrl}");
    }
}

/// IDs from `zzzzzzzz` down, in reverse lexical order, skipping any taken.
struct Reversed(u64);

impl NewIds for Reversed {
    fn next(&mut self, taken: &dyn Fn(&ElementId) -> bool) -> Result<ElementId, Error> {
        const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
        loop {
            self.0 += 1;
            let mut n = u64::pow(32, 8) - self.0;
            let mut text = [b'0'; 8];
            for c in text.iter_mut().rev() {
                *c = ALPHABET[usize::try_from(n % 32).unwrap()];
                n /= 32;
            }
            let id = ElementId::new(std::str::from_utf8(&text).unwrap()).unwrap();
            if !taken(&id) {
                return Ok(id);
            }
        }
    }
}

#[test]
fn ids_handed_out_in_reverse_order_leave_the_same_relations_out() {
    // The L and the chain along the axis again, their IDs from `zzzzzzzz` down: the same
    // adds, described by place, so the same relations left out, whatever order IDs sort in.
    for gesture in [the_l as fn(&mut Drive), |d: &mut Drive| {
        for x in [0.0, 10.0, 20.0, 30.0] {
            d.click(x, 0.0);
        }
        d.send(Event::Enter);
    }] {
        let mut ours = Drive::new(ToolKind::Line, empty());
        gesture(&mut ours);
        let mut reversed = Drive::new(ToolKind::Line, empty());
        reversed.ids = Box::new(Reversed(0));
        gesture(&mut reversed);
        assert_eq!(ours.adds(), reversed.adds());
    }
}

// ---- #431's review, round 1: what the classes read ----

/// `forms` as `s`'s constraints, numbered k0000001 on, in order.
fn constrained(mut s: Sketch, forms: Vec<Form>) -> Sketch {
    for (k, form) in forms.into_iter().enumerate() {
        s.constraints.push(SketchConstraint {
            id: id(&format!("k{:07}", k + 1)),
            form,
        });
    }
    s
}

/// A read's states marking the points `ids` fully defined, and nothing else.
fn fixing(ids: &[&str]) -> SketchStates {
    let mut states = no_states().clone();
    for p in ids {
        states.points.insert(id(p), MemberState::FullyDefined);
    }
    states
}

fn el(text: &str) -> Reference {
    Reference::Element(id(text))
}

fn on(point: &str, curve: Reference) -> Form {
    Form::On {
        point: el(point),
        curve,
    }
}

fn level(a: &str, b: &str) -> Form {
    Form::HorizontalPoints { a: el(a), b: el(b) }
}

fn plumb(a: &str, b: &str) -> Form {
    Form::VerticalPoints { a: el(a), b: el(b) }
}

#[test]
fn a_line_from_the_origin_up_the_y_axis_has_no_vertical() {
    // (0, 0), then (0, 40), then Enter: the start at the origin, level and plumb with it;
    // the end on the y axis, plumb with it. Both in the origin's X class, so the line's
    // `vertical` goes: the chain along the x axis's case, mirrored onto y.
    let mut d = Drive::new(ToolKind::Line, empty());
    d.click(0.0, 0.0);
    d.click(0.0, 40.0);
    d.send(Event::Enter);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (0, 0)",
            "point (0, 40)",
            "line (0, 0)-(0, 40)",
            "horizontal_points (0, 0) origin",
            "vertical_points (0, 0) origin",
            "on (0, 40) y_axis",
        ])]
    );
}

/// `free`, at (10, 10), held level by `horizontal_points` with `held`, at (40, 10) and fully
/// defined, and q0000003 at (70, 10.5), fully defined: `free`'s Y class is fixed through
/// `held`, so `horizontal_points` of `free` and q0000003 reads two fixed Y classes and is
/// implied (Rule E). Its X class is its own, free, so `vertical_points` of the two isn't.
fn fixed_through(free: &str, held: &str) {
    let mut s = empty();
    s.points.push(point(free, 10.0, 10.0));
    s.points.push(point(held, 40.0, 10.0));
    s.points.push(point("q0000003", 70.0, 10.5));
    let s = constrained(s, vec![level(free, held)]);
    let states = fixing(&[held, "q0000003"]);
    assert!(implied_by(&s, &states, &level(free, "q0000003")));
    assert!(!implied_by(&s, &states, &plumb(free, "q0000003")));
}

#[test]
fn a_point_held_level_with_a_fixed_point_of_a_later_id_is_fixed_through_it() {
    // The free point's ID first: their class's root is the free point's.
    fixed_through("q0000001", "q0000002");
}

#[test]
fn a_point_held_level_with_a_fixed_point_of_an_earlier_id_is_fixed_through_it() {
    // The fixed point's ID first: their class's root is the fixed point's.
    fixed_through("q0000002", "q0000001");
}

#[test]
fn a_point_on_a_line_is_level_with_it_whatever_order_its_relations_come_in() {
    // M (10, 20)-(50, 20) `horizontal`; q0000001 (20, 20) `on` M; q0000002 (70, 20) level
    // with M's first end by `horizontal_points`: one Y class, so `horizontal_points` of the
    // two points is implied (Rule E), in each of the six orders the three can come in. An
    // `on` read before M is level joins once M is.
    let mut base = with_line(empty(), "m0000001", [10.0, 20.0], [50.0, 20.0]);
    base.points.push(point("q0000001", 20.0, 20.0));
    base.points.push(point("q0000002", 70.0, 20.0));
    let forms = [
        Form::Horizontal {
            line: el("m0000001"),
        },
        on("q0000001", el("m0000001")),
        level("q0000002", "f0000001"),
    ];
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let s = constrained(base.clone(), order.map(|i| forms[i].clone()).to_vec());
        assert!(
            implied_by(&s, &fixing(&[]), &level("q0000001", "q0000002")),
            "{order:?}"
        );
    }
}

#[test]
fn a_point_on_a_line_whose_end_is_on_a_level_line_is_level_with_it_once_that_line_is() {
    // N (10, 20)-(50, 20), made `horizontal` last; L (90, 20)-(70, 20), its first end level
    // with N's by `horizontal_points`, its second `on` N; q0000001 (80, 20) `on` L, before
    // all. Once N is level, L's second end joins N's class through N, which makes L level,
    // which puts q0000001 in the class too: a second round, after the first's join. So
    // `horizontal_points` of q0000001 and N's first end is implied.
    let mut s = with_line(empty(), "n0000001", [10.0, 20.0], [50.0, 20.0]);
    s = with_line(s, "n0000002", [90.0, 20.0], [70.0, 20.0]);
    s.points.push(point("q0000001", 80.0, 20.0));
    let s = constrained(
        s,
        vec![
            on("q0000001", el("n0000002")),
            on("t0000002", el("n0000001")),
            level("f0000002", "f0000001"),
            Form::Horizontal {
                line: el("n0000001"),
            },
        ],
    );
    assert!(implied_by(
        &s,
        &fixing(&[]),
        &level("q0000001", "f0000001")
    ));
}

/// A `distance_y` (`y` true) or `distance_x` of `nm` nanometres between q0000001 (10, 10)
/// and q0000002 (40, 10), driving or not, as their one constraint.
fn distance(y: bool, nm: i64, driving: bool) -> Sketch {
    let mut s = empty();
    s.points.push(point("q0000001", 10.0, 10.0));
    s.points.push(point("q0000002", 40.0, 10.0));
    let (a, b, value) = (el("q0000001"), el("q0000002"), crate::document::Length::from_nm(nm));
    let form = if y {
        Form::DistanceY {
            a,
            b,
            value,
            driving,
        }
    } else {
        Form::DistanceX {
            a,
            b,
            value,
            driving,
        }
    };
    constrained(s, vec![form])
}

#[test]
fn a_driving_distance_of_zero_holds_two_points_level_and_no_other_does() {
    // A driving `distance_y` of exactly 0 asserts y1 = y2: the two points in one Y class, so
    // `horizontal_points` of them is implied (Rule E), and nothing of X is. One of 1 nm
    // asserts they differ; a reference one of 0 asserts nothing: two classes either way.
    let states = fixing(&[]);
    let (a, b) = ("q0000001", "q0000002");
    assert!(implied_by(&distance(true, 0, true), &states, &level(a, b)));
    assert!(!implied_by(&distance(true, 0, true), &states, &plumb(a, b)));
    assert!(!implied_by(&distance(true, 1, true), &states, &level(a, b)));
    assert!(!implied_by(&distance(true, 0, false), &states, &level(a, b)));
}

#[test]
fn a_driving_x_distance_of_zero_holds_two_points_plumb_and_no_other_does() {
    // The x twin: a driving `distance_x` of exactly 0 asserts x1 = x2, one X class.
    let states = fixing(&[]);
    let (a, b) = ("q0000001", "q0000002");
    assert!(implied_by(&distance(false, 0, true), &states, &plumb(a, b)));
    assert!(!implied_by(&distance(false, 0, true), &states, &level(a, b)));
    assert!(!implied_by(&distance(false, 1, true), &states, &plumb(a, b)));
    assert!(!implied_by(&distance(false, 0, false), &states, &plumb(a, b)));
}
