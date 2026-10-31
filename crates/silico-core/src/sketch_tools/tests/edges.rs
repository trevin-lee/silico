//! The tools at their edges: which candidate the pointer takes when several are in reach,
//! the angles and distances at the tolerances' edges, drags against clicks, and what a tool
//! does when the sketch changes under it. Expected values worked by hand in each comment.

use super::*;
use crate::sketch_tools::geometry::distance;
use crate::sketch_tools::infer;

/// The arc about `centre`, radius `r`, counter-clockwise from `from`° to `to`°: centre
/// `c0000001`, start `q0000101`, end `q0000102`, the arc `a0000001`.
fn with_arc(mut s: Sketch, centre: Xy, r: f64, from: f64, to: f64) -> Sketch {
    let at = |deg: f64| {
        let a = deg.to_radians();
        [centre[0] + r * libm::cos(a), centre[1] + r * libm::sin(a)]
    };
    let (start, end) = (at(from), at(to));
    s.points.push(point("c0000001", centre[0], centre[1]));
    s.points.push(point("q0000101", start[0], start[1]));
    s.points.push(point("q0000102", end[0], end[1]));
    s.arcs.push(crate::document::SketchArc {
        id: id("a0000001"),
        center: id("c0000001"),
        start: id("q0000101"),
        end: id("q0000102"),
        construction: false,
    });
    s
}

/// The point `length` from `from` at `deg`°.
fn toward(from: Xy, deg: f64, length: f64) -> Xy {
    let a = deg.to_radians();
    [
        from[0] + length * libm::cos(a),
        from[1] + length * libm::sin(a),
    ]
}

/// The forms of each constraint the tool's adds made, by type, sorted.
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

// --- Review item 1: a line leaving an arc along an axis --------------------------------

#[test]
fn a_line_leaving_an_arc_along_an_axis_is_tangent_and_horizontal() {
    // The arc about (50, 50), radius 10, from 270° (50, 40) to 90° (50, 60): its right half.
    // Past its end it runs along −x; back past its start, along −x too. A line from its end
    // to (20, 60.3), 0.57° off −x, shows both the tangent and horizontal (FreeCAD 1.1.3
    // adds both; #278's G), its end on the −x ray, (20, 60). The same from its start, to
    // (20, 39.8): the arc's start's direction is the radius turned a quarter right.
    let s = with_arc(empty(), [50.0, 50.0], 10.0, 270.0, 90.0);
    let mut d = Drive::new(ToolKind::Line, s.clone());
    d.drag([50.0, 60.0], [20.0, 60.3]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (20, 60)",
            "line #(50, 60)-(20, 60)",
            "horizontal line #(50, 60)-(20, 60)",
            "tangent_at arc #(50, 50) #(50, 40)->#(50, 60) line #(50, 60)-(20, 60) at #(50, 60) same",
        ])]
    );
    let mut d = Drive::new(ToolKind::Line, s);
    d.drag([50.0, 40.0], [20.0, 39.8]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (20, 40)",
            "line #(50, 40)-(20, 40)",
            "horizontal line #(50, 40)-(20, 40)",
            "tangent_at arc #(50, 50) #(50, 40)->#(50, 60) line #(50, 40)-(20, 40) at #(50, 40) same",
        ])]
    );
}

// --- Review item 2: only woken points line up --------------------------------------------

#[test]
fn a_point_far_off_at_the_same_x_lines_up_only_once_hovered() {
    // A point at (30, 10), 40 mm below the click at (30.3, 50): not woken, no relation and
    // no move. Hovered first (0.22 mm off it), it's woken: `vertical_points`.
    let mut s = empty();
    s.points.push(point("q0000001", 30.0, 10.0));
    let mut d = Drive::new(ToolKind::Point, s.clone());
    d.click(30.3, 50.0);
    assert_eq!(d.adds(), [sorted(&["point (30.3, 50)"])]);
    let mut d = Drive::new(ToolKind::Point, s);
    d.to(30.2, 10.1).click(30.3, 50.0);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (30, 50)",
            "vertical_points (30, 50) #(30, 10)"
        ])]
    );
}

#[test]
fn the_pointer_keeps_the_last_eight_points_woken_and_forgets_them_on_escape() {
    // Nine points along y = 10, at x = 10, 20, ... 90, hovered in turn: the first, x = 10, is
    // the ninth back, forgotten; the second, x = 20, still woken. A click at (10.3, 50) lines
    // up with nothing; one at (20.3, 60), with (20, 10).
    let mut s = empty();
    for i in 1..=9 {
        s.points
            .push(point(&format!("q000000{i}"), f64::from(i) * 10.0, 10.0));
    }
    let hovered = || {
        let mut d = Drive::new(ToolKind::Point, s.clone());
        for i in 1..=9 {
            d.to(f64::from(i) * 10.0 + 0.1, 10.1);
        }
        d
    };
    let mut d = hovered();
    d.click(10.3, 50.0);
    assert_eq!(d.adds(), [sorted(&["point (10.3, 50)"])]);
    let mut d = hovered();
    d.click(20.3, 60.0);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (20, 60)",
            "vertical_points (20, 60) #(20, 10)"
        ])]
    );
    // Esc forgets them all.
    let mut d = hovered();
    d.key(Event::Escape).click(20.3, 60.0);
    assert_eq!(d.adds(), [sorted(&["point (20.3, 60)"])]);
}

#[test]
fn the_points_a_tool_adds_are_woken() {
    // A chain from (10, 10): its fourth click, (10.3, 30.2), runs horizontal from (40, 30)
    // and so lines up, by a guide, with the chain's start, woken by the add that made it,
    // though the pointer never hovered it as a point: (10, 30).
    let mut d = Drive::new(ToolKind::Line, empty());
    d.click(10.0, 10.0)
        .click(40.0, 10.3)
        .click(40.2, 30.0)
        .click(10.3, 30.2);
    assert_eq!(
        d.adds()[2],
        sorted(&[
            "point (10, 30)",
            "line #(40, 30)-(10, 30)",
            "horizontal line #(40, 30)-(10, 30)",
        ])
    );
}

// --- Review item 3: a crossing at a middle -----------------------------------------------

#[test]
fn a_crossing_at_a_lines_middle_is_its_midpoint_and_on_the_other() {
    // (10, 30)-(50, 30), its middle (30, 30), where (30, 10)-(30, 40) crosses it: the middle
    // first, and the other line's `on` too (#278's G).
    let mut s = empty();
    s.points.push(point("q0000001", 10.0, 30.0));
    s.points.push(point("q0000002", 50.0, 30.0));
    s.points.push(point("q0000003", 30.0, 10.0));
    s.points.push(point("q0000004", 30.0, 40.0));
    s.lines.push(line("n0000001", "q0000001", "q0000002"));
    s.lines.push(line("n0000002", "q0000003", "q0000004"));
    let mut d = Drive::new(ToolKind::Point, s);
    d.click(30.1, 30.1);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (30, 30)",
            "midpoint (30, 30) line #(10, 30)-#(50, 30)",
            "on (30, 30) line #(30, 10)-#(30, 40)",
        ])]
    );
}

// --- Review items 4, 5, 6: the sketch changing under a tool ------------------------------

#[test]
fn a_chains_next_segment_starts_where_the_solve_put_its_last_point() {
    // The chain's last point, (40, 30), solved to (41, 31) by the next read: the segment
    // under way starts there.
    let mut d = Drive::new(ToolKind::Line, empty());
    d.click(10.0, 10.0).click(40.0, 30.0);
    let last = d.made[0].lines[0].to.clone();
    let p = d.sketch.points.iter_mut().find(|p| p.id == last).unwrap();
    p.x = Coordinate::new(41.0).unwrap();
    p.y = Coordinate::new(31.0).unwrap();
    d.to(70.0, 80.0);
    assert_eq!(d.tool.preview().lines, [[[41.0, 31.0], [70.0, 80.0]]]);
}

#[test]
fn an_undo_mid_chain_ends_the_chain_and_the_next_click_starts_afresh() {
    // A segment, then an undo (the sketch as it was, at a new revision): the chain's last
    // point is gone, so the chain ends, and the next click begins one, adding nothing.
    let mut d = Drive::new(ToolKind::Line, empty());
    d.click(10.0, 10.0).click(40.0, 30.0);
    d.sketch = empty();
    d.revision = d.revision.next().unwrap();
    d.click(60.0, 5.0);
    assert!(d.tool.busy(), "a new chain from (60, 5)");
    d.click(70.0, 20.0);
    assert_eq!(d.adds().len(), 2);
    assert_eq!(
        d.adds()[1],
        sorted(&["point (60, 5)", "point (70, 20)", "line (60, 5)-(70, 20)"])
    );
}

