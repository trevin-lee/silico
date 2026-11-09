//! The relations a sketch already implies, which a tool leaves out and the pointer never
//! shows (#278's G as amended, Fable, 2026-11-08, g5-redundant-autoconstraints-fable.md;
//! `sketch_tools::implied`). Every expected value is worked by hand from its two rules: an
//! inferred relation asserting two points level (or plumb) goes when they're already in one
//! class, or both their classes are fixed (Rule E); an inferred `perpendicular`, `parallel`,
//! or `tangent_at` goes when every point it reads is fixed both ways (Rule F). A class is
//! fixed when it holds the origin or a point the read marks fully defined.

use super::*;
use crate::sketch_states::MemberState;
use crate::sketch_tools::implied::Partition;
use crate::sketch_tools::implied_by;

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

/// The L drawn from the origin (docs/ports/sketch-tools.md), by six clicks and a seventh
/// back on its first point: (0, 0), (40, 0), (40, 10), (10, 10), (10, 30), (0, 30), (0, 0).
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
fn a_chain_along_the_x_axis_on_a_read_that_never_catches_up_has_no_direction_at_all() {
    // The chain above, (0, 0), (10, 0), (20, 0), (30, 0), each click's add left pending on a
    // read that stays empty: the classes are of the view, the read with the pending adds on
    // it, so each segment's start, a pending point `on` the x axis, is in the origin's Y class
    // before its `horizontal` is judged, as on a read that catches up. Three segments, each
    // with its end `on` the axis and no `horizontal` or `vertical` (the review's T10).
    let sketch = empty();
    let mut tool = SketchTool::new(ToolKind::Line);
    let out = clicks_on_a_stale_read(
        &mut tool,
        &sketch,
        &[[0.0, 0.0], [10.0, 0.0], [20.0, 0.0], [30.0, 0.0]],
    );
    let adds: Vec<&Members> = out.iter().filter_map(|o| o.add.as_ref()).collect();
    assert_eq!(adds.len(), 3, "{out:?}");
    for (k, add) in adds.iter().enumerate() {
        let directions: Vec<&Form> = add
            .constraints
            .iter()
            .map(|c| &c.form)
            .filter(|form| matches!(form, Form::Horizontal { .. } | Form::Vertical { .. }))
            .collect();
        assert!(directions.is_empty(), "segment {k}: {directions:?}");
        let on_the_axis = add
            .constraints
            .iter()
            .filter(|c| {
                matches!(
                    c.form,
                    Form::On {
                        curve: Reference::XAxis,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(on_the_axis, 1, "segment {k}: {:?}", add.constraints);
    }
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
fn a_line_from_a_point_named_as_the_tools_first_unnamed_one_judges_it_as_the_sketchs_point() {
    // The test above with its point on the axis named zz000000, the first ID the tools give a
    // gesture's point not yet named: the IDs they give are none the view holds, so the
    // sketch's zz000000 is judged as itself, in the origin's fixed Y class, and the
    // `horizontal` goes (the review's T13).
    let mut s = empty();
    s.points.push(point("zz000000", 10.0, 0.0));
    s.points.push(point("q0000002", 40.0, 0.0));
    let s = constrained(s, vec![on("zz000000", Reference::XAxis)]);
    let mut d = Drive::new(ToolKind::Line, s);
    fixed(&mut d, &["q0000002"]);
    d.drag([10.0, 0.0], [40.0, 0.0]);
    assert_eq!(d.adds(), [sorted(&["line #(10, 0)-#(40, 0)"])]);
    // And a line from a new point, free, at (10, 10) to q0000002, now at (40, 10), with the
    // sketch's zz000000, fully defined, at (70, 10): the new point is judged as a point of
    // its own, not as zz000000, so its Y class is free and the `horizontal` stays.
    let mut s = empty();
    s.points.push(point("zz000000", 70.0, 10.0));
    s.points.push(point("q0000002", 40.0, 10.0));
    let mut d = Drive::new(ToolKind::Line, s);
    fixed(&mut d, &["zz000000", "q0000002"]);
    d.drag([10.0, 10.0], [40.0, 10.0]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (10, 10)",
            "line (10, 10)-#(40, 10)",
            "horizontal line (10, 10)-#(40, 10)",
        ])]
    );
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
    for (k, form) in [
        Form::Horizontal {
            line: el("m0000001"),
        },
        Form::On {
            point: el("q0000001"),
            curve: el("m0000001"),
        },
        Form::HorizontalPoints {
            a: el("q0000002"),
            b: el("f0000001"),
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
    // `covers`' regression, not the classes': the Point tool at (30, 0.1), the middle of
    // (10, 0)-(50, 0), which lies along the x axis: `midpoint`, and no `on` of the axis,
    // which the line covers there, so the snap never offers it to be judged. At the middle
    // of (30, -10)-(30, 10), which the x axis crosses there, (30, 0): both, `midpoint` and
    // `on` the axis, each joining classes.
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
        let mut forms = vec![
            Form::Horizontal {
                line: el("m0000001"),
            },
            Form::Horizontal {
                line: el("n0000002"),
            },
        ];
        if joined {
            forms.push(Form::HorizontalPoints {
                a: el("f0000002"),
                b: el("f0000001"),
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
fn a_point_at_the_middle_of_a_plumb_line_is_on_a_line_plumb_with_it_only_once() {
    // The x twin of the test above: M (20, 10)-(20, 50) and N (20, 0)-(20, 60), both
    // `vertical`, and N's first end plumb with M's by `vertical_points`: one X class. The
    // Point tool at M's middle, (20, 30), which N runs through exactly: `midpoint` of M, and
    // no `on` of N (the middle is plumb with N's ends already, through M). Without the
    // `vertical_points`, M and N are two classes, and the `on` of N joins them: kept (the
    // review's T11).
    for joined in [true, false] {
        let mut s = with_line(empty(), "m0000001", [20.0, 10.0], [20.0, 50.0]);
        s = with_line(s, "n0000002", [20.0, 0.0], [20.0, 60.0]);
        let mut forms = vec![
            Form::Vertical {
                line: el("m0000001"),
            },
            Form::Vertical {
                line: el("n0000002"),
            },
        ];
        if joined {
            forms.push(plumb("f0000002", "f0000001"));
        }
        let mut d = Drive::new(ToolKind::Point, constrained(s, forms));
        d.click(20.1, 30.0);
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
fn a_perpendicular_to_a_line_not_fixed_whole_stays_however_fixed_the_new_line_is() {
    // The test above's K (10, 10)-(30, 20) and the line from its end (30, 20) to (20, 40),
    // fully defined, as is K's end there; K's first end (10, 10) free. The new line's every
    // point is fixed, but K's direction isn't: `perpendicular` reads a value nothing holds,
    // so it stays (Rule F reads both lines' points; the review's T12).
    let mut s = with_line(empty(), "n0000001", [10.0, 10.0], [30.0, 20.0]);
    s.points.push(point("q0000001", 20.0, 40.0));
    let mut d = Drive::new(ToolKind::Line, s);
    fixed(&mut d, &["t0000001", "q0000001"]);
    d.drag([30.0, 20.0], [20.0, 40.0]);
    assert_eq!(d.made.len(), 1);
    assert_eq!(types(&d), ["perpendicular"]);
}

#[test]
fn a_tangent_to_an_arc_whose_centre_isnt_fixed_stays_however_fixed_the_rest_is() {
    // The test above's arc about (50, 50), radius 10, from (60, 50) to 45°, and the line from
    // its end along 135°, 20 long, to a point fully defined, as are both the arc's ends; the
    // arc's centre free. The line's every point is fixed, and the point they meet at, but the
    // arc's centre isn't: `tangent_at` reads a value nothing holds, so it stays (Rule F reads
    // both curves' points; the review's T12).
    let h = 5.0 * std::f64::consts::SQRT_2;
    let end = [50.0 + h, 50.0 + h];
    let far = [end[0] - 2.0 * h, end[1] + 2.0 * h];
    let mut s = empty();
    s.points.push(point("c0000001", 50.0, 50.0));
    s.points.push(point("q0000101", 60.0, 50.0));
    s.points.push(point("q0000102", end[0], end[1]));
    s.points.push(point("q0000001", far[0], far[1]));
    s.arcs.push(crate::document::SketchArc {
        id: id("a0000001"),
        center: id("c0000001"),
        start: id("q0000101"),
        end: id("q0000102"),
        construction: false,
    });
    let mut d = Drive::new(ToolKind::Line, s);
    fixed(&mut d, &["q0000101", "q0000102", "q0000001"]);
    d.drag(end, far);
    assert_eq!(d.made.len(), 1);
    assert_eq!(types(&d), ["tangent_at"]);
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

// ---- #431's review, round 1: what the classes read, and how much work they take ----

/// A read's states marking the points `ids` fully defined, and nothing else.
fn fixing(ids: &[&str]) -> SketchStates {
    let mut states = no_states().clone();
    for p in ids {
        states.points.insert(id(p), MemberState::FullyDefined);
    }
    states
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
fn implied_by_keeps_a_midpoint_only_partly_implied_as_the_tools_do() {
    // N (10, 0)-(50, 0) `on` the x axis at both ends, and (30, 0) `on` it too: all three in
    // the origin's Y class. A `midpoint` of (30, 0) and N asserts its y, which the class
    // holds, and its x, which nothing holds: only partly implied, so kept, as the Point tool
    // keeps its `midpoint` at a line's middle (`a_point_at_the_middle_of_a_line_on_the_axis_is
    // _its_midpoint_alone`), never left out.
    let mut s = with_line(empty(), "n0000001", [10.0, 0.0], [50.0, 0.0]);
    s.points.push(point("q0000001", 30.0, 0.0));
    let s = constrained(
        s,
        vec![
            on("f0000001", Reference::XAxis),
            on("t0000001", Reference::XAxis),
            on("q0000001", Reference::XAxis),
        ],
    );
    let midpoint = Form::Midpoint {
        point: el("q0000001"),
        line: el("n0000001"),
    };
    assert!(!implied_by(&s, &fixing(&[]), &midpoint));
    // Its y alone, as `horizontal_points` with an end, is implied: the class holds it.
    assert!(implied_by(&s, &fixing(&[]), &level("q0000001", "f0000001")));
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
/// If `grown`, `held` is held level with q0000004 at (100, 10), free, first: `held`'s class
/// is the larger when `free` joins it.
fn fixed_through(free: &str, held: &str, grown: bool) {
    let mut s = empty();
    s.points.push(point(free, 10.0, 10.0));
    s.points.push(point(held, 40.0, 10.0));
    s.points.push(point("q0000003", 70.0, 10.5));
    s.points.push(point("q0000004", 100.0, 10.0));
    let mut forms = Vec::new();
    if grown {
        forms.push(level(held, "q0000004"));
    }
    forms.push(level(free, held));
    let s = constrained(s, forms);
    let states = fixing(&[held, "q0000003"]);
    assert!(implied_by(&s, &states, &level(free, "q0000003")));
    assert!(!implied_by(&s, &states, &plumb(free, "q0000003")));
}

#[test]
fn a_point_held_level_with_a_fixed_point_is_fixed_through_it() {
    // Two classes of one point each: the tie goes to the class of the point the relation
    // names first, the free one, so the fixed point's class is the smaller, hung under it.
    fixed_through("q0000001", "q0000002", false);
}

#[test]
fn a_point_held_level_with_a_larger_fixed_class_is_fixed_through_it() {
    // The fixed point's class grown to two first: the free point's class is the smaller,
    // hung under the fixed one, which stays fixed (the review's T9).
    fixed_through("q0000001", "q0000002", true);
}

#[test]
fn a_fixed_class_stays_fixed_when_a_free_point_joins_it_as_the_smaller() {
    // q0000001 and q0000002 fully defined and held level: one fixed Y class of two. q0000003
    // held level with q0000001: a free class of one, joined to it as the smaller, so the
    // class stays fixed. q0000005 and q0000006 each `on` the x axis: q0000005 joins the
    // origin's fixed class (a tie, its own class taking the origin's under it), and q0000006,
    // free, joins that class of two as the smaller: it stays fixed too. q0000004, fully
    // defined, is a fixed class of its own. So `horizontal_points` of q0000003 or q0000006
    // with q0000004 reads two fixed Y classes and is implied (Rule E); q0000003's X class is
    // its own, free, so `vertical_points` of it and q0000004 isn't (the review's T9).
    let mut s = empty();
    for (name, x, y) in [
        ("q0000001", 10.0, 10.0),
        ("q0000002", 40.0, 10.0),
        ("q0000003", 70.0, 10.0),
        ("q0000004", 100.0, 10.0),
        ("q0000005", 10.0, 0.0),
        ("q0000006", 20.0, 0.0),
    ] {
        s.points.push(point(name, x, y));
    }
    let s = constrained(
        s,
        vec![
            level("q0000001", "q0000002"),
            level("q0000003", "q0000001"),
            on("q0000005", Reference::XAxis),
            on("q0000006", Reference::XAxis),
        ],
    );
    let states = fixing(&["q0000001", "q0000002", "q0000004"]);
    assert!(implied_by(&s, &states, &level("q0000003", "q0000004")));
    assert!(implied_by(&s, &states, &level("q0000006", "q0000004")));
    assert!(!implied_by(&s, &states, &plumb("q0000003", "q0000004")));
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
fn a_point_on_a_plumb_line_is_plumb_with_it_whatever_order_its_relations_come_in() {
    // The x twin of the test above: M (20, 10)-(20, 50) `vertical`; q0000001 (20, 20) `on`
    // M; q0000002 (20, 70) plumb with M's first end by `vertical_points`: one X class, so
    // `vertical_points` of the two points is implied (Rule E), and so is an `on` of M for
    // q0000002 (plumb with M's ends, M plumb), in each of the six orders the three can come
    // in. Their y is held by nothing: `horizontal_points` of the two isn't (the review's
    // T11).
    let mut base = with_line(empty(), "m0000001", [20.0, 10.0], [20.0, 50.0]);
    base.points.push(point("q0000001", 20.0, 20.0));
    base.points.push(point("q0000002", 20.0, 70.0));
    let forms = [
        Form::Vertical {
            line: el("m0000001"),
        },
        on("q0000001", el("m0000001")),
        plumb("q0000002", "f0000001"),
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
        let states = fixing(&[]);
        assert!(
            implied_by(&s, &states, &plumb("q0000001", "q0000002")),
            "{order:?}"
        );
        assert!(
            implied_by(&s, &states, &on("q0000002", el("m0000001"))),
            "{order:?}"
        );
        assert!(
            !implied_by(&s, &states, &level("q0000001", "q0000002")),
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
    assert!(implied_by(&s, &fixing(&[]), &level("q0000001", "f0000001")));
}

/// A `distance_y` (`partition` Y, the y the two would share) or `distance_x` (X) of `nm`
/// nanometres between q0000001 (10, 10) and q0000002 (40, 10), driving or not, as their one
/// constraint.
fn distance(partition: Partition, nm: i64, driving: bool) -> Sketch {
    let mut s = empty();
    s.points.push(point("q0000001", 10.0, 10.0));
    s.points.push(point("q0000002", 40.0, 10.0));
    let (a, b, value) = (
        el("q0000001"),
        el("q0000002"),
        crate::document::Length::from_nm(nm),
    );
    let form = if partition == Partition::Y {
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
    assert!(implied_by(
        &distance(Partition::Y, 0, true),
        &states,
        &level(a, b)
    ));
    assert!(!implied_by(
        &distance(Partition::Y, 0, true),
        &states,
        &plumb(a, b)
    ));
    assert!(!implied_by(
        &distance(Partition::Y, 1, true),
        &states,
        &level(a, b)
    ));
    assert!(!implied_by(
        &distance(Partition::Y, 0, false),
        &states,
        &level(a, b)
    ));
}

#[test]
fn a_driving_x_distance_of_zero_holds_two_points_plumb_and_no_other_does() {
    // The x twin: a driving `distance_x` of exactly 0 asserts x1 = x2, one X class.
    let states = fixing(&[]);
    let (a, b) = ("q0000001", "q0000002");
    assert!(implied_by(
        &distance(Partition::X, 0, true),
        &states,
        &plumb(a, b)
    ));
    assert!(!implied_by(
        &distance(Partition::X, 0, true),
        &states,
        &level(a, b)
    ));
    assert!(!implied_by(
        &distance(Partition::X, 1, true),
        &states,
        &plumb(a, b)
    ));
    assert!(!implied_by(
        &distance(Partition::X, 0, false),
        &states,
        &plumb(a, b)
    ));
}

#[test]
fn a_tangent_at_between_two_arcs_is_not_implied_by_their_centres_alone() {
    // Two arcs about (0, 0) and (20, 0), of radius 10, meeting at (10, 0): a `tangent_at` of
    // the two there reads the point they meet at as well as their centres. With both centres
    // fully defined and the point free, it asserts something no class holds, so it isn't
    // implied (Rule F reads every point a relation reads); with the point fixed too, it is.
    let mut s = empty();
    for (name, x, y) in [
        ("c0000001", 0.0, 0.0),
        ("c0000002", 20.0, 0.0),
        ("j0000001", 10.0, 0.0),
        ("e0000001", 0.0, 10.0),
        ("e0000002", 30.0, 0.0),
    ] {
        s.points.push(point(name, x, y));
    }
    for (name, center, start, end) in [
        ("a0000001", "c0000001", "j0000001", "e0000001"),
        ("a0000002", "c0000002", "e0000002", "j0000001"),
    ] {
        s.arcs.push(crate::document::SketchArc {
            id: id(name),
            center: id(center),
            start: id(start),
            end: id(end),
            construction: false,
        });
    }
    let tangent = Form::TangentAt {
        a: el("a0000001"),
        b: el("a0000002"),
        at: el("j0000001"),
        direction: crate::document::Direction::Opposite,
    };
    assert!(!implied_by(
        &s,
        &fixing(&["c0000001", "c0000002"]),
        &tangent
    ));
    // The point fixed and either centre free: it still reads a value nothing holds, so it
    // isn't implied, whichever of the two arcs is the free one (the review's T12).
    for fixed in [["c0000002", "j0000001"], ["c0000001", "j0000001"]] {
        assert!(!implied_by(&s, &fixing(&fixed), &tangent), "{fixed:?}");
    }
    assert!(implied_by(
        &s,
        &fixing(&["c0000001", "c0000002", "j0000001"]),
        &tangent
    ));
}

/// A grid of 20 rows and 20 columns, each a line from end to end, rows `horizontal` and
/// columns `vertical`, and 400 points, each `on` its row and its column: 480 points and 840
/// constraints, the 40 directions first.
fn grid() -> Sketch {
    let mut s = empty();
    let mut forms = Vec::new();
    for k in 0..20u32 {
        let at = 10.0 * f64::from(k + 1);
        s = with_line(s, &format!("r{k:07}"), [0.0, at], [210.0, at]);
        s = with_line(s, &format!("c{k:07}"), [at, 0.0], [at, 210.0]);
        forms.push(Form::Horizontal {
            line: el(&format!("r{k:07}")),
        });
        forms.push(Form::Vertical {
            line: el(&format!("c{k:07}")),
        });
    }
    for row in 0..20u32 {
        for column in 0..20u32 {
            let name = format!("g{:07}", row * 20 + column);
            s.points.push(point(
                &name,
                10.0 * f64::from(column + 1),
                10.0 * f64::from(row + 1),
            ));
            forms.push(on(&name, el(&format!("r{row:07}"))));
            forms.push(on(&name, el(&format!("c{column:07}"))));
        }
    }
    let s = constrained(s, forms);
    assert_eq!((s.points.len(), s.constraints.len()), (480, 840));
    s
}

#[test]
fn a_line_previewed_over_a_constrained_grid_visits_each_point_on_a_line_a_few_times_a_move() {
    // The Line tool started at (300, 300), clear of the grid, and moved to (340, 300.5),
    // 0.7° off level: the preview shows `horizontal`, judged against the grid's classes,
    // built for this view on this move. Building them visits each point on a line a few
    // times at most, not once for each `on` before it: at most four times the grid's 840
    // constraints. Moved again, to (345, 300.5), on the same view: they're kept, and only
    // the gesture's own relation is joined into a copy, a few visits at most.
    let mut d = Drive::new(ToolKind::Line, grid());
    d.click(300.0, 300.0);
    let visits = || infer::work::VISITS.with(|c| c.get());
    infer::work::VISITS.with(|c| c.set(0));
    d.to(340.0, 300.5);
    let built = visits();
    assert!(shows(&d, &Relation::Horizontal));
    assert!(built > 0 && built <= 4 * 840, "{built} visits");
    d.to(345.0, 300.5);
    assert!(shows(&d, &Relation::Horizontal));
    assert!(visits() - built <= 4, "{} visits", visits() - built);
}

#[test]
fn a_perpendicular_to_an_end_fixed_one_way_only_stays() {
    // K from (10, 10) to (30, 20), both ends fully defined; from K's end (30, 20) a line to
    // q0000001 at (20, 40), at right angles to K: `perpendicular` to K shows. q0000001 is
    // fixed one way only, its x by `vertical_points` with q0000002 at (20, 5), fully
    // defined, or its y by `horizontal_points` with q0000002 at (50, 40): not every point
    // the relation reads is fixed both ways, so it stays (Rule F).
    for (q, form) in [
        ([20.0, 5.0], plumb("q0000001", "q0000002")),
        ([50.0, 40.0], level("q0000001", "q0000002")),
    ] {
        let mut s = with_line(empty(), "n0000001", [10.0, 10.0], [30.0, 20.0]);
        s.points.push(point("q0000001", 20.0, 40.0));
        s.points.push(point("q0000002", q[0], q[1]));
        let s = constrained(s, vec![form]);
        let mut d = Drive::new(ToolKind::Line, s);
        fixed(&mut d, &["f0000001", "t0000001", "q0000002"]);
        d.drag([30.0, 20.0], [20.0, 40.0]);
        assert_eq!(d.made.len(), 1, "{q:?}");
        assert_eq!(types(&d), ["perpendicular"], "{q:?}");
    }
}

#[test]
fn a_line_to_an_over_defined_point_keeps_its_direction() {
    // (10, 10), fully defined, and (40, 10.785), which the read marks over defined: 1.499°
    // off level, so the pointer finds `horizontal`. An over defined point isn't fixed (its
    // values are the solver's last try, not held), so its class isn't, and the `horizontal`
    // stays, as the port note has it (`over_defined` doesn't count).
    let mut s = empty();
    s.points.push(point("q0000001", 10.0, 10.0));
    s.points.push(point("q0000002", 40.0, 10.785));
    let mut d = Drive::new(ToolKind::Line, s);
    fixed(&mut d, &["q0000001"]);
    d.states
        .points
        .insert(id("q0000002"), MemberState::OverDefined);
    d.drag([10.0, 10.0], [40.0, 10.785]);
    assert_eq!(types(&d), ["horizontal"]);
}

#[test]
fn a_line_from_the_origin_to_a_fixed_point_off_level_keeps_its_starts_relations() {
    // From the origin to q0000001 at (40, 1), fully defined: 1.43° off level, so the
    // pointer finds `horizontal`. The start's Y class is the origin's, fixed, and so is the
    // end's: the `horizontal` goes (added, it would conflict). FreeCAD's solver filter drops
    // every auto-constraint of a conflicting group, the start's two with it; Silico keeps
    // them, each sound on its own (docs/ports/sketch-tools.md, *Only the direction goes in
    // a conflict*).
    let mut s = empty();
    s.points.push(point("q0000001", 40.0, 1.0));
    let mut d = Drive::new(ToolKind::Line, s);
    fixed(&mut d, &["q0000001"]);
    d.click(0.0, 0.0);
    d.click(40.0, 1.0);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (0, 0)",
            "line (0, 0)-#(40, 1)",
            "horizontal_points (0, 0) origin",
            "vertical_points (0, 0) origin",
        ])]
    );
}

#[test]
fn a_newer_read_of_the_same_sketch_with_other_states_is_judged_by_its_states() {
    // #431 round 3, T14. q0000001 (10, 10) and q0000002 (40, 10.785), both free: a line from
    // the first to the second is 1.5° off level, so the pointer finds `horizontal`, their Y
    // classes free and apart: shown. Then a newer read, as the app gives the tool each one:
    // a revision on, its sketch the same (as the read of an add the view already held is,
    // where the solve moved nothing: a dimension placed at its measured value, say), its
    // states marking both points fully defined. The classes are built again for the new
    // states (`ViewClasses::keep_for` keys them by the view and the states), so both Y
    // classes are fixed and the `horizontal` goes (Rule E: added, it would conflict): not
    // shown at the same place, and not added.
    let mut s = empty();
    s.points.push(point("q0000001", 10.0, 10.0));
    s.points.push(point("q0000002", 40.0, 10.785));
    let mut d = Drive::new(ToolKind::Line, s);
    d.to(10.0, 10.0).press(10.0, 10.0).to(40.0, 10.785);
    assert!(shows(&d, &Relation::Horizontal));
    fixed(&mut d, &["q0000001", "q0000002"]);
    d.revision = d.revision.next().unwrap();
    d.to(40.0, 10.785);
    assert!(!shows(&d, &Relation::Horizontal));
    d.release(40.0, 10.785);
    assert_eq!(d.made.len(), 1);
    assert_eq!(types(&d), Vec::<&str>::new());
}

#[test]
fn a_line_beside_a_level_line_named_as_the_tools_unnamed_line_keeps_its_horizontal() {
    // #431 round 3, T15. A level line named zz000002, (10, 50) to (50, 50), its points
    // fz000002 and tz000002: the tools' first three unnamed IDs are zz000000 and zz000001
    // for a gesture's points and, zz000002 being a curve of the view, zz000003 for its
    // line. A new line from (10, 10) to (40, 10.785), both ends free: its `horizontal` is
    // judged on its own ends, free and apart, so it stays. Named zz000002, the gesture's
    // line would be taken for the level line, whose ends are level already, and the
    // `horizontal` would go.
    let s = with_line(empty(), "zz000002", [10.0, 50.0], [50.0, 50.0]);
    let s = constrained(
        s,
        vec![Form::Horizontal {
            line: el("zz000002"),
        }],
    );
    let mut d = Drive::new(ToolKind::Line, s);
    d.drag([10.0, 10.0], [40.0, 10.785]);
    assert_eq!(types(&d), ["horizontal"], "{:?}", d.adds());
}

/// T16's sketch: n0000001 (0, 20)-(110, 20) and n0000002 (40, 20)-(80, 20), each held
/// `horizontal`, so each one's ends are a Y class of their own; and n0000003 (50, 10)-(50,
/// 30), held by nothing, whose middle, (50, 20), both level lines run through. None fixed.
fn two_level_lines_and_a_cross() -> Sketch {
    let mut s = with_line(empty(), "n0000001", [0.0, 20.0], [110.0, 20.0]);
    s = with_line(s, "n0000002", [40.0, 20.0], [80.0, 20.0]);
    s = with_line(s, "n0000003", [50.0, 10.0], [50.0, 30.0]);
    constrained(
        s,
        vec![
            Form::Horizontal {
                line: el("n0000001"),
            },
            Form::Horizontal {
                line: el("n0000002"),
            },
        ],
    )
}

#[test]
fn a_lines_end_at_a_middle_is_judged_after_its_new_starts_relations() {
    // #431 round 3, T16 (docs/ports/sketch-tools.md: the start point's relations, then the
    // end point's, each kept merged before the next). Three lines, none fixed: n0000001
    // (0, 20)-(110, 20) and n0000002 (40, 20)-(80, 20), each held `horizontal`, so each
    // one's ends are a Y class of their own; n0000003 (50, 10)-(50, 30), held by nothing. A
    // line dragged from (50, 20) to (60, 20), within the pick tolerance of no point:
    // - its start snaps to n0000003's middle, (50, 20), through which n0000001 and n0000002
    //   both run: `midpoint` of n0000003 (whose ends share no class, so it joins none),
    //   `on` n0000001 (the new point's own class joins n0000001's: kept), and `on` n0000002
    //   (n0000001's class and n0000002's apart: kept, and now one);
    // - its end snaps to n0000002's middle, (60, 20), through which n0000001 runs:
    //   `midpoint` of n0000002 puts it in n0000002's class, which the start's `on`s have
    //   joined to n0000001's, so its `on` n0000001 is implied (Rule E) and goes. Judged on
    //   the view's classes alone, without the start's, the two were apart and it stayed:
    //   three `on`s.
    // The segment's `horizontal` goes too, its ends in one class. Two `on`s, the start's.
    let mut d = Drive::new(ToolKind::Line, two_level_lines_and_a_cross());
    d.drag([50.0, 20.0], [60.0, 20.0]);
    let ons = d.made[0]
        .constraints
        .iter()
        .filter(|c| matches!(c.form, Form::On { .. }))
        .count();
    assert_eq!(ons, 2, "{:?}", d.adds());
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (50, 20)",
            "point (60, 20)",
            "line (50, 20)-(60, 20)",
            "midpoint (50, 20) line #(50, 10)-#(50, 30)",
            "on (50, 20) line #(0, 20)-#(110, 20)",
            "on (50, 20) line #(40, 20)-#(80, 20)",
            "midpoint (60, 20) line #(40, 20)-#(80, 20)",
        ])]
    );
}

#[test]
fn a_line_from_the_origin_to_a_middle_on_the_x_axis_keeps_its_ends_on() {
    // #431 round 4, T17: the keep path of a new start's relations. n0000001 (30, -10)-(30,
    // 10), held by nothing. A line dragged from the origin to (30, 0):
    // - its start, a new point at the origin, level and plumb with it, so in the origin's
    //   classes, fixed;
    // - its end, n0000001's middle, through which the x axis runs: `midpoint` of a line
    //   whose ends share no class, so it joins none; its `on` of the x axis would join its
    //   own Y class, free, to the origin's: not implied, kept, with the start's relations
    //   merged first as without. Judged as the start itself (its unnamed ID), it would be in
    //   the origin's class already, and the `on` would go;
    // - the segment's `horizontal`: its ends both in the origin's Y class: goes.
    let s = with_line(empty(), "n0000001", [30.0, -10.0], [30.0, 10.0]);
    let mut d = Drive::new(ToolKind::Line, s);
    d.drag([0.0, 0.0], [30.0, 0.0]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (0, 0)",
            "point (30, 0)",
            "line (0, 0)-(30, 0)",
            "horizontal_points (0, 0) origin",
            "vertical_points (0, 0) origin",
            "midpoint (30, 0) line #(30, -10)-#(30, 10)",
            "on (30, 0) x_axis",
        ])]
    );
}

#[test]
fn a_lines_end_at_a_middle_is_judged_after_its_starts_midpoint_as_well_as_its_ons() {
    // #431 round 4, T19. n0000001 (0, 20)-(100, 20) and n0000002 (40, 20)-(80, 20), each held
    // `horizontal`, none fixed. A line dragged from (50, 20) to (60, 20):
    // - its start, n0000001's middle, through which n0000002 runs: `midpoint` of n0000001
    //   puts it in n0000001's Y class; its `on` n0000002 joins that to n0000002's: kept;
    // - its end, n0000002's middle, through which n0000001 runs: `midpoint` of n0000002 puts
    //   it in n0000002's class, now one with n0000001's, so its `on` n0000001 is implied and
    //   goes. With the start's `on` merged but not its `midpoint`, n0000001's class would
    //   still be apart, and it would stay;
    // - the segment's `horizontal`, its ends in one class: goes. One `on`, the start's.
    let s = with_line(empty(), "n0000001", [0.0, 20.0], [100.0, 20.0]);
    let s = with_line(s, "n0000002", [40.0, 20.0], [80.0, 20.0]);
    let s = constrained(
        s,
        vec![
            Form::Horizontal {
                line: el("n0000001"),
            },
            Form::Horizontal {
                line: el("n0000002"),
            },
        ],
    );
    let mut d = Drive::new(ToolKind::Line, s);
    d.drag([50.0, 20.0], [60.0, 20.0]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (50, 20)",
            "point (60, 20)",
            "line (50, 20)-(60, 20)",
            "midpoint (50, 20) line #(0, 20)-#(100, 20)",
            "on (50, 20) line #(40, 20)-#(80, 20)",
            "midpoint (60, 20) line #(40, 20)-#(80, 20)",
        ])]
    );
}