#[test]
fn a_refused_add_mid_chain_is_forgotten_with_the_chain() {
    // The second click's add, refused by the core (not applied): `forget_pending` drops it
    // and the chain, so the next click begins afresh rather than naming its points.
    let sketch = empty();
    let sid = sketch_id();
    let context = Context {
        sketch_id: &sid,
        revision: Revision::OPENED,
        sketch: &sketch,
        tolerance: TOLERANCE,
        automatic_relations: true,
    };
    let mut tool = SketchTool::new(ToolKind::Line);
    let mut ids = Counter(0);
    let keys = Modifiers::default();
    let mut click = |tool: &mut SketchTool, at: Xy| {
        tool.handle(&context, &mut ids, Event::Press(at), keys);
        tool.handle(&context, &mut ids, Event::Release(at), keys)
    };
    click(&mut tool, [10.0, 10.0]);
    assert!(click(&mut tool, [40.0, 30.0]).add.is_some());
    tool.forget_pending();
    assert!(!tool.busy());
    assert_eq!(click(&mut tool, [60.0, 5.0]).add, None);
    let next = click(&mut tool, [70.0, 20.0]).add.unwrap();
    // Every point the segment names is its own: none of the refused add's.
    let points: Vec<&ElementId> = next.points.iter().map(|p| &p.id).collect();
    assert!(points.contains(&&next.lines[0].from) && points.contains(&&next.lines[0].to));
    assert!(sketch_edit::add(&sketch, &next).is_ok());
}

#[test]
fn a_shared_first_point_removed_by_another_client_becomes_a_new_one_there() {
    // A rectangle pressed on the point (10, 10); before its release, another client removes
    // the point (a read at a later revision without it). The corner is a new point there.
    let mut s = empty();
    s.points.push(point("q0000001", 10.0, 10.0));
    let mut d = Drive::new(ToolKind::CornerRectangle, s);
    d.press(10.0, 10.0).to(40.0, 30.0);
    d.sketch = empty();
    d.revision = d.revision.next().unwrap();
    d.release(40.0, 30.0);
    assert_eq!(d.adds(), [rectangle_10_10_40_30()]);
}

#[test]
fn what_a_read_past_an_adds_revision_lacks_is_forgotten() {
    // A point added at revision 2. A read of revision 1 (stale) doesn't hold it: still
    // found, so a click there adds nothing. A read of revision 3 without it (another
    // client's remove, an undo, a rollback): forgotten, a click there adds a point again.
    let mut d = Drive::new(ToolKind::Point, empty());
    d.click(12.0, 34.0);
    let before = empty();
    d.sketch = before.clone();
    d.revision = Revision::OPENED;
    d.click(12.0, 34.0);
    assert_eq!(d.messages, [Message::PointThere(id("t0000001"))]);
    d.sketch = before;
    d.revision = Revision::from_number(3).unwrap();
    d.click(12.0, 34.0);
    assert_eq!(d.adds().len(), 2);
}

#[test]
fn a_switch_of_sketch_starts_the_tool_afresh() {
    // A chain under way, its segment added; the context names another sketch: the chain
    // ends and nothing of the first sketch is found there.
    let first = empty();
    let other = FeatureId::new("s0000002").unwrap();
    let sid = sketch_id();
    let mut tool = SketchTool::new(ToolKind::Point);
    let mut ids = Counter(0);
    let keys = Modifiers::default();
    let mut click = |tool: &mut SketchTool, sketch_id: &FeatureId, at: Xy| {
        let context = Context {
            sketch_id,
            revision: Revision::OPENED,
            sketch: &first,
            tolerance: TOLERANCE,
            automatic_relations: true,
        };
        tool.handle(&context, &mut ids, Event::Press(at), keys);
        tool.handle(&context, &mut ids, Event::Release(at), keys)
    };
    assert!(click(&mut tool, &sid, [12.0, 34.0]).add.is_some());
    assert!(click(&mut tool, &sid, [12.0, 34.0]).message.is_some());
    assert!(click(&mut tool, &other, [12.0, 34.0]).add.is_some());
    let mut line = SketchTool::new(ToolKind::Line);
    let context = |sketch_id| Context {
        sketch_id,
        revision: Revision::OPENED,
        sketch: &first,
        tolerance: TOLERANCE,
        automatic_relations: true,
    };
    let mut ids = Counter(0);
    line.handle(&context(&sid), &mut ids, Event::Press([1.0, 2.0]), keys);
    line.handle(&context(&sid), &mut ids, Event::Release([1.0, 2.0]), keys);
    assert!(line.busy());
    line.handle(&context(&other), &mut ids, Event::Move([5.0, 6.0]), keys);
    assert!(!line.busy());
}

// --- Review item 7: the work is bounded --------------------------------------------------

#[test]
fn a_sketch_at_the_gates_limits_builds_each_shape_once_and_tries_few_crossings() {
    // 512 points (the gate's 1,024 unknowns) and lines up to the gate's 10,000 pieces of
    // geometry (3 are the origin and axes): 9,485 lines. Two cross exactly where the pointer
    // is, (50, 128): `n9999998`, y = 128 from (−50, 128) to (100, 128), and `n9999999`,
    // x = 50 from (50, 90) to (50, 200), the highest IDs. The other 9,483 run from
    // (−100, i) to (100, j), 254 points a side, passing x = 50 at 0.25 i + 0.75 j: those
    // nearest it first, and none through (50, 128) itself: 170 pass 0.25 mm above or below
    // it there (through (50, 127.75) or (50, 128.25), each a point where they cross), within
    // the tolerance, and none nearer. Each shape is built once a view; at most
    // 32·31/2 = 496 pairs are tried, of the 32 curves nearest; and those include the two
    // that cross at the pointer, so that's the crossing found: a point at (50, 128), on
    // each. (Taken in ID order instead, the 32 would be the fan's lowest, which cross at
    // (50, 127.75) or (50, 128.25).)
    let mut s = empty();
    for i in 0..254 {
        s.points
            .push(point(&format!("p{i:07}"), -100.0, f64::from(i)));
        s.points
            .push(point(&format!("r{i:07}"), 100.0, f64::from(i)));
    }
    for (name, x, y) in [
        ("q0000001", -50.0, 128.0),
        ("q0000002", 100.0, 128.0),
        ("q0000003", 50.0, 90.0),
        ("q0000004", 50.0, 200.0),
    ] {
        s.points.push(point(name, x, y));
    }
    let off = |(i, j): (u32, u32)| (0.25 * f64::from(i) + 0.75 * f64::from(j) - 128.0).abs();
    let mut pairs: Vec<(u32, u32)> = (0..254)
        .flat_map(|i| (0..254).map(move |j| (i, j)))
        .filter(|&p| off(p) > 0.0)
        .collect();
    pairs.sort_by(|&a, &b| off(a).total_cmp(&off(b)).then(a.cmp(&b)));
    for (k, (i, j)) in pairs.iter().take(10_000 - 512 - 3 - 2).enumerate() {
        s.lines.push(SketchLine {
            id: ElementId::new(&format!("n{k:07}")).unwrap(),
            from: ElementId::new(&format!("p{i:07}")).unwrap(),
            to: ElementId::new(&format!("r{j:07}")).unwrap(),
            construction: false,
        });
    }
    s.lines.push(line("n9999998", "q0000001", "q0000002"));
    s.lines.push(line("n9999999", "q0000003", "q0000004"));
    assert_eq!(s.points.len() * 2, 1024);
    assert_eq!(s.points.len() + s.lines.len() + 3, 10_000);
    assert_eq!(pairs.iter().filter(|&&p| off(p) == 0.25).count(), 170);
    let sid = sketch_id();
    let context = Context {
        sketch_id: &sid,
        revision: Revision::OPENED,
        sketch: &s,
        tolerance: TOLERANCE,
        automatic_relations: true,
    };
    let mut tool = SketchTool::new(ToolKind::Point);
    let mut ids = Counter(0);
    let keys = Modifiers::default();
    infer::work::SHAPES.with(|c| c.set(0));
    infer::work::PAIRS.with(|c| c.set(0));
    tool.handle(&context, &mut ids, Event::Move([50.0, 128.0]), keys);
    assert_eq!(infer::work::SHAPES.with(|c| c.get()), s.lines.len());
    let tried = infer::work::PAIRS.with(|c| c.get());
    assert!(tried > 0 && tried <= 496, "{tried} pairs");
    tool.handle(&context, &mut ids, Event::Press([50.0, 128.0]), keys);
    let added = tool
        .handle(&context, &mut ids, Event::Release([50.0, 128.0]), keys)
        .add
        .unwrap();
    assert_eq!(added.points.len(), 1);
    let p = &added.points[0];
    assert_eq!((p.x.mm(), p.y.mm()), (50.0, 128.0));
    let on = |curve: &str| Form::On {
        point: Reference::Element(p.id.clone()),
        curve: Reference::Element(id(curve)),
    };
    assert_eq!(
        added
            .constraints
            .iter()
            .map(|c| c.form.clone())
            .collect::<Vec<_>>(),
        [on("n9999998"), on("n9999999")]
    );
}

// --- Review items 8, 9, 10: arcs ---------------------------------------------------------

#[test]
fn an_arc_with_a_radius_under_the_kernels_floor_is_too_small() {
    // With a pick tolerance of 1e-9: a 3-point arc from (5, 5) to (5 + 1.5e-7, 5) through
    // the top of the circle between them, radius 0.75e-7, under the gate's 1e-7; and a
    // tangent arc from the line's end (20, 10), along +x, to 1.5e-7 above it: its centre on
    // the normal at |d|²/(2 d·n) = 0.75e-7. Each says it's too small; neither reaches the
    // gate.
    let mut d = Drive::new(ToolKind::ThreePointArc, empty());
    d.tolerance = 1e-9;
    d.click(5.0, 5.0)
        .click(5.0 + 1.5e-7, 5.0)
        .click(5.0 + 0.75e-7, 5.0 + 0.75e-7);
    assert!(d.adds().is_empty());
    assert_eq!(d.messages, [Message::TooSmall]);
    let mut d = Drive::new(ToolKind::TangentArc, one_line());
    d.tolerance = 1e-9;
    d.drag([20.0, 10.0], [20.0, 10.0 + 1.5e-7]);
    assert!(d.adds().is_empty());
    assert_eq!(d.messages, [Message::TooSmall]);
}

#[test]
fn nearly_in_line_arcs_whose_points_are_in_reach_say_so() {
    // Three points 1e-9 mm off a line: the circle through them is far past a kilometre, but
    // every point is near: in a line, not too far. A tangent arc to a point 1e-9 off its
    // line's direction, 40 mm on: the same.
    let mut d = Drive::new(ToolKind::ThreePointArc, empty());
    d.click(10.0, 10.0)
        .click(30.0, 10.0)
        .click(20.0, 10.0 + 1e-9);
    assert_eq!(d.messages, [Message::InLine]);
    let mut d = Drive::new(ToolKind::TangentArc, one_line());
    d.drag([20.0, 10.0], [60.0, 10.0 + 1e-9]);
    assert_eq!(d.messages, [Message::InLine]);
    assert!(d.adds().is_empty());
}

#[test]
fn a_centrepoint_arcs_end_snapped_to_the_origin_off_its_circle_stays_on_it() {
    // The review's case. Centre (0, 5.2), on the y axis; start (5, 5.2), radius 5; swept
    // clockwise toward (0.3, −2): the end on the circle toward (0.3, −7.2) from the centre,
    // (0, 5.2) + 5·(0.3, −7.2)/√51.93, 0.29 mm from the origin, which the pointer would take.
    // The origin is 5.2 from the centre, off the circle: the end stays on it, with no
    // relation; the arc written from it (clockwise) to the start.
    let mut d = Drive::new(ToolKind::CenterpointArc, empty());
    d.click(0.0, 5.2).click(5.0, 5.2).click(0.3, -2.0);
    let made = &d.made[0];
    let length = libm::hypot(0.3, -7.2);
    let end = [5.0 * 0.3 / length, 5.2 + 5.0 * -7.2 / length];
    let descriptions = &d.adds()[0];
    assert!(
        !descriptions.iter().any(|l| l.contains("origin")),
        "{descriptions:?}"
    );
    let e = made
        .points
        .iter()
        .find(|p| (p.y.mm() - end[1]).abs() < 1e-9)
        .unwrap();
    assert!((e.x.mm() - end[0]).abs() < 1e-9, "{e:?} {end:?}");
    assert_eq!(made.arcs[0].start, e.id);
}

#[test]
fn a_centrepoint_arcs_end_on_a_line_is_where_the_line_crosses_its_circle() {
    // Centre (50, 50), start (60, 50), radius 10; swept clockwise to (50, 40). There the
    // pointer finds the line x = 50.3, at its foot (50.3, 40), off the circle: the end goes
    // where the line crosses the circle, (50.3, 50 − √99.91), with `on` the line.
    let mut s = empty();
    s.points.push(point("q0000001", 50.3, 30.0));
    s.points.push(point("q0000002", 50.3, 45.0));
    s.lines.push(line("n0000001", "q0000001", "q0000002"));
    let mut d = Drive::new(ToolKind::CenterpointArc, s);
    d.click(50.0, 50.0)
        .click(60.0, 50.0)
        .to(57.0, 43.0)
        .click(50.0, 40.0);
    let made = &d.made[0];
    let end = made.arcs[0].start.clone();
    let p = made.points.iter().find(|p| p.id == end).unwrap();
    assert!((p.x.mm() - 50.3).abs() < 1e-9, "{p:?}");
    assert!((p.y.mm() - (50.0 - 99.91f64.sqrt())).abs() < 1e-9, "{p:?}");
    assert_eq!(
        made.constraints.iter().map(|c| &c.form).collect::<Vec<_>>(),
        [&Form::On {
            point: Reference::Element(end),
            curve: Reference::Element(id("n0000001")),
        }]
    );
}

// --- Review item 11: the nearest candidate, and ties by ID -------------------------------

#[test]
fn the_nearest_point_wins_and_a_tie_goes_to_the_lower_id() {
    // Points 0.2 and 0.4 mm from the click: the nearer. Two 0.25 mm either side: the lower
    // ID, though it comes second in the sketch's list.
    let mut s = empty();
    s.points.push(point("q0000002", 10.4, 10.0));
    s.points.push(point("q0000001", 10.2, 10.0));
    s.points.push(point("q0000004", 30.25, 10.0));
    s.points.push(point("q0000003", 29.75, 10.0));
    let mut d = Drive::new(ToolKind::Point, s);
    d.click(10.0, 10.0).click(30.0, 10.0);
    assert_eq!(
        d.messages,
        [
            Message::PointThere(id("q0000001")),
            Message::PointThere(id("q0000003")),
        ]
    );
}

#[test]
fn the_nearest_middle_wins_and_a_tie_goes_to_the_lower_id() {
    // Middles 0.2 and 0.4 mm from the click, (25, 20) and (25, 20.6); then two 0.25 mm
    // either side, (65, 19.75) and (65, 20.25).
    let mut s = empty();
    for (k, (y, x0)) in [(20.6, 20.0), (20.0, 20.0), (20.25, 60.0), (19.75, 60.0)]
        .into_iter()
        .enumerate()
    {
        let (a, b) = (format!("q00000{k}a"), format!("q00000{k}b"));
        s.points.push(point(&a, x0, y));
        s.points.push(point(&b, x0 + 10.0, y));
        s.lines.push(line(&format!("n000000{k}"), &a, &b));
    }
    let mut d = Drive::new(ToolKind::Point, s);
    d.click(25.0, 20.2).click(65.0, 20.0);
    assert_eq!(
        d.adds(),
        [
            sorted(&[
                "point (25, 20)",
                "midpoint (25, 20) line #(20, 20)-#(30, 20)"
            ]),
            sorted(&[
                "point (65, 20.25)",
                "midpoint (65, 20.25) line #(60, 20.25)-#(70, 20.25)",
            ]),
        ]
    );
}

#[test]
fn the_nearest_curve_wins_over_an_axis() {
    // The line (10, 0.2)-(40, 0.2) and the x axis, parallel (no crossing): a click at
    // (20, 0.4) is 0.2 from the line and 0.4 from the axis: on the line, at (20, 0.2).
    let mut s = empty();
    s.points.push(point("q0000001", 10.0, 0.2));
    s.points.push(point("q0000002", 40.0, 0.2));
    s.lines.push(line("n0000001", "q0000001", "q0000002"));
    let mut d = Drive::new(ToolKind::Point, s);
    d.click(20.0, 0.4);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (20, 0.2)",
            "on (20, 0.2) line #(10, 0.2)-#(40, 0.2)"
        ])]
    );
}

#[test]
fn the_nearest_woken_point_lines_up_and_a_tie_goes_to_the_lower_id() {
    // Woken points at x = 30.2 and 30.4: a click at (30, 50) is 0.2 and 0.4 off their
    // verticals, so above the first. Woken at 29.75 and 30.25: 0.25 each, the lower ID.
    for (points, want) in [
        (
            [("q0000002", 30.4, 20.0), ("q0000001", 30.2, 10.0)],
            (30.2, "#(30.2, 10)"),
        ),
        (
            [("q0000004", 30.25, 20.0), ("q0000003", 29.75, 10.0)],
            (29.75, "#(29.75, 10)"),
        ),
    ] {
        let mut s = empty();
        for (name, x, y) in points {
            s.points.push(point(name, x, y));
        }
        let mut d = Drive::new(ToolKind::Point, s);
        for (_, x, y) in points {
            d.to(x, y + 0.1);
        }
        d.click(30.0, 50.0);
        let at = format!("({}, 50)", want.0);
        assert_eq!(
            d.adds(),
            [sorted(&[
                &format!("point {at}"),
                &format!("vertical_points {at} {}", want.1),
            ])]
        );
    }
}

#[test]
fn the_nearer_of_two_lines_at_a_chains_start_is_the_perpendicular() {
    // Two lines end at (20, 20), leaving it at 45° and 46°. A line drawn from there at
    // 134.5° is 0.5° off perpendicular to the first, 1.5° off the second: perpendicular to
    // the first, though its ID is the higher.
    let mut s = empty();
    s.points.push(point("q0000001", 20.0, 20.0));
    let back = |deg: f64| toward([20.0, 20.0], deg + 180.0, 10.0);
    let (a, b) = (back(45.0), back(46.0));
    s.points.push(point("q0000002", a[0], a[1]));
    s.points.push(point("q0000003", b[0], b[1]));
    s.lines.push(line("n0000002", "q0000002", "q0000001"));
    s.lines.push(line("n0000001", "q0000003", "q0000001"));
    let mut d = Drive::new(ToolKind::Line, s);
    d.drag([20.0, 20.0], toward([20.0, 20.0], 134.5, 20.0));
    let made = &d.made[0];
    assert_eq!(
        made.constraints[0].form,
        Form::Perpendicular {
            a: Reference::Element(id("n0000002")),
            b: Reference::Element(made.lines[0].id.clone()),
        }
    );
}

// --- Review item 12: the long way round, and across the seam -------------------------------

#[test]
fn a_centrepoint_arc_swept_270_degrees_counter_clockwise_is_the_long_arc() {
    // Centre (50, 50), start (60, 50); swept through (50, 60) and (40, 50) to (50, 40): 270°
    // counter-clockwise, the arc from (60, 50) to (50, 40), not the short way back.
    let mut d = Drive::new(ToolKind::CenterpointArc, empty());
    d.click(50.0, 50.0)
        .click(60.0, 50.0)
        .to(50.0, 60.0)
        .to(40.0, 50.0)
        .click(50.0, 40.0);
    assert!(d.adds()[0].contains(&"arc (50, 50) (60, 50)->(50, 40)".to_owned()));
}

#[test]
fn a_centrepoint_arc_swept_across_the_seam_is_the_short_arc() {
    // Centre (50, 50), radius 10, start at 170°, swept 20° counter-clockwise to 190° (past
    // ±180°): the arc from the start to the end, 20° long.
    let start = toward([50.0, 50.0], 170.0, 10.0);
    let end = toward([50.0, 50.0], 190.0, 10.0);
    let mut d = Drive::new(ToolKind::CenterpointArc, empty());
    d.click(50.0, 50.0)
        .click(start[0], start[1])
        .to(toward([50.0, 50.0], 180.0, 10.0)[0], 50.0)
        .click(end[0], end[1]);
    let made = &d.made[0];
    let place = |id: &ElementId| {
        let p = made.points.iter().find(|p| &p.id == id).unwrap();
        [p.x.mm(), p.y.mm()]
    };
    let (s, e) = (place(&made.arcs[0].start), place(&made.arcs[0].end));
    assert!(distance(s, start) < 1e-9, "{s:?}");
    assert!(distance(e, end) < 1e-9, "{e:?}");
}

// --- Review item 14: a drag against a click, for every tool -----------------------------

#[test]
fn a_drag_back_to_its_press_stays_a_drag_and_draws_nothing() {
    // Press at (12, 14) (at the line's free end (10, 10), for the tangent arc), move far,
    // release 0.22 mm from the press: still a drag, ended there, too small, the tool idle.
    // The point tool places its point at the release.
    for kind in ToolKind::ALL {
        let mut d = Drive::new(kind, one_line());
        let (press, release) = if kind == ToolKind::TangentArc {
            ([10.0, 10.0], [10.2, 10.1])
        } else {
            ([12.0, 14.0], [12.2, 14.1])
        };
        d.press(press[0], press[1])
            .to(40.0, 40.0)
            .release(release[0], release[1]);
        if kind == ToolKind::Point {
            assert_eq!(d.adds(), [sorted(&["point (12.2, 14.1)"])]);
            continue;
        }
        assert!(d.adds().is_empty(), "{kind:?}");
        assert_eq!(d.messages, [Message::TooSmall], "{kind:?}");
        assert!(!d.tool.busy(), "{kind:?}");
    }
}

#[test]
fn a_press_and_a_release_far_apart_with_no_move_is_a_drag() {
    // Press at (12, 14), release at (30, 41), nothing between: each tool takes the drag.
    // A line, a rectangle, a circle, a tangent arc (from the line's end (20, 10)) are
    // drawn; the centrepoint and 3-point arcs have their second point and wait for their
    // third; the point is placed at the release.
    for kind in ToolKind::ALL {
        let mut d = Drive::new(kind, one_line());
        let from = if kind == ToolKind::TangentArc {
            [20.0, 10.0]
        } else {
            [12.0, 14.0]
        };
        d.press(from[0], from[1]).release(30.0, 41.0);
        let waits = matches!(kind, ToolKind::CenterpointArc | ToolKind::ThreePointArc);
        assert_eq!(d.adds().len(), usize::from(!waits), "{kind:?}");
        assert_eq!(d.tool.busy(), waits, "{kind:?}");
        assert!(d.messages.is_empty(), "{kind:?}: {:?}", d.messages);
    }
}

// --- Review item 15: angles at the tolerance's edge --------------------------------------

/// The constraint types a line from `from` at `deg`°, 20 mm long, adds in `sketch`.
fn line_at(sketch: &Sketch, from: Xy, deg: f64) -> Vec<&'static str> {
    let mut d = Drive::new(ToolKind::Line, sketch.clone());
    d.drag(from, toward(from, deg, 20.0));
    let mut types = types(&d);
    types.dedup();
    types
}

#[test]
fn horizontal_and_vertical_show_within_2_degrees_either_side() {
    let s = empty();
    for (deg, want) in [
        (1.9, vec!["horizontal"]),
        (-1.9, vec!["horizontal"]),
        (2.1, vec![]),
        (-2.1, vec![]),
        (178.1, vec!["horizontal"]),
        (181.9, vec!["horizontal"]),
        (91.9, vec!["vertical"]),
        (88.1, vec!["vertical"]),
        (92.1, vec![]),
        (87.9, vec![]),
    ] {
        assert_eq!(line_at(&s, [10.0, 10.0], deg), want, "{deg}°");
    }
}

#[test]
fn perpendicular_and_parallel_show_within_2_degrees_either_side() {
    // A line ending at (20, 20), leaving it at 30°: perpendicular at 120° ± 1.9, not ± 2.1;
    // parallel at 30° ± 1.9, not ± 2.1.
    let mut s = empty();
    let back = toward([20.0, 20.0], 210.0, 10.0);
    s.points.push(point("q0000001", 20.0, 20.0));
    s.points.push(point("q0000002", back[0], back[1]));
    s.lines.push(line("n0000001", "q0000002", "q0000001"));
    for (deg, want) in [
        (121.9, vec!["perpendicular"]),
        (118.1, vec!["perpendicular"]),
        (122.1, vec![]),
        (117.9, vec![]),
        (31.9, vec!["parallel"]),
        (28.1, vec!["parallel"]),
        (32.1, vec![]),
        (27.9, vec![]),
    ] {
        assert_eq!(line_at(&s, [20.0, 20.0], deg), want, "{deg}°");
    }
}

#[test]
fn a_tangent_shows_within_2_degrees_either_side_of_an_arcs_end_and_its_start() {
    // The arc about (50, 50), radius 10, from 0° to 120°: past its end, at 120°, it runs
    // along 210°; back past its start, at 0°, along 270°, which is vertical, so the start is
    // tried with the arc from 30° (back past it along 300°... −60°).
    let s = with_arc(empty(), [50.0, 50.0], 10.0, 0.0, 120.0);
    let end = toward([50.0, 50.0], 120.0, 10.0);
    for (deg, want) in [
        (211.9, vec!["tangent_at"]),
        (208.1, vec!["tangent_at"]),
        (212.1, vec![]),
        (207.9, vec![]),
        (213.0, vec![]),
        (207.0, vec![]),
    ] {
        assert_eq!(line_at(&s, end, deg), want, "{deg}° from the end");
    }
    let s = with_arc(empty(), [50.0, 50.0], 10.0, 30.0, 120.0);
    let start = toward([50.0, 50.0], 30.0, 10.0);
    for (deg, want) in [
        (-58.1, vec!["tangent_at"]),
        (-61.9, vec!["tangent_at"]),
        (-57.9, vec![]),
        (-62.1, vec![]),
        // The start's direction the other way (+x turned a quarter left, 120°): no.
        (120.0, vec![]),
    ] {
        assert_eq!(line_at(&s, start, deg), want, "{deg}° from the start");
    }
}

// --- Review item 16: the order, and the tolerance's edge for each kind -------------------

#[test]
fn each_kind_comes_before_the_next_whatever_their_distances() {
    // Point before midpoint: the point (25.4, 20) 0.3 away, the middle (25, 20) 0.1.
    let mut s = empty();
    s.points.push(point("q0000001", 20.0, 20.0));
    s.points.push(point("q0000002", 30.0, 20.0));
    s.lines.push(line("n0000001", "q0000001", "q0000002"));
    let mut with_point = s.clone();
    with_point.points.push(point("q0000009", 25.4, 20.0));
    let mut d = Drive::new(ToolKind::Point, with_point);
    d.click(25.1, 20.0);
    assert_eq!(d.messages, [Message::PointThere(id("q0000009"))]);
    // Origin before midpoint: the line (−10, 0.3)-(10, 0.3), its middle (0, 0.3) 0.05 from
    // the click at (0, 0.25), the origin 0.25.
    let mut o = empty();
    o.points.push(point("q0000001", -10.0, 0.3));
    o.points.push(point("q0000002", 10.0, 0.3));
    o.lines.push(line("n0000001", "q0000001", "q0000002"));
    let mut d = Drive::new(ToolKind::Point, o);
    d.click(0.0, 0.25);
    assert!(d.adds()[0].contains(&"horizontal_points (0, 0) origin".to_owned()));
    // Midpoint before crossing: the line x = 25.35 crosses (20, 20)-(30, 20) 0.05 from the
    // click at (25.3, 20), the middle 0.3 from it.
    let mut m = s.clone();
    m.points.push(point("q0000003", 25.35, 10.0));
    m.points.push(point("q0000004", 25.35, 60.0));
    m.lines.push(line("n0000002", "q0000003", "q0000004"));
    let mut d = Drive::new(ToolKind::Point, m);
    d.click(25.3, 20.0);
    assert!(
        d.adds()[0]
            .iter()
            .any(|l| l.starts_with("midpoint (25, 20)"))
    );
    // Crossing before a curve: the click at (25.35, 20.4) is on the line x = 25.35 and 0.4
    // from where it crosses y = 20 (the long lines' middles far off).
    let mut c = empty();
    for (k, (a, b)) in [([10.0, 20.0], [50.0, 20.0]), ([25.35, 10.0], [25.35, 60.0])]
        .into_iter()
        .enumerate()
    {
        let (p, q) = (format!("q000000{k}"), format!("r000000{k}"));
        c.points.push(point(&p, a[0], a[1]));
        c.points.push(point(&q, b[0], b[1]));
        c.lines.push(line(&format!("n000000{k}"), &p, &q));
    }
    let mut d = Drive::new(ToolKind::Point, c);
    d.click(25.35, 20.4);
    assert!(
        d.adds()[0].contains(&"point (25.35, 20)".to_owned()),
        "{:?}",
        d.adds()
    );
}

#[test]
fn each_kind_snaps_at_exactly_the_tolerance_and_not_a_float_past_it() {
    let past = |v: f64| f64::from_bits(v.to_bits() + 1);
    // Each case: a sketch, a click 0.5 mm off its candidate (each difference exact), and
    // what that click's add holds; one float farther, it doesn't.
    let mut lines = empty();
    for (k, (a, b)) in [([10.0, 20.0], [50.0, 20.0]), ([33.0, 10.0], [33.0, 60.0])]
        .into_iter()
        .enumerate()
    {
        let (p, q) = (format!("q000000{k}"), format!("r000000{k}"));
        lines.points.push(point(&p, a[0], a[1]));
        lines.points.push(point(&q, b[0], b[1]));
        lines.lines.push(line(&format!("n000000{k}"), &p, &q));
    }
    let mut circle = empty();
    circle.points.push(point("c0000001", 100.0, 100.0));
    circle.circles.push(crate::document::SketchCircle {
        id: id("k0000001"),
        center: id("c0000001"),
        radius: Coordinate::new(10.0).unwrap(),
        construction: false,
    });
    let cases: [(&str, Sketch, Xy, Xy, &str); 6] = [
        (
            "origin",
            empty(),
            [0.5, 0.0],
            [past(0.5), 0.0],
            "horizontal_points (0, 0) origin",
        ),
        (
            "middle",
            lines.clone(),
            [30.0, 20.5],
            [30.0, past(20.5)],
            "midpoint (30, 20) line #(10, 20)-#(50, 20)",
        ),
        (
            "crossing",
            lines.clone(),
            [33.0, 20.5],
            [33.0, past(20.5)],
            "point (33, 20)",
        ),
        (
            "line",
            lines,
            [40.0, 20.5],
            [40.0, past(20.5)],
            "on (40, 20) line #(10, 20)-#(50, 20)",
        ),
        (
            "circle",
            circle,
            [110.5, 100.0],
            [past(110.5), 100.0],
            "on (110, 100) circle #(100, 100) r10",
        ),
        (
            "axis",
            empty(),
            [40.0, 0.5],
            [40.0, past(0.5)],
            "on (40, 0) x_axis",
        ),
    ];
    for (what, sketch, at, beyond, want) in cases {
        let mut d = Drive::new(ToolKind::Point, sketch.clone());
        d.click(at[0], at[1]);
        assert!(
            d.adds()[0].contains(&want.to_owned()),
            "{what}: {:?}",
            d.adds()
        );
        let mut d = Drive::new(ToolKind::Point, sketch);
        d.click(beyond[0], beyond[1]);
        assert!(
            !d.adds()[0].contains(&want.to_owned()),
            "{what} past: {:?}",
            d.adds()
        );
    }
}

#[test]
fn a_line_is_found_only_as_drawn_and_an_arc_only_where_it_is() {
    // A click 1 mm past the end of (10, 20)-(50, 20), on its line: nothing. A click on the
    // circle of the arc about (100, 100) from 0° to 90°, at 180°: nothing.
    let mut s = with_arc(empty(), [100.0, 100.0], 10.0, 0.0, 90.0);
    s.points.push(point("q0000001", 10.0, 20.0));
    s.points.push(point("q0000002", 50.0, 20.0));
    s.lines.push(line("n0000001", "q0000001", "q0000002"));
    let mut d = Drive::new(ToolKind::Point, s);
    d.click(51.0, 20.0).click(90.0, 100.0);
    assert_eq!(
        d.adds(),
        [sorted(&["point (51, 20)"]), sorted(&["point (90, 100)"])]
    );
}

// --- Review item 17 -----------------------------------------------------------------------

#[test]
fn shift_squares_in_every_quadrant_and_about_a_centre() {
    // From (50, 50), the pointer's farther offset (30) each way: the opposite corner.
    for (to, corner) in [
        ([80.0, 65.0], "(80, 80)"),
        ([20.0, 65.0], "(20, 80)"),
        ([20.0, 35.0], "(20, 20)"),
        ([80.0, 35.0], "(80, 20)"),
    ] {
        let mut d = Drive::new(ToolKind::CornerRectangle, empty());
        d.keys.shift = true;
        d.drag([50.0, 50.0], to);
        assert!(d.adds()[0].contains(&format!("point {corner}")), "{to:?}");
    }
    // About the centre (50, 50), the pointer at (60, 55): the corner (60, 60), the opposite
    // (40, 40).
    let mut d = Drive::new(ToolKind::CenterRectangle, empty());
    d.keys.shift = true;
    d.drag([50.0, 50.0], [60.0, 55.0]);
    let add = &d.adds()[0];
    assert!(
        add.contains(&"point (60, 60)".to_owned()) && add.contains(&"point (40, 40)".to_owned())
    );
}

#[test]
fn a_vertical_lines_end_level_with_a_woken_point_follows_a_guide() {
    // The woken point (5, 40.3); a line from (20, 10) to (20.3, 40), vertical, its end on
    // the vertical, (20, 40), then moved level with the point, (20, 40.3): a guide, with no
    // `horizontal_points`.
    let mut s = empty();
    s.points.push(point("q0000001", 5.0, 40.3));
    let mut d = Drive::new(ToolKind::Line, s);
    d.to(5.1, 40.3).drag([20.0, 10.0], [20.3, 40.0]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (20, 10)",
            "point (20, 40.3)",
            "line (20, 10)-(20, 40.3)",
            "vertical line (20, 10)-(20, 40.3)",
        ])]
    );
}

#[test]
fn a_rectangles_corner_follows_a_guide_and_a_centre_takes_the_relation() {
    // The woken point (30, 10), and a first point 0.3 off its vertical at (30.3, 50): a
    // corner rectangle's corner moves to (30, 50) with no relation; a circle's and a
    // centrepoint arc's centre, and a centre rectangle's, with `vertical_points`.
    let mut s = empty();
    s.points.push(point("q0000001", 30.0, 10.0));
    let mut d = Drive::new(ToolKind::CornerRectangle, s.clone());
    d.to(30.1, 10.1).drag([30.3, 50.0], [60.0, 70.0]);
    assert!(d.adds()[0].contains(&"point (30, 50)".to_owned()));
    assert!(!d.adds()[0].iter().any(|l| l.starts_with("vertical_points")));
    for kind in [
        ToolKind::Circle,
        ToolKind::CenterRectangle,
        ToolKind::CenterpointArc,
    ] {
        let mut d = Drive::new(kind, s.clone());
        d.to(30.1, 10.1).drag([30.3, 50.0], [40.0, 57.0]);
        if kind == ToolKind::CenterpointArc {
            d.to(30.0, 62.0).click(22.0, 57.0);
        }
        assert!(
            d.adds()[0].contains(&"vertical_points (30, 50) #(30, 10)".to_owned()),
            "{kind:?}: {:?}",
            d.adds()
        );
    }
}

#[test]
fn a_circles_radius_set_on_the_origin_passes_through_it() {
    // Centre (10, 10), the radius set 0.2 mm from the origin: through it, radius √200.
    let mut d = Drive::new(ToolKind::Circle, empty());
    d.drag([10.0, 10.0], [0.1, 0.1]);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (10, 10)",
            "circle (10, 10) r14.142135624",
            "on origin circle (10, 10) r14.142135624",
        ])]
    );
}

#[test]
fn small_gestures_that_add_nothing() {
    // A 3-point arc's third click on its end: a double-click's, nothing, still waiting.
    let mut d = Drive::new(ToolKind::ThreePointArc, empty());
    d.click(10.0, 10.0).click(30.0, 10.0).click(30.1, 10.0);
    assert!(d.adds().is_empty() && d.messages.is_empty());
    assert!(d.tool.busy());
    // The point tool's release with no press.
    let mut d = Drive::new(ToolKind::Point, empty());
    d.release(12.0, 34.0);
    assert!(d.adds().is_empty());
}

/// An ID source that gives one the sketch has.
struct Taken;

impl NewIds for Taken {
    fn next(&mut self, _: &dyn Fn(&ElementId) -> bool) -> Result<ElementId, Error> {
        Ok(id("q0000001"))
    }
}

#[test]
fn an_id_source_that_gives_a_taken_id_is_caught() {
    let mut s = empty();
    s.points.push(point("q0000001", 1.0, 1.0));
    let sid = sketch_id();
    let context = Context {
        sketch_id: &sid,
        revision: Revision::OPENED,
        sketch: &s,
        tolerance: TOLERANCE,
        automatic_relations: true,
    };
    let mut tool = SketchTool::new(ToolKind::Point);
    let keys = Modifiers::default();
    tool.handle(&context, &mut Taken, Event::Press([12.0, 34.0]), keys);
    let out = tool.handle(&context, &mut Taken, Event::Release([12.0, 34.0]), keys);
    assert_eq!(out.add, None);
    let Some(Message::NoId(error)) = out.message else {
        panic!("{out:?}");
    };
    assert_eq!(error.code(), ErrorCode::Internal);
    assert_eq!(
        error.message(),
        "the IDs' source gave q0000001, which is taken"
    );
}

#[test]
fn each_tools_preview_draws_its_entity_under_way() {
    let mut d = Drive::new(ToolKind::CornerRectangle, empty());
    d.press(10.0, 10.0).to(40.0, 30.0);
    assert_eq!(
        d.tool.preview().lines,
        [
            [[10.0, 10.0], [40.0, 10.0]],
            [[40.0, 10.0], [40.0, 30.0]],
            [[40.0, 30.0], [10.0, 30.0]],
            [[10.0, 30.0], [10.0, 10.0]],
        ]
    );
    let mut d = Drive::new(ToolKind::CenterRectangle, empty());
    d.press(20.0, 10.0).to(30.0, 15.0);
    assert_eq!(d.tool.preview().construction_points, [[20.0, 10.0]]);
    assert_eq!(d.tool.preview().lines.len(), 4);
    let mut d = Drive::new(ToolKind::Circle, empty());
    d.press(10.0, 10.0).to(13.0, 14.0);
    assert_eq!(d.tool.preview().circles, [([10.0, 10.0], 5.0)]);
    let mut d = Drive::new(ToolKind::ThreePointArc, empty());
    d.click(10.0, 10.0).click(30.0, 10.0).to(20.0, 20.0);
    assert_eq!(
        d.tool.preview().arcs,
        [PreviewArc {
            center: [20.0, 10.0],
            start: [30.0, 10.0],
            end: [10.0, 10.0],
        }]
    );
    let mut d = Drive::new(ToolKind::TangentArc, one_line());
    d.press(20.0, 10.0).to(20.0, 20.0);
    assert_eq!(
        d.tool.preview().arcs,
        [PreviewArc {
            center: [20.0, 15.0],
            start: [20.0, 10.0],
            end: [20.0, 20.0],
        }]
    );
}

// --- Round 2, item 1: the tangent wins over an axis it isn't on ---------------------------

#[test]
fn a_line_leaving_an_arc_a_degree_off_an_axis_is_tangent_alone() {
    // The arc about (50, 50), radius 10, from 270° to 91°: past its end it runs along 181°,
    // 1° off −x. A line from its end toward 180.5°, 30 mm: 0.5° off the tangent and 0.5° off
    // horizontal. The tangent wins, the arc not leaving along the axis: `tangent_at` alone,
    // the line's end on the 181° ray, 30·cos 0.5° along it.
    let s = with_arc(empty(), [50.0, 50.0], 10.0, 270.0, 91.0);
    let end = toward([50.0, 50.0], 91.0, 10.0);
    let mut d = Drive::new(ToolKind::Line, s);
    d.drag(end, toward(end, 180.5, 30.0));
    let made = &d.made[0];
    assert_eq!(
        made.constraints
            .iter()
            .map(|c| c.form.clone())
            .collect::<Vec<_>>(),
        [Form::TangentAt {
            a: Reference::Element(id("a0000001")),
            b: Reference::Element(made.lines[0].id.clone()),
            at: Reference::Element(id("q0000102")),
            direction: crate::document::Direction::Same,
        }]
    );
    let want = toward(end, 181.0, 30.0 * libm::cos(0.5f64.to_radians()));
    let p = &made.points[0];
    assert!(
        distance([p.x.mm(), p.y.mm()], want) < 1e-9,
        "{p:?} {want:?}"
    );
}

// --- Round 2, item 2: each tool's gesture against a sketch that lost what it named --------

/// `d`'s sketch without the members `ids`, at the next revision: another client's remove.
fn removed(d: &mut Drive, ids: &[&str]) {
    let gone = |e: &ElementId| ids.contains(&e.as_str());
    d.sketch.points.retain(|p| !gone(&p.id));
    d.sketch.lines.retain(|l| !gone(&l.id));
    d.sketch.circles.retain(|c| !gone(&c.id));
    d.sketch.arcs.retain(|a| !gone(&a.id));
    d.sketch.constraints.retain(|c| !gone(&c.id));
    d.revision = d.revision.next().unwrap();
}

/// A lone point at (10, 10), `q0000001`.
fn lone() -> Sketch {
    let mut s = empty();
    s.points.push(point("q0000001", 10.0, 10.0));
    s
}

#[test]
fn a_circles_shared_centre_gone_is_a_new_point_there() {
    let mut d = Drive::new(ToolKind::Circle, lone());
    d.press(10.0, 10.0).to(12.0, 12.0);
    removed(&mut d, &["q0000001"]);
    d.to(13.0, 14.0).release(13.0, 14.0);
    assert_eq!(
        d.adds(),
        [sorted(&["point (10, 10)", "circle (10, 10) r5"])]
    );
}

#[test]
fn a_centrepoint_arcs_shared_points_gone_are_new_points_there() {
    // The centre clicked on the point (10, 10), which goes before the start's click; and an
    // arc whose centre and start both shared points that go mid-sweep.
    let mut d = Drive::new(ToolKind::CenterpointArc, lone());
    d.click(10.0, 10.0);
    removed(&mut d, &["q0000001"]);
    d.click(18.0, 16.0).to(13.0, 20.0).click(4.0, 18.0);
    let want = [sorted(&[
        "point (10, 10)",
        "point (18, 16)",
        "point (4, 18)",
        "arc (10, 10) (18, 16)->(4, 18)",
    ])];
    assert_eq!(d.adds(), want);
    let mut s = lone();
    s.points.push(point("q0000002", 18.0, 16.0));
    let mut d = Drive::new(ToolKind::CenterpointArc, s);
    d.click(10.0, 10.0).click(18.0, 16.0).to(13.0, 20.0);
    removed(&mut d, &["q0000001", "q0000002"]);
    d.click(4.0, 18.0);
    assert_eq!(d.adds(), want);
}

#[test]
fn a_three_point_arcs_shared_ends_gone_are_new_points_there() {
    // The start on (10, 10), gone before the end; the end on (30, 10), gone before the third
    // click.
    let mut s = lone();
    s.points.push(point("q0000002", 30.0, 10.0));
    let mut d = Drive::new(ToolKind::ThreePointArc, s);
    d.click(10.0, 10.0);
    removed(&mut d, &["q0000001"]);
    d.click(30.0, 10.0);
    removed(&mut d, &["q0000002"]);
    d.click(20.0, 20.0);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (20, 10)",
            "point (10, 10)",
            "point (30, 10)",
            "arc (20, 10) (30, 10)->(10, 10)",
        ])]
    );
}

#[test]
fn a_tangent_arc_whose_base_is_gone_ends() {
    // Pressed on the line's end (20, 10); the line and the end go: the gesture ends, idle,
    // and the release adds nothing. The same for a base that's still a point but no longer
    // any curve's end.
    for gone in [&["n0000001", "q0000002"][..], &["n0000001"][..]] {
        let mut d = Drive::new(ToolKind::TangentArc, one_line());
        d.press(20.0, 10.0).to(20.0, 15.0);
        removed(&mut d, gone);
        d.to(20.0, 20.0);
        assert!(!d.tool.busy(), "{gone:?}");
        d.release(20.0, 20.0);
        assert!(d.adds().is_empty(), "{gone:?}");
    }
}

#[test]
fn a_lines_first_point_gone_is_a_new_point_there() {
    // Pressed on the point (10, 10), which goes before the release: the line from a new
    // point there. And a chain's first click on a line, at (20, 30), whose line goes before
    // the second click: the segment from a new point there, with no `on`.
    let mut d = Drive::new(ToolKind::Line, lone());
    d.press(10.0, 10.0).to(30.0, 20.0);
    removed(&mut d, &["q0000001"]);
    d.to(40.0, 25.0).release(40.0, 25.0);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (10, 10)",
            "point (40, 25)",
            "line (10, 10)-(40, 25)"
        ])]
    );
    let mut s = empty();
    s.points.push(point("q0000001", 10.0, 30.0));
    s.points.push(point("q0000002", 50.0, 30.0));
    s.lines.push(line("n0000001", "q0000001", "q0000002"));
    let mut d = Drive::new(ToolKind::Line, s);
    d.click(20.0, 30.2);
    removed(&mut d, &["n0000001"]);
    d.click(40.0, 60.0);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (20, 30)",
            "point (40, 60)",
            "line (20, 30)-(40, 60)"
        ])]
    );
}

#[test]
fn a_first_point_on_a_middle_a_crossing_or_a_curve_gone_is_a_new_point_there() {
    // Lines (10, 30)-(50, 30) and (33, 10)-(33, 60). A line pressed at the first's middle
    // (30, 30), at their crossing (33, 30), and on the first at (20, 30); the line it names
    // (for the crossing, the second) goes before the release, 30 mm up and right: each from
    // a new point there, with no relation.
    let mut s = empty();
    s.points.push(point("q0000001", 10.0, 30.0));
    s.points.push(point("q0000002", 50.0, 30.0));
    s.points.push(point("q0000003", 33.0, 10.0));
    s.points.push(point("q0000004", 33.0, 60.0));
    s.lines.push(line("n0000001", "q0000001", "q0000002"));
    s.lines.push(line("n0000002", "q0000003", "q0000004"));
    for (press, gone, at) in [
        ([30.1, 30.1], "n0000001", "(30, 30)"),
        ([33.1, 30.1], "n0000002", "(33, 30)"),
        ([20.0, 30.2], "n0000001", "(20, 30)"),
    ] {
        let mut d = Drive::new(ToolKind::Line, s.clone());
        d.press(press[0], press[1]).to(50.0, 50.0);
        removed(&mut d, &[gone]);
        let end = [press[0] + 30.0, press[1] + 40.0];
        d.to(end[0], end[1]).release(end[0], end[1]);
        let made = &d.made[0];
        assert!(made.constraints.is_empty(), "{press:?}: {made:?}");
        assert!(
            d.adds()[0].contains(&format!("point {at}")),
            "{press:?}: {:?}",
            d.adds()
        );
    }
}

// --- Round 2, item 5: each add answered by its token ---------------------------------------

#[test]
fn each_add_is_answered_by_its_token_whatever_the_order() {
    // Three points added, P1 (12, 34), P2 (22, 47), P3 (37, 58) (lined up with nothing),
    // none answered. P2 refused; P3 answered, at revision 3, before P1, at revision 2. A read
    // of revision 2 holding P1 (the core's own) and lagging P3: P2 is forgotten (a click
    // there adds it again), P1 is the read's, and P3 still the tool's (3 is past 2), so a
    // click on either adds nothing.
    let sid = sketch_id();
    let mut tool = SketchTool::new(ToolKind::Point);
    let mut ids = Counter(0);
    let keys = Modifiers::default();
    let mut click = |tool: &mut SketchTool, sketch: &Sketch, revision: u64, at: Xy| {
        let context = Context {
            sketch_id: &sid,
            revision: Revision::from_number(revision).unwrap(),
            sketch,
            tolerance: TOLERANCE,
            automatic_relations: true,
        };
        tool.handle(&context, &mut ids, Event::Press(at), keys);
        tool.handle(&context, &mut ids, Event::Release(at), keys)
    };
    let none = empty();
    let (p1, p2, p3) = ([12.0, 34.0], [22.0, 47.0], [37.0, 58.0]);
    let made: Vec<Outcome> = [p1, p2, p3]
        .into_iter()
        .map(|at| click(&mut tool, &none, 1, at))
        .collect();
    let token = |i: usize| made[i].token.unwrap();
    assert!(token(0) != token(1) && token(1) != token(2));
    tool.refused(token(1));
    tool.added(token(2), Revision::from_number(3).unwrap());
    tool.added(token(0), Revision::from_number(2).unwrap());
    let read = sketch_edit::add(&none, made[0].add.as_ref().unwrap())
        .unwrap()
        .sketch;
    assert!(click(&mut tool, &read, 2, p1).message.is_some());
    assert!(click(&mut tool, &read, 2, p3).message.is_some());
    let again = click(&mut tool, &read, 2, p2);
    assert!(again.add.is_some(), "{again:?}");
    // A read of revision 3 without P3 (removed): forgotten.
    assert!(click(&mut tool, &read, 3, p3).add.is_some());
}

// --- Round 2, item 6: a point woken again is the newest ------------------------------------

#[test]
fn a_point_woken_again_is_the_newest_and_the_oldest_goes() {
    // Points along y = 10 at x = 10, 20, ... 90. Hovered: 1 to 8, then 1 again (now the
    // newest), then 9: the oldest, 2, is forgotten, and 1 kept. A click at (20.3, 50) lines
    // up with nothing; at (10.3, 60), with (10, 10).
    let mut s = empty();
    for i in 1..=9 {
        s.points
            .push(point(&format!("q000000{i}"), f64::from(i) * 10.0, 10.0));
    }
    let hovered = || {
        let mut d = Drive::new(ToolKind::Point, s.clone());
        for i in [1, 2, 3, 4, 5, 6, 7, 8, 1, 9] {
            d.to(f64::from(i) * 10.0 + 0.1, 10.1);
        }
        d
    };
    let mut d = hovered();
    d.click(20.3, 50.0);
    assert_eq!(d.adds(), [sorted(&["point (20.3, 50)"])]);
    let mut d = hovered();
    d.click(10.3, 60.0);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (10, 60)",
            "vertical_points (10, 60) #(10, 10)"
        ])]
    );
}

// --- Round 3 ------------------------------------------------------------------------------

#[test]
fn a_point_hovered_by_several_moves_takes_one_slot() {
    // Nine points at (10 i, 10). Hovered 1 to 8 once each, then 8 three times more: 8 is
    // still one of the eight woken, so 1 is too. A click at (10.3, 60) lines up with
    // (10, 10).
    let mut s = empty();
    for i in 1..=9 {
        s.points
            .push(point(&format!("q000000{i}"), f64::from(i) * 10.0, 10.0));
    }
    let mut d = Drive::new(ToolKind::Point, s);
    for i in [1, 2, 3, 4, 5, 6, 7, 8, 8, 8, 8] {
        d.to(f64::from(i) * 10.0 + 0.1, 10.1);
    }
    d.click(10.3, 60.0);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (10, 60)",
            "vertical_points (10, 60) #(10, 10)"
        ])]
    );
}

/// The `relation`s the preview shows.
fn shown(d: &Drive) -> Vec<Relation> {
    d.tool
        .preview()
        .shown
        .iter()
        .map(|s| s.relation.clone())
        .collect()
}

#[test]
fn a_clicked_circles_shared_centre_gone_is_a_new_point_there() {
    let mut d = Drive::new(ToolKind::Circle, lone());
    d.click(10.0, 10.0);
    removed(&mut d, &["q0000001"]);
    d.click(13.0, 14.0);
    assert_eq!(
        d.adds(),
        [sorted(&["point (10, 10)", "circle (10, 10) r5"])]
    );
}

#[test]
fn a_clicked_rectangles_shared_corner_gone_is_a_new_point_there() {
    let mut d = Drive::new(ToolKind::CornerRectangle, lone());
    d.click(10.0, 10.0);
    removed(&mut d, &["q0000001"]);
    d.click(40.0, 30.0);
    assert_eq!(d.adds(), [rectangle_10_10_40_30()]);
}

#[test]
fn a_clicked_tangent_arcs_base_gone_ends_the_gesture() {
    // Clicked on the line's end (20, 10), which goes with the line: the gesture ends, and
    // the next click, on nothing, starts none (it says there's no end to follow).
    let mut d = Drive::new(ToolKind::TangentArc, one_line());
    d.click(20.0, 10.0);
    assert!(d.tool.busy());
    removed(&mut d, &["n0000001", "q0000002"]);
    d.click(20.0, 20.0);
    assert!(d.adds().is_empty());
    assert_eq!(d.messages, [Message::NoEndToFollow]);
    assert!(!d.tool.busy());
}

#[test]
fn a_pressed_centrepoint_arcs_shared_centre_gone_is_a_new_point_there() {
    // The centre pressed on (10, 10) and dragged; the point goes before the release, which
    // sets the start: from there on the centre is a new point, shown sharing nothing.
    let mut d = Drive::new(ToolKind::CenterpointArc, lone());
    d.press(10.0, 10.0).to(18.0, 16.0);
    removed(&mut d, &["q0000001"]);
    d.release(18.0, 16.0);
    assert!(
        !shown(&d).contains(&Relation::Shared(id("q0000001"))),
        "{:?}",
        shown(&d)
    );
    d.to(13.0, 20.0).click(4.0, 18.0);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (10, 10)",
            "point (18, 16)",
            "point (4, 18)",
            "arc (10, 10) (18, 16)->(4, 18)",
        ])]
    );
}

#[test]
fn a_pressed_three_point_arcs_shared_start_gone_is_a_new_point_there() {
    // The start pressed on (10, 10) and dragged; the point goes before the release, which
    // sets the end: from there on the start is a new point, shown sharing nothing.
    let mut d = Drive::new(ToolKind::ThreePointArc, lone());
    d.press(10.0, 10.0).to(30.0, 10.0);
    removed(&mut d, &["q0000001"]);
    d.release(30.0, 10.0);
    assert!(
        !shown(&d).contains(&Relation::Shared(id("q0000001"))),
        "{:?}",
        shown(&d)
    );
    d.click(20.0, 20.0);
    assert_eq!(
        d.adds(),
        [sorted(&[
            "point (20, 10)",
            "point (10, 10)",
            "point (30, 10)",
            "arc (20, 10) (30, 10)->(10, 10)",
        ])]
    );
}

/// The constraint types a line from the end of the arc about (50, 50), radius 10, from 270°
/// to `to` radians, adds, drawn toward 180.3° (within 2° of the tangent and of −x).
fn leaving_arc_ending_at(to: f64) -> Vec<&'static str> {
    let s = with_arc(empty(), [50.0, 50.0], 10.0, 270.0, to.to_degrees());
    let end = toward([50.0, 50.0], to.to_degrees(), 10.0);
    let mut d = Drive::new(ToolKind::Line, s);
    d.drag(end, toward(end, 180.3, 30.0));
    types(&d)
}

#[test]
fn an_arc_leaving_an_axis_by_a_nanoradian_or_less_gets_both_and_by_a_microradian_the_tangent_alone()
{
    // Past its end at π/2 + ε an arc runs along π + ε: −x, its y component −sin ε. At
    // ε = 1e-12 and 1e-10 that's within 1e-9: tangent and horizontal. At 1e-8 and 1e-6, past
    // it: tangent alone. So the bound is pinned within a factor of ten either way.
    let half = std::f64::consts::FRAC_PI_2;
    for epsilon in [1e-12, 1e-10] {
        assert_eq!(
            leaving_arc_ending_at(half + epsilon),
            ["horizontal", "tangent_at"],
            "{epsilon}"
        );
    }
    for epsilon in [1e-8, 1e-6] {
        assert_eq!(
            leaving_arc_ending_at(half + epsilon),
            ["tangent_at"],
            "{epsilon}"
        );
    }
}

#[test]
fn another_tools_token_answers_nothing() {
    // Tool A adds a point, and tool B one; both tokens would be the first of their tool. A's
    // token given to B: refused, it drops nothing of B's; answered at revision 2, it stamps
    // nothing, so a read of revision 3 without B's point still finds it.
    let sid = sketch_id();
    let none = empty();
    let keys = Modifiers::default();
    let mut ids = Counter(0);
    let mut click = |tool: &mut SketchTool, revision: u64, at: Xy| {
        let context = Context {
            sketch_id: &sid,
            revision: Revision::from_number(revision).unwrap(),
            sketch: &none,
            tolerance: TOLERANCE,
            automatic_relations: true,
        };
        tool.handle(&context, &mut ids, Event::Press(at), keys);
        tool.handle(&context, &mut ids, Event::Release(at), keys)
    };
    let mut a = SketchTool::new(ToolKind::Point);
    let mut b = SketchTool::new(ToolKind::Point);
    let from_a = click(&mut a, 1, [12.0, 34.0]).token.unwrap();
    assert!(click(&mut b, 1, [22.0, 47.0]).add.is_some());
    b.refused(from_a);
    b.added(from_a, Revision::from_number(2).unwrap());
    let again = click(&mut b, 3, [22.0, 47.0]);
    assert_eq!(again.add, None, "{again:?}");
    assert!(matches!(again.message, Some(Message::PointThere(_))));
}
